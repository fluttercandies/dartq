//! `analyze`: apply → resolution-graph build → topological recompute of the
//! dirty libraries (batch fan-out) → k-way merge of the per-library
//! canonically-sorted diagnostic runs (incremental-l1-spec §3.2 step 6,
//! engine-api-spec §1.3).

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use dartq_diagnostics::Diagnostic;
use dartq_index::FileCapture;
use dartq_resolution::{
    Combinators, ExportEdge, ImportEdge, LibraryGraph, LibraryGraphBuilder, Namespaces,
    ResolutionCode, ResolvedTarget, UNDEFINED_HIDDEN_NAME, UNDEFINED_SHOWN_NAME,
};
use dartq_semantic::MicroKind;
use dartq_source::{FileId, FileTable, StringTable};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::budget;
use crate::cancel::{CancellationToken, Cancelled};
use crate::event::{EngineEvent, duration_us};
use crate::l1;
use crate::state::{
    DirectiveFacts, DirectiveKind, EngineState, FactsKey, LibraryId, LibraryInfo, LibraryResult,
    ScannedFace,
};
use crate::{CsrGraph, LibraryView, SemanticDiagnoser};

/// The resolution pipeline stage: the interned library graph
/// (dartq-resolution) over the current library set and the engine-library →
/// graph-library mapping. The semantic seam reads per-file element models
/// through `LibraryView::facts`; this stage carries only the cross-library
/// face. The graph's import namespaces build once per generation, on first
/// use — every dirty-library seam, surface rebuild, and query between graph
/// rebuilds shares them.
pub(crate) struct ResolvedGraph {
    pub graph: LibraryGraph,
    pub graph_ids: FxHashMap<LibraryId, dartq_resolution::LibraryId>,
    /// The synthetic `dart:` SDK face nodes by library name — the
    /// directive-side lookup for the combinator check's `dart:` face (the
    /// graph build joins External imports to these nodes; the nodes have no
    /// engine-library mapping, so `graph_ids` cannot answer for them).
    sdk_faces: FxHashMap<String, dartq_resolution::LibraryId>,
    namespaces: OnceLock<Namespaces>,
}

impl ResolvedGraph {
    /// The graph's import namespaces (built once per graph generation).
    pub(crate) fn namespaces(&self) -> &Namespaces {
        self.namespaces
            .get_or_init(|| Namespaces::build(&self.graph))
    }
}

