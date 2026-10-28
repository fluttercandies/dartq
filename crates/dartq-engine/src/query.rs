//! The index-backed query faces: the by-name `query symbol` answer, the
//! caller walk over its invocation rows, the offset-keyed position
//! resolution, and the built-in completion
//! candidate set — all
//! answered from the engine's maintained index
//! state — the per-file captures (declaration surface + raw sites) and the
//! per-library reference surfaces analyze keeps current at the
//! `LibraryAnalyzed` point, indexed by the name-keyed postings directories
//! the state mounts over them (dartq-index `Postings`; the state's
//! mutators maintain them under the crate's whole-unit law). The decl
//! directory names the files declaring a name, the ref directory the
//! libraries holding resolved uses of it.
//!
//! The token face ([`declaration_span`]) is the second family: a
//! span-keyed read over the retained scan face (the state's token table),
//! no index rows involved. It shares the same freshness contract — a pure
//! read of the latest analyze's state — and re-derives the pure scan face
//! locally on a cache miss, never filling it (a query does not write).
//!
//! # Freshness contract
//!
//! A query is a pure read of the state the latest [`crate::Engine::analyze`]
//! produced: callers run analyze first (the read-after-write), and a query
//! never applies the overlay, re-analyzes, or otherwise mutates derived
//! state — the only side effect is interning the queried name. Querying
//! before the first analyze answers an empty set. Reference surfaces track
//! the completed-library partition of the latest analyze exactly (a
//! cancelled analyze leaves the not-yet-recomputed libraries on their
//! previous surfaces, the same partition their cached diagnostic results
//! keep).
//!
//! # Cost shape
//!
//! The answer assembles in O(answer + units): one hash probe per face
//! names the contributing units (ascending — the deterministic order the
//! `SymbolIndex` build prescribed), decl rows come off each file's
//! name-sorted surface run, and the ref join runs one binary search per
//! referencing library over the surface's name-keyed run groups
//! ([`LibraryIndex::references_named`]) plus one decl-set membership per
//! matched run — O(L·log R + runs + rows + D) where the retired nested
//! loop paid a search per (decl file, library) pair and kept hub names
//! quadratic in the workspace. A stable target sort then restores the
//! answer order. No scan, no parse, no namespace rebuild — the
//! whole-workspace recapture this face used to run per query died with the
//! maintained index (the audit's query-path multiplication), and the
//! per-query scan over the maintained arenas died with the directories.

use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};

use dartq_index::{DeclRow, LibraryIndex, RefRow, TargetRef};
use dartq_scanner::{TokenKind, TokenTable};
use dartq_source::{FileId, SourceRange, StringId};

use crate::state::{EngineState, ScannedFace};

pub use dartq_index::{DeclKind, RefKind};

/// The relation an answer row reports: the stored capture kind
/// ([`RefKind`], the syntax face the capture mints without type
/// information) widened with [`ReferenceKind::Override`] — a row that is
/// not a use at all but an overriding declaration of the queried member.
/// Override-ness is a subtype-closure property, so it is derived here at
/// the query boundary; the stored rows keep their 1:1 kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceKind {
    /// A non-invocation, non-value use: a type name, an `extends`/`on`
    /// clause name, a metadata name.
    Referenced,
    /// A value read.
    Read,
    /// A call site, constructor reference, or tear-off-shaped use.
    Invoked,
    /// The target of a plain `=` assignment.
    Written,
    /// The target of a compound assignment.
    ReadWritten,
    /// An overriding declaration of the queried member: a member
    /// declaration with the queried name whose declaring class is a
    /// transitively-declared subtype of the queried member's declaring
    /// class. The row locates the override's declaration site; its
    /// `target_file` is the overridden declaration's file.
    Override,
}

impl From<RefKind> for ReferenceKind {
    fn from(kind: RefKind) -> Self {
        match kind {
            RefKind::Referenced => Self::Referenced,
            RefKind::Read => Self::Read,
            RefKind::Invoked => Self::Invoked,
            RefKind::Written => Self::Written,
            RefKind::ReadWritten => Self::ReadWritten,
        }
    }
}

/// The `query symbol` answer: declaration rows plus resolved
/// reference rows. Rows borrow the engine's interned spellings — lookups
/// run on interned ids and resolve to `&str` only at this boundary; nothing
/// clones per row.
#[derive(Debug, Clone)]
pub struct SymbolAnswer<'a> {
    /// Every declaration named by the query, ascending by file.
    pub decls: Vec<DeclHit<'a>>,
    /// Every resolved reference to those declarations, grouped by target
    /// file (ascending), offset-ascending within a target; a queried
    /// member's overriding declarations ride the same grouping after its
    /// use rows, kinded [`ReferenceKind::Override`].
    pub refs: Vec<RefHit<'a>>,
}

/// One declaration hit of [`SymbolAnswer`].
#[derive(Clone, Copy, Debug)]
pub struct DeclHit<'a> {
    /// The declaring file.
    pub file: FileId,
    /// The declaration family.
    pub kind: DeclKind,
    /// The declared name (setters under their `name=` lookup spelling).
    pub name: &'a str,
    /// The enclosing container's name; [`None`] for top-level rows and
    /// unnamed-extension members.
    pub container: Option<&'a str>,
    /// The name token's span (UTF-16, the engine's external convention).
    pub range: SourceRange,
}

/// One resolved reference hit of [`SymbolAnswer`].
#[derive(Clone, Copy, Debug)]
pub struct RefHit<'a> {
    /// The referenced declaration's file.
    pub target_file: FileId,
    /// The referenced declaration's enclosing container for member targets
    /// (#2618) — `(file, name)` alone cannot name a member (two classes in
    /// one file may declare the same member name); [`None`] for top-level
    /// targets. The override-closure rows carry the queried member's own
    /// container (their target is that member's hierarchy).
    pub target_container: Option<&'a str>,
    /// The use site's file — the library member whose capture recorded the
    /// row: a library spans its defining unit plus parts, so the target
    /// pair alone cannot locate the use (dartq-index `RefRow::file`).
    pub file: FileId,
    /// The relation kind of the use — the stored kind widened with
    /// [`ReferenceKind::Override`] for the overriding-declaration rows the
    /// face appends.
    pub kind: ReferenceKind,
    /// The use is explicitly qualified (`p.x`) and cannot be shadowed by a
    /// local declaration.
    pub explicit: bool,
    /// The import prefix spelling that armed the resolution (`p` in
    /// `p.fa`) — the row borrows the interned spelling like
    /// [`DeclHit::name`]; the
    /// JSON face assembles the qualified-name column as
    /// `<qualifier>.<queried name>`. [`None`] for bare uses.
    pub qualified_name: Option<&'a str>,
    /// The use site's span (UTF-16).
    pub range: SourceRange,
}

/// Answers a by-name symbol query over the maintained index state (module
/// docs for the freshness and cost contracts). The declaration surface
/// spans two tiers — the resident workspace captures and the dependency
/// interface units' captures ([`crate::deps`]) — with residency winning a
/// file both hold; reference rows resolve against the union, so a use in
/// the workspace whose declaration sits in a lazily indexed dependency
/// keeps its target (the rename face's cross-package refusal contract).
pub(crate) fn symbols<'a>(state: &'a EngineState, name: &str) -> SymbolAnswer<'a> {
    // The queried name interns once; every lookup below runs on the id.
    let id = state.strings.intern(name);

    // Declaration rows through the decl directory: one hash probe names the
    // declaring files (ascending — the arena slot order the scan served),
    // and each file's equal-name run comes off its own capture,
    // offset-ascending. Both tiers join — the resident captures first,
    // then the dependency units' captures through the dep tier's own
    // postings — and the merged list re-sorts by file so the ascending
    // order holds across tiers.
    let mut decls: Vec<DeclHit<'a>> = state
        .decl_postings
        .units(id)
        .iter()
        .copied()
        .filter_map(|file| {
            let capture = state.capture(file)?;
            let rows = capture.index.by_name(id)?;
            Some(rows.iter().map(move |decl| DeclHit {
                file,
                kind: decl.kind,
                name: state.strings.resolve(decl.name),
                container: (decl.container != DeclRow::NONE).then(|| {
                    state
                        .strings
                        .resolve(capture.index.row(decl.container).name)
                }),
                range: decl.range,
            }))
        })
        .flatten()
        .collect();
    decls.extend(
        state
            .deps
            .decl_postings
            .units(id)
            .iter()
            .copied()
            .filter_map(|file| {
                let capture = &state.deps.units.get(&file)?.capture;
                let rows = capture.index.by_name(id)?;
                Some(rows.iter().map(move |decl| DeclHit {
                    file,
                    kind: decl.kind,
                    name: state.strings.resolve(decl.name),
                    container: (decl.container != DeclRow::NONE).then(|| {
                        state
                            .strings
                            .resolve(capture.index.row(decl.container).name)
                    }),
                    range: decl.range,
                }))
            })
            .flatten(),
    );
    decls.sort_by_key(|hit| hit.file);

    // Reference rows through the ref directory, the name-keyed face of
    // each library's surface, and the exact-target filter: the directory
    // names the libraries holding resolved uses of `name` (ascending —
    // the surface slot order the scan served), and one binary search per
    // library serves its whole run group for the name; only runs whose
    // target is one of the declaring files survive (the postings are
    // keyed by name, the answer by `(file, name)`). One search per
    // library replaces the per-(decl file, library) probe — D × L
    // searches, nearly all answering None, which kept hub names
    // quadratic in the workspace size. Rows collect library-major; the
    // stable target sort restores the contract order — target-major,
    // then library, then (use file, offset) — byte-identical to the
    // nested loop this join retired.
    let mut declaring: FxHashSet<FileId> = state.decl_postings.units(id).iter().copied().collect();
    // The dependency declarations join the declaring set: a workspace use
    // whose target sits in the interface tier keeps its run (the filter
    // below would otherwise drop the very rows the cross-package faces —
    // rename's refusal, goto-def — resolve through).
    declaring.extend(state.deps.decl_postings.units(id).iter().copied());
    let mut refs: Vec<RefHit<'a>> = state
        .ref_postings
        .units(id)
        .iter()
        .copied()
        .filter_map(|library| state.surface(library))
        .flat_map(|surface: &LibraryIndex| surface.references_named(id))
        .filter(|run| declaring.contains(&run[0].target_file))
        .flatten()
        .map(|row: &RefRow| RefHit {
            target_file: row.target_file,
            target_container: (row.target_container != StringId::NONE)
                .then(|| state.strings.resolve(row.target_container)),
            file: row.file,
            kind: ReferenceKind::from(row.kind),
            explicit: row.qualified(),
            qualified_name: row
                .qualified()
                .then(|| state.strings.resolve(row.qualifier)),
            range: row.range,
        })
        .collect();

    // The overriding declarations: for every member declaration of the
    // queried name, the same-name members of the transitively-declared
    // subtypes of its declaring class — one [`ReferenceKind::Override`] row
    // per (anchor declaration, override site) pair, appended after the use
    // rows so a target's group keeps uses first. A name with no member
    // declarations (top-level, or a miss) skips the walk entirely; the
    // closure is a pure read of the surfaces' resolved supertype edges. A
    // member row's container lives in the row's own file, so the root is
    // the (file, name) pair of its declaring class.
    let mut member_roots: Vec<(FileId, StringId)> = state
        .decl_postings
        .units(id)
        .iter()
        .copied()
        .filter_map(|file| {
            let capture = state.capture(file)?;
            let rows = capture.index.by_name(id)?;
            Some(
                rows.iter()
                    .filter(|row| row.container != DeclRow::NONE && is_member_kind(row.kind))
                    .map(|row| (file, capture.index.row(row.container).name))
                    .collect::<Vec<_>>(),
            )
        })
        .flatten()
        .collect();
    member_roots.sort_unstable();
    member_roots.dedup();
    if !member_roots.is_empty() {
        let subtypes = reverse_supertypes(state);
        // The anchor law: a row's target is the queried symbol's own
        // declaration, so a root whose declaring class already sits inside
        // another root's closure is itself an override row, never a second
        // anchor — its whole subtree arrives through that anchor's
        // transitive closure, and the intermediate member anchors no rows.
        let closures: Vec<FxHashSet<(FileId, StringId)>> = member_roots
            .iter()
            .map(|root| subtype_closure(&subtypes, *root).collect())
            .collect();
        let mut overrides: Vec<RefHit<'a>> = Vec::new();
        let mut seen: FxHashSet<(FileId, FileId, u32)> = FxHashSet::default();
        for (at, root) in member_roots.iter().enumerate() {
            if closures
                .iter()
                .enumerate()
                .any(|(other, set)| other != at && set.contains(root))
            {
                continue;
            }
            for sub in &closures[at] {
                if *sub == *root {
                    continue;
                }
                let Some(sub_capture) = state.capture(sub.0) else {
                    continue;
                };
                let Some(rows) = sub_capture.index.by_name(id) else {
                    continue;
                };
                for row in rows {
                    if row.container == DeclRow::NONE
                        || sub_capture.index.row(row.container).name != sub.1
                        || !is_member_kind(row.kind)
                        || !seen.insert((root.0, sub.0, row.range.offset))
                    {
                        continue;
                    }
                    overrides.push(RefHit {
                        target_file: root.0,
                        // The row's target is the queried member's own
                        // hierarchy: its container names the anchor's
                        // declaring class, so the member-scoped consumers
                        // (rename's target filter) keep the closure whole.
                        target_container: Some(state.strings.resolve(root.1)),
                        file: sub.0,
                        kind: ReferenceKind::Override,
                        explicit: false,
                        qualified_name: None,
                        range: row.range,
                    });
                }
            }
        }
        overrides.sort_unstable_by_key(|hit| (hit.file, hit.range.offset));
        refs.extend(overrides);
    }

    refs.sort_by_key(|hit| hit.target_file);
    SymbolAnswer { decls, refs }
}

