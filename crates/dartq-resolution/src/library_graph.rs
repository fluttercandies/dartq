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
//! `target: None` — it contributes no names, matching the oracle's name-table
//! filter (`PrefixScope` admits only `DirectiveUriWithLibraryImpl` imports,
//! `element/scope.dart:530-535`); an import-side `as` prefix still registers
//! with an empty table downstream (the namespace layer's contract). The §4.2
//! codes for such directives stay with the caller's
//! [`crate::UriResolution`]; this layer only computes namespaces — except
//! for the directive-combinator static warnings
//! ([`LibraryGraph::undefined_combinator_names`]), whose emit-side decision
//! is a pure export-scope query over a resolved edge.
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

use dartq_source::{FileId, SourceRange, StringId, StringTable};
use rustc_hash::{FxBuildHasher, FxHashMap};
use std::collections::hash_map;

use crate::modifiers::ClassModifiers;

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

/// One written combinator token, captured at directive construction: the
/// clause kind it was written under, its bare identifier, and the token's
/// UTF-16 span as captured. The written sequence — duplicates preserved, in
/// written order — is what [`LibraryGraph::undefined_combinator_names`]
/// iterates, one report per written token (the oracle's
/// `dead_code_verifier.dart:148-158` walks the AST name list as written).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WrittenName {
    name: StringId,
    hidden: bool,
    span: SourceRange,
}

/// The ordered combinator list of one directive; empty means unfiltered.
///
/// Alongside the deduplicated clause sets it carries the two derived faces
/// the decision layers consume: the **written** token sequence
/// ([`LibraryGraph::undefined_combinator_names`]'s iteration source) and the
/// interned setter-spelling pairing (the undefined check's setter probe —
/// derived where the constructor expands the spellings, so the query pays
/// no string work).
#[derive(Clone, Debug, Default)]
pub struct Combinators {
    clauses: Box<[CombinatorClause]>,
    written: Box<[WrittenName]>,
    setters: Box<[(StringId, StringId)]>,
}

impl Combinators {
    /// Builds the combinator list from source-ordered clauses, expanding each
    /// clause's names with their `name=` setter spellings (interned into
    /// `strings`) and sorting for binary-search membership. Clause names are
    /// bare identifiers (combinator grammar has no `=`); a written set is
    /// taken as given.
    ///
    /// The written sequence is synthesized from the clause input (order and
    /// duplicates preserved); the spans are the zero-length range at offset 0
    /// — this constructor receives no capture. The span-carrying constructor
    /// is [`Self::with_spans`].
    pub fn new(clauses: impl IntoIterator<Item = CombinatorClause>, strings: &StringTable) -> Self {
        let clauses: Vec<CombinatorClause> = clauses.into_iter().collect();
        let written: Vec<WrittenName> = clauses
            .iter()
            .flat_map(|clause| {
                let (names, hidden) = clause_names(clause);
                names.iter().copied().map(move |name| WrittenName {
                    name,
                    hidden,
                    span: SourceRange {
                        offset: 0,
                        length: 0,
                    },
                })
            })
            .collect();
        Self::build(clauses, written, strings)
    }

    /// [`Self::new`] with the written tokens' captured spans: each clause
    /// rides with a span list parallel to its names (built from the same
    /// capture loop, so the pairing cannot desync). The spans land on the
    /// yielded [`UndefinedCombinatorName`]s — the report anchors.
    ///
    /// # Panics
    ///
    /// Panics when a clause's span list is not exactly as long as its name
    /// list — a caller capture bug, surfaced rather than misaligned silently.
    pub fn with_spans(
        clauses: impl IntoIterator<Item = (CombinatorClause, Box<[SourceRange]>)>,
        strings: &StringTable,
    ) -> Self {
        let clauses: Vec<(CombinatorClause, Box<[SourceRange]>)> = clauses.into_iter().collect();
        let written: Vec<WrittenName> = clauses
            .iter()
            .flat_map(|(clause, spans)| {
                let (names, hidden) = clause_names(clause);
                assert_eq!(
                    names.len(),
                    spans.len(),
                    "combinator capture: span list must parallel the clause's names"
                );
                names
                    .iter()
                    .copied()
                    .zip(spans.iter().copied())
                    .map(move |(name, span)| WrittenName { name, hidden, span })
            })
            .collect();
        let clauses = clauses.into_iter().map(|(clause, _)| clause).collect();
        Self::build(clauses, written, strings)
    }

    /// The unfiltered list — the no-combinator common case.
    #[must_use]
    pub fn unfiltered() -> Self {
        Self {
            clauses: Box::new([]),
            written: Box::new([]),
            setters: Box::new([]),
        }
    }

    /// Shared assembly: expand the clause sets for membership, and derive the
    /// setter pairing from the written tokens (the intern of each spelling
    /// hash-hits [`expand_names`]' work — the pairing just records it).
    fn build(
        clauses: Vec<CombinatorClause>,
        written: impl IntoIterator<Item = WrittenName>,
        strings: &StringTable,
    ) -> Self {
        let written: Box<[WrittenName]> = written.into_iter().collect();
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
        let mut setters: Vec<(StringId, StringId)> = Vec::new();
        for token in &written {
            if setters.iter().any(|(bare, _)| *bare == token.name) {
                continue;
            }
            let setter = strings.intern(&format!("{}=", strings.resolve(token.name)));
            setters.push((token.name, setter));
        }
        Self {
            clauses,
            written,
            setters: setters.into_boxed_slice(),
        }
    }

