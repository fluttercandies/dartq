//! The library graph: interned library nodes, import/export edges, and the
//! eager export-scope fixpoint (`docs/facts/resolution-design-spec.md` §3/§5,
//! module `library_graph.rs`).
//!
//! Nodes are libraries keyed by their canonical identity string
//! ([`crate::UriResolver::canonical_uri`] — `package:` inside package lib
//! dirs, `file:` elsewhere), interned to [`LibraryId`]; no string is hashed
//! after construction. Edges are the caller's resolved import/export
//! directives ([`ImportEdge`]/[`ExportEdge`]): URI resolution and conditional
//! selection are upstream ([`crate::UriResolver::resolve`]), and a directive
//! whose target is external (`dart:`), missing, or not a library carries
//! `target: None` — it contributes no names, matching the oracle's
//! null-library guard (`imports_verifier.dart:63-70`). The §4.2 codes for
//! such directives stay with the caller's [`crate::UriResolution`]; this
//! layer only computes namespaces.
//!
//! # Export scopes (pinned oracle behavior)
//!
//! A library's export scope is the name face an import of it sees. It is
//! seeded with the library's local public declarations and then resolved to a
//! fixed point over the export edges, mirroring the pinned analyzer oracle
//! (`analyzer-8.4.1`):
//!
//! - Local public declarations are declared first and are never overwritten,
//!   so **locals beat re-exports** (`summary2/library_builder.dart:252-262`
//!   `buildInitialExportScope`, which also drops `_`-private names and
//!   prefixes; CFE agreement in `source_library_builder.dart:430-431`).
//! - Setters live under their `name=` spelling
//!   (`library_builder.dart:136-140`); combinator name sets are expanded with
//!   the setter spellings at construction ([`Combinators::new`]) so matching
//!   is pure id membership, equivalent to `summary2/combinator.dart`
//!   `Combinator.matches` stripping the trailing `=`.
//! - During propagation an entry is inserted only if the name is absent:
//!   a **conflicting re-export loses silently to the first flow**
//!   (`summary2/export.dart:105-127` `ExportScope.export` — "Ambiguous
//!   declaration detected. `if (existing != null) return false;`"); the same
//!   target arriving again through another path dedupes (no conflict). There
//!   is no ambiguity marker and no non-`dart:`-over-`dart:` priority in
//!   export scopes — those CFE rules (`source_library_builder.dart:401-434`)
//!   are not the analyzer's behavior and are not implemented.
//! - Cycles converge to the same least fixed point regardless of order
//!   because insertion is monotone (entries are never removed or replaced);
//!   order only decides which target wins a genuine re-export race. dartq
//!   pins the oracle's order exactly: the initial pass sweeps exported
//!   libraries in registration (discovery) order, then the newly-added-name
//!   worklist drains LIFO (`link.dart:167-206`).
//!
//! ## Deviation from the spec sketch (§5), oracle wins
//!
//! The spec sketch orders the fixpoint as Tarjan SCC condensation plus a
//! per-SCC iteration. The pinned oracle (`summary2/link.dart:144-218`
//! `_buildExportScopes`) applies every exported library's whole scope through
//! its export records directly — exported builders visited in `builders`
//! registration order — and then runs a **newly-added-name worklist** drained
//! LIFO (`additionalExportData.removeLast()`); both compute the same least
//! fixed point of a monotone operator. The oracle shape is implemented here
//! verbatim because the visit order is observable in first-come-wins races,
//! and its termination invariant is exact — a name enters any scope at most
//! once, so each (flow, name) pair is applied at most once per pass and
//! enqueued at most once, for total work `O(Σ_flow |final scope of
//! flow.from|)` hash operations, with no SCC pass, no rounds counter, and no
//! visited-set bookkeeping beyond the insert itself.
//!
//! # Contracts
//!
//! - The caller must [`declare`](LibraryGraphBuilder::declare) every
//!   library whose public face should contribute names; a node registered
//!   only as an edge target exports nothing.
//! - A file belongs to exactly one library, so an export entry is identified
//!   by its declaring file alone (the name is the map key).
//! - The graph is sized entirely by its input (libraries × names × edges);
//!   nothing grows after [`LibraryGraphBuilder::finish`].

use dartq_source::{FileId, StringId, StringTable};
use rustc_hash::FxHashMap;

/// An interned library node: the resolution layer's logical identity
/// (spec §5; the engine's arena `LibraryId` is a separate, downstream type).
/// Issued by [`LibraryGraphBuilder::intern`]; indexes the [`LibraryGraph`]
/// vectors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LibraryId(u32);

