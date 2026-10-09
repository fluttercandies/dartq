//! `analyze`: apply → resolution-graph build → topological recompute of the
//! dirty libraries (batch fan-out) → merged, canonically sorted diagnostics
//! (incremental-l1-spec §3.2 step 6, engine-api-spec §1.3).

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use dartq_diagnostics::Diagnostic;
use dartq_resolution::{
    Combinators, ExportEdge, ImportEdge, LibraryGraph, LibraryGraphBuilder, Namespaces,
    ResolutionCode, ResolvedTarget,
};
use dartq_semantic::MicroKind;
use dartq_source::FileId;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::cancel::{CancellationToken, Cancelled};
use crate::event::EngineEvent;
use crate::l1;
use crate::state::{DirectiveKind, EngineState, LibraryId, LibraryInfo, LibraryResult};
use crate::{CsrGraph, LibraryView, SemanticDiagnoser};

/// The resolution pipeline stage: the interned library graph
/// (dartq-resolution) over the current library set and the engine-library →
/// graph-library mapping. The semantic seam reads per-file element models
/// through `LibraryView::facts`; this stage carries only the cross-library
/// face.
pub(crate) struct ResolvedGraph {
    pub graph: LibraryGraph,
    pub graph_ids: FxHashMap<LibraryId, dartq_resolution::LibraryId>,
}

/// The analyze driver (engine-api-spec §1.3): phase 1 is always
/// [`l1::apply`] (an explicit earlier apply sees an empty queue and emits
/// nothing); phase 2 recomputes the dirty libraries in import-edge
/// topological order and returns the merged diagnostic set of every live
/// library, canonically sorted. Clean libraries keep their cached results.
pub(crate) fn analyze(
    state: &mut EngineState,
    graph: &mut Option<ResolvedGraph>,
    semdiag: Option<&dyn SemanticDiagnoser>,
    targets: &[FileId],
    on_event: &mut dyn FnMut(EngineEvent),
    cancel: &CancellationToken,
) -> Result<Vec<Diagnostic>, Cancelled> {
    l1::apply(state, on_event);

    if state.structure_dirty || graph.is_none() {
        *graph = (!state.residency.is_empty()).then(|| build_library_graph(state));
        state.structure_dirty = false;
    }

    let dirty = std::mem::take(&mut state.invalidated);
    let order = topo_order(state, &dirty);

    // Parallel recompute: per-library diagnosis is a pure function of the
    // (already fresh) facts and the resolution graph, so the dirty set runs
    // in one fan-out; ordering matters only for event delivery. The token
    // checkpoint stays per library: a fired token turns the remaining items
    // into no-ops, completed libraries keep their results.
    let cancelled = AtomicBool::new(false);
    let resolved = graph.as_ref();

    // The semantic seam's shared context (#236): the import namespaces every
    // per-library fold resolves through, built once when a diagnoser is
    // configured and there is work — a per-library build would square the
    // cost, and a clean workset needs none. The element models stay
    // un-cloned here: the fold indexes only its own library's members (see
    // `semdiag`), so no per-analyze workspace-wide copy exists.
    let seam = (semdiag.is_some() && !order.is_empty())
        .then(|| Namespaces::build(&resolved.expect("graph exists when libraries exist").graph));

    let computed: Vec<Option<(Duration, Vec<Diagnostic>)>> = order
        .par_iter()
        .map(|library| {
            if cancel.check().is_err() {
                cancelled.store(true, Ordering::Relaxed);
                return None;
            }
            let start = Instant::now();
            let diagnostics = diagnose_library(
                state,
                resolved.expect("graph exists when libraries exist"),
                semdiag,
                seam.as_ref(),
                *library,
            );
            Some((start.elapsed(), diagnostics))
        })
        .collect();

    // Store completed results and emit their events in work order.
    let mut completed: FxHashSet<LibraryId> = FxHashSet::default();
    for (library, slot) in order.iter().zip(&computed) {
        let Some((duration, diagnostics)) = slot else {
            continue;
        };
        let info = state.libraries.info(*library).expect("live library");
        state.results.insert(
            *library,
            LibraryResult {
                content: info.content.clone(),
                deps: info.deps.clone(),
                part_targets: info.part_targets.clone(),
                diagnostics: diagnostics.clone(),
            },
        );
        completed.insert(*library);
        on_event(EngineEvent::LibraryAnalyzed {
            library: *library,
            from_cache: false,
            duration: *duration,
        });
    }

    if cancelled.load(Ordering::Relaxed) {
        // Cancelled run: completed libraries keep their results (complete
        // units per §3.1), the rest stay dirty for the next analyze.
        state.invalidated = dirty
            .into_iter()
            .filter(|library| !completed.contains(library))
            .collect();
        return Err(Cancelled);
    }

    // Target amplification the event contract observes: clean libraries that
    // were asked for report cache hits (§1.3/§1.4 — the AnalyzeFile face).
    let mut clean_targets: Vec<LibraryId> = targets
        .iter()
        .filter_map(|file| state.library_of_file.get(file).copied())
        .filter(|library| !completed.contains(library))
        .collect();
    clean_targets.sort_unstable();
    clean_targets.dedup();
    for library in clean_targets {
        on_event(EngineEvent::LibraryAnalyzed {
            library,
            from_cache: true,
            duration: Duration::ZERO,
        });
    }

    // Merge: every live library's (fresh or cached) set, canonically sorted.
    let mut out = Vec::new();
    for library in state.libraries.live() {
        if let Some(result) = state.results.get(&library) {
            out.extend(result.diagnostics.iter().cloned());
        }
    }
    canonical_sort(state, &mut out);
    Ok(out)
}