    /// The written token sequence — `(name, hidden, span)` per token, in
    /// written order, duplicates preserved. The facts-snapshot projection
    /// face (#539): the expanded clause sets this struct stores are sorted
    /// and deduplicated, so the written surface is unreadable from them;
    /// the payload carries this sequence instead and rebuilds through
    /// [`Self::with_spans`], keeping a warm start's undefined-combinator
    /// reports on the cold capture's anchors.
    pub fn written(&self) -> impl Iterator<Item = (StringId, bool, SourceRange)> + '_ {
        self.written
            .iter()
            .map(|token| (token.name, token.hidden, token.span))
    }

    /// The interned `name=` spelling of the written bare identifier `name`,
    /// derived at construction ([`Self::build`]) — the undefined check's
    /// setter probe face, so the query pays no string work. `setters`
    /// derives from the written tokens, so every written name is present.
    pub(crate) fn setter_spelling(&self, name: StringId) -> Option<StringId> {
        self.setters
            .iter()
            .find(|(bare, _)| *bare == name)
            .map(|(_, setter)| *setter)
    }

    /// Whether `name` survives this directive's combinators, applied in
    /// source order (`summary2/combinator.dart` `allows`: a `show` the name
    /// is not in rejects it; a `hide` containing it rejects it; later
    /// clauses see what earlier clauses left).
    #[must_use]
    pub fn allows(&self, name: StringId) -> bool {
        self.clauses.iter().all(|clause| match clause {
            CombinatorClause::Show(names) => names.binary_search(&name).is_ok(),
            CombinatorClause::Hide(names) => names.binary_search(&name).is_err(),
        })
    }

    /// Whether the directive carries a `show` clause (the oracle's
    /// `showCombinators.isEmpty` test, `element.dart:8229`).
    #[must_use]
    pub fn has_show(&self) -> bool {
        self.clauses
            .iter()
            .any(|clause| matches!(clause, CombinatorClause::Show(_)))
    }

    /// Whether any `show` clause lists `name` — the undefined-guard
    /// suppression membership (`element.dart:8234-8238`
    /// `combinator.shownNames.contains(name)`, consulted per show
    /// combinator; `hide` clauses never qualify a name). Written names plus
    /// their setter spellings; guard queries are bare identifiers, so the
    /// extra spellings are unreachable.
    #[must_use]
    pub fn shows(&self, name: StringId) -> bool {
        self.clauses.iter().any(|clause| {
            matches!(clause, CombinatorClause::Show(names) if names.binary_search(&name).is_ok())
        })
    }
}

/// A clause's name list with its kind — the shared read of both variants
/// (`false` for `show`, `true` for `hide`).
fn clause_names(clause: &CombinatorClause) -> (&[StringId], bool) {
    match clause {
        CombinatorClause::Show(names) => (names, false),
        CombinatorClause::Hide(names) => (names, true),
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
/// no names, while its `as` prefix still registers with an empty table (the
/// oracle sweeps every import's prefix with no resolution gating,
/// `element.dart:7529-7536`; the name-table filter is
/// `DirectiveUriWithLibraryImpl`, `scope.dart:530-535`).
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

/// One undefined directive-combinator name ([`LibraryGraph::
/// undefined_combinator_names`]): one report per **written** `show`/`hide`
/// token the directive target's export scope defines under neither spelling
/// — the oracle iterates the AST name list as written and reports at each
/// node (`dead_code_verifier.dart:148-158`), so a duplicated written name
/// yields duplicated reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UndefinedCombinatorName {
    /// The written (bare) identifier — the message's `{1}` argument.
    pub name: StringId,
    /// `true` for a `hide` clause (`undefined_hidden_name`), `false` for
    /// `show` (`undefined_shown_name`).
    pub hidden: bool,
    /// The written token's captured UTF-16 span — the report anchor (the
    /// oracle anchors `atNode(name)`, `dead_code_verifier.dart:153-158`).
    /// Zero-length at offset 0 when the capture carried no spans
    /// ([`Combinators::new`]).
    pub span: SourceRange,
}

/// One spelling's slot in a library's locals layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LocalSlot {
    /// The spelling is locally declared: its face's declaring file.
    Decl(FileId),
    /// The coupled spelling of a locally declared base name (the getter
    /// spelling when only the setter is declared, or vice versa): imports
    /// are suppressed for it, but it has no local face of its own.
    ShadowOnly,
}