impl LibraryId {
    /// The underlying integer, stable for the lifetime of the graph.
    #[must_use]
    pub const fn index(self) -> u32 {
        self.0
    }

    /// Crate-internal constructor from a vector index (`namespace.rs` walks
    /// the graph positionally when building per-library namespaces).
    pub(crate) fn from_index(index: usize) -> Self {
        Self(u32::try_from(index).expect("more than u32::MAX libraries"))
    }
}

/// One source-ordered `show`/`hide` clause of an import or export directive.
///
/// The name sets are stored **expanded**: [`Combinators::new`] joins each
/// written bare identifier `n` with its setter spelling `n=`, pinning the
/// oracle's match rule — `summary2/combinator.dart` `Combinator.matches`
/// strips a trailing `=` from the entry name before membership, and
/// `NamespaceBuilder._show`/`_hide` (`dart/resolver/scope.dart:252-282`) move
/// `name` and `name=` together. Expansion moves that text work to
/// construction, so the hot path is binary search over interned ids.
#[derive(Clone, Debug)]
pub enum CombinatorClause {
    /// `show n1, …` — keeps only the listed names (and their setters).
    Show(Box<[StringId]>),
    /// `hide n1, …` — drops the listed names (and their setters).
    Hide(Box<[StringId]>),
}

/// The ordered combinator list of one directive; empty means unfiltered.
#[derive(Clone, Debug, Default)]
pub struct Combinators(Box<[CombinatorClause]>);

impl Combinators {
    /// Builds the combinator list from source-ordered clauses, expanding each
    /// clause's names with their `name=` setter spellings (interned into
    /// `strings`) and sorting for binary-search membership. Clause names are
    /// bare identifiers (combinator grammar has no `=`); a written set is
    /// taken as given.
    pub fn new(clauses: impl IntoIterator<Item = CombinatorClause>, strings: &StringTable) -> Self {
        let clauses: Box<[CombinatorClause]> = clauses
            .into_iter()
            .map(|clause| match clause {
                CombinatorClause::Show(names) => {
                    CombinatorClause::Show(expand_names(&names, strings))
                }
                CombinatorClause::Hide(names) => {
                    CombinatorClause::Hide(expand_names(&names, strings))
                }
            })
            .collect();
        Self(clauses)
    }

    /// The unfiltered list — the no-combinator common case.
    #[must_use]
    pub fn unfiltered() -> Self {
        Self(Box::new([]))
    }

    /// Whether `name` survives this directive's combinators, applied in
    /// source order (`summary2/combinator.dart` `allows`: a `show` the name
    /// is not in rejects it; a `hide` containing it rejects it; later
    /// clauses see what earlier clauses left).
    #[must_use]
    pub fn allows(&self, name: StringId) -> bool {
        self.0.iter().all(|clause| match clause {
            CombinatorClause::Show(names) => names.binary_search(&name).is_ok(),
            CombinatorClause::Hide(names) => names.binary_search(&name).is_err(),
        })
    }
}

/// Expands bare combinator names with their `name=` setter spellings, sorted
/// and deduplicated for binary-search membership.
fn expand_names(names: &[StringId], strings: &StringTable) -> Box<[StringId]> {
    let mut expanded: Vec<StringId> = Vec::with_capacity(names.len() * 2);
    for &name in names {
        expanded.push(name);
        let setter = format!("{}=", strings.resolve(name));
        expanded.push(strings.intern(&setter));
    }
    expanded.sort_unstable();
    expanded.dedup();
    expanded.into_boxed_slice()
}

/// One resolved `import` directive as a graph edge (spec §2.5/§5).
///
/// The caller resolves the directive URI upstream and maps the outcome: a
/// library of the analysis set becomes `Some`, an external (`dart:`),
/// missing, or part-file target becomes `None` — such an import contributes
/// no names and registers no prefix.
#[derive(Clone, Debug)]
pub struct ImportEdge {
    /// The imported library, or `None` when it contributes nothing.
    pub target: Option<LibraryId>,
    /// The `as` prefix name, if any (interned).
    pub prefix: Option<StringId>,
    /// Whether the import is `deferred` (§2.5: recorded, never evaluated —
    /// `loadLibrary` synthesis is the semantic layer's job).
    pub deferred: bool,
    /// The directive's filters, applied at namespace-build time
    /// ([`crate::namespace`]; §1 — combinator filtering is build-time).
    pub combinators: Combinators,
}

/// One resolved `export` directive as a graph edge. Its combinators filter
/// the names that flow through it during the export fixpoint
/// (`summary2/export.dart:22-28` `Export.addToExportScope`).
#[derive(Clone, Debug)]
pub struct ExportEdge {
    /// The exported library, or `None` when it contributes nothing.
    pub target: Option<LibraryId>,
    /// The directive's filters, applied per propagated name.
    pub combinators: Combinators,
}

