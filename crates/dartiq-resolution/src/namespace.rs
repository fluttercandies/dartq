//! Namespace construction and lookup over the library graph
//! (`docs/facts/resolution-design-spec.md` §1/§5, module `namespace.rs`) —
//! the dartiq face of the analyzer's `NamespaceBuilder`
//! (`analyzer-8.4.1 lib/src/dart/resolver/scope.dart:140-283`) applied over
//! [`crate::library_graph`]'s export scopes.
//!
//! # What is precomputed
//!
//! Everything. [`Namespaces::build`] eagerly materializes, per library:
//!
//! - the **import namespace**: the merged face of all un-prefixed imports,
//!   each filtered by its directive combinators at build time (§1 — the
//!   oracle filters during construction, `NamespaceBuilder
//!   .createImportNamespaceForDirective` + `_applyCombinators`,
//!   scope.dart:159-170/227-244), and
//! - one **prefix table** per distinct `as` prefix, merged across all
//!   imports sharing it (CFE `PrefixBuilder` reuse,
//!   `source_compilation_unit.dart:1507-1540`); the prefix name itself is a
//!   marker entry in the import namespace (the prefix is injected into the
//!   import scope as a name, `import.dart:69-79`), so `p.x` is two hash
//!   lookups — the analyzer's `PrefixedNamespace` lookup-time composition
//!   (scope.dart:287-328) with interned ids instead of string splicing.
//!
//! Lookup is therefore pure `FxHashMap` hits on `u32` keys: no string work,
//! no allocation.
//!
//! # Ambiguity
//!
//! Import merging follows the lazy rule (Dart 18.1; `front_end`
//! `computeAmbiguousDeclarationForImport`, spec §1): the **same** target
//! arriving through several paths is one entry; **different** targets under
//! one name make the entry ambiguous with all candidates kept — the
//! diagnosis surfaces at the use point (`ambiguous_import`), never at build
//! time, and this layer only exposes it through [`Lookup::Ambiguous`].
//! A prefix marker conflicting with an imported top-level name is the same
//! kind of ambiguity.
//!
//! Lookup order is the M1 form of the `front_end` scope chain (spec §1): local
//! declarations first (they shadow imports), then the import namespace. The
//! deeper chain (type-parameter scopes, part-local scopes) is the semantic
//! layer's.
//!
//! # Deviations from the spec sketch (§5), pinned here
//!
//! - `NameEntry::Ambiguous` keeps **all** candidates, not `[DefSource; 2]` —
//!   the oracle's `ambiguous_import` message template lists every defining
//!   library (`diagnostic.g.dart:396`), and truncating would lose them.
//! - Targets are declaring files, not `DefId`s: this layer has no element
//!   table, and `(file, lookup-key name)` identifies a top-level declaration
//!   for the semantic layer to resolve.
//! - External (`dart:`) namespaces are opaque at this layer — the SDK's
//!   names are grafted downstream (analyzer fills them from summaries). The
//!   `front_end` non-`dart:`-over-`dart:` merge priority (scope.dart:177-181)
//!   becomes relevant only once SDK namespaces exist and is deferred with
//!   that graft.
//! - The spec's `Namespace`/`Scope` type split is folded into one index
//!   ([`Namespaces`]) with the chain order fixed (local → imports): M1 has a
//!   single two-level chain, and a parent-delegation abstraction would model
//!   nothing that varies yet.

use dartiq_source::{FileId, StringId};
use rustc_hash::FxHashMap;
use std::collections::hash_map;

use crate::library_graph::{LibraryGraph, LibraryId};

/// One lookup target in a namespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamespaceEntry {
    /// A top-level declaration: its declaring file (the looked-up name is the
    /// declaration's name, and a file belongs to one library, so the pair is
    /// the declaration's identity — see [`crate::library_graph::ExportEntry`]).
    Decl(FileId),
    /// An import prefix: the index into the owning library's prefix tables.
    Prefix(PrefixId),
}

/// A namespace entry under one name: a single target, or the full candidate
/// set of a lazily-ambiguous name (Dart 18.1 — importing a name from several
/// libraries is only an error when the name is used).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameEntry {
    /// One target.
    Single(NamespaceEntry),
    /// Two or more distinct targets; all candidates, in arrival (import
    /// source) order.
    Ambiguous(Box<[NamespaceEntry]>),
}

/// An import prefix of one library: the index into that library's prefix
/// tables (built by [`Namespaces::build`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PrefixId(u32);

impl PrefixId {
    /// The underlying integer, meaningful within the owning library.
    #[must_use]
    pub const fn index(self) -> u32 {
        self.0
    }
}

