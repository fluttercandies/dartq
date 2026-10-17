//! The postings directories behind the workspace query faces: name-keyed
//! inverted lists that turn the `query symbol` pair from workspace scans
//! into match-sized lookups.
//!
//! # Shape
//!
//! One directory per query face, both keyed by the interned name:
//! declarations map a name to the files declaring it, references map a
//! name to the libraries holding resolved uses of it. The lists hold unit
//! ids only, ascending — the deterministic iteration order the
//! `BTreeMap`-scan faces produced. The rows themselves are *not*
//! materialized here: every contributing unit already carries its rows in
//! a name-sorted table ([`FileIndex::by_name`]'s equal-name run,
//! [`LibraryIndex::references_of`](crate::library::LibraryIndex::references_of)'s
//! CSR run), so a posting list is a
//! directory over those runs and a query serves rows through the owning
//! unit's existing binary search. One hash probe plus one binary search
//! per hit; no per-query allocation, no duplicated spans or kinds.
//!
//! This is the CSR posture of the adjudicated design with the row
//! duplication elided: a global `(file, span, kind)` row array would copy
//! every declaration and reference row a second time (~12-16 B/row) for
//! zero latency gain at exact-name semantics — the rows must be read from
//! the per-unit tables either way to answer with `&DeclRow`/`&RefRow`.
//! Directory memory instead: 4 B per (name, unit) pair plus one hash
//! slot (~33 B: key + `Vec` header + control byte) per distinct posted
//! name — an order of magnitude under the materialized shape, charged
//! honestly by [`SymbolIndex::postings_bytes`](crate::SymbolIndex::postings_bytes).
//!
//! # Update law
//!
//! The crate's law is whole-unit replacement, and the
//! directories follow it literally: re-indexing a unit unlists it from
//! the names of its old surface and lists it under the names of the new
//! one. Listing keeps each unit vec sorted (binary search + insert), so
//! queries stay order-deterministic without a re-sort pass. The rejected
//! alternative — tombstoned rows in a global arena plus a pending buffer
//! and threshold compaction — needs three more moving parts and branches
//! on every query to serve the same result sets; its pending buffer would
//! be this very map-of-lists anyway.
//!
//! Cost shape: a unit replacement touches only the names its surface
//! declares or targets; a cold build is one append-ordered insert per
//! (name, unit) pair. Shared names pay a sorted-insert memmove per
//! contributing unit — the term the bench adjudicates against the
//! tombstone shape.

use rustc_hash::FxHashMap;

use dartq_source::StringId;

/// One query face's postings directory: interned name → the units whose
/// surface carries rows for it, ascending by unit id.
///
/// Public for the engine boundary: the engine mounts one directory per
/// face over its own dense-id arenas (captures, library surfaces) and
/// applies this crate's whole-unit law at the same mutation points — see
/// the module docs. The rows stay in the owning units; a directory holds
/// unit ids only.
#[derive(Debug)]
pub struct Postings<U> {
    dir: FxHashMap<StringId, Vec<U>>,
}

impl<U> Default for Postings<U> {
    fn default() -> Self {
        Self {
            dir: FxHashMap::default(),
        }
    }
}

impl<U: Copy + Ord> Postings<U> {
    /// Lists `unit` under `name`. Idempotent per (name, unit); the vec
    /// stays ascending.
    pub fn list(&mut self, name: StringId, unit: U) {
        let units = self.dir.entry(name).or_default();
        match units.binary_search(&unit) {
            Ok(_) => {}
            Err(at) => units.insert(at, unit),
        }
    }

    /// Drops `unit`'s listing under `name`; a name left with no units
    /// leaves the map, so the directory never grows past the live name set.
    pub fn unlist(&mut self, name: StringId, unit: U) {
        if let Some(units) = self.dir.get_mut(&name) {
            if let Ok(at) = units.binary_search(&unit) {
                units.remove(at);
            }
            if units.is_empty() {
                self.dir.remove(&name);
            }
        }
    }

    /// The units carrying rows for `name`, ascending — empty for a name
    /// nothing lists.
    #[must_use]
    pub fn units(&self, name: StringId) -> &[U] {
        self.dir.get(&name).map_or(&[], Vec::as_slice)
    }