/// One entry of a library's export scope: which declaration the name resolves
/// to, and whether it is exported directly or arrived via a re-export — the
/// "exports directly vs re-exports" split of the public face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExportEntry {
    /// The file declaring the target. The name itself is the map key and a
    /// file belongs to exactly one library, so this identifies the
    /// declaration.
    pub file: FileId,
    /// `true` for a local public declaration, `false` for a re-export.
    pub is_local: bool,
}

/// A propagation flow: the export scope of `from` flows into `to` through
/// `to`'s export directive, whose combinators live at the `(library,
/// directive)` position. One per export edge with an in-graph target — the
/// oracle's `Export` objects registered on the exported builder
/// (`library_builder.dart:125-158` `addExporters`).
struct Flow {
    from: LibraryId,
    to: LibraryId,
    combinators: (usize, usize),
}

/// Builds a [`LibraryGraph`]: register libraries, their local declarations,
/// and their resolved directive edges, then [`finish`](Self::finish) runs the
/// export fixpoint.
pub struct LibraryGraphBuilder<'t> {
    strings: &'t StringTable,
    nodes: Vec<NodeBuilder>,
    by_uri: FxHashMap<Box<str>, LibraryId>,
}

struct NodeBuilder {
    uri: Box<str>,
    decls: Vec<(StringId, FileId)>,
    imports: Vec<ImportEdge>,
    exports: Vec<ExportEdge>,
}

impl<'t> LibraryGraphBuilder<'t> {
    /// Creates a builder interning names into `strings` (shared with the
    /// caller's tables so combinator expansion and private-name filtering can
    /// read text).
    pub fn new(strings: &'t StringTable) -> Self {
        Self {
            strings,
            nodes: Vec::new(),
            by_uri: FxHashMap::default(),
        }
    }

    /// Interns `canonical_uri` as a library identity, creating the node on
    /// first sight. Directive edges may target libraries that are never
    /// described; such nodes simply export nothing (see the module
    /// contracts).
    ///
    /// # Panics
    ///
    /// Panics when more than [`u32::MAX`] libraries are interned — a program
    /// bug at any realistic scale.
    pub fn intern(&mut self, canonical_uri: &str) -> LibraryId {
        if let Some(&id) = self.by_uri.get(canonical_uri) {
            return id;
        }
        let id = LibraryId::from_index(self.nodes.len());
        self.by_uri.insert(Box::from(canonical_uri), id);
        self.nodes.push(NodeBuilder {
            uri: Box::from(canonical_uri),
            decls: Vec::new(),
            imports: Vec::new(),
            exports: Vec::new(),
        });
        id
    }

    /// Records one top-level declaration `name` of `lib`, declared in `file`
    /// (the library file or one of its parts; folding the part tree is the
    /// caller's job). Private names — leading `_`, per
    /// `library_builder.dart:254-256` — are dropped: they never enter any
    /// export scope. Duplicate names keep the first: a library's members have
    /// unique names by construction, and duplicates are the semantic layer's
    /// diagnosis, not a namespace-shaping event.
    pub fn declare(&mut self, lib: LibraryId, name: StringId, file: FileId) {
        let strings = self.strings;
        if strings.resolve(name).starts_with('_') {
            return;
        }
        self.nodes[lib.index() as usize].decls.push((name, file));
    }

    /// Records `lib`'s resolved `import` directive, in source order.
    pub fn import(&mut self, lib: LibraryId, edge: ImportEdge) {
        self.nodes[lib.index() as usize].imports.push(edge);
    }

    /// Records `lib`'s resolved `export` directive, in source order.
    pub fn export(&mut self, lib: LibraryId, edge: ExportEdge) {
        self.nodes[lib.index() as usize].exports.push(edge);
    }