/// The L3 write batch (PRD §17.2): the apply's changed contents persist
/// into the armed content store, then the batch flushes (manifest save +
/// cap enforcement). Store failures degrade inside the store — nothing
/// here can fail an analysis (the accelerator invariant). Runs on the
/// engine's two apply sites ([`l1::apply`] via [`crate::Engine`]'s
/// `apply_pending` and the analyze driver) so no content batch can slip
/// between them unflushed.
pub(crate) fn flush_l3(state: &mut EngineState, changed: &[FileId]) {
    let mut l3 = match state.l3.take() {
        Some(l3) => l3,
        None => return,
    };
    for file in changed {
        let bytes = state
            .residency
            .get(file)
            .and_then(|facts| state.store.get(facts.content));
        if let Some(bytes) = bytes {
            l3.put(bytes);
        }
    }
    l3.flush();
    state.l3 = Some(l3);
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
    let applied = l1::apply(state, on_event);
    // The L3 write batch (PRD §17.2): the batch's new contents persist
    // before the recompute — a cancelled analyze still leaves its bytes
    // cross-session (the store is content-addressed; correctness never
    // depends on it). Disarmed engines pay one `Option` test.
    flush_l3(state, &applied.changed);
    // The maintained index inputs refresh before anything consumes them:
    // changed files recapture (declaration spans move under body edits the
    // L2 gate keeps library-clean), so the query faces and the dirty
    // libraries' surface rebuilds below read current captures.
    drain_captures(state);

    if state.structure_dirty || graph.is_none() {
        *graph = (!state.residency.is_empty()).then(|| build_library_graph(state));
        state.structure_dirty = false;
    }

    let dirty = std::mem::take(&mut state.invalidated);
    let order = topo_order(state, &dirty);

    // Phase-boundary budget poll (PRD §17.1): the ladder's first chance to
    // descend after the apply/capture phase. Armed engines pay one `Option`
    // test here; unarmed ones a no-op. Rows anchor at the first dirty
    // library's defining file (any live library when nothing is dirty; no
    // library — an empty workspace — sheds nothing worth reporting).
    let mut budget_rows: Vec<Diagnostic> = Vec::new();
    if let Some((tier, limit_mib)) = state.poll_budget()
        && let Some(file) = budget_anchor(state, &order)
    {
        budget_rows.push(budget::degraded_diagnostic(tier, limit_mib, file));
        if let Some(sink) = state.trace.as_mut()
            && let Some(path) = state.files.path(file)
        {
            sink.memory_degrade(tier.key(), limit_mib, &path.to_string_lossy());
        }
        on_event(EngineEvent::MemoryDegrade {
            tier: tier.key(),
            limit_mib,
            file,
        });
    }

    // Parallel recompute: per-library diagnosis is a pure function of the
    // (already fresh) facts and the resolution graph, so the dirty set runs
    // in one fan-out; ordering matters only for event delivery. The token
    // checkpoint stays per library: a fired token turns the remaining items
    // into no-ops, completed libraries keep their results.
    let cancelled = AtomicBool::new(false);
    let resolved = graph.as_ref();

    // The semantic seam's shared context (#236): the import namespaces every
    // per-library fold resolves through, built once per graph generation and
    // shared by the surface rebuilds below — a per-library or per-analyze
    // build would multiply the cost, and a clean workset needs none. The
    // element models stay un-cloned here: the fold indexes only its own
    // library's members (see `semdiag`), so no per-analyze workspace-wide
    // copy exists.
    let seam = (semdiag.is_some() && !order.is_empty()).then(|| {
        resolved
            .expect("graph exists when libraries exist")
            .namespaces()
    });

    let computed: Vec<Option<(Duration, Vec<Diagnostic>)>> = order
        .par_iter()
        .map(|library| {
            if cancel.check().is_err() {
                cancelled.store(true, AtomicOrdering::Relaxed);
                return None;
            }
            let start = Instant::now();
            let diagnostics = diagnose_library(
                state,
                resolved.expect("graph exists when libraries exist"),
                semdiag,
                seam,
                *library,
            );
            Some((start.elapsed(), diagnostics))
        })
        .collect();

    // Store completed results and emit their events in work order. The run
    // enters the cache in canonical order — sorted once per recompute — and
    // moves in without a clone; every later analyze merges the stored runs
    // as-is (see `merge_canonical`). The library's symbol-index surface
    // rebuilds at the same point (index-design-spec §6/§8: whole-unit
    // replacement at the `LibraryAnalyzed` point, the Engine owning dirty
    // scheduling) — so `query_symbols` serves maintained surfaces instead of
    // recapturing the whole workspace per query.
    let mut completed: FxHashSet<LibraryId> = FxHashSet::default();
    for (library, slot) in order.iter().zip(computed) {
        let Some((duration, mut diagnostics)) = slot else {
            continue;
        };
        let resolved = resolved.expect("graph exists when libraries exist");
        diagnostics.sort_by(|a, b| compare_canonical(&state.files, a, b));
        state
            .results
            .insert(*library, LibraryResult { diagnostics });
        let info = state.libraries.info(*library).expect("live library");
        let defining = info.defining;
        let surface = library_surface(state, resolved, *library, info);
        state.library_surfaces.insert(*library, surface);
        completed.insert(*library);
        // Library-boundary budget poll (PRD §17.1): sustained pressure walks
        // the ladder across these sequential boundaries, one tier each.
        if let Some((tier, limit_mib)) = state.poll_budget() {
            budget_rows.push(budget::degraded_diagnostic(tier, limit_mib, defining));
            if let Some(sink) = state.trace.as_mut()
                && let Some(path) = state.files.path(defining)
            {
                sink.memory_degrade(tier.key(), limit_mib, &path.to_string_lossy());
            }
            on_event(EngineEvent::MemoryDegrade {
                tier: tier.key(),
                limit_mib,
                file: defining,
            });
        }
        if let Some(sink) = state.trace.as_mut()
            && let Some(path) = state.files.path(defining)
        {
            sink.library_analyzed(&path.to_string_lossy(), false, duration_us(duration));
        }
        on_event(EngineEvent::LibraryAnalyzed {
            library: *library,
            from_cache: false,
            duration,
        });
    }

    if cancelled.load(AtomicOrdering::Relaxed) {
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
        if let Some(sink) = state.trace.as_mut()
            && let Some(path) = state
                .libraries
                .info(library)
                .and_then(|info| state.files.path(info.defining))
        {
            sink.library_analyzed(&path.to_string_lossy(), true, 0);
        }
        on_event(EngineEvent::LibraryAnalyzed {
            library,
            from_cache: true,
            duration: Duration::ZERO,
        });
    }

    // Merge: every live library's (fresh or cached) run, each already in
    // canonical order — the k-way merge interleaves with borrowed
    // comparisons only: no per-diagnostic key materialization, no re-sort of
    // the cached sets. A zero-dirty analyze allocates just the output.
    let runs: Vec<&[Diagnostic]> = state
        .libraries
        .live()
        .filter_map(|library| state.results.get(&library))
        .map(|result| result.diagnostics.as_slice())
        .collect();
    let mut out = Vec::with_capacity(runs.iter().map(|run| run.len()).sum::<usize>());
    merge_canonical(&state.files, &runs, |diagnostic| {
        out.push(diagnostic.clone());
    });
    // The budget rows join last and re-sort the (rare, cold) addition into
    // the canonical order — the options-diagnostic face's shape. Empty when
    // the engine is unarmed: the output set is exactly the unbudgeted run's.
    if !budget_rows.is_empty() {
        out.extend(budget_rows);
        out.sort_by(|a, b| compare_canonical(&state.files, a, b));
    }
    Ok(out)
}

