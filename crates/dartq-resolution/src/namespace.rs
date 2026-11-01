//! Namespace lookup over the library graph — the query-time namespace
//! combinator
//! — the dartq face of the analyzer's `NamespaceBuilder`
//! (`analyzer-8.4.1 lib/src/dart/resolver/scope.dart:140-283`) composed over
//! [`crate::library_graph`]'s export scopes at lookup time.
//!
//! # What is precomputed
//!
//! Only the compact import tables — never merged namespaces (#1068). The
//! previous shape eagerly materialized, per library, the merged import
//! namespace and per-prefix tables; at the memory charter's red line (10⁵
//! libraries in 0.5–8 GB) that duplication dominates — every consumer of
//! `dart:core` held its own map over the same thousand-plus names. This
//! shape stores one 16-byte row per import directive in flat arrays
//! ([`Namespaces`]): the row carries the `as` prefix id, the resolved
//! target, and a range over the directive's combinator clauses, while two
//! small index arrays mark where each library's rows start and where each
//! writing unit's run starts. A lookup composes the namespace on the fly —
//! walk the applicable rows in directive source order, filter each through
//! its clauses, and probe the target's export scope in the graph. Memory is
//! O(import directives), bounded by the input; there are no per-library maps
//! and no string work on the hot path (rows are interned ids; clause name
//! sets arrive sorted and setter-expanded from
//! [`crate::library_graph::Combinators::build`], so filtering is binary
//! search over `u32`s).
//!
//! The oracle filters during construction (`NamespaceBuilder
//!.createImportNamespaceForDirective` + `_applyCombinators`,
//! scope.dart:159-170/227-244); composing per probe at query time applies
//! the identical per-import filter with identical per-import timing, so the
//! observable result is the same without holding the merged product.
//!
//! # Ambiguity
//!
//! Import merging follows the lazy rule (Dart 18.1): the **same** target
//! arriving through several paths is one entry; **different** targets under
//! one name make the entry ambiguous with all candidates kept — the
//! diagnosis surfaces at the use point (`ambiguous_import`), never at build
//! time. One exception, the oracle's SDK preference: a non-SDK declaration
//! hides a same-named SDK one (`scope.dart:811-832`) — a package spelling
//! and the `dart:` import of the same name never conflict, the package wins
//! in either directive order. Because the composition walks imports in
//! source order on every probe, the full candidate set is discovered per
//! lookup and returned **owned** ([`Lookup::Ambiguous`]) — an allocation is
//! paid only on the ambiguous (error) path; hits and misses allocate
//! nothing.
//!
//! # The combined import scope (enhanced parts)
//!
//! A Dart file's import scope is its own: "each Dart file … defines a
//! combined import scope which extends the scope chain of the parent file's
//! combined import scope with its own imports and import prefixes … each
//! scope able to shadow names of its enclosing scope" (the `parts-with-imports`
//! rule; upstream `CombinedScopeForFile` / `LibraryFragmentScope` chains every
//! fragment over its parent). The rows are therefore regrouped by the unit
//! that wrote them ([`crate::library_graph::LibraryGraph::import_runs`]) and
//! a use-site lookup walks that chain
//! ([`Namespaces::lookup_in_file`]): the file's own imports, then its parent
//! part's, up to the library file, then the library-wide rows (the implicit
//! `dart:core` prelude). A name imported by one part resolves in that part and
//! its sub-parts only — never in a sibling part or the library file.
//!
//! The chain is exact shadowing, not merging: the first scope that carries
//! the name decides. The library-level entry points ([`Namespaces::lookup`],
//! [`Namespaces::lookup_prefixed`]) start at the defining unit, which is the
//! whole scope of a single-file library and of the classic part shape (a
//! classic part writes no directives, so its combined scope *is* its
//! parent's).
//!
//! # Lookup order
//!
//! Lookup order is the oracle's scope chain (`LibraryFragmentScope.lookup`,
//! scope.dart:419-433): the name-coupled locals layer first (a local face
//! under either spelling of a base name shadows imports for both), then —
//! per scope of the combined import scope chain — prefixes (the
//! `_prefixElements` branch of `_lookupCombined` returns the prefix
//! unconditionally, before the import namespace — scope.dart:455-471), then
//! the un-prefixed imports in first-come-wins per-import composition. Bare
//! `_` never hits the prefix branch: with `wildcard_variables` (Dart 3.7+
//! default-on; pinned here) `_shouldTryPrefixElement` skips it
//! (scope.dart:486-494). The deeper chain (type-parameter scopes, part-local
//! scopes) is the semantic layer's.
//!
//! # Why no merge cache
//!
//! Direction (c) of #1068 — a bounded LRU of merged tables for hot
//! libraries — was considered and rejected: a library's composition walk is
//! bounded by its own import count (rows are contiguous 16-byte records,
//! typically a few dozen per library), so the walk is already cache-local
//! with no unbounded growth anywhere. A cache would add invalidation and a
//! capacity knob without a measured need; revisit only if the serial-phase
//! benchmark shows walk latency above budget.
//!
//! # Deviations from the spec sketch, pinned here
//!
//! - Targets are declaring files, not `DefId`s: this layer has no element
//!   table, and `(file, lookup-key name)` identifies a top-level declaration
//!   for the semantic layer to resolve.
//! - The SDK-vs-package merge priority is pinned here: when one name arrives
//!   from two distinct declarations, a non-SDK declaration hides an SDK one
//!   regardless of arrival order and no ambiguity is recorded — the oracle's
//!   `PrefixScope._merge` preference (`element/scope.dart:811-832`,
//!   `_isSdkElement` at `:860`: the *declaring* library is in the SDK). Only
//!   same-sidedness collisions are ambiguous. Sidedness reads the declaring
//!   file's library URI ([`LibraryGraph::library_of_file`] +
//!   [`LibraryGraph::uri`]), so an SDK name re-exported through a package
//!   still counts as SDK; the check runs only on genuine distinct-target
//!   collisions, never on the hit/miss fast path.
//! - The spec's `Namespace`/`Scope` type split is folded into one index
//!   ([`Namespaces`]) with the chain order fixed (locals → prefixes →
//!   imports): M1 has a single two-level chain, and a parent-delegation
//!   abstraction would model nothing that varies yet.

use dartq_source::{FileId, StringId};

use crate::library_graph::{CombinatorClause, LibraryGraph, LibraryId, LocalLookup};

/// Row sentinel: the import directive carries no `as` prefix. Ids are
/// sequential `u32`s issued by the interners, so the sentinel is reachable
/// only with 2³²−1 distinct interned names — not a real workspace.
const NO_PREFIX: u32 = u32::MAX;

/// Row sentinel: the directive's URI resolved to no library of the analysis
/// set (`dart:`, missing, or not a library) — the row still registers its
/// prefix but contributes no names. Same reachability argument as
/// [`NO_PREFIX`].
const NO_TARGET: u32 = u32::MAX;

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

