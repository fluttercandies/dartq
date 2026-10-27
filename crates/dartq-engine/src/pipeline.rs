//! `analyze`: apply → resolution-graph build → topological recompute of the
//! dirty libraries (batch fan-out) → k-way merge of the per-library
//! canonically-sorted diagnostic runs. The merged output is cached against the
//! contributors' change generations (#1039, audit g1034 F1) and handed over
//! by refcount (#1387): a cycle where no contributor re-analyzed serves the
//! standing buffer as one `Arc` bump instead of re-walking the merge, and a
//! partial recompute splices only the changed contributors' runs.

use std::cmp::Ordering;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use dartq_diagnostics::{CodeInfo, DARTQ_INTERNAL_ERROR, Diagnostic};
use dartq_index::FileCapture;
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    ClassModifiers, Combinators, DUPLICATE_EXPORT, DUPLICATE_HIDDEN_NAME, DUPLICATE_IMPORT,
    DUPLICATE_SHOWN_NAME, ExportEdge, ImportEdge, LibraryClassModifiers, LibraryGraph,
    LibraryGraphBuilder, Namespaces, ResolutionCode, ResolvedTarget, UNDEFINED_HIDDEN_NAME,
    UNDEFINED_SHOWN_NAME, compose_canonical_uri,
};
use dartq_semantic::{ElementIndex, MicroKind};
use dartq_source::{FileId, FileTable, SourceRange, StringId, StringTable, saturate_slot_u32};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::budget;
use crate::cancel::{CancellationToken, Cancelled};
use crate::cycle::Condensation;
use crate::event::{EngineEvent, duration_us};
use crate::l1;
use crate::sdk_faces_gen::SDK_LIBRARY_FACES;
use crate::state::{
    DirectiveFacts, DirectiveKind, EngineState, LibraryId, LibraryInfo, LibraryResult,
    MergedOutput, ScannedFace, UnitFacts,
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
    /// The dependency-interface nodes by interned target file (`deps`
    /// module): the lazily indexed pub-cache files' graph identities, so an
    /// import edge landing outside residency resolves to a real node the
    /// same way an in-set target does.
    dep_nodes: FxHashMap<FileId, dartq_resolution::LibraryId>,
    namespaces: OnceLock<Namespaces>,
}

impl ResolvedGraph {
    /// The graph's import namespaces (built once per graph generation).
    pub(crate) fn namespaces(&self) -> &Namespaces {
        self.namespaces
            .get_or_init(|| Namespaces::build(&self.graph))
    }
}

/// The L3 write batch: the apply's changed contents persist
/// into the armed content store, then the batch flushes (journal append +
/// amortized compaction + cap enforcement). Store failures degrade inside
/// the store — nothing here can fail an analysis (the accelerator
/// invariant). Runs on the engine's two apply sites ([`l1::apply`] via
/// [`crate::Engine`]'s `apply_pending` and the analyze driver) so no
/// content batch can slip between them unflushed.
pub(crate) fn flush_l3(state: &mut EngineState, changed: &[FileId]) {
    let Some(mut l3) = state.l3.take() else {
        return;
    };
    // One batch put: the blob writes fan out, the books land serially in
    // the changed order (`ContentStore::put_known_batch`).
    let batch: Vec<(dartq_source::ContentId, &[u8])> = changed
        .iter()
        .filter_map(|file| {
            let content = state.residency.get(file)?.content;
            state.store.get(content).map(|bytes| (content, bytes))
        })
        .collect();
    l3.put_known_batch(&batch);
    drop(batch);
    l3.flush();
    state.l3 = Some(l3);
}