    /// Freezes the graph: seeds every export scope with its local public
    /// declarations, then resolves the export scopes in the oracle's pinned
    /// order — a direct initial pass over flows grouped by exported-library
    /// registration order, then the LIFO newly-added-name worklist (see the
    /// module docs for the pinned behavior and the termination invariant).
    ///
    /// # Panics
    ///
    /// Panics when the export-flow count or any library count exceeds
    /// [`u32::MAX`] — a program bug at any realistic scale.
    #[must_use]
    pub fn finish(self) -> LibraryGraph {
        // Seed: locals first, so no propagation can ever displace them.
        let mut scopes: Vec<FxHashMap<StringId, ExportEntry>> = self
            .nodes
            .iter()
            .map(|node| {
                let mut scope = FxHashMap::default();
                for (name, file) in &node.decls {
                    scope.entry(*name).or_insert(ExportEntry {
                        file: *file,
                        is_local: true,
                    });
                }
                scope
            })
            .collect();

        // Flows: one per export edge with an in-graph target, grouped by
        // source library; within a source, (exporter registration order,
        // directive source order) — the oracle's Export records on the
        // exported builder (`addExporters` runs in `builders.values` order).
        let mut flows: Vec<Flow> = Vec::new();
        let mut flows_from: Vec<Vec<u32>> = vec![Vec::new(); self.nodes.len()];
        let mut is_exporter: Vec<bool> = vec![false; self.nodes.len()];
        for (lib_index, node) in self.nodes.iter().enumerate() {
            let to = LibraryId::from_index(lib_index);
            for (dir_index, edge) in node.exports.iter().enumerate() {
                let Some(target) = edge.target else { continue };
                let flow_index =
                    u32::try_from(flows.len()).expect("more than u32::MAX export flows");
                flows.push(Flow {
                    from: target,
                    to,
                    combinators: (lib_index, dir_index),
                });
                flows_from[target.index() as usize].push(flow_index);
                is_exporter[lib_index] = true;
            }
        }
        let nodes = self.nodes;

        // Initial pass: every flow carries the whole scope of its source as
        // of its slot, applied directly — exported libraries swept in
        // registration (intern) order, flows within a source in (exporter,
        // directive) order (`link.dart:167-179`). This order is the
        // first-come-wins tiebreak in genuine races.
        for source in 0..nodes.len() {
            if flows_from[source].is_empty() {
                continue;
            }
            let names: Vec<StringId> = scopes[source].keys().copied().collect();
            for &flow_index in &flows_from[source] {
                let flow = &flows[flow_index as usize];
                for &name in &names {
                    apply_flow(flow, name, &nodes, &mut scopes);
                }
            }
        }

        // Both pass: libraries that carry export flows and are themselves
        // flow targets re-apply their grown scopes — the oracle's `both` set
        // (`link.dart:181-190`) — enqueuing each newly added name for
        // further propagation.
        let mut pending: Vec<(u32, StringId)> = Vec::new();
        for source in 0..nodes.len() {
            if flows_from[source].is_empty() || !is_exporter[source] {
                continue;
            }
            let names: Vec<StringId> = scopes[source].keys().copied().collect();
            for &flow_index in &flows_from[source] {
                let flow = &flows[flow_index as usize];
                for &name in &names {
                    if apply_flow(flow, name, &nodes, &mut scopes) {
                        pending.push((flow.to.index(), name));
                    }
                }
            }
        }

        // Drain LIFO (`additionalExportData.removeLast()`,
        // `link.dart:196-206`): a newly added name propagates through the
        // gaining library's own outflows, and each further insert re-enqueues.
        // Fixpoint: each (flow, name) is applied at most once successfully —
        // an insert happens at most once per name per scope — so the stack
        // drains.
        while let Some((source, name)) = pending.pop() {
            for &flow_index in &flows_from[source as usize] {
                let flow = &flows[flow_index as usize];
                if apply_flow(flow, name, &nodes, &mut scopes) {
                    pending.push((flow.to.index(), name));
                }
            }
        }

        let nodes = nodes
            .into_iter()
            .zip(scopes)
            .map(|(node, export_scope)| LibraryNode {
                uri: node.uri,
                imports: node.imports,
                exports: node.exports,
                export_scope,
            })
            .collect();
        LibraryGraph { nodes }
    }
}

/// Applies one flow for `name`: combinator filter, then a first-come-wins
/// insert of the source's current entry into the target scope
/// (`export.dart:22-28` `addToExportScope` wrapping `ExportScope.export`'s
/// absent-only insert). Returns whether the name was newly added — the
/// oracle's enqueue signal.
fn apply_flow(
    flow: &Flow,
    name: StringId,
    nodes: &[NodeBuilder],
    scopes: &mut [FxHashMap<StringId, ExportEntry>],
) -> bool {
    let (lib, dir) = flow.combinators;
    if !nodes[lib].exports[dir].combinators.allows(name) {
        return false;
    }
    let file = scopes[flow.from.index() as usize]
        .get(&name)
        .expect("flow source scope holds the name")
        .file;
    let target = &mut scopes[flow.to.index() as usize];
    if target.contains_key(&name) {
        return false; // first-come-wins: locals and earlier flows stay
    }
    target.insert(
        name,
        ExportEntry {
            file,
            is_local: false,
        },
    );
    true
}