    /// Whether nothing is listed — the update law's no-dead-entries
    /// invariant, asserted by the tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.dir.is_empty()
    }

    /// The directory's memory: hash slots (data + control byte) plus the
    /// unit lists at capacity — the bench's total-bytes input.
    #[must_use]
    pub fn bytes(&self) -> usize {
        let slot = std::mem::size_of::<(StringId, Vec<U>)>() + 1;
        self.dir.capacity() * slot
            + self
                .dir
                .values()
                .map(|units| units.capacity() * std::mem::size_of::<U>())
                .sum::<usize>()
    }
}

/// The distinct declaration names of a file surface, ascending — the rows
/// sort by `(name, offset)`, so adjacency dedupe is exact.
pub fn distinct_decl_names(index: &crate::FileIndex) -> impl Iterator<Item = StringId> + '_ {
    let mut prev: Option<StringId> = None;
    index.decls().iter().filter_map(move |row| {
        if prev == Some(row.name) {
            None
        } else {
            prev = Some(row.name);
            Some(row.name)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::DeclKind;
    use crate::library::tests::{SRC_B, capture_one, fixture};
    use crate::{RefKind, SymbolIndex};
    use dartq_source::{FileId, FileTable, StringTable};
    use std::path::Path;

    /// `(file, kind, span)` of every declaration hit — the projection the
    /// equivalence checks compare.
    fn symbol_hits(index: &SymbolIndex, name: StringId) -> Vec<(FileId, DeclKind, u32, u32)> {
        index
            .symbols(name)
            .map(|hit| {
                (
                    hit.file,
                    hit.decl.kind,
                    hit.decl.range.offset,
                    hit.decl.range.length,
                )
            })
            .collect()
    }

    /// The old scan face, rebuilt over the public accessors: every indexed
    /// file ascending, its equal-name run in offset order.
    fn scanned_symbols(index: &SymbolIndex, name: StringId) -> Vec<(FileId, DeclKind, u32, u32)> {
        index
            .files()
            .flat_map(|(file, surface)| {
                surface
                    .by_name(name)
                    .into_iter()
                    .flatten()
                    .map(move |row| (file, row.kind, row.range.offset, row.range.length))
            })
            .collect()
    }

    /// The old reference scan face, rebuilt over the public accessors:
    /// every indexed library ascending, its target run in use order.
    fn scanned_references(
        index: &SymbolIndex,
        file: FileId,
        name: StringId,
    ) -> Vec<(FileId, RefKind, u32)> {
        index
            .libraries()
            .filter_map(|(_, surface)| surface.references_of(file, name))
            .flatten()
            .map(|row| (row.file, row.kind, row.range.offset))
            .collect()
    }

    fn reference_hits(
        index: &SymbolIndex,
        file: FileId,
        name: StringId,
    ) -> Vec<(FileId, RefKind, u32)> {
        index
            .references(file, name)
            .map(|row| (row.file, row.kind, row.range.offset))
            .collect()
    }

    /// Both faces answer exactly what the `BTreeMap` scans answered, name
    /// for name — the fixture workspace, probe set covering hits, misses,
    /// and both files.
    #[test]
    fn queries_match_the_workspace_scans() {
        let fx = fixture();
        let file_a = fx.files.try_file_id(Path::new("a.dart")).unwrap();
        let file_b = fx.files.try_file_id(Path::new("b.dart")).unwrap();
        let strings = fx.strings;

        let mut builder = crate::LibraryBuilder::new(fx.lib_a, &fx.graph, &fx.namespaces);
        builder.add_file(&fx.capture_a);
        let a_surface = builder.finish();
        let mut builder = crate::LibraryBuilder::new(fx.lib_b, &fx.graph, &fx.namespaces);
        builder.add_file(&fx.capture_b);
        let b_surface = builder.finish();

        let mut index = SymbolIndex::default();
        index.insert_file(fx.capture_a.file, fx.capture_a.index);
        index.insert_file(fx.capture_b.file, fx.capture_b.index);
        index.insert_library(fx.lib_a, a_surface);
        index.insert_library(fx.lib_b, b_surface);

        for name in ["A", "B", "m", "fa", "top", "use", "nope", "zz_absent"] {
            let id = strings.intern(name);
            assert_eq!(
                symbol_hits(&index, id),
                scanned_symbols(&index, id),
                "symbols({name})"
            );
        }
        for &file in &[file_a, file_b] {
            for name in ["A", "B", "fa", "top", "m", "nope", "zz_absent"] {
                let id = strings.intern(name);
                assert_eq!(
                    reference_hits(&index, file, id),
                    scanned_references(&index, file, id),
                    "references({file:?}, {name})"
                );
            }
        }
    }

    /// Declaration listing order follows file id, not insertion order: a
    /// workspace inserted in reverse still answers ascending, and the
    /// equivalence holds across every declared name of a many-file build.
    #[test]
    fn decl_order_survives_shuffled_insertion() {
        let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
        let mut files = FileTable::new();
        let count = 16;
        let mut captures = Vec::with_capacity(count);
        for i in 0..count {
            let source = format!(
                "class C{i} {{ void m{i}() {{}} }}\nvoid top{i}() {{}}\nvoid shared() {{}}\n"
            );
            captures.push(capture_one(
                strings,
                &mut files,
                &format!("o{i}.dart"),
                &source,
            ));
        }
        let expected: Vec<FileId> = captures.iter().map(|capture| capture.file).collect();
        let mut index = SymbolIndex::default();
        for capture in captures.into_iter().rev() {
            index.insert_file(capture.file, capture.index);
        }

        let shared = strings.intern("shared");
        let hits: Vec<FileId> = index.symbols(shared).map(|hit| hit.file).collect();
        assert_eq!(hits, expected, "ascending by file id, insertion reversed");

        let probes: Vec<String> = (0..count)
            .flat_map(|i| [format!("C{i}"), format!("m{i}"), format!("top{i}")])
            .collect();
        for name in &probes {
            let id = strings.intern(name);
            assert_eq!(
                symbol_hits(&index, id),
                scanned_symbols(&index, id),
                "symbols({name})"
            );
        }
        assert!(symbol_hits(&index, strings.intern("zz_absent")).is_empty());
    }

    /// Reference listing order follows library id, not insertion order:
    /// two importers inserted in reverse still answer ascending.
    #[test]
    fn reference_order_survives_shuffled_insertion() {
        let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
        let mut files = FileTable::new();
        let decl = capture_one(strings, &mut files, "x.dart", "void t() {}\n");
        let first = capture_one(
            strings,
            &mut files,
            "y.dart",
            "import 'x.dart';\nvoid main() {\n  t();\n}\n",
        );
        let second = capture_one(
            strings,
            &mut files,
            "z.dart",
            "import 'x.dart';\nvoid main() {\n  t();\n}\n",
        );
        let file_x = decl.file;
        let file_y = first.file;
        let file_z = second.file;

        let mut builder = dartq_resolution::LibraryGraphBuilder::new(strings);
        let lib_x = builder.intern("package:t/x.dart");
        let lib_y = builder.intern("package:t/y.dart");
        let lib_z = builder.intern("package:t/z.dart");
        for row in decl
            .index
            .decls()
            .iter()
            .filter(|row| row.is_library_face())
        {
            builder.declare(lib_x, row.name, file_x);
        }
        for (lib, target) in [(lib_y, lib_x), (lib_z, lib_x)] {
            builder.import(
                lib,
                dartq_resolution::ImportEdge {
                    target: Some(target),
                    prefix: None,
                    deferred: false,
                    combinators: dartq_resolution::Combinators::unfiltered(),
                },
            );
        }
        let graph = builder.finish();
        let namespaces = dartq_resolution::Namespaces::build(&graph);

        let surface_of = |lib, capture: &crate::FileCapture| {
            let mut builder = crate::LibraryBuilder::new(lib, &graph, &namespaces);
            builder.add_file(capture);
            builder.finish()
        };
        let z_surface = surface_of(lib_z, &second);
        let y_surface = surface_of(lib_y, &first);
        let x_surface = surface_of(lib_x, &decl);

        let mut index = SymbolIndex::default();
        // Reverse library insertion order: z before y.
        index.insert_library(lib_z, z_surface);
        index.insert_library(lib_y, y_surface);
        index.insert_library(lib_x, x_surface);
        index.insert_file(file_x, decl.index);
        index.insert_file(file_z, second.index);
        index.insert_file(file_y, first.index);

        let t = strings.intern("t");
        let hits: Vec<FileId> = index.references(file_x, t).map(|row| row.file).collect();
        assert_eq!(
            hits,
            vec![file_y, file_z],
            "ascending by library id, insertion reversed"
        );
        assert_eq!(
            reference_hits(&index, file_x, t),
            scanned_references(&index, file_x, t)
        );
    }

    /// Whole-unit replacement: reindexing one file detaches exactly its
    /// old postings and attaches the new surface's; restoring the original
    /// surface restores the original answers. Removal drops the unit from
    /// every listing; a library's replacement moves only its own rows.
    #[test]
    fn reindex_replaces_one_unit_contribution() {
        let fx = fixture();
        let file_a = fx.files.try_file_id(Path::new("a.dart")).unwrap();
        let file_b = fx.files.try_file_id(Path::new("b.dart")).unwrap();
        let strings = fx.strings;
        let fa = strings.intern("fa");
        let top = strings.intern("top");

        let mut builder = crate::LibraryBuilder::new(fx.lib_a, &fx.graph, &fx.namespaces);
        builder.add_file(&fx.capture_a);
        let a_surface = builder.finish();
        let mut builder = crate::LibraryBuilder::new(fx.lib_b, &fx.graph, &fx.namespaces);
        builder.add_file(&fx.capture_b);
        let b_surface = builder.finish();
        let mut index = SymbolIndex::default();
        index.insert_file(fx.capture_a.file, fx.capture_a.index);
        index.insert_file(fx.capture_b.file, fx.capture_b.index);
        index.insert_library(fx.lib_a, a_surface);
        index.insert_library(fx.lib_b, b_surface);
        assert_eq!(reference_hits(&index, file_a, fa).len(), 2);
        assert_eq!(symbol_hits(&index, fa).len(), 1);
        assert!(index.postings_bytes() > 0);

        // b.dart rewritten without the `fa` uses: the rebuilt surface drops
        // both rows and the reference listing follows.
        let mut files = fx.files;
        let rewritten = "import 'a.dart';\nimport 'a.dart' as p;\n\nvoid main() {\n  top = 2;\n}\n";
        let fresh = capture_one(strings, &mut files, "b.dart", rewritten);
        assert_eq!(fresh.file, file_b, "same path, same id");
        let mut builder = crate::LibraryBuilder::new(fx.lib_b, &fx.graph, &fx.namespaces);
        builder.add_file(&fresh);
        index.insert_file(file_b, fresh.index);
        index.insert_library(fx.lib_b, builder.finish());
        assert!(
            reference_hits(&index, file_a, fa).is_empty(),
            "the rewritten file no longer references fa"
        );
        assert_eq!(
            reference_hits(&index, file_a, top).len(),
            1,
            "the surviving `top = 2` write keeps its row"
        );
        assert_eq!(symbol_hits(&index, fa).len(), 1, "declarations untouched");

        // The original surface restores the original answers.
        let restored = capture_one(strings, &mut files, "b.dart", SRC_B);
        let mut builder = crate::LibraryBuilder::new(fx.lib_b, &fx.graph, &fx.namespaces);
        builder.add_file(&restored);
        index.insert_file(file_b, restored.index);
        index.insert_library(fx.lib_b, builder.finish());
        assert_eq!(reference_hits(&index, file_a, fa).len(), 2);

        // Eviction: the declaring file out of the set empties its
        // declarations, and the referencing library out of the set empties
        // its reference rows — while the other face keeps its answers (the
        // scan faces' exact semantics).
        index.remove_file(file_a).expect("a indexed");
        assert!(symbol_hits(&index, fa).is_empty());
        assert_eq!(
            reference_hits(&index, file_a, fa).len(),
            2,
            "reference rows do not read the declaration surface"
        );
        index.remove_library(fx.lib_b).expect("b indexed");
        assert!(reference_hits(&index, file_a, fa).is_empty());
        // The restored b.dart is still indexed and rightly keeps its own
        // declaration postings; evicting the last file unit detaches them
        // too — no dead entries survive full eviction.
        index.remove_file(file_b).expect("b indexed");
        assert!(index.decl_dir.is_empty() && index.ref_dir.is_empty());
    }

    /// Duplicate listing of one unit stays one listing, and a name with no
    /// units never appears.
    #[test]
    fn listings_are_idempotent_and_self_cleaning() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let a = files.intern(Path::new("p1.dart"));
        let b = files.intern(Path::new("p2.dart"));
        let mut dir: Postings<FileId> = Postings::default();
        let name = strings.intern("x");
        dir.list(name, b);
        dir.list(name, b);
        dir.list(name, a);
        assert_eq!(
            dir.units(name),
            [a, b].as_slice(),
            "ascending, no duplicate"
        );
        dir.unlist(name, a);
        assert_eq!(dir.units(name), [b].as_slice());
        dir.unlist(name, b);
        assert!(dir.is_empty(), "the emptied name leaves the map");
        assert!(dir.units(name).is_empty());
        dir.unlist(name, b);
        assert!(dir.is_empty(), "unlisting an absent unit is a no-op");
    }
}