/// The analyze driver: phase 1 is always
/// [`l1::apply`] (an explicit earlier apply sees an empty queue and emits
/// nothing); phase 2 recomputes the dirty libraries that fail the
/// fingerprint reuse gate (g485 W2) in import-edge topological order and
/// returns the merged diagnostic set of every live library, canonically
/// sorted. Clean libraries keep their cached results.
#[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
pub(crate) fn analyze(
    state: &mut EngineState,
    graph: &mut Option<ResolvedGraph>,
    semdiag: Option<&dyn SemanticDiagnoser>,
    targets: &[FileId],
    on_event: &mut dyn FnMut(EngineEvent),
    cancel: &CancellationToken,
) -> Result<Arc<Vec<Diagnostic>>, Cancelled> {
    let applied = l1::apply(state, on_event);
    // The L3 write batch: the batch's new contents persist
    // before the recompute — a cancelled analyze still leaves its bytes
    // cross-session (the store is content-addressed; correctness never
    // depends on it). Disarmed engines pay one `Option` test.
    flush_l3(state, &applied.changed);
    // The maintained index inputs refresh before anything consumes them:
    // changed files recapture (declaration spans move under body edits the
    // L2 gate keeps library-clean), so the query faces and the dirty
    // libraries' surface rebuilds below read current captures.
    drain_captures(state);

    // The structure-fingerprint gate's refresh input: the out-of-analyze
    // apply batches ([`Engine::apply_pending`]) recorded their moved files
    // in the accumulator, and this analyze's own apply adds its sets —
    // drained together, since the accumulator is the only witness of the
    // former's moves (the local applied sets are empty there, and a missed
    // digest would serve a standing graph that lacks the batch's formed
    // libraries). Folds ahead of the gate unconditionally: the fold is
    // O(moved files), and a body-only batch's digests are no-ops.
    let mut structure_changed = std::mem::take(&mut state.structure_changed);
    let mut structure_removed = std::mem::take(&mut state.structure_removed);
    structure_changed.extend(applied.changed.iter().copied());
    // Relinked residents rewrote inside the apply: their digests refresh
    // with the batch's — a resolved-target FileId flip (the generated-file
    // fallback) moves the folded face, a pure `code` flip digests equal.
    structure_changed.extend(applied.relinked.iter().copied());
    structure_removed.extend(applied.removed.iter().copied());
    let structure_moved = refresh_structure_digests(state, &structure_changed, &structure_removed);

    if state.structure_dirty || graph.is_none() {
        // The condensation rides the same gate: any
        // structure move re-derives it — the partition wholesale, the
        // fingerprints salvaged (#1387). A cycle whose member signatures and
        // dependency digests are byte-equal to the standing ones reuses the
        // previous digest with no hash, so a structure keystroke re-folds
        // only its dirty front and probes the standing front; the values are
        // identical to a wholesale rebuild either way. The re-derivation is
        // unconditional under the flag — even when the structure digest
        // below stands — because the cycle fingerprints fold the members'
        // api signatures and the post-closure reuse gate (g485 W2) compares
        // them against the recorded runs: skipping it would serve
        // fingerprints recorded under the pre-edit faces and could hold a
        // result a dependency's member-shape move invalidated.
        if state.structure_dirty {
            let prev = std::mem::take(&mut state.condensation);
            state.condensation = Condensation::build(Some(&prev), &state.libraries, |file| {
                state.residency[&file].api_signature
            });
            if let Some(sink) = state.trace.as_mut() {
                sink.cycle_rebuilt(state.condensation.cycle_count());
            }
        }
        // The structure-fingerprint gate (#1040): the O(workspace) graph +
        // namespace rebuild runs only when a tracked file's structural face
        // moved against the standing graph (a flipped digest, a newly
        // resident file, or a removal). A signature edit that adds no
        // declaration and touches no directive recomputes the dirty
        // libraries against the standing graph and namespaces instead.
        if structure_moved || graph.is_none() {
            *graph = (!state.residency.is_empty()).then(|| build_library_graph(state));
        }
        state.structure_dirty = false;
    }

    // The fingerprint-keyed reuse gate: the closure's
    // candidates whose recorded cycle fingerprint stands keep their product
    // instead of recomputing it. The key is the W1 digest — member
    // signatures folded with the dependency-cycle fingerprint set — so
    // `recorded == current` means every member face and every transitive
    // dependency face is byte-identical to the recorded run's inputs; the
    // apply drops a result on every identity move, so a present result is
    // identity-fresh by construction, and any doubt (a mapping gap, a moved
    // digest) recomputes. Within one apply the gate never fires — every
    // seed the closure propagated from moved a face, and that move flips
    // every downstream cycle fingerprint — so it decides exactly the
    // cancelled-leftover carry-forward (#428): an invalidated library whose
    // cycle face returned (an undo) keeps its recorded run.
    let candidates = std::mem::take(&mut state.invalidated);
    let mut dirty = Vec::with_capacity(candidates.len());
    let mut reused = 0usize;
    for library in candidates {
        let stands = state.results.get(&library).is_some_and(|recorded| {
            state.condensation.fingerprint_of(library) == Some(recorded.cycle_fingerprint)
        });
        if stands {
            reused += 1;
        } else {
            dirty.push(library);
        }
    }
    if reused > 0
        && let Some(sink) = state.trace.as_mut()
    {
        sink.cycle_reused(reused);
    }
    let order = state.condensation.topo_order(&dirty);

    // Phase-boundary budget poll: the ladder's first chance to
    // descend after the apply/capture phase. Armed engines pay one `Option`
    // test here; unarmed ones a no-op. Rows anchor at the first dirty
    // library's defining file (any live library when nothing is dirty; no
    // library — an empty workspace — sheds nothing worth reporting).
    let mut budget_rows: Vec<Diagnostic> = Vec::new();
    if let Some((tier, limit_mib, structural_bytes)) = state.poll_budget()
        && let Some(file) = budget_anchor(state, &order)
    {
        budget_rows.push(budget::degraded_diagnostic(
            tier,
            limit_mib,
            structural_bytes,
            file,
        ));
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
    // build would multiply the cost, and a clean workset needs none.
    let folding = semdiag.is_some() && !order.is_empty();
    // The fold's graph rides the fold window: a queued library over no
    // standing graph is the rebuild gate's pairing miss, and the fold's
    // shared contexts skip with the graph (the per-library arms below
    // degrade to under-report, never a panic).
    let fold_graph = if folding { resolved } else { None };
    let seam = fold_graph.map(ResolvedGraph::namespaces);
    // The fold's element index (#775): every resident file's model, indexed
    // once per analyze and shared by reference through the whole fan-out —
    // the retired per-library rebuild over the full residency cost
    // O(libraries × residency) hash inserts on a cold analyze, plus a
    // transient residency-sized map per worker. The facts cannot move under
    // the fold (the jobs read `&EngineState`), so the index is a stable
    // snapshot; entries are the facts' own `Arc`s (see `semdiag`).
    let index = folding.then(|| {
        let mut index = crate::semdiag::build_index(&state.residency);
        // The dependency-interface units join the same index: a dep file's
        // containers are the faces the workspace's own rows resolve members
        // and supertypes against.
        for (&file, unit) in &state.deps.units {
            index.insert(file, Arc::clone(&unit.facts.elements));
        }
        // The SDK extension-member faces (#2186 S5): the synthetic
        // `<dart:…>` face files carry no element model, so the semantic
        // extension pool saw nothing for them and `dart:js_interop`'s
        // implicit-application members over-fired the undefined family.
        // The bundled excerpts index under the same face-file ids the
        // graph build interned — one fixed ~15-line parse per analyze.
        crate::sdk_extension_faces::insert_sdk_extension_faces(
            &mut index,
            &mut state.files,
            &state.strings,
        );
        index
    });
    // The per-library `class-modifiers` posture (#1294): every live
    // library's defining file's settled version (the `@dart` override
    // applied) resolved through the feature registry — built once per
    // analyze beside the element index and shared by every fold, the same
    // shared-artifact shape.
    let class_modifiers =
        fold_graph.map(|graph| crate::semdiag::build_class_modifiers(state, graph));

    // The generic-container name set (#2229): the clause-tree capture
    // gates on it — a member file whose containers clause none of these
    // names keeps the name-level edges byte-identically, so only
    // clause-bearing files re-parse their headers once for the written
    // argument trees. One residency scan per analyze, the same
    // shared-artifact shape as the index and the posture above.
    let generic_containers = folding.then(|| {
        let mut names = FxHashSet::default();
        for facts in state.residency.values() {
            let elements = &facts.elements;
            for &name in elements.container_order() {
                if elements
                    .container(name)
                    .is_some_and(|container| container.type_params > 0)
                {
                    names.insert(name);
                }
            }
        }
        // The dependency-interface containers join the set: a workspace
        // class writing `extends DelegatingList<XmlNode>` captures its real
        // argument tree only when the dep generic counts as one.
        for unit in state.deps.units.values() {
            let elements = &unit.facts.elements;
            for &name in elements.container_order() {
                if elements
                    .container(name)
                    .is_some_and(|container| container.type_params > 0)
                {
                    names.insert(name);
                }
            }
        }
        names
    });

    // The clause trees, captured once per analyze over every gated unit —
    // the workspace residency and the dependency units alike (the same
    // bounded gate): a library's snapshot edges the written supertype trees
    // of every container its registry reaches, and a container reached
    // through an import from a sibling workspace library (`Column<T>
    // extends Expression<T>` in a `dsl` library, read by the query-builder
    // library's fold) carries its argument-bearing edge exactly as a member
    // or a dep container does. The per-fold member capture it replaces left
    // such a container on the name-level to-bounds face, where
    // `GeneratedColumn<Object> <: Expression<Object>` answered false through
    // `Column → Expression<dynamic>` and the `is` promotion was lost. A
    // settled-face, store, or UTF-8 miss keeps the name-level edges — the
    // open direction — and each unit parses once here instead of once per
    // fold that lists it.
    let clauses = folding.then(|| {
        let generic = generic_containers.as_ref().expect("built above when folding");
        let gated: Vec<FileId> = state
            .residency
            .iter()
            .filter(|(_, facts)| captures_a_header(facts, generic))
            .map(|(&file, _)| file)
            .collect();
        let captured: Vec<dartq_semantic::ClauseTrees> = gated
            .par_iter()
            .filter_map(|&file| {
                // The persisted face when a cold recompute stored one; a
                // warm-started or not-yet-folded file re-derives the pure
                // face locally — the same fallback `class_modifiers_of`
                // runs. The capture must not depend on fold order: the
                // analyzed package's own files are exactly the ones no
                // earlier fold has cached yet, and dropping them here left
                // every in-package supertype edge on the name-level
                // to-bounds face (`Parser<dynamic>`), losing the `is`
                // promotions that ride the written clause arguments.
                let face = state.settled_face(file).map(|face| (*face).clone());
                let (face, text) = match face {
                    Some(face) => {
                        let content = state.residency[&file].content;
                        let bytes = state.store.get(content)?;
                        let text = std::str::from_utf8(bytes).ok()?;
                        (face, text)
                    }
                    None => {
                        let content = state.residency[&file].content;
                        let bytes = state.store.get(content)?;
                        let text = std::str::from_utf8(bytes).ok()?;
                        let version =
                            l1::language_version_for(&state.packages, &state.files, file);
                        let (feature_set, settled, scanned, _) =
                            l1::scan_unit(text, version, file, &state.grants());
                        (
                            crate::state::ScannedFace {
                                tokens: scanned.tokens,
                                features: feature_set,
                                version: settled,
                            },
                            text,
                        )
                    }
                };
                let mut trees = dartq_semantic::ClauseTrees::empty();
                dartq_semantic::capture_clause_trees(
                    file,
                    text,
                    &face.tokens,
                    ParserFeatures::from_feature_set(face.features),
                    &state.strings,
                    &mut trees,
                );
                Some(trees)
            })
            .collect();
        let mut trees = dartq_semantic::ClauseTrees::empty();
        for unit_trees in &captured {
            dartq_semantic::extend_clause_trees(&mut trees, unit_trees);
        }
        for (&file, unit) in &state.deps.units {
            if !captures_a_header(&unit.facts, generic) {
                continue;
            }
            let Some(bytes) = state.store.get(unit.facts.content()) else {
                continue;
            };
            let Ok(text) = std::str::from_utf8(bytes) else {
                continue;
            };
            // Dep units retain no token table (the interface face only);
            // the capture re-scans the retained bytes — a pure function of
            // (text, settled version), and the scanner is the fastest pass
            // in the pipeline. The scan's own diagnostics are dropped: a
            // dependency file never reports rows.
            let (features, _settled, scanned, _) =
                l1::scan_unit(text, unit.version, file, &state.grants());
            dartq_semantic::capture_clause_trees(
                file,
                text,
                &scanned.tokens,
                ParserFeatures::from_feature_set(features),
                &state.strings,
                &mut trees,
            );
        }
        trees
    });

    // Each job also rebuilds its library's symbol-index surface: the build
    // reads only the member captures (fresh from the drain above) and the
    // graph namespaces, so it rides the fan-out instead of the sequential
    // store loop below — where re-deriving every shed capture's sites (a
    // scan + parse per member past the site cache's cap) serialized a full
    // workspace parse on the main thread.
    let computed: Vec<
        Option<(
            Duration,
            Vec<Arc<Diagnostic>>,
            dartq_index::LibraryIndex,
            crate::state::UseView,
        )>,
    > = order
        .par_iter()
        .map(|library| {
            if cancel.check().is_err() {
                cancelled.store(true, AtomicOrdering::Relaxed);
                return None;
            }
            let start = Instant::now();
            debug_assert!(resolved.is_some(), "a queued library's graph exists");
            let Some(resolved) = resolved else {
                // The pairing miss the context builds above document: the
                // library skips this run — under-report, never a panic.
                return None;
            };
            let mut member_refs = Vec::new();
            let mut diagnostics = diagnose_library(
                state,
                resolved,
                semdiag,
                seam,
                index.as_ref(),
                class_modifiers.as_ref(),
                clauses.as_ref(),
                *library,
                &mut member_refs,
            );
            // The run's canonical sort rides the job too (a pure read of the
            // file table).
            diagnostics.sort_by(|a, b| compare_canonical(&state.files, a, b));
            let info = state.libraries.info(*library).expect("live library");
            let surface = library_surface(state, resolved, *library, info, &member_refs);
            // The surface's use view is a pure function of its rows: it
            // rides the job too, so the serial store loop only installs it.
            let view = crate::state::use_view_of(&surface);
            Some((start.elapsed(), diagnostics, surface, view))
        })
        .collect();

    // Store completed results and emit their events in work order. The run
    // enters the cache in canonical order — sorted once per recompute, in
    // its job — and moves in without a clone; every later analyze merges the stored runs
    // as-is (see `merge_canonical`). The library's symbol-index surface
    // (built in the job) installs at the same point (whole-unit
    // replacement at the `LibraryAnalyzed` point, the Engine owning dirty
    // scheduling) — so `query_symbols` serves maintained surfaces instead of
    // recapturing the whole workspace per query.
    let mut completed: FxHashSet<LibraryId> = FxHashSet::default();
    // The reference listings of every stored surface land in one batch
    // after the loop (`EngineState::set_surface`): nothing reads the
    // reference postings inside it.
    let mut listings: Vec<(StringId, LibraryId)> = Vec::new();
    for (library, slot) in order.iter().zip(computed) {
        let Some((duration, diagnostics, surface, view)) = slot else {
            continue;
        };
        // The reuse key rides the record (g485 W2): the condensation is
        // fresh for this analyze (rebuilt above under the structure gate,
        // else standing over unchanged signatures), so every live dirty
        // library answers a cycle.
        state.results.insert(
            *library,
            LibraryResult {
                diagnostics,
                // A live dirty library answers its cycle — the gap (the
                // cycle build's bail-to-empty degrade) records the zero
                // digest: content-derived digests fold to it only from
                // byte-equal inputs, where the reuse it could grant later
                // is exact anyway, so the record never false-matches a
                // moved face.
                cycle_fingerprint: state
                    .condensation
                    .fingerprint_of(*library)
                    .unwrap_or_else(|| dartq_semantic::ApiSignature::from_bytes([0; 16])),
            },
        );
        // The change-generation bump (#1039, audit g1034 F1): the library
        // re-analyzed, so every derived product keyed on its previous
        // generation — the standing merged output — is stale from this point.
        // Monotone per id and never swept: the arena freelist recycles
        // library ids, and a recycled id must not match a cache recorded
        // under its previous incarnation.
        *state.library_generations.entry(*library).or_insert(0) += 1;
        let info = state.libraries.info(*library).expect("live library");
        let defining = info.defining;
        state.set_surface(*library, surface, view, &mut listings);
        completed.insert(*library);
        // Library-boundary budget poll: sustained pressure walks
        // the ladder across these sequential boundaries, one tier each.
        if let Some((tier, limit_mib, structural_bytes)) = state.poll_budget() {
            budget_rows.push(budget::degraded_diagnostic(
                tier,
                limit_mib,
                structural_bytes,
                defining,
            ));
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

    // The stored surfaces' reference listings land before either exit: a
    // cancelled run's completed libraries keep their surfaces, and those
    // must answer the query faces exactly as a full run's do.
    state.list_references(&mut listings);

    if cancelled.load(AtomicOrdering::Relaxed) {
        // Cancelled run: completed libraries keep their results (complete
        // units), the rest stay dirty for the next analyze.
        state.invalidated = dirty
            .into_iter()
            .filter(|library| !completed.contains(library))
            .collect();
        return Err(Cancelled);
    }

    // The declaration reachability pass (#1199, #1383): lazily rebuilt —
    // the gate only moves the memo's flags; the build runs at the first
    // `Engine::reachability` read after it, salvaged per unit. The armed
    // condition is exactly the one the eager build fired on: changed or
    // removed residency (captures re-drained at the phase head) or any
    // recomputed library (its reference surface replaced above). A clean
    // cycle leaves the memo standing: zero-dirty keystrokes pay nothing,
    // and an edit whose consumer never mounts the reachability lint now
    // pays nothing for the pass either.
    state.reachability_gate(
        !applied.changed.is_empty() || !applied.removed.is_empty() || !dirty.is_empty(),
    );

    // Target amplification the event contract observes: clean libraries that
    // were asked for report cache hits.
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

    // The merged-output cache (#1039, audit g1034 F1): the standing merge of
    // every live library's run, keyed by the contributors' change
    // generations. Exact list equality between the recorded contributor list
    // and the current one ([`merge_contributors`]) means no run moved, no
    // library joined or left, and no record was dropped since the cache was
    // stored — the previous output is byte-identical to anything a fresh
    // merge would produce, so the standing rows serve directly and the
    // k-way walk (O(D log L) comparisons plus a clone per row) is skipped
    // outright. Any mismatch — a re-analyzed contributor, a retirement, a
    // recycled id — falls through to the rebuild below. The zero-dirty share
    // is a refcount bump (#1387): the caller's handle and the standing
    // buffer are the same allocation, so a cycle where nothing re-analyzed
    // no longer clones a row per output diagnostic. Budget rows are
    // cycle-specific degradation events and never enter the cache: an armed
    // cycle copies the standing rows and joins them after, exactly the
    // options-row shape of the pre-cache code.
    let contributors = merge_contributors(state);
    if state
        .merged
        .as_ref()
        .is_some_and(|cache| cache.contributors == contributors)
    {
        let cache = state.merged.as_ref().expect("cache checked fresh above");
        if budget_rows.is_empty() {
            return Ok(Arc::clone(&cache.output));
        }
        let mut out = cache.output.as_ref().clone();
        out.extend(budget_rows);
        out.sort_by(|a, b| compare_canonical(&state.files, a, b));
        return Ok(Arc::new(out));
    }

    // Rebuild: the full k-way merge over every live run — or, when the stale
    // standing output is unshared (no caller still holds a copy), a splice
    // that moves its unchanged rows and re-emits only the changed
    // contributors' fresh rows (byte-equal to the full merge; see
    // [`splice_merged`]). The rebuilt vector moves into the Arc that becomes
    // both the standing cache and the returned share — refcounts on one
    // allocation, no store-time row copy (#1387) — so an interactive stream
    // is dominated by zero-dirty cycles, and each of them walks nothing.
    let runs: Vec<&[Arc<Diagnostic>]> = state
        .libraries
        .live()
        .filter_map(|library| state.results.get(&library))
        .map(|result| result.diagnostics.as_slice())
        .collect();
    let shared: Arc<Vec<Diagnostic>> = match state.merged.take() {
        Some(cache) => match Arc::try_unwrap(cache.output) {
            Ok(old) => {
                // The changed contributors: current run holders whose
                // (library, generation) pair the cache does not record. The
                // splice drops exactly these libraries' old rows — a
                // retired library has no run here and its stale rows go the
                // same way (the splice's staleness test reads current
                // contributor standing, not just the changed set). The
                // results gate leads, mirroring [`merge_contributors`] —
                // the generation lookup reads only run holders.
                let fresh: Vec<(LibraryId, &[Arc<Diagnostic>])> = state
                    .libraries
                    .live()
                    .filter(|library| state.results.contains_key(library))
                    .filter_map(|library| {
                        let generation = state.library_generations[&library];
                        if cache.contributors.contains(&(library, generation)) {
                            return None;
                        }
                        let result = &state.results[&library];
                        Some((library, result.diagnostics.as_slice()))
                    })
                    .collect();
                let fresh_runs: Vec<&[Arc<Diagnostic>]> =
                    fresh.into_iter().map(|(_, run)| run).collect();
                Arc::new(splice_merged(&state.files, old, &fresh_runs, |row| {
                    // A row is stale when its owning library is no longer an
                    // unchanged contributor: not a current run holder
                    // (retired, or its record dropped), or re-analyzed since
                    // the cache was stored (its generation moved off the
                    // recorded pair). An ownerless row drops unconditionally:
                    // formation gives every resident file an owner (orphan
                    // parts become singleton libraries), so the full merge
                    // over the live runs never carries one — and the probe
                    // on residency buys nothing.
                    match state.library_of_file.get(&row.file) {
                        Some(owner) => {
                            !state.results.contains_key(owner)
                                || !cache
                                    .contributors
                                    .contains(&(*owner, state.library_generations[owner]))
                        }
                        None => true,
                    }
                }))
            }
            // A caller still holds the previous output: its rows cannot be
            // moved out, so the rebuild re-clones every row through the full
            // merge — the pre-cache shape, unchanged.
            Err(_) => Arc::new(merged_from_runs(&state.files, &runs)),
        },
        None => Arc::new(merged_from_runs(&state.files, &runs)),
    };
    // The rebuilt buffer is the standing output for the cycles that follow.
    // The budget rows join a private copy after the store — the cache holds
    // the unbudgeted run, the output set carries the cycle's degradation row
    // re-sorted into place (the options-diagnostic face's shape). Empty when
    // the engine is unarmed.
    state.merged = Some(MergedOutput {
        output: Arc::clone(&shared),
        contributors,
    });
    if !budget_rows.is_empty() {
        let mut out = shared.as_ref().clone();
        out.extend(budget_rows);
        out.sort_by(|a, b| compare_canonical(&state.files, a, b));
        return Ok(Arc::new(out));
    }
    Ok(shared)
}

/// The (library, generation) list of every live library contributing a run
/// to the merged output, ascending by id — the cache key of the standing
/// merged output. A stored result always carries its generation (bumped at
/// store time in the recompute loop), so the lookup cannot miss; a live
/// library without a result contributes no run and joins neither this list
/// nor the cache's recorded one.
fn merge_contributors(state: &EngineState) -> Vec<(LibraryId, u32)> {
    state
        .libraries
        .live()
        .filter(|library| state.results.contains_key(library))
        .map(|library| (library, state.library_generations[&library]))
        .collect()
}

/// The full k-way merge over every stored run — the rebuild's fallback shape
/// and the byte-parity oracle of [`splice_merged`]. Runs hold refcount
/// handles (#1501) and each row's text bytes are refcounted too (#1510), so
/// the flat copy bumps handles — it never copies message or correction
/// bytes.
fn merged_from_runs(files: &FileTable, runs: &[&[Arc<Diagnostic>]]) -> Vec<Diagnostic> {
    let mut out = Vec::with_capacity(runs.iter().map(|run| run.len()).sum());
    merge_canonical(files, runs, |diagnostic| out.push(diagnostic.clone()));
    out
}

/// Splices the fresh runs of the re-analyzed contributors into the previous
/// merged output, moving the unchanged rows instead of re-cloning them
/// (#1039's partial recompute): a row whose owning library did not
/// re-analyze keeps its bytes and its relative position (its pairwise order
/// against the other unchanged rows is key-pure, so the order the previous
/// full merge gave it is the order this rebuild needs), stale rows drop in
/// favor of their library's fresh run, and the fresh side interleaves
/// through the same k-way merge.
///
/// The result is byte-identical to a full [`merge_canonical`] over the union
/// of the runs: within the fresh side, ties resolve by run ordinal — the
/// ascending-library-id order the runs arrive in, the same rule the full
/// merge applies — and a cross-source tie (an unchanged row exactly equal to
/// a fresh row) cannot arise, because equal keys open with equal path bytes
/// and a file has one owning library, which would make it both changed and
/// unchanged. The branch takes the standing row should it ever arise,
/// keeping the walk deterministic regardless.
fn splice_merged(
    files: &FileTable,
    old: Vec<Diagnostic>,
    fresh_runs: &[&[Arc<Diagnostic>]],
    stale: impl Fn(&Diagnostic) -> bool,
) -> Vec<Diagnostic> {
    let fresh_total: usize = fresh_runs.iter().map(|run| run.len()).sum();
    let mut out = Vec::with_capacity(old.len() + fresh_total);
    // The fresh side pre-merges through the canonical heap: its rows enter
    // sorted, ties already resolved by run ordinal.
    let mut fresh: Vec<Diagnostic> = Vec::with_capacity(fresh_total);
    merge_canonical(files, fresh_runs, |diagnostic| {
        fresh.push(diagnostic.clone());
    });
    let mut fresh = fresh.into_iter().peekable();
    let mut old = old.into_iter();
    // The surviving old row under the merge cursor: fresh heads strictly
    // leading it emit ahead of it while any do (`next_if` tests and takes
    // the head in one probe), and it emits only once none does — the
    // k-way rule, with no head invariant left to assert.
    let mut held = old.next();
    while let Some(row) = held {
        if stale(&row) {
            // The owning library re-analyzed (or left): its fresh run
            // carries the replacement rows, this one goes.
            held = old.next();
            continue;
        }
        match fresh.next_if(|candidate| compare_canonical(files, candidate, &row) == Ordering::Less)
        {
            Some(candidate) => {
                out.push(candidate);
                held = Some(row);
            }
            None => {
                out.push(row);
                held = old.next();
            }
        }
    }
    // The old side is exhausted and every surviving row sits in `out`. Each
    // of them emitted only against a fresh head that did not strictly lead
    // it, and the fresh side walks in canonical order — so the unemitted
    // fresh rows all sort at or after everything emitted, and the tail is
    // exactly the k-way merge's drain. Without it, fresh rows canonically
    // after the last surviving old row are silently lost: a single-library
    // edit's whole output, the last-sorting library's fresh run.
    out.extend(fresh);
    out
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

/// Whether the unit's headers carry anything the clause-tree capture can
/// sharpen: a container clause naming a generic head (the written argument
/// tree `Box<int>` vs the to-bounds `Box<dynamic>` — mixin `on` constraints
/// ride the same capture), or a container declaring type parameters of its
/// own (#2508 — the `<T extends B>` window's bound trees feed the
/// snapshot's `ParamFacts`, and a bounded parameter transports only through
/// this capture). A bare head still maps to the same instantiate-to-bounds
/// edge the name-level arm produces, so neither arm changes the default
/// corpus's fold.
fn captures_a_header(facts: &UnitFacts, generic: &FxHashSet<StringId>) -> bool {
    let elements = &facts.elements;
    elements.container_order().iter().any(|&name| {
        let Some(container) = elements.container(name) else {
            return false;
        };
        if container.type_params > 0 {
            return true;
        }
        container
            .superclass
            .iter()
            .copied()
            .chain(container.with_clause.iter().copied())
            .chain(container.implements_clause.iter().copied())
            .chain(container.on_clause.iter().copied())
            .any(|head| generic.contains(&head))
    })
}

/// Diagnoses one library: the member files' syntax + URI-layer diagnostics,
/// the overlay-corrected URI staleness (removals the baked set cannot
/// express), then the semantic seam when configured (#120/#121 plug point).
/// The run holds refcount handles (#1501): the members' facts rows and the
/// memoized fold rows enter by bump, only this library's own fresh rows
/// take a new allocation.
#[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
fn diagnose_library(
    state: &EngineState,
    resolved: &ResolvedGraph,
    semdiag: Option<&dyn SemanticDiagnoser>,
    seam: Option<&Namespaces>,
    index: Option<&ElementIndex>,
    class_modifiers: Option<&LibraryClassModifiers>,
    clauses: Option<&dartq_semantic::ClauseTrees>,
    library: LibraryId,
    member_refs: &mut Vec<(FileId, std::sync::Arc<[dartq_semantic::MemberRef]>)>,
) -> Vec<Arc<Diagnostic>> {
    let info = state.libraries.info(library).expect("live library");
    let members = member_files(info);
    let mut out = Vec::new();
    for file in &members {
        let facts = &state.residency[file];
        // A part's missing-target row spells the RESOLVED absolute form:
        // the oracle's part-include state lands the URI on a `FileState`
        // even when the file is absent, and the non-part-kind arm reports
        // `includedFile.uriStr` — the `file://` URI
        // (library_analyzer.dart:1030-1046). Imports/exports keep the
        // written literal (`selectedUri.relativeUriStr`), so only `part`
        // rows re-spell, and only ones the URI layer coded at a target.
        out.extend(facts.diagnostics.iter().map(|row| {
            if row.code.name != "uri_does_not_exist"
                && row.code.name != "uri_has_not_been_generated"
            {
                return row.clone();
            }
            let Some(directive) = facts.directives.iter().find(|d| {
                d.kind == DirectiveKind::Part
                    && d.uri_span == row.range
                    && matches!(
                        d.code,
                        Some(
                            ResolutionCode::UriDoesNotExist
                                | ResolutionCode::UriHasNotBeenGenerated
                        )
                    )
            }) else {
                return row.clone();
            };
            let ResolvedTarget::Resolved(target) = directive.target else {
                return row.clone();
            };
            let Some(path) = state.files.path(target) else {
                return row.clone();
            };
            let code = directive.code.expect("matched some above");
            Arc::new(l1::uri_diagnostic(
                code,
                &format!("file://{}", path.display()),
                row.range,
                row.file,
            ))
        }));
        // Directive-combinator static warnings (dead_code_verifier.dart
        // :85-98/:58-70 visit the import/export directives alike): every
        // written `show`/`hide` token the resolved target's export scope
        // defines under neither spelling reports at the token, unless the
        // directive has no resolved library — the oracle's nonexistent-
        // library gate (:63/:91; the missing target is the reported root
        // cause and replaces the combinator symptom). Each clause kind
        // picks its code face (`hide` -> `undefined_hidden_name`, `show` ->
        // `undefined_shown_name`). These undefined rows are the
        // DeadCodeVerifier's, which the oracle runs ahead of the
        // imports-verification gate (library_analyzer.dart:462) — they
        // report regardless of it; the gate suppresses only the
        // ImportsVerifier faces, emitted after the semantic pass below.
        for directive in &facts.directives {
            // A directive that resolved to a file which has since left
            // residency points outside the analyzed world (the graph layer
            // already treats it as edgeless); the baked diagnostic cannot see
            // the removal — it is a function of the file's own content and
            // predates it. Targets the resolver already coded (absent on disk
            // at parse time) carry their diagnostic in the baked set, so only
            // the uncoded gap is emitted here — never a duplicate.
            if let ResolvedTarget::Resolved(target) = directive.target
                && directive.code.is_none()
                && !state.residency.contains_key(&target)
                // An indexed dependency target is the analyzed world's
                // face, not a staleness gap: its load exists exactly so
                // this check stays silent.
                && !state.deps.units.contains_key(&target)
            {
                // A part spells the resolved absolute form like the baked
                // rows above (`includedFile.uriStr`); imports/exports keep
                // the written literal.
                let uri = match directive.kind {
                    DirectiveKind::Part => state
                        .files
                        .path(target)
                        .map(|path| format!("file://{}", path.display()))
                        .unwrap_or_else(|| {
                            directive.uri.clone().unwrap_or_default().to_string()
                        }),
                    _ => directive.uri.clone().unwrap_or_default().to_string(),
                };
                out.push(Arc::new(l1::uri_diagnostic(
                    ResolutionCode::UriDoesNotExist,
                    &uri,
                    directive.uri_span,
                    *file,
                )));
            }
            if matches!(
                directive.kind,
                DirectiveKind::Import | DirectiveKind::Export
            ) && directive.combinators.written().next().is_some()
                && let Some(target) = directive_graph_target(state, resolved, directive)
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
                    // The `{0}` library spelling materializes only for a
                    // row actually reported — an all-defined directive
                    // formats nothing on the hot path.
                    if let Some(library_uri) =
                        directive_library_uri(state, resolved, target, directive)
                    {
                        out.push(Arc::new(l1::combinator_diagnostic(
                            info,
                            &library_uri,
                            state.strings.resolve(found.name),
                            found.span,
                            *file,
                        )));
                    }
                }
            }
        }
    }
    part_family_diagnostics(state, library, &members, &mut out);
    // The analyze head builds the three fold contexts beside the fold (a
    // configured diagnoser over a non-empty order with a standing graph —
    // `fold_graph` above); a `None` here is that pairing's miss — the
    // library degrades to its syntax + URI rows this run, never a panic.
    debug_assert!(
        semdiag.is_none() || (seam.is_some() && index.is_some() && class_modifiers.is_some()),
        "the fold's shared contexts exist when a diagnoser is configured",
    );
    if let (Some(diagnoser), Some(namespaces), Some(index), Some(class_modifiers)) =
        (semdiag, seam, index, class_modifiers)
    {
        // The persisted scan faces of the members (apply-phase recompute):
        // hits ride in as refcount bumps, misses fall back to a local scan
        // inside the fold — no per-member re-scan on the hot path either way.
        let scanned: FxHashMap<FileId, Arc<ScannedFace>> = members
            .iter()
            .filter_map(|&file| state.settled_face(file).map(|face| (file, face)))
            .collect();
        // The defining file's strict modes govern the fold (#588 item 4):
        // the `analyzer.language.strict-*` flags resolved through the
        // governing options entry.
        let strict = state.strict_modes_for(info.defining);
        // The clause trees (#2229) are the analyze-wide capture: every
        // gated workspace and dependency unit's written supertype trees and
        // parameter bounds, captured once before the fan-out, so a
        // container the registry reaches through an import from a sibling
        // library edges its real `extends S<T>` face here exactly as a
        // member's does. A container with no capture keeps the name-level
        // edges — an absent entry only ever under-reports.
        let empty_clauses = dartq_semantic::ClauseTrees::empty();
        let clause_trees = clauses.unwrap_or(&empty_clauses);
        // The per-library resolved-type snapshot (#2204 phase A): one
        // `dartq_types` table + hierarchy + name registry over the whole
        // residency's element index, built once per library beside the
        // shared index and shared by every member fold through the flow
        // layer's `TypeOps` seam.
        let types = dartq_semantic::LibraryTypes::build_with_clauses(
            &resolved.graph,
            namespaces,
            index,
            &state.strings,
            resolved.graph_ids[&library],
            clause_trees,
        );
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
            grants: &state.feature_grants,
            namespaces,
            index,
            // The per-library resolved-type snapshot built above (#2204
            // phase A) — the fold mounts its `TypeOps` face on each
            // member's `Flow` through `set_ops`.
            types: Some(&types),
            strict,
            scanned,
            // The per-library `class-modifiers` posture (#1294), resolved
            // once per analyze (see `semdiag::build_class_modifiers`).
            class_modifiers,
            // The member-fold memo's identity inputs (#1384): the
            // recompute loop's libraries all answer a cycle (the same
            // expectation the result store below runs on), so the
            // fingerprint lookup is total here.
            memo: crate::semdiag::MemoHandle {
                memo: &state.semdiag,
                inputs: crate::semdiag::FoldInputs {
                    generation: state.graph_generation,
                    // A live library answers its cycle — the gap (the cycle
                    // build's bail-to-empty degrade) rides the zero digest:
                    // content-derived digests fold to it only from
                    // byte-equal inputs, so memo entries recorded under it
                    // can never serve a moved face.
                    fingerprint: state
                        .condensation
                        .fingerprint_of(library)
                        .unwrap_or_else(|| dartq_semantic::ApiSignature::from_bytes([0; 16])),
                    strict,
                    posture_epoch: state.semdiag_posture_epoch,
                },
            },
            // The residency-pairing degrade log (stability lane #1459): the
            // fold records the members it skips on a store/UTF-8 miss.
            degrades: &state.residency_degrades,
        };
        diagnoser.diagnose_library(&view, &mut out);
        // The resolved member-access rows (#2618): read back through the
        // same view (the default diagnoser serves them from the fold memo
        // the call above just populated) — the surface build folds them in
        // right after this returns.
        member_refs.extend(diagnoser.member_refs(&view));
    }
    // The imports-verification faces (library_analyzer.dart:501-509): the
    // whole ImportsVerifier — duplicate imports/exports and duplicate
    // shown/hidden names alike — runs only when no member file reported a
    // resolution-level root cause (`_hasDiagnosticReportedThatPreventsImportWarnings`,
    // :605-633). Its verify-errors loop completes over every file before
    // any warnings run, so the gate reads the whole library — the rows
    // accumulated above. Without it, every resolution-broken library grows
    // duplicate-import noise on top of the root causes.
    if !suppresses_import_verification(&out) {
        for file in &members {
            let facts = &state.residency[file];
            // In-clause repeats of a target-defined name report
            // `duplicate_hidden_name`/`duplicate_shown_name` at the repeat
            // token (`imports_verifier.dart:328-358`) — inside the gated
            // ImportsVerifier (`addImports` runs behind the same check).
            for directive in &facts.directives {
                if matches!(
                    directive.kind,
                    DirectiveKind::Import | DirectiveKind::Export
                ) && directive.combinators.written().next().is_some()
                    && let Some(target) = directive_graph_target(state, resolved, directive)
                {
                    for found in resolved
                        .graph
                        .duplicate_combinator_names(Some(target), &directive.combinators)
                    {
                        let info = if found.hidden {
                            DUPLICATE_HIDDEN_NAME
                        } else {
                            DUPLICATE_SHOWN_NAME
                        };
                        out.push(Arc::new(l1::duplicate_diagnostic(info, found.span, *file)));
                    }
                }
            }
            // The import/export duplicate walk (`imports_verifier.dart:361-390`
            // `_duplicates`): the unit's registered directives grouped by
            // resolved library (the URI sort's grouping), each group's
            // shape-identical neighbors reporting at the later repeat's URI.
            for (is_import, span) in duplicate_directives(
                &facts.directives,
                |directive| duplicate_target(state, resolved, directive).is_some(),
                |a, b| duplicate_target(state, resolved, a) == duplicate_target(state, resolved, b),
            ) {
                out.push(Arc::new(l1::duplicate_diagnostic(
                    if is_import {
                        DUPLICATE_IMPORT
                    } else {
                        DUPLICATE_EXPORT
                    },
                    span,
                    *file,
                )));
            }
        }
    }
    cap_per_file(&mut out);
    out
}

/// Per-file diagnostics cap at the engine's aggregation sink
/// (defense-in-depth). The scanner and parser fuses bound their own faces
/// at 10_000 rows per file and the lint stage bounds its sink the same way
/// (`MAX_LINTS_PER_FILE`), but the rows reaching this sink — baked syntax
/// rows, URI staleness, combinators, the part family, imports verification,
/// and above all the semantic fold's report — carry no cap as a number. A
/// hostile or corrupted input must not stream an unbounded diagnostic run
/// out of a single file through any face, so past this many rows for one
/// file the remainder drop and one terminal `dartq/internal_error` row
/// records the truncation — the parser fuse's terminal shape, never a
/// silent cut.
const MAX_DIAGNOSTICS_PER_FILE: usize = 10_000;

/// Applies [`MAX_DIAGNOSTICS_PER_FILE`] to one library's accumulated run:
/// the first `CAP` rows per file in aggregation order survive, the rest
/// drop, and each capped file gains one terminal row (the same
/// `dartq/internal_error` family and severity the parser fuse lands with).
/// Runs after every face has accumulated, so the gates above (notably the
/// imports-verification read of `out`) see the accumulation exactly as
/// before; the caller's canonical sort then orders survivors and terminal
/// rows together, so the cap cannot disturb any under-cap file's output.
/// The length gate is the hot path: a library whose whole run is at or
/// under the cap cannot carry an over-cap file, so normal inputs — whose
/// per-file runs sit orders of magnitude below the cap — pay one
/// comparison.
fn cap_per_file(out: &mut Vec<Arc<Diagnostic>>) {
    if out.len() <= MAX_DIAGNOSTICS_PER_FILE {
        return;
    }
    let mut counts: FxHashMap<FileId, usize> = FxHashMap::default();
    let mut capped: FxHashSet<FileId> = FxHashSet::default();
    out.retain(|row| {
        let count = counts.entry(row.file).or_insert(0);
        if *count < MAX_DIAGNOSTICS_PER_FILE {
            *count += 1;
            true
        } else {
            capped.insert(row.file);
            false
        }
    });
    out.extend(capped.into_iter().map(|file| {
        Arc::new(Diagnostic::new(
            DARTQ_INTERNAL_ERROR.dtype.severity(),
            DARTQ_INTERNAL_ERROR,
            format!("Too many diagnostics ({MAX_DIAGNOSTICS_PER_FILE}); the rest were dropped."),
            file,
            SourceRange {
                offset: 0,
                length: 0,
            },
            None,
        ))
    }));
}

/// Whether `defining`'s settled feature set enables `enhanced-parts` — the
/// gate behind the name-form mismatch rows (the oracle's
/// `_libraryElement.featureSet`). The persisted scan face answers exactly
/// (`@dart` override applied); a miss (warm-started facts, cap eviction)
/// falls back to the granted set — the state's documented re-derivation
/// shape, override-free.
fn enhanced_parts_enabled(state: &EngineState, defining: FileId) -> bool {
    if let Some(face) = state.settled_face(defining) {
        return face
            .features
            .is_enabled(dartq_language::Feature::EnhancedParts);
    }
    let grants = state.grants();
    let version = l1::language_version_for(grants.packages, grants.files, defining);
    grants
        .granted(defining, version)
        .is_enabled(dartq_language::Feature::EnhancedParts)
}

/// The part/part-of family over one library's `part` directives (oracle
/// `library_analyzer.dart` `_resolvePartDirective`, both in-tree analyzer
/// copies verbatim — 3.15 main :1004-1113, vendored 14.5.0 :1112-1207):
/// per directive, in member-file processing order, at most one row anchors
/// on the directive's URI literal (`at(directive.uri)`), decided in the
/// oracle's order — URI-layer codes first (already baked per directive;
/// a non-resident target is the staleness face's `uri_does_not_exist`
/// above), then the included file's kind (`part_of_non_part`), then
/// uniqueness among the files that joined the library (`duplicate_part`),
/// then the included file's failed `part of` claim (`part_of_unnamed_library`
/// against an unnamed library, else `part_of_different_library` — name form
/// expected/actual library names, URI form the enclosing/included files'
/// canonical URIs). The name-form rows sit below the enhanced-parts horizon:
/// with the feature on, the part-of name may resolve through the bounded
/// name scan to a library this include knows nothing about, so the analyzer
/// reports nothing (`if (!_libraryElement.featureSet.isEnabled(
/// Feature.enhanced_parts))`); the URI-form row is ungated.
///
/// "Joined the library" is the per-library membership test (`may_claim` on
/// the library's defining file — a name-form part joins every matching
/// library, the oracle's per-library `isPartOf` linkage), which mirrors the
/// oracle's attach order: only an attached part enters the duplicate window
/// (the oracle's `_libraryFiles`), so a mismatched file included twice
/// re-reports its claim row instead of reporting `duplicate_part`.
fn part_family_diagnostics(
    state: &EngineState,
    library: LibraryId,
    members: &[FileId],
    out: &mut Vec<Arc<Diagnostic>>,
) {
    let info = state.libraries.info(library).expect("live library");
    let defining = info.defining;
    let library_name = l1::declared_library_name(&state.residency[&defining]);
    let enhanced_parts = enhanced_parts_enabled(state, info.defining);
    let mut attached: FxHashSet<FileId> = FxHashSet::default();
    for file in members {
        for directive in &state.residency[file].directives {
            if directive.kind != DirectiveKind::Part || directive.code.is_some() {
                continue;
            }
            let ResolvedTarget::Resolved(target) = directive.target else {
                continue;
            };
            let Some(target_facts) = state.residency.get(&target) else {
                continue;
            };
            let target_uri = canonical_uri_spelling(state, target);
            if !target_facts.is_part() {
                out.push(Arc::new(part_diagnostic(
                    ResolutionCode::PartOfNonPart.info(),
                    &[&target_uri],
                    directive.uri_span,
                    *file,
                )));
                continue;
            }
            if attached.contains(&target) {
                out.push(Arc::new(part_diagnostic(
                    ResolutionCode::DuplicatePart.info(),
                    &[&target_uri],
                    directive.uri_span,
                    *file,
                )));
                continue;
            }
            if l1::may_claim(state, defining, library_name, target) {
                attached.insert(target);
                continue;
            }
            // `is_part` (checked above) computed `part_of().is_some()` over
            // these same facts; a miss is a logic bug — the directive skips
            // its part-family rows this run, never a panic.
            let part_of = target_facts.part_of();
            debug_assert!(part_of.is_some(), "is_part guarantees a part-of directive");
            let Some(part_of) = part_of else {
                continue;
            };
            match part_of.name.as_deref() {
                // Name form: the mismatch rows live below the enhanced-parts
                // horizon only. With the feature on, the part-of name joins
                // through the bounded name scan and may name a library this
                // include neither claims nor knows — the URI include stays
                // silent (upstream Dart SDK source, library_analyzer.dart
                // `_resolvePartDirective`, the `Feature.enhanced_parts`
                // branch). An equal name emits nothing: the oracle's rows
                // fire only when the include failed to join, and that join
                // is decided by this same equality (`may_claim`'s
                // comparison).
                Some(name) if !enhanced_parts && library_name != Some(name) => match library_name {
                    Some(expected) => out.push(Arc::new(part_diagnostic(
                        ResolutionCode::PartOfDifferentLibrary.info(),
                        &[expected, name],
                        directive.uri_span,
                        *file,
                    ))),
                    None => out.push(Arc::new(part_diagnostic(
                        ResolutionCode::PartOfUnnamedLibrary.info(),
                        &[name],
                        directive.uri_span,
                        *file,
                    ))),
                },
                Some(_) => {}
                None => {
                    // URI form: the claim binds to the file that directly
                    // includes this part, not the defining library — a
                    // depth-2 chain (`root ← mid ← leaf`, leaf's
                    // `part of 'mid.dart'`) joins through the attached
                    // middle (upstream Dart SDK source, file_state.dart's
                    // `LibraryFileKind` part check: `Expected this library
                    // to be part of '<includer uri>'`).
                    if part_of.target == ResolvedTarget::Resolved(*file) {
                        attached.insert(target);
                        continue;
                    }
                    let enclosing_uri = canonical_uri_spelling(state, *file);
                    out.push(Arc::new(part_diagnostic(
                        ResolutionCode::PartOfDifferentLibrary.info(),
                        &[&enclosing_uri, &target_uri],
                        directive.uri_span,
                        *file,
                    )));
                }
            }
        }
    }
}

/// The file's canonical URI spelling (the oracle's `FileState.uriStr`,
/// `pathToUri`): the formation memo's entry when present, else the same
/// pure composition the memo stores — `package:` inside a package, `file://`
/// outside. Error-path only, so no memo write back.
fn canonical_uri_spelling(state: &EngineState, file: FileId) -> String {
    if let Some(uri) = state.canonical_uris.get(&file) {
        return uri.to_string();
    }
    state
        .files
        .path(file)
        .map(|path| compose_canonical_uri(&state.uri_packages, path).into())
        .unwrap_or_default()
}

/// One part-family row: the resolution code's descriptor with its
/// positional `{N}` arguments filled in order (at most two across the
/// family), the descriptor deciding type, severity, and correction.
fn part_diagnostic(
    info: dartq_resolution::ResolutionCodeInfo,
    args: &[&str],
    range: SourceRange,
    file: FileId,
) -> Diagnostic {
    let mut message = info.message.to_owned();
    for (index, arg) in args.iter().enumerate() {
        message = message.replace(&format!("{{{index}}}"), arg);
    }
    Diagnostic::new(
        info.dtype.severity(),
        CodeInfo::new(info.name, info.machine, info.dtype, info.has_published),
        message,
        file,
        range,
        info.correction.map(str::to_owned),
    )
}

/// Rebuilds a library's symbol-index reference surface at its
/// `LibraryAnalyzed` point: the member captures (fresh from the drain)
/// resolve through the graph namespaces — whole-unit replacement, the shape
/// `query_symbols` serves — and the semantically resolved member-access
/// rows the diagnose fold just published (#2618) fold in beside the
/// parser-level sites.
fn library_surface(
    state: &EngineState,
    resolved: &ResolvedGraph,
    library: LibraryId,
    info: &LibraryInfo,
    member_refs: &[(FileId, std::sync::Arc<[dartq_semantic::MemberRef]>)],
) -> dartq_index::LibraryIndex {
    let mut builder = dartq_index::LibraryBuilder::new(
        resolved.graph_ids[&library],
        &resolved.graph,
        resolved.namespaces(),
    );
    for file in member_files(info) {
        // Members are resident and captured after an analyze — the same
        // live-member contract the reachability build's unit walk runs on.
        // A capture whose sites the site cache shed re-derives them here
        // (the `#1859` fallback posture); a pairing miss skips the file's
        // rows, records a bounded degrade row (#1459), and the whole-unit
        // rebuild at the next analyze restores them.
        let Some(capture) = state.rebuild_capture(file) else {
            state.residency_degrades.record(&[(file, "capture")]);
            continue;
        };
        builder.add_file(&capture);
    }
    // The semantic edges (#2618): interned spellings and u32 offsets
    // through — the map is the index's own kind byte; no text on this
    // path. Rows the capture pairing skipped stay dropped (the degrade
    // posture the captures keep).
    for (file, rows) in member_refs {
        builder.add_resolved_refs(rows.iter().map(|row| dartq_index::RefRow {
            file: *file,
            target_file: row.target_file,
            target_name: row.target_name,
            target_container: row.target_container,
            kind: match row.kind {
                dartq_semantic::MemberRefKind::Read => dartq_index::RefKind::Read,
                dartq_semantic::MemberRefKind::Invoked => dartq_index::RefKind::Invoked,
                dartq_semantic::MemberRefKind::Written => dartq_index::RefKind::Written,
                dartq_semantic::MemberRefKind::ReadWritten => dartq_index::RefKind::ReadWritten,
            },
            range: row.range,
            qualifier: StringId::NONE,
        }));
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
pub(crate) fn drain_captures(state: &mut EngineState) {
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
    let captured: Vec<_> = dirty
        .par_iter()
        .filter_map(|&file| {
            let facts = &state.residency[&file];
            // The resident content backs the facts (the residency contract)
            // and is lossy-decoded UTF-8 (the store contract) — a pairing
            // miss skips the recapture and records a bounded degrade row
            // (the #1459 posture): the file keeps its standing capture
            // until the next apply re-queues it.
            let Some(bytes) = state.store.get(facts.content) else {
                state.residency_degrades.record(&[(file, "store")]);
                return None;
            };
            let Ok(text) = std::str::from_utf8(bytes) else {
                state.residency_degrades.record(&[(file, "utf8")]);
                return None;
            };
            let key = l1::facts_key(&state.grants(), file, facts.content);
            if let Some(face) = state.cached_scanned(key) {
                Some((
                    file,
                    capture_surface(file, text, &face, &state.strings),
                    None,
                ))
            } else {
                let version = l1::language_version_for(&state.packages, &state.files, file);
                let (feature_set, settled, scanned, _) =
                    l1::scan_unit(text, version, file, &state.grants());
                let face = ScannedFace {
                    tokens: scanned.tokens,
                    features: feature_set,
                    version: settled,
                };
                let capture = capture_surface(file, text, &face, &state.strings);
                Some((file, capture, Some((key, face))))
            }
        })
        .collect();
    for (file, capture, miss) in captured {
        if let Some((key, face)) = miss {
            state.cache_scanned(key, face);
        }
        state.set_capture(file, Arc::new(capture));
    }
}

/// Captures one resident file's declaration and reference surface through a
/// scan face.
pub(crate) fn capture_surface(
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

/// The declared modifier facts of a class-family declaration (class, mixin,
/// enum), read through the semantic model's kind mapping — `None` for every
/// other declaration shape and for container names without a captured model.
fn class_modifier_facts(
    elements: &dartq_semantic::FileElements,
    name: dartq_source::StringId,
) -> Option<ClassModifiers> {
    elements
        .container(name)
        .map(dartq_semantic::container_modifiers)
}

/// One library's graph-build rows, collected off-thread by the parallel pass
/// of [`build_library_graph`]: owned values read straight out of the
/// resident facts — pre-interned name ids and cloned edge payloads, no
/// interner or builder touch — for the serial merge to fold in ascending
/// engine-library order. The merge is the sole ordering authority: the fold
/// reproduces the serial walk byte for byte (node registration order,
/// per-node declaration and edge sequences).
struct LibraryRows {
    /// Top-level declarations `(name, file, class-modifier facts)` in the
    /// serial walk's order — member files, then each unit's declaration
    /// face — with `TypeParam` entries already skipped.
    decls: Vec<(StringId, FileId, Option<ClassModifiers>)>,
    /// Resolved import edges in directive source order; the implicit core
    /// import joins at merge time, in front of these.
    imports: Vec<ImportEdge>,
    /// Resolved export edges in directive source order.
    exports: Vec<ExportEdge>,
}

#[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
fn build_library_graph(state: &mut EngineState) -> ResolvedGraph {
    state.graph_generation += 1;
    // The lazy dependency closure: pub-cache imports resolve to files
    // discovery never made resident; their interface facts load here (the
    // bounded BFS over the live libraries' targets) so the node pass below
    // registers them beside the synthetic SDK faces.
    crate::deps::load_dep_closure(state);
    let mut builder = LibraryGraphBuilder::new(&state.strings);
    let mut graph_ids: FxHashMap<LibraryId, dartq_resolution::LibraryId> = FxHashMap::default();

    // The serial interning hoist: the builder is a plain `&mut self`
    // structure, and node registration order — the live-library order — is
    // an observable contract the downstream orderings lean on (the export
    // fixpoint's first-come tiebreak, the condensation fingerprints).
    // O(libraries), one hash lookup per URI; everything per-file runs in the
    // lanes below. Each node's unit face registers here too — defining unit
    // first, then the parts in directive order, the oracle's
    // `RuleContext.allUnits` insertion order — straight off the formed
    // `LibraryInfo` (no second directive walk, no `member_files` clone).
    let live: Vec<LibraryId> = state.libraries.live().collect();
    for library in &live {
        let info = state.libraries.info(*library).expect("live library");
        let graph_id = builder.intern(&info.uri);
        builder.library_name(graph_id, &info.name);
        builder.unit(graph_id, info.defining);
        for &file in &info.parts {
            builder.unit(graph_id, file);
        }
        graph_ids.insert(*library, graph_id);
    }

    // The SDK nodes: one per `dart:` library. A library whose sources
    // loaded (`crate::sdk_units`) registers unit-backed — its entry and
    // part files as the node's units, every file's declarations with
    // their class-modifier facts — so the semantic layer resolves its
    // faces through the element model exactly as it resolves a dep file's
    // (`FormatException`'s constructor formals, `List`'s real members and
    // `implements Iterable<E>` edge). A library the SDK root does not
    // carry keeps the generated names-only face: a hit answers "defined"
    // without an element model, and a name outside the face still
    // reports. Stays serial: it mutates the file table and the builder.
    let mut sdk_faces: FxHashMap<String, dartq_resolution::LibraryId> = FxHashMap::default();
    let sdk_loaded: Vec<(String, Vec<FileId>)> = {
        let mut loaded: Vec<(String, Vec<FileId>)> = state
            .deps
            .sdk_libraries
            .iter()
            .map(|(name, files)| (name.clone(), files.clone()))
            .collect();
        loaded.sort_by(|a, b| a.0.cmp(&b.0));
        loaded
    };
    let sdk_files: FxHashSet<FileId> = sdk_loaded
        .iter()
        .flat_map(|(_, files)| files.iter().copied())
        .collect();
    let register_sdk_units =
        |builder: &mut LibraryGraphBuilder<'_>, node: dartq_resolution::LibraryId, files: &[FileId]| {
            for &file in files {
                let Some(unit) = state.deps.units.get(&file) else {
                    continue;
                };
                builder.unit(node, file);
                for (name, declaration) in unit.facts.elements.declarations() {
                    if declaration.kind == MicroKind::TypeParam {
                        continue;
                    }
                    match class_modifier_facts(&unit.facts.elements, name) {
                        Some(modifiers) => builder.declare_class(node, name, file, modifiers),
                        None => builder.declare(node, name, file),
                    }
                }
            }
        };
    for (uri, names) in SDK_LIBRARY_FACES {
        let node = builder.intern(&format!("dart:{uri}"));
        match sdk_loaded.iter().find(|(name, _)| name == uri) {
            Some((_, files)) => {
                register_sdk_units(&mut builder, node, files);
                // The face names the sources never declare (`Never` is a
                // built-in type with no `dart:core` declaration; the
                // generated face reads the analyzer's export namespace,
                // which carries it) stay declared on the synthetic face
                // file, so the spelling resolves exactly as before.
                let declared: FxHashSet<StringId> = files
                    .iter()
                    .filter_map(|file| state.deps.units.get(file))
                    .flat_map(|unit| unit.facts.elements.declarations().map(|(name, _)| name))
                    .collect();
                let face_only: Vec<StringId> = names
                    .iter()
                    .map(|name| state.strings.intern(name))
                    .filter(|name| !declared.contains(name))
                    .collect();
                if !face_only.is_empty() {
                    let file = state.files.intern(Path::new(&format!("<dart:{uri}>")));
                    builder.unit(node, file);
                    for name in face_only {
                        builder.declare(node, name, file);
                    }
                }
            }
            None => {
                let file = state.files.intern(Path::new(&format!("<dart:{uri}>")));
                // The face file registers as the node's unit so the graph's
                // `library_of_file` answers it: the namespace merge's SDK
                // preference reads the declaring library's URI (a `dart:`
                // name loses to a same-spelling package declaration,
                // `element/scope.dart:811-832`).
                builder.unit(node, file);
                for name in *names {
                    builder.declare(node, state.strings.intern(name), file);
                }
            }
        }
        sdk_faces.insert((*uri).to_string(), node);
    }
    // The loaded libraries outside the public face table (`dart:_internal`
    // and the other `_`-prefixed implementation libraries `dart:core`
    // imports) register the same way — their declarations are what the
    // public libraries' supertype and member walks resolve through.
    for (name, files) in &sdk_loaded {
        if sdk_faces.contains_key(name) {
            continue;
        }
        let node = builder.intern(&format!("dart:{name}"));
        register_sdk_units(&mut builder, node, files);
        sdk_faces.insert(name.clone(), node);
    }
    // dart:core's node doubles as the implicit core import's target; its
    // face carries the library's own surface plus the async/collection
    // re-exports (`Future`, `Stream`, …) — the oracle's import-namespace
    // shape at name granularity.
    let core = sdk_faces["core"];
    // The loaded libraries' own edges: the entry file's `import`/`export`
    // directives (every one a `dart:` URI) map onto the library nodes by
    // name — `dart:core`'s `export 'dart:async' show Future, Stream;` is
    // the re-export the prelude's namespace carries.
    for (name, files) in &sdk_loaded {
        let (Some(&entry), Some(&node)) = (files.first(), sdk_faces.get(name)) else {
            continue;
        };
        let Some(unit) = state.deps.units.get(&entry) else {
            continue;
        };
        for directive in &unit.facts.directives {
            let Some(target) = directive
                .uri
                .as_deref()
                .and_then(crate::sdk_units::sdk_library_of)
                .and_then(|library| sdk_faces.get(library).copied())
            else {
                continue;
            };
            match directive.kind {
                DirectiveKind::Import => builder.import(
                    node,
                    ImportEdge {
                        target: Some(target),
                        prefix: directive.prefix,
                        deferred: directive.deferred,
                        combinators: directive.combinators.clone(),
                    },
                ),
                DirectiveKind::Export => builder.export(
                    node,
                    ExportEdge {
                        target: Some(target),
                        combinators: directive.combinators.clone(),
                    },
                ),
                _ => {}
            }
        }
    }
    // Every other SDK library gets the same implicit `dart:core` prelude a
    // real source library's graph row carries — `<dart:collection>`'s
    // `HashMap implements Map<K,V>` resolves `Map` through the prelude.
    // Idempotent against a library whose own directives already import
    // `dart:core` (an edge keyed on the same target dedups in the
    // namespace merge).
    for (uri, &node) in &sdk_faces {
        if uri != "core" {
            builder.import(
                node,
                ImportEdge {
                    target: Some(core),
                    prefix: None,
                    deferred: false,
                    combinators: Combinators::unfiltered(),
                },
            );
        }
    }

    // The dependency-interface nodes: one per lazily indexed dep file, in
    // path order (deterministic registration — node order is an observable
    // contract downstream). Two passes, the live libraries' own shape:
    // identities and declarations first (so an edge between two dep files
    // resolves regardless of which registers first), then the edges.
    let mut dep_nodes: FxHashMap<FileId, dartq_resolution::LibraryId> = FxHashMap::default();
    let mut dep_files: Vec<FileId> = state
        .deps
        .units
        .keys()
        .copied()
        .filter(|file| !sdk_files.contains(file))
        .collect();
    dep_files.sort_by(|a, b| {
        let key = |file: FileId| {
            state
                .files
                .path(file)
                .map_or(&[][..], |path| path.as_os_str().as_encoded_bytes())
        };
        key(*a).cmp(key(*b))
    });
    for &file in &dep_files {
        let Some(unit) = state.deps.units.get(&file) else {
            continue;
        };
        let Some(path) = state.files.path(file) else {
            continue;
        };
        let uri = compose_canonical_uri(&state.uri_packages, path);
        let node = builder.intern(&uri);
        builder.library_name(
            node,
            l1::declared_library_name(&unit.facts).unwrap_or_default(),
        );
        builder.unit(node, file);
        for (name, declaration) in unit.facts.elements.declarations() {
            if declaration.kind == MicroKind::TypeParam {
                continue;
            }
            match class_modifier_facts(&unit.facts.elements, name) {
                Some(modifiers) => builder.declare_class(node, name, file, modifiers),
                None => builder.declare(node, name, file),
            }
        }
        dep_nodes.insert(file, node);
    }
    for &file in &dep_files {
        let Some(unit) = state.deps.units.get(&file) else {
            continue;
        };
        let Some(&node) = dep_nodes.get(&file) else {
            continue;
        };
        builder.import(
            node,
            ImportEdge {
                target: Some(core),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        for (directive, &target) in unit.facts.directives.iter().zip(unit.targets.iter()) {
            match directive.kind {
                DirectiveKind::Import => {
                    let sdk_target = (target == ResolvedTarget::External)
                        .then(|| {
                            directive
                                .uri
                                .as_ref()
                                .and_then(|uri| uri.strip_prefix("dart:"))
                                .and_then(|name| sdk_faces.get(name).copied())
                        })
                        .flatten();
                    builder.import(
                        node,
                        ImportEdge {
                            target: sdk_target
                                .or_else(|| graph_target(state, &graph_ids, &dep_nodes, target)),
                            prefix: directive.prefix,
                            deferred: directive.deferred,
                            combinators: directive.combinators.clone(),
                        },
                    );
                }
                DirectiveKind::Export => {
                    // The `directive_graph_target` arm: a `dart:` export
                    // (`export 'dart:typed_data' show Uint8List;` — the
                    // drift barrel's face) resolves to its synthetic SDK
                    // face node exactly like an import, or its names never
                    // join the export flow and every re-exported SDK type
                    // reads undefined at the importing library.
                    let sdk_target = (target == ResolvedTarget::External)
                        .then(|| {
                            directive
                                .uri
                                .as_ref()
                                .and_then(|uri| uri.strip_prefix("dart:"))
                                .and_then(|name| sdk_faces.get(name).copied())
                        })
                        .flatten();
                    builder.export(
                        node,
                        ExportEdge {
                            target: sdk_target
                                .or_else(|| graph_target(state, &graph_ids, &dep_nodes, target)),
                            combinators: directive.combinators.clone(),
                        },
                    );
                }
                _ => {}
            }
        }
    }

    // The parallel collection: one task per live library over the frozen
    // facts — declaration names arrive pre-interned, edge payloads are owned
    // clones, and target resolution reads the completed `graph_ids` /
    // `sdk_faces` maps — so the lanes mutate nothing. `sdk_faces` must be
    // complete first (an External import/export's face lookup reads it),
    // which the serial section above guarantees.
    let rows: Vec<LibraryRows> = live
        .par_iter()
        .map(|&library| {
            let info = state.libraries.info(library).expect("live library");
            let mut row = LibraryRows {
                decls: Vec::new(),
                imports: Vec::new(),
                exports: Vec::new(),
            };
            for file in member_files(info) {
                let facts = &state.residency[&file];
                for (name, declaration) in facts.elements.declarations() {
                    if declaration.kind == MicroKind::TypeParam {
                        continue;
                    }
                    row.decls
                        .push((name, file, class_modifier_facts(&facts.elements, name)));
                }
                for directive in &facts.directives {
                    match directive.kind {
                        // An explicit `dart:` import that classified External
                        // joins its synthetic face node; an unknown face
                        // stays edgeless (the resolver's posture).
                        DirectiveKind::Import => {
                            let sdk_target = (directive.target == ResolvedTarget::External)
                                .then(|| {
                                    directive
                                        .uri
                                        .as_ref()
                                        .and_then(|uri| uri.strip_prefix("dart:"))
                                        .and_then(|name| sdk_faces.get(name).copied())
                                })
                                .flatten();
                            row.imports.push(ImportEdge {
                                target: sdk_target.or_else(|| {
                                    graph_target(state, &graph_ids, &dep_nodes, directive.target)
                                }),
                                prefix: directive.prefix,
                                deferred: directive.deferred,
                                combinators: directive.combinators.clone(),
                            });
                        }
                        DirectiveKind::Export => {
                            // The import arm's `dart:` resolution on the
                            // export side: an SDK face target joins the
                            // export flow (`directive_graph_target`'s
                            // External arm); every other target resolves
                            // through the graph as before.
                            let sdk_target = (directive.target == ResolvedTarget::External)
                                .then(|| {
                                    directive
                                        .uri
                                        .as_ref()
                                        .and_then(|uri| uri.strip_prefix("dart:"))
                                        .and_then(|name| sdk_faces.get(name).copied())
                                })
                                .flatten();
                            row.exports.push(ExportEdge {
                                target: sdk_target.or_else(|| {
                                    graph_target(state, &graph_ids, &dep_nodes, directive.target)
                                }),
                                combinators: directive.combinators.clone(),
                            });
                        }
                        _ => {}
                    }
                }
            }
            row
        })
        .collect();

    // The serial merge — the sole builder writer and ordering authority:
    // rows fold in ascending library-id order, exactly the serial walk's
    // sequence (the builder's per-node declaration and edge vectors are
    // independent, so folding them per library changes no byte). Class-family
    // declarations register their declared modifier flags with the name (the
    // M2 cross-library restriction facts, `dartq_resolution::modifiers`);
    // every other shape registers as a plain declaration.
    for (library, row) in live.into_iter().zip(rows) {
        let graph_id = graph_ids[&library];
        for (name, file, modifiers) in row.decls {
            match modifiers {
                Some(modifiers) => builder.declare_class(graph_id, name, file, modifiers),
                None => builder.declare(graph_id, name, file),
            }
        }
        builder.import(
            graph_id,
            ImportEdge {
                target: Some(core),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        for edge in row.imports {
            builder.import(graph_id, edge);
        }
        for edge in row.exports {
            builder.export(graph_id, edge);
        }
    }
    let graph = builder.finish();
    ResolvedGraph {
        graph,
        graph_ids,
        sdk_faces,
        dep_nodes,
        namespaces: OnceLock::new(),
    }
}

/// The structure-fingerprint gate's refresh (#1040): recomputes every
/// changed file's digest from its already-captured facts — no re-parse, no
/// second walk over source; the fold reads the resident [`UnitFacts`] the
/// apply phase just wrote — sweeps removed files' entries, and reports
/// whether any structural face moved against the standing graph: a flipped
/// digest, a digest that never existed (a newly resident file), or any
/// removal (the live file set moved, and with it the membership the graph
/// builds from). Digests refresh even under a removal so the stored map
/// always mirrors the current facts: a stale entry could bless a later
/// skip against a graph that was built from intermediate facts.
///
/// Every caller passes the same apply's outcome, and the gate only runs
/// when `structure_dirty` or the graph is cold — a body edit with a stable
/// signature never enters here, which is sound because the digest face is
/// a subset of the api-signature face (see [`structure_digest`]): a
/// signature-stable edit cannot move any folded input.
fn refresh_structure_digests(
    state: &mut EngineState,
    changed: &[FileId],
    removed: &[FileId],
) -> bool {
    for file in removed {
        state.structure_digests.remove(file);
    }
    let mut moved = !removed.is_empty();
    let mut scratch = Vec::new();
    for file in changed {
        // A later batch inside the same refresh window may have removed
        // the file (create → tombstone in one turn: the base restore's set
        // and the standing tombstone's removal apply back to back). Its
        // digest left with the removal loop above, and a non-resident file
        // has no facts to read a fresh digest from — skip it; `removed`
        // already carries the move.
        let Some(facts) = state.residency.get(file) else {
            continue;
        };
        let digest = structure_digest(facts, &mut scratch);
        moved |= state.structure_digests.insert(*file, digest) != Some(digest);
    }
    moved
}

/// The version tag of the structure digest: bumped with any change to the
/// folded face, so folds of different shapes can never compare equal (the
/// map is memory-resident, so a bump just re-derives everything at the
/// next gate).
const STRUCTURE_DIGEST_TAG: &[u8; 12] = b"dq-struct-v1";

/// The structure fingerprint of one resident file (#1040): the first 8
/// bytes of a blake3 digest over every fact the O(workspace) structural
/// rebuild — [`build_library_graph`] plus the `Namespaces` it lazily
/// carries — reads from this file's facts. The enumerated face:
///
/// - every top-level declaration's interned name and kind, ascending by
///   name id (`declarations` is a hash map and the build is
///   order-insensitive over the declared set), skipping
///   `MicroKind::TypeParam` exactly as the build does, plus the declared
///   [`ClassModifiers`] the build registers for every name that resolves
///   to a container;
/// - every directive in source order (the namespace build merges imports
///   in source order, so order is load-bearing): kind, resolved target,
///   URI text (the `dart:` face lookup), dotted name, `as` prefix,
///   `deferred` flag, and the written combinator sequence (names and
///   clause kind — a conservative superset of the clause sets the graph
///   filters namespaces through).
///
/// Deliberately excluded: member shapes, types, annotations, enum
/// witnesses — element-face facts the rebuild never reads (they ride the
/// condensation fingerprints and the semantic fold, both kept fresh
/// elsewhere); directive and combinator spans (report anchors, position
/// only); `configurations` (already folded into the baked target at
/// resolution time); baked diagnostics and the line index.
///
/// # Conservative invariant
///
/// Digest equality must imply structural equivalence for everything the
/// rebuild consumes: a false "moved" costs one unnecessary rebuild (perf
/// only), a false "stood" would serve a stale graph — a correctness bug —
/// so anything not provably unconsumed is folded in. The skip path also
/// leans on `digest face ⊆ api-signature face`: names, kinds, modifier
/// flags and directive lexemes are all signature members, and the resolved
/// target is a pure function of the directive text within a session, so a
/// body edit that leaves the L2 signature standing cannot move any folded
/// input. Names and file targets fold as raw interned ids: the digest is
/// memory-resident and only ever compares two folds of the same session,
/// where one text is one id.
fn structure_digest(facts: &UnitFacts, scratch: &mut Vec<u8>) -> u64 {
    scratch.clear();
    scratch.extend_from_slice(STRUCTURE_DIGEST_TAG);

    // The declared set, canonicalized ascending by interned name.
    let mut declarations: Vec<(StringId, MicroKind)> = facts
        .elements
        .declarations()
        .filter(|(_, declaration)| declaration.kind != MicroKind::TypeParam)
        .map(|(name, declaration)| (name, declaration.kind))
        .collect();
    declarations.sort_unstable_by_key(|(name, _)| name.index());
    digest_u64(scratch, declarations.len() as u64);
    for (name, kind) in declarations {
        digest_u32(scratch, name.index());
        digest_u8(scratch, kind as u8);
        match class_modifier_facts(&facts.elements, name) {
            Some(modifiers) => {
                digest_u8(scratch, 1);
                digest_u8(scratch, modifiers.kind as u8);
                for flag in [
                    modifiers.is_abstract,
                    modifiers.is_base,
                    modifiers.is_final,
                    modifiers.is_interface,
                    modifiers.is_sealed,
                    modifiers.is_mixin_class,
                ] {
                    digest_u8(scratch, u8::from(flag));
                }
            }
            None => digest_u8(scratch, 0),
        }
    }

    // The directive face, source order preserved.
    digest_u64(scratch, facts.directives.len() as u64);
    for directive in &facts.directives {
        digest_u8(
            scratch,
            match directive.kind {
                DirectiveKind::Import => 0,
                DirectiveKind::Export => 1,
                DirectiveKind::Part => 2,
                DirectiveKind::PartOf => 3,
                DirectiveKind::Library => 4,
            },
        );
        match directive.target {
            ResolvedTarget::Resolved(file) => {
                digest_u8(scratch, 0);
                digest_u32(scratch, file.index());
            }
            ResolvedTarget::External => digest_u8(scratch, 1),
            ResolvedTarget::Missing => digest_u8(scratch, 2),
        }
        match &directive.uri {
            Some(uri) => {
                digest_u8(scratch, 1);
                digest_bytes(scratch, uri.as_bytes());
            }
            None => digest_u8(scratch, 0),
        }
        match &directive.name {
            Some(name) => {
                digest_u8(scratch, 1);
                digest_bytes(scratch, name.as_bytes());
            }
            None => digest_u8(scratch, 0),
        }
        match directive.prefix {
            Some(prefix) => {
                digest_u8(scratch, 1);
                digest_u32(scratch, prefix.index());
            }
            None => digest_u8(scratch, 0),
        }
        digest_u8(scratch, u8::from(directive.deferred));
        digest_u64(scratch, directive.combinators.written().count() as u64);
        for (name, hidden, _) in directive.combinators.written() {
            digest_u32(scratch, name.index());
            digest_u8(scratch, u8::from(hidden));
        }
    }

    let digest = blake3::hash(scratch);
    let mut lead = [0; 8];
    lead.copy_from_slice(&digest.as_bytes()[..8]);
    u64::from_le_bytes(lead)
}

fn digest_u8(buf: &mut Vec<u8>, value: u8) {
    buf.push(value);
}

fn digest_u32(buf: &mut Vec<u8>, value: u32) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn digest_u64(buf: &mut Vec<u8>, value: u64) {
    buf.extend_from_slice(&value.to_le_bytes());
}

/// A length-prefixed byte string (URI / dotted-name text).
fn digest_bytes(buf: &mut Vec<u8>, bytes: &[u8]) {
    digest_u32(buf, saturate_slot_u32(bytes.len()));
    buf.extend_from_slice(bytes);
}

/// The defining file plus the parts, part-directive order.
pub(crate) fn member_files(info: &LibraryInfo) -> Vec<FileId> {
    std::iter::once(info.defining)
        .chain(info.parts.iter().copied())
        .collect()
}

/// Maps a directive's resolved target onto a graph library: in-set file
/// targets map to their owning library, dependency-interface files (the
/// `deps` module) to their dep node; external/missing map to `None` (the
/// edge contributes no names, per the `ImportEdge` contract).
fn graph_target(
    state: &EngineState,
    graph_ids: &FxHashMap<LibraryId, dartq_resolution::LibraryId>,
    dep_nodes: &FxHashMap<FileId, dartq_resolution::LibraryId>,
    target: ResolvedTarget,
) -> Option<dartq_resolution::LibraryId> {
    match target {
        ResolvedTarget::Resolved(file) => state
            .library_of_file
            .get(&file)
            .and_then(|library| graph_ids.get(library).copied())
            .or_else(|| dep_nodes.get(&file).copied()),
        _ => None,
    }
}

/// The resolved-library gate of one import/export directive for the
/// combinator walks — the graph target without the `{0}` message spelling
/// (which only the undefined rows render). This is the oracle's
/// dead-code gate (`dead_code_verifier.dart:63`/:91, `library != null &&
/// !isOriginNotExistingFile`): a directive with no resolved library — or
/// one whose file target left the analyzed world — joins neither the
/// undefined nor the duplicate combinator walk. (The import/export
/// duplicate walk gates separately, see [`duplicate_target`].)
fn directive_graph_target(
    state: &EngineState,
    resolved: &ResolvedGraph,
    directive: &DirectiveFacts,
) -> Option<dartq_resolution::LibraryId> {
    match directive.target {
        ResolvedTarget::Resolved(_) => {
            graph_target(state, &resolved.graph_ids, &resolved.dep_nodes, directive.target)
        }
        ResolvedTarget::External => directive
            .uri
            .as_deref()?
            .strip_prefix("dart:")
            .and_then(|name| resolved.sdk_faces.get(name))
            .copied(),
        ResolvedTarget::Missing => None,
    }
}

/// The duplicate-walk identity of one import/export directive — the
/// `addImports` registration gate plus the `libraryUriStr` grouping key
/// (`imports_verifier.dart:59-97`, `:401`). Imports register only behind a
/// resolved library — a nonexistent-file target is skipped outright
/// (:68-70) — while an export registers behind any non-null library
/// (:75-83), a nonexistent file included: `isOriginNotExistingFile` does
/// not gate exports, so `export 'gone.dart'; export 'gone.dart';` groups
/// where the import pair does not. The key is the resolved library
/// identity, never the written spelling and never an SDK name-table entry:
/// analysis-set targets key by their owning library's node, `dart:`
/// externals by the URI face itself and a nonexistent export file by its
/// interned file (the oracle builds one library per missing file, keyed by
/// its absolute URI — the interned file is that identity) — so
/// `dart:async` needs no face-table hit to group.
#[derive(PartialEq, Eq)]
enum DuplicateTarget<'a> {
    /// An analysis-set library — the owning library's graph node.
    Library(dartq_resolution::LibraryId),
    /// An external `dart:` face — the URI text itself.
    Uri(&'a str),
    /// A nonexistent-file export target — the interned file.
    File(FileId),
}

/// The registration gate plus identity of one directive for the
/// import/export duplicate walk — `Some` registers, equality groups — see
/// the enum for the oracle anchors.
fn duplicate_target<'a>(
    state: &EngineState,
    resolved: &ResolvedGraph,
    directive: &'a DirectiveFacts,
) -> Option<DuplicateTarget<'a>> {
    match directive.target {
        ResolvedTarget::Resolved(file) => {
            match graph_target(state, &resolved.graph_ids, &resolved.dep_nodes, directive.target) {
                Some(node) => Some(DuplicateTarget::Library(node)),
                // Outside the analyzed world (absent on disk, or removed
                // since parse — the oracle re-resolves to a library flagged
                // `isOriginNotExistingFile` either way): imports skip it,
                // exports still register.
                None => match directive.kind {
                    DirectiveKind::Export => Some(DuplicateTarget::File(file)),
                    _ => None,
                },
            }
        }
        ResolvedTarget::External => directive.uri.as_deref().map(DuplicateTarget::Uri),
        // No library at all (unknown `dart:` name, unresolvable text) —
        // null `importedLibrary`/`exportedLibrary`, neither family
        // registers (`imports_verifier.dart:63-67`/:75-79).
        ResolvedTarget::Missing => None,
    }
}

/// The import/export duplicate walk over one compilation unit's directives
/// (`imports_verifier.dart:361-390` `_duplicates`): the oracle sorts the
/// unit's registered directives by resolved library URI and slides a pair
/// window over the sorted list — grouping by `same_target` here mirrors
/// that sort, each group keeping written order (the tie order the oracle's
/// sort preserves at directive-list sizes). Within a group, neighbors whose
/// shapes are identical — the `as` prefix and the written combinator
/// clauses, `areSyntacticallyIdenticalExceptUri`
/// (`ast.dart:29097`); `deferred` is not part of the identity — report
/// `duplicate_import`/`duplicate_export` at the later directive's URI span
/// (:377-384, the pair's larger offset; inside a written-order group that
/// is always the next directive), so adjacent and interleaved duplicates
/// alike report at every repeat but the group's first. Imports and exports
/// form separate groups (the oracle's separate `_duplicates` lists,
/// :87-96). `registered` is the `addImports` registration gate
/// ([`duplicate_target`]); `same_target` the `libraryUriStr` equality
/// (:372), evaluated against the group's first member — target equality is
/// transitive, so the URI sort groups no differently. Returns
/// `(is_import, uri_span)` per duplicate.
fn duplicate_directives(
    directives: &[DirectiveFacts],
    mut registered: impl FnMut(&DirectiveFacts) -> bool,
    mut same_target: impl FnMut(&DirectiveFacts, &DirectiveFacts) -> bool,
) -> Vec<(bool, SourceRange)> {
    /// One same-target family group: member indices in written order, the
    /// first member standing for the group's target.
    struct Group {
        is_import: bool,
        exemplar: usize,
        members: Vec<usize>,
    }

    let mut groups: Vec<Group> = Vec::new();
    for (index, directive) in directives.iter().enumerate() {
        let is_import = match directive.kind {
            DirectiveKind::Import => true,
            DirectiveKind::Export => false,
            DirectiveKind::Part | DirectiveKind::PartOf | DirectiveKind::Library => continue,
        };
        if !registered(directive) {
            continue;
        }
        let slot = groups.iter_mut().position(|group| {
            group.is_import == is_import && same_target(&directives[group.exemplar], directive)
        });
        match slot {
            Some(slot) => groups[slot].members.push(index),
            None => groups.push(Group {
                is_import,
                exemplar: index,
                members: vec![index],
            }),
        }
    }

    let mut duplicates = Vec::new();
    for group in &groups {
        let mut previous = &directives[group.members[0]];
        for &index in &group.members[1..] {
            let next = &directives[index];
            if previous.prefix == next.prefix
                && previous.combinators.written_shapes_equal(&next.combinators)
            {
                duplicates.push((group.is_import, next.uri_span));
            }
            // Unconditional advance (`imports_verifier.dart:386`): the next
            // pair is this directive vs the following one.
            previous = next;
        }
    }
    duplicates
}

/// The `{0}` message spelling of one directive's resolved library — the
/// oracle's ``${library.uri}`` (`dead_code_verifier.dart:156`) — `dart:<name>`
/// for the synthetic SDK faces, the defining file's absolute `file:///` URI
/// for analysis-set libraries (the oracle's corpus-run spelling; the same
/// decision dartq-resolution's `undefined_combinator_names` doc pins).
/// Materialized only when an undefined-combinator row is actually reported;
/// `None` for an edgeless directive — unreachable behind the
/// [`directive_graph_target`] gate.
fn directive_library_uri(
    state: &EngineState,
    resolved: &ResolvedGraph,
    node: dartq_resolution::LibraryId,
    directive: &DirectiveFacts,
) -> Option<String> {
    match directive.target {
        ResolvedTarget::Resolved(file) => {
            if let Some(library) = state.library_of_file.get(&file).copied()
                && let Some(info) = state.libraries.info(library)
                && let Some(path) = state.files.path(info.defining)
            {
                return Some(format!("file://{}", path.display()));
            }
            // A dependency-interface target: the library URI is the dep
            // file's own path — the oracle's defining-unit spelling for a
            // resolved library outside the analyzed set.
            if resolved.dep_nodes.contains_key(&file)
                && let Some(path) = state.files.path(file)
            {
                return Some(format!("file://{}", path.display()));
            }
            None
        }
        ResolvedTarget::External => Some(resolved.graph.uri(node).to_owned()),
        ResolvedTarget::Missing => None,
    }
}

/// The diagnostic codes whose presence anywhere in the library suppresses
/// the entire imports verification (`library_analyzer.dart:605-633`
/// `_hasDiagnosticReportedThatPreventsImportWarnings`, Dart 3.15 main,
/// list verbatim): the resolution-level breakage they flag replaces the
/// derived import warnings with the root causes.
const IMPORT_VERIFICATION_SUPPRESSING: &[&str] = &[
    "ambiguous_import",
    "const_with_non_type",
    "deprecated_export_use",
    "extends_non_class",
    "implements_non_class",
    "mixin_of_non_class",
    "new_with_non_type",
    "not_a_type",
    "prefix_identifier_not_followed_by_dot",
    "undefined_annotation",
    "undefined_class",
    "undefined_function",
    "undefined_identifier",
    "undefined_prefixed_name",
];

/// Whether any of the library's accumulated diagnostics carries a
/// suppressing code: the oracle collects every member file's reported codes
/// and skips the `ImportsVerifier` when one joins the set — its verify-errors
/// pass completes over all files before any warnings run, so a whole-
/// library scan is the same decision.
fn suppresses_import_verification(diagnostics: &[Arc<Diagnostic>]) -> bool {
    diagnostics
        .iter()
        .any(|diagnostic| IMPORT_VERIFICATION_SUPPRESSING.contains(&diagnostic.code.name))
}

/// The canonical diagnostic order as a borrowed
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
    runs: &[&'a [Arc<Diagnostic>]],
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
/// compute backends.
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
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
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
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    use dartq_diagnostics::{CodeInfo, DiagnosticType, Severity};
    use dartq_package::PackageTable;
    use dartq_pub::{LockFile, PackageConfig, Pubspec, build_graph};
    use dartq_resolution::CombinatorClause;
    use dartq_sdk::SdkSnapshot;
    use dartq_source::{FileTable, SourceRange, StringId};

    use super::*;
    use crate::semdiag::diagnoser;
    use crate::{Engine, EngineConfig, memory_store};

    /// A throwaway single-package workspace + synthetic core-only SDK —
    /// the semdiag fixture shape trimmed to what a default-diagnoser
    /// [`Engine::new`] binds — backing the clause-tree fold pins.
    fn clause_workspace(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "dartq-engine-clause-{}-{}-{}",
            tag,
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
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
        // Canonicalize before the pub faces resolve it (the #287 spelling
        // universe — macOS /var vs /private/var).
        root.canonicalize().expect("fixture root resolves")
    }

    /// A fresh default-diagnoser engine over [`clause_workspace`]'s faces.
    fn clause_engine(root: &Path) -> Engine {
        let config_dir = root.join(".dart_tool");
        let config = Arc::new(
            PackageConfig::parse(
                &std::fs::read_to_string(config_dir.join("package_config.json")).expect("config"),
            )
            .expect("valid config"),
        );
        let lock = LockFile::parse("packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n")
            .expect("valid lock");
        let root_pubspec = Pubspec::parse("name: t\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n")
            .expect("valid pubspec");
        let graph = build_graph(
            &config,
            &lock,
            &root_pubspec,
            &BTreeMap::new(),
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
            semdiag: Some(diagnoser()),
            target: crate::DEFAULT_TARGET.to_owned(),
            declared_variables: FxHashMap::default(),
            experiments: Vec::new(),
        })
        .expect("engine")
    }

    /// Runs the cross-unit clause fixture and answers whether the write's
    /// demotion read reports `undefined_getter`. `a.dart` declares the
    /// generic `Seq<T>`; `b.dart` declares `class IntSeq extends
    /// Seq<int>` and writes an `IntSeq` back into a variable promoted to
    /// `Seq<num>`: the demotion question `IntSeq <: Seq<num>` answers
    /// through the written `Seq<int>` clause edge (`int <: num` covariant)
    /// — the to-bounds `Seq<dynamic>` edge refuses and demotes the
    /// variable, whose follow-up read misses.
    fn demotion_reports_undefined_getter(write_ty: &str) -> bool {
        let root = clause_workspace(write_ty);
        let mut engine = clause_engine(&root);
        let a = engine.set_content(
            &root.join("lib/a.dart"),
            b"class Seq<T> {\n  int get length => 0;\n}\n",
        );
        let b = engine.set_content(
            &root.join("lib/b.dart"),
            format!(
                "import 'a.dart';\n\n class IntSeq extends Seq<int> {{}}\n\n \
                 void f(Object? o) {{\n  if (o is Seq<num>) {{\n    o.length;\n    \
                 o = {write_ty}();\n    o.length;\n  }}\n}}\n"
            )
            .as_bytes(),
        );
        engine.apply_pending(&mut |_| {});
        let rows = engine
            .analyze(&[a, b], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");
        rows.iter()
            .any(|row| row.code.name == "undefined_getter")
    }

    /// The clause-tree wiring's engine-level pin (#2229): the cross-unit
    /// written `Seq<int>` clause edge keeps the `Seq<num>` promotion
    /// through the `IntSeq` write, so the follow-up read resolves — the
    /// bare-head comparison (the to-bounds `Seq<dynamic>` edge) demotes
    /// and reports `undefined_getter` at the same read, which the control
    /// proves the fold face still fires for a genuinely-uncovered write.
    #[test]
    fn cross_unit_generic_clause_resolves_through_the_written_tree() {
        assert!(
            !demotion_reports_undefined_getter("IntSeq"),
            "the written Seq<int> edge keeps the promotion; the read must resolve"
        );
        assert!(
            demotion_reports_undefined_getter("Seq<String>"),
            "the control's Seq<String> write must demote: String is not a subtype of num"
        );
    }

    fn diagnostic(
        files: &mut FileTable,
        path: &str,
        offset: u32,
        message: &'static str,
    ) -> Diagnostic {
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
    /// shape `LibraryResult::diagnostics` has since #234. Rows take their
    /// refcount here, the store form since #1501.
    fn run(files: &mut FileTable, entries: &[(&str, u32, &'static str)]) -> Vec<Arc<Diagnostic>> {
        let mut run: Vec<Arc<Diagnostic>> = entries
            .iter()
            .map(|&(path, offset, message)| Arc::new(diagnostic(files, path, offset, message)))
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

    fn merged(files: &FileTable, runs: &[&[Arc<Diagnostic>]]) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        merge_canonical(files, runs, |diagnostic| out.push(diagnostic.clone()));
        out
    }

    /// The full merge over every live run at the state level — the splice's
    /// true oracle: built from the stored runs themselves, never from a
    /// re-serve of the standing cache (which would green-light a truncated
    /// splice against its own truncation).
    fn oracle_from_runs(state: &EngineState) -> Vec<Diagnostic> {
        let runs: Vec<&[Arc<Diagnostic>]> = state
            .libraries
            .live()
            .filter(|library| state.results.contains_key(library))
            .map(|library| state.results[&library].diagnostics.as_slice())
            .collect();
        merged_from_runs(&state.files, &runs)
    }

    /// One synthetic import/export fact carrying the duplicate walk's
    /// identity inputs (kind, URI text, prefix, combinator shape, deferred);
    /// the rest of the scan face stays at its quiet defaults. `offset` is
    /// the URI span the walk would report at.
    fn import_like(
        kind: DirectiveKind,
        offset: u32,
        uri: &str,
        prefix: Option<StringId>,
        deferred: bool,
        combinators: Combinators,
    ) -> DirectiveFacts {
        DirectiveFacts {
            kind,
            uri: Some(uri.into()),
            uri_span: SourceRange {
                offset,
                length: u32::try_from(uri.len()).expect("test uri fits u32"),
            },
            target: ResolvedTarget::External,
            code: None,
            prefix,
            deferred,
            combinators,
            name: None,
            configurations: Box::from([]),
            configuration_spans: Box::from([]),
        }
    }

    /// Directives grouped by resolved target report at each repeat but the
    /// group's first (`imports_verifier.dart:361-390` `_duplicates` sorts by
    /// `libraryUriStr` before the window, so written-order adjacency is not
    /// required): two identical imports report the 2nd; an interleaved
    /// `math, io, math` still reports the later `math` (the URI sort lines
    /// the math pair up); a three-run reports at both repeats; the import
    /// and export sequences group independently (separate `_duplicates`
    /// lists); and mixed shapes inside one group stay silent (the window
    /// compares neighbors in written order).
    #[test]
    fn duplicate_directives_report_per_target_group_after_uri_sort() {
        let strings = StringTable::new();
        // The resolved-library face of a synthetic directive (`libraryUriStr`).
        let face = |directive: &DirectiveFacts| match directive.uri.as_deref() {
            Some("math" | "math.dart") => Some("dart:math"),
            Some("io") => Some("dart:io"),
            _ => None,
        };
        let duplicates = |directives: Vec<DirectiveFacts>| {
            duplicate_directives(
                &directives,
                |directive| face(directive).is_some(),
                |a, b| face(a) == face(b),
            )
        };
        let plain = |offset, uri| {
            import_like(
                DirectiveKind::Import,
                offset,
                uri,
                None,
                false,
                Combinators::unfiltered(),
            )
        };
        let at = |offset, length| SourceRange { offset, length };

        // Two identical imports: the 2nd reports, at its own URI.
        assert_eq!(
            duplicates(vec![plain(8, "math"), plain(40, "math")]),
            [(true, at(40, 4))]
        );
        // Interleaved targets still group: the URI sort lines the math pair
        // up, so [math, io, math] reports at the later math — the later
        // offset of the pair (imports_verifier.dart:377-384).
        assert_eq!(
            duplicates(vec![plain(8, "math"), plain(40, "io"), plain(72, "math")]),
            [(true, at(72, 4))]
        );
        // [io, math, math]: the math pair reports the 3rd directive.
        assert_eq!(
            duplicates(vec![plain(8, "io"), plain(40, "math"), plain(72, "math")]),
            [(true, at(72, 4))]
        );
        // A three-run slides: the 2nd and the 3rd both report.
        assert_eq!(
            duplicates(vec![plain(8, "math"), plain(40, "math"), plain(72, "math")]),
            [(true, at(40, 4)), (true, at(72, 4))]
        );
        // The export list is independent: one import + one identical export
        // of the same target never group across families.
        assert!(
            duplicates(vec![
                plain(8, "math"),
                import_like(
                    DirectiveKind::Export,
                    40,
                    "math",
                    None,
                    false,
                    Combinators::unfiltered()
                ),
            ])
            .is_empty()
        );
        assert_eq!(
            duplicates(vec![
                import_like(
                    DirectiveKind::Export,
                    8,
                    "math",
                    None,
                    false,
                    Combinators::unfiltered()
                ),
                import_like(
                    DirectiveKind::Export,
                    40,
                    "math",
                    None,
                    false,
                    Combinators::unfiltered()
                ),
            ]),
            [(false, at(40, 4))]
        );
        // Mixed shapes inside one group: the written-order window compares
        // neighbors, and [show, plain, show] has no identical pair.
        let shown = |offset| {
            import_like(
                DirectiveKind::Import,
                offset,
                "math",
                None,
                false,
                Combinators::new(
                    [CombinatorClause::Show(Box::from([strings.intern("max")]))],
                    &strings,
                ),
            )
        };
        assert!(
            duplicates(vec![shown(8), plain(40, "math"), shown(72)]).is_empty(),
            "shape runs interleaved never report"
        );
    }

    /// The duplicate identity is shape plus resolved target
    /// (`areSyntacticallyIdenticalExceptUri`, `ast.dart:29097`): a prefix
    /// difference or a combinator-shape difference keeps two same-library
    /// imports silent, while a `deferred` difference does not (not part of
    /// the identity), and two spellings resolving to one library group —
    /// the oracle compares the resolved `libraryUriStr`, not the URI text.
    /// A nonexistent-file export registers by its file identity where an
    /// import does not (`imports_verifier.dart:68-70` vs :75-83).
    #[test]
    #[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
    fn duplicate_identity_is_shape_and_resolved_target() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let gone = files.intern(Path::new("/w/gone.dart"));
        // The resolved-library identity of a synthetic directive, as
        // `duplicate_target` classifies it: two spellings, one resolved
        // library; and the F5 asymmetry — only the export of a missing file
        // carries a group identity (its interned file standing for the
        // library URI the oracle builds for it).
        let identity = |directive: &DirectiveFacts| match directive.uri.as_deref() {
            Some("math" | "math.dart") => Some((0, 0)),
            Some("gone") if matches!(directive.kind, DirectiveKind::Export) => {
                Some((1, gone.index()))
            }
            _ => None,
        };
        let duplicates = |directives: Vec<DirectiveFacts>| {
            duplicate_directives(
                &directives,
                |directive| identity(directive).is_some(),
                |a, b| identity(a) == identity(b),
            )
        };

        let prefixed = import_like(
            DirectiveKind::Import,
            40,
            "math",
            Some(strings.intern("m")),
            false,
            Combinators::unfiltered(),
        );
        let plain = import_like(
            DirectiveKind::Import,
            8,
            "math",
            None,
            false,
            Combinators::unfiltered(),
        );
        assert!(duplicates(vec![plain.clone(), prefixed]).is_empty());

        // Combinator shape counts: `show max` vs plain is not a duplicate;
        // `show max` vs `show max` is.
        let shown = || {
            import_like(
                DirectiveKind::Import,
                40,
                "math",
                None,
                false,
                Combinators::new(
                    [CombinatorClause::Show(Box::from([strings.intern("max")]))],
                    &strings,
                ),
            )
        };
        assert!(duplicates(vec![plain.clone(), shown()]).is_empty());
        assert_eq!(
            duplicates(vec![shown(), shown()]).len(),
            1,
            "identical shapes report"
        );

        // `deferred` is not part of the identity
        // (`areSyntacticallyIdenticalExceptUri` reads prefix and
        // combinators only).
        let deferred = import_like(
            DirectiveKind::Import,
            40,
            "math",
            None,
            true,
            Combinators::unfiltered(),
        );
        assert_eq!(
            duplicates(vec![plain, deferred]).len(),
            1,
            "deferred vs non-deferred identical shape reports"
        );

        // The resolved-library key: different URI spellings of one library
        // group (`libraryUriStr` comparison, not URI text).
        let other_spelling = import_like(
            DirectiveKind::Import,
            40,
            "math.dart",
            None,
            false,
            Combinators::unfiltered(),
        );
        let plain_math = import_like(
            DirectiveKind::Import,
            8,
            "math",
            None,
            false,
            Combinators::unfiltered(),
        );
        assert_eq!(
            duplicates(vec![plain_math, other_spelling]).len(),
            1,
            "one library under two spellings groups"
        );

        // Unregistered targets join no group (`addImports` skips them — for
        // imports a nonexistent file too, imports_verifier.dart:68-70).
        let missing_import = || DirectiveFacts {
            kind: DirectiveKind::Import,
            uri: Some("gone".into()),
            uri_span: SourceRange {
                offset: 8,
                length: 4,
            },
            ..import_like(
                DirectiveKind::Import,
                8,
                "gone",
                None,
                false,
                Combinators::unfiltered(),
            )
        };
        assert!(duplicates(vec![missing_import(), missing_import()]).is_empty());

        // The same missing file exported twice groups by the file identity
        // (:75-83 — exports know no `isOriginNotExistingFile` skip).
        let gone_export = |offset| DirectiveFacts {
            kind: DirectiveKind::Export,
            ..import_like(
                DirectiveKind::Export,
                offset,
                "gone",
                None,
                false,
                Combinators::unfiltered(),
            )
        };
        assert_eq!(
            duplicates(vec![gone_export(8), gone_export(40)]),
            [(
                false,
                SourceRange {
                    offset: 40,
                    length: 4
                }
            )],
            "nonexistent-file exports register"
        );
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
            .flat_map(|library| library.iter().map(|row| row.as_ref().clone()))
            .collect();
        let expected = legacy_sorted(&files, concatenated);
        let actual = merged(&files, &stored);
        assert_eq!(expected, actual);
    }

    /// The oracle legitimately emits several rows under one
    /// (severity, code, file, line, col) key: the implementation-override
    /// pass reports one row per offending interface-member name, all
    /// anchored at the class-name token (upstream `inheritance_override.dart`
    /// — its own TODO admits "The classNameNode is always wrong"), and the
    /// required-parameters pass reports one row per missing parameter at the
    /// argument list (`required_parameters_verifier.dart` walks `parameters`
    /// and reports each miss at the same `errorEntity`). Key-level dedup
    /// anywhere in the merge would therefore diverge from the oracle on
    /// healthy corpora — the real reduced shapes (three override rows at one
    /// class name, two missing-argument rows at one call) must survive
    /// intact, ordered by the message tiebreaker, byte-identically across
    /// merges.
    #[test]
    fn same_key_rows_survive_the_merge_in_canonical_order() {
        fn row(
            files: &mut FileTable,
            code: (&'static str, &'static str),
            path: &str,
            offset: u32,
            message: &'static str,
        ) -> Arc<Diagnostic> {
            let file = files.intern(Path::new(path));
            Arc::new(Diagnostic::new(
                Severity::Error,
                CodeInfo::new(code.0, code.1, DiagnosticType::CompileTimeError, false),
                message,
                file,
                SourceRange {
                    offset,
                    length: 16,
                },
                None,
            ))
        }

        const OVERRIDE: (&str, &str) = (
            "invalid_implementation_override",
            "INVALID_IMPLEMENTATION_OVERRIDE",
        );
        const MISSING_ARG: (&str, &str) =
            ("missing_required_argument", "MISSING_REQUIRED_ARGUMENT");

        let mut files = FileTable::new();
        let library = {
            let mut rows = vec![
                row(
                    &mut files,
                    MISSING_ARG,
                    "/w/async_value.dart",
                    200,
                    "The named parameter '_error' is required, but there's no corresponding argument.",
                ),
                row(
                    &mut files,
                    MISSING_ARG,
                    "/w/async_value.dart",
                    200,
                    "The named parameter '_loading' is required, but there's no corresponding argument.",
                ),
                row(
                    &mut files,
                    OVERRIDE,
                    "/w/element.dart",
                    100,
                    "'FieldElementImpl.baseElement' isn't a valid concrete implementation of 'InternalFieldElement.baseElement'.",
                ),
                row(
                    &mut files,
                    OVERRIDE,
                    "/w/element.dart",
                    100,
                    "'PropertyInducingElementImpl.fragments' isn't a valid concrete implementation of 'InternalFieldElement.fragments'.",
                ),
                row(
                    &mut files,
                    OVERRIDE,
                    "/w/element.dart",
                    100,
                    "'ElementImpl.reference' isn't a valid concrete implementation of 'PropertyInducingElementImpl.reference'.",
                ),
            ];
            rows.sort_by(|a, b| compare_canonical(&files, a, b));
            rows
        };

        let out = merged(&files, &[library.as_slice()]);
        assert_eq!(out.len(), 5, "no key dedup: every same-key row survives");
        // Canonical order groups by (path, offset, code) with the message
        // tiebreaker inside each same-key group.
        assert_eq!(out[0].message, "The named parameter '_error' is required, but there's no corresponding argument.");
        assert_eq!(out[1].message, "The named parameter '_loading' is required, but there's no corresponding argument.");
        assert_eq!(out[2].message, "'ElementImpl.reference' isn't a valid concrete implementation of 'PropertyInducingElementImpl.reference'.");
        assert_eq!(out[3].message, "'FieldElementImpl.baseElement' isn't a valid concrete implementation of 'InternalFieldElement.baseElement'.");
        assert_eq!(out[4].message, "'PropertyInducingElementImpl.fragments' isn't a valid concrete implementation of 'InternalFieldElement.fragments'.");
        // Determinism: a second merge over the same runs is byte-identical.
        assert_eq!(merged(&files, &[library.as_slice()]), out);
    }

    /// No runs, or only empty runs: the merge emits nothing.
    #[test]
    fn merge_of_empty_runs_emits_nothing() {
        let files = FileTable::new();
        assert_eq!(merged(&files, &[]), Vec::new());
        let empty: [&[Arc<Diagnostic>]; 3] = [&[], &[], &[]];
        assert_eq!(merged(&files, &empty), Vec::new());
    }

    /// The splice must reproduce the full merge byte for byte: unchanged
    /// rows move in their previous relative order, a re-analyzed
    /// contributor's stale rows drop in favor of its fresh run, the fresh
    /// side's exact cross-run duplicate resolves by run ordinal (the full
    /// merge's rule), an empty fresh run in the mix stays silent, and the
    /// two tail shapes the weave's exit edge must drain — a fresh run
    /// canonically after every surviving old row, and the all-stale rebuild
    /// whose output is exactly the fresh side.
    #[test]
    fn splice_matches_the_full_merge() {
        let mut files = FileTable::new();
        // The unchanged contributors' runs: their rows move as-is.
        let unchanged_first = run(
            &mut files,
            &[("/w/a.dart", 10, "a one"), ("/w/z.dart", 40, "z one")],
        );
        let unchanged_second = run(&mut files, &[("/w/m.dart", 20, "m one")]);
        // The re-analyzed contributors' previous runs — the rows the splice
        // must drop (each sits on its changed library's own file).
        let stale_first = run(
            &mut files,
            &[("/w/b.dart", 5, "b old"), ("/w/b.dart", 50, "b old two")],
        );
        let stale_second = run(&mut files, &[("/w/y.dart", 15, "y old")]);
        // The fresh runs: paths weaving between the unchanged ones, an exact
        // duplicate across the two runs (tie resolves by run ordinal), and
        // an empty run in the mix.
        let fresh_first = run(
            &mut files,
            &[
                ("/w/b.dart", 5, "b one"),
                ("/w/a.dart", 30, "a two"),
                ("/w/y.dart", 15, "dup"),
            ],
        );
        let fresh_second = run(&mut files, &[("/w/y.dart", 15, "dup")]);
        let previous = merged(
            &files,
            &[
                unchanged_first.as_slice(),
                unchanged_second.as_slice(),
                stale_first.as_slice(),
                stale_second.as_slice(),
            ],
        );
        let fresh_runs: [&[Arc<Diagnostic>]; 3] =
            [fresh_first.as_slice(), fresh_second.as_slice(), &[]];
        let changed_paths = ["/w/b.dart", "/w/y.dart"];
        let spliced = splice_merged(&files, previous, &fresh_runs, |row| {
            let path = files.path(row.file).expect("test path interned");
            changed_paths.contains(&path.to_string_lossy().as_ref())
        });
        let expected = merged(
            &files,
            &[
                unchanged_first.as_slice(),
                unchanged_second.as_slice(),
                fresh_first.as_slice(),
                fresh_second.as_slice(),
            ],
        );
        assert_eq!(spliced, expected);

        // The trailing shape the weave's exit edge must drain: the fresh run
        // canonically follows every surviving old row, so the weave never
        // compares against it — old exhausts first and only the tail drain
        // carries the run (the last-sorting-library edit).
        let trailing_old = run(&mut files, &[("/w/a.dart", 1, "a head")]);
        let trailing_fresh = run(&mut files, &[("/w/z.dart", 70, "z tail")]);
        let previous = merged(&files, &[trailing_old.as_slice()]);
        let spliced = splice_merged(&files, previous, &[trailing_fresh.as_slice()], |_| false);
        let expected = merged(
            &files,
            &[trailing_old.as_slice(), trailing_fresh.as_slice()],
        );
        assert_eq!(spliced, expected, "the trailing fresh run survives");

        // The all-stale shape: every old row drops (the single-library
        // edit), the weave never runs, and the output is exactly the fresh
        // side — the tail drain carries all of it.
        let previous = merged(&files, &[stale_first.as_slice()]);
        let spliced = splice_merged(&files, previous, &[fresh_first.as_slice()], |_| true);
        let expected = merged(&files, &[fresh_first.as_slice()]);
        assert_eq!(spliced, expected, "the all-stale rebuild is the fresh side");
    }

    /// The structure-digest refresh must skip a changed file a LATER batch
    /// in the same refresh window removed (create → tombstone in one daemon
    /// turn: the base restore's set and the standing tombstone's removal
    /// apply back to back, and the accumulator carries the file on both
    /// sides). Reading `residency` for it indexed a map the removal had
    /// just emptied — the recovery-missing-uri-relink panic (pipeline
    /// refresh, #2100-adjacent): a reachable hot-path panic, fixed to the
    /// skip (the removal loop already purged the digest; a non-resident
    /// file has no facts to read).
    #[test]
    fn refresh_skips_changed_files_a_later_batch_removed() {
        let fixture = crate::l1::tests::Fixture::new();
        let mut state = fixture.state();
        let lib = fixture.root.join("lib");
        let a = crate::l1::tests::set(&mut state, &lib.join("a.dart"), "int a() => 1;\n");
        let b = crate::l1::tests::set(&mut state, &lib.join("b.dart"), "int b() => 2;\n");
        // Batch A lands both sets with no analyze between (the daemon's
        // wear-barrier shape — Engine::apply_pending appends each batch's
        // outcome to the structure accumulator; mirrored here at the state
        // level): both files enter the refresh accumulator's changed side.
        let mut sink = |_| {};
        let batch_a = crate::l1::apply(&mut state, &mut sink);
        state
            .structure_changed
            .extend(batch_a.changed.iter().copied());
        // Batch B removes b — the same refresh window, the removal side.
        state.pending_order.push(b);
        state
            .pending
            .insert(b, crate::state::PendingChange::Remove);
        let batch_b = crate::l1::apply(&mut state, &mut sink);
        state
            .structure_removed
            .extend(batch_b.removed.iter().copied());
        // The next analyze's refresh reads residency for every changed
        // file — b is changed (batch A) AND removed (batch B): the skip,
        // not a panic.
        let mut graph: Option<ResolvedGraph> = None;
        let cancel = CancellationToken::new();
        let merged = analyze(&mut state, &mut graph, None, &[], &mut |_| {}, &cancel)
            .expect("analyze after a create-then-remove window");
        assert!(
            merged.iter().all(|row| row.file != b),
            "the removed file's rows are gone: {merged:?}"
        );
        assert!(
            state.structure_digests.get(&a).is_some(),
            "the survivor's digest stands"
        );
        assert!(
            state.structure_digests.get(&b).is_none(),
            "the removed file's digest left with the removal loop"
        );
    }

    /// The relink-up arm of the directive relink (#2100): a resident whose
    /// import target was missing at intake must re-resolve when a later
    /// batch creates the target — the intake resolution ran before the
    /// target existed, and nothing else ever re-runs it. Without the
    /// relink the holder keeps its stale `uri_does_not_exist` row while a
    /// full recompute reports none (the recovery-missing-uri-relink
    /// divergence).
    #[test]
    fn created_target_relinks_missing_import_holders() {
        let fixture = crate::l1::tests::Fixture::new();
        let mut state = fixture.state();
        let lib = fixture.root.join("lib");
        let holder = crate::l1::tests::set(
            &mut state,
            &lib.join("holder.dart"),
            "import 'target.dart';\n",
        );
        let batch = crate::l1::apply(&mut state, &mut |_| {});
        assert!(batch.relinked.is_empty(), "no move to relink against");
        let facts = &state.residency[&holder];
        assert_eq!(facts.diagnostics.len(), 1);
        assert_eq!(facts.diagnostics[0].code.name, "uri_does_not_exist");
        assert_eq!(
            facts.directives[0].code,
            Some(dartq_resolution::ResolutionCode::UriDoesNotExist),
        );
        let holder_lib = state.library_of_file[&holder];

        // Batch 2 creates the target: the holder's directive target now
        // exists — the row drops, the code clears, the holder reports as
        // relinked, and its owner library re-enters the dirty set (its
        // output face moved even though no folded identity input did).
        crate::l1::tests::set(
            &mut state,
            &lib.join("target.dart"),
            "int target() => 0;\n",
        );
        let batch = crate::l1::apply(&mut state, &mut |_| {});
        assert_eq!(
            batch.relinked,
            vec![holder],
            "the holder is the only relinked resident"
        );
        let facts = &state.residency[&holder];
        assert_eq!(
            facts.directives[0].code, None,
            "the resolved target dropped its code"
        );
        assert!(
            facts.diagnostics.is_empty(),
            "the uri_does_not_exist tail row dropped: {:?}",
            facts.diagnostics
        );
        assert!(
            batch.invalidated.contains(&holder_lib),
            "the holder's owner re-enters the dirty set"
        );
    }

    /// The relink-down arm (#2100): removing a file whose target FileId a
    /// resident's directive points at must re-code the directive
    /// `uri_does_not_exist` and restore the tail row — symmetric to
    /// relink-up, and the second half of the same divergence class.
    #[test]
    fn removed_target_unresolves_import_holders() {
        let fixture = crate::l1::tests::Fixture::new();
        let mut state = fixture.state();
        let lib = fixture.root.join("lib");
        let holder = crate::l1::tests::set(
            &mut state,
            &lib.join("holder.dart"),
            "import 'target.dart';\n",
        );
        let target = crate::l1::tests::set(
            &mut state,
            &lib.join("target.dart"),
            "int target() => 0;\n",
        );
        let batch = crate::l1::apply(&mut state, &mut |_| {});
        assert!(batch.relinked.is_empty());
        assert!(state.residency[&holder].diagnostics.is_empty());
        let holder_lib = state.library_of_file[&holder];

        // Batch 2 removes the target: the holder's directive un-resolves —
        // the code returns, the tail row returns, and the holder reports
        // relinked.
        state.pending_order.push(target);
        state
            .pending
            .insert(target, crate::state::PendingChange::Remove);
        let batch = crate::l1::apply(&mut state, &mut |_| {});
        assert_eq!(batch.removed, vec![target]);
        assert_eq!(
            batch.relinked,
            vec![holder],
            "the holder is the only relinked resident"
        );
        let facts = &state.residency[&holder];
        assert_eq!(
            facts.directives[0].code,
            Some(dartq_resolution::ResolutionCode::UriDoesNotExist),
        );
        assert_eq!(facts.diagnostics.len(), 1);
        assert_eq!(facts.diagnostics[0].code.name, "uri_does_not_exist");
        assert!(
            batch.invalidated.contains(&holder_lib),
            "the holder's owner re-enters the dirty set"
        );
    }

    /// The no-op arm (#2100): a set-moving batch that touches no holder —
    /// the created file is nobody's directive target — must not relink
    /// anything, must leave the bystander's owner library's recorded run
    /// standing, and must not grow the invalidated set with it. The relink
    /// is an index-driven exact set, not a residency scan that rewrites
    /// every resident on every move.
    #[test]
    fn set_moving_batch_skips_unaffected_residents() {
        let fixture = crate::l1::tests::Fixture::new();
        let mut state = fixture.state();
        let lib = fixture.root.join("lib");
        let bystander = crate::l1::tests::set(
            &mut state,
            &lib.join("bystander.dart"),
            "import 'other.dart';\n",
        );
        crate::l1::tests::set(
            &mut state,
            &lib.join("other.dart"),
            "int other() => 0;\n",
        );
        let mut graph: Option<ResolvedGraph> = None;
        let cancel = CancellationToken::new();
        let merged = analyze(&mut state, &mut graph, None, &[], &mut |_| {}, &cancel)
            .expect("cold analyze");
        assert!(
            merged
                .iter()
                .all(|row| row.file != bystander || row.code.name != "uri_does_not_exist"),
            "the bystander's import resolved"
        );
        let bystander_lib = state.library_of_file[&bystander];
        let recorded = state
            .results
            .get(&bystander_lib)
            .map(|recorded| recorded.cycle_fingerprint);
        assert!(recorded.is_some(), "the bystander's run recorded");

        // Batch 2 creates an unrelated file: a set move, but nobody's
        // directives point at it. The relink gathers zero holders.
        crate::l1::tests::set(
            &mut state,
            &lib.join("unrelated.dart"),
            "int unrelated() => 0;\n",
        );
        let batch = crate::l1::apply(&mut state, &mut |_| {});
        assert!(
            batch.relinked.is_empty(),
            "no directive targets the created file: {:?}",
            batch.relinked
        );
        assert!(
            !batch.invalidated.contains(&bystander_lib),
            "the bystander's library is untouched"
        );
        assert_eq!(
            state
                .results
                .get(&bystander_lib)
                .map(|recorded| recorded.cycle_fingerprint),
            recorded,
            "the bystander's recorded run stands"
        );
    }

    /// The delete-create regression shape (#2100 follow-up): a resident
    /// importer whose missing target is removed and re-created across two
    /// batches keeps its reverse edge to the target, so a later signature
    /// edit on the target propagates the dirty set to the importer's
    /// library. The bug paired two facts: `was_resident` answered
    /// post-batch residency, so the relinked holder's own edge event saw
    /// the re-created target on both sides and never added the edge — and
    /// `close`'s membership-move seed gate could not tell relinked files
    /// (content-unchanged) from content-changed ones. Without the edge the
    /// step-4 signature splice starved the importer's recompute (in the
    /// sequence: `argument_type_not_assignable` at the `legacyCompute(2)`
    /// call site never reached the incremental face).
    #[test]
    fn recreated_target_keeps_importer_edge_for_signature_propagation() {
        let fixture = crate::l1::tests::Fixture::new();
        let mut state = fixture.state();
        let lib = fixture.root.join("lib");
        let main = crate::l1::tests::set(
            &mut state,
            &lib.join("main.dart"),
            "import 'legacy.dart';\nint useLegacy() => legacyCompute(2);\n",
        );
        let legacy = crate::l1::tests::set(
            &mut state,
            &lib.join("legacy.dart"),
            "int legacyCompute(int x) => x;\n",
        );
        crate::l1::apply(&mut state, &mut |_| {});
        let main_lib = state.library_of_file[&main];

        // Step 2: remove the target — the importer relinks down and the
        // edge to it drops with the target's residency.
        state.pending_order.push(legacy);
        state
            .pending
            .insert(legacy, crate::state::PendingChange::Remove);
        let batch = crate::l1::apply(&mut state, &mut |_| {});
        assert_eq!(batch.relinked, vec![main]);
        assert!(
            !state
                .importers
                .row(legacy)
                .iter()
                .any(|edge| edge.library == main_lib),
            "the edge dropped with the removed target"
        );

        // Step 3: re-create the target — the importer relinks up AND its
        // reverse edge must come back through its own edge event (the
        // sources loop skips it as an own-event-ran file; `was_resident`
        // must answer pre-apply residency so the add is not folded away).
        crate::l1::tests::set(
            &mut state,
            &lib.join("legacy.dart"),
            "int legacyCompute(int x) => x;\n",
        );
        let batch = crate::l1::apply(&mut state, &mut |_| {});
        assert_eq!(batch.relinked, vec![main]);
        assert!(
            state
                .importers
                .row(legacy)
                .iter()
                .any(|edge| edge.library == main_lib),
            "the reverse edge to the re-created target was restored"
        );

        // Step 4: a signature splice on the target — the seed propagates
        // through the restored edge to the importer's library. The target
        // library's id is re-read after re-creation: retirement freed the
        // first incarnation's arena slot and re-interning may recycle it.
        let legacy_lib = state.library_of_file[&legacy];
        crate::l1::tests::set(
            &mut state,
            &lib.join("legacy.dart"),
            "int legacyCompute(String x) => x.length;\n",
        );
        let batch = crate::l1::apply(&mut state, &mut |_| {});
        assert!(
            batch.invalidated.contains(&legacy_lib),
            "the edited library dirties"
        );
        assert!(
            batch.invalidated.contains(&main_lib),
            "the importer's library dirties through the restored edge"
        );
    }

    /// The cache-reuse proof at the state level (#1039, audit g1034 F1): a
    /// zero-dirty cycle serves the standing merged output — the same Arc
    /// allocation, no rebuild — a signature edit advances the contributors'
    /// generations and replaces it, and a library removal drops its rows
    /// from the rebuilt output. The held Arcs drop before each rebuild, so
    /// both rebuilds take the splice arm (the standing allocation is left
    /// unshared), and each spliced output pins byte-equal against a full
    /// merge built from the live runs — a true oracle, not a re-serve of
    /// the standing cache.
    #[test]
    fn zero_dirty_analyze_serves_the_standing_merged_output() {
        let fixture = crate::l1::tests::Fixture::new();
        let mut state = fixture.state();
        let lib = fixture.root.join("lib");
        let a = crate::l1::tests::set(&mut state, &lib.join("a.dart"), "int a() => 1;\n");
        let b = crate::l1::tests::set(
            &mut state,
            &lib.join("b.dart"),
            "import 'a.dart';\nint b() => a;\n",
        );
        let mut graph: Option<ResolvedGraph> = None;
        let cancel = CancellationToken::new();
        let mut analyze_now =
            |state: &mut EngineState| analyze(state, &mut graph, None, &[], &mut |_| {}, &cancel);

        let cold = analyze_now(&mut state).expect("cold analyze");
        let cold_rows: Vec<Diagnostic> = cold.as_ref().clone();
        let (stamp, cold_contributors) = {
            let cache = state
                .merged
                .as_ref()
                .expect("cold analyze stores the cache");
            assert_eq!(cache.contributors.len(), 2, "both libraries contribute");
            assert_eq!(*cache.output, cold_rows);
            (Arc::as_ptr(&cache.output), cache.contributors.clone())
        };

        // Zero-dirty cycle: no apply movement, no candidates — the same
        // allocation serves the rows.
        let warm = analyze_now(&mut state).expect("warm analyze");
        assert_eq!(*warm, cold_rows, "byte-identical");
        let cache = state.merged.as_ref().expect("cache stands");
        assert_eq!(
            Arc::as_ptr(&cache.output),
            stamp,
            "the standing Arc is reused"
        );
        assert_eq!(cache.contributors, cold_contributors);
        // The held Arcs drop: the standing allocation is left unshared, so
        // the next rebuild moves its rows through the splice instead of
        // falling back to the full merge.
        drop(warm);
        drop(cold);

        // A signature edit (a new import directive) moves both libraries'
        // faces: the cascade re-analyzes them and the cache rebuilds under
        // advanced generations. The edit also reports a row (the synthetic
        // SDK registers only core, so `dart:nope` misses), so the edited
        // face visibly differs and the splice pins against real movement.
        // Every old row is stale (both contributors re-analyzed), so the
        // spliced output is the tail drain over the fresh runs — the shape
        // the weave's exit edge carries.
        crate::l1::tests::set(
            &mut state,
            &lib.join("a.dart"),
            "import 'dart:nope';\nint a() => 1;\n",
        );
        let edited = analyze_now(&mut state).expect("edited analyze");
        assert_ne!(*edited, cold_rows, "the re-analyzed faces report");
        let cache = state.merged.as_ref().expect("rebuild stores the cache");
        // The rebuild unwraps the standing allocation and moves its rows, so
        // comparing addresses against `stamp` proves nothing — the allocator
        // may hand the freed block straight back to the new `Arc`. The stable
        // form of "the rebuild replaced the standing output" is the
        // store's one-allocation contract: the cache and the returned handle
        // refcount the same buffer (#1387).
        assert!(
            Arc::ptr_eq(&cache.output, &edited),
            "the rebuilt output is the standing one, not a store-time copy"
        );
        assert_ne!(
            cache.contributors, cold_contributors,
            "generations advanced"
        );
        assert_eq!(
            *edited,
            oracle_from_runs(&state),
            "the spliced rebuild is the full merge"
        );
        let rebuilt_stamp = Arc::as_ptr(&cache.output);
        let rebuilt_contributors = cache.contributors.clone();

        // Zero-dirty again: the rebuilt Arc stands.
        let warm_again = analyze_now(&mut state).expect("warm after edit");
        assert_eq!(*warm_again, *edited);
        assert_eq!(
            Arc::as_ptr(&state.merged.as_ref().expect("cache stands").output),
            rebuilt_stamp
        );
        drop(warm_again);
        drop(edited);

        // Removing b's defining file retires its library: its rows leave
        // the output and the cache rebuilds one contributor short — again
        // through the splice, the survivor's rows moved, the retired
        // library's rows dropped for a fresh run that does not exist.
        state.pending_order.push(b);
        state.pending.insert(b, crate::state::PendingChange::Remove);
        let removed = analyze_now(&mut state).expect("removal analyze");
        let cache = state.merged.as_ref().expect("cache stands after removal");
        assert_eq!(cache.contributors.len(), 1, "a's library remains");
        assert_ne!(cache.contributors, rebuilt_contributors);
        assert_eq!(
            *removed,
            oracle_from_runs(&state),
            "byte-equal to the full rebuild without the removed library"
        );
        assert!(
            removed.iter().all(|row| row.file == a),
            "only the surviving contributor's rows remain"
        );
    }

    /// The removal face of the splice parity: a file whose diagnostics stand
    /// in the merged cache leaves residency — its rows must leave the output
    /// with it, byte-equal to the full rebuild without the file. The suite's
    /// other removal tests retire clean files, so the staleness closure's
    /// ownerless arm never held rows to retain.
    #[test]
    fn removing_a_file_with_diagnostics_matches_the_full_rebuild_without_it() {
        let fixture = crate::l1::tests::Fixture::new();
        let mut state = fixture.state();
        let lib = fixture.root.join("lib");
        // Each file carries its own unresolved import, so both hold rows
        // before the removal and the survivor's rows are independent of the
        // file going away.
        let a = crate::l1::tests::set(
            &mut state,
            &lib.join("a.dart"),
            "import 'gone.dart';\nint a() => 1;\n",
        );
        let b = crate::l1::tests::set(
            &mut state,
            &lib.join("b.dart"),
            "import 'missing.dart';\nimport 'a.dart';\nint b() => a;\n",
        );
        let mut graph: Option<ResolvedGraph> = None;
        let cancel = CancellationToken::new();
        let mut analyze_now =
            |state: &mut EngineState| analyze(state, &mut graph, None, &[], &mut |_| {}, &cancel);

        let cold = analyze_now(&mut state).expect("cold analyze");
        assert!(
            cold.iter().any(|row| row.file == b),
            "the file to remove holds diagnostics in the standing output"
        );
        // Once b leaves, the live runs are a's alone: the full rebuild
        // without b is exactly a's rows in canonical order.
        let a_rows: Vec<Diagnostic> = cold.iter().filter(|row| row.file == a).cloned().collect();
        assert!(!a_rows.is_empty(), "the surviving library reports");
        // The held Arc drops first: the standing allocation is left
        // unshared, so the removal rebuild moves the survivor's rows
        // through the splice — the staleness-drop path this pin exists for.
        drop(cold);

        state.pending_order.push(b);
        state.pending.insert(b, crate::state::PendingChange::Remove);
        let removed = analyze_now(&mut state).expect("removal analyze");
        assert!(
            removed.iter().all(|row| row.file != b),
            "the removed file's rows drop from the output"
        );
        assert_eq!(
            removed.as_ref(),
            &a_rows,
            "byte-equal to the full rebuild without the removed file"
        );
    }

    /// The splice's tail shapes end to end: with the standing allocation
    /// unshared the rebuild takes the splice arm, and the output pins
    /// byte-equal against the full merge over the live runs — the
    /// single-library edit (every old row stale, the shape whose spliced
    /// buffer was entirely empty before the tail drain) and the
    /// last-sorting library's edit (its fresh run trails every surviving
    /// row, the run the weave's exit edge used to drop).
    #[test]
    fn splice_rebuild_matches_the_full_merge_on_the_tail_shapes() {
        let cancel = CancellationToken::new();
        let analyze_now = |state: &mut EngineState, graph: &mut Option<ResolvedGraph>| {
            analyze(state, graph, None, &[], &mut |_| {}, &cancel)
        };

        // Shape (a): the only library edits. Every old row drops, the weave
        // never runs, and the output is exactly the fresh run — the tail
        // drain's whole cargo.
        let fixture = crate::l1::tests::Fixture::new();
        let mut state = fixture.state();
        let lib = fixture.root.join("lib");
        crate::l1::tests::set(
            &mut state,
            &lib.join("only.dart"),
            "import 'dart:math' show zork;\nint a() => 1;\n",
        );
        let mut graph: Option<ResolvedGraph> = None;
        let cold = analyze_now(&mut state, &mut graph).expect("cold analyze");
        assert_eq!(
            cold.len(),
            1,
            "the fixture's only row is its dart:math URI miss — the synthetic \
             SDK table registers no math library, and the function is valid \
             Dart: {cold:?}"
        );
        drop(cold);
        crate::l1::tests::set(
            &mut state,
            &lib.join("only.dart"),
            "import 'dart:math' show werm;\nint b() => 2;\n",
        );
        let edited = analyze_now(&mut state, &mut graph).expect("edited analyze");
        assert_eq!(
            edited.as_ref(),
            &oracle_from_runs(&state),
            "the spliced single-library rebuild is the full fresh run"
        );
        assert!(!edited.is_empty(), "the fresh run survives the splice");

        // Shape (b): the last-sorting library edits. The survivor's rows
        // stand, the fresh run canonically trails them, and only the tail
        // drain carries it into the output.
        let fixture = crate::l1::tests::Fixture::new();
        let mut state = fixture.state();
        let lib = fixture.root.join("lib");
        crate::l1::tests::set(
            &mut state,
            &lib.join("a.dart"),
            "import 'dart:math' show zork;\nint a() => 1;\n",
        );
        let b = crate::l1::tests::set(
            &mut state,
            &lib.join("b.dart"),
            "import 'dart:math' show werm;\nint b() => 2;\n",
        );
        let mut graph: Option<ResolvedGraph> = None;
        let pair = analyze_now(&mut state, &mut graph).expect("pair analyze");
        assert_eq!(
            pair.len(),
            2,
            "one row per library — the flat existence face resolves dart:math, \
             so each fixture's bogus shown name (`zork`/`werm`, absent from the \
             math face) is the only row valid Dart produces here: {pair:?}"
        );
        drop(pair);
        crate::l1::tests::set(
            &mut state,
            &lib.join("b.dart"),
            "import 'dart:math' show werm;\nint b() => 2;\nint more() => 4;\n",
        );
        let trailing = analyze_now(&mut state, &mut graph).expect("trailing analyze");
        assert_eq!(
            trailing.as_ref(),
            &oracle_from_runs(&state),
            "the trailing fresh run survives the splice"
        );
        assert!(
            trailing
                .iter()
                .filter(|row| row.file == b && row.code.name == "undefined_shown_name")
                .count()
                == 1,
            "the re-analyzed contributor's fresh shown-name miss pins from the \
             runs, not a served cache — the edited fixture is valid Dart, so its \
             one row is the combinator miss: {trailing:?}"
        );
    }

    /// The analyze closure the gate tests share: one standing `graph`
    /// variable carried across cycles, exactly the Engine's shape.
    struct GateFixture {
        state: EngineState,
        graph: Option<ResolvedGraph>,
        cancel: CancellationToken,
        lib: std::path::PathBuf,
    }

    impl GateFixture {
        fn new(initial: &str) -> (Self, FileId) {
            let fixture = crate::l1::tests::Fixture::new();
            let lib = fixture.root.join("lib");
            let mut state = fixture.state();
            let a = crate::l1::tests::set(&mut state, &lib.join("a.dart"), initial);
            let this = Self {
                state,
                graph: None,
                cancel: CancellationToken::new(),
                lib,
            };
            (this, a)
        }

        fn analyze(&mut self) -> Vec<Diagnostic> {
            analyze(
                &mut self.state,
                &mut self.graph,
                None,
                &[],
                &mut |_| {},
                &self.cancel,
            )
            .expect("analyze")
            .as_ref()
            .clone()
        }

        /// Rewrites `a.dart` and runs one analyze.
        fn edit(&mut self, text: &str) -> Vec<Diagnostic> {
            crate::l1::tests::set(&mut self.state, &self.lib.join("a.dart"), text);
            self.analyze()
        }
    }

    /// Body-only edits never reach the gate (a stable signature leaves
    /// `structure_dirty` clear), and a non-structural signature edit — a
    /// return-type swap, the ticket's headline case — reaches it and
    /// stands the graph down: the generation probe holds and the refreshed
    /// digest equals the stored one.
    #[test]
    fn non_structural_edits_keep_the_standing_graph() {
        let (mut fix, a) = GateFixture::new("int f() => 1;\n");
        fix.analyze();
        let cold = fix.state.graph_generation;
        assert!(cold >= 1, "cold analyze builds the graph");
        let cold_digest = fix.state.structure_digests[&a];

        fix.edit("int f() => 2;\n");
        assert_eq!(
            fix.state.graph_generation, cold,
            "body edit never reaches the gate"
        );
        assert_eq!(fix.state.structure_digests[&a], cold_digest);

        // The return type is an api-signature member (the library
        // recomputes) but no rebuild input: the graph stands.
        fix.edit("String f() => '';\n");
        assert!(!fix.state.structure_dirty, "the cycle consumed the flag");
        assert_eq!(
            fix.state.graph_generation, cold,
            "non-structural signature edit keeps the graph"
        );
        assert_eq!(fix.state.structure_digests[&a], cold_digest);
    }

    /// Structural edits rebuild: a new directive and a new top-level
    /// declaration each flip the digest, and the generation probe moves
    /// once per flip.
    #[test]
    fn structural_edits_rebuild_the_graph() {
        let (mut fix, _) = GateFixture::new("int f() => 1;\n");
        fix.analyze();
        let cold = fix.state.graph_generation;

        fix.edit("import 'dart:math';\nint f() => 1;\n");
        assert_eq!(
            fix.state.graph_generation,
            cold + 1,
            "a directive edit rebuilds the graph"
        );

        fix.edit("import 'dart:math';\nint f() => 1;\nint g() => 2;\n");
        assert_eq!(
            fix.state.graph_generation,
            cold + 2,
            "a top-level declaration edit rebuilds the graph"
        );
    }

    /// A structural edit of one file rebuilds even when a sibling's
    /// signature moved in the same cycle for non-structural reasons — the
    /// refresh must not let one file's stood digest mask another's moved
    /// one.
    #[test]
    fn one_moved_digest_rebuilds_despite_stood_siblings() {
        let fixture = crate::l1::tests::Fixture::new();
        let mut state = fixture.state();
        let lib = fixture.root.join("lib");
        crate::l1::tests::set(&mut state, &lib.join("a.dart"), "int f() => 1;\n");
        crate::l1::tests::set(&mut state, &lib.join("b.dart"), "int b() => 2;\n");
        let mut graph: Option<ResolvedGraph> = None;
        let cancel = CancellationToken::new();
        let mut analyze_now =
            |state: &mut EngineState| analyze(state, &mut graph, None, &[], &mut |_| {}, &cancel);
        analyze_now(&mut state).expect("cold analyze");
        let cold = state.graph_generation;

        // a.dart grows a declaration (structural); b.dart swaps its return
        // type in the same batch (signature-only). One rebuild serves both.
        crate::l1::tests::set(
            &mut state,
            &lib.join("a.dart"),
            "int f() => 1;\nint g() => 3;\n",
        );
        crate::l1::tests::set(&mut state, &lib.join("b.dart"), "String b() => '';\n");
        analyze_now(&mut state).expect("mixed analyze");
        assert_eq!(
            state.graph_generation,
            cold + 1,
            "the moved digest rebuilds despite the stood sibling"
        );
    }

    /// Digest storage is swept with residency: a removed file's entry
    /// leaves the map, which stays exactly residency-sized.
    #[test]
    fn digest_storage_sweeps_with_residency() {
        let fixture = crate::l1::tests::Fixture::new();
        let mut state = fixture.state();
        let lib = fixture.root.join("lib");
        crate::l1::tests::set(&mut state, &lib.join("a.dart"), "int f() => 1;\n");
        let b = crate::l1::tests::set(&mut state, &lib.join("b.dart"), "int b() => 2;\n");
        let mut graph: Option<ResolvedGraph> = None;
        let cancel = CancellationToken::new();
        let mut analyze_now =
            |state: &mut EngineState| analyze(state, &mut graph, None, &[], &mut |_| {}, &cancel);
        analyze_now(&mut state).expect("cold analyze");
        assert_eq!(state.structure_digests.len(), state.residency.len());
        assert!(state.structure_digests.contains_key(&b));

        state.pending_order.push(b);
        state.pending.insert(b, crate::state::PendingChange::Remove);
        analyze_now(&mut state).expect("removal analyze");
        assert!(
            !state.structure_digests.contains_key(&b),
            "the removed file's digest is swept"
        );
        assert_eq!(
            state.structure_digests.len(),
            state.residency.len(),
            "the map stays bounded by residency"
        );
    }

    /// The retired serial walk of [`build_library_graph`], verbatim — the
    /// parity oracle the parallel pass must reproduce byte for byte: node
    /// registration order, per-node declared library names, per-node
    /// declaration and edge sequences, and the target numbering they carry.
    fn serial_library_graph(state: &mut EngineState) -> LibraryGraph {
        let mut builder = LibraryGraphBuilder::new(&state.strings);
        let mut graph_ids: FxHashMap<LibraryId, dartq_resolution::LibraryId> = FxHashMap::default();
        // The parity fixtures carry no package dependencies, so the dep-node
        // face this oracle predates stays empty.
        let dep_nodes: FxHashMap<FileId, dartq_resolution::LibraryId> = FxHashMap::default();
        for library in state.libraries.live() {
            let info = state.libraries.info(library).expect("live library");
            let graph_id = builder.intern(&info.uri);
            builder.library_name(graph_id, &info.name);
            builder.unit(graph_id, info.defining);
            for &file in &info.parts {
                builder.unit(graph_id, file);
            }
            graph_ids.insert(library, graph_id);
        }
        for library in state.libraries.live() {
            let graph_id = graph_ids[&library];
            for file in member_files(state.libraries.info(library).expect("live library")) {
                for (name, declaration) in state.residency[&file].elements.declarations() {
                    if declaration.kind == MicroKind::TypeParam {
                        continue;
                    }
                    match class_modifier_facts(&state.residency[&file].elements, name) {
                        Some(modifiers) => builder.declare_class(graph_id, name, file, modifiers),
                        None => builder.declare(graph_id, name, file),
                    }
                }
            }
        }
        let sdk_faces: FxHashMap<&str, dartq_resolution::LibraryId> = SDK_LIBRARY_FACES
            .iter()
            .map(|(uri, names)| {
                let node = builder.intern(&format!("dart:{uri}"));
                let file = state.files.intern(Path::new(&format!("<dart:{uri}>")));
                builder.unit(node, file);
                for name in *names {
                    builder.declare(node, state.strings.intern(name), file);
                }
                (*uri, node)
            })
            .collect();
        let core = sdk_faces["core"];
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
                                    target: sdk_target.or_else(|| {
                                        graph_target(state, &graph_ids, &dep_nodes, directive.target)
                                    }),
                                    prefix: directive.prefix,
                                    deferred: directive.deferred,
                                    combinators: directive.combinators.clone(),
                                },
                            );
                        }
                        DirectiveKind::Export => {
                            let sdk_target = (directive.target == ResolvedTarget::External)
                                .then(|| {
                                    directive
                                        .uri
                                        .as_ref()
                                        .and_then(|uri| uri.strip_prefix("dart:"))
                                        .and_then(|name| sdk_faces.get(name).copied())
                                })
                                .flatten();
                            builder.export(
                                graph_id,
                                ExportEdge {
                                    target: sdk_target.or_else(|| {
                                        graph_target(state, &graph_ids, &dep_nodes, directive.target)
                                    }),
                                    combinators: directive.combinators.clone(),
                                },
                            );
                        }
                        _ => {}
                    }
                }
            }
        }
        builder.finish()
    }

    /// Byte-parity assert: the parallel-built graph and the serial oracle
    /// carry identical node counts and, for every engine library (ascending),
    /// identical uri, declared library name, import/export edge sequences
    /// (target ids included — registration order is pinned), declaration
    /// lookups, class-modifier faces, and `declaring_libraries` registration
    /// order.
    fn assert_graphs_match(state: &EngineState, built: &ResolvedGraph, oracle: &LibraryGraph) {
        assert_eq!(built.graph.len(), oracle.len(), "node count");
        let mut names: Vec<StringId> = state
            .residency
            .values()
            .flat_map(|facts| facts.elements.declarations().map(|(name, _)| name))
            .collect();
        names.sort_by_key(|name| name.index());
        names.dedup();

        let mut libraries: Vec<LibraryId> = built.graph_ids.keys().copied().collect();
        libraries.sort_by_key(|library| library.index());
        for library in libraries {
            let built_id = built.graph_ids[&library];
            let uri = built.graph.uri(built_id);
            assert_eq!(
                uri,
                oracle.uri(built_id),
                "node order at {}",
                library.index()
            );
            assert_eq!(
                built.graph.name(built_id),
                oracle.name(built_id),
                "declared name of {uri}"
            );
            assert_eq!(
                format!("{:?}", built.graph.import_edges(built_id)),
                format!("{:?}", oracle.import_edges(built_id)),
                "import edges of {uri}"
            );
            assert_eq!(
                format!("{:?}", built.graph.export_edges(built_id)),
                format!("{:?}", oracle.export_edges(built_id)),
                "export edges of {uri}"
            );
            assert_eq!(
                built.graph.unit_files(built_id),
                oracle.unit_files(built_id),
                "unit registration order of {uri}"
            );
            for name in &names {
                assert_eq!(
                    built.graph.local(built_id, *name),
                    oracle.local(built_id, *name),
                    "local face of {uri}"
                );
                assert_eq!(
                    built.graph.class_modifiers(built_id, *name),
                    oracle.class_modifiers(built_id, *name),
                    "class modifiers of {uri}"
                );
            }
        }
        for name in &names {
            assert_eq!(
                format!("{:?}", built.graph.declaring_libraries(*name)),
                format!("{:?}", oracle.declaring_libraries(*name)),
                "declaring-library order"
            );
        }
    }

    /// The parallel build's byte-parity pin (#1042): the same residency
    /// pushed through the parallel pass and through the serial oracle
    /// produces identical graphs. The fixture: five libraries with
    /// cross-imports, a shared target imported twice, an explicit `dart:math`
    /// import, unknown `dart:` faces, a `dart:` export with a show
    /// combinator (the SDK-face export edge), a part file (multi-file
    /// membership), a modifier-carrying class family, and a
    /// declaration-free library.
    #[test]
    fn parallel_graph_build_matches_the_serial_walk() {
        let fixture = crate::l1::tests::Fixture::new();
        let mut state = fixture.state();
        let lib = fixture.root.join("lib");
        crate::l1::tests::set(
            &mut state,
            &lib.join("a.dart"),
            "class A {}\nabstract class B {}\nint a() => 1;\n",
        );
        crate::l1::tests::set(
            &mut state,
            &lib.join("b.dart"),
            "import 'a.dart';\nimport 'dart:math' show max;\npart 'p.dart';\nint b() => 2;\n",
        );
        crate::l1::tests::set(
            &mut state,
            &lib.join("p.dart"),
            "part of 'b.dart';\nint p() => 22;\n",
        );
        crate::l1::tests::set(
            &mut state,
            &lib.join("c.dart"),
            "import 'a.dart';\nimport 'a.dart' show A;\nexport 'a.dart' hide a;\nint c() => 3;\n",
        );
        crate::l1::tests::set(
            &mut state,
            &lib.join("d.dart"),
            "import 'dart:async';\nimport 'dart:nope';\nint d() => 4;\n",
        );
        crate::l1::tests::set(
            &mut state,
            &lib.join("e.dart"),
            "export 'dart:typed_data' show Uint8List;\nimport 'a.dart';\n",
        );
        crate::l1::apply(&mut state, &mut |_| {});

        let built = build_library_graph(&mut state);
        let oracle = serial_library_graph(&mut state);
        assert_graphs_match(&state, &built, &oracle);
    }

    /// The library-name feed (#2117): the declared `library` name reaches
    /// the built graph — the package:meta recognition key the semantic
    /// layer reads (`element.dart:2100-2117`) — and a directive-less
    /// library carries the oracle's empty-string name
    /// (`library_builder.dart:750-763`). The serial oracle makes the
    /// identical feed, so the two graphs stay byte-equal on the face.
    #[test]
    fn graph_library_name_carries_the_declared_name_or_empty() {
        let fixture = crate::l1::tests::Fixture::new();
        let mut state = fixture.state();
        let lib = fixture.root.join("lib");
        let meta_file = crate::l1::tests::set(&mut state, &lib.join("meta.dart"), "library meta;\n");
        let plain_file =
            crate::l1::tests::set(&mut state, &lib.join("plain.dart"), "int f() => 1;\n");
        crate::l1::apply(&mut state, &mut |_| {});

        let built = build_library_graph(&mut state);
        let oracle = serial_library_graph(&mut state);
        assert_graphs_match(&state, &built, &oracle);
        for (file, expected) in [(meta_file, "meta"), (plain_file, "")] {
            let graph_id = built.graph_ids[&state.library_of_file[&file]];
            assert_eq!(built.graph.name(graph_id), expected, "fed name");
            assert_eq!(oracle.name(graph_id), expected, "oracle name");
        }
    }

    /// The core face carries dart:core's `Iterable` class: the engine's
    /// implicit core import must resolve the name every `for-in` loop and
    /// iterable spread desugars against, and a missing entry turns the
    /// unresolved use into an overfired `undefined_class` while the host
    /// invalid gate swallows the diagnostics that should follow. The table
    /// also stays ASCII-sorted, the review convention every prior entry kept.
    #[test]
    fn core_face_resolves_iterable_in_sorted_order() {
        let core = SDK_LIBRARY_FACES
            .iter()
            .find(|(uri, _)| *uri == "core")
            .expect("dart:core face")
            .1;
        assert!(
            core.contains(&"Iterable"),
            "the implicit core import must resolve `Iterable`"
        );
        assert!(
            core.windows(2).all(|pair| pair[0] <= pair[1]),
            "the core face table stays ASCII-sorted"
        );
    }

    /// Directive resolution under the parallel lanes (#1042): multiple
    /// libraries importing shared targets — `a.dart` from three of them and
    /// `dart:core` — resolve through the parallel-built graph; the one
    /// written `show` token the target's export scope lacks reports, every
    /// defined one stays silent. (`dart:core` because the fixture's
    /// synthetic SDK registers only that library — a `dart:math` import
    /// would add its own URI-miss row; the core face declares `int`.)
    #[test]
    fn parallel_build_resolves_shared_directive_targets() {
        let fixture = crate::l1::tests::Fixture::new();
        let mut state = fixture.state();
        let lib = fixture.root.join("lib");
        crate::l1::tests::set(
            &mut state,
            &lib.join("a.dart"),
            "class A {}\nint a() => 1;\n",
        );
        crate::l1::tests::set(
            &mut state,
            &lib.join("b.dart"),
            "import 'a.dart';\nimport 'dart:core' show int;\nint b() => 2;\n",
        );
        crate::l1::tests::set(
            &mut state,
            &lib.join("c.dart"),
            "import 'a.dart' show a;\nint c() => 3;\n",
        );
        crate::l1::tests::set(
            &mut state,
            &lib.join("d.dart"),
            "import 'a.dart' show nope;\nint d() => 4;\n",
        );
        let mut graph: Option<ResolvedGraph> = None;
        let cancel = CancellationToken::new();
        let rows =
            analyze(&mut state, &mut graph, None, &[], &mut |_| {}, &cancel).expect("cold analyze");
        assert_eq!(rows.len(), 1, "one undefined shown name: {rows:?}");
        assert_eq!(rows[0].code.name, "undefined_shown_name");
    }

    /// The per-file cap's passthrough face: at or under the cap the run is
    /// byte-identical (the hot path's one-comparison gate — a library whose
    /// whole run fits under the cap cannot carry an over-cap file), and a
    /// file sitting exactly at the cap is not truncated either (the parser
    /// fuse's full run — 9_999 admitted rows plus its terminal row — passes
    /// intact, so a fuse-tripped file needs no engine truncation of its
    /// own).
    #[test]
    fn cap_passthrough_leaves_under_cap_runs_untouched() {
        let mut files = FileTable::default();
        let mut rows: Vec<Arc<Diagnostic>> = (0..100)
            .map(|i| Arc::new(diagnostic(&mut files, "/t/a.dart", i, "under")))
            .collect();
        let before = rows.clone();
        cap_per_file(&mut rows);
        assert!(rows.iter().zip(&before).all(|(a, b)| Arc::ptr_eq(a, b)));

        let mut rows: Vec<Arc<Diagnostic>> = (0..MAX_DIAGNOSTICS_PER_FILE)
            .map(|i| Arc::new(diagnostic(&mut files, "/t/a.dart", i as u32, "at")))
            .collect();
        cap_per_file(&mut rows);
        assert_eq!(rows.len(), MAX_DIAGNOSTICS_PER_FILE);
        assert!(
            rows.iter().all(|row| row.code.name == "undefined_function"),
            "no terminal row at exactly the cap"
        );
    }

    /// The per-file cap's truncation face: past the cap the file's first
    /// `MAX_DIAGNOSTICS_PER_FILE` rows survive in aggregation order, one
    /// terminal `dartq/internal_error` row anchors the truncation at the
    /// file (the parser fuse's terminal shape, never a silent cut), and
    /// sibling files' rows are unaffected. Drives `cap_per_file` directly —
    /// the pinned contract is the sink's own truncation shape; a full
    /// engine fixture over a >10_000-row file would only re-derive the
    /// scanner/parser fuses to manufacture the input rows.
    #[test]
    fn cap_truncates_over_cap_files_with_one_terminal_row() {
        let mut files = FileTable::default();
        let over = MAX_DIAGNOSTICS_PER_FILE + 5;
        let mut rows: Vec<Arc<Diagnostic>> = (0..over)
            .map(|i| Arc::new(diagnostic(&mut files, "/t/a.dart", i as u32, "over")))
            .collect();
        rows.extend((0..3).map(|i| Arc::new(diagnostic(&mut files, "/t/b.dart", i, "sibling"))));
        cap_per_file(&mut rows);
        assert_eq!(rows.len(), MAX_DIAGNOSTICS_PER_FILE + 1 + 3);

        let path_of = |row: &Diagnostic| {
            files
                .path(row.file)
                .expect("interned")
                .to_str()
                .expect("utf8 path")
        };
        let mut a_syntax = 0usize;
        let mut a_terminal = 0usize;
        let mut b_rows = 0usize;
        for row in &rows {
            match path_of(row) {
                "/t/a.dart" => {
                    if row.code.name == DARTQ_INTERNAL_ERROR.name {
                        a_terminal += 1;
                        assert_eq!(
                            row.message,
                            format!(
                                "Too many diagnostics ({MAX_DIAGNOSTICS_PER_FILE}); the rest \
                                 were dropped."
                            )
                            .as_str()
                        );
                        assert_eq!(
                            row.range,
                            SourceRange {
                                offset: 0,
                                length: 0
                            }
                        );
                    } else {
                        assert_eq!(row.code.name, "undefined_function");
                        a_syntax += 1;
                    }
                }
                "/t/b.dart" => b_rows += 1,
                other => panic!("unexpected file {other}"),
            }
        }
        assert_eq!(a_syntax, MAX_DIAGNOSTICS_PER_FILE, "first CAP rows keep");
        assert_eq!(a_terminal, 1, "exactly one terminal row");
        assert_eq!(b_rows, 3, "sibling file untouched");
        // The survivors are the run's head in aggregation order; the five
        // over-cap rows are the dropped tail.
        let last_kept = rows
            .iter()
            .filter(|row| path_of(row) == "/t/a.dart" && row.code.name == "undefined_function")
            .map(|row| row.range.offset)
            .max()
            .expect("kept rows");
        assert_eq!(last_kept, (MAX_DIAGNOSTICS_PER_FILE - 1) as u32);
    }
}
