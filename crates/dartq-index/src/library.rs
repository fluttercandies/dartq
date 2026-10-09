//! The per-library reference index (spec §8 keep list, second face):
//! identifier uses resolved against the file-local scope and the #117
//! [`Namespaces`], the unresolved-names recovery table, and the resolved
//! supertype edges.
//!
//! # What resolves, what stays a name row
//!
//! A bare site resolves through the [`lookup_scoped`] chain's M1 core — the
//! file's own declarations first (private included, locals beat imports),
//! then the library's namespace. A `.`-continuation resolves through the
//! prefix table when the receiver identifier armed a prefix (prefix markers
//! live in the import namespace, so arming is one bare lookup); without an
//! arm it is a member access on an untyped receiver, and the member name is
//! kept qualified in the names table. Ambiguous lookups are also names-table
//! rows: the ambiguity is diagnosed at the use point by #120, and the index
//! records only the residual signal.
//!
//! Resolution runs **after** the parse pass over the sites in source order
//! (Dart top-levels are order-independent, so a use may precede its local
//! declaration; the file's complete [`FileIndex`] must exist first). Local
//! variables and members accessed without qualification land in the names
//! table too — M1 has no local/member scope chain; that residual is the
//! documented under-report posture (spec §12.2), and the names table is
//! exactly where the `undefined_*` recovery reads it.
//!
//! # The #120 seam
//!
//! The `undefined_*` family resolves identifiers as a side effect of its
//! checks; those miss rows belong here, not in a parallel path (spec §8's
//! recovery-surface ruling on ticket #120). [`LibraryIndex::record_unresolved`]
//! appends to an overflow buffer served merged through
//! [`LibraryIndex::unresolved`] — no re-sort, no allocation on the probe
//! path. Rows recorded this way live until the library rebuilds whole, at
//! which point the resolver that produced them regenerates them.
//!
//! The `lookup_scoped` chain order is re-stated here rather than imported:
//! keeping dartq-semantic out of this crate's dependency set (its model is
//! mid-flight on #120, and the reference surface must not move with it).

use dartq_resolution::{LibraryGraph, LibraryId, Lookup, NamespaceEntry, Namespaces};
use dartq_source::{FileId, SourceRange, StringId};

use crate::capture::{Family, FileCapture, FileIndex};

/// The relation kind of a reference or name row — the §8 M1 subset.
/// Constructor-reference specials, named-argument references, and comment
/// references are folded into these per the §8 cut list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RefKind {
    /// A non-invocation, non-value use: a type name, an `extends`/`on`
    /// clause name, a metadata name.
    Referenced = 0,
    /// A value read (a bare identifier expression, not called, not the
    /// target of a simple assignment).
    Read = 1,
    /// A call site, constructor reference, or tear-off-shaped use.
    Invoked = 2,
    /// The target of a plain `=` assignment.
    Written = 3,
    /// The target of a compound assignment (`+=`, `++`-shaped reads are
    /// deliberately *not* folded here — see the capture module's cut list).
    ReadWritten = 4,
}

impl RefKind {
    /// Decodes a raw kind byte written by [`RefKind as u8`]. Private
    /// encoding: the only writer is the capture's own `as u8`.
    #[must_use]
    pub(crate) fn from_u8(raw: u8) -> Self {
        match raw {
            0 => Self::Referenced,
            1 => Self::Read,
            2 => Self::Invoked,
            3 => Self::Written,
            _ => Self::ReadWritten,
        }
    }
}

/// One resolved reference site: the use, its target declaration, and the
/// relation kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RefRow {
    /// The target's declaring file.
    pub target_file: FileId,
    /// The target's lookup-spelling name.
    pub target_name: StringId,
    /// The relation kind.
    pub kind: RefKind,
    /// The use site's span (UTF-16).
    pub range: SourceRange,
    /// The use has an explicit qualifier (`p.x`) and cannot be shadowed by
    /// a local declaration (the official `isQualified` bit, spec §4).
    pub qualified: bool,
}

/// One unresolved name use — the §8 names table row (the recovery surface
/// for the `undefined_*` family and the only reference signal when
/// resolution is partial).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NameRow {
    /// The used name.
    pub name: StringId,
    /// The relation kind the use would have carried.
    pub kind: RefKind,
    /// The use site's span (UTF-16).
    pub range: SourceRange,
    /// The use is qualified (a member access or prefix miss).
    pub qualified: bool,
}