/// The outcome of a namespace lookup — the shape the semantic layer's
/// `undefined_*` family consumes: a hit, a lazily-ambiguous name (reported at
/// the use point as `ambiguous_import`), or a miss. The ambiguous variant is
/// owned because the composition walk materializes the candidate set per
/// probe; it is the only allocating outcome and only on the error path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    /// The name resolves to one target.
    Found(NamespaceEntry),
    /// The name is ambiguous between two or more distinct targets; all
    /// candidates, in arrival (import source) order.
    Ambiguous(Box<[NamespaceEntry]>),
    /// The name is not in the namespace at all.
    NotFound,
}

/// One compact per-directive import row: everything namespace lookup needs
/// from a resolved `import`, flattened out of the graph edge (whose
/// [`crate::library_graph::Combinators`] also carries diagnostic faces this
/// layer never reads). 16 bytes; one per directive, in source order.
#[derive(Clone, Copy, Debug)]
struct ImportRow {
    /// The `as` prefix name id, or [`NO_PREFIX`]. Comparison-only: lookups
    /// match raw ids, the id is never resolved back to a name.
    prefix: u32,
    /// The resolved target library's id, or [`NO_TARGET`].
    target: u32,
    /// The directive's combinator clauses as a range into
    /// [`Namespaces::clauses`]. An empty range is the unfiltered directive —
    /// the common case, decided with no work at all.
    clauses: (u32, u32),
}

/// One combinator clause of an import row: the kind and a sorted,
/// deduplicated, setter-expanded name-id range into
/// [`Namespaces::filter_names`] — binary-search membership, exactly the
/// stored face of [`crate::library_graph::CombinatorClause`].
#[derive(Clone, Copy, Debug)]
struct FilterClause {
    /// `true` for `hide` (drop the listed names), `false` for `show` (keep
    /// only the listed names).
    hide: bool,
    /// The clause's name ids as a range into [`Namespaces::filter_names`].
    names: (u32, u32),
}

/// The namespace combinator over a graph's libraries: compact per-library
/// import tables (CSR — contiguous rows grouped by [`LibraryId`] with an
/// offset index) composed into lookups at query time.
///
/// Sized entirely by the graph's import directives at
/// [`Namespaces::build`]; nothing grows afterwards. Steady-state bytes are
/// reportable via [`Namespaces::memory_bytes`] (the memory charter's counted
/// metric).
#[derive(Debug)]
pub struct Namespaces {
    /// All import rows, grouped by library in [`LibraryId`] order — and,
    /// within a library, by the unit that wrote them: the library-wide rows
    /// first, then one run per writing unit in ascending file order
    /// ([`LibraryGraph::import_runs`]). This is the composition's iteration
    /// order.
    rows: Vec<ImportRow>,
    /// Per-library offsets into `rows`: library `i` occupies
    /// `rows[offs[i]..offs[i+1]]`; `len == graph.len + 1`.
    offs: Vec<u32>,
    /// Per-library offsets into `runs`; `len == graph.len + 1`.
    run_offs: Vec<u32>,
    /// The per-unit runs of the libraries, flattened: `(file, first row)`,
    /// ascending by file within a library, each run ending where the next
    /// starts or at the library's row end.
    runs: Vec<(FileId, u32)>,
    /// All combinator clauses, grouped by row in row order.
    clauses: Vec<FilterClause>,
    /// All clause name ids (sorted, deduplicated, setter-expanded within
    /// each clause), grouped by clause.
    filter_names: Vec<u32>,
}

impl Namespaces {
    /// Flattens `graph`'s import directives into the compact row tables.
    /// O(total directives + total clause names); no export scope is read and
    /// no merged table is built — composition happens per lookup.
    ///
    /// # Panics
    ///
    /// Panics when the directive, clause, or clause-name count exceeds
    /// [`u32::MAX`] — a program bug at any realistic scale.
    #[must_use]
    pub fn build(graph: &LibraryGraph) -> Self {
        let mut rows: Vec<ImportRow> = Vec::new();
        let mut offs: Vec<u32> = Vec::with_capacity(graph.len() + 1);
        let mut run_offs: Vec<u32> = Vec::with_capacity(graph.len() + 1);
        let mut runs: Vec<(FileId, u32)> = Vec::new();
        let mut clauses: Vec<FilterClause> = Vec::new();
        let mut filter_names: Vec<u32> = Vec::new();
        offs.push(0);
        run_offs.push(0);
        for index in 0..graph.len() {
            // Positional walk: `offs` is indexed by LibraryId (the
            // graph's library iteration order is contractually
            // unspecified, only `0..len` addressing is).
            let lib = LibraryId::from_index(index);
            let base = u32::try_from(rows.len()).expect("more than u32::MAX import directives");
            for edge in graph.import_edges(lib) {
                // The prefix registers unconditionally, target resolved or
                // not: `_buildLibraryImportPrefixes` (element.dart:7529-7536)
                // sweeps every import's prefix with no resolution gating,
                // and only the prefix's NAME table stays empty when the URI
                // carries no library (`PrefixScope` admits only
                // `DirectiveUriWithLibraryImpl` imports, scope.dart:530-535).
                let clause_start =
                    u32::try_from(clauses.len()).expect("more than u32::MAX combinator clauses");
                for clause in edge.combinators.clauses() {
                    let names_start = u32::try_from(filter_names.len())
                        .expect("more than u32::MAX combinator names");
                    let (names, hide) = match clause {
                        CombinatorClause::Show(names) => (names, false),
                        CombinatorClause::Hide(names) => (names, true),
                    };
                    filter_names.extend(names.iter().map(|name| name.index()));
                    clauses.push(FilterClause {
                        hide,
                        names: (
                            names_start,
                            u32::try_from(filter_names.len())
                                .expect("more than u32::MAX combinator names"),
                        ),
                    });
                }
                rows.push(ImportRow {
                    prefix: edge.prefix.map_or(NO_PREFIX, StringId::index),
                    target: edge.target.map_or(NO_TARGET, LibraryId::index),
                    clauses: (
                        clause_start,
                        u32::try_from(clauses.len())
                            .expect("more than u32::MAX combinator clauses"),
                    ),
                });
            }
            // The per-unit runs, shifted onto the global row array: the
            // graph already regrouped the rows (library-wide first, then one
            // run per writing unit), so only the boundaries travel.
            for &(file, start) in graph.import_runs(lib) {
                runs.push((file, base + start));
            }
            run_offs.push(u32::try_from(runs.len()).expect("more than u32::MAX import runs"));
            offs.push(u32::try_from(rows.len()).expect("more than u32::MAX import directives"));
        }
        Self {
            rows,
            offs,
            run_offs,
            runs,
            clauses,
            filter_names,
        }
    }

