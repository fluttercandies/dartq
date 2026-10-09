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
//!   imports sharing it, keyed by prefix name — the analyzer's
//!   `_prefixElements` map (`LibraryFragmentScope._`,
//!   scope.dart:348-358). Prefixes live *outside* the combined import
//!   namespace: `_lookupCombined` consults `_prefixElements` first and
//!   returns the prefix unconditionally, before the import namespace
//!   (scope.dart:455-471), and `p.x` always goes through the prefix's
//!   table (`PrefixScope`, scope.dart:352-357) — a prefix name never
//!   merges into, or conflicts with, imported names.
//!
//! Lookup is therefore pure `FxHashMap` hits on interned keys: no string
//! work, no allocation.
//!
//! # Ambiguity
//!
//! Import merging follows the lazy rule (Dart 18.1; `front_end`
//! `computeAmbiguousDeclarationForImport`, spec §1): the **same** target
//! arriving through several paths is one entry; **different** targets under
//! one name make the entry ambiguous with all candidates kept — the
//! diagnosis surfaces at the use point (`ambiguous_import`), never at build
//! time, and this layer only exposes it through [`Lookup::Ambiguous`].
//!
//! Lookup order is the oracle's scope chain (`LibraryFragmentScope.lookup`,
//! scope.dart:418-432): local declarations first (they shadow imports),
//! then prefixes (the `_prefixElements` branch of `_lookupCombined`), then
//! the merged import namespace. The deeper chain (type-parameter scopes,
//! part-local scopes) is the semantic layer's.
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
    /// An import prefix: its merged table is the owning library's prefix
    /// entry under the looked-up name itself (the oracle's
    /// `_prefixElements[id]`, scope.dart:458-460).
    Prefix,
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
/// namespace and the prefix tables keyed by prefix name.
#[derive(Debug)]
pub struct Namespaces {
    libs: Vec<LibraryNamespaces>,
}

#[derive(Debug)]
struct LibraryNamespaces {
    /// Bare-name entries of the merged import namespace (un-prefixed
    /// imports). Prefixes are deliberately not here — the oracle keeps them
    /// in a separate `_prefixElements` map (scope.dart:348-358).
    names: FxHashMap<StringId, NameEntry>,
    /// Prefix name → its merged table (across all imports sharing the
    /// prefix), the [`Prefix`] face of [`NamespaceEntry`].
    prefixes: FxHashMap<StringId, FxHashMap<StringId, NameEntry>>,
}

impl Namespaces {
    /// Builds the namespaces of every library in `graph`: imports merged in
    /// directive source order, combinators applied per import, prefixes
    /// merged across imports into per-name tables. Sized entirely by the
    /// graph; nothing grows afterwards.
    #[must_use]
    pub fn build(graph: &LibraryGraph) -> Self {
        let libs = (0..graph.len())
            .map(|index| build_library(graph, LibraryId::from_index(index)))
            .collect();
        Self { libs }
    }

    /// Bare-name lookup in `lib`'s scope chain (`LibraryFragmentScope.lookup`,
    /// scope.dart:418-432): local declarations first (they shadow imports),
    /// then a registered prefix — the `_prefixElements` branch of
    /// `_lookupCombined` returns the prefix unconditionally, before the
    /// import namespace (scope.dart:455-471) — then the merged import
    /// namespace.
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
        let lib_ns = &self.libs[lib.index() as usize];
        if lib_ns.prefixes.contains_key(&name) {
            return Lookup::Found(NamespaceEntry::Prefix);
        }
        to_lookup(lib_ns.names.get(&name))
    }

    /// `prefix.name` lookup in `lib`: the prefix's merged table, whenever the
    /// prefix is registered — independent of any same-named imported
    /// declaration (`p.x` goes through `PrefixScope`, scope.dart:352-357,
    /// regardless of bare-name collisions).
    ///
    /// Returns [`Lookup::NotFound`] when `prefix` is not a registered prefix
    /// of `lib`, or registers no `name` (a `p.x` through a non-prefix is not
    /// a resolution hit; the misuse is the semantic layer's diagnosis).
    ///
    /// # Panics
    ///
    /// Same contract as [`Self::lookup`].
    #[must_use]
    pub fn lookup_prefixed(&self, lib: LibraryId, prefix: StringId, name: StringId) -> Lookup<'_> {
        let lib_ns = &self.libs[lib.index() as usize];
        to_lookup(
            lib_ns
                .prefixes
                .get(&prefix)
                .and_then(|table| table.get(&name)),
        )
    }
}