/// The fuzzy discovery answer: the workspace's declarations whose names
/// fuzzy-match `query`, best-first (the dartq-index `fuzzy` module's
/// ranking spec — exact prefix, then word-boundary humps, then contiguous
/// substring, then sparse subsequence; ties by name length, then name
/// bytes), capped at that module's result cap. Both declaration tiers
/// scan (the resident workspace captures and the dependency interface
/// units, the exact face's answer domain), a name both hold ranking once.
///
/// References are not part of the answer: discovery names symbols, the
/// exact faces resolve their uses — `refs` stays empty. Row order inside
/// one matched name is file-then-offset; across names it is the rank, NOT
/// the exact face's file-ascending contract.
///
/// # Freshness contract
///
/// [`symbols`]'s, verbatim — except the scan interns nothing: the query
/// folds to a scratch buffer, the names resolve off the existing table,
/// and the state is a pure read.
///
/// # Cost shape
///
/// One linear subsequence pass per distinct indexed name across the two
/// decl directories (the dartq-index `fuzzy` module's posture —
/// sub-millisecond at corpus scale), then the winners' rows off their
/// files' equal-name runs: the scan plus the answer's size. No parse, no
/// namespace rebuild, no workspace scan.
pub(crate) fn symbols_fuzzy<'a>(state: &'a EngineState, query: &str) -> SymbolAnswer<'a> {
    let folded = dartq_index::fold_query(query);
    let mut seen: FxHashSet<StringId> = FxHashSet::default();
    let mut candidates: Vec<dartq_index::Candidate> = state
        .decl_postings
        .names()
        .chain(state.deps.decl_postings.names())
        .filter_map(|(name, _)| {
            let class = dartq_index::classify(&folded, state.strings.resolve(name))?;
            seen.insert(name)
                .then_some(dartq_index::Candidate { class, name })
        })
        .collect();
    dartq_index::rank(&mut candidates, &state.strings);

    let decls: Vec<DeclHit<'a>> = candidates
        .into_iter()
        .flat_map(move |candidate| {
            let name = candidate.name;
            let mut rows: Vec<DeclHit<'a>> = state
                .decl_postings
                .units(name)
                .iter()
                .copied()
                .filter_map(|file| {
                    let capture = state.capture(file)?;
                    Some(
                        capture
                            .index
                            .by_name(name)?
                            .iter()
                            .map(move |decl| decl_hit(state, file, &capture.index, decl)),
                    )
                })
                .flatten()
                .collect();
            rows.extend(
                state
                    .deps
                    .decl_postings
                    .units(name)
                    .iter()
                    .copied()
                    .filter_map(|file| {
                        let capture = &state.deps.units.get(&file)?.capture;
                        Some(
                            capture
                                .index
                                .by_name(name)?
                                .iter()
                                .map(move |decl| decl_hit(state, file, &capture.index, decl)),
                        )
                    })
                    .flatten(),
            );
            rows.sort_by_key(|hit| (hit.file, hit.range.offset));
            rows
        })
        .collect();
    SymbolAnswer {
        decls,
        refs: Vec::new(),
    }
}

/// The declaration row's answer projection — the row shape every by-name
/// face serves (file, kind, resolved name and container, span).
fn decl_hit<'a>(
    state: &'a EngineState,
    file: FileId,
    index: &dartq_index::FileIndex,
    row: &DeclRow,
) -> DeclHit<'a> {
    DeclHit {
        file,
        kind: row.kind,
        name: state.strings.resolve(row.name),
        container: (row.container != DeclRow::NONE)
            .then(|| state.strings.resolve(index.row(row.container).name)),
        range: row.range,
    }
}

/// The reverse supertype adjacency over the whole analyzed workspace: one
/// resolved edge per (subtype container, clause) target, grouped by
/// supertype — the subtype walk's substrate (the surfaces index edges
/// subtype-major, so the reverse grouping is built per query, O(units +
/// containers + edges), and dropped with the answer).
fn reverse_supertypes(
    state: &EngineState,
) -> FxHashMap<(FileId, StringId), Vec<(FileId, StringId)>> {
    let mut map: FxHashMap<(FileId, StringId), Vec<(FileId, StringId)>> = FxHashMap::default();
    for (&file, &library) in &state.library_of_file {
        let Some(capture) = state.capture(file) else {
            continue;
        };
        let Some(surface) = state.surface(library) else {
            continue;
        };
        for row in capture.index.decls() {
            if !matches!(
                row.kind,
                DeclKind::Class
                    | DeclKind::Mixin
                    | DeclKind::Enum
                    | DeclKind::Extension
                    | DeclKind::ExtensionType
            ) {
                continue;
            }
            for edge in surface.supertypes_of(file, row.name).into_iter().flatten() {
                if let TargetRef::Decl(supertype_file, supertype_name) = edge.supertype {
                    map.entry((supertype_file, supertype_name))
                        .or_default()
                        .push((file, row.name));
                }
            }
        }
    }
    map
}

/// The transitively-declared subtypes of `root` over the reverse adjacency:
/// a worklist walk with a visited set, so each subtype is visited once
/// whatever the edge fan-out (and whatever cycles recovery recorded — the
/// visited set is the termination proof).
fn subtype_closure(
    subtypes: &FxHashMap<(FileId, StringId), Vec<(FileId, StringId)>>,
    root: (FileId, StringId),
) -> impl Iterator<Item = (FileId, StringId)> + '_ {
    let mut visited: FxHashSet<(FileId, StringId)> = FxHashSet::default();
    let mut worklist = vec![root];
    while let Some(node) = worklist.pop() {
        for sub in subtypes.get(&node).into_iter().flatten() {
            if visited.insert(*sub) {
                worklist.push(*sub);
            }
        }
    }
    visited.into_iter()
}

/// The member kinds the override relation spans: a method, getter, setter,
/// or field declaration of a class-like container — the predicate the
/// override-row mint and the family face ([`override_family`]) share.
fn is_member_kind(kind: DeclKind) -> bool {
    matches!(
        kind,
        DeclKind::Method | DeclKind::Getter | DeclKind::Setter | DeclKind::Field
    )
}

/// Whether `container` declares a member named `name` — an
/// [`is_member_kind`] row of the name whose container anchor names
/// `container` — the family membership predicate. A container whose file
/// the resident captures do not hold answers false.
fn declares_member(state: &EngineState, container: (FileId, StringId), name: StringId) -> bool {
    let Some(capture) = state.capture(container.0) else {
        return false;
    };
    capture.index.by_name(name).is_some_and(|rows| {
        rows.iter().any(|row| {
            row.container != DeclRow::NONE
                && capture.index.row(row.container).name == container.1
                && is_member_kind(row.kind)
        })
    })
}

/// The transitive supertypes of `container` over the surfaces' resolved
/// edges — the upward twin of [`subtype_closure`]: one worklist walk over
/// `supertypes_of`, workspace declarations only (a bare-name target outside
/// the analyzed set carries no member rows to rename), the visited set the
/// termination proof whatever the edge fan-out or the cycles recovery
/// recorded.
fn supertype_closure(
    state: &EngineState,
    container: (FileId, StringId),
) -> FxHashSet<(FileId, StringId)> {
    let mut visited: FxHashSet<(FileId, StringId)> = FxHashSet::default();
    let mut worklist = vec![container];
    while let Some(node) = worklist.pop() {
        let Some(&library) = state.library_of_file.get(&node.0) else {
            continue;
        };
        let Some(surface) = state.surface(library) else {
            continue;
        };
        for edge in surface.supertypes_of(node.0, node.1).into_iter().flatten() {
            if let TargetRef::Decl(file, name) = edge.supertype {
                if visited.insert((file, name)) {
                    worklist.push((file, name));
                }
            }
        }
    }
    visited
}

/// The override family of one member declaration: every container that
/// declares `name` connected to `container` through the inheritance
/// closure — the whole hierarchy a member rename edits (the analyzer's
/// rename of a method renames every declaration of the member in the
/// family: supertype and subtype declarers, implementers, mixins — plus
/// every reference whose static target is any of them, the caller's
/// join).
///
/// The derivation is the one the override-row mint above walks —
/// [`reverse_supertypes`] + [`subtype_closure`] down, [`supertype_closure`]
/// up — run as a fixpoint: each newly-joined member contributes the
/// declaring relatives of BOTH directions, so a subtype reached downward
/// that joins a second branch (a mixin, a base class, a second interface)
/// pulls that branch and its own implementers in too — a one-pass up/down
/// walk would stop at the first branch and split the family by anchor.
/// The fixpoint makes the family an equivalence class: anchoring at ANY
/// member — interface, concrete override, or a use resolving to one —
/// answers the identical set. A class reached through the closure that
/// does not declare the member is walked through, never answered (it has
/// no declaration to rename).
///
/// # Freshness contract
///
/// [`Engine::query_symbols`]'s, verbatim: a pure read of the latest
/// [`crate::Engine::analyze`]'s maintained state — the only side effect is
/// the caller-side interning of the queried spellings (the
/// [`crate::Engine::override_family`] wrapper).
///
/// # Cost shape
///
/// One reverse adjacency build (O(units + containers + edges),
/// [`reverse_supertypes`]) plus one up- and one down-closure walk per
/// family member, each member processed exactly once (the family set
/// doubles as the worklist's visited set) — the same cost class one
/// `query symbols` answer pays at hierarchy scale. The answer sorts by
/// `(file, container id)` and borrows the interned spellings — no
/// allocation beyond the result Vec and the walk's transient sets.
pub(crate) fn override_family<'a>(
    state: &'a EngineState,
    container: (FileId, StringId),
    name: StringId,
) -> Vec<(FileId, &'a str)> {
    let subtypes = reverse_supertypes(state);
    // The fixpoint worklist: a member joins exactly once, when first
    // discovered, and is then processed once — contributing every
    // declaring transitive supertype and subtype, whose own processing
    // continues the alternation. Monotone growth over the finite container
    // universe is the termination proof.
    let mut family: FxHashSet<(FileId, StringId)> = FxHashSet::default();
    family.insert(container);
    let mut worklist = vec![container];
    while let Some(member) = worklist.pop() {
        for supertype in supertype_closure(state, member) {
            if declares_member(state, supertype, name) && family.insert(supertype) {
                worklist.push(supertype);
            }
        }
        for sub in subtype_closure(&subtypes, member) {
            if declares_member(state, sub, name) && family.insert(sub) {
                worklist.push(sub);
            }
        }
    }
    let mut rows: Vec<(FileId, StringId)> = family.into_iter().collect();
    rows.sort_unstable();
    rows.into_iter()
        .map(|(file, container)| (file, state.strings.resolve(container)))
        .collect()
}