/// The outcome of a namespace lookup — the shape the semantic layer's
/// `undefined_*` family consumes: a hit, a lazily-ambiguous name (reported at
/// the use point as `ambiguous_import`), or a miss.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lookup<'a> {
    /// The name resolves to one target.
    Found(NamespaceEntry),
    /// The name is ambiguous between two or more distinct targets.
    Ambiguous(&'a [NamespaceEntry]),
    /// The name is not in the namespace at all.
    NotFound,
}

/// The namespaces of a graph's libraries: per library, the merged import
/// namespace and one table per distinct import prefix.
#[derive(Debug)]
pub struct Namespaces {
    libs: Vec<LibraryNamespaces>,
}

#[derive(Debug)]
struct LibraryNamespaces {
    /// Bare-name entries (un-prefixed imports, plus prefix markers).
    names: FxHashMap<StringId, NameEntry>,
    /// One table per distinct prefix name, merged across the imports
    /// sharing it; indexed by [`PrefixId`].
    prefixes: Vec<FxHashMap<StringId, NameEntry>>,
}

impl Namespaces {
    /// Builds the namespaces of every library in `graph`: imports merged in
    /// directive source order, combinators applied per import, prefixes
    /// merged across imports and registered as markers. Sized entirely by
    /// the graph; nothing grows afterwards.
    #[must_use]
    pub fn build(graph: &LibraryGraph) -> Self {
        let libs = (0..graph.len())
            .map(|index| build_library(graph, LibraryId::from_index(index)))
            .collect();
        Self { libs }
    }

    /// Bare-name lookup in `lib`'s scope chain: local declarations first
    /// (they shadow imports — `front_end` `CompilationUnitScope` consults the
    /// library namespace before the import namespace, spec §1), then the
    /// merged import namespace.
    ///
    /// # Panics
    ///
    /// Panics when `lib` is not a [`LibraryId`] of the graph `self` was built
    /// from — a caller bug (ids only come from that graph).
    #[must_use]
    pub fn lookup(&self, graph: &LibraryGraph, lib: LibraryId, name: StringId) -> Lookup<'_> {
        if let Some(file) = graph.local(lib, name) {
            return Lookup::Found(NamespaceEntry::Decl(file));
        }
        to_lookup(self.libs[lib.index() as usize].names.get(&name))
    }

    /// `prefix.name` lookup in `lib`: resolves `prefix` through the import
    /// namespace's marker entry, then the prefix's merged table.
    ///
    /// Returns [`Lookup::NotFound`] when `prefix` is not unambiguously a
    /// prefix of `lib` — unknown, or a name shared with an imported top-level
    /// declaration (the ambiguity is visible through
    /// [`Self::lookup`]; `p.x` through a non-prefix is not a resolution hit).
    ///
    /// # Panics
    ///
    /// Same contract as [`Self::lookup`].
    #[must_use]
    pub fn lookup_prefixed(&self, lib: LibraryId, prefix: StringId, name: StringId) -> Lookup<'_> {
        let lib_ns = &self.libs[lib.index() as usize];
        match lib_ns.names.get(&prefix) {
            Some(NameEntry::Single(NamespaceEntry::Prefix(id))) => {
                to_lookup(lib_ns.prefixes[id.index() as usize].get(&name))
            }
            _ => Lookup::NotFound,
        }
    }
}

/// Builds one library's namespaces over `graph`'s import edges (§1: imports
/// merge in directive source order; combinator filtering is build-time).
fn build_library(graph: &LibraryGraph, lib: LibraryId) -> LibraryNamespaces {
    let mut names: FxHashMap<StringId, NameEntry> = FxHashMap::default();
    let mut prefixes: Vec<FxHashMap<StringId, NameEntry>> = Vec::new();
    let mut prefix_ids: FxHashMap<StringId, PrefixId> = FxHashMap::default();
    for edge in graph.import_edges(lib) {
        let Some(target) = edge.target else {
            continue; // external/missing/non-library: contributes nothing
        };
        let mut table: Option<usize> = None;
        if let Some(prefix) = edge.prefix {
            let id = if let Some(&id) = prefix_ids.get(&prefix) {
                id
            } else {
                let id = PrefixId(u32::try_from(prefixes.len()).expect("prefix count fits u32"));
                prefixes.push(FxHashMap::default());
                prefix_ids.insert(prefix, id);
                id
            };
            merge(&mut names, prefix, NamespaceEntry::Prefix(id));
            table = Some(id.index() as usize);
        }
        for (name, entry) in graph.export_scope(target) {
            if !edge.combinators.allows(name) {
                continue;
            }
            let entry = NamespaceEntry::Decl(entry.file);
            match table {
                Some(index) => merge(&mut prefixes[index], name, entry),
                None => merge(&mut names, name, entry),
            }
        }
    }
    LibraryNamespaces { names, prefixes }
}