/// The frozen library graph: interned nodes with directive edges and computed
/// export scopes. Each node's `uri` is the single owner of the canonical
/// identity string (the builder's interning map is discarded here).
pub struct LibraryGraph {
    nodes: Vec<LibraryNode>,
}

struct LibraryNode {
    uri: Box<str>,
    imports: Vec<ImportEdge>,
    exports: Vec<ExportEdge>,
    export_scope: FxHashMap<StringId, ExportEntry>,
}

impl LibraryGraph {
    /// The number of library nodes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the graph has no libraries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The canonical identity string of `lib`.
    #[must_use]
    pub fn uri(&self, lib: LibraryId) -> &str {
        &self.nodes[lib.index() as usize].uri
    }

    /// The resolved import edges of `lib`, in directive source order.
    #[must_use]
    pub fn import_edges(&self, lib: LibraryId) -> &[ImportEdge] {
        &self.nodes[lib.index() as usize].imports
    }

    /// The resolved export edges of `lib`, in directive source order.
    #[must_use]
    pub fn export_edges(&self, lib: LibraryId) -> &[ExportEdge] {
        &self.nodes[lib.index() as usize].exports
    }

    /// Every public name `lib` makes available to importers — local
    /// declarations and re-exports alike, the split carried by
    /// [`ExportEntry::is_local`].
    pub fn export_scope(&self, lib: LibraryId) -> impl Iterator<Item = (StringId, &ExportEntry)> {
        self.nodes[lib.index() as usize]
            .export_scope
            .iter()
            .map(|(name, entry)| (*name, entry))
    }

    /// The export-face entry for `name` — what an import of `lib` sees.
    #[must_use]
    pub fn exported(&self, lib: LibraryId, name: StringId) -> Option<&ExportEntry> {
        self.nodes[lib.index() as usize].export_scope.get(&name)
    }