/// The target of a resolved supertype edge: a workspace declaration, or the
/// bare name when the supertype lies outside the analyzed set (`dart:`
/// SDK, unanalyzed dependency) — the official string-keyed fallback (spec
/// §5, `SubtypeIndexElementId`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetRef {
    /// A workspace declaration.
    Decl(FileId, StringId),
    /// Outside the analyzed set; the bare name is the signal.
    Name(StringId),
}

/// One resolved supertype edge: the subtype `(file, name)` and its direct
/// supertype.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubtypeEdge {
    /// The subtype's file.
    pub file: FileId,
    /// The subtype's name.
    pub name: StringId,
    /// The direct supertype.
    pub supertype: TargetRef,
}

/// The reference surface of one library (defining unit plus parts), built
/// whole by [`LibraryBuilder`] and replaced whole on re-analysis (spec §6:
/// no partial index update).
///
/// Invariants (the §5 sorted-column discipline): `refs` ascend by
/// `(target, offset)` with [`Self::runs`] holding each distinct-target
/// run's start — a CSR offsets array over the rows; `names` ascend by
/// `(name, offset)`; `supers` ascend by `(subtype file, name)`. Every probe
/// is a binary search plus an equal-run scan.
#[derive(Debug, Default)]
pub struct LibraryIndex {
    refs: Vec<RefRow>,
    runs: Vec<u32>,
    names: Vec<NameRow>,
    names_overflow: Vec<NameRow>,
    supers: Vec<SubtypeEdge>,
}

impl LibraryIndex {
    /// The reference rows targeting `(file, name)`, ascending by offset.
    #[must_use]
    pub fn references_of(&self, file: FileId, name: StringId) -> Option<&[RefRow]> {
        let key = |row: &RefRow| (row.target_file, row.target_name);
        let start = self
            .runs
            .partition_point(|&run| key(&self.refs[run as usize]) < (file, name));
        let run = *self.runs.get(start)?;
        if key(&self.refs[run as usize]) != (file, name) {
            return None;
        }
        let end = self
            .runs
            .get(start + 1)
            .map_or(self.refs.len(), |&end| end as usize);
        Some(&self.refs[run as usize..end])
    }

    /// The unresolved uses of `name`: built-in rows from the sorted table
    /// plus seam rows appended after the build ([`Self::record_unresolved`]).
    pub fn unresolved(&self, name: StringId) -> impl Iterator<Item = &NameRow> + '_ {
        let lo = self.names.partition_point(|row| row.name < name);
        let hi = self.names.partition_point(|row| row.name <= name);
        self.names[lo..hi].iter().chain(
            self.names_overflow
                .iter()
                .filter(move |row| row.name == name),
        )
    }

    /// The direct supertypes of `(file, name)`.
    #[must_use]
    pub fn supertypes_of(&self, file: FileId, name: StringId) -> Option<&[SubtypeEdge]> {
        let key = |edge: &SubtypeEdge| (edge.file, edge.name);
        let start = self.supers.partition_point(|edge| key(edge) < (file, name));
        let end = self.supers[start..].partition_point(|edge| key(edge) <= (file, name)) + start;
        (start < end).then(|| &self.supers[start..end])
    }

    /// Appends an unresolved-name row after the build — the #120 seam
    /// (`undefined_*` lookup byproducts; see the module docs).
    ///
    /// The row joins [`Self::unresolved`] probes immediately without
    /// re-sorting; a library rebuild replaces the whole index, and the
    /// resolver that produced the byproduct regenerates it.
    pub fn record_unresolved(&mut self, row: NameRow) {
        self.names_overflow.push(row);
    }

    /// Every reference row, ordered by the table's sort (CSR source data).
    #[must_use]
    pub fn refs(&self) -> &[RefRow] {
        &self.refs
    }

    /// The number of resolved reference rows.
    #[must_use]
    pub fn reference_count(&self) -> usize {
        self.refs.len()
    }
}