/// The locals-link outcome for one spelling — the first link of the
/// namespace scope chain ([`crate::namespace`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalLookup {
    /// The spelling is locally declared.
    Found(FileId),
    /// The base name has a local face but not this spelling's: the whole
    /// base name shadows imports (the oracle consults the name-coupled
    /// getter/setter pair and never reaches the import scope when either
    /// face is non-null, `element/scope.dart:419-433/473-486`), so the
    /// lookup stops here with no target.
    Shadowed,
    /// No local declaration under the base name; import lookup proceeds.
    Absent,
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
    /// Class declarations with their modifier flags, keyed by name — the
    /// cross-library modifier facts ([`LibraryGraph::class_modifiers`]).
    classes: Vec<(StringId, ClassModifiers)>,
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
            classes: Vec::new(),
        });
        id
    }

    /// Records one top-level declaration `name` of `lib`, declared in `file`
    /// (the library file or one of its parts; folding the part tree is the
    /// caller's job). The declaration enters both faces: the locals layer
    /// ([`LibraryGraph::local`], privates included — the oracle's locals
    /// table has no `_` filter, `element/scope.dart:272-289`) and the export
    /// face, where privates are dropped at seeding (the oracle's `_` filter
    /// lives only in `buildInitialExportScope`,
    /// `library_builder.dart:252-262`). Duplicate names keep the first: a
    /// library's members have unique names by construction, and duplicates
    /// are the semantic layer's diagnosis, not a namespace-shaping event.
    pub fn declare(&mut self, lib: LibraryId, name: StringId, file: FileId) {
        self.nodes[lib.index() as usize].decls.push((name, file));
    }

    /// Records one class declaration `name` of `lib` with its declared
    /// modifier flags ([`ClassModifiers`]) — the M2 modifier facts the
    /// cross-library restriction decisions consume. The name/file pair is
    /// registered exactly like [`Self::declare`] (one call covers both
    /// faces), so a class is never a namespace ghost.
    pub fn declare_class(
        &mut self,
        lib: LibraryId,
        name: StringId,
        file: FileId,
        modifiers: ClassModifiers,
    ) {
        self.declare(lib, name, file);
        self.nodes[lib.index() as usize]
            .classes
            .push((name, modifiers));
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
        let strings = self.strings;
        // The wildcard id: a prefix registered under `_` is gated out of the
        // bare-name prefix branch (`_shouldTryPrefixElement`,
        // `element/scope.dart:486-494`; `wildcard_variables` is default-on
        // and pinned here).
        let underscore = strings.intern("_");

        // Seed: locals first, so no propagation can ever displace them;
        // `_`-private names stay out of the export face (the oracle's
        // `buildInitialExportScope` filter, `library_builder.dart:252-262`)
        // while entering the locals layer.
        let mut scopes: Vec<FxHashMap<StringId, ExportEntry>> =
            Vec::with_capacity(self.nodes.len());
        let mut locals: Vec<FxHashMap<StringId, LocalSlot>> = Vec::with_capacity(self.nodes.len());
        for node in &self.nodes {
            let mut scope = FxHashMap::default();
            for (name, file) in &node.decls {
                if strings.resolve(*name).starts_with('_') {
                    continue;
                }
                // Deliberate deviation: first-wins, where the oracle's
                // `ExportScope.declare` unconditionally overwrites
                // (summary2/export.dart:97-100, last-wins) — the divergence
                // only manifests in already-erroneous duplicate
                // declarations, which the spec pins as
                // non-namespace-shaping.
                scope.entry(*name).or_insert(ExportEntry {
                    file: *file,
                    is_local: true,
                });
            }
            scopes.push(scope);
            locals.push(build_locals(&node.decls, strings));
        }

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

        // The name → declaring-libraries inverted index over class
        // declarations ([`LibraryGraph::declaring_libraries`]). Duplicates
        // keep the first, matching [`LibraryGraphBuilder::declare`]'s
        // first-wins contract.
        let mut declarations: FxHashMap<StringId, Vec<LibraryId>> = FxHashMap::default();
        let nodes = nodes
            .into_iter()
            .zip(scopes)
            .zip(locals)
            .enumerate()
            .map(|(index, ((node, export_scope), local_slots))| {
                let wildcard_prefix = node
                    .imports
                    .iter()
                    .any(|edge| edge.prefix == Some(underscore))
                    .then_some(underscore);
                let lib = LibraryId::from_index(index);
                let mut classes: FxHashMap<StringId, ClassModifiers> =
                    FxHashMap::with_capacity_and_hasher(node.classes.len(), FxBuildHasher);
                for (name, modifiers) in node.classes {
                    classes.entry(name).or_insert(modifiers);
                    let owners = declarations.entry(name).or_default();
                    if !owners.contains(&lib) {
                        owners.push(lib);
                    }
                }
                LibraryNode {
                    uri: node.uri,
                    imports: node.imports,
                    exports: node.exports,
                    export_scope,
                    locals: local_slots,
                    wildcard_prefix,
                    classes,
                }
            })
            .collect();
        LibraryGraph {
            nodes,
            declarations,
        }
    }
}