/// Diagnoses one library: the member files' syntax + URI-layer diagnostics,
/// the overlay-corrected URI staleness (removals the baked set cannot
/// express), then the semantic seam when configured (#120/#121 plug point).
fn diagnose_library(
    state: &EngineState,
    resolved: &ResolvedGraph,
    semdiag: Option<&dyn SemanticDiagnoser>,
    seam: Option<&Namespaces>,
    library: LibraryId,
) -> Vec<Diagnostic> {
    let info = state.libraries.info(library).expect("live library");
    let members = member_files(info);
    let mut out = Vec::new();
    for file in &members {
        let facts = &state.residency[file];
        out.extend(facts.diagnostics.iter().cloned());
        // A directive that resolved to a file which has since left residency
        // points outside the analyzed world (the graph layer already treats
        // it as edgeless); the baked diagnostic cannot see the removal — it
        // is a function of the file's own content and predates it. Targets
        // the resolver already coded (absent on disk at parse time) carry
        // their diagnostic in the baked set, so only the uncoded gap is
        // emitted here — never a duplicate.
        for directive in &facts.directives {
            if let ResolvedTarget::Resolved(target) = directive.target
                && directive.code.is_none()
                && !state.residency.contains_key(&target)
            {
                out.push(l1::uri_diagnostic(
                    ResolutionCode::UriDoesNotExist,
                    directive.uri.as_deref().unwrap_or_default(),
                    directive.uri_span,
                    *file,
                ));
            }
        }
    }
    if let Some(diagnoser) = semdiag {
        let namespaces = seam.expect("seam context exists when a diagnoser is configured");
        let view = LibraryView {
            library,
            files: &members,
            graph: &resolved.graph,
            graph_library: resolved.graph_ids[&library],
            strings: &state.strings,
            facts: &state.residency,
            sdk: &state.sdk,
            store: state.store.as_ref(),
            packages: &state.packages,
            file_table: &state.files,
            namespaces,
        };
        diagnoser.diagnose_library(view, &mut out);
    }
    out
}

/// The dart:core public face the engine's implicit core import resolves
/// against: dart:core's own top-level declarations plus its
/// `dart:async`/`dart:collection` export face, verbatim from the reference
/// SDK (`sdk/lib/core/*.dart` class/typedef/top-level-function surface).
/// Names are declared into the synthetic core graph node (`build_library_graph`);
/// a local or imported declaration of the same name shadows the face
/// (locals beat imports), matching the implicit import's lowest precedence.
const DART_CORE_FACE: &[&str] = &[
    "ArgumentError",
    "AssertionError",
    "BigInt",
    "Comparable",
    "Comparator",
    "ConcurrentModificationError",
    "DateTime",
    "Deprecated",
    "Duration",
    "Enum",
    "Error",
    "Exception",
    "Expando",
    "Finalizer",
    "FormatException",
    "Function",
    "Future",
    "FutureExtensions",
    "FutureIterable",
    "FutureRecord2",
    "FutureRecord3",
    "FutureRecord4",
    "FutureRecord5",
    "FutureRecord6",
    "FutureRecord7",
    "FutureRecord8",
    "FutureRecord9",
    "IndexError",
    "IntegerDivisionByZeroException",
    "Invocation",
    "IterableExtensions",
    "Iterator",
    "List",
    "Map",
    "MapEntry",
    "Match",
    "NoSuchMethodError",
    "Null",
    "Object",
    "OutOfMemoryError",
    "ParallelWaitError",
    "Pattern",
    "RangeError",
    "Record",
    "RegExp",
    "RegExpMatch",
    "RuneIterator",
    "Runes",
    "Set",
    "Sink",
    "StackOverflowError",
    "StackTrace",
    "StateError",
    "Stopwatch",
    "Stream",
    "String",
    "StringBuffer",
    "StringSink",
    "Symbol",
    "Type",
    "TypeError",
    "UnimplementedError",
    "UnsupportedError",
    "Uri",
    "UriData",
    "WeakReference",
    "bool",
    "double",
    "identical",
    "identityHashCode",
    "int",
    "num",
    "pragma",
    "print",
];