/// The `query callers` answer: every declaration whose body invokes the
/// queried symbol — the caller→callee walk over the resolution face.
///
/// # Freshness contract
///
/// [`Engine::query_symbols`]'s, verbatim: a pure read of the latest
/// [`crate::Engine::analyze`]'s maintained state — callers run analyze
/// first, and the only side effect is interning the queried name.
///
/// # Answer domain
///
/// The reuse of the reference rows is the capture's invocation law:
/// [`RefKind::Invoked`] covers direct calls, constructor references, and
/// tear-off-shaped uses, so exactly those sites name a caller; bare reads
/// and writes do not. Each site projects to its enclosing declaration by
/// the containment law — the last declaration row at or before the site
/// (the `decl_order` partition point [`query::resolve_at`]'s declaration
/// arm uses), inflated to the whole declaration by
/// [`query::declaration_span`], must cover the site. A site the projection
/// cannot claim — before the first declaration (an import section, the
/// first declaration's metadata arguments), inside a span the capture
/// cannot inflate (a bailed parse), or within a local function the row
/// surface does not model — names no caller. One caller per declaration
/// however many of its sites invoke the name: the answer is the set of
/// caller symbols, not the site list (the references face serves sites).
///
/// Cross-file callers answer like local ones: the reference rows carry the
/// use file beside the target file, so the projection runs on the use
/// file's own rows.
///
/// # Cost shape
///
/// O(sites + units): one directory probe names the referencing libraries
/// ([`LibraryIndex::references_named`], the exact-target filter of
/// [`query::symbols`]), sites sort file-major so each projection walks its
/// file's own rows and scan face once, and a hashed seen-set dedupes the
/// answer before the deterministic `(file, offset)` sort.
pub(crate) fn callers<'a>(state: &'a EngineState, name: &str) -> Vec<DeclHit<'a>> {
    let id = state.strings.intern(name);
    let mut declaring: FxHashSet<FileId> = state.decl_postings.units(id).iter().copied().collect();
    // The dependency declarations join the declaring set: a workspace use
    // whose target sits in the interface tier keeps its run (the filter
    // below would otherwise drop the very rows the cross-package faces —
    // rename's refusal, goto-def — resolve through).
    declaring.extend(state.deps.decl_postings.units(id).iter().copied());

    // The invocation sites resolving to the queried name and targeting one
    // of its declarations, file-major (a file belongs to one library, so
    // the runs arrive file-grouped; the sort only restores it across
    // targets).
    let mut sites: Vec<(FileId, u32)> = state
        .ref_postings
        .units(id)
        .iter()
        .copied()
        .filter_map(|library| state.surface(library))
        .flat_map(|surface: &LibraryIndex| surface.references_named(id))
        .filter(|run| declaring.contains(&run[0].target_file))
        .flatten()
        .filter(|row| row.kind == RefKind::Invoked)
        .map(|row| (row.file, row.range.offset))
        .collect();
    if sites.is_empty() {
        return Vec::new();
    }
    sites.sort_unstable();

    // One row per enclosing declaration, however many sites it holds.
    let mut callers: Vec<DeclHit<'a>> = Vec::new();
    let mut seen: FxHashSet<(FileId, u32)> = FxHashSet::default();
    for (file, offset) in sites {
        let Some(hit) = enclosing_declaration(state, file, offset) else {
            continue;
        };
        if seen.insert((file, hit.range.offset)) {
            callers.push(hit);
        }
    }
    callers.sort_by_key(|hit| (hit.file, hit.range.offset));
    callers
}

/// The declaration whose whole span contains `offset` — the caller
/// projection's containment law (module docs of [`callers`]): the
/// partition-point candidate row, inflated by [`declaration_span`], must
/// cover the site.
fn enclosing_declaration<'a>(
    state: &'a EngineState,
    file: FileId,
    offset: u32,
) -> Option<DeclHit<'a>> {
    let capture = state.capture(file)?;
    let order = state.decl_order(file)?;
    let decls = capture.index.decls();
    let cursor = order.partition_point(|&i| decls[i as usize].range.offset <= offset);
    let &row = cursor.checked_sub(1).and_then(|at| order.get(at))?;
    let row = &decls[row as usize];
    let span = declaration_span(state, file, row.range)?;
    (offset < span.end()).then(|| DeclHit {
        file,
        kind: row.kind,
        name: state.strings.resolve(row.name),
        container: (row.container != DeclRow::NONE)
            .then(|| state.strings.resolve(capture.index.row(row.container).name)),
        range: row.range,
    })
}

/// The offset-keyed resolution answer: what the maintained index rows hold
/// under one file position.
#[derive(Clone, Copy, Debug)]
pub enum PositionAnswer<'a> {
    /// The position sits on a declaration's name token — the declaration
    /// itself is the answer (its [`DeclHit`]; goto-def's target is the hit
    /// itself).
    Declaration(DeclHit<'a>),
    /// The position sits on a resolved use of a declaration.
    Use(UseHit<'a>),
}

/// One resolved use hit of [`PositionAnswer`]: the use site's own row plus
/// the target declaration's identity and name-token span — goto-def's
/// target, hover's subject, rename's anchor.
#[derive(Clone, Copy, Debug)]
pub struct UseHit<'a> {
    /// The use site's file — the queried file.
    pub file: FileId,
    /// The use site's span (UTF-16, half-open) in that file.
    pub range: SourceRange,
    /// The relation kind of the use.
    pub kind: RefKind,
    /// The use is explicitly qualified (`p.x`) and cannot be shadowed by a
    /// local declaration.
    pub qualified: bool,
    /// The import prefix that armed the resolution (`p` in `p.fa`),
    /// resolved at this boundary like every spelling; [`None`] for bare
    /// uses.
    pub prefix: Option<&'a str>,
    /// The target declaration's file.
    pub target_file: FileId,
    /// The target's lookup-spelling name (setters under `name=`).
    pub name: &'a str,
    /// The target's declaration family.
    pub decl_kind: DeclKind,
    /// The target's enclosing container's name; [`None`] for top-level rows
    /// and unnamed-extension members.
    pub container: Option<&'a str>,
    /// The target declaration's name-token span in
    /// [`UseHit::target_file`] — the span a goto-def jumps to.
    pub target_range: SourceRange,
}

/// Which maintained surface proved one completion candidate visible — the
/// origins are disjoint by the resolution law (a prefix arms only where the
/// name is no library face; a namespace hit is either a declaration target
/// or a prefix, never both), so one candidate names one origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CompletionOrigin {
    /// The queried file declares the name: a library-scope row of its own
    /// capture.
    Declaration,
    /// The file already resolves the name through its imports: a target
    /// name of its own resolved-reference rows (the resolution is the
    /// import-visibility proof — no scope recomputation).
    Reference,
    /// An import prefix the file arms (`p` in `p.fa`): a qualifier of its
    /// own resolved-reference rows.
    Prefix,
}

/// One completion candidate: the visible name plus whatever identity the
/// proving surface carries. `Declaration` and `Reference` items carry the
/// declaration's identity and name-token span (a completed pick can jump
/// to it); a `Prefix` item is a wire word, not a declaration — every
/// identity column stays `None`.
#[derive(Clone, Copy, Debug)]
pub struct CompletionItem<'a> {
    /// The candidate's spelling (setters under their `name=` lookup
    /// spelling, like every query face).
    pub name: &'a str,
    /// The surface that proved the name visible.
    pub origin: CompletionOrigin,
    /// The declaration family; [`None`] for prefix items.
    pub decl_kind: Option<DeclKind>,
    /// The enclosing container's name; [`None`] for top-level rows,
    /// unnamed-extension members, and prefix items.
    pub container: Option<&'a str>,
    /// The declaring file (the queried file itself for
    /// [`CompletionOrigin::Declaration`], the resolution's target for
    /// [`CompletionOrigin::Reference`]); [`None`] for prefix items.
    pub file: Option<FileId>,
    /// The declaration's name-token span (UTF-16) in [`CompletionItem::file`];
    /// [`None`] for prefix items.
    pub range: Option<SourceRange>,
}

/// The completion answer: the identifier fragment under the cursor plus
/// the visible candidates, ascending by name.
#[derive(Clone, Debug)]
pub struct CompletionAnswer<'a> {
    /// The characters immediately before the cursor that extend an
    /// identifier — the fragment the candidates complete. Empty when the
    /// cursor does not follow identifier characters: every visible
    /// candidate answers.
    pub prefix: &'a str,
    /// The visible candidates, ascending by name (then origin, declaring
    /// file, span — the deterministic total order; same input, same
    /// output). A name both declared locally and resolved through a prefix
    /// keeps one row, its declaration (the local-hop law: the file's own
    /// declaration beats imports).
    pub items: Vec<CompletionItem<'a>>,
}