    /// The steady-state memory of the compact tables, counted as
    /// `size_of` sums over the stored arrays (the memory charter's counted
    /// metric; allocator overhead and the graph's own edges excluded).
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.rows.len() * size_of::<ImportRow>()
            + self.offs.len() * size_of::<u32>()
            + self.run_offs.len() * size_of::<u32>()
            + self.runs.len() * size_of::<(FileId, u32)>()
            + self.clauses.len() * size_of::<FilterClause>()
            + self.filter_names.len() * size_of::<u32>()
    }

    /// Bare-name lookup in `lib`'s scope chain (`LibraryFragmentScope.lookup`,
    /// scope.dart:419-433): the name-coupled locals layer first, then the
    /// library's **combined import scope** — a registered prefix (the
    /// `_prefixElements` branch of `_lookupCombined` returns the prefix
    /// unconditionally, before the import namespace, scope.dart:455-471) and
    /// then the un-prefixed imports, composed first-come-wins.
    ///
    /// The combined import scope is a chain over the library's files: the
    /// library file's own imports, then each part's own imports on top
    /// ([`Self::lookup_in_file`]). This entry point is the library question —
    /// it starts at the defining unit, which is the whole scope for the
    /// single-file libraries and for the classic part shape (a part file
    /// carries no directives of its own, so its combined scope *is* its
    /// parent's).
    ///
    /// The locals link is all-or-nothing per base name: `_lookupLibrary`
    /// hands back the getter/setter pair and `_lookupCombined` is never
    /// consulted when either face is non-null (scope.dart:473-486), so a
    /// local face under either spelling suppresses import lookup for both
    /// while contributing only its own face (`Shadowed` → [`Lookup::NotFound`],
    /// mirroring the pair's absent face). Bare `_` never hits the prefix
    /// branch: with `wildcard_variables` (Dart 3.7+ default-on; pinned here)
    /// `_shouldTryPrefixElement` skips it (scope.dart:486-494).
    ///
    /// # Panics
    ///
    /// Panics when `lib` is not a [`LibraryId`] of the graph `self` was built
    /// from — a caller bug (ids only come from that graph).
    #[must_use]
    pub fn lookup(&self, graph: &LibraryGraph, lib: LibraryId, name: StringId) -> Lookup {
        let start = graph.unit_files(lib).first().copied();
        self.scope_lookup(graph, lib, start, NO_PREFIX, name)
    }

    /// Bare-name lookup at one use site: the locals layer, then `file`'s
    /// **combined import scope** — `file`'s own imports and prefixes, then
    /// its parent file's combined scope, up the part tree to the library
    /// file, and finally the library-wide rows.
    ///
    /// This is the enhanced-parts rule the spec states for the scope chain
    /// of a file: "each Dart file extends the scope chain of the parent
    /// file's combined import scope with its own imports and import
    /// prefixes … each scope able to shadow names of its enclosing scope"
    /// (upstream `CombinedScopeForFile` / `LibraryFragmentScope`, chaining
    /// every fragment over its parent). A name imported by a part resolves
    /// in that part and its sub-parts only; a name imported by a sibling
    /// part or by the library file does not resolve in it. Shadowing is
    /// strict per scope: the first scope of the chain that carries the name
    /// decides, so an inner hit is never merged with an outer one.
    ///
    /// `file` outside `lib`'s unit set asks the library question and
    /// resolves through [`Self::lookup`] — inventing a per-file scope for a
    /// file the graph never registered would silently drop every import.
    ///
    /// # Panics
    ///
    /// Same contract as [`Self::lookup`].
    #[must_use]
    pub fn lookup_in_file(
        &self,
        graph: &LibraryGraph,
        lib: LibraryId,
        file: Option<FileId>,
        name: StringId,
    ) -> Lookup {
        match self.scope_start(graph, lib, file) {
            Some(start) => self.scope_lookup(graph, lib, Some(start), NO_PREFIX, name),
            None => self.lookup(graph, lib, name),
        }
    }

    /// `prefix.name` lookup in `lib`: the prefix's merged table, whenever the
    /// prefix is registered — independent of any same-named imported
    /// declaration (`p.x` goes through `PrefixScope`, scope.dart:352-357,
    /// regardless of bare-name collisions). "Merged" is composed here the
    /// same way as the bare namespace: the same-prefix imports in directive
    /// source order, combinators applied per import, first-come-wins, over
    /// the library's combined import scope chain (the defining unit's, for
    /// this library-level entry — [`Self::lookup_prefixed_in_file`] is the
    /// use-site form).
    ///
    /// Returns [`Lookup::NotFound`] when `prefix` is not a registered prefix
    /// of `lib`, or registers no `name` (a `p.x` through a non-prefix is not
    /// a resolution hit; the misuse is the semantic layer's diagnosis).
    ///
    /// # Panics
    ///
    /// Same contract as [`Self::lookup`].
    #[must_use]
    pub fn lookup_prefixed(
        &self,
        graph: &LibraryGraph,
        lib: LibraryId,
        prefix: StringId,
        name: StringId,
    ) -> Lookup {
        let start = graph.unit_files(lib).first().copied();
        let hit = self.scope_lookup(graph, lib, start, prefix.index(), name);
        // A prefix an enhanced part registered is still THE prefix of every
        // use site that resolved `p` as a prefix in its own scope, and this
        // entry point carries no use site to ask ([`Self::lookup_prefixed_in_file`]
        // does). Spanning the library's per-unit runs restores the merged
        // per-prefix table those callers expect; the chain gate on the bare
        // name already decided whether `p` is visible at the use site.
        if !matches!(hit, Lookup::NotFound) || self.runs_of(lib).is_empty() {
            return hit;
        }
        let prefix = prefix.index();
        fold(
            graph,
            self.file_rows(lib)
                .filter(|row| row.prefix == prefix)
                .filter_map(|row| self.probe(graph, row, name)),
        )
    }

    /// `prefix.name` lookup at one use site: the same chain as
    /// [`Self::lookup_in_file`], restricted to `prefix`'s imports — an
    /// import prefix belongs to the file that wrote the `as` clause, so a
    /// part's prefix resolves in that part and its sub-parts only (the
    /// `scope_A07_t01` shape: `import … as l1;` in a part does not put `l1`
    /// in the library file's scope).
    ///
    /// # Panics
    ///
    /// Same contract as [`Self::lookup`].
    #[must_use]
    pub fn lookup_prefixed_in_file(
        &self,
        graph: &LibraryGraph,
        lib: LibraryId,
        file: Option<FileId>,
        prefix: StringId,
        name: StringId,
    ) -> Lookup {
        match self.scope_start(graph, lib, file) {
            Some(start) => self.scope_lookup(graph, lib, Some(start), prefix.index(), name),
            None => self.lookup_prefixed(graph, lib, prefix, name),
        }
    }

    /// The chain's starting unit: `file` when it is a registered unit of
    /// `lib`, `None` when the caller asked a library question (no file, or a
    /// file this library does not own) — the answer is then
    /// [`Self::lookup`]'s, the defining unit's scope.
    fn scope_start(
        &self,
        graph: &LibraryGraph,
        lib: LibraryId,
        file: Option<FileId>,
    ) -> Option<FileId> {
        file.filter(|&file| graph.library_of_file(file) == Some(lib))
    }

    /// The locals layer plus the combined import scope walk — the whole
    /// bare/prefixed lookup body once the chain start is known.
    fn scope_lookup(
        &self,
        graph: &LibraryGraph,
        lib: LibraryId,
        start: Option<FileId>,
        prefix: u32,
        name: StringId,
    ) -> Lookup {
        // The locals layer answers for bare names only: `p.x` is the
        // prefix's table (a local `p` declaration is the semantic layer's
        // misuse diagnosis, never a namespace hit).
        if prefix == NO_PREFIX {
            match graph.local(lib, name) {
                LocalLookup::Found(file) => return Lookup::Found(NamespaceEntry::Decl(file)),
                LocalLookup::Shadowed => return Lookup::NotFound,
                LocalLookup::Absent => {}
            }
        }
        self.walk(graph, lib, start, prefix, name)
    }

    /// Walks `lib`'s combined import scope from `start` up the part tree,
    /// then through the library-wide rows: per scope, the prefix branch
    /// (bare names only, [`NO_PREFIX`] marks the bare walk) and then the
    /// import composition, stopping at the first scope that carries the name
    /// — the chain's strict shadowing.
    ///
    /// The part tree is cycle-broken at graph build, so the walk terminates;
    /// the hop bound over the library's unit count is a belt-and-braces stop
    /// for the termination invariant.
    fn walk(
        &self,
        graph: &LibraryGraph,
        lib: LibraryId,
        start: Option<FileId>,
        prefix: u32,
        name: StringId,
    ) -> Lookup {
        let bare = prefix == NO_PREFIX;
        let wildcard = graph.wildcard_prefix(lib);
        let mut current = start;
        let mut hops = 0;
        let limit = graph.unit_files(lib).len().max(1);
        while let Some(file) = current {
            if let Some(rows) = self.rows_of_file(lib, file) {
                if bare
                    && wildcard != Some(name)
                    && rows.iter().any(|row| row.prefix == name.index())
                {
                    return Lookup::Found(NamespaceEntry::Prefix);
                }
                match self.compose(graph, rows, prefix, name) {
                    Lookup::NotFound => {}
                    hit => return hit,
                }
            }
            current = graph.part_parent(lib, file);
            hops += 1;
            if hops > limit {
                break;
            }
        }
        // The library-wide rows close the chain: the implicit `dart:core`
        // prelude and the synthetic SDK/dependency faces belong to every
        // file of the library, so they sit outermost.
        let wide = self.library_rows(lib);
        if bare && wildcard != Some(name) && wide.iter().any(|row| row.prefix == name.index()) {
            return Lookup::Found(NamespaceEntry::Prefix);
        }
        self.compose(graph, wide, prefix, name)
    }

    /// Folds one scope's rows into a lookup outcome: the prefix filter (a
    /// `p.name` probe reads only the imports that registered `p`; the bare
    /// walk reads the un-prefixed ones), the per-row combinator filter, and
    /// the target's export-scope probe — the merge rules of [`fold`].
    fn compose(
        &self,
        graph: &LibraryGraph,
        rows: &[ImportRow],
        prefix: u32,
        name: StringId,
    ) -> Lookup {
        fold(
            graph,
            rows.iter()
                .filter(|row| row.prefix == prefix)
                .filter_map(|row| self.probe(graph, row, name)),
        )
    }

    /// The library-wide rows of `lib` — the head of its row range, up to the
    /// first per-unit run (the whole range when the library has no runs).
    fn library_rows(&self, lib: LibraryId) -> &[ImportRow] {
        let index = lib.index() as usize;
        let start = self.offs[index] as usize;
        let end = self
            .runs_of(lib)
            .first()
            .map_or(self.offs[index + 1], |&(_, start)| start) as usize;
        &self.rows[start..end]
    }

    /// The rows one unit wrote, `None` when the unit wrote none.
    fn rows_of_file(&self, lib: LibraryId, file: FileId) -> Option<&[ImportRow]> {
        let index = lib.index() as usize;
        let runs = self.runs_of(lib);
        let position = runs.binary_search_by_key(&file, |&(file, _)| file).ok()?;
        let start = runs[position].1 as usize;
        let end = runs
            .get(position + 1)
            .map_or(self.offs[index + 1], |&(_, start)| start) as usize;
        Some(&self.rows[start..end])
    }

    /// `lib`'s per-unit run slice.
    fn runs_of(&self, lib: LibraryId) -> &[(FileId, u32)] {
        let index = lib.index() as usize;
        &self.runs[self.run_offs[index] as usize..self.run_offs[index + 1] as usize]
    }

    /// Every per-unit row of `lib`, in run order — the library-wide rows
    /// excluded (they are [`Self::library_rows`]).
    fn file_rows(&self, lib: LibraryId) -> impl Iterator<Item = &ImportRow> + '_ {
        let runs = self.runs_of(lib);
        let end = self.offs[lib.index() as usize + 1];
        runs.iter()
            .enumerate()
            .flat_map(move |(position, &(_, start))| {
                let stop = runs.get(position + 1).map_or(end, |&(_, start)| start);
                self.rows[start as usize..stop as usize].iter()
            })
    }

    /// One row's contribution to a composition walk: combinator filter, then
    /// the target's export scope probe. Rows with an unresolved target
    /// contribute nothing (their prefix registered, their table is empty —
    /// `PrefixScope`, scope.dart:530-535).
    fn probe(
        &self,
        graph: &LibraryGraph,
        row: &ImportRow,
        name: StringId,
    ) -> Option<NamespaceEntry> {
        if row.target == NO_TARGET || !self.allows(row, name) {
            return None;
        }
        let target = LibraryId::from_index(row.target as usize);
        graph
            .exported(target, name)
            .map(|entry| NamespaceEntry::Decl(entry.file))
    }

    /// Whether `name` survives `row`'s clauses, applied in source order —
    /// the stored twin of [`crate::library_graph::Combinators::allows`]: a
    /// `show` the name is not in rejects it; a `hide` containing it rejects
    /// it; later clauses see what earlier clauses left. An unfiltered row
    /// (no clauses) passes with no work.
    fn allows(&self, row: &ImportRow, name: StringId) -> bool {
        let id = name.index();
        self.clauses[row.clauses.0 as usize..row.clauses.1 as usize]
            .iter()
            .all(|clause| {
                let names = &self.filter_names[clause.names.0 as usize..clause.names.1 as usize];
                if clause.hide {
                    names.binary_search(&id).is_err()
                } else {
                    names.binary_search(&id).is_ok()
                }
            })
    }
}