/// Builds one library's namespaces over `graph`'s import edges (§1: imports
/// merge in directive source order; combinator filtering is build-time).
fn build_library(graph: &LibraryGraph, lib: LibraryId) -> LibraryNamespaces {
    let mut names: FxHashMap<StringId, NameEntry> = FxHashMap::default();
    let mut prefixes: FxHashMap<StringId, FxHashMap<StringId, NameEntry>> = FxHashMap::default();
    for edge in graph.import_edges(lib) {
        let Some(target) = edge.target else {
            continue; // external/missing/non-library: contributes nothing
        };
        // `Option<&mut _>` is not `Copy`: reborrow through `as_deref_mut` so
        // the table survives every iteration of the name loop.
        let mut table: Option<&mut FxHashMap<StringId, NameEntry>> = edge
            .prefix
            .map(|prefix| prefixes.entry(prefix).or_default());
        for (name, entry) in graph.export_scope(target) {
            if !edge.combinators.allows(name) {
                continue;
            }
            let entry = NamespaceEntry::Decl(entry.file);
            match table.as_deref_mut() {
                Some(table) => merge(table, name, entry),
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

    /// `import as p` registers the prefix in its own name-keyed table, and
    /// same-prefix imports merge into one table (the oracle's
    /// `_prefixElements`, scope.dart:348-358; `p.x` through `PrefixScope`,
    /// scope.dart:352-357).
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
            Lookup::Found(NamespaceEntry::Prefix),
            "one prefix entry for both same-prefix imports"
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

    /// A prefix never merges into the combined import namespace: the oracle
    /// keeps prefixes in the separate `_prefixElements` map, which
    /// `_lookupCombined` consults first and hits unconditionally
    /// (scope.dart:455-471). So bare `p` resolves to the prefix even when an
    /// import also brings a top-level `p`, and `p.x` resolves through the
    /// prefix's table (`PrefixScope`, scope.dart:352-357) regardless.
    #[test]
    fn prefix_wins_over_imported_top_level_name() {
        let mut rig = Rig::new();
        let l1 = rig.builder.intern("package:l1/l1.dart");
        let l2 = rig.builder.intern("package:l2/l2.dart");
        let consumer = rig.builder.intern("package:c/c.dart");
        let f1 = rig.file("l1.dart");
        let f2 = rig.file("l2.dart");
        rig.declare(l1, "p", f1);
        rig.declare(l2, "x", f2);
        rig.import(consumer, Some(l1), None, Combinators::unfiltered());
        rig.import(consumer, Some(l2), Some("p"), Combinators::unfiltered());
        let (strings, graph, namespaces) = rig.settle();

        assert_eq!(
            namespaces.lookup(&graph, consumer, strings.intern("p")),
            Lookup::Found(NamespaceEntry::Prefix),
            "the prefix wins unconditionally, no Decl/Prefix ambiguity"
        );
        assert_eq!(
            namespaces.lookup(&graph, consumer, strings.intern("x")),
            Lookup::NotFound,
            "l2 is imported only `as p`: `x` is not in the bare namespace"
        );
        assert_eq!(
            namespaces.lookup_prefixed(consumer, strings.intern("p"), strings.intern("x")),
            Lookup::Found(NamespaceEntry::Decl(f2)),
            "`p.x` resolves through the prefix table"
        );
        assert_eq!(
            namespaces.lookup_prefixed(consumer, strings.intern("p"), strings.intern("y")),
            Lookup::NotFound
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
            "no prefix entry without a target"
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