/// Answers the completion candidates visible at `offset` (UTF-16 code
/// units, the engine's external convention) in `file` — the built-in
/// completion primitive. There is no plugin escape: the plugin protocol
/// rejects a `completions` capability by construction, so this face is the
/// whole completion story.
///
/// # Freshness contract
///
/// [`Engine::query_symbols`]'s, verbatim: a pure read of the latest
/// [`Engine::analyze`]'s maintained state — callers analyze first, and a
/// read never applies the overlay, re-analyzes, or otherwise mutates
/// derived state. The word extraction reads the analyzed snapshot's own
/// text (the residency facts' content handle and line index — the exact
/// bytes and index the capture was built from), so fragment and rows can
/// never disagree.
///
/// # Answer domain
///
/// The candidate set is what the resolution graph proves visible at the
/// file, at M1 fidelity:
///
/// * the file's own library-scope declarations (members and type
///   parameters are not scope names — the same law the resolution's local
///   hop applies);
/// * the distinct target names of the file's own resolved references
///   (import visibility, proven by resolution — a use the current state
///   does not resolve is a names-table row and offers no candidate);
/// * the import prefixes the file arms.
///
/// The `None` boundary is [`query::resolve_at`]'s: a file the maintained
/// rows do not cover (not resident, not yet analyzed) answers `None`, and
/// so does an offset past the file's end. A cursor whose preceding
/// character is `.` is a member-access continuation — M1 has no member
/// enumeration (an untyped-receiver member is a names-table row, the same
/// boundary) — and answers the empty set, not a miss.
///
/// # Word extraction
///
/// The fragment is the run of identifier characters immediately before the
/// cursor, byte-walked left: Dart identifiers are ASCII-only (the official
/// scanner's law), so no wide character can sit inside a fragment, and the
/// residency line index converts the UTF-16 offset with two indexed
/// searches. The suffix right of the cursor is not stripped — the prefix
/// filter is case-sensitive over exactly the left fragment.
///
/// # Cost shape
///
/// Two indexed offset conversions, the fragment walk, and one pass over
/// the queried file's own resident rows (its declaration surface — the
/// rows sort by interned id, not spelling, so the prefix filter walks the
/// run list — and its use-view rows), plus one name-run binary search per
/// distinct reference target and the answer sort. No other file is
/// touched: no workspace scan, no parse, no namespace rebuild.
pub(crate) fn complete_at(
    state: &EngineState,
    file: FileId,
    offset: u32,
) -> Option<CompletionAnswer<'_>> {
    let capture = state.capture(file)?;
    let facts = state.residency.get(&file)?;
    let source = std::str::from_utf8(state.store.get(facts.content)?).ok()?;
    let index = &facts.line_index;

    // The past-end boundary: `utf16_to_byte` asserts in-range, and a
    // past-end offset must answer `None`, not panic the reader. The file's
    // total UTF-16 length comes off the same index, one indexed conversion
    // from the end offset (the index was built from this exact source,
    // whose length it already stores as `u32`).
    let total = index.byte_to_utf16(source, source.len() as u32);
    if offset > total {
        return None;
    }
    let byte_at = index.utf16_to_byte(source, offset) as usize;
    let bytes = source.as_bytes();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$';
    let mut start = byte_at;
    while start > 0 && is_ident(bytes[start - 1]) {
        start -= 1;
    }
    let prefix = &source[start..byte_at];
    let member_context = start > 0 && bytes[start - 1] == b'.';
    if member_context {
        return Some(CompletionAnswer {
            prefix,
            items: Vec::new(),
        });
    }

    let mut items: Vec<CompletionItem<'_>> = Vec::new();

    // The file's own library-scope names: one item per equal-name run (the
    // rows sort by `(name, offset)`, so a run is contiguous and its first
    // row is the item).
    let mut run = None;
    for row in capture.index.decls() {
        if !row.is_library_face() || run == Some(row.name) {
            continue;
        }
        run = Some(row.name);
        let name = state.strings.resolve(row.name);
        if !name.starts_with(prefix) {
            continue;
        }
        items.push(CompletionItem {
            name,
            origin: CompletionOrigin::Declaration,
            decl_kind: Some(row.kind),
            container: (row.container != DeclRow::NONE)
                .then(|| state.strings.resolve(capture.index.row(row.container).name)),
            file: Some(file),
            range: Some(row.range),
        });
    }

    // The names this file's own resolved references prove import-visible,
    // plus the prefixes it arms. The use view serves the file's rows
    // offset-ascending; the dedupe is hashed (a hub file's use count must
    // not go quadratic), the first-offset order preserved in the vecs.
    // Uses of the file's own names target the file itself — the
    // declaration pass already covers them.
    if let Some(library) = state.library_of_file.get(&file).copied() {
        let surface = state.surface(library);
        let rows = state.use_view(library).and_then(|view| {
            view.iter()
                .find(|(member, _)| *member == file)
                .map(|(_, rows)| rows)
        });
        if let (Some(surface), Some(rows)) = (surface, rows) {
            let refs = surface.refs();
            let mut seen: FxHashSet<(FileId, StringId)> = FxHashSet::default();
            let mut targets: Vec<(FileId, StringId)> = Vec::new();
            let mut prefixes: Vec<StringId> = Vec::new();
            let mut armed: FxHashSet<StringId> = FxHashSet::default();
            for &i in rows {
                let row = &refs[i as usize];
                if row.target_file != file && seen.insert((row.target_file, row.target_name)) {
                    targets.push((row.target_file, row.target_name));
                }
                if row.qualified() && armed.insert(row.qualifier) {
                    prefixes.push(row.qualifier);
                }
            }
            for (target_file, target_name) in targets {
                let name = state.strings.resolve(target_name);
                if !name.starts_with(prefix) {
                    continue;
                }
                // The target's declaration row: a resolved target is a
                // library face of the target file — resolve_at's pairing
                // invariant, the same walk its use arm runs. A phase
                // disagreement (the resolution outran the target's capture
                // or its declaring rows) skips this target's item —
                // under-report, never a panic.
                let Some(target) = state.capture(target_file) else {
                    continue;
                };
                let Some(decl) = target
                    .index
                    .by_name(target_name)
                    .and_then(|run| run.iter().find(|decl| decl.is_library_face()))
                else {
                    continue;
                };
                items.push(CompletionItem {
                    name,
                    origin: CompletionOrigin::Reference,
                    decl_kind: Some(decl.kind),
                    container: (decl.container != DeclRow::NONE)
                        .then(|| state.strings.resolve(target.index.row(decl.container).name)),
                    file: Some(target_file),
                    range: Some(decl.range),
                });
            }
            for qualifier in prefixes {
                let name = state.strings.resolve(qualifier);
                if !name.starts_with(prefix) {
                    continue;
                }
                items.push(CompletionItem {
                    name,
                    origin: CompletionOrigin::Prefix,
                    decl_kind: None,
                    container: None,
                    file: None,
                    range: None,
                });
            }
        }
    }

    // The deterministic total order, then the by-name dedupe: a name both
    // declared locally and resolved through a prefix keeps its declaration
    // row (the origin ranks put `Declaration` first), and a name resolved
    // through several prefixed paths keeps the target-ascending first.
    items.sort_by_key(|item| {
        (
            item.name,
            item.origin,
            item.file,
            item.range.map(|r| r.offset),
        )
    });
    items.dedup_by(|a, b| a.name == b.name);
    Some(CompletionAnswer { prefix, items })
}

/// Answers what the maintained index rows hold under `offset` (UTF-16 code
/// units, the engine's external convention) in `file` — the shared
/// substrate the cursor-interactive faces (goto-def, hover, rename,
/// completion context) resolve through.
///
/// # Freshness contract
///
/// [`Engine::query_symbols`]'s, verbatim: a pure read of the latest
/// [`Engine::analyze`]'s maintained state — callers analyze first, and a
/// read never applies the overlay, re-analyzes, or otherwise mutates
/// derived state. A file (or a position) the maintained rows do not cover
/// answers `None`. No side effects: resolution reads already-interned ids,
/// so not even the intern table moves.
///
/// # Answer domain
///
/// * The position inside a declaration's name token —
///   [`PositionAnswer::Declaration`].
/// * The position inside a resolved use — [`PositionAnswer::Use`] with the
///   target's identity and name span.
/// * Anything else — `None`. A use the current resolution state does not
///   resolve (unresolved bare name, member access without an armed prefix,
///   an ambiguous use) carries no target row by construction, and an
///   import prefix (`p` in `p.fa`) arms a continuation without being one —
///   both, like every non-identifier position, answer `None`; no consuming
///   face distinguishes them (each answers empty either way). Members and
///   locals resolve under the M1 chain's documented fidelity: a local
///   variable or an untyped-receiver member is a names-table row, so its
///   sites answer `None` too.
///
/// # Cost shape
///
/// Two binary searches over resident rows — the capture's decl view (span
/// start order, the state's re-indexing of the `(name, offset)` table) and
/// the library surface's use view (the file's rows, offset order, the
/// state's regrouping of the target-sorted table) — plus one name-run
/// probe for the target's span. No scan, no parse, no allocation; the
/// answer borrows the interned spellings, never clones.
pub(crate) fn resolve_at(
    state: &EngineState,
    file: FileId,
    offset: u32,
) -> Option<PositionAnswer<'_>> {
    let capture = state.capture(file)?;

    // Declaration first: declaration name tokens and use sites are disjoint
    // token spans (declaration names are never sites), so the first hit
    // decides.
    if let Some(decl) = state.decl_order(file).and_then(|order| {
        let decls = capture.index.decls();
        let cursor = order.partition_point(|&i| decls[i as usize].range.offset <= offset);
        let &row = cursor.checked_sub(1).and_then(|at| order.get(at))?;
        let row = &decls[row as usize];
        (offset < row.range.end()).then(|| DeclHit {
            file,
            kind: row.kind,
            name: state.strings.resolve(row.name),
            container: (row.container != DeclRow::NONE)
                .then(|| state.strings.resolve(capture.index.row(row.container).name)),
            range: row.range,
        })
    }) {
        return Some(PositionAnswer::Declaration(decl));
    }

    // Then resolved uses: the queried file's group of its library's
    // surface, offset-ordered by the state's use view.
    let library = *state.library_of_file.get(&file)?;
    let surface = state.surface(library)?;
    let columns = state
        .use_view(library)?
        .iter()
        .find(|(member, _)| *member == file)
        .map(|(_, rows)| rows)?;
    let refs = surface.refs();
    let cursor = columns.partition_point(|&i| refs[i as usize].range.offset <= offset);
    let &row = cursor.checked_sub(1).and_then(|at| columns.get(at))?;
    if offset >= refs[row as usize].range.end() {
        return None;
    }
    let row = &refs[row as usize];

    // The target's declaration row: a parser-level target is a library
    // face of the target file (the namespace declarer and the local hop
    // both admit library faces only), so the name run's first face row is
    // the resolution's target; a member target (#2618) names its declaring
    // CONTAINER — `(file, name)` alone cannot pick between two classes in
    // one file declaring the same member name — the span the cursor faces
    // jump to. The capture reads residency first, then the dependency
    // interface tier (a use whose declaration sits in a lazily indexed dep
    // file names its target exactly like a same-package one — the
    // cross-package rename face's substrate). A phase disagreement (the
    // resolution outran the target's capture or its declaring rows)
    // answers `None` — not found, never a panic.
    let target = state.capture(row.target_file).or_else(|| {
        state
            .deps
            .units
            .get(&row.target_file)
            .map(|unit| &unit.capture)
    })?;
    let decl = target.index.by_name(row.target_name).and_then(|run| {
        if row.target_container == StringId::NONE {
            run.iter().find(|decl| decl.is_library_face())
        } else {
            run.iter().find(|decl| {
                decl.container != DeclRow::NONE
                    && target.index.row(decl.container).name == row.target_container
            })
        }
    })?;
    Some(PositionAnswer::Use(UseHit {
        file,
        range: row.range,
        kind: row.kind,
        qualified: row.qualified(),
        prefix: row
            .qualified()
            .then(|| state.strings.resolve(row.qualifier)),
        target_file: row.target_file,
        name: state.strings.resolve(decl.name),
        decl_kind: decl.kind,
        container: (decl.container != DeclRow::NONE)
            .then(|| state.strings.resolve(target.index.row(decl.container).name)),
        target_range: decl.range,
    }))
}

// ——— Token faces ————————————————————————————————————————————————

/// The token table a token face reads: the persisted scan face when the
/// state still holds it, else a fresh local scan of the resident content.
enum Tokens {
    Face(Arc<ScannedFace>),
    Local(TokenTable),
}

impl std::ops::Deref for Tokens {
    type Target = TokenTable;

    fn deref(&self) -> &TokenTable {
        match self {
            Tokens::Face(face) => &face.tokens,
            Tokens::Local(table) => table,
        }
    }
}

/// The retained scan face of `file`: the exact token table the engine's own
/// passes parse through, addressed under the same identity the apply
/// recompute stored it under. A miss (warm-started facts, a face capped out
/// of the bounded scanned cache) re-derives the pure face with a local scan
/// — the state's documented fallback shape — read-only: a query never
/// fills the cache (the freshness contract) and never parses beyond the
/// one scan.
fn scanned(state: &EngineState, file: FileId) -> Option<Tokens> {
    if let Some(face) = state.settled_face(file) {
        return Some(Tokens::Face(face));
    }
    let facts = state.residency.get(&file)?;
    let bytes = state.store.get(facts.content)?;
    let text = std::str::from_utf8(bytes).ok()?;
    let version = crate::l1::language_version_for(&state.packages, &state.files, file);
    let (_, _, scanned, _) = crate::l1::scan_unit(text, version, file, &state.grants());
    Some(Tokens::Local(scanned.tokens))
}

/// The matching close index of the group opener at `at`, `None` for every
/// other token. The scanner's own pairing — `TokenTable::matching_end`,
/// recovery synthetics included — is the only bracket tracker the faces
/// use; none of them re-scans characters for strings, comments, or
/// interpolation, all of which the token stream has already collapsed.
fn opener_close(tokens: &TokenTable, at: usize) -> Option<usize> {
    if !tokens.get(at)?.kind.is_begin_group() {
        return None;
    }
    tokens.matching_end(at)
}