    /// The locally declared entry for `name`, or `None` — the shadowing face:
    /// a library's own declarations beat its imports in
    /// [`crate::namespace`] lookup.
    #[must_use]
    pub fn local(&self, lib: LibraryId, name: StringId) -> Option<FileId> {
        let entry = self.nodes[lib.index() as usize].export_scope.get(&name)?;
        entry.is_local.then_some(entry.file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dartq_source::FileTable;
    use std::path::Path;

    /// Shared per-test context: strings, files, a builder, and shorthand
    /// constructors for unfiltered edges. The string table is leaked for
    /// `'static` so the builder can hold it alongside the rig (every
    /// `StringTable` method takes `&self`).
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

        fn export(&mut self, lib: LibraryId, target: Option<LibraryId>) {
            self.builder.export(
                lib,
                ExportEdge {
                    target,
                    combinators: Combinators::unfiltered(),
                },
            );
        }
    }

    fn import_edge(target: Option<LibraryId>) -> ImportEdge {
        ImportEdge {
            target,
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        }
    }

    // ---- combinators ----

    /// `show foo` keeps the `foo=` setter too: `Combinator.matches` strips a
    /// trailing `=` from the entry name (summary2/combinator.dart), and
    /// `NamespaceBuilder._show` brings `name` and `name=` along together
    /// (resolver/scope.dart:265-282). Expansion must be equivalent.
    #[test]
    fn show_clause_admits_setter_spelling() {
        let strings = StringTable::new();
        let foo = strings.intern("foo");
        let setter = strings.intern("foo=");
        let combinators = Combinators::new([CombinatorClause::Show(Box::from([foo]))], &strings);
        assert!(combinators.allows(foo));
        assert!(combinators.allows(setter), "`show foo` must keep `foo=`");
        assert!(!combinators.allows(strings.intern("bar")));
        assert!(!combinators.allows(strings.intern("bar=")));
    }

    /// Clauses apply in source order: a later `hide` removes what an earlier
    /// `show` admitted (`_applyCombinators` iterates in order,
    /// resolver/scope.dart:227-244).
    #[test]
    fn combinator_clauses_apply_in_source_order() {
        let strings = StringTable::new();
        let a = strings.intern("a");
        let clauses = [
            CombinatorClause::Show(Box::from([a])),
            CombinatorClause::Hide(Box::from([a])),
        ];
        assert!(!Combinators::new(clauses, &strings).allows(a));
    }

    /// `hide foo` removes both `foo` and `foo=` (`_hide`,
    /// resolver/scope.dart:252-262).
    #[test]
    fn hide_clause_drops_setter_spelling() {
        let strings = StringTable::new();
        let foo = strings.intern("foo");
        let setter = strings.intern("foo=");
        let combinators = Combinators::new([CombinatorClause::Hide(Box::from([foo]))], &strings);
        assert!(!combinators.allows(foo));
        assert!(!combinators.allows(setter));
    }

    // ---- export scope fixpoint ----

    /// Transitive chains: a exports b, b exports c — a's face carries c's
    /// names (initial pass plus worklist, link.dart:159-186).
    #[test]
    fn export_chain_propagates_transitively() {
        let mut rig = Rig::new();
        let c = rig.builder.intern("package:c/c.dart");
        let lib_b = rig.builder.intern("package:b/b.dart");
        let a = rig.builder.intern("package:a/a.dart");
        let c_file = rig.file("c.dart");
        rig.declare(c, "cName", c_file);
        rig.export(lib_b, Some(c));
        rig.export(a, Some(lib_b));
        let graph = rig.builder.finish();

        let entry = graph
            .exported(a, rig.strings.intern("cName"))
            .expect("propagated");
        assert_eq!(entry.file, c_file);
        assert!(!entry.is_local, "arrived via re-export");
        assert!(graph.local(a, rig.strings.intern("cName")).is_none());
    }

    /// Export cycles converge: a and b export each other; both faces carry
    /// both names (link.dart `both` set + worklist, spec §3.1).
    #[test]
    fn export_cycle_reaches_fixpoint() {
        let mut rig = Rig::new();
        let a = rig.builder.intern("package:a/a.dart");
        let lib_b = rig.builder.intern("package:b/b.dart");
        let a_file = rig.file("a.dart");
        let b_file = rig.file("b.dart");
        rig.declare(a, "aName", a_file);
        rig.declare(lib_b, "bName", b_file);
        rig.export(a, Some(lib_b));
        rig.export(lib_b, Some(a));
        let graph = rig.builder.finish();

        let strings = &rig.strings;
        let on_a = graph
            .exported(a, strings.intern("bName"))
            .expect("cycle in-flow");
        let on_b = graph
            .exported(lib_b, strings.intern("aName"))
            .expect("cycle in-flow");
        assert_eq!(on_a.file, b_file);
        assert_eq!(on_b.file, a_file);
        // Locals survive the cycle untouched.
        assert!(
            graph
                .exported(a, strings.intern("aName"))
                .expect("local")
                .is_local
        );
    }

    /// A three-library cycle terminates and converges (termination
    /// invariant: each (flow, name) is enqueued exactly once).
    #[test]
    fn three_library_cycle_terminates() {
        let mut rig = Rig::new();
        let a = rig.builder.intern("package:a/a.dart");
        let lib_b = rig.builder.intern("package:b/b.dart");
        let c = rig.builder.intern("package:c/c.dart");
        let a_file = rig.file("a.dart");
        let b_file = rig.file("b.dart");
        let c_file = rig.file("c.dart");
        rig.declare(a, "aName", a_file);
        rig.declare(lib_b, "bName", b_file);
        rig.declare(c, "cName", c_file);
        rig.export(a, Some(lib_b));
        rig.export(lib_b, Some(c));
        rig.export(c, Some(a));
        let graph = rig.builder.finish();

        let strings = &rig.strings;
        assert!(graph.exported(a, strings.intern("cName")).is_some());
        assert!(graph.exported(lib_b, strings.intern("aName")).is_some());
        assert!(graph.exported(c, strings.intern("bName")).is_some());
    }

    /// A self-export (`export 'a.dart'` inside a) is a one-node cycle: the
    /// local stays local, the re-flow is a no-op, the worklist drains.
    #[test]
    fn self_export_is_a_no_op() {
        let mut rig = Rig::new();
        let a = rig.builder.intern("package:a/a.dart");
        let a_file = rig.file("a.dart");
        rig.declare(a, "aName", a_file);
        rig.export(a, Some(a));
        let graph = rig.builder.finish();

        let entry = graph
            .exported(a, rig.strings.intern("aName"))
            .expect("present");
        assert!(entry.is_local);
        assert_eq!(entry.file, a_file);
    }

    /// Local declarations beat re-exports: they are seeded first and
    /// propagation never overwrites (`buildInitialExportScope` declares
    /// before any flow; `ExportScope.export` never replaces an existing
    /// entry — export.dart:101-127; CFE source_library_builder.dart:430-431).
    #[test]
    fn local_declaration_beats_reexport() {
        let mut rig = Rig::new();
        let a = rig.builder.intern("package:a/a.dart");
        let lib_b = rig.builder.intern("package:b/b.dart");
        let a_file = rig.file("a.dart");
        let b_file = rig.file("b.dart");
        rig.declare(a, "shared", a_file);
        rig.declare(lib_b, "shared", b_file);
        rig.export(a, Some(lib_b));
        let graph = rig.builder.finish();

        let entry = graph
            .exported(a, rig.strings.intern("shared"))
            .expect("present");
        assert!(entry.is_local, "the local declaration must win");
        assert_eq!(entry.file, a_file);
    }

    /// Conflicting re-exports of two different declarations: the first flow
    /// in directive source order wins, the later is dropped silently
    /// (export.dart:118-121 — "Ambiguous declaration detected. if (existing
    /// != null) return false;"). Directive order decides, deterministically.
    #[test]
    fn conflicting_reexports_first_come_wins() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let mut builder = LibraryGraphBuilder::new(&strings);
        let src1 = builder.intern("package:s1/s1.dart");
        let src2 = builder.intern("package:s2/s2.dart");
        let top = builder.intern("package:t/t.dart");
        let f1 = files.intern(Path::new("s1.dart"));
        let f2 = files.intern(Path::new("s2.dart"));
        builder.declare(src1, strings.intern("dup"), f1);
        builder.declare(src2, strings.intern("dup"), f2);
        builder.export(
            top,
            ExportEdge {
                target: Some(src1),
                combinators: Combinators::unfiltered(),
            },
        );
        builder.export(
            top,
            ExportEdge {
                target: Some(src2),
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let entry = graph.exported(top, strings.intern("dup")).expect("present");
        assert_eq!(entry.file, f1, "the first directive's target wins");
    }

    /// Registration order beats directive order in a genuine race: with s2
    /// interned before s1 and `top` exporting s1 first, the oracle's initial
    /// pass sweeps exported builders in registration order
    /// (`link.dart:167-179` `for (var exported in exportedBuilders)`), so
    /// s2's declaration reaches `top` first and s1's loses — even though
    /// s1's export directive comes first.
    #[test]
    fn conflicting_reexports_follow_registration_order() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let mut builder = LibraryGraphBuilder::new(&strings);
        let s2 = builder.intern("package:s2/s2.dart"); // registered first
        let s1 = builder.intern("package:s1/s1.dart");
        let top = builder.intern("package:t/t.dart");
        let f2 = files.intern(Path::new("s2.dart"));
        let f1 = files.intern(Path::new("s1.dart"));
        builder.declare(s2, strings.intern("dup"), f2);
        builder.declare(s1, strings.intern("dup"), f1);
        builder.export(
            top,
            ExportEdge {
                target: Some(s1),
                combinators: Combinators::unfiltered(),
            },
        );
        builder.export(
            top,
            ExportEdge {
                target: Some(s2),
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let entry = graph.exported(top, strings.intern("dup")).expect("present");
        assert_eq!(
            entry.file, f2,
            "the registered-first source wins over the first directive"
        );
    }

    /// The propagation worklist drains LIFO
    /// (`additionalExportData.removeLast()`, `link.dart:196-206`): with every
    /// consumer registered before its source, the initial pass finds the mid
    /// scopes empty and both inserts happen in the `both` pass, enqueued as
    /// (z1, dup) then (z2, dup) in registration order. LIFO processing takes
    /// (z2, dup) first, so s2's declaration wins at `w`; FIFO would hand the
    /// race to s1.
    ///
    /// Chain: s1/s2 declare `dup`; x1 exports s1, x2 exports s2; z1 exports
    /// x1, z2 exports x2; w exports z1 then z2.
    #[test]
    fn propagation_worklist_drains_lifo() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let mut builder = LibraryGraphBuilder::new(&strings);
        let w = builder.intern("package:w/w.dart");
        let z1 = builder.intern("package:z1/z1.dart");
        let z2 = builder.intern("package:z2/z2.dart");
        let x1 = builder.intern("package:x1/x1.dart");
        let x2 = builder.intern("package:x2/x2.dart");
        let s1 = builder.intern("package:s1/s1.dart");
        let s2 = builder.intern("package:s2/s2.dart");
        let f1 = files.intern(Path::new("s1.dart"));
        let f2 = files.intern(Path::new("s2.dart"));
        builder.declare(s1, strings.intern("dup"), f1);
        builder.declare(s2, strings.intern("dup"), f2);
        for (from, target) in [(x1, s1), (x2, s2), (z1, x1), (z2, x2), (w, z1), (w, z2)] {
            builder.export(
                from,
                ExportEdge {
                    target: Some(target),
                    combinators: Combinators::unfiltered(),
                },
            );
        }
        let graph = builder.finish();

        let entry = graph.exported(w, strings.intern("dup")).expect("present");
        assert_eq!(
            entry.file, f2,
            "the LIFO head — z2, registered later — wins"
        );
        // The losing flow still filled its own mid scope.
        assert_eq!(
            graph.exported(x1, strings.intern("dup")).expect("mid").file,
            f1
        );
    }

    /// The same declaration arriving through two paths is one entry, not a
    /// conflict (same-reference dedupe, export.dart:111-116): a diamond
    /// `top → m/k → d` leaves exactly one `dName` entry on `top`.
    #[test]
    fn same_target_through_two_paths_is_one_entry() {
        let mut rig = Rig::new();
        let d = rig.builder.intern("package:d/d.dart");
        let m = rig.builder.intern("package:m/m.dart");
        let k = rig.builder.intern("package:k/k.dart");
        let top = rig.builder.intern("package:t/t.dart");
        let d_file = rig.file("d.dart");
        rig.declare(d, "dName", d_file);
        rig.export(m, Some(d));
        rig.export(k, Some(d));
        rig.export(top, Some(m));
        rig.export(top, Some(k));
        let graph = rig.builder.finish();

        let hits: Vec<_> = graph
            .export_scope(top)
            .filter(|(name, _)| *name == rig.strings.intern("dName"))
            .collect();
        assert_eq!(hits.len(), 1, "diamond re-export dedupes to one entry");
        assert_eq!(hits[0].1.file, d_file);
    }

    /// Private names never enter an export scope
    /// (`buildInitialExportScope` skips `_`-prefixed names,
    /// library_builder.dart:254-256).
    #[test]
    fn private_names_stay_unexported() {
        let mut rig = Rig::new();
        let a = rig.builder.intern("package:a/a.dart");
        let hidden_file = rig.file("a.dart");
        rig.declare(a, "_hidden", hidden_file);
        rig.declare(a, "shown", hidden_file);
        let graph = rig.builder.finish();

        assert!(graph.exported(a, rig.strings.intern("_hidden")).is_none());
        assert!(graph.exported(a, rig.strings.intern("shown")).is_some());
        assert_eq!(
            graph.export_scope(a).count(),
            1,
            "only the public declaration is in the face"
        );
    }

    /// Directive targets that are external/missing/not-a-library
    /// (`target: None`) contribute nothing — the oracle skips directives
    /// whose exported library is null (`addExporters` continue,
    /// library_builder.dart:128-131; import-side guard
    /// imports_verifier.dart:63-70).
    #[test]
    fn unresolved_targets_contribute_nothing() {
        let mut rig = Rig::new();
        let a = rig.builder.intern("package:a/a.dart");
        let a_file = rig.file("a.dart");
        rig.declare(a, "own", a_file);
        rig.export(a, None);
        rig.builder.import(a, import_edge(None));
        let graph = rig.builder.finish();

        assert_eq!(graph.export_scope(a).count(), 1);
        assert_eq!(graph.import_edges(a).len(), 1, "the edge is recorded");
    }

    /// The export face splits direct vs re-exported names via `is_local`.
    #[test]
    fn export_scope_splits_direct_vs_reexport() {
        let mut rig = Rig::new();
        let a = rig.builder.intern("package:a/a.dart");
        let lib_b = rig.builder.intern("package:b/b.dart");
        let a_file = rig.file("a.dart");
        let b_file = rig.file("b.dart");
        rig.declare(a, "direct", a_file);
        rig.declare(lib_b, "reexported", b_file);
        rig.export(a, Some(lib_b));
        let graph = rig.builder.finish();

        let strings = &rig.strings;
        let mut direct: Vec<bool> = Vec::new();
        let mut reexported: Vec<bool> = Vec::new();
        for (name, entry) in graph.export_scope(a) {
            if name == strings.intern("direct") {
                direct.push(entry.is_local);
            }
            if name == strings.intern("reexported") {
                reexported.push(entry.is_local);
            }
        }
        assert_eq!(direct, vec![true]);
        assert_eq!(reexported, vec![false]);
    }

    /// Import edges are recorded with prefix, deferred, and combinators for
    /// downstream consumption (§2.5: deferred is recorded, never evaluated).
    #[test]
    fn import_edges_record_their_directive_state() {
        let strings = StringTable::new();
        let mut builder = LibraryGraphBuilder::new(&strings);
        let a = builder.intern("package:a/a.dart");
        let prefix = strings.intern("p");
        builder.import(
            a,
            ImportEdge {
                target: None,
                prefix: Some(prefix),
                deferred: true,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let edge = &graph.import_edges(a)[0];
        assert!(edge.deferred);
        assert_eq!(edge.prefix, Some(prefix));
        assert!(edge.target.is_none());
    }
}