/// The outcome of a bare-name resolution attempt.
enum BareHit {
    /// A declaration: the current file (locals beat imports) or a namespace
    /// target.
    Decl(FileId),
    /// An import prefix: its name, which arms the next continuation.
    Prefix(StringId),
    /// Not found, or lazily ambiguous — a names-table row either way.
    Miss,
}

/// Builds one library's [`LibraryIndex`] from its files' captures.
///
/// The caller captures every file of the library (defining unit plus
/// parts) and adds them in any order; the namespaces must already exist
/// (the graph's export scopes fold the whole workspace, so cross-library
/// targets resolve exactly like the L1 `LibraryAnalyzed` point — spec §8's
/// build timing).
pub struct LibraryBuilder<'a> {
    lib: LibraryId,
    graph: &'a LibraryGraph,
    namespaces: &'a Namespaces,
    refs: Vec<RefRow>,
    names: Vec<NameRow>,
    supers: Vec<SubtypeEdge>,
}

impl<'a> LibraryBuilder<'a> {
    /// Starts a build for `lib` over the workspace's `graph` and
    /// `namespaces`.
    #[must_use]
    pub fn new(lib: LibraryId, graph: &'a LibraryGraph, namespaces: &'a Namespaces) -> Self {
        Self {
            lib,
            graph,
            namespaces,
            refs: Vec::new(),
            names: Vec::new(),
            supers: Vec::new(),
        }
    }

    /// Resolves and folds one file's raw sites into the tables.
    pub fn add_file(&mut self, capture: &FileCapture) {
        let file = capture.file;
        let sites = &capture.sites;
        let mut armed: Option<StringId> = None;
        for i in 0..sites.len() {
            let name = sites.name(i);
            let kind = sites.kind(i);
            let range = sites.range(i);
            match sites.family(i) {
                Family::Bare => {
                    // A prefix arms only the continuation immediately
                    // following it: any intervening bare site disarms.
                    armed = None;
                    match self.resolve_bare(file, &capture.index, name) {
                        BareHit::Decl(target) => self.refs.push(RefRow {
                            target_file: target,
                            target_name: name,
                            kind,
                            range,
                            qualified: false,
                        }),
                        BareHit::Prefix(prefix) => armed = Some(prefix),
                        BareHit::Miss => self.names.push(NameRow {
                            name,
                            kind,
                            range,
                            qualified: false,
                        }),
                    }
                }
                Family::Continuation => {
                    let hit = armed
                        .take()
                        .map(|prefix| self.namespaces.lookup_prefixed(self.lib, prefix, name));
                    match hit {
                        Some(Lookup::Found(NamespaceEntry::Decl(target))) => {
                            self.refs.push(RefRow {
                                target_file: target,
                                target_name: name,
                                kind,
                                range,
                                qualified: true,
                            });
                        }
                        // No armed prefix (member access on an untyped
                        // receiver), an ambiguous prefix hit, or a miss.
                        _ => self.names.push(NameRow {
                            name,
                            kind,
                            range,
                            qualified: true,
                        }),
                    }
                }
            }
        }
        for edge in capture.index.supers() {
            let container = capture.index.row(edge.container);
            let supertype = match self.resolve_bare(capture.file, &capture.index, edge.name) {
                BareHit::Decl(target) => TargetRef::Decl(target, edge.name),
                BareHit::Prefix(_) | BareHit::Miss => TargetRef::Name(edge.name),
            };
            self.supers.push(SubtypeEdge {
                file: capture.file,
                name: container.name,
                supertype,
            });
        }
    }

    /// The M1 scope chain for a bare name: the file's own declarations
    /// first (library faces only — members and type parameters are not
    /// scope names), then the library's namespace.
    fn resolve_bare(&self, file: FileId, index: &FileIndex, name: StringId) -> BareHit {
        if index.has_library_face(name) {
            return BareHit::Decl(file);
        }
        match self.namespaces.lookup(self.graph, self.lib, name) {
            Lookup::Found(NamespaceEntry::Decl(target)) => BareHit::Decl(target),
            Lookup::Found(NamespaceEntry::Prefix) => BareHit::Prefix(name),
            Lookup::Ambiguous(_) | Lookup::NotFound => BareHit::Miss,
        }
    }