/// The file a budget row anchors at: the first dirty library's defining
/// file, else any live library's — `None` only for an empty workspace,
/// where there is nothing to shed and nothing to report against.
fn budget_anchor(state: &EngineState, order: &[LibraryId]) -> Option<FileId> {
    let defining = |library: LibraryId| state.libraries.info(library).map(|info| info.defining);
    order
        .iter()
        .find_map(|library| defining(*library))
        .or_else(|| state.libraries.live().find_map(defining))
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
            // Directive-combinator static warnings (dead_code_verifier.dart
            // :85-98/:58-70 visit the import/export directives alike): every
            // written `show`/`hide` token the resolved target's export scope
            // defines under neither spelling reports at the token, unless the
            // directive has no resolved target — the oracle's
            // nonexistent-library gate (:63/:91; the missing target is the
            // reported root cause and replaces the combinator symptom). Each
            // clause kind picks its code face (`hide` →
            // `undefined_hidden_name`, `show` → `undefined_shown_name`).
            if matches!(
                directive.kind,
                DirectiveKind::Import | DirectiveKind::Export
            ) && directive.combinators.written().next().is_some()
                && let Some((target, library_uri)) =
                    combinator_check_target(state, resolved, directive)
            {
                for found in resolved
                    .graph
                    .undefined_combinator_names(Some(target), &directive.combinators)
                {
                    let info = if found.hidden {
                        UNDEFINED_HIDDEN_NAME
                    } else {
                        UNDEFINED_SHOWN_NAME
                    };
                    out.push(l1::combinator_diagnostic(
                        info,
                        &library_uri,
                        state.strings.resolve(found.name),
                        found.span,
                        *file,
                    ));
                }
            }
        }
    }
    if let Some(diagnoser) = semdiag {
        let namespaces = seam.expect("seam context exists when a diagnoser is configured");
        // The persisted scan faces of the members (apply-phase recompute):
        // hits ride in as refcount bumps, misses fall back to a local scan
        // inside the fold — no per-member re-scan on the hot path either way.
        let scanned: FxHashMap<FileId, Arc<ScannedFace>> = members
            .iter()
            .filter_map(|&file| scanned_face(state, file).map(|face| (file, face)))
            .collect();
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
            // The defining file's strict modes govern the fold (#588 item
            // 4): the `analyzer.language.strict-*` flags resolved through
            // the governing options entry.
            strict: state.strict_modes_for(info.defining),
            scanned,
        };
        diagnoser.diagnose_library(view, &mut out);
    }
    out
}