/// Builds one library's locals layer: per-spelling slots with precomputed
/// base-name coupling. The oracle keys the getter/setter pair of a base name
/// into shared maps — `_addSetter` strips the trailing `=` from the lookup
/// name (`element/scope.dart:801-806`) — and consults the pair wholesale
/// before any import (`_lookupLibrary`, scope.dart:473-486, all-or-nothing
/// per base name); dartq keeps per-spelling interned keys and materializes
/// the coupling as [`LocalSlot::ShadowOnly`] markers, so a lookup is a
/// single hash hit on interned ids. Privates are included: the oracle's
/// locals table has no `_` filter (`LibraryDeclarations`,
/// scope.dart:272-289).
fn build_locals(
    decls: &[(StringId, FileId)],
    strings: &StringTable,
) -> FxHashMap<StringId, LocalSlot> {
    let mut slots: FxHashMap<StringId, LocalSlot> = FxHashMap::default();
    for &(name, file) in decls {
        match slots.entry(name) {
            hash_map::Entry::Vacant(slot) => {
                slot.insert(LocalSlot::Decl(file));
            }
            // A coupling marker yields to the real declaration; a duplicate
            // declaration keeps the first (the oracle's `_getters[id] ??=`,
            // scope.dart:797-806).
            hash_map::Entry::Occupied(mut slot) if matches!(slot.get(), LocalSlot::ShadowOnly) => {
                slot.insert(LocalSlot::Decl(file));
            }
            hash_map::Entry::Occupied(_) => {}
        }
        // Either local face suppresses import lookup for BOTH spellings of
        // the base name: register the coupled spelling as a shadow marker.
        let text = strings.resolve(name);
        let coupled = match text.strip_suffix('=') {
            Some(base) => strings.intern(base),
            None => strings.intern(&format!("{text}=")),
        };
        slots.entry(coupled).or_insert(LocalSlot::ShadowOnly);
    }
    slots
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
    // First-come-wins insert (`export.dart:105-127`): the vacant branch is
    // the hot path and pays a single probe — no `contains_key` + `insert`
    // double lookup.
    match scopes[flow.to.index() as usize].entry(name) {
        hash_map::Entry::Vacant(slot) => {
            slot.insert(ExportEntry {
                file,
                is_local: false,
            });
            true
        }
        hash_map::Entry::Occupied(_) => false, // locals and earlier flows stay
    }
}

/// The frozen library graph: interned nodes with directive edges and computed
/// export scopes. Each node's `uri` is the single owner of the canonical
/// identity string (the builder's interning map is discarded here).
pub struct LibraryGraph {
    nodes: Vec<LibraryNode>,
    /// Class-declaration name → declaring libraries, in library
    /// registration order ([`LibraryGraph::declaring_libraries`]).
    declarations: FxHashMap<StringId, Vec<LibraryId>>,
}

struct LibraryNode {
    uri: Box<str>,
    imports: Vec<ImportEdge>,
    exports: Vec<ExportEdge>,
    export_scope: FxHashMap<StringId, ExportEntry>,
    /// The locals layer: per-spelling slots with base-name coupling,
    /// privates included ([`LibraryGraph::local`]).
    locals: FxHashMap<StringId, LocalSlot>,
    /// The interned `_` when one of `imports` registers that prefix.
    wildcard_prefix: Option<StringId>,
    /// Class declarations with their modifier flags
    /// ([`LibraryGraph::class_modifiers`]).
    classes: FxHashMap<StringId, ClassModifiers>,
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