/// Folds one composition walk's ordered hits into the lookup outcome — the
/// import-merge conflict rule (Dart 18.1) plus the oracle's SDK preference
/// (`PrefixScope._merge`, `element/scope.dart:811-832`): the first arrival
/// anchors, the same target arriving again dedupes to one entry, and
/// distinct targets make the name ambiguous with every candidate kept in
/// arrival order — except across sidedness, where a non-SDK declaration
/// hides an SDK one whichever arrived first (and the hidden one is dropped
/// entirely, `return other` / `return existing`). The preference applies
/// only while the anchor is a sole entry: once the walk is ambiguous, later
/// hits join the candidate set unchanged (the oracle's merge keeps folding
/// into the `MultiplyDefinedElement`, whose sidedness is non-SDK).
fn fold(graph: &LibraryGraph, hits: impl Iterator<Item = NamespaceEntry>) -> Lookup {
    let mut first: Option<NamespaceEntry> = None;
    // Empty until a genuine second candidate appears — the ambiguity error
    // path; hits and misses allocate nothing.
    let mut rest: Vec<NamespaceEntry> = Vec::new();
    for hit in hits {
        match first {
            None => first = Some(hit),
            Some(existing) if existing == hit => {}
            Some(existing) => {
                if rest.is_empty() {
                    let (existing_sdk, hit_sdk) = (is_sdk(graph, &existing), is_sdk(graph, &hit));
                    if existing_sdk && !hit_sdk {
                        first = Some(hit);
                        continue;
                    }
                    if !existing_sdk && hit_sdk {
                        continue;
                    }
                    // Two SDK entries where one is a names-only face: the
                    // face stands in for an SDK element re-exported from a
                    // library the element model carries (`dart:core`'s
                    // `Future` is `dart:async`'s class), so both are the one
                    // element — the declaration-backed entry wins.
                    if existing_sdk && hit_sdk {
                        match (is_face(graph, &existing), is_face(graph, &hit)) {
                            (true, false) => {
                                first = Some(hit);
                                continue;
                            }
                            (false, true) => continue,
                            _ => {}
                        }
                    }
                }
                if !rest.contains(&hit) {
                    rest.push(hit);
                }
            }
        }
    }
    match first {
        None => Lookup::NotFound,
        Some(first) if rest.is_empty() => Lookup::Found(first),
        Some(first) => {
            rest.insert(0, first);
            Lookup::Ambiguous(rest.into_boxed_slice())
        }
    }
}