    /// Sorts the tables into their probed order and finishes the index.
    #[must_use]
    pub fn finish(mut self) -> LibraryIndex {
        // Deterministic total orders (spec §12.4: queries merge name-keyed
        // and element-keyed data, so any nondeterminism leaks into output).
        self.refs
            .sort_by_key(|row| (row.target_file, row.target_name, row.range.offset));
        let mut runs = Vec::new();
        let mut prev: Option<(FileId, StringId)> = None;
        for (i, row) in self.refs.iter().enumerate() {
            let key = (row.target_file, row.target_name);
            if prev != Some(key) {
                runs.push(crate::narrow(i));
                prev = Some(key);
            }
        }
        self.names.sort_by_key(|row| (row.name, row.range.offset));
        self.supers.sort_by_key(|edge| (edge.file, edge.name));
        LibraryIndex {
            refs: self.refs,
            runs,
            names: self.names,
            names_overflow: Vec::new(),
            supers: self.supers,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::capture::{DeclKind, capture};
    use crate::{SymbolDecl, SymbolIndex};
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_parser::ParserFeatures;
    use dartq_resolution::{
        Combinators, ImportEdge, LibraryGraph, LibraryGraphBuilder, Namespaces,
    };
    use dartq_scanner::{ScanConfig, TokenTable};
    use dartq_source::{FileTable, StringTable};
    use std::path::Path;

    const SRC_A: &str = "class A {\n  void m() {}\n}\n\nvoid fa() {}\nint top = 0;\n";
    const SRC_B: &str = "import 'a.dart';\nimport 'a.dart' as p;\n\nclass B extends A {\n  void use() {\n    A();\n    fa();\n    p.fa();\n    top = 1;\n    nope;\n  }\n}\n";

    /// The two-file fixture (index-design-spec §8-§10): `a.dart` declares
    /// the surface, `b.dart` imports it twice (bare and prefixed), extends
    /// `A`, invokes across the boundary, writes a top-level, and holds one
    /// unresolvable use.
    pub(crate) struct Fixture {
        pub(crate) strings: &'static StringTable,
        pub(crate) files: FileTable,
        pub(crate) graph: LibraryGraph,
        pub(crate) namespaces: Namespaces,
        pub(crate) lib_a: LibraryId,
        pub(crate) lib_b: LibraryId,
        pub(crate) capture_a: FileCapture,
        pub(crate) capture_b: FileCapture,
    }

    fn capture_one(
        strings: &StringTable,
        files: &mut FileTable,
        path: &str,
        source: &str,
    ) -> FileCapture {
        let file = files.intern(Path::new(path));
        let result = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(result.diagnostics.is_empty(), "{source:?}");
        let tokens: &'static TokenTable = Box::leak(Box::new(result.tokens));
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            LanguageVersion::new(3, 15),
        ));
        capture(file, source, tokens, features, strings)
    }