/// Answers the enclosing declaration's full span for a `name` position
/// (UTF-16) in `file` — the name token's row inflated from the name token
/// to the whole declaration (`class Foo { … }`, `void f() {}`, `int x =
/// 3;`) — the caller projection's containment inflation ([`callers`]).
///
/// # Freshness contract
///
/// [`Engine::query_symbols`]'s, verbatim. A position no declaration row
/// covers answers [`None`].
///
/// # Answer domain
///
/// A type parameter and any position that runs into the enclosing body's
/// end before finding a terminator (an enum constant between commas)
/// answer the name token itself. Otherwise the head walks back over the
/// declaration's written header (type, modifiers, metadata — balanced
/// groups included) to the previous declaration's terminator, and the tail
/// walks forward: the first declaration-level `;`, or the body `{…}` when
/// the header shape allows one — an `=` or `=>` between the name and a
/// brace marks a collection-literal initializer, not a body, so `var x =
/// {1: 2};` ends at the `;`. A multi-variable declaration
/// (`int x = 1, y = 2;`) answers the whole statement for each of its rows
/// — the comma split needs initializer structure the token stream does
/// not carry.
pub(crate) fn declaration_span(
    state: &EngineState,
    file: FileId,
    name: SourceRange,
) -> Option<SourceRange> {
    let capture = state.capture(file)?;
    let order = state.decl_order(file)?;
    let decls = capture.index.decls();
    // The row the position lands in — the declaration arm's probe
    // (resolve_at's containment law; declaration name tokens are the only
    // spans the rows cover, so a use or whitespace position falls out).
    let cursor = order.partition_point(|&i| decls[i as usize].range.offset <= name.offset);
    let &row = cursor.checked_sub(1).and_then(|at| order.get(at))?;
    let row = &decls[row as usize];
    if name.offset >= row.range.end() {
        return None;
    }
    if row.kind == DeclKind::TypeParam {
        return Some(row.range);
    }

    let tokens = scanned(state, file)?;
    let toks = tokens.as_slice();
    let name_at = toks
        .partition_point(|t| t.utf16_offset <= row.range.offset)
        .checked_sub(1)?;
    if toks[name_at].utf16_offset != row.range.offset {
        // The rows carry name-token spans; a misalignment means the state
        // the caller read has moved — no answer.
        return None;
    }

    // The head: backward over the header tokens (type, modifiers,
    // metadata — balanced groups included atomically), stopping at the
    // previous construct's terminator. An opener at balance zero begins a
    // foreign construct — stop before it.
    let mut head = name_at;
    let mut depth: u32 = 0;
    while head > 0 {
        match toks[head - 1].kind {
            TokenKind::CloseParen | TokenKind::CloseSquare => depth += 1,
            TokenKind::OpenParen | TokenKind::OpenSquare => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            TokenKind::Semicolon
            | TokenKind::OpenCurly
            | TokenKind::CloseCurly
            | TokenKind::Eof
            | TokenKind::ScriptTag => break,
            TokenKind::Identifier
            | TokenKind::Keyword(_)
            | TokenKind::At
            | TokenKind::Period
            | TokenKind::Comma
            | TokenKind::Question
            | TokenKind::Lt
            | TokenKind::Gt
            | TokenKind::GtGt
            | TokenKind::GtGtGt => head -= 1,
            _ => break,
        }
    }

    // The tail: the first declaration-level `;`, or the body `{…}` — an
    // `=` or `=>` at declaration level ahead of a brace marks an
    // initializer, not a body. Encountering the enclosing body's `}` (an
    // enum constant between commas) answers the name alone. Every arm
    // either breaks or advances `at`, so the walk terminates at EOF.
    let mut banned_body = false;
    let mut at = name_at + 1;
    let span = loop {
        let Some(token) = toks.get(at) else {
            break row.range;
        };
        match token.kind {
            TokenKind::Semicolon => {
                break SourceRange {
                    offset: toks[head].utf16_offset,
                    length: token.utf16_offset + token.byte_len - toks[head].utf16_offset,
                };
            }
            TokenKind::Eq | TokenKind::Function => {
                banned_body = true;
                at += 1;
            }
            TokenKind::OpenCurly if !banned_body => {
                let close = opener_close(&tokens, at).unwrap_or(at);
                break SourceRange {
                    offset: toks[head].utf16_offset,
                    length: toks[close].utf16_offset + toks[close].byte_len
                        - toks[head].utf16_offset,
                };
            }
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::Lt
            | TokenKind::StringInterpolationExpression => {
                at = opener_close(&tokens, at).map_or(at + 1, |close| close + 1);
            }
            TokenKind::CloseCurly | TokenKind::Eof => break row.range,
            _ => at += 1,
        }
    };
    Some(span)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::{
        CompletionOrigin, DeclKind, PositionAnswer, RefKind, ReferenceKind, declaration_span,
    };
    use crate::{CancellationToken, DEFAULT_TARGET, Engine, EngineConfig, memory_store};
    use dartq_package::PackageTable;
    use dartq_pub::{LockFile, PackageConfig, Pubspec, build_graph};
    use dartq_sdk::SdkSnapshot;
    use dartq_source::FileId;

    const A: &str = "class Foo {\n  void meth() {}\n}\nvoid fa() {}\n";
    const B: &str =
        "import 'a.dart';\nimport 'a.dart' as p;\n\nvoid use() {\n  fa();\n  p.fa();\n}\n";

    /// A throwaway pub workspace with a synthetic VM-segment SDK — the
    /// bench fixture's shape, inlined (the engine crate has no shared
    /// test-support crate to borrow it from). `extra` libraries land in
    /// `lib/` beside the two fixed files.
    fn workspace(tag: &str, extra: &[(&str, &str)]) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "dartq-engine-query-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let write = |relative: &str, content: &str| {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(path, content).expect("file");
        };
        write(
            "pubspec.yaml",
            "name: t\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            "pubspec.lock",
            "packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n",
        );
        write(
            ".dart_tool/package_config.json",
            r#"{"configVersion": 2, "packages": [{"name": "t", "rootUri": "../", "languageVersion": "3.5"}], "generator": "pub"}"#,
        );
        write("sdk/version", "3.13.1\n");
        write(
            "sdk/lib/libraries.json",
            r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#,
        );
        write(
            "sdk/lib/_internal/allowed_experiments.json",
            r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
        );
        write("lib/a.dart", A);
        write("lib/b.dart", B);
        for (path, source) in extra {
            write(path, source);
        }
        root
    }

    /// A fresh engine over the fixture workspace (no semantic diagnoser —
    /// the parser-level fixtures' posture).
    fn engine_over(root: &std::path::Path) -> Engine {
        engine_over_with(root, None)
    }

    /// [`engine_over`] with an explicit semantic diagnoser — the
    /// member-ref surface (#2618) rides the default fold, so its pins
    /// configure it where the other fixtures keep `None`.
    fn engine_over_with(
        root: &std::path::Path,
        semdiag: Option<Arc<dyn crate::SemanticDiagnoser>>,
    ) -> Engine {
        let config_dir = root.join(".dart_tool");
        let config = Arc::new(
            PackageConfig::parse(
                &std::fs::read_to_string(config_dir.join("package_config.json")).expect("config"),
            )
            .expect("valid config"),
        );
        let lock = LockFile::parse("packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n")
            .expect("valid lock");
        let pubspec = Pubspec::parse("name: t\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n")
            .expect("valid pubspec");
        let graph = build_graph(
            &config,
            &lock,
            &pubspec,
            &std::collections::BTreeMap::new(),
            None,
            &config_dir,
        )
        .expect("graph");
        Engine::new(EngineConfig {
            store: memory_store(),
            sdk: SdkSnapshot::load(&root.join("sdk"), None).expect("synthetic SDK loads"),
            packages: PackageTable::build(&config, &graph, &config_dir).expect("table"),
            pub_config: Arc::clone(&config),
            config_dir,
            semdiag,
            target: DEFAULT_TARGET.to_owned(),
            declared_variables: rustc_hash::FxHashMap::default(),
            experiments: Vec::new(),
        })
        .expect("engine")
    }

    /// The #2618 pin: a member send on an interface-typed receiver reaches
    /// the query surface — one `Invoked` reference row targeting the
    /// INTERFACE declaration (the statically resolved one), with the
    /// override closure appending the implementing declaration's
    /// `Override` row beside it.
    #[test]
    fn member_sends_reach_the_reference_surface() {
        let source = concat!(
            "abstract class R {\n",
            "  Future<void> createTask();\n",
            "}\n",
            "class Impl implements R {\n",
            "  Future<void> createTask() async {}\n",
            "}\n",
            "void f(R r) {\n",
            "  r.createTask();\n",
            "}\n"
        );
        let root = workspace("member-refs", &[("lib/m.dart", source)]);
        let mut engine = engine_over_with(&root, Some(crate::semdiag::diagnoser()));
        engine.discover_workspace().expect("discovery");
        engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");

        // The declaration surface: the interface's declaration and the
        // override, both named `createTask`.
        let decls = decl_hits(&engine, "createTask");
        assert_eq!(decls.len(), 2, "the interface declaration and the override");
        let (interface, impl_offset) = (
            source.find("createTask();").expect("interface decl"),
            source.rfind("createTask() async").expect("override decl"),
        );

        // The reference surface: exactly one Invoked row at the call site
        // targeting the interface's declaration, plus the override row at
        // the implementing declaration — the closure's widening.
        let m = engine
            .file_table()
            .try_file_id(&root.join("lib/m.dart"))
            .expect("interned");
        assert_eq!(
            ref_hits(&engine, "createTask"),
            vec![
                (
                    m,
                    m,
                    ReferenceKind::Invoked,
                    (source.find("r.createTask()").expect("call") + 2) as u32,
                    10
                ),
                (m, m, ReferenceKind::Override, impl_offset as u32, 10),
            ],
            "the call site's Invoked row, then the override's row (uses before overrides)"
        );
        assert_eq!(
            decls
                .iter()
                .filter(|d| d.0 == m && d.2 == interface as u32)
                .count(),
            1,
            "the interface declaration is in the decl set"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The qualified-name plumbing end to end: the armed-prefix hit
    /// resolves its qualifier to the interned spelling at the query
    /// boundary, the bare twin stays `None`,
    /// and the machine face's bool bit mirrors the spelling.
    #[test]
    fn prefixed_hits_resolve_the_qualifier_spelling() {
        let root = workspace("spelling", &[]);
        let mut engine = engine_over(&root);
        engine.discover_workspace().expect("discovery");
        engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");

        let answer = engine.query_symbols("fa");
        let spellings: Vec<Option<&str>> = answer.refs.iter().map(|r| r.qualified_name).collect();
        assert_eq!(
            spellings,
            vec![None, Some("p")],
            "the bare call, then the prefixed twin (offset order)"
        );
        let bits: Vec<bool> = answer.refs.iter().map(|r| r.explicit).collect();
        assert_eq!(bits, vec![false, true], "the bit mirrors the spelling");

        std::fs::remove_dir_all(&root).ok();
    }

    /// The directory face's declaration projection: `(file, kind, offset,
    /// length)` of every hit.
    fn decl_hits(engine: &Engine, name: &str) -> Vec<(FileId, DeclKind, u32, u32)> {
        engine
            .query_symbols(name)
            .decls
            .iter()
            .map(|d| (d.file, d.kind, d.range.offset, d.range.length))
            .collect()
    }

    /// The multi-byte-prefix regression (#2019): a multi-byte comment
    /// block ahead of the queried code must not shift the engine faces'
    /// coordinates. The capture rows and the position faces are UTF-16
    /// keyed (the substrate convention), so the declaration row, the
    /// reference row, and the cursor answer must sit at the UTF-16
    /// offsets — and on the multi-byte arm the byte coordinates, 12 units
    /// ahead here, must miss.
    #[test]
    fn query_faces_join_utf16_coordinates_after_a_multibyte_prefix() {
        for (source, comment_head) in [
            (
                "// ———\n// ———\nvar bad = 1;\nvoid f() {\n  print(bad);\n}\n",
                12_usize,
            ),
            (
                "// c1\n// c2\nvar bad = 1;\nvoid f() {\n  print(bad);\n}\n",
                0,
            ),
        ] {
            let root = workspace("multibyte-coordinates", &[("lib/m.dart", source)]);
            let mut engine = engine_over(&root);
            engine.discover_workspace().expect("discovery");
            let m = engine.set_content(&root.join("lib/m.dart"), source.as_bytes());
            engine
                .analyze(&[], &mut |_| {}, &CancellationToken::default())
                .expect("analyze");

            let use_byte = source.find("print(bad);").expect("repro shape") + "print(".len();
            let use_utf16: usize = source[..use_byte].chars().map(char::len_utf16).sum();
            assert_eq!(
                use_byte - use_utf16,
                comment_head,
                "the fixture must carry the documented byte/UTF-16 delta"
            );

            // The declaration row: the top-level variable's name token at
            // its UTF-16 offset (the ASCII control carries the same
            // relative layout, so the expected offsets agree by
            // construction: the delta sits wholly in the comment block;
            // the name runs 28 units back from the use across the shared
            // statement layout).
            assert_eq!(
                decl_hits(&engine, "bad"),
                vec![(m, DeclKind::TopLevelVar, (use_utf16 - 28) as u32, 3)],
                "the declaration row sits at its UTF-16 offset"
            );
            // The reference row: the read inside `f`, at its UTF-16
            // offset, kinded Read.
            assert_eq!(
                ref_hits(&engine, "bad"),
                vec![(m, m, ReferenceKind::Read, use_utf16 as u32, 3)],
                "the reference row sits at its UTF-16 offset"
            );
            // The cursor face answers at the UTF-16 coordinate.
            match engine.resolve_at(m, use_utf16 as u32) {
                Some(PositionAnswer::Use(hit)) => {
                    assert_eq!(hit.file, m);
                    assert_eq!(hit.name, "bad");
                    assert_eq!(hit.target_file, m);
                    assert_eq!(hit.range.offset as usize, use_utf16);
                }
                other => panic!("expected the bad use, got {other:?}"),
            }

            // On the multi-byte arm the byte coordinate is 12 past the
            // row: a byte-keyed position must miss (the convention does
            // not cross-match). The ASCII twin has no second coordinate
            // system.
            if comment_head > 0 {
                assert!(
                    engine.resolve_at(m, use_byte as u32).is_none(),
                    "byte coordinates must not join the UTF-16 rows"
                );
            }

            std::fs::remove_dir_all(&root).ok();
        }
    }

    /// The directory face's reference projection: `(target, use file,
    /// kind, offset, length)` of every hit.
    fn ref_hits(engine: &Engine, name: &str) -> Vec<(FileId, FileId, ReferenceKind, u32, u32)> {
        engine
            .query_symbols(name)
            .refs
            .iter()
            .map(|r| {
                (
                    r.target_file,
                    r.file,
                    r.kind,
                    r.range.offset,
                    r.range.length,
                )
            })
            .collect()
    }

    /// The retired scan face, rebuilt over the public read accessors: the
    /// fixture's captures (given ascending by id — the arena slot order the
    /// scan served), each equal-name run in offset order.
    fn decl_scan(
        engine: &Engine,
        files: &[FileId],
        name: &str,
    ) -> Vec<(FileId, DeclKind, u32, u32)> {
        let id = engine.strings().intern(name);
        files
            .iter()
            .copied()
            .filter_map(|file| Some((file, engine.file_capture(file)?)))
            .flat_map(|(file, capture)| {
                capture
                    .index
                    .by_name(id)
                    .into_iter()
                    .flatten()
                    .map(move |row| (file, row.kind, row.range.offset, row.range.length))
            })
            .collect()
    }

    /// The retired reference scan face: every live library ascending, its
    /// exact-target run in use order (the fixture's library surfaces are
    /// all live libraries, so the domain is complete).
    fn ref_scan(
        engine: &Engine,
        target: FileId,
        name: &str,
    ) -> Vec<(FileId, FileId, ReferenceKind, u32, u32)> {
        let id = engine.strings().intern(name);
        engine
            .library_surfaces()
            .filter_map(|(_, surface)| surface.references_of(target, id))
            .flatten()
            .map(|row| {
                (
                    row.target_file,
                    row.file,
                    ReferenceKind::from(row.kind),
                    row.range.offset,
                    row.range.length,
                )
            })
            .collect()
    }

    /// RESULT-SET EQUIVALENCE, the hard gate: point, hot, and miss names —
    /// the directory probes answer exactly what the retired arena scans
    /// answered, same rows, same order, same empty miss.
    #[test]
    fn directory_answers_match_the_scan_faces() {
        let root = workspace("equivalence", &[]);
        let mut engine = engine_over(&root);
        engine.discover_workspace().expect("discovery");
        let a = engine.set_content(&root.join("lib/a.dart"), A.as_bytes());
        let b = engine.set_content(&root.join("lib/b.dart"), B.as_bytes());
        engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");
        let files = [a, b];

        for name in ["Foo", "meth", "fa", "use", "nope", "zz_absent"] {
            assert_eq!(
                decl_hits(&engine, name),
                decl_scan(&engine, &files, name),
                "decls({name})"
            );
            for &target in &files {
                let mut from_answer = ref_hits(&engine, name);
                from_answer.retain(|row| row.0 == target);
                assert_eq!(
                    from_answer,
                    ref_scan(&engine, target, name),
                    "refs({name}, target {})",
                    target.index()
                );
            }
        }
        assert!(
            decl_hits(&engine, "zz_absent").is_empty() && ref_hits(&engine, "zz_absent").is_empty(),
            "a miss is empty on both faces"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The hand-built answer over the fixture workspace: one top-level
    /// `fa` in a.dart, its two uses in b.dart — the bare call first
    /// (offset order), the prefixed twin armed with its qualifier —
    /// with every ref targeting the declaring file.
    #[test]
    fn fa_answers_the_hand_built_expectations() {
        let root = workspace("hand", &[]);
        let mut engine = engine_over(&root);
        engine.discover_workspace().expect("discovery");
        let a = engine.set_content(&root.join("lib/a.dart"), A.as_bytes());
        let b = engine.set_content(&root.join("lib/b.dart"), B.as_bytes());
        engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");

        let answer = engine.query_symbols("fa");
        assert_eq!(answer.decls.len(), 1, "one declaration: {answer:?}");
        let decl = answer.decls[0];
        assert_eq!(decl.file, a);
        assert_eq!(decl.name, "fa");
        assert!(decl.container.is_none(), "top-level row");

        let uses: Vec<(FileId, FileId, bool, Option<&str>)> = answer
            .refs
            .iter()
            .map(|r| (r.target_file, r.file, r.explicit, r.qualified_name))
            .collect();
        assert_eq!(
            uses,
            vec![(a, b, false, None), (a, b, true, Some("p"))],
            "the bare call, then the prefixed twin (offset order)"
        );
        assert!(
            answer.refs[0].range.offset < answer.refs[1].range.offset,
            "offset-ascending within the use file"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The multi-decl join (the hub shape in miniature): `fa` is declared
    /// in a.dart and c.dart, and both b.dart and c.dart hold resolved uses
    /// — so both faces' postings carry two units and the join must serve
    /// exact-target rows across both. The answer stays the scan faces'
    /// exact rows in the contract order — targets ascending, library-minor
    /// within a target, offset order within a run.
    #[test]
    fn multi_decl_join_matches_the_scan_faces() {
        let c = "import 'a.dart' as q;\n\nvoid fa() {}\n\nvoid use2() {\n  fa();\n  q.fa();\n}\n";
        let root = workspace("multidecl", &[("lib/c.dart", c)]);
        let mut engine = engine_over(&root);
        engine.discover_workspace().expect("discovery");
        let a = engine.set_content(&root.join("lib/a.dart"), A.as_bytes());
        let b = engine.set_content(&root.join("lib/b.dart"), B.as_bytes());
        let c_file = engine.set_content(&root.join("lib/c.dart"), c.as_bytes());
        engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");

        let answer = engine.query_symbols("fa");
        let decl_files: Vec<FileId> = answer.decls.iter().map(|d| d.file).collect();
        assert_eq!(
            decl_files,
            vec![a, c_file],
            "both declaring files, ascending"
        );

        let targets: Vec<FileId> = answer.refs.iter().map(|r| r.target_file).collect();
        assert_eq!(
            targets,
            vec![a, a, a, c_file],
            "target-major: b's bare then prefixed use, c's prefixed use, c's self-use"
        );
        let uses: Vec<(FileId, bool, Option<&str>)> = answer
            .refs
            .iter()
            .map(|r| (r.file, r.explicit, r.qualified_name))
            .collect();
        assert_eq!(
            uses,
            vec![
                (b, false, None),
                (b, true, Some("p")),
                (c_file, true, Some("q")),
                (c_file, false, None),
            ],
            "library-minor within a target, offset order within a run"
        );

        // Set-and-order equivalence with the retired scan face, per target.
        for &target in &[a, b, c_file] {
            let mut from_answer = ref_hits(&engine, "fa");
            from_answer.retain(|row| row.0 == target);
            assert_eq!(
                from_answer,
                ref_scan(&engine, target, "fa"),
                "refs(fa -> {target:?})"
            );
        }

        std::fs::remove_dir_all(&root).ok();
    }

    /// The incremental case: reindexing one file swaps exactly its
    /// contribution. Rewriting b.dart drops its `fa` uses while the
    /// declaration stands; rewriting a.dart unlists the declaration while
    /// its other names keep their listings; restoring both contents
    /// restores the original answers.
    #[test]
    fn reindex_swaps_each_unit_contribution() {
        let root = workspace("incremental", &[]);
        let mut engine = engine_over(&root);
        engine.discover_workspace().expect("discovery");
        engine.set_content(&root.join("lib/a.dart"), A.as_bytes());
        engine.set_content(&root.join("lib/b.dart"), B.as_bytes());
        engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");
        assert_eq!(decl_hits(&engine, "fa").len(), 1);
        assert_eq!(ref_hits(&engine, "fa").len(), 2);

        engine.set_content(
            &root.join("lib/b.dart"),
            b"import 'a.dart';\n\nvoid main() {}\n",
        );
        engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");
        assert_eq!(decl_hits(&engine, "fa").len(), 1, "declarations untouched");
        assert!(
            ref_hits(&engine, "fa").is_empty(),
            "the uses dropped with the rebuilt surface"
        );

        engine.set_content(&root.join("lib/a.dart"), b"class Foo {}\n");
        engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");
        assert!(
            decl_hits(&engine, "fa").is_empty(),
            "the declaration unlisted"
        );
        assert!(ref_hits(&engine, "fa").is_empty());
        assert_eq!(
            decl_hits(&engine, "Foo").len(),
            1,
            "the surviving name keeps its listing"
        );

        engine.set_content(&root.join("lib/a.dart"), A.as_bytes());
        engine.set_content(&root.join("lib/b.dart"), B.as_bytes());
        engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");
        assert_eq!(
            decl_hits(&engine, "fa").len(),
            1,
            "the original answers return"
        );
        assert_eq!(ref_hits(&engine, "fa").len(), 2);

        std::fs::remove_dir_all(&root).ok();
    }

    /// A two-file engine analyzed once, the file ids handed back — the
    /// position tests' shared fixture.
    fn analyzed(tag: &str) -> (std::path::PathBuf, Engine, FileId, FileId) {
        let root = workspace(tag, &[]);
        let mut engine = engine_over(&root);
        engine.discover_workspace().expect("discovery");
        let a = engine.set_content(&root.join("lib/a.dart"), A.as_bytes());
        let b = engine.set_content(&root.join("lib/b.dart"), B.as_bytes());
        engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");
        (root, engine, a, b)
    }

    /// The declaration arm: the cursor on a declaration's name token
    /// answers the declaration itself — at the span start and on the last
    /// unit, with the container column for a member — and the boundary unit
    /// just past the span belongs to the next token and answers nothing.
    #[test]
    fn resolve_at_answers_the_declaration_under_the_cursor() {
        let (root, engine, a, _b) = analyzed("resolve-decl");

        let at = A.find("void fa()").expect("fa decl") + 5;
        for offset in [at, at + 1] {
            match engine.resolve_at(a, offset as u32) {
                Some(PositionAnswer::Declaration(decl)) => {
                    assert_eq!(decl.file, a);
                    assert_eq!(decl.name, "fa");
                    assert_eq!(decl.kind, DeclKind::TopLevelFn);
                    assert!(decl.container.is_none(), "top-level row");
                    assert_eq!(decl.range.offset as usize, at);
                    assert_eq!(decl.range.length, 2);
                }
                other => panic!("offset {offset}: expected the fa declaration, got {other:?}"),
            }
        }

        // One unit past the name's end is the `(` — no row covers it.
        assert!(
            engine.resolve_at(a, (at + 2) as u32).is_none(),
            "the span end belongs to the next token"
        );

        // The container row and the member's anchor column.
        let foo_at = A.find("class Foo").expect("Foo decl") + 6;
        match engine.resolve_at(a, foo_at as u32) {
            Some(PositionAnswer::Declaration(decl)) => {
                assert_eq!(decl.name, "Foo");
                assert_eq!(decl.kind, DeclKind::Class);
                assert!(decl.container.is_none());
            }
            other => panic!("expected the Foo class, got {other:?}"),
        }
        let meth_at = A.find("void meth").expect("meth decl") + 5;
        match engine.resolve_at(a, meth_at as u32) {
            Some(PositionAnswer::Declaration(decl)) => {
                assert_eq!(decl.name, "meth");
                assert_eq!(decl.container, Some("Foo"));
            }
            other => panic!("expected the meth member, got {other:?}"),
        }

        std::fs::remove_dir_all(&root).ok();
    }

    /// The use arm: a resolved use answers its target — identity, relation,
    /// qualified/prefix columns, and the target declaration's own name span
    /// the cursor faces jump to — and the prefix itself arms a continuation
    /// without being one.
    #[test]
    fn resolve_at_answers_the_target_of_resolved_uses() {
        let (root, engine, a, b) = analyzed("resolve-use");

        let decl_at = A.find("void fa()").expect("fa decl") + 5;
        let bare_at = B.find("  fa();").expect("bare call") + 2;
        match engine.resolve_at(b, bare_at as u32) {
            Some(PositionAnswer::Use(hit)) => {
                assert_eq!(hit.file, b);
                assert_eq!(hit.target_file, a);
                assert_eq!(hit.name, "fa");
                assert_eq!(hit.decl_kind, DeclKind::TopLevelFn);
                assert!(hit.container.is_none());
                assert_eq!(hit.kind, RefKind::Invoked);
                assert!(!hit.qualified);
                assert_eq!(hit.prefix, None);
                assert_eq!(hit.range.offset as usize, bare_at);
                assert_eq!(
                    hit.target_range.offset as usize, decl_at,
                    "the target's own name span"
                );
                assert_eq!(hit.target_range.length, 2);
            }
            other => panic!("expected the bare use, got {other:?}"),
        }

        let qualified_at = B.find("  p.fa();").expect("prefixed call") + 4;
        match engine.resolve_at(b, qualified_at as u32) {
            Some(PositionAnswer::Use(hit)) => {
                assert!(hit.qualified);
                assert_eq!(hit.prefix, Some("p"));
                assert_eq!(hit.target_file, a);
            }
            other => panic!("expected the prefixed use, got {other:?}"),
        }

        let prefix_at = B.find("  p.fa();").expect("prefixed call") + 2;
        assert!(
            engine.resolve_at(b, prefix_at as u32).is_none(),
            "the prefix arms the continuation without being one"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// Every uncovered shape answers `None`: whitespace between tokens, a
    /// keyword position, a use the resolution state does not resolve (the
    /// names-table residual), and an offset past the file's end.
    #[test]
    fn resolve_at_misses_are_none_at_every_uncovered_shape() {
        let c = "void w() {\n  nope;\n}\n";
        let root = workspace("resolve-miss", &[("lib/c.dart", c)]);
        let mut engine = engine_over(&root);
        engine.discover_workspace().expect("discovery");
        let b = engine.set_content(&root.join("lib/b.dart"), B.as_bytes());
        let c_file = engine.set_content(&root.join("lib/c.dart"), c.as_bytes());
        engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");

        let blank_at = B.find("\n\nvoid use").expect("blank line") + 1;
        assert!(
            engine.resolve_at(b, blank_at as u32).is_none(),
            "whitespace"
        );
        let void_at = B.find("void use").expect("keyword") + 1;
        assert!(
            engine.resolve_at(b, void_at as u32).is_none(),
            "the `void` keyword is neither a name row nor a site"
        );
        let nope_at = c.find("nope").expect("unresolved name");
        assert!(
            engine.resolve_at(c_file, nope_at as u32).is_none(),
            "an unresolved use carries no target row"
        );
        assert!(
            engine.resolve_at(b, B.len() as u32 + 10).is_none(),
            "past the file's end"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The views swap with their rows: dropping the declaration unresolves
    /// its uses (the sites fall into the names table, the answers go
    /// `None`), and restoring it restores them.
    #[test]
    fn reanalysis_swaps_the_position_answers_with_the_rows() {
        let root = workspace("resolve-swap", &[]);
        let mut engine = engine_over(&root);
        engine.discover_workspace().expect("discovery");
        let a = engine.set_content(&root.join("lib/a.dart"), A.as_bytes());
        let b = engine.set_content(&root.join("lib/b.dart"), B.as_bytes());
        engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");

        let bare_at = B.find("  fa();").expect("bare call") + 2;
        assert!(engine.resolve_at(b, bare_at as u32).is_some());

        engine.set_content(&root.join("lib/a.dart"), b"class Foo {}\n");
        engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");
        assert!(
            engine.resolve_at(b, bare_at as u32).is_none(),
            "the dropped declaration unresolves its uses"
        );

        engine.set_content(&root.join("lib/a.dart"), A.as_bytes());
        engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");
        match engine.resolve_at(b, bare_at as u32) {
            Some(PositionAnswer::Use(hit)) => assert_eq!(hit.target_file, a),
            other => panic!("the restored answers return, got {other:?}"),
        }

        std::fs::remove_dir_all(&root).ok();
    }

    /// The `(name, origin, declaring file, span)` projection of one
    /// completion answer — the assertions' shared shape.
    fn completion_rows(
        engine: &Engine,
        file: FileId,
        offset: u32,
    ) -> Vec<(&str, CompletionOrigin, Option<FileId>, Option<(u32, u32)>)> {
        engine
            .complete_at(file, offset)
            .expect("an analyzed file answers")
            .items
            .iter()
            .map(|item| {
                (
                    item.name,
                    item.origin,
                    item.file,
                    item.range.map(|r| (r.offset, r.length)),
                )
            })
            .collect()
    }

    /// The candidate composition over the two-file fixture: a.dart answers
    /// its own library-scope names (the member `meth` excluded — not a
    /// scope name), b.dart answers the names its resolved references prove
    /// import-visible plus its import prefix, all ascending by name with
    /// the prefix filter and member-context boundary holding.
    #[test]
    fn completion_answers_the_visible_names_of_a_file() {
        let (root, engine, a, b) = analyzed("completion-visible");

        // a.dart, empty prefix (the cursor at the file start): its own
        // declarations, `Foo` before `fa` (byte order), the member
        // `meth` absent.
        assert_eq!(
            completion_rows(&engine, a, 0),
            vec![
                ("Foo", CompletionOrigin::Declaration, Some(a), Some((6, 3))),
                ("fa", CompletionOrigin::Declaration, Some(a), Some((36, 2))),
            ],
        );

        // b.dart completing `fa`: the import-proven name, carrying its
        // target's identity and name span. Case-sensitive: `Foo` does not
        // answer the `fa` fragment.
        let fa_at = B.find("  fa();").expect("bare call") + 4;
        assert_eq!(
            completion_rows(&engine, b, fa_at as u32),
            vec![("fa", CompletionOrigin::Reference, Some(a), Some((36, 2)))],
        );

        // b.dart with the whole set (the cursor on whitespace): the
        // file-local `use`, the import-proven `fa`, and the armed prefix
        // `p` — a wire word with no identity columns.
        let blank_at = B.find("\n\nvoid use").expect("blank") + 1;
        assert_eq!(
            completion_rows(&engine, b, blank_at as u32),
            vec![
                ("fa", CompletionOrigin::Reference, Some(a), Some((36, 2))),
                ("p", CompletionOrigin::Prefix, None, None),
                ("use", CompletionOrigin::Declaration, Some(b), Some((45, 3))),
            ],
        );

        // The fragment filter over a keyword position (`vo|`): nothing
        // visible starts with it — the empty set, not a miss.
        let void_at = B.find("void use").expect("keyword") + 2;
        assert!(completion_rows(&engine, b, void_at as u32).is_empty());

        // The member-access boundary (`p.` and its `p.fa|` continuation):
        // M1 has no member enumeration — the empty set, the prefix echoed.
        let dot_at = B.find("  p.fa();").expect("prefixed call") + 4;
        let answer = engine.complete_at(b, dot_at as u32).expect("answer");
        assert_eq!(answer.prefix, "");
        assert!(answer.items.is_empty(), "a `p.` cursor enumerates nothing");
        // The offset `+ 6` ends the fragment (`  p.fa`): the cursor sits
        // right of the whole `fa` run, the same placement `fa_at` uses.
        let cont_at = B.find("  p.fa();").expect("prefixed call") + 6;
        let answer = engine.complete_at(b, cont_at as u32).expect("answer");
        assert_eq!(answer.prefix, "fa");
        assert!(answer.items.is_empty(), "a member continuation stays empty");

        std::fs::remove_dir_all(&root).ok();
    }

    /// The dedupe law: a name both declared locally and resolved through a
    /// prefix keeps exactly one row, its declaration (the local hop beats
    /// imports) — the file's own bare use targets itself and adds nothing.
    #[test]
    fn completion_local_beats_import_on_a_shared_name() {
        let c = "import 'a.dart' as p;\nvoid fa() {}\nvoid use3() {\n  fa();\n  p.fa();\n}\n";
        let root = workspace("completion-local", &[("lib/c.dart", c)]);
        let mut engine = engine_over(&root);
        engine.discover_workspace().expect("discovery");
        engine.set_content(&root.join("lib/a.dart"), A.as_bytes());
        let c_file = engine.set_content(&root.join("lib/c.dart"), c.as_bytes());
        engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");

        let rows = completion_rows(&engine, c_file, 0);
        let fa_rows: Vec<_> = rows.iter().filter(|row| row.0 == "fa").collect();
        assert_eq!(
            fa_rows,
            vec![&(
                "fa",
                CompletionOrigin::Declaration,
                Some(c_file),
                Some((27, 2))
            )],
            "one fa: the local declaration, the prefixed twin deduped away"
        );
        assert!(
            rows.contains(&("p", CompletionOrigin::Prefix, None, None)),
            "the prefix answers beside it: {rows:?}"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The `None` boundary: a file whose rows the latest analyze has not
    /// produced answers `None`, and so does an offset past the file's end
    /// — resolve_at's boundary, verbatim.
    #[test]
    fn completion_misses_answer_none_outside_the_maintained_rows() {
        let (root, engine, _a, b) = analyzed("completion-none");
        assert!(engine.complete_at(b, B.len() as u32 + 10).is_none());

        let mut fresh = engine_over(&root);
        let cold = fresh.set_content(&root.join("lib/cold.dart"), b"void cold() {}\n");
        assert!(fresh.complete_at(cold, 0).is_none(), "set, never analyzed");

        std::fs::remove_dir_all(&root).ok();
    }

    // ——— Token faces ————————————————————————————————————————

    /// An engine over one extra analyzed file, its id handed back — the
    /// token faces' shared fixture.
    fn analyzed_extra(tag: &str, source: &'static str) -> (std::path::PathBuf, Engine, FileId) {
        let root = workspace(tag, &[("lib/f.dart", source)]);
        let mut engine = engine_over(&root);
        engine.discover_workspace().expect("discovery");
        let f = engine.set_content(&root.join("lib/f.dart"), source.as_bytes());
        engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");
        (root, engine, f)
    }

    /// The `(start, end)` projection of one face answer — the assertions'
    /// shared shape.
    fn bounds(range: dartq_source::SourceRange) -> (u32, u32) {
        (range.offset, range.end())
    }

    /// The declaration-span inflation: a type's whole declaration, a
    /// field's statement ending at its `;` past a map-literal
    /// initializer, a method's body braces included, an enum constant
    /// and a type parameter answering their name tokens.
    #[test]
    fn declaration_span_answers_the_whole_declaration() {
        let decls = "class Foo {\n  int x = {1: 2};\n  void m() {}\n}\n\nclass Pair<T> {}\n\nenum E { one, two }\n\nint top = 3;\n";
        let (root, engine, f) = analyzed_extra("decl-span", decls);

        let name_of = |marker: &str| -> dartq_source::SourceRange {
            let offset = decls.find(marker).expect(marker) as u32;
            dartq_source::SourceRange {
                offset,
                length: marker.len() as u32,
            }
        };
        let answers =
            |marker: &str| declaration_span(&engine.state, f, name_of(marker)).map(bounds);

        // The class: keyword through the closing brace.
        assert_eq!(
            answers("Foo").expect("the class"),
            (
                decls.find("class Foo").expect("head") as u32,
                decls.find("\n\nclass Pair").expect("tail") as u32
            )
        );
        // The field: `=` marks the braces an initializer, so the `;`
        // ends it — head at the type.
        assert_eq!(
            answers("x").expect("the field"),
            (
                decls.find("int x").expect("head") as u32,
                decls.find("int x = {1: 2};").expect("tail") as u32 + 15
            )
        );
        // The method: body braces included.
        assert_eq!(
            answers("m()").expect("the method"),
            (
                decls.find("void m()").expect("head") as u32,
                decls.find("\n}\n\nclass Pair").expect("tail") as u32
            )
        );
        // A type parameter answers its name token.
        assert_eq!(
            answers("T").expect("the type parameter"),
            bounds(name_of("T"))
        );
        // The enum: the whole body.
        assert_eq!(
            answers("E").expect("the enum"),
            (
                decls.find("enum E").expect("head") as u32,
                decls.find("\n\nint top").expect("tail") as u32
            )
        );
        // An enum constant between commas: the name alone.
        assert_eq!(
            answers("one").expect("the constant"),
            bounds(name_of("one"))
        );
        // The top-level variable: head at the type, `;` end.
        assert_eq!(
            answers("top").expect("the variable"),
            (
                decls.find("int top").expect("head") as u32,
                decls.find("int top = 3;").expect("tail") as u32 + 12
            )
        );
        // A position no declaration row covers: `None`.
        let blank = decls.find("\n\nenum").expect("blank") as u32;
        assert!(
            declaration_span(
                &engine.state,
                f,
                dartq_source::SourceRange {
                    offset: blank,
                    length: 0
                }
            )
            .is_none()
        );

        std::fs::remove_dir_all(&root).ok();
    }

    // ——— Callers face ———————————————————————————————————————

    const CALLERS_A: &str = "void target() {}\nvoid wrap() {\n  target();\n}\nvoid rec() {\n  rec();\n}\nvoid take(void Function() g) {}\nvoid pass() {\n  take(target);\n}\n";
    const CALLERS_B: &str = "import 'c.dart';\nvoid main() {\n  target();\n  target();\n}\n";

    /// The callers face over the resolution face's invocation rows: the
    /// direct call and the cross-file twin project to their enclosing
    /// declarations, two sites in one caller dedupe to one row, a recursive
    /// self-call names its own declaration, a bare read (an argument
    /// position) is no caller, and a never-invoked name answers empty.
    #[test]
    fn callers_project_invocation_sites_to_their_enclosing_declarations() {
        let root = workspace(
            "callers",
            &[("lib/c.dart", CALLERS_A), ("lib/d.dart", CALLERS_B)],
        );
        let mut engine = engine_over(&root);
        engine.discover_workspace().expect("discovery");
        let c = engine.set_content(&root.join("lib/c.dart"), CALLERS_A.as_bytes());
        let d = engine.set_content(&root.join("lib/d.dart"), CALLERS_B.as_bytes());
        engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");

        fn rows<'e>(engine: &'e Engine, name: &str) -> Vec<(FileId, &'e str, u32)> {
            engine
                .query_callers(name)
                .iter()
                .map(|hit| (hit.file, hit.name, hit.range.offset))
                .collect()
        }
        let decl_at = |source: &str, marker: &str| (source.find(marker).expect(marker)) as u32;

        // `target`: wrap's direct call (c.dart) and main's two cross-file
        // sites deduped to one row (d.dart) — `pass`'s argument-position
        // read of `target` names no caller.
        assert_eq!(
            rows(&engine, "target"),
            vec![
                (c, "wrap", decl_at(CALLERS_A, "wrap")),
                (d, "main", decl_at(CALLERS_B, "main")),
            ],
            "file-ascending, one row per caller symbol"
        );
        // The recursive self-call: the caller is its own declaration.
        assert_eq!(
            rows(&engine, "rec"),
            vec![(c, "rec", decl_at(CALLERS_A, "rec()"))],
        );
        // `take` is invoked once, inside `pass`.
        assert_eq!(
            rows(&engine, "take"),
            vec![(c, "pass", decl_at(CALLERS_A, "pass"))],
        );
        // Declared, never invoked — and a name no file declares.
        assert!(rows(&engine, "meth").is_empty());
        assert!(rows(&engine, "zz_absent").is_empty());

        std::fs::remove_dir_all(&root).ok();
    }

    // ——— Override kind on the references face —————————————————

    const OVERRIDE_C: &str = "class C {\n  void m() {}\n}\nvoid m() {}\nvoid use4() {\n  m();\n}\n";
    const OVERRIDE_D: &str = "import 'c.dart';\nclass D extends C {\n  void m() {}\n}\nclass E extends D {\n  void m() {}\n}\nclass F {\n  void m() {}\n}\n";

    /// The references face's override kind: the queried member `C.m` gains
    /// one `Override` row per declaring subtype — `D.m` directly, `E.m`
    /// transitively through `D` — while the unrelated same-name member
    /// `F.m` produces no row and the top-level `m`'s own use row keeps its
    /// stored `invoked` kind. The anchor law: every row targets the queried
    /// symbol's own declaration, so `D.m` — itself an override row — is no
    /// second anchor and `E.m` reports exactly once, targeting `C.m`.
    /// Override rows are bare declarations: never `explicit`, never
    /// qualified.
    #[test]
    fn references_report_overriding_declarations_as_override() {
        let root = workspace(
            "override",
            &[("lib/c.dart", OVERRIDE_C), ("lib/d.dart", OVERRIDE_D)],
        );
        let mut engine = engine_over(&root);
        engine.discover_workspace().expect("discovery");
        let c = engine.set_content(&root.join("lib/c.dart"), OVERRIDE_C.as_bytes());
        let d = engine.set_content(&root.join("lib/d.dart"), OVERRIDE_D.as_bytes());
        engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");

        let answer = engine.query_symbols("m");
        // Every same-name declaration stays a declaration: C.m, the
        // top-level m, and the subtypes' D.m / E.m / F.m.
        let decl_files: Vec<FileId> = answer.decls.iter().map(|decl| decl.file).collect();
        assert_eq!(
            decl_files,
            vec![c, c, d, d, d],
            "C.m, top-level m, D.m, E.m, F.m"
        );

        // The rows: the top-level m's invocation first (uses before
        // overrides within the target group), then D.m and E.m as
        // overriding declarations targeting C.m's file — F.m absent.
        let rows: Vec<(FileId, FileId, ReferenceKind, u32)> = answer
            .refs
            .iter()
            .map(|r| (r.target_file, r.file, r.kind, r.range.offset))
            .collect();
        let m_sites: Vec<usize> = OVERRIDE_D
            .match_indices("void m()")
            .map(|(at, _)| at)
            .collect();
        assert_eq!(
            rows,
            vec![
                (
                    c,
                    c,
                    ReferenceKind::Invoked,
                    (OVERRIDE_C.find("m();").expect("use")) as u32
                ),
                (c, d, ReferenceKind::Override, (m_sites[0] + 5) as u32),
                (c, d, ReferenceKind::Override, (m_sites[1] + 5) as u32),
            ],
            "the stored kind survives beside the override rows; F.m is no override"
        );
        assert!(
            answer.refs[1..]
                .iter()
                .all(|r| !r.explicit && r.qualified_name.is_none()),
            "override rows are bare declaration sites"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The override-family face: anchoring at any declaration of the member
    /// — the interface, the implementing override, the extending override —
    /// answers the identical container set (the whole hierarchy declaring
    /// the member, upward walk included), while an unrelated same-name
    /// member outside the hierarchy answers its own singleton family.
    #[test]
    fn override_family_answers_the_whole_hierarchy_from_any_member() {
        let source = concat!(
            "abstract class R {\n",
            "  void m();\n",
            "}\n",
            "class A implements R {\n",
            "  void m() {}\n",
            "}\n",
            "class B extends A {\n",
            "  void m() {}\n",
            "}\n",
            "class S {\n",
            "  void m() {}\n",
            "}\n"
        );
        let root = workspace("override-family", &[("lib/m.dart", source)]);
        let mut engine = engine_over(&root);
        engine.discover_workspace().expect("discovery");
        engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");
        let m = engine
            .file_table()
            .try_file_id(&root.join("lib/m.dart"))
            .expect("interned");

        // The raw answers are identical (same set, same deterministic
        // order) from every anchor; spelling-sorted, the family is the
        // whole R/A/B hierarchy.
        let family = |container: &str| engine.override_family(m, container, "m");
        for container in ["A", "B"] {
            assert_eq!(
                family(container),
                family("R"),
                "anchored at {container}.m, the same family as at R.m"
            );
        }
        let mut spellings: Vec<&str> = family("R").iter().map(|(_, c)| *c).collect();
        spellings.sort_unstable();
        assert_eq!(spellings, vec!["A", "B", "R"]);
        // The sibling's own family: itself, none of the hierarchy's.
        assert_eq!(family("S"), vec![(m, "S")]);

        std::fs::remove_dir_all(&root).ok();
    }

    /// The diamond pin: a subtype that joins a second branch (`A extends
    /// Base implements R`) must pull that branch into the family — a
    /// one-pass up/down walk anchored at `R` would stop at R's subtypes
    /// and never climb A's other supertype, splitting the family by
    /// anchor. The fixpoint answers the identical {Base, R, A, C} from
    /// every member of the diamond.
    #[test]
    fn override_family_closes_the_diamond_through_every_branch() {
        let source = concat!(
            "class Base {\n",
            "  void m() {}\n",
            "}\n",
            "abstract class R {\n",
            "  void m();\n",
            "}\n",
            "class A extends Base implements R {\n",
            "  void m() {}\n",
            "}\n",
            "class C implements R {\n",
            "  void m() {}\n",
            "}\n"
        );
        let root = workspace("override-diamond", &[("lib/m.dart", source)]);
        let mut engine = engine_over(&root);
        engine.discover_workspace().expect("discovery");
        engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");
        let m = engine
            .file_table()
            .try_file_id(&root.join("lib/m.dart"))
            .expect("interned");

        let family = |container: &str| engine.override_family(m, container, "m");
        for container in ["R", "A", "C"] {
            assert_eq!(
                family(container),
                family("Base"),
                "anchored at {container}.m, the same family as at Base.m"
            );
        }
        let mut spellings: Vec<&str> = family("Base").iter().map(|(_, c)| *c).collect();
        spellings.sort_unstable();
        assert_eq!(spellings, vec!["A", "Base", "C", "R"]);

        std::fs::remove_dir_all(&root).ok();
    }

    /// Replaces `file`'s capture with one captured over `text` — the seam
    /// that forges a producer/consumer phase disagreement (a resolved use
    /// whose target capture lost the declaring rows) without going through
    /// the apply path.
    fn forge_capture(engine: &mut Engine, file: FileId, text: &str) {
        let capture = {
            let state = &engine.state;
            let version = crate::l1::language_version_for(&state.packages, &state.files, file);
            let (feature_set, settled, scanned, _) =
                crate::l1::scan_unit(text, version, file, &state.grants());
            let face = crate::state::ScannedFace {
                tokens: scanned.tokens,
                features: feature_set,
                version: settled,
            };
            crate::pipeline::capture_surface(file, text, &face, &state.strings)
        };
        engine.state.set_capture(file, Arc::new(capture));
    }

    /// A resolved reference whose target capture lost its declaring row is
    /// a phase disagreement: `resolve_at` answers `None` — not found — and
    /// completion skips the reference item while the file's own
    /// declarations still complete. Neither face panics.
    #[test]
    fn a_target_phase_disagreement_degrades_the_reference_faces() {
        let root = workspace("phase-miss", &[]);
        let mut engine = engine_over(&root);
        engine.discover_workspace().expect("discovery");
        engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");

        let b = engine
            .file_table()
            .try_file_id(&root.join("lib/b.dart"))
            .expect("b interned");
        let call = B.find("fa();").expect("the bare call") as u32;
        let cursor = call + "fa".len() as u32;

        // Sanity: the use resolves, and its item completes as a reference.
        assert!(
            matches!(engine.resolve_at(b, call), Some(PositionAnswer::Use(_))),
            "the bare call resolves before the forge"
        );
        let answer = engine.complete_at(b, cursor).expect("covered cursor");
        assert!(
            answer
                .items
                .iter()
                .any(|item| item.name == "fa" && matches!(item.origin, CompletionOrigin::Reference)),
            "the resolved call completes as a reference"
        );

        let a = engine
            .file_table()
            .try_file_id(&root.join("lib/a.dart"))
            .expect("a interned");
        forge_capture(&mut engine, a, "// gone\n");

        assert!(
            engine.resolve_at(b, call).is_none(),
            "the target-less use answers not-found"
        );
        // The one Reference item the prefix proved pre-forge is the one the
        // forge drops; nothing else in b's surface matches `fa`.
        let answer = engine.complete_at(b, cursor).expect("covered cursor");
        assert!(
            answer.items.is_empty(),
            "the reference item skips: {:?}",
            answer.items
        );

        std::fs::remove_dir_all(&root).ok();
    }
}