/// The persisted scan face of a resident file, under the exact key the apply
/// recompute stored it under ([`l1::facts_key`]).
fn scanned_face(state: &EngineState, file: FileId) -> Option<Arc<ScannedFace>> {
    let facts = state.residency.get(&file)?;
    let key = l1::facts_key(&state.packages, &state.files, file, facts.content);
    state.cached_scanned(key)
}

/// Rebuilds a library's symbol-index reference surface at its
/// `LibraryAnalyzed` point: the member captures (fresh from the drain)
/// resolve through the graph namespaces — whole-unit replacement, the shape
/// `query_symbols` serves.
fn library_surface(
    state: &EngineState,
    resolved: &ResolvedGraph,
    library: LibraryId,
    info: &LibraryInfo,
) -> dartq_index::LibraryIndex {
    let mut builder = dartq_index::LibraryBuilder::new(
        resolved.graph_ids[&library],
        &resolved.graph,
        resolved.namespaces(),
    );
    for file in member_files(info) {
        builder.add_file(&state.captures[&file]);
    }
    builder.finish()
}

/// Recaptures every file whose facts moved since the last analyze (the
/// apply-phase [`crate::state::EngineState::mark_captures_dirty`] queue):
/// one capture per actually-changed file per analyze, replacing the per-
/// query whole-workspace recapture. Files removed since the queue entry
/// (change then removal between analyzes) filter out here.
///
/// Each job resolves the file's persisted scan face — a cache hit, or a
/// local scan whose face feeds back into the cache — and captures the
/// declaration + raw-site surface through those tokens. The parallel phase
/// reads shared state only; the maps fill in one sequential pass.
fn drain_captures(state: &mut EngineState) {
    let mut dirty = state.take_capture_dirty();
    if dirty.is_empty() {
        return;
    }
    dirty.sort_unstable();
    dirty.dedup();
    dirty.retain(|file| state.residency.contains_key(file));
    if dirty.is_empty() {
        return;
    }
    let captured: Vec<(FileId, FileCapture, Option<(FactsKey, ScannedFace)>)> = dirty
        .par_iter()
        .map(|&file| {
            let facts = &state.residency[&file];
            let bytes = state.store.get(facts.content).expect("resident content");
            let text = std::str::from_utf8(bytes).expect("engine content is UTF-8");
            let key = l1::facts_key(&state.packages, &state.files, file, facts.content);
            if let Some(face) = state.cached_scanned(key) {
                (
                    file,
                    capture_surface(file, text, &face, &state.strings),
                    None,
                )
            } else {
                let version = l1::language_version_for(&state.packages, &state.files, file);
                let (feature_set, scanned, _) = l1::scan_unit(text, version, file);
                let face = ScannedFace {
                    tokens: scanned.tokens,
                    features: feature_set,
                };
                let capture = capture_surface(file, text, &face, &state.strings);
                (file, capture, Some((key, face)))
            }
        })
        .collect();
    for (file, capture, miss) in captured {
        if let Some((key, face)) = miss {
            state.cache_scanned(key, face);
        }
        state.captures.insert(file, Arc::new(capture));
    }
}