fn build_library_graph(state: &mut EngineState) -> ResolvedGraph {
    let mut builder = LibraryGraphBuilder::new(&state.strings);
    let mut graph_ids: FxHashMap<LibraryId, dartq_resolution::LibraryId> = FxHashMap::default();

    for library in state.libraries.live() {
        let info = state.libraries.info(library).expect("live library");
        let graph_id = builder.intern(&info.uri);
        graph_ids.insert(library, graph_id);
        for file in member_files(info) {
            for (name, declaration) in state.residency[&file].elements.declarations() {
                if declaration.kind != MicroKind::TypeParam {
                    builder.declare(graph_id, name, file);
                }
            }
        }
    }

    // The implicit `dart:core` import (the analyzer's synthesized core
    // import): one synthetic graph node carrying dart:core's public face,
    // imported unfiltered by every library. A hit answers "defined" without
    // an element model — kind-unknown, so container and member questions
    // about core faces suppress (the M1 under-report posture) — while a
    // name outside the face still reports: the edge carries declarations,
    // not suppressions, and the semantic engine's undefined guard reads
    // broken (`target: None`) edges only. The node has no engine-library
    // mapping: no directive resolves to it, and it contributes no
    // dependency edges (cascade and CSR shapes are untouched).
    let core_file = state.files.intern(Path::new("<dart:core>"));
    let core = builder.intern("dart:core");
    for name in DART_CORE_FACE {
        builder.declare(core, state.strings.intern(name), core_file);
    }

    for library in state.libraries.live() {
        let graph_id = graph_ids[&library];
        builder.import(
            graph_id,
            ImportEdge {
                target: Some(core),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        for file in member_files(state.libraries.info(library).expect("live library")) {
            for directive in &state.residency[&file].directives {
                match directive.kind {
                    DirectiveKind::Import => builder.import(
                        graph_id,
                        ImportEdge {
                            target: graph_target(state, &graph_ids, directive.target),
                            prefix: directive.prefix,
                            deferred: directive.deferred,
                            combinators: directive.combinators.clone(),
                        },
                    ),
                    DirectiveKind::Export => builder.export(
                        graph_id,
                        ExportEdge {
                            target: graph_target(state, &graph_ids, directive.target),
                            combinators: directive.combinators.clone(),
                        },
                    ),
                    _ => {}
                }
            }
        }
    }
    let graph = builder.finish();
    ResolvedGraph { graph, graph_ids }
}

/// The defining file plus the parts, part-directive order.
pub(crate) fn member_files(info: &LibraryInfo) -> Vec<FileId> {
    std::iter::once(info.defining)
        .chain(info.parts.iter().copied())
        .collect()
}

/// Maps a directive's resolved target onto a graph library: in-set file
/// targets map to their owning library; external/missing map to `None` (the
/// edge contributes no names, per the `ImportEdge` contract).
fn graph_target(
    state: &EngineState,
    graph_ids: &FxHashMap<LibraryId, dartq_resolution::LibraryId>,
    target: ResolvedTarget,
) -> Option<dartq_resolution::LibraryId> {
    match target {
        ResolvedTarget::Resolved(file) => state
            .library_of_file
            .get(&file)
            .and_then(|library| graph_ids.get(library).copied()),
        _ => None,
    }
}

/// Topological order over the dirty set: dependencies (import targets)
/// before dependents, deterministic by library id, cycles broken in id
/// order (order affects event delivery, never results).
fn topo_order(state: &EngineState, dirty: &[LibraryId]) -> Vec<LibraryId> {
    let dirty_set: FxHashSet<LibraryId> = dirty.iter().copied().collect();
    let mut indegree: FxHashMap<LibraryId, usize> = FxHashMap::default();
    let mut dependents: FxHashMap<LibraryId, Vec<LibraryId>> = FxHashMap::default();
    for &library in dirty {
        let info = state.libraries.info(library).expect("live library");
        let count = info
            .deps
            .iter()
            .filter(|dep| dirty_set.contains(dep))
            .count();
        indegree.insert(library, count);
        for dep in &info.deps {
            if dirty_set.contains(dep) {
                dependents.entry(*dep).or_default().push(library);
            }
        }
    }
    let mut ready: BinaryHeap<Reverse<LibraryId>> = dirty
        .iter()
        .filter(|library| indegree[library] == 0)
        .map(|&library| Reverse(library))
        .collect();
    let mut order = Vec::with_capacity(dirty.len());
    while let Some(Reverse(library)) = ready.pop() {
        order.push(library);
        for dependent in dependents.get(&library).into_iter().flatten() {
            let degree = indegree.get_mut(dependent).expect("dependent is dirty");
            *degree -= 1;
            if *degree == 0 {
                ready.push(Reverse(*dependent));
            }
        }
    }
    if order.len() < dirty.len() {
        // Import cycles: append the remainder in ascending id order.
        let ordered: FxHashSet<LibraryId> = order.iter().copied().collect();
        let mut rest: Vec<LibraryId> = dirty
            .iter()
            .copied()
            .filter(|library| !ordered.contains(library))
            .collect();
        rest.sort_unstable();
        order.extend(rest);
    }
    order
}

/// The canonical diagnostic order (engine-api-spec §1.3): file path byte
/// order, offset, code name — with message, length, and correction as
/// deterministic tiebreakers so incremental and full arms byte-match. The key
/// is owned (`sort_by_cached_key` keys cannot borrow the sorted items) and is
/// computed once per element.
pub(crate) fn canonical_sort(state: &EngineState, diagnostics: &mut [Diagnostic]) {
    diagnostics.sort_by_cached_key(|diagnostic| diagnostic_key(state, diagnostic));
}

fn diagnostic_key(
    state: &EngineState,
    diagnostic: &Diagnostic,
) -> (Vec<u8>, u32, &'static str, String, u32, Option<String>) {
    let path = state
        .files
        .path(diagnostic.file)
        .map_or_else(Vec::new, |path| {
            path.as_os_str().as_encoded_bytes().to_vec()
        });
    (
        path,
        diagnostic.range.offset,
        diagnostic.code.name,
        diagnostic.message.clone(),
        diagnostic.range.length,
        diagnostic.correction.clone(),
    )
}

/// Library-level import dependency graph in CSR form: `offsets[i]..offsets[i+1]`
/// slices `edges` for the library ids this library imports/exports. Node
/// space = engine [`LibraryId`] index space (retired ids leave empty rows).
/// The flat `&[u32]` views are the zero-copy export face for external
/// compute backends (GPU posture, engine-api-spec §3 rider).
pub(crate) fn library_import_csr(state: &EngineState) -> CsrGraph {
    let nodes = state.libraries.arena_len() as usize;
    let mut rows: Vec<Vec<u32>> = vec![Vec::new(); nodes];
    for (id, info) in state.libraries.iter_infos() {
        rows[id.index() as usize] = info.deps.iter().map(|dep| dep.index()).collect();
    }
    csr_from_rows(&rows)
}

/// File-level directive reference graph in CSR form: `offsets[f]..` slices
/// the resolved file targets of `f`'s import/export/part directives. Node
/// space = `FileId` index space up to the highest resident or referenced id;
/// non-resident and unresolved targets contribute no edge.
pub(crate) fn file_reference_csr(state: &EngineState) -> CsrGraph {
    let mut max = state
        .residency
        .keys()
        .map(|file| file.index())
        .max()
        .unwrap_or(0);
    for facts in state.residency.values() {
        for directive in &facts.directives {
            if let ResolvedTarget::Resolved(file) = directive.target {
                max = max.max(file.index());
            }
        }
    }
    let mut rows: Vec<Vec<u32>> = vec![Vec::new(); max as usize + 1];
    for (file, facts) in &state.residency {
        let row = &mut rows[file.index() as usize];
        for directive in &facts.directives {
            if let ResolvedTarget::Resolved(target) = directive.target {
                row.push(target.index());
            }
        }
    }
    csr_from_rows(&rows)
}

/// Packs per-node rows into the flat offsets/edges arrays.
fn csr_from_rows(rows: &[Vec<u32>]) -> CsrGraph {
    let mut offsets = Vec::with_capacity(rows.len() + 1);
    let mut edges = Vec::new();
    offsets.push(0u32);
    for row in rows {
        edges.extend_from_slice(row);
        offsets.push(edges.len() as u32);
    }
    CsrGraph { offsets, edges }
}