/// Whether `entry`'s declaration lives in an SDK library — the oracle's
/// `_isSdkElement` predicate (scope.dart:860-866) reduced to this layer's
/// element model: a declaration is SDK when its declaring file's library URI
/// is `dart:`. The unindexed face cannot occur here (`probe` only returns
/// declarations the graph's export scopes carry), and an unregistered file
/// reads as non-SDK — the plain-ambiguity behavior.
fn is_sdk(graph: &LibraryGraph, entry: &NamespaceEntry) -> bool {
    let NamespaceEntry::Decl(file) = *entry else {
        return false;
    };
    graph
        .library_of_file(file)
        .is_some_and(|lib| graph.uri(lib).starts_with("dart:"))
}

/// Whether `entry` is declared on a synthetic SDK names-only face file
/// ([`LibraryGraph::is_face_file`]).
fn is_face(graph: &LibraryGraph, entry: &NamespaceEntry) -> bool {
    matches!(*entry, NamespaceEntry::Decl(file) if graph.is_face_file(file))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_graph::{CombinatorClause, Combinators, ImportEdge, LibraryGraphBuilder};
    use dartq_source::{FileTable, StringTable};
    use std::path::Path;

    /// The owned [`Lookup::Ambiguous`] value over `candidates`, in order —
    /// the test face of the composition walk's output.
    fn ambiguous(candidates: &[NamespaceEntry]) -> Lookup {
        Lookup::Ambiguous(Box::from(candidates))
    }

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

        /// [`Self::import`] as one unit of a multi-file library wrote it — a
        /// per-file import row ([`LibraryGraphBuilder::import_in`]).
        fn import_in(
            &mut self,
            lib: LibraryId,
            file: FileId,
            target: Option<LibraryId>,
            prefix: Option<&str>,
            combinators: Combinators,
        ) {
            let prefix = prefix.map(|p| self.strings.intern(p));
            self.builder.import_in(
                lib,
                file,
                ImportEdge {
                    target,
                    prefix,
                    deferred: false,
                    combinators,
                },
            );
        }

        /// Registers `file` as one compilation unit of `lib`.
        fn unit(&mut self, lib: LibraryId, file: FileId) {
            self.builder.unit(lib, file);
        }

        /// Registers the `part`-include edge `includer` → `included`.
        fn part(&mut self, lib: LibraryId, includer: FileId, included: FileId) {
            self.builder.part(lib, includer, included);
        }

        fn settle(self) -> (&'static StringTable, LibraryGraph, Namespaces) {
            let strings = self.strings;
            let graph = self.builder.finish();
            let namespaces = Namespaces::build(&graph);
            (strings, graph, namespaces)
        }
    }

    /// The import namespace is the imported library's export face filtered by
    /// the directive combinators at probe time (`NamespaceBuilder
    /// .createImportNamespaceForDirective` + `_applyCombinators`,
    /// resolver/scope.dart:159-170/227-244; combinator timing matches it).
    #[test]
    fn import_combinators_filter_at_probe() {
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
            "filtered per import, so the name never surfaces"
        );
    }

    /// Local declarations shadow imported names: the library namespace is
    /// consulted before the import namespace (`front_end` scope chain, spec
    /// — `CompilationUnitScope` over the import scope).
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

    /// Locals shadow a registered prefix, too: the locals layer precedes the
    /// `_prefixElements` branch of `_lookupCombined`
    /// (scope.dart:419-433/455-471), so a local top-level `p` wins bare `p`
    /// over `import 'x' as p` — while `p.x` still resolves through the
    /// prefix's own table (`PrefixScope`, scope.dart:352-357), which the
    /// bare-name chain never reaches.
    #[test]
    fn locals_shadow_prefix() {
        let mut rig = Rig::new();
        let target = rig.builder.intern("package:t/t.dart");
        let consumer = rig.builder.intern("package:c/c.dart");
        let t_file = rig.file("t.dart");
        let c_file = rig.file("c.dart");
        rig.declare(target, "x", t_file);
        rig.declare(consumer, "p", c_file);
        rig.import(consumer, Some(target), Some("p"), Combinators::unfiltered());
        let (strings, graph, namespaces) = rig.settle();

        assert_eq!(
            namespaces.lookup(&graph, consumer, strings.intern("p")),
            Lookup::Found(NamespaceEntry::Decl(c_file)),
            "the local declaration beats the registered prefix"
        );
        assert_eq!(
            namespaces.lookup_prefixed(&graph, consumer, strings.intern("p"), strings.intern("x")),
            Lookup::Found(NamespaceEntry::Decl(t_file)),
            "`p.x` still resolves through the prefix table"
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
            ambiguous(&[NamespaceEntry::Decl(f1), NamespaceEntry::Decl(f2)]),
        );
    }

    /// A package spelling hides the same-named SDK declaration in either
    /// directive order, with no ambiguity — the oracle's SDK preference
    /// (`PrefixScope._merge`, scope.dart:811-832: non-SDK hides SDK,
    /// `return other` / `return existing`). This is the empty_unmodifiable_set
    /// face: `import 'dart:collection'` grafts the SDK's `UnmodifiableSetView`
    /// beside the package's, and the package spelling must keep the class the
    /// `@override` climb walks.
    #[test]
    fn package_name_hides_sdk_name_in_either_directive_order() {
        let mut rig = Rig::new();
        let sdk = rig.builder.intern("dart:collection");
        let pkg = rig.builder.intern("package:p/view.dart");
        let sdk_file = rig.file("<dart:collection>");
        let pkg_file = rig.file("view.dart");
        rig.builder.unit(sdk, sdk_file);
        rig.builder.unit(pkg, pkg_file);
        rig.declare(sdk, "View", sdk_file);
        rig.declare(pkg, "View", pkg_file);

        let first_sdk = rig.builder.intern("package:c/sdk_first.dart");
        rig.import(first_sdk, Some(sdk), None, Combinators::unfiltered());
        rig.import(first_sdk, Some(pkg), None, Combinators::unfiltered());
        let first_pkg = rig.builder.intern("package:c/pkg_first.dart");
        rig.import(first_pkg, Some(pkg), None, Combinators::unfiltered());
        rig.import(first_pkg, Some(sdk), None, Combinators::unfiltered());
        let (strings, graph, namespaces) = rig.settle();
        let view = strings.intern("View");

        assert_eq!(
            namespaces.lookup(&graph, first_sdk, view),
            Lookup::Found(NamespaceEntry::Decl(pkg_file)),
            "SDK import first: the package spelling still wins"
        );
        assert_eq!(
            namespaces.lookup(&graph, first_pkg, view),
            Lookup::Found(NamespaceEntry::Decl(pkg_file)),
            "package import first: unchanged"
        );
    }

    /// Two SDK declarations of one name stay ambiguous — the preference only
    /// spans sidedness, never within the SDK (`_merge` falls through to the
    /// `MultiplyDefinedElement` when both sides are SDK).
    #[test]
    fn sdk_vs_sdk_stays_ambiguous() {
        let mut rig = Rig::new();
        let s1 = rig.builder.intern("dart:collection");
        let s2 = rig.builder.intern("dart:core");
        let consumer = rig.builder.intern("package:c/c.dart");
        let f1 = rig.file("<dart:collection>");
        let f2 = rig.file("<dart:core>");
        rig.builder.unit(s1, f1);
        rig.builder.unit(s2, f2);
        rig.declare(s1, "Clash", f1);
        rig.declare(s2, "Clash", f2);
        rig.import(consumer, Some(s1), None, Combinators::unfiltered());
        rig.import(consumer, Some(s2), None, Combinators::unfiltered());
        let (strings, graph, namespaces) = rig.settle();

        assert_eq!(
            namespaces.lookup(&graph, consumer, strings.intern("Clash")),
            ambiguous(&[NamespaceEntry::Decl(f1), NamespaceEntry::Decl(f2)]),
        );
    }

    /// Sidedness reads the *declaring* library, not the import edge's target:
    /// an SDK name re-exported through a package (`export 'dart:collection';`)
    /// still loses to a same-named package declaration (`element.library
    /// .isInSdk` is the declaring library's flag, scope.dart:860-866).
    #[test]
    fn sdk_name_reexported_through_package_still_loses_to_package_name() {
        let mut rig = Rig::new();
        let sdk = rig.builder.intern("dart:collection");
        let reexporter = rig.builder.intern("package:r/r.dart");
        let pkg = rig.builder.intern("package:p/view.dart");
        let consumer = rig.builder.intern("package:c/c.dart");
        let sdk_file = rig.file("<dart:collection>");
        let pkg_file = rig.file("view.dart");
        rig.builder.unit(sdk, sdk_file);
        rig.builder.unit(pkg, pkg_file);
        rig.declare(sdk, "View", sdk_file);
        rig.declare(pkg, "View", pkg_file);
        rig.builder.export(
            reexporter,
            crate::library_graph::ExportEdge {
                target: Some(sdk),
                combinators: Combinators::unfiltered(),
            },
        );
        rig.import(consumer, Some(reexporter), None, Combinators::unfiltered());
        rig.import(consumer, Some(pkg), None, Combinators::unfiltered());
        let (strings, graph, namespaces) = rig.settle();

        assert_eq!(
            namespaces.lookup(&graph, consumer, strings.intern("View")),
            Lookup::Found(NamespaceEntry::Decl(pkg_file)),
            "the re-exported SDK name is SDK by its declaring library"
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
            ambiguous(&[
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
            namespaces.lookup_prefixed(
                &graph,
                consumer,
                strings.intern("p"),
                strings.intern("alpha")
            ),
            Lookup::Found(NamespaceEntry::Decl(f1))
        );
        assert_eq!(
            namespaces.lookup_prefixed(
                &graph,
                consumer,
                strings.intern("p"),
                strings.intern("beta")
            ),
            Lookup::Found(NamespaceEntry::Decl(f2))
        );
        assert_eq!(
            namespaces.lookup_prefixed(
                &graph,
                consumer,
                strings.intern("p"),
                strings.intern("gamma")
            ),
            Lookup::NotFound
        );
    }

    /// Same-prefix imports compose first-come-wins: two `as p` imports
    /// bringing distinct declarations of one name make `p.name` ambiguous
    /// with both candidates, in import source order — the prefix table's
    /// merge is the same lazy rule as the bare namespace's.
    #[test]
    fn prefixed_imports_first_come_wins_on_clash() {
        let mut rig = Rig::new();
        let l1 = rig.builder.intern("package:l1/l1.dart");
        let l2 = rig.builder.intern("package:l2/l2.dart");
        let consumer = rig.builder.intern("package:c/c.dart");
        let f1 = rig.file("l1.dart");
        let f2 = rig.file("l2.dart");
        rig.declare(l1, "clash", f1);
        rig.declare(l2, "clash", f2);
        rig.import(consumer, Some(l1), Some("p"), Combinators::unfiltered());
        rig.import(consumer, Some(l2), Some("p"), Combinators::unfiltered());
        let (strings, graph, namespaces) = rig.settle();

        assert_eq!(
            namespaces.lookup_prefixed(
                &graph,
                consumer,
                strings.intern("p"),
                strings.intern("clash")
            ),
            ambiguous(&[NamespaceEntry::Decl(f1), NamespaceEntry::Decl(f2)]),
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
            namespaces.lookup_prefixed(&graph, consumer, strings.intern("p"), strings.intern("x")),
            Lookup::Found(NamespaceEntry::Decl(f2)),
            "`p.x` resolves through the prefix table"
        );
        assert_eq!(
            namespaces.lookup_prefixed(&graph, consumer, strings.intern("p"), strings.intern("y")),
            Lookup::NotFound
        );
    }

    /// An import whose URI has no library contributes no names, but its
    /// `as` prefix still registers with an empty table: the oracle builds
    /// the prefix set from every import with no resolution gating
    /// (`_buildLibraryImportPrefixes`, element.dart:7529-7536;
    /// `LibraryFragmentScope._`, scope.dart:343-353) and only the prefix's
    /// name table is filtered to resolved libraries (`PrefixScope`,
    /// scope.dart:530-535). So bare `p` resolves to the prefix while `p.x`
    /// finds nothing in it.
    #[test]
    fn missing_import_target_still_registers_its_prefix() {
        let mut rig = Rig::new();
        let consumer = rig.builder.intern("package:c/c.dart");
        rig.import(consumer, None, None, Combinators::unfiltered());
        rig.import(consumer, None, Some("p"), Combinators::unfiltered());
        let (strings, graph, namespaces) = rig.settle();

        assert_eq!(
            namespaces.lookup(&graph, consumer, strings.intern("anything")),
            Lookup::NotFound,
            "an unresolved target contributes no names"
        );
        assert_eq!(
            namespaces.lookup(&graph, consumer, strings.intern("p")),
            Lookup::Found(NamespaceEntry::Prefix),
            "the prefix registers without a target"
        );
        assert_eq!(
            namespaces.lookup_prefixed(&graph, consumer, strings.intern("p"), strings.intern("x")),
            Lookup::NotFound,
            "the prefix's name table stays empty"
        );
    }

    /// A library with no imports at all: empty tables — every bare and
    /// prefixed lookup misses (the composition walk runs over zero rows).
    #[test]
    fn library_without_imports_misses_everywhere() {
        let mut rig = Rig::new();
        let lone = rig.builder.intern("package:lone/lone.dart");
        let only_file = rig.file("lone.dart");
        rig.declare(lone, "only", only_file);
        let (strings, graph, namespaces) = rig.settle();

        assert_eq!(
            namespaces.lookup(&graph, lone, strings.intern("only")),
            Lookup::Found(NamespaceEntry::Decl(only_file)),
        );
        assert_eq!(
            namespaces.lookup(&graph, lone, strings.intern("absent")),
            Lookup::NotFound
        );
        assert_eq!(
            namespaces.lookup_prefixed(&graph, lone, strings.intern("nope"), strings.intern("x")),
            Lookup::NotFound
        );
    }

    /// Locals shadow imports at the NAME level, not per spelling: a local
    /// getter under a base name suppresses the import namespace for the
    /// setter spelling too — `_lookupLibrary` hands back the getter/setter
    /// pair and `_lookupCombined` is never consulted when either face is
    /// non-null (scope.dart:419-433/473-486; setters keyed by the base
    /// name, scope.dart:801-806). The locals contribute only their own
    /// face, so the imported setter is unreachable (a setter use misses).
    #[test]
    fn local_getter_shadows_imported_setter() {
        let mut rig = Rig::new();
        let target = rig.builder.intern("package:t/t.dart");
        let consumer = rig.builder.intern("package:c/c.dart");
        let t_file = rig.file("t.dart");
        let c_file = rig.file("c.dart");
        rig.declare(target, "foo=", t_file);
        rig.declare(consumer, "foo", c_file);
        rig.import(consumer, Some(target), None, Combinators::unfiltered());
        let (strings, graph, namespaces) = rig.settle();

        assert_eq!(
            namespaces.lookup(&graph, consumer, strings.intern("foo")),
            Lookup::Found(NamespaceEntry::Decl(c_file)),
            "the local getter wins"
        );
        assert_eq!(
            namespaces.lookup(&graph, consumer, strings.intern("foo=")),
            Lookup::NotFound,
            "the imported setter is shadowed by the local getter's base name"
        );
    }

    /// The mirror coupling: a local setter suppresses the imported getter
    /// spelling (same all-or-nothing `_lookupLibrary` rule,
    /// scope.dart:419-433/473-486).
    #[test]
    fn local_setter_shadows_imported_getter() {
        let mut rig = Rig::new();
        let target = rig.builder.intern("package:t/t.dart");
        let consumer = rig.builder.intern("package:c/c.dart");
        let t_file = rig.file("t.dart");
        let c_file = rig.file("c.dart");
        rig.declare(target, "foo", t_file);
        rig.declare(consumer, "foo=", c_file);
        rig.import(consumer, Some(target), None, Combinators::unfiltered());
        let (strings, graph, namespaces) = rig.settle();

        assert_eq!(
            namespaces.lookup(&graph, consumer, strings.intern("foo=")),
            Lookup::Found(NamespaceEntry::Decl(c_file)),
            "the local setter wins"
        );
        assert_eq!(
            namespaces.lookup(&graph, consumer, strings.intern("foo")),
            Lookup::NotFound,
            "the imported getter is shadowed by the local setter's base name"
        );
    }

    /// The locals layer is not privacy-filtered: private top-levels resolve
    /// for same-library references — including ones declared in a part file
    /// — while staying out of the export face (`LibraryDeclarations` adds
    /// all library members with no `_` filter, scope.dart:272-289; the `_`
    /// drop lives only in `buildInitialExportScope`,
    /// library_builder.dart:252-262, so cross-library privates never leak
    /// through imports).
    #[test]
    fn private_top_level_resolves_same_library() {
        let mut rig = Rig::new();
        let a = rig.builder.intern("package:a/a.dart");
        let a_file = rig.file("a.dart"); // declared in the library file
        let lib_b = rig.builder.intern("package:b/b.dart");
        let b_part = rig.file("b_part.dart"); // declared in a part file
        let consumer = rig.builder.intern("package:c/c.dart");
        rig.declare(a, "_helper", a_file);
        rig.declare(lib_b, "_helper", b_part);
        rig.import(consumer, Some(a), None, Combinators::unfiltered());
        rig.import(consumer, Some(lib_b), None, Combinators::unfiltered());
        let (strings, graph, namespaces) = rig.settle();

        assert_eq!(
            namespaces.lookup(&graph, a, strings.intern("_helper")),
            Lookup::Found(NamespaceEntry::Decl(a_file))
        );
        assert_eq!(
            namespaces.lookup(&graph, lib_b, strings.intern("_helper")),
            Lookup::Found(NamespaceEntry::Decl(b_part)),
            "same-library, declared from a part file"
        );
        assert_eq!(
            namespaces.lookup(&graph, consumer, strings.intern("_helper")),
            Lookup::NotFound,
            "privates never leak through imports"
        );
    }

    /// With `wildcard_variables` (Dart 3.7+ default-on; pinned here) a bare
    /// `_` never hits the prefix branch: `_shouldTryPrefixElement` skips it
    /// (scope.dart:486-494), so `import 'x' as _;` does not make a bare `_`
    /// reference resolve to the prefix.
    #[test]
    fn wildcard_underscore_lookup_skips_prefix_branch() {
        let mut rig = Rig::new();
        let target = rig.builder.intern("package:t/t.dart");
        let consumer = rig.builder.intern("package:c/c.dart");
        let t_file = rig.file("t.dart");
        rig.declare(target, "x", t_file);
        rig.import(consumer, Some(target), Some("_"), Combinators::unfiltered());
        let (strings, graph, namespaces) = rig.settle();

        assert_eq!(
            namespaces.lookup(&graph, consumer, strings.intern("_")),
            Lookup::NotFound,
            "the wildcard `_` skips the prefix branch"
        );
        assert_eq!(
            namespaces.lookup_prefixed(&graph, consumer, strings.intern("_"), strings.intern("x")),
            Lookup::Found(NamespaceEntry::Decl(t_file)),
            "`_.x` still resolves through the prefix table"
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
            namespaces.lookup_prefixed(&graph, consumer, strings.intern("p"), strings.intern("x")),
            Lookup::NotFound
        );
    }

    /// The enhanced-parts chain (co19 `scope_A07_t01`): each file's combined
    /// import scope is its own imports over its parent's. A part's import
    /// resolves in that part and in its sub-parts, never in the library file
    /// and never sideways; the library-level question answers the library
    /// file's own scope.
    #[test]
    fn part_scopes_chain_up_the_part_tree() {
        let mut rig = Rig::new();
        let lib = rig.builder.intern("package:a/a.dart");
        let imported1 = rig.builder.intern("package:l1/l1.dart");
        let imported2 = rig.builder.intern("package:l2/l2.dart");
        let a = rig.file("a.dart");
        let part1 = rig.file("part1.dart");
        let part2 = rig.file("part2.dart");
        let l1_file = rig.file("lib1.dart");
        let l2_file = rig.file("lib2.dart");
        rig.declare(imported1, "libVar", l1_file);
        rig.declare(imported2, "counter", l2_file);
        rig.unit(lib, a);
        rig.unit(lib, part1);
        rig.unit(lib, part2);
        rig.part(lib, a, part1);
        rig.part(lib, part1, part2);
        rig.import_in(lib, part1, Some(imported1), None, Combinators::unfiltered());
        rig.import_in(lib, part2, Some(imported2), None, Combinators::unfiltered());
        let (strings, graph, namespaces) = rig.settle();
        let lib_var = strings.intern("libVar");
        let counter = strings.intern("counter");

        assert_eq!(
            namespaces.lookup_in_file(&graph, lib, Some(a), lib_var),
            Lookup::NotFound,
            "the library file wrote no import: it does not inherit a part's"
        );
        assert_eq!(
            namespaces.lookup_in_file(&graph, lib, Some(part1), lib_var),
            Lookup::Found(NamespaceEntry::Decl(l1_file)),
            "part1's own import"
        );
        assert_eq!(
            namespaces.lookup_in_file(&graph, lib, Some(part2), lib_var),
            Lookup::Found(NamespaceEntry::Decl(l1_file)),
            "a sub-part inherits its includer's combined scope"
        );
        assert_eq!(
            namespaces.lookup_in_file(&graph, lib, Some(part1), counter),
            Lookup::NotFound,
            "a sub-part's import never reaches its includer"
        );
        assert_eq!(
            namespaces.lookup_in_file(&graph, lib, Some(part2), counter),
            Lookup::Found(NamespaceEntry::Decl(l2_file)),
            "part2's own import"
        );
        assert_eq!(
            namespaces.lookup(&graph, lib, lib_var),
            Lookup::NotFound,
            "the library-level lookup answers the library file's scope"
        );
    }

    /// co19 `scope_A02_t04`: `show`/`hide` filter per import, across the part
    /// boundary — a filtered-out name stays out even when a sibling part
    /// imports the same library under a different combinator.
    #[test]
    fn part_show_hide_filters_across_the_boundary() {
        let mut rig = Rig::new();
        let lib = rig.builder.intern("package:a/a.dart");
        let lib1 = rig.builder.intern("package:l1/l1.dart");
        let lib2 = rig.builder.intern("package:l2/l2.dart");
        let a = rig.file("a.dart");
        let part1 = rig.file("part1.dart");
        let part2 = rig.file("part2.dart");
        let f1 = rig.file("lib1.dart");
        let f2 = rig.file("lib2.dart");
        rig.declare(lib1, "libVar", f1);
        rig.declare(lib1, "LibClass", f1);
        rig.declare(lib2, "libId", f2);
        rig.unit(lib, a);
        rig.unit(lib, part1);
        rig.unit(lib, part2);
        rig.part(lib, a, part1);
        rig.part(lib, part1, part2);
        let strings = rig.strings;
        let show_lib_class = Combinators::new(
            [CombinatorClause::Show(Box::from([
                strings.intern("LibClass")
            ]))],
            strings,
        );
        let hide_lib_id = Combinators::new(
            [CombinatorClause::Hide(Box::from([strings.intern("libId")]))],
            strings,
        );
        let show_lib_id = Combinators::new(
            [CombinatorClause::Show(Box::from([strings.intern("libId")]))],
            strings,
        );
        rig.import_in(lib, part1, Some(lib1), None, show_lib_class);
        rig.import_in(lib, part1, Some(lib2), None, hide_lib_id);
        rig.import_in(lib, part2, Some(lib2), None, show_lib_id);
        let (strings, graph, namespaces) = rig.settle();

        assert_eq!(
            namespaces.lookup_in_file(&graph, lib, Some(part1), strings.intern("LibClass")),
            Lookup::Found(NamespaceEntry::Decl(f1)),
            "the shown name resolves"
        );
        assert_eq!(
            namespaces.lookup_in_file(&graph, lib, Some(part1), strings.intern("libVar")),
            Lookup::NotFound,
            "`show LibClass` drops the rest of lib1"
        );
        assert_eq!(
            namespaces.lookup_in_file(&graph, lib, Some(part1), strings.intern("libId")),
            Lookup::NotFound,
            "`hide libId` drops it for part1"
        );
        assert_eq!(
            namespaces.lookup_in_file(&graph, lib, Some(part2), strings.intern("libId")),
            Lookup::Found(NamespaceEntry::Decl(f2)),
            "part2's own `show libId` brings it back"
        );
    }

    /// An import prefix belongs to the file that wrote the `as` clause: the
    /// library file does not see a part's `l1`, and `l1.x` resolves only from
    /// that part's chain. The library-level prefixed entry — which carries no
    /// use site — keeps spanning every file's prefixes (the callers that use
    /// it already gated the bare name on their own scope).
    #[test]
    fn part_prefix_is_confined_to_its_file() {
        let mut rig = Rig::new();
        let lib = rig.builder.intern("package:a/a.dart");
        let imported = rig.builder.intern("package:l2/l2.dart");
        let a = rig.file("a.dart");
        let part1 = rig.file("part1.dart");
        let f = rig.file("lib2.dart");
        rig.declare(imported, "lib2Func", f);
        rig.unit(lib, a);
        rig.unit(lib, part1);
        rig.part(lib, a, part1);
        rig.import_in(
            lib,
            part1,
            Some(imported),
            Some("l1"),
            Combinators::unfiltered(),
        );
        let (strings, graph, namespaces) = rig.settle();
        let prefix = strings.intern("l1");
        let leaf = strings.intern("lib2Func");

        assert_eq!(
            namespaces.lookup_in_file(&graph, lib, Some(a), prefix),
            Lookup::NotFound,
            "the library file never wrote `as l1`"
        );
        assert_eq!(
            namespaces.lookup_in_file(&graph, lib, Some(part1), prefix),
            Lookup::Found(NamespaceEntry::Prefix),
            "part1 registered it"
        );
        assert_eq!(
            namespaces.lookup_prefixed_in_file(&graph, lib, Some(a), prefix, leaf),
            Lookup::NotFound,
            "`l1.lib2Func` does not resolve from the library file"
        );
        assert_eq!(
            namespaces.lookup_prefixed_in_file(&graph, lib, Some(part1), prefix, leaf),
            Lookup::Found(NamespaceEntry::Decl(f)),
            "`l1.lib2Func` resolves from part1"
        );
        assert_eq!(
            namespaces.lookup_prefixed(&graph, lib, prefix, leaf),
            Lookup::Found(NamespaceEntry::Decl(f)),
            "the library-level prefixed entry spans every file's prefixes"
        );
    }
}