    pub(crate) fn fixture() -> Fixture {
        let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
        let mut files = FileTable::new();
        let capture_a = capture_one(strings, &mut files, "a.dart", SRC_A);
        let capture_b = capture_one(strings, &mut files, "b.dart", SRC_B);

        let mut builder = LibraryGraphBuilder::new(strings);
        let lib_a = builder.intern("package:t/a.dart");
        let lib_b = builder.intern("package:t/b.dart");
        let file_a = files.try_file_id(Path::new("a.dart")).unwrap();
        let file_b = files.try_file_id(Path::new("b.dart")).unwrap();
        for (lib, file, elements) in [
            (lib_a, file_a, &capture_a.index),
            (lib_b, file_b, &capture_b.index),
        ] {
            for row in elements.decls() {
                if row.kind == DeclKind::TypeParam || row.container != crate::capture::DeclRow::NONE
                {
                    continue; // members and type params are not library faces
                }
                builder.declare(lib, row.name, file);
            }
        }
        builder.import(
            lib_b,
            ImportEdge {
                target: Some(lib_a),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        builder.import(
            lib_b,
            ImportEdge {
                target: Some(lib_a),
                prefix: Some(strings.intern("p")),
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        Fixture {
            strings,
            files,
            graph,
            namespaces,
            lib_a,
            lib_b,
            capture_a,
            capture_b,
        }
    }

    /// The fallthrough arms of the resolution core: a continuation with no
    /// armed prefix (member access on an undeclared receiver), a prefix
    /// armed but the name missing from its scope, a supertype outside the
    /// analyzed set (the `TargetRef::Name` fallback), the absent-reference
    /// probe, and a seam row merging with built-in rows of the same name.
    #[test]
    fn name_row_fallthrough_and_seam_merge() {
        let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
        let mut files = FileTable::new();
        let main = capture_one(
            strings,
            &mut files,
            "m.dart",
            "import 'd.dart' as p;\nclass C extends Missing {}\nvoid main() {\n  x.foo;\n  p.missing();\n}\n",
        );
        let dep = capture_one(strings, &mut files, "d.dart", "void gone() {}\n");

        let mut builder = LibraryGraphBuilder::new(strings);
        let lib_m = builder.intern("package:t/m.dart");
        let lib_d = builder.intern("package:t/d.dart");
        let file_m = files.try_file_id(Path::new("m.dart")).unwrap();
        let file_d = files.try_file_id(Path::new("d.dart")).unwrap();
        for (lib, file, cap) in [(lib_m, file_m, &main.index), (lib_d, file_d, &dep.index)] {
            for row in cap.decls() {
                if row.kind == DeclKind::TypeParam || row.container != crate::capture::DeclRow::NONE
                {
                    continue; // members and type params are not library faces
                }
                builder.declare(lib, row.name, file);
            }
        }
        builder.import(
            lib_m,
            ImportEdge {
                target: Some(lib_d),
                prefix: Some(strings.intern("p")),
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);

        let mut builder = LibraryBuilder::new(lib_m, &graph, &namespaces);
        builder.add_file(&main);
        builder.add_file(&dep);
        let surface = builder.finish();

        // `x.foo;`: the receiver is an unresolvable bare name (unqualified
        // row); `foo` is a member access with no armed prefix (qualified
        // row).
        let x_rows: Vec<&NameRow> = surface.unresolved(strings.intern("x")).collect();
        assert_eq!(x_rows.len(), 1);
        assert_eq!(x_rows[0].kind, RefKind::Read);
        assert!(!x_rows[0].qualified);
        let x_offset = x_rows[0].range.offset;
        let foo: Vec<&NameRow> = surface.unresolved(strings.intern("foo")).collect();
        assert_eq!(foo.len(), 1);
        assert!(foo[0].qualified, "member access with no prefix arm");

        // `p.missing();`: the prefix arms, the name misses its scope — the
        // invocation keeps its kind and gains the qualified bit.
        let missing: Vec<&NameRow> = surface.unresolved(strings.intern("missing")).collect();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].kind, RefKind::Invoked);
        assert!(missing[0].qualified);

        // No bare use resolved, so the reference table stays empty and
        // every probe misses.
        assert_eq!(surface.reference_count(), 0);
        assert!(
            surface
                .references_of(file_d, strings.intern("gone"))
                .is_none()
        );

        // `extends Missing`: the supertype lies outside the analyzed set —
        // the bare-name fallback, not a declaration.
        let supers = surface
            .supertypes_of(file_m, strings.intern("C"))
            .expect("C supertype edge");
        assert_eq!(supers.len(), 1);
        assert_eq!(
            supers[0].supertype,
            TargetRef::Name(strings.intern("Missing"))
        );

        // The seam merge: a byproduct row for a name that already has a
        // built-in row is served after it, from one probe.
        let mut surface = surface;
        surface.record_unresolved(NameRow {
            name: strings.intern("x"),
            kind: RefKind::Read,
            range: SourceRange {
                offset: 9_999,
                length: 1,
            },
            qualified: false,
        });
        let merged: Vec<(u32, bool)> = surface
            .unresolved(strings.intern("x"))
            .map(|row| (row.range.offset, row.qualified))
            .collect();
        assert_eq!(
            merged,
            vec![(x_offset, false), (9_999, false)],
            "built-in rows first, seam rows after"
        );
    }

    /// The two-file fixture through the whole face: cross-file resolution
    /// (bare and prefixed), write tagging, the supertype edge, the
    /// unresolved-names table, the #120 seam, and the workspace query
    /// round-trip.
    #[test]
    fn cross_file_resolution_and_query_round_trip() {
        let fx = fixture();
        let strings = fx.strings;
        let file_a = fx.files.try_file_id(Path::new("a.dart")).unwrap();
        let file_b = fx.files.try_file_id(Path::new("b.dart")).unwrap();

        let mut builder = LibraryBuilder::new(fx.lib_b, &fx.graph, &fx.namespaces);
        builder.add_file(&fx.capture_b);
        let b_surface = builder.finish();
        let mut builder = LibraryBuilder::new(fx.lib_a, &fx.graph, &fx.namespaces);
        builder.add_file(&fx.capture_a);
        let a_surface = builder.finish();

        // Bare cross-file invocation, and the prefixed twin: same target,
        // one unqualified, one qualified — the `isQualified` bit (spec §4).
        let fa_refs = b_surface
            .references_of(file_a, strings.intern("fa"))
            .expect("fa referenced from b");
        assert_eq!(fa_refs.len(), 2);
        assert!(fa_refs[0].range.offset < fa_refs[1].range.offset);
        assert!(fa_refs.iter().all(|row| row.kind == RefKind::Invoked));
        assert_eq!(
            fa_refs.iter().map(|row| row.qualified).collect::<Vec<_>>(),
            vec![false, true]
        );
        // The call span pins to the source (`fa()` line 6 of b).
        let fa_call = SRC_B.find("    fa();").expect("bare call") + 4;
        assert_eq!(fa_refs[0].range.offset as usize, fa_call);
        assert_eq!(fa_refs[0].range.length as usize, 2);

        // Write tagging across the boundary: `top = 1` writes.
        let top_refs = b_surface
            .references_of(file_a, strings.intern("top"))
            .expect("top written from b");
        assert_eq!(top_refs.len(), 1);
        assert_eq!(top_refs[0].kind, RefKind::Written);

        // `A()`: an invocation; `extends A`: a reference — same target.
        let a_refs = b_surface
            .references_of(file_a, strings.intern("A"))
            .expect("A referenced from b");
        assert_eq!(
            a_refs.iter().map(|row| row.kind).collect::<Vec<_>>(),
            vec![RefKind::Referenced, RefKind::Invoked]
        );

        // Unresolved: the member access `nope` (unknown receiver shape →
        // the §8 names table), and nothing else from this fixture's uses.
        let nope = b_surface
            .unresolved(strings.intern("nope"))
            .collect::<Vec<_>>();
        assert_eq!(nope.len(), 1);
        assert_eq!(nope[0].kind, RefKind::Read);
        assert!(!nope[0].qualified);
        assert!(
            b_surface.unresolved(strings.intern("fa")).next().is_none(),
            "resolved uses never land in the names table"
        );

        // Supertype edge: B extends A resolves cross-file.
        let supers = b_surface
            .supertypes_of(file_b, strings.intern("B"))
            .expect("B supertype edge");
        assert_eq!(supers.len(), 1);
        assert_eq!(
            supers[0].supertype,
            TargetRef::Decl(file_a, strings.intern("A"))
        );

        // The #120 seam: a byproduct row joins the probe immediately.
        let mut b_surface = b_surface;
        b_surface.record_unresolved(NameRow {
            name: strings.intern("orphan"),
            kind: RefKind::Read,
            range: SourceRange {
                offset: 42,
                length: 6,
            },
            qualified: false,
        });
        assert_eq!(
            b_surface.unresolved(strings.intern("orphan")).count(),
            1,
            "seam rows are served merged"
        );

        // Workspace round trip: query symbol by name → files+kinds+spans;
        // the reverse face loops libraries (spec §9 fan-out).
        let mut index = SymbolIndex::default();
        index.insert_file(fx.capture_a.file, fx.capture_a.index);
        index.insert_file(fx.capture_b.file, fx.capture_b.index);
        index.insert_library(fx.lib_a, a_surface);
        index.insert_library(fx.lib_b, b_surface);
        let hits: Vec<SymbolDecl<'_>> = index.symbols(strings.intern("A")).collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].file, file_a);
        assert_eq!(hits[0].decl.kind, DeclKind::Class);
        let back: Vec<&RefRow> = index.references(file_a, strings.intern("fa")).collect();
        assert_eq!(
            back.len(),
            2,
            "both b-side call sites, one per library scan"
        );
        assert!(index.library(fx.lib_b).is_some());
        assert!(index.file(file_a).is_some());
    }

    /// The local hop of a bare lookup matches library-face rows only: a
    /// member sharing a name with an imported top-level must not shadow it,
    /// a type parameter is no scope name either, and a genuine top-level
    /// declaration still beats the namespace.
    #[test]
    fn local_hop_matches_library_faces_only() {
        let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
        let mut files = FileTable::new();
        let dep = capture_one(strings, &mut files, "d.dart", "void fa() {}\n");
        let main = capture_one(
            strings,
            &mut files,
            "m.dart",
            "import 'd.dart';\nclass K {\n  void fa() {}\n}\nclass K2<Q> {\n  Q? f;\n}\nvoid fa2() {}\nvoid main() {\n  fa();\n  fa2();\n}\n",
        );

        let mut builder = LibraryGraphBuilder::new(strings);
        let lib_m = builder.intern("package:t/m.dart");
        let lib_d = builder.intern("package:t/d.dart");
        let file_m = files.try_file_id(Path::new("m.dart")).unwrap();
        let file_d = files.try_file_id(Path::new("d.dart")).unwrap();
        for (lib, file, cap) in [(lib_m, file_m, &main.index), (lib_d, file_d, &dep.index)] {
            for row in cap.decls() {
                if row.kind == DeclKind::TypeParam || row.container != crate::capture::DeclRow::NONE
                {
                    continue; // members and type params are not library faces
                }
                builder.declare(lib, row.name, file);
            }
        }
        builder.import(
            lib_m,
            ImportEdge {
                target: Some(lib_d),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);

        let mut builder = LibraryBuilder::new(lib_m, &graph, &namespaces);
        builder.add_file(&main);
        builder.add_file(&dep);
        let surface = builder.finish();

        // `fa();`: declared only as a member locally — the member row must
        // not satisfy the local hop, so the namespace hit on d.dart wins.
        let fa = surface
            .references_of(file_d, strings.intern("fa"))
            .expect("fa resolves through the namespace");
        assert_eq!(fa.len(), 1);
        assert_eq!(fa[0].kind, RefKind::Invoked);
        assert!(
            surface
                .references_of(file_m, strings.intern("fa"))
                .is_none(),
            "the member row must not shadow the import"
        );

        // `fa2();`: a genuine top-level declaration still beats imports.
        let fa2 = surface
            .references_of(file_m, strings.intern("fa2"))
            .expect("local top-level wins");
        assert_eq!(fa2.len(), 1);

        // `Q? f;`: a type parameter is not a library face — the use falls
        // through to the namespace, misses, and lands in the names table.
        assert!(
            surface.references_of(file_m, strings.intern("Q")).is_none(),
            "a type parameter must not satisfy the local hop"
        );
        let q = surface.unresolved(strings.intern("Q")).collect::<Vec<_>>();
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].kind, RefKind::Referenced);
    }

    /// A member of an unnamed extension is not a library face (the
    /// `unnamed_extension_member` bit): a same-named import wins the bare
    /// use instead of the file-local hop shadowing it.
    #[test]
    fn unnamed_extension_member_loses_to_import() {
        let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
        let mut files = FileTable::new();
        let dep = capture_one(strings, &mut files, "d.dart", "void fa() {}\n");
        let main = capture_one(
            strings,
            &mut files,
            "m.dart",
            "import 'd.dart';\nextension on String {\n  int get fa => 1;\n}\nvoid main() {\n  fa;\n}\n",
        );

        let mut builder = LibraryGraphBuilder::new(strings);
        let lib_m = builder.intern("package:t/m.dart");
        let lib_d = builder.intern("package:t/d.dart");
        let file_m = files.try_file_id(Path::new("m.dart")).unwrap();
        let file_d = files.try_file_id(Path::new("d.dart")).unwrap();
        for (lib, file, cap) in [(lib_m, file_m, &main.index), (lib_d, file_d, &dep.index)] {
            for row in cap.decls() {
                if row.is_library_face() {
                    builder.declare(lib, row.name, file);
                }
            }
        }
        builder.import(
            lib_m,
            ImportEdge {
                target: Some(lib_d),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);

        let mut builder = LibraryBuilder::new(lib_m, &graph, &namespaces);
        builder.add_file(&main);
        builder.add_file(&dep);
        let surface = builder.finish();

        // The bare use falls through the local hop (the member row is not a
        // face) and resolves through the namespace to d.dart.
        let fa = surface
            .references_of(file_d, strings.intern("fa"))
            .expect("the import wins the bare use");
        assert_eq!(fa.len(), 1);
        assert_eq!(fa[0].kind, RefKind::Read);
        assert!(
            surface
                .references_of(file_m, strings.intern("fa"))
                .is_none(),
            "the unnamed-extension member must not shadow the import"
        );
    }

    /// A prefix arms only the continuation immediately following it: an
    /// intervening bare site disarms, so the next continuation is a member
    /// access even when its name exists in the prefix's scope.
    #[test]
    fn prefix_arm_does_not_leak_across_bare_sites() {
        let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
        let mut files = FileTable::new();
        let dep = capture_one(
            strings,
            &mut files,
            "d.dart",
            "void gone() {}\nvoid w() {}\n",
        );
        let main = capture_one(
            strings,
            &mut files,
            "m.dart",
            "import 'd.dart' as p;\nvoid main() {\n  p.gone;\n  z.w;\n}\n",
        );

        let mut builder = LibraryGraphBuilder::new(strings);
        let lib_m = builder.intern("package:t/m.dart");
        let lib_d = builder.intern("package:t/d.dart");
        let file_m = files.try_file_id(Path::new("m.dart")).unwrap();
        let file_d = files.try_file_id(Path::new("d.dart")).unwrap();
        for (lib, file, cap) in [(lib_m, file_m, &main.index), (lib_d, file_d, &dep.index)] {
            for row in cap.decls() {
                if row.kind == DeclKind::TypeParam || row.container != crate::capture::DeclRow::NONE
                {
                    continue; // members and type params are not library faces
                }
                builder.declare(lib, row.name, file);
            }
        }
        builder.import(
            lib_m,
            ImportEdge {
                target: Some(lib_d),
                prefix: Some(strings.intern("p")),
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);

        let mut builder = LibraryBuilder::new(lib_m, &graph, &namespaces);
        builder.add_file(&main);
        builder.add_file(&dep);
        let surface = builder.finish();

        // `p.gone;`: the prefix arms and the continuation resolves.
        let gone = surface
            .references_of(file_d, strings.intern("gone"))
            .expect("prefixed hit");
        assert_eq!(gone.len(), 1);
        assert!(gone[0].qualified);

        // `z.w;`: the bare miss `z` disarmed the prefix, so `w` is a member
        // access even though d.dart declares it.
        assert!(
            surface.references_of(file_d, strings.intern("w")).is_none(),
            "the disarmed prefix must not resolve the later continuation"
        );
        let w = surface.unresolved(strings.intern("w")).collect::<Vec<_>>();
        assert_eq!(w.len(), 1);
        assert!(w[0].qualified, "member access shape");
        let z = surface.unresolved(strings.intern("z")).collect::<Vec<_>>();
        assert_eq!(z.len(), 1);
        assert!(!z[0].qualified);
    }

    /// Whole-unit eviction: removing a file's or a library's surface drops
    /// it from every probe, and removing an absent id is a no-op.
    #[test]
    fn removal_drops_surfaces() {
        let fx = fixture();
        let mut builder = LibraryBuilder::new(fx.lib_a, &fx.graph, &fx.namespaces);
        builder.add_file(&fx.capture_a);
        let surface = builder.finish();
        let mut index = SymbolIndex::default();
        index.insert_file(fx.capture_a.file, fx.capture_a.index);
        index.insert_library(fx.lib_a, surface);

        assert!(index.remove_library(fx.lib_a).is_some());
        assert!(index.library(fx.lib_a).is_none());
        assert!(index.remove_library(fx.lib_a).is_none(), "absent id: no-op");
        assert!(index.remove_file(fx.capture_a.file).is_some());
        assert!(index.file(fx.capture_a.file).is_none());
    }
}