/// Import-merge conflict rule (Dart 18.1; `front_end`
/// `computeAmbiguousDeclarationForImport`, spec §1): the same target through
/// several paths is one entry; distinct targets make the name ambiguous with
/// every candidate kept, in arrival order. Never replaces an existing
/// candidate — the first arrival anchors the order.
fn merge(map: &mut FxHashMap<StringId, NameEntry>, name: StringId, entry: NamespaceEntry) {
    match map.entry(name) {
        hash_map::Entry::Vacant(slot) => {
            slot.insert(NameEntry::Single(entry));
        }
        hash_map::Entry::Occupied(slot) => {
            let grown = match slot.get() {
                NameEntry::Single(existing) => (*existing != entry)
                    .then(|| NameEntry::Ambiguous(Box::from([*existing, entry]))),
                NameEntry::Ambiguous(candidates) => (!candidates.contains(&entry)).then(|| {
                    let mut grown = candidates.to_vec();
                    grown.push(entry);
                    NameEntry::Ambiguous(grown.into_boxed_slice())
                }),
            };
            if let Some(grown) = grown {
                *slot.into_mut() = grown;
            }
        }
    }
}

/// The shared `Option<&NameEntry>` → [`Lookup`] projection.
fn to_lookup(entry: Option<&NameEntry>) -> Lookup<'_> {
    match entry {
        None => Lookup::NotFound,
        Some(NameEntry::Single(target)) => Lookup::Found(*target),
        Some(NameEntry::Ambiguous(candidates)) => Lookup::Ambiguous(candidates),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_graph::{CombinatorClause, Combinators, ImportEdge, LibraryGraphBuilder};
    use dartiq_source::{FileTable, StringTable};
    use std::path::Path;

    /// Shared per-test context. The string table is leaked for `'static` so
    /// the builder can hold it alongside the rig (every `StringTable` method
    /// takes `&self`).
    struct Rig {
        strings: &'static StringTable,
        files: FileTable,
        builder: LibraryGraphBuilder<'static>,
    }

    impl Rig {
        fn new() -> Self {
            let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
            let builder = LibraryGraphBuilder::new(strings);
            Self {
                strings,
                files: FileTable::new(),
                builder,
            }
        }

        fn file(&mut self, name: &str) -> FileId {
            self.files.intern(Path::new(name))
        }

        fn declare(&mut self, lib: LibraryId, name: &str, file: FileId) {
            self.builder.declare(lib, self.strings.intern(name), file);
        }

        fn import(
            &mut self,
            lib: LibraryId,
            target: Option<LibraryId>,
            prefix: Option<&str>,
            combinators: Combinators,
        ) {
            let prefix = prefix.map(|p| self.strings.intern(p));
            self.builder.import(
                lib,
                ImportEdge {
                    target,
                    prefix,
                    deferred: false,
                    combinators,
                },
            );
        }

        fn settle(self) -> (&'static StringTable, LibraryGraph, Namespaces) {
            let strings = self.strings;
            let graph = self.builder.finish();
            let namespaces = Namespaces::build(&graph);
            (strings, graph, namespaces)
        }
    }

    /// The import namespace is the imported library's export face filtered by
    /// the directive combinators at build time (`NamespaceBuilder
    /// .createImportNamespaceForDirective` + `_applyCombinators`,
    /// resolver/scope.dart:159-170/227-244; combinator timing per spec §1).
    #[test]
    fn import_combinators_filter_at_build() {
        let mut rig = Rig::new();
        let target = rig.builder.intern("package:t/t.dart");
        let consumer = rig.builder.intern("package:c/c.dart");
        let t_file = rig.file("t.dart");
        rig.declare(target, "alpha", t_file);
        rig.declare(target, "beta", t_file);
        let strings = rig.strings;
        let show_alpha = Combinators::new(
            [CombinatorClause::Show(Box::from([strings.intern("alpha")]))],
            strings,
        );
        rig.import(consumer, Some(target), None, show_alpha);
        let (strings, graph, namespaces) = rig.settle();

        assert_eq!(
            namespaces.lookup(&graph, consumer, strings.intern("alpha")),
            Lookup::Found(NamespaceEntry::Decl(t_file))
        );
        assert_eq!(
            namespaces.lookup(&graph, consumer, strings.intern("beta")),
            Lookup::NotFound,
            "filtered at build time, not at lookup time"
        );
    }

    /// Local declarations shadow imported names: the library namespace is
    /// consulted before the import namespace (`front_end` scope chain, spec
    /// §1 — `CompilationUnitScope` over the import scope).
    #[test]
    fn locals_shadow_imports() {
        let mut rig = Rig::new();
        let target = rig.builder.intern("package:t/t.dart");
        let consumer = rig.builder.intern("package:c/c.dart");
        let t_file = rig.file("t.dart");
        let c_file = rig.file("c.dart");
        rig.declare(target, "shared", t_file);
        rig.declare(consumer, "shared", c_file);
        rig.import(consumer, Some(target), None, Combinators::unfiltered());
        let (strings, graph, namespaces) = rig.settle();

        assert_eq!(
            namespaces.lookup(&graph, consumer, strings.intern("shared")),
            Lookup::Found(NamespaceEntry::Decl(c_file)),
            "the local declaration wins"
        );
    }

    /// Two imports bringing distinct declarations of one name make it
    /// ambiguous with both candidates kept, in import source order (Dart 18.1
    /// laziness: the diagnosis is the use-point `ambiguous_import`, never a
    /// namespace-build failure).
    #[test]
    fn distinct_import_targets_are_ambiguous() {
        let mut rig = Rig::new();
        let l1 = rig.builder.intern("package:l1/l1.dart");
        let l2 = rig.builder.intern("package:l2/l2.dart");
        let consumer = rig.builder.intern("package:c/c.dart");
        let f1 = rig.file("l1.dart");
        let f2 = rig.file("l2.dart");
        rig.declare(l1, "clash", f1);
        rig.declare(l2, "clash", f2);
        rig.import(consumer, Some(l1), None, Combinators::unfiltered());
        rig.import(consumer, Some(l2), None, Combinators::unfiltered());
        let (strings, graph, namespaces) = rig.settle();

        assert_eq!(
            namespaces.lookup(&graph, consumer, strings.intern("clash")),
            Lookup::Ambiguous(&[NamespaceEntry::Decl(f1), NamespaceEntry::Decl(f2)]),
        );
    }

    /// The same declaration through two imports is one entry, not an
    /// ambiguity (same-reference dedupe, export.dart:111-116): the library
    /// imported twice resolves cleanly.
    #[test]
    fn same_target_through_two_imports_is_single() {
        let mut rig = Rig::new();
        let target = rig.builder.intern("package:t/t.dart");
        let consumer = rig.builder.intern("package:c/c.dart");
        let t_file = rig.file("t.dart");
        rig.declare(target, "name", t_file);
        rig.import(consumer, Some(target), None, Combinators::unfiltered());
        rig.import(consumer, Some(target), None, Combinators::unfiltered());
        let (strings, graph, namespaces) = rig.settle();

        assert_eq!(
            namespaces.lookup(&graph, consumer, strings.intern("name")),
            Lookup::Found(NamespaceEntry::Decl(t_file))
        );
    }

    /// A three-way clash keeps every candidate (`ambiguous_import`'s message
    /// template lists all defining libraries, diagnostic.g.dart:396 — the
    /// sketch's two-slot form would lose them).
    #[test]
    fn three_way_ambiguity_keeps_all_candidates() {
        let mut rig = Rig::new();
        let libs = [
            rig.builder.intern("package:l1/l1.dart"),
            rig.builder.intern("package:l2/l2.dart"),
            rig.builder.intern("package:l3/l3.dart"),
        ];
        let consumer = rig.builder.intern("package:c/c.dart");
        let files = [
            rig.file("l1.dart"),
            rig.file("l2.dart"),
            rig.file("l3.dart"),
        ];
        for (lib, file) in libs.iter().zip(&files) {
            rig.declare(*lib, "clash", *file);
            rig.import(consumer, Some(*lib), None, Combinators::unfiltered());
        }
        let (strings, graph, namespaces) = rig.settle();

        assert_eq!(
            namespaces.lookup(&graph, consumer, strings.intern("clash")),
            Lookup::Ambiguous(&[
                NamespaceEntry::Decl(files[0]),
                NamespaceEntry::Decl(files[1]),
                NamespaceEntry::Decl(files[2]),
            ]),
        );
    }

    /// `import as p` registers the prefix as a marker in the import
    /// namespace, and same-prefix imports merge into one table (CFE
    /// `PrefixBuilder` reuse, source_compilation_unit.dart:1507-1540;
    /// `PrefixedNamespace` composition, scope.dart:287-328).
    #[test]
    fn prefixes_merge_across_imports() {
        let mut rig = Rig::new();
        let t1 = rig.builder.intern("package:t1/t1.dart");
        let t2 = rig.builder.intern("package:t2/t2.dart");
        let consumer = rig.builder.intern("package:c/c.dart");
        let f1 = rig.file("t1.dart");
        let f2 = rig.file("t2.dart");
        rig.declare(t1, "alpha", f1);
        rig.declare(t2, "beta", f2);
        rig.import(consumer, Some(t1), Some("p"), Combinators::unfiltered());
        rig.import(consumer, Some(t2), Some("p"), Combinators::unfiltered());
        let (strings, graph, namespaces) = rig.settle();

        assert_eq!(
            namespaces.lookup(&graph, consumer, strings.intern("p")),
            Lookup::Found(NamespaceEntry::Prefix(PrefixId(0))),
            "one marker for both same-prefix imports"
        );
        assert_eq!(
            namespaces.lookup_prefixed(consumer, strings.intern("p"), strings.intern("alpha")),
            Lookup::Found(NamespaceEntry::Decl(f1))
        );
        assert_eq!(
            namespaces.lookup_prefixed(consumer, strings.intern("p"), strings.intern("beta")),
            Lookup::Found(NamespaceEntry::Decl(f2))
        );
        assert_eq!(
            namespaces.lookup_prefixed(consumer, strings.intern("p"), strings.intern("gamma")),
            Lookup::NotFound
        );
    }

    /// A prefix name shared with an imported top-level declaration is
    /// ambiguous (the prefix is injected into the import namespace as a name,
    /// import.dart:69-79; merge rules per spec §1), and `p.x` through it is
    /// no resolution hit.
    #[test]
    fn prefix_marker_conflicting_with_imported_name_is_ambiguous() {
        let mut rig = Rig::new();
        let l1 = rig.builder.intern("package:l1/l1.dart");
        let t2 = rig.builder.intern("package:t2/t2.dart");
        let consumer = rig.builder.intern("package:c/c.dart");
        let f1 = rig.file("l1.dart");
        rig.declare(l1, "p", f1);
        rig.import(consumer, Some(l1), None, Combinators::unfiltered());
        rig.import(consumer, Some(t2), Some("p"), Combinators::unfiltered());
        let (strings, graph, namespaces) = rig.settle();

        assert_eq!(
            namespaces.lookup(&graph, consumer, strings.intern("p")),
            Lookup::Ambiguous(&[
                NamespaceEntry::Decl(f1),
                NamespaceEntry::Prefix(PrefixId(0))
            ]),
        );
        assert_eq!(
            namespaces.lookup_prefixed(consumer, strings.intern("p"), strings.intern("x")),
            Lookup::NotFound,
            "`p` is not unambiguously a prefix"
        );
    }

    /// Imports resolving to nothing contribute no names and register no
    /// prefix — the oracle skips directives whose imported library is null
    /// (imports_verifier.dart:63-70).
    #[test]
    fn missing_imports_contribute_nothing() {
        let mut rig = Rig::new();
        let consumer = rig.builder.intern("package:c/c.dart");
        rig.import(consumer, None, None, Combinators::unfiltered());
        rig.import(consumer, None, Some("p"), Combinators::unfiltered());
        let (strings, graph, namespaces) = rig.settle();

        assert_eq!(
            namespaces.lookup(&graph, consumer, strings.intern("anything")),
            Lookup::NotFound
        );
        assert_eq!(
            namespaces.lookup(&graph, consumer, strings.intern("p")),
            Lookup::NotFound,
            "no prefix marker without a target"
        );
        assert_eq!(
            namespaces.lookup_prefixed(consumer, strings.intern("p"), strings.intern("x")),
            Lookup::NotFound
        );
    }

    /// `p.x` where `p` resolves to a declaration is a plain miss here; the
    /// misuse is the semantic layer's diagnosis.
    #[test]
    fn prefixed_lookup_through_declaration_misses() {
        let mut rig = Rig::new();
        let l1 = rig.builder.intern("package:l1/l1.dart");
        let consumer = rig.builder.intern("package:c/c.dart");
        let f1 = rig.file("l1.dart");
        rig.declare(l1, "p", f1);
        rig.declare(l1, "x", f1);
        rig.import(consumer, Some(l1), None, Combinators::unfiltered());
        let (strings, graph, namespaces) = rig.settle();

        assert_eq!(
            namespaces.lookup(&graph, consumer, strings.intern("p")),
            Lookup::Found(NamespaceEntry::Decl(f1))
        );
        assert_eq!(
            namespaces.lookup_prefixed(consumer, strings.intern("p"), strings.intern("x")),
            Lookup::NotFound
        );
    }
}