/// Captures one resident file's declaration and reference surface through a
/// scan face (index-design-spec §8: parse-only, the official Layer A face).
fn capture_surface(
    file: FileId,
    text: &str,
    face: &ScannedFace,
    strings: &StringTable,
) -> FileCapture {
    dartq_index::capture(
        file,
        text,
        &face.tokens,
        dartq_parser::ParserFeatures::from_feature_set(face.features),
        strings,
    )
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

/// The explicit-import SDK library faces beyond dart:core, verbatim from the
/// reference SDK (`sdk/lib/<library>/*.dart` public top-level surface).
/// `import 'dart:<name>'` directives the resolver classified
/// [`ResolvedTarget::External`] join the matching synthetic node; a library
/// absent from the table stays edgeless (the resolver's posture).
const DART_SDK_FACES: &[(&str, &[&str])] = &[(
    "math",
    &[
        "E",
        "LN10",
        "LN2",
        "LOG10E",
        "LOG2E",
        "MutableRectangle",
        "Point",
        "Random",
        "Rectangle",
        "atan",
        "atan2",
        "acos",
        "asin",
        "cos",
        "e",
        "exp",
        "ln10",
        "ln2",
        "log",
        "log10e",
        "log2e",
        "max",
        "min",
        "pi",
        "pow",
        "sin",
        "sqrt",
        "sqrt1_2",
        "sqrt2",
        "tan",
    ],
)];

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

    // The synthetic SDK nodes: dart:core (the analyzer's synthesized implicit
    // import) plus every `dart:<library>` face an explicit import names.
    // A hit answers "defined" without an element model — kind-unknown, so
    // container and member questions about core faces suppress (the M1
    // under-report posture) — while a name outside the face still reports:
    // the edge carries declarations, not suppressions, and the semantic
    // engine's undefined guard reads broken (`target: None`) edges only.
    // The nodes have no engine-library mapping: no directive resolves to
    // them, and they contribute no dependency edges (cascade and CSR shapes
    // are untouched).
    let core_file = state.files.intern(Path::new("<dart:core>"));
    let core = builder.intern("dart:core");
    for name in DART_CORE_FACE {
        builder.declare(core, state.strings.intern(name), core_file);
    }
    let mut sdk_faces: FxHashMap<&str, dartq_resolution::LibraryId> = DART_SDK_FACES
        .iter()
        .map(|(uri, names)| {
            let node = builder.intern(&format!("dart:{uri}"));
            let file = state.files.intern(Path::new(&format!("<dart:{uri}>")));
            for name in *names {
                builder.declare(node, state.strings.intern(name), file);
            }
            (*uri, node)
        })
        .collect();
    sdk_faces.insert("core", core);

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
                    DirectiveKind::Import => {
                        // An explicit `dart:` import that classified External
                        // joins its synthetic face node; an unknown face
                        // stays edgeless (the resolver's posture).
                        let sdk_target = (directive.target == ResolvedTarget::External)
                            .then(|| {
                                directive
                                    .uri
                                    .as_ref()
                                    .and_then(|uri| uri.strip_prefix("dart:"))
                                    .and_then(|name| sdk_faces.get(name).copied())
                            })
                            .flatten();
                        builder.import(
                            graph_id,
                            ImportEdge {
                                target: sdk_target
                                    .or_else(|| graph_target(state, &graph_ids, directive.target)),
                                prefix: directive.prefix,
                                deferred: directive.deferred,
                                combinators: directive.combinators.clone(),
                            },
                        );
                    }
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
    ResolvedGraph {
        graph,
        graph_ids,
        sdk_faces: sdk_faces
            .into_iter()
            .map(|(name, node)| (name.to_owned(), node))
            .collect(),
        namespaces: OnceLock::new(),
    }
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

/// The combinator-check face of one import/export directive: its resolved
/// graph target paired with the `{0}` message spelling of that target (the
/// oracle's ``${library.uri}``, dead_code_verifier.dart:156) — `dart:<name>`
/// for the synthetic SDK faces, the defining file's absolute `file:///` URI
/// for analysis-set libraries (the oracle's corpus-run spelling; the same
/// decision dartq-resolution's `undefined_combinator_names` doc pins).
/// `None` for an edgeless directive — the oracle's nonexistent-library gate
/// (dead_code_verifier.dart:63/:91): the missing target is the reported root
/// cause and replaces the combinator symptom.
fn combinator_check_target(
    state: &EngineState,
    resolved: &ResolvedGraph,
    directive: &DirectiveFacts,
) -> Option<(dartq_resolution::LibraryId, String)> {
    match directive.target {
        ResolvedTarget::Resolved(file) => {
            let library = state.library_of_file.get(&file).copied()?;
            let info = state.libraries.info(library)?;
            let path = state.files.path(info.defining)?;
            Some((
                graph_target(state, &resolved.graph_ids, directive.target)?,
                format!("file://{}", path.display()),
            ))
        }
        ResolvedTarget::External => {
            let node = directive
                .uri
                .as_deref()
                .and_then(|uri| uri.strip_prefix("dart:"))
                .and_then(|name| resolved.sdk_faces.get(name))
                .copied()?;
            Some((node, resolved.graph.uri(node).to_owned()))
        }
        ResolvedTarget::Missing => None,
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

/// The canonical diagnostic order (engine-api-spec §1.3) as a borrowed
/// comparison: file path byte order, offset, code name — message, length,
/// and correction as deterministic tiebreakers, in the exact field order of
/// the retired owned-key sort. Everything compares in place (path bytes from
/// the file table, texts from the diagnostics), so neither the store-time
/// run sort nor the analyze-time merge materializes a key. Also the merge
/// key when the options-file rows join the analyze output (lib.rs, #457).
pub(crate) fn compare_canonical(files: &FileTable, a: &Diagnostic, b: &Diagnostic) -> Ordering {
    let path = |diagnostic: &Diagnostic| {
        files
            .path(diagnostic.file)
            .map_or(&[][..], |path| path.as_os_str().as_encoded_bytes())
    };
    path(a)
        .cmp(path(b))
        .then_with(|| a.range.offset.cmp(&b.range.offset))
        .then_with(|| a.code.name.cmp(b.code.name))
        .then_with(|| a.message.cmp(&b.message))
        .then_with(|| a.range.length.cmp(&b.range.length))
        .then_with(|| a.correction.cmp(&b.correction))
}

/// Cross-library merge of the per-library diagnostic runs, each stored in
/// canonical order ([`LibraryResult::diagnostics`]): a binary min-heap over
/// run heads pops the global minimum per output slot — O(D log L) borrowed
/// comparisons and O(L) heap space, with no per-diagnostic key allocation
/// and no re-sort of the cached sets. Ties resolve by run ordinal then
/// position — the ascending-library-id order the retired
/// concatenate-then-stable-sort preserved — so the output is byte-identical
/// to that sort.
fn merge_canonical<'a>(
    files: &FileTable,
    runs: &[&'a [Diagnostic]],
    mut emit: impl FnMut(&'a Diagnostic),
) {
    /// Min-heap sift-down over `(run, position)` entries under `less`.
    fn sift_down(
        heap: &mut [(usize, usize)],
        mut root: usize,
        less: &impl Fn(&(usize, usize), &(usize, usize)) -> bool,
    ) {
        loop {
            let left = 2 * root + 1;
            let right = left + 1;
            let mut best = root;
            if left < heap.len() && less(&heap[left], &heap[best]) {
                best = left;
            }
            if right < heap.len() && less(&heap[right], &heap[best]) {
                best = right;
            }
            if best == root {
                return;
            }
            heap.swap(root, best);
            root = best;
        }
    }

    /// Min-heap sift-up over `(run, position)` entries under `less`.
    fn sift_up(
        heap: &mut [(usize, usize)],
        mut child: usize,
        less: &impl Fn(&(usize, usize), &(usize, usize)) -> bool,
    ) {
        while child > 0 {
            let parent = (child - 1) / 2;
            if !less(&heap[child], &heap[parent]) {
                return;
            }
            heap.swap(child, parent);
            child = parent;
        }
    }

    let less = |a: &(usize, usize), b: &(usize, usize)| match compare_canonical(
        files,
        &runs[a.0][a.1],
        &runs[b.0][b.1],
    ) {
        Ordering::Less => true,
        Ordering::Equal => a < b,
        Ordering::Greater => false,
    };

    let mut heap: Vec<(usize, usize)> = Vec::new();
    for (run, source) in runs.iter().enumerate() {
        if source.is_empty() {
            continue;
        }
        heap.push((run, 0));
        let index = heap.len() - 1;
        sift_up(&mut heap, index, &less);
    }

    while let Some(&(run, position)) = heap.first() {
        emit(&runs[run][position]);
        // Vacate the root with the heap's tail, then re-enter the run's
        // successor — if the run continues — from the top.
        let last = heap.pop().expect("heap non-empty");
        if let Some(slot) = heap.first_mut() {
            *slot = last;
            sift_down(&mut heap, 0, &less);
        }
        let next = position + 1;
        if next < runs[run].len() {
            heap.push((run, next));
            let index = heap.len() - 1;
            sift_up(&mut heap, index, &less);
        }
    }
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use dartq_diagnostics::{CodeInfo, DiagnosticType, Severity};
    use dartq_source::{FileTable, SourceRange};

    use super::*;

    fn diagnostic(files: &mut FileTable, path: &str, offset: u32, message: &str) -> Diagnostic {
        let file = files.intern(Path::new(path));
        Diagnostic::new(
            Severity::Error,
            CodeInfo::new(
                "undefined_function",
                "UNDEFINED_FUNCTION",
                DiagnosticType::CompileTimeError,
                false,
            ),
            message,
            file,
            SourceRange { offset, length: 4 },
            None,
        )
    }

    /// A stored run: diagnostics of one library, canonically sorted — the
    /// shape `LibraryResult::diagnostics` has since #234.
    fn run(files: &mut FileTable, entries: &[(&str, u32, &str)]) -> Vec<Diagnostic> {
        let mut run: Vec<Diagnostic> = entries
            .iter()
            .map(|&(path, offset, message)| diagnostic(files, path, offset, message))
            .collect();
        run.sort_by(|a, b| compare_canonical(files, a, b));
        run
    }

    /// The retired owned-key full sort: the byte-parity oracle for
    /// `merge_canonical` (the #234 pre-merge shape — concatenate the runs in
    /// library-id order, then `sort_by_cached_key` the owned key).
    fn legacy_sorted(files: &FileTable, mut diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
        diagnostics.sort_by_cached_key(|diagnostic| {
            (
                files.path(diagnostic.file).map_or_else(Vec::new, |path| {
                    path.as_os_str().as_encoded_bytes().to_vec()
                }),
                diagnostic.range.offset,
                diagnostic.code.name,
                diagnostic.message.clone(),
                diagnostic.range.length,
                diagnostic.correction.clone(),
            )
        });
        diagnostics
    }

    fn merged(files: &FileTable, runs: &[&[Diagnostic]]) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        merge_canonical(files, runs, |diagnostic| out.push(diagnostic.clone()));
        out
    }

    /// The merge must reproduce the retired full sort byte for byte: paths
    /// weaving across three runs, tiebreakers below the offset level, an
    /// exact cross-run duplicate (equal keys pop in run order), and an empty
    /// run in the mix.
    #[test]
    fn merge_matches_the_owned_key_full_sort() {
        let mut files = FileTable::new();
        let library_a = run(
            &mut files,
            &[
                ("/w/z.dart", 40, "z one"),
                ("/w/a.dart", 10, "a one"),
                ("/w/a.dart", 10, "a tie a"), // same path+offset: message decides
            ],
        );
        let library_b = run(
            &mut files,
            &[
                ("/w/m.dart", 20, "m one"),
                ("/w/m.dart", 30, "m two"),
                ("/w/a.dart", 10, "a tie a"), // exact duplicate of library_a's tie
            ],
        );
        let library_c = run(
            &mut files,
            &[
                ("/w/b.dart", 5, "b one"),
                ("/w/m.dart", 20, "m one"),   // cross-run duplicate
                ("/w/a.dart", 10, "a tie b"), // length decides below the message
            ],
        );
        let stored = [
            library_a.as_slice(),
            library_b.as_slice(),
            &[][..],
            library_c.as_slice(),
        ];

        let concatenated: Vec<Diagnostic> = stored
            .iter()
            .flat_map(|library| library.iter().cloned())
            .collect();
        let expected = legacy_sorted(&files, concatenated);
        let actual = merged(&files, &stored);
        assert_eq!(expected, actual);
    }

    /// No runs, or only empty runs: the merge emits nothing.
    #[test]
    fn merge_of_empty_runs_emits_nothing() {
        let files = FileTable::new();
        assert_eq!(merged(&files, &[]), Vec::new());
        let empty: [&[Diagnostic]; 3] = [&[], &[], &[]];
        assert_eq!(merged(&files, &empty), Vec::new());
    }
}