    /// Every library in the graph — described libraries and nodes registered
    /// only as directive-edge targets alike (the latter export nothing, see
    /// the module contracts). Creation is O(1) with no allocation, each
    /// library is yielded exactly once, and the iteration order is
    /// unspecified: callers must not depend on it.
    pub fn libraries(&self) -> impl Iterator<Item = LibraryId> {
        (0..self.nodes.len()).map(LibraryId::from_index)
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

    /// The directive-combinator findings for one resolved directive edge:
    /// every **written** `show`/`hide` token the target's export scope does
    /// not define — the `undefined_hidden_name`/`undefined_shown_name`
    /// `STATIC_WARNING` pair ([`crate::codes::UNDEFINED_HIDDEN_NAME`]/[
    /// `crate::codes::UNDEFINED_SHOWN_NAME`]). One entry per written token,
    /// in written order, each carrying its captured span: the oracle
    /// iterates `combinator.shownNames`/`hiddenNames` as written and reports
    /// at each node (`dead_code_verifier.dart:148-158`), so a duplicated
    /// written name yields duplicated reports.
    ///
    /// Decision per token mirrors the oracle exactly. The namespace is the
    /// target library's export scope — `element_resolver.dart:407-432`
    /// `_resolveCombinators` resolves each written name against
    /// `library.exportNamespace` through both spellings (`get2(name)`,
    /// `get2('name=')`), and `dead_code_verifier.dart:141-158`
    /// `_checkCombinator` reports only when both faces come back null.
    /// dartq's export scope is that namespace with setters under their
    /// `name=` spelling, so the probe is `exported(target, name)` then —
    /// through the construction-derived setter pairing, no string work —
    /// `exported(target, setter)`. Each token decides independently; an
    /// `as` prefix or `deferred` is not consulted
    /// (`dead_code_verifier.dart:77-113` checks every import and export
    /// directive's combinators alike).
    ///
    /// `target` is the directive's resolved target library, passed as the
    /// edge carries it: `None` (invalid URI, missing file, not a library)
    /// yields nothing — the oracle's `importedLibrary == null ||
    /// library.isOriginNotExistingFile` gate (`dead_code_verifier.dart:104-
    /// 112`; the missing target is the reported root cause and replaces the
    /// combinator symptom). Synthetic `dart:` face nodes decide from the
    /// names they carry, exactly the corpus fixtures' shape.
    ///
    /// Message arguments for the caller: `{1}` is the written name's text;
    /// `{0}` is the oracle's `${library.uri}` — `dart:<name>` for SDK faces
    /// (the synthetic node's uri spelling is exactly that), but the
    /// defining file's `file:///` URI for analysis-set libraries, which is
    /// *not* a package-lib node's `package:` identity spelling (oracle runs
    /// print `file:///…` there) — render `{0}` from the target's file path,
    /// not [`Self::uri`].
    pub fn undefined_combinator_names<'a>(
        &'a self,
        target: Option<LibraryId>,
        combinators: &'a Combinators,
    ) -> impl Iterator<Item = UndefinedCombinatorName> + 'a {
        target.into_iter().flat_map(move |target| {
            combinators.written.iter().filter_map(move |token| {
                let defined = self.exported(target, token.name).is_some()
                    || combinators
                        .setter_spelling(token.name)
                        .is_some_and(|setter| self.exported(target, setter).is_some());
                (!defined).then(|| UndefinedCombinatorName {
                    name: token.name,
                    hidden: token.hidden,
                    span: token.span,
                })
            })
        })
    }

    /// The locals link of the scope chain for `name` (either spelling): what
    /// `lib` itself declares, before any import is consulted. The oracle
    /// couples the two faces of a base name — `_lookupLibrary` hands back
    /// the (getter, setter) pair and `_lookupCombined` is never consulted
    /// when either face is non-null (`element/scope.dart:419-433/455-471`),
    /// with setters keyed by the base name (`_addSetter` strips the trailing
    /// `=`, scope.dart:801-806). dartq precomputes that coupling per
    /// spelling ([`LocalSlot`]), keeping the check a single hash hit.
    #[must_use]
    pub fn local(&self, lib: LibraryId, name: StringId) -> LocalLookup {
        match self.nodes[lib.index() as usize].locals.get(&name) {
            None => LocalLookup::Absent,
            Some(LocalSlot::Decl(file)) => LocalLookup::Found(*file),
            Some(LocalSlot::ShadowOnly) => LocalLookup::Shadowed,
        }
    }

    /// The interned `_` when `lib` registers a prefix of that name — the
    /// wildcard-variable gate of the bare-name prefix branch
    /// ([`crate::namespace`]; `_shouldTryPrefixElement`,
    /// `element/scope.dart:486-494`).
    pub(crate) fn wildcard_prefix(&self, lib: LibraryId) -> Option<StringId> {
        self.nodes[lib.index() as usize].wildcard_prefix
    }

    /// The declared modifier flags of `lib`'s class `name` — the
    /// declaration-side fact for the M2 cross-library restriction
    /// decisions ([`crate::modifiers`]). `None` when `lib` does not
    /// declare a class under that name (or registered only a
    /// non-class declaration).
    #[must_use]
    pub fn class_modifiers(&self, lib: LibraryId, name: StringId) -> Option<ClassModifiers> {
        self.nodes[lib.index() as usize].classes.get(&name).copied()
    }

    /// Every library that declares a *class* under `name`, in library
    /// registration order. The ownership half of the cross-library
    /// restriction facts: the semantic layer pairs a use site's library
    /// with each candidate's home via this face (duplicates across
    /// libraries are the ordinary ambiguity case — the same-library
    /// flag is per candidate). `FxBuildHasher`-backed; one hash hit per
    /// query.
    #[must_use]
    pub fn declaring_libraries(&self, name: StringId) -> &[LibraryId] {
        self.declarations
            .get(&name)
            .map(Vec::as_slice)
            .unwrap_or(&[])
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

    /// One-clause combinator list over `names` — `hidden: true` builds
    /// `hide`, else `show`. Built through `with_spans` with synthetic
    /// per-token spans (offset `4 * index`, length 2) so the capture path
    /// — not just the span-less constructor — is what every decision pin
    /// exercises.
    fn combinators(hidden: bool, names: &[&str], strings: &StringTable) -> Combinators {
        let ids: Vec<StringId> = names.iter().map(|n| strings.intern(n)).collect();
        let spans: Box<[SourceRange]> = (0..ids.len())
            .map(|index| SourceRange {
                offset: (4 * index) as u32,
                length: 2,
            })
            .collect();
        let clause = if hidden {
            CombinatorClause::Hide(ids.into())
        } else {
            CombinatorClause::Show(ids.into())
        };
        Combinators::with_spans([(clause, spans)], strings)
    }

    /// The undefined-combinator findings as (name text, hidden) pairs.
    fn findings(
        graph: &LibraryGraph,
        strings: &StringTable,
        target: Option<LibraryId>,
        combinators: &Combinators,
    ) -> Vec<(String, bool)> {
        graph
            .undefined_combinator_names(target, combinators)
            .map(|found| (strings.resolve(found.name).to_owned(), found.hidden))
            .collect()
    }

    // ---- directive-combinator diagnostics ----

    /// The corpus fixtures' shapes: a combinator naming a member the target's
    /// export face does not define reports with the clause's code face —
    /// `hide` as `undefined_hidden_name`, `show` as `undefined_shown_name` —
    /// while a defined name stays silent on both clause kinds.
    #[test]
    fn combinator_misses_report_per_clause_and_defined_names_stay_silent() {
        let mut rig = Rig::new();
        let math = rig.builder.intern("dart:math");
        let face_file = rig.file("<dart:math>");
        rig.declare(math, "max", face_file);
        rig.declare(math, "min", face_file);
        let strings = rig.strings;
        let graph = rig.builder.finish();

        assert_eq!(
            findings(
                &graph,
                strings,
                Some(math),
                &combinators(true, &["noSuchName"], strings)
            ),
            [("noSuchName".to_owned(), true)]
        );
        assert_eq!(
            findings(
                &graph,
                strings,
                Some(math),
                &combinators(false, &["noSuchName"], strings)
            ),
            [("noSuchName".to_owned(), false)]
        );
        let defined = Combinators::new(
            [
                CombinatorClause::Show(Box::from([strings.intern("max")])),
                CombinatorClause::Hide(Box::from([strings.intern("min")])),
            ],
            strings,
        );
        assert!(findings(&graph, strings, Some(math), &defined).is_empty());
    }

    /// Per-token reporting: the oracle iterates the written name list and
    /// reports at each node (`dead_code_verifier.dart:148-158` — `for
    /// (SimpleIdentifier name in names)` with `_diagnosticReporter.atNode(
    /// name, ...)`), so `show noSuch, noSuch` on an undefined name is TWO
    /// findings, in written order, each anchored at its own token's span.
    #[test]
    fn duplicate_written_names_report_per_token() {
        let mut rig = Rig::new();
        let math = rig.builder.intern("dart:math");
        let face_file = rig.file("<dart:math>");
        rig.declare(math, "max", face_file);
        let strings = rig.strings;
        let graph = rig.builder.finish();

        let no_such = strings.intern("noSuch");
        let spans = [
            SourceRange {
                offset: 18,
                length: 6,
            },
            SourceRange {
                offset: 26,
                length: 6,
            },
        ];
        let combinators = Combinators::with_spans(
            [(
                CombinatorClause::Show(Box::from([no_such, no_such])),
                Box::from(spans.as_slice()),
            )],
            strings,
        );
        let found: Vec<UndefinedCombinatorName> = graph
            .undefined_combinator_names(Some(math), &combinators)
            .collect();
        assert_eq!(
            found,
            [
                UndefinedCombinatorName {
                    name: no_such,
                    hidden: false,
                    span: spans[0],
                },
                UndefinedCombinatorName {
                    name: no_such,
                    hidden: false,
                    span: spans[1],
                },
            ],
            "one finding per written token, written order, per-token spans"
        );
    }

    /// The oracle checks export directives' combinators identically
    /// (`visitExportDirective`, `dead_code_verifier.dart:77-89`): an export
    /// edge's `show` of an unexported name reports the shown-name warning.
    #[test]
    fn export_directive_combinators_are_checked() {
        let mut rig = Rig::new();
        let math = rig.builder.intern("dart:math");
        let face_file = rig.file("<dart:math>");
        rig.declare(math, "max", face_file);
        let consumer = rig.builder.intern("package:c/c.dart");
        let strings = rig.strings;
        rig.builder.export(
            consumer,
            ExportEdge {
                target: Some(math),
                combinators: combinators(false, &["noSuch"], strings),
            },
        );
        let graph = rig.builder.finish();

        let edge = &graph.export_edges(consumer)[0];
        assert_eq!(
            findings(&graph, strings, edge.target, &edge.combinators),
            [("noSuch".to_owned(), false)]
        );
    }

    /// A name defined only as a setter is defined for the check (the
    /// both-spellings probe, `element_resolver.dart:429-431`:
    /// `get2(name)` / `get2('name=')`), and the constructor's `name=`
    /// expansions are never reported — an empty face yields each written
    /// name once, not its setter spelling too.
    #[test]
    fn setter_face_defines_and_expansions_never_report() {
        let mut rig = Rig::new();
        let target = rig.builder.intern("package:t/t.dart");
        let t_file = rig.file("t.dart");
        rig.declare(target, "foo=", t_file); // setter-only top-level
        let empty = rig.builder.intern("package:e/e.dart"); // exports nothing
        let strings = rig.strings;
        let graph = rig.builder.finish();

        assert!(
            findings(
                &graph,
                strings,
                Some(target),
                &combinators(false, &["foo"], strings)
            )
            .is_empty()
        );
        assert_eq!(
            findings(
                &graph,
                strings,
                Some(empty),
                &combinators(true, &["max"], strings)
            ),
            [("max".to_owned(), true)]
        );
    }

    /// Suppression and interplay per the oracle: an unresolved target
    /// (`None` — the `importedLibrary == null ||
    /// library.isOriginNotExistingFile` gate) yields nothing; clauses decide
    /// independently in source order (`show noSuch hide noSuch` reports both
    /// codes); an `as` prefix is not consulted (`dead_code_verifier.dart:
    /// 100-113` checks prefixed imports' combinators alike).
    #[test]
    fn suppression_clause_interplay_and_prefix_insensitivity() {
        let mut rig = Rig::new();
        let math = rig.builder.intern("dart:math");
        let face_file = rig.file("<dart:math>");
        rig.declare(math, "max", face_file);
        let consumer = rig.builder.intern("package:c/c.dart");
        let strings = rig.strings;
        let both = Combinators::new(
            [
                CombinatorClause::Show(Box::from([strings.intern("noSuch")])),
                CombinatorClause::Hide(Box::from([strings.intern("noSuch")])),
            ],
            strings,
        );
        let mixed = Combinators::new(
            [
                CombinatorClause::Show(Box::from([strings.intern("max")])),
                CombinatorClause::Hide(Box::from([strings.intern("noSuch")])),
            ],
            strings,
        );
        rig.builder.import(
            consumer,
            ImportEdge {
                target: Some(math),
                prefix: Some(strings.intern("m")),
                deferred: false,
                combinators: both.clone(),
            },
        );
        let graph = rig.builder.finish();

        assert!(findings(&graph, strings, None, &both).is_empty());
        assert_eq!(
            findings(&graph, strings, Some(math), &both),
            [("noSuch".to_owned(), false), ("noSuch".to_owned(), true)]
        );
        assert_eq!(
            findings(&graph, strings, Some(math), &mixed),
            [("noSuch".to_owned(), true)]
        );
        // The prefixed edge decides identically — the prefix is not consulted.
        let edge = &graph.import_edges(consumer)[0];
        assert_eq!(
            findings(&graph, strings, edge.target, &edge.combinators),
            [("noSuch".to_owned(), false), ("noSuch".to_owned(), true)]
        );
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

    /// The undefined-guard show face (`shouldIgnoreUndefined`,
    /// element.dart:8217-8250): only `show` clauses count — unfiltered and
    /// hide-only directives have no show list, membership is per-clause
    /// containment (`show foo hide foo` still shows for the guard even
    /// though it no longer admits through `allows`).
    #[test]
    fn show_face_answers_membership_and_presence() {
        let strings = StringTable::new();
        let foo = strings.intern("foo");
        let bar = strings.intern("bar");

        let unfiltered = Combinators::unfiltered();
        assert!(!unfiltered.has_show());
        assert!(!unfiltered.shows(foo));

        let hide_only = Combinators::new([CombinatorClause::Hide(Box::from([foo]))], &strings);
        assert!(!hide_only.has_show());
        assert!(!hide_only.shows(foo));

        let show_foo = Combinators::new([CombinatorClause::Show(Box::from([foo]))], &strings);
        assert!(show_foo.has_show());
        assert!(show_foo.shows(foo));
        assert!(!show_foo.shows(bar));

        let shown_then_hidden = Combinators::new(
            [
                CombinatorClause::Show(Box::from([foo])),
                CombinatorClause::Hide(Box::from([foo])),
            ],
            &strings,
        );
        assert!(shown_then_hidden.shows(foo));
        assert!(!shown_then_hidden.allows(foo));
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
        assert_eq!(
            graph.local(a, rig.strings.intern("cName")),
            LocalLookup::Absent,
            "a re-exported name is not a local face"
        );
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

    /// Export-side combinator filtering (`apply_flow`'s `allows` gate — the
    /// fixpoint's per-name filter, `summary2/export.dart:22-28`): an
    /// `export 'x.dart' show Foo;`-shaped edge flows only the shown name
    /// into the exporter's face — the hidden name never enters the scope —
    /// and the shown one arrives as a re-export (`is_local: false`),
    /// mirroring the import-side `import_combinators_filter_at_build`.
    #[test]
    fn export_combinators_filter_at_propagation() {
        let mut rig = Rig::new();
        let target = rig.builder.intern("package:t/t.dart");
        let exporter = rig.builder.intern("package:e/e.dart");
        let t_file = rig.file("t.dart");
        rig.declare(target, "shown", t_file);
        rig.declare(target, "hidden", t_file);
        let strings = rig.strings;
        let show_only = Combinators::new(
            [CombinatorClause::Show(Box::from([strings.intern("shown")]))],
            strings,
        );
        rig.builder.export(
            exporter,
            ExportEdge {
                target: Some(target),
                combinators: show_only,
            },
        );
        let graph = rig.builder.finish();

        assert!(
            graph.exported(exporter, strings.intern("hidden")).is_none(),
            "the hidden name never enters the exporter's scope"
        );
        let entry = graph
            .exported(exporter, strings.intern("shown"))
            .expect("the shown name flows through");
        assert_eq!(entry.file, t_file);
        assert!(!entry.is_local, "carried as a re-export");
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
    /// library_builder.dart:254-256) but stay reachable in the locals layer.
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

    /// The locals layer couples the two spellings of a base name and keeps
    /// privates: a local getter shadows the setter spelling (and vice versa)
    /// without contributing a face for it — the oracle's `_lookupLibrary`
    /// pair, all-or-nothing per base name (element/scope.dart:473-486;
    /// `_addSetter` keys setters by the base name, scope.dart:801-806) — and
    /// `_`-private top-levels resolve same-library (`LibraryDeclarations`
    /// has no `_` filter, scope.dart:272-289).
    #[test]
    fn local_layer_couples_faces_and_keeps_privates() {
        let mut rig = Rig::new();
        let a = rig.builder.intern("package:a/a.dart");
        let a_file = rig.file("a.dart");
        let b = rig.builder.intern("package:b/b.dart");
        let b_file = rig.file("b.dart");
        rig.declare(a, "foo", a_file); // getter only
        rig.declare(b, "_helper", b_file); // private
        let graph = rig.builder.finish();

        let strings = &rig.strings;
        assert_eq!(
            graph.local(a, strings.intern("foo")),
            LocalLookup::Found(a_file)
        );
        assert_eq!(
            graph.local(a, strings.intern("foo=")),
            LocalLookup::Shadowed,
            "imports suppressed, but no local setter face"
        );
        assert_eq!(
            graph.local(b, strings.intern("_helper")),
            LocalLookup::Found(b_file),
            "privates resolve same-library"
        );
        assert_eq!(graph.local(a, strings.intern("bar")), LocalLookup::Absent);
    }

    /// Directive targets that are external/missing/not-a-library
    /// (`target: None`) contribute nothing — the oracle skips directives
    /// whose exported library is null (`addExporters` continue,
    /// library_builder.dart:128-131; the import-side name-table filter is
    /// `DirectiveUriWithLibraryImpl`, scope.dart:530-535).
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

    // ---- library enumeration ----

    /// `libraries` enumerates every interned node — declared libraries and
    /// edge-target-only nodes alike — exactly once per walk, stable across
    /// repeated walks, with each id answering the per-library faces.
    #[test]
    fn libraries_enumerate_every_node_exactly_once() {
        let mut rig = Rig::new();
        let a = rig.builder.intern("package:a/a.dart");
        let lib_b = rig.builder.intern("package:b/b.dart");
        let c = rig.builder.intern("package:c/c.dart"); // edge target only
        let a_file = rig.file("a.dart");
        rig.declare(a, "aName", a_file);
        rig.export(a, Some(lib_b));
        rig.builder.import(a, import_edge(Some(c)));
        let graph = rig.builder.finish();

        let sort = |walk: Vec<LibraryId>| {
            let mut ids = walk;
            ids.sort_by_key(|id| id.index());
            ids
        };
        let first = sort(graph.libraries().collect());
        let second = sort(graph.libraries().collect());
        assert_eq!(first, vec![a, lib_b, c], "every node, no duplicates");
        assert_eq!(first, second, "stable across repeated walks");
        assert_eq!(first.len(), graph.len());
        for lib in graph.libraries() {
            assert!(graph.uri(lib).starts_with("package:"));
        }
    }

    /// An empty graph yields nothing.
    #[test]
    fn libraries_of_empty_graph_yield_nothing() {
        let strings = StringTable::new();
        let graph = LibraryGraphBuilder::new(&strings).finish();
        assert!(graph.is_empty());
        assert_eq!(graph.libraries().count(), 0);
    }

    // ---- cross-library modifier facts (ticket #281) ----

    /// `declare_class` registers the name on both faces — the namespace
    /// locals and the modifier map — and the inverted index answers
    /// `declaring_libraries` in registration order; a duplicate class
    /// name in one library keeps the first modifiers.
    #[test]
    fn class_modifier_facts_cover_declaration_and_ownership() {
        let mut rig = Rig::new();
        let a = rig.builder.intern("package:a/a.dart");
        let lib_b = rig.builder.intern("package:b/b.dart");
        let a_file = rig.file("a.dart");
        let b_file = rig.file("b.dart");
        let sealed = ClassModifiers {
            kind: crate::modifiers::DeclKind::Class,
            is_sealed: true,
            ..ClassModifiers::default()
        };
        let plain = ClassModifiers::default();
        rig.builder
            .declare_class(a, rig.strings.intern("Shape"), a_file, sealed);
        rig.builder
            .declare_class(lib_b, rig.strings.intern("Shape"), b_file, plain);
        rig.builder
            .declare_class(lib_b, rig.strings.intern("Local"), b_file, plain);
        // A duplicate registration keeps the first modifiers (the
        // declare-side first-wins contract).
        rig.builder.declare_class(
            lib_b,
            rig.strings.intern("Shape"),
            b_file,
            ClassModifiers {
                is_base: true,
                ..plain
            },
        );
        let graph = rig.builder.finish();
        // A class name is also a locals declaration (either face lookup
        // hits).
        assert!(matches!(
            graph.local(a, rig.strings.intern("Shape")),
            LocalLookup::Found(_)
        ));
        assert_eq!(
            graph.class_modifiers(a, rig.strings.intern("Shape")),
            Some(sealed)
        );
        assert_eq!(
            graph.class_modifiers(lib_b, rig.strings.intern("Shape")),
            Some(plain)
        );
        assert_eq!(graph.class_modifiers(a, rig.strings.intern("Local")), None);
        assert_eq!(
            graph.declaring_libraries(rig.strings.intern("Shape")),
            &[a, lib_b]
        );
        assert!(
            graph
                .declaring_libraries(rig.strings.intern("Missing"))
                .is_empty()
        );
    }
}
