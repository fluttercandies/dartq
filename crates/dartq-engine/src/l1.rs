//! `apply_pending`: the `ContentId` gate → facts recompute → the
//! identity-keyed library-graph delta → cascade closure
//!.

use std::sync::Arc;

use dartq_diagnostics::{CodeInfo, CodeRegistry, Diagnostic, Severity};
use dartq_language::{
    DEFAULT_LANGUAGE_VERSION, Feature, FeatureSet, LanguageVersion, MIN_LANGUAGE_VERSION_CONSTRAINT,
};
use dartq_package::PackageTable as IdentityTable;
use dartq_parser::{Message, MultiplexListener, Parser, ParserFeatures, ParserListener, TokenSpan};
use dartq_resolution::{
    CombinatorClause, Combinators, Configuration, ResolvedTarget, SelectedUri, UriResolution,
    UriResolver, resolve_conditional,
};
use dartq_scanner::{Keyword, KeywordStyle, ScanConfig, ScanResult, TokenKind, TokenTable, scan};
use dartq_sdk::AllowedExperiments;
use dartq_snapshot::EntryKey;
use dartq_source::{
    ContentId, FileId, FileTable, MAX_SOURCE_BYTES, SourceRange, SourceText, StringId, StringTable,
    utf16_len_of,
};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::event::{EngineEvent, KNOWN_ONLY_GRANT, RevalidateGate};
use crate::state::{
    DirectiveFacts, DirectiveKind, EngineState, FACTS_SEMANTIC_VERSION, FactsKey, FeatureGrants,
    ImportEdge, LibraryId, LibraryInfo, PendingChange, ScannedFace, TargetRows, UnitFacts,
};

/// The outcome of one apply.
#[derive(Debug, Default)]
pub(crate) struct ApplyOutcome {
    pub changed: Vec<FileId>,
    pub removed: Vec<FileId>,
    pub invalidated: Vec<LibraryId>,
    /// Residents outside the batch whose directive targets re-resolved to a
    /// different outcome because the file set moved (a created file now
    /// satisfies a previously missing target; a removed file un-resolves
    /// one). Their content is untouched — only `target`/`code` columns and
    /// the URI diagnostic tail moved — so they join the structure gate's
    /// refresh input (a target FileId flip, e.g. the generated-file
    /// fallback, moves the folded digest face; a pure code flip digests
    /// equal).
    pub relinked: Vec<FileId>,
}

impl ApplyOutcome {
    fn empty() -> Self {
        Self::default()
    }
}

/// Drains the pending overlay and runs the L1 cascade.
///
/// Event sequence for a non-empty queue: `OverlayApplied`, then
/// `FileRevalidated` per touched file (ascending `FileId`), then one
/// `CascadeInvalidated` (possibly empty). An empty queue emits nothing.
#[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
pub(crate) fn apply(
    state: &mut EngineState,
    on_event: &mut dyn FnMut(EngineEvent),
) -> ApplyOutcome {
    if state.pending.is_empty() {
        return ApplyOutcome::empty();
    }

    // Step 1: the ContentId gate (official defers the comparison to here,
    // never to set_content — a rewrite with identical bytes is a legal no-op).
    // The id comes off the pending lease (hashed once at canonicalization/
    // load); `put_text` shares the lease into the store with no second hash
    // and, for mapping-backed stores, no copy.
    let order = std::mem::take(&mut state.pending_order);
    let mut pending = std::mem::take(&mut state.pending);
    let mut changed: Vec<(FileId, dartq_source::ContentId, SourceText)> = Vec::new();
    let mut removed: Vec<FileId> = Vec::new();
    let mut gate_hits: Vec<FileId> = Vec::new();
    // Previous facts of every touched file — the OLD face the graph delta
    // subtracts. Captured here because both arms destroy the entries: a
    // removal drops residency, a set overwrites it below.
    let mut old_facts: FxHashMap<FileId, Arc<UnitFacts>> = FxHashMap::default();
    for file in order {
        match pending.remove(&file) {
            Some(PendingChange::Set(text)) => {
                // The span-capacity gate: this funnel is the one point
                // every intake (mmap discovery, `set_content`, the manager
                // overlay) passes through on the way to residency, so
                // holding the line here keeps every downstream position
                // derivation — line starts, UTF-16 offsets, range ends —
                // inside u32 by construction. A rejected set is dropped
                // whole: the file keeps its previous residency (or stays
                // absent), the rest of the batch proceeds, and the
                // rejection rides the event face.
                if text.len() > MAX_SOURCE_BYTES {
                    if let Some(path) = state.files.path(file) {
                        on_event(EngineEvent::SourceRejected {
                            path: path.to_path_buf(),
                            len: text.len() as u64,
                        });
                    }
                    continue;
                }
                if let Some(previous) = state.residency.get(&file) {
                    old_facts.insert(file, Arc::clone(previous));
                }
                let content = state.store.put_text(&text);
                if state
                    .residency
                    .get(&file)
                    .is_some_and(|facts| facts.content == content)
                {
                    gate_hits.push(file);
                } else {
                    changed.push((file, content, text));
                }
            }
            Some(PendingChange::Remove) => {
                if let Some(previous) = state.residency.remove(&file) {
                    old_facts.insert(file, previous);
                    // The maintained index inputs drop with residency: a
                    // removed file's capture must not serve a later query.
                    state.clear_capture(file);
                    // The canonical-URI memo drops with residency too
                    // (bounded, audit #398 F10 — the map previously grew
                    // monotonically with every path ever resident). Eviction
                    // is safe because re-formation lazily recomputes the
                    // entry from the file's immutable interned path (the
                    // package table and config dir are fixed at state
                    // assembly), so a returning file reproduces the
                    // identical string and no library identity can fork
                    // across the gap.
                    state.canonical_uris.remove(&file);
                    // The semdiag fold memo tracks residency (#1384): the
                    // removed member's entry goes with it.
                    state.clear_semdiag_fold(file);
                    removed.push(file);
                }
            }
            None => {}
        }
    }
    debug_assert!(pending.is_empty(), "drained entries must all be consumed");
    let changed_files: Vec<FileId> = changed.iter().map(|(file, _, _)| *file).collect();

    on_event(EngineEvent::OverlayApplied {
        changed: changed_files.clone(),
        removed: removed.clone(),
    });
    if let Some(sink) = state.trace.as_mut() {
        sink.overlay_applied(changed_files.len(), removed.len());
    }

    // Steps 2–3: facts recompute for gate-passing files (parallel scan/parse/
    // collect, sequential URI resolution), then residency update. The
    // pre-apply signatures are captured first — the L2 gate
    // compares them against the recomputed ones to classify the cascade.
    let recomputed = recompute_facts(state, &changed);
    let old_signatures: FxHashMap<FileId, dartq_semantic::ApiSignature> = recomputed
        .iter()
        .filter_map(|(file, _, _, _)| {
            state
                .residency
                .get(file)
                .map(|facts| (*file, facts.api_signature))
        })
        .collect();
    let mut gates: Vec<(FileId, RevalidateGate)> = gate_hits
        .into_iter()
        .map(|file| (file, RevalidateGate::ContentIdHit))
        .collect();
    gates.extend(recomputed.iter().map(|(file, _facts, cache_hit, _)| {
        (
            *file,
            RevalidateGate::Parsed {
                ast_cache_hit: *cache_hit,
            },
        )
    }));
    // Recapture bookkeeping: every changed file's declaration spans may
    // have moved (a body edit under a stable signature keeps its library
    // clean but shifts offsets), so recapture tracks changed files, not
    // dirty libraries. Files whose capture rode the apply-side multiplexed
    // parse store directly; the recompute-derivated rest (batch siblings,
    // cross-batch hits, warm starts — no parse ran for them) queue for the
    // analyze-phase recapture. Gate hits (identical bytes) are not in the
    // changed set at all.
    for (file, facts, _, capture) in &recomputed {
        state.admit_residency(*file, Arc::clone(facts));
        match capture {
            Some(capture) => state.set_capture(*file, Arc::clone(capture)),
            None => state.mark_captures_dirty(*file),
        }
    }
    gates.sort_unstable_by_key(|(file, _)| *file);
    for (file, gate) in &gates {
        on_event(EngineEvent::FileRevalidated {
            file: *file,
            gate: *gate,
        });
        if let Some(sink) = state.trace.as_mut()
            && let Some(path) = state.files.path(*file)
        {
            sink.file_revalidated(&path.to_string_lossy(), *gate);
        }
    }

    // The directive relink (#2100): intake resolves every changed file
    // against the post-batch world, but a batch that moves the FILE SET —
    // a newly resident file, or a non-empty removal list — changes the
    // existence face for residents that never entered the queue. A
    // previously missing import target that just appeared must drop its
    // `uri_does_not_exist` row; a removed target must re-acquire it. The
    // holders are already indexed: every file scheme target interned its
    // FileId at intake whether the file existed or not, so the maintained
    // target→sources projections name exactly the affected residents — no
    // residency scan. Runs before the graph delta so the relinked faces
    // feed `UnitFace`/`build_library_info` like any changed file's.
    let relinked = relink_directives(state, &recomputed, &removed, &old_facts);

    // The L2 classification: a file whose API signature moved (or that is
    // newly resident — no previous signature to match) re-seeds the cascade;
    // a body/comment edit with a stable face does not.
    let sig_changed: Vec<FileId> = recomputed
        .iter()
        .filter(|(file, facts, _, _)| old_signatures.get(file) != Some(&facts.api_signature))
        .map(|(file, _, _, _)| *file)
        .collect();

    // The semdiag fold memo's posture epoch (#1384): the per-library
    // `class-modifiers` posture and the members' settled feature sets are
    // the one fold input no per-library cycle fingerprint folds — an
    // `// @dart` header is a comment, so it moves the settled feature face
    // (and with it other libraries' posture answers) under a standing
    // signature. Any changed file whose settled face moved — or whose
    // previous settled face is no longer derivable (an evicted scanned
    // entry, warm-started facts) — bumps the epoch once, forking every
    // memo key conservatively. Pure comment/body/signature churn leaves
    // the settled face standing and pays two cache probes.
    let posture_moved = changed.iter().any(|(file, _, _)| {
        let old_face = old_facts.get(file).and_then(|facts| {
            state.cached_scanned(facts_key(&state.grants(), *file, facts.content))
        });
        let new_face = state.settled_face(*file);
        match (old_face, new_face) {
            (Some(old), Some(new)) => old.features.fingerprint() != new.features.fingerprint(),
            _ => true,
        }
    });
    if posture_moved {
        state.semdiag_posture_epoch += 1;
    }

    // Steps 4–5: library-graph delta, dirty seeds, importer closure. The
    // closure output *replaces* the phase-2 work queue
    // (state.rs: "post-closure dirty set of the last apply; consumed by
    // analyze") — assignment, never a union. A cancelled analyze may leave
    // unfinished libraries queued; close() seeds those leftovers into its
    // dirty set, filtered to the post-delta live set: the delta alone cannot
    // see closure-stale leftovers — a library invalidated
    // because a DEPENDENCY's face moved carries that staleness nowhere in
    // its own identity (that is why cancel is a token, not an identity diff)
    // — so dropping such a leftover would serve its stale cached result
    // forever, and incremental would diverge from a full recompute
    // permanently. Assignment still preserves the invariant
    // `state.invalidated ⊆ live()`; a union would resurrect ids retired by
    // an intervening remove, and analyze unwraps every queued id's info.
    //
    // The closure is signature-gated: seeds are the owners of
    // signature-changed files, and both live owners of every file whose
    // membership moved (a `part of` detach strands the losing library's
    // importers on stale facts; a claim transfer hands its part to a
    // competitor library that no other seed reaches). The delta reads the
    // pre-apply owners straight out of `library_of_file` before it writes
    // any membership entry.
    let (invalidated, retired_any) = maintain_and_close(
        state,
        &changed,
        &removed,
        &relinked,
        &old_facts,
        &sig_changed,
    );
    state.invalidated.clone_from(&invalidated);
    // The resolution graph rebuilds only when its inputs moved: a
    // signature-bearing face (element or directive), a removal, or a library
    // retirement. A body edit with a stable face leaves the graph byte-identical.
    // Accumulate, never overwrite (#1567): batches landing between two
    // analyzes accumulate — a later body-only apply must not retire an
    // earlier apply's move, or the gated analyze skips the condensation
    // rebuild and the reuse gate serves fingerprints folded under pre-move
    // faces. The analyze phase consumes and clears the flag once.
    state.structure_dirty |= retired_any || !removed.is_empty() || !sig_changed.is_empty();
    on_event(EngineEvent::CascadeInvalidated {
        libraries: invalidated.clone(),
    });
    if state.trace.is_some() {
        let libraries: Vec<String> = invalidated
            .iter()
            .filter_map(|library| {
                state
                    .libraries
                    .info(*library)
                    .and_then(|info| state.files.path(info.defining))
                    .map(|path| path.to_string_lossy().into_owned())
            })
            .collect();
        if let Some(sink) = state.trace.as_mut() {
            sink.cascade_invalidated(&libraries);
        }
    }
    // The version-skew degrade telemetry (#1304): the grant lanes queue one
    // record per never-reported unknown whitelist name; the drain emits
    // them after the standard apply sequence — at most one event per
    // unknown name per engine, never per file. A record derived outside an
    // apply (none today; every grant lane runs under one) rides the next
    // non-empty apply's drain.
    for record in state.feature_grants.take_skew() {
        if let Some(sink) = state.trace.as_mut() {
            sink.experiment_degrade(&record.sdk_version, &record.unknown);
        }
        on_event(EngineEvent::ExperimentDegrade {
            sdk_version: record.sdk_version,
            unknown: record.unknown,
            granted: KNOWN_ONLY_GRANT,
        });
    }
    // The residency-pairing degrade telemetry (stability lane #1459): the
    // derived passes queue one row per (file, face) pairing miss they
    // salvaged over — the map serves the miss, the analyze never aborts.
    // The drain emits each pair once per engine (the report set dedupes),
    // trace-only so the event goldens stay byte-stable (the g485 D3
    // posture).
    for (file, face) in state.residency_degrades.take() {
        if let Some(sink) = state.trace.as_mut()
            && let Some(path) = state.files.path(file)
        {
            sink.residency_degrade(&path.to_string_lossy(), face);
        }
    }
    // Amortized store collection: this apply's replaced and removed
    // residency contents are the new dead candidates (gate hits carry none
    // — their put deduplicated); facts-cache evictions tallied themselves
    // during the recompute. Runs after the residency rewrite above, so the
    // replaced contents are already unreferenced if the sweep fires.
    state.collect_garbage(changed.len() + removed.len());
    // The reachability memo's in-flight window (#1383): residency moved
    // without the drain/recompute pass, so reads serve the standing map
    // until the next analyze's gate clears it.
    state.mark_reachability_applied();
    ApplyOutcome {
        changed: changed_files,
        removed,
        invalidated,
        relinked: relinked.iter().map(|(file, _)| *file).collect(),
    }
}

/// Recomputes the facts of every gate-passing file: fan-out over the
/// pure per-file work (language version → scan → directive diet walk → one
/// multiplexed parse feeding diagnostics, element collection, and index
/// capture), then a sequential URI-resolution pass
/// (it interns directive targets into the shared `FileTable`).
///
/// A file whose `(content, feature fingerprint)` key already sits in the
/// facts cache skips the fan-out entirely (`ast_cache_hit`). Batch members
/// sharing a key — identical bytes at the same feature set — compute once:
/// the fan-out runs for the first member only and the rest derive from it
/// (also reported as `ast_cache_hit`, no scan/parse ran). Cross-batch hits
/// derive from the cached face the same way. The derivation re-runs only
/// the per-file steps: diagnostics are re-attributed to the member's own
/// file, and its directives re-resolve against its own path — a relative
/// `import 'u.dart'` resolves differently per directory, so the cache holds
/// exactly the pure `(content, feature set)` face, pre-resolution.
/// The cold-apply fan-out's in-flight bound: misses per compute→resolve
/// chunk. One chunk's batch products — a facts face (~0.6–0.9 KB), a
/// scanned face (~20 B/token × 60–120 tokens), and a capture (~0.8–1.6 KB)
/// per miss — peak at ≈ 4,096 × ~5 KB ≈ 20 MB, where the unchunked fan-out
/// held every miss's products for the whole apply (the scanned faces alone
/// bracket ~120–240 MB at the 10⁵-file face). Only the in-flight bound
/// moves: chunks consume `misses` consecutively, so the resolution pass,
/// the cache-insertion sequence, and the LRU caps' eviction order are
/// exactly the unchunked ones — the persisted face is byte-identical.
const APPLY_FANOUT_CHUNK: usize = 4096;

/// One recomputed file face: file id, parsed facts, whether the file's
/// exports changed (the signature delta), and the index capture when queued.
type RecomputedFacts = (
    FileId,
    Arc<UnitFacts>,
    bool,
    Option<Arc<dartq_index::FileCapture>>,
);
/// The per-file compute outcome without its file id (the batch slot face).
type FactsFace = (Arc<UnitFacts>, bool, Option<Arc<dartq_index::FileCapture>>);

/// Changed files parse **once** on the apply side: the multiplexer
/// ([`MultiplexListener`]) fans that parse's events out to the recovery
/// diagnostics, the element collector, and the index capturer. The
/// analyze-side semdiag fold keeps its own parse (graph-dependent), so a
/// changed file costs at most two parses per cycle; the capture rides the
/// apply parse, and only recompute-derivated files (batch siblings,
/// cross-batch hits, warm starts) queue for the analyze-phase recapture.
#[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
fn recompute_facts(
    state: &mut EngineState,
    changed: &[(FileId, ContentId, SourceText)],
) -> Vec<RecomputedFacts> {
    // Language version per file: the owning package's version (the engine's
    // single source of truth for file → package attribution).
    let versions: Vec<LanguageVersion> = changed
        .iter()
        .map(|(file, _, _)| language_version_for(&state.packages, &state.files, *file))
        .collect();

    // Cache lookups up front (read-only); misses dispatch to the fan-out
    // below, deduplicated by key so a batch computes each distinct
    // `(content, feature set)` once.
    let mut slots: Vec<Option<FactsFace>> = Vec::with_capacity(changed.len());
    slots.resize_with(changed.len(), || None);
    let mut misses: Vec<(usize, FactsKey)> = Vec::new();
    // FactsKey → batch slot of the first miss carrying it.
    let mut first_miss: FxHashMap<FactsKey, usize> = FxHashMap::default();
    // Batch slots deriving from an earlier slot's computation (same key),
    // grouped by their template slot.
    let mut siblings: FxHashMap<usize, Vec<usize>> = FxHashMap::default();
    // Cross-batch facts-cache hits (#303): faces computed for another
    // batch's files. Derivation is deferred to the resolution pass below —
    // it must re-run the per-file steps under the one sequential resolver.
    let mut hits: Vec<(usize, Arc<UnitFacts>)> = Vec::new();
    // Warm-start slots (#482): identities the staged snapshot answers. The
    // map lookup IS the content-hash verification — the key's hash column
    // is the file's live ContentId. Checked before the sibling dedup
    // deliberately: a snapshot payload is post-resolution, so unlike the
    // facts cache it cannot serve as a derive template; every same-key
    // slot materializes its own face.
    let mut warm: Vec<(usize, FactsKey)> = Vec::new();
    for (index, (_, content, _)) in changed.iter().enumerate() {
        let key = facts_key(&state.grants(), changed[index].0, *content);
        if let Some(facts) = state.cached_facts(key) {
            hits.push((index, facts));
        } else if state.warm_start.contains_key(&snapshot_key(key)) {
            warm.push((index, key));
        } else if let Some(&template) = first_miss.get(&key) {
            siblings.entry(template).or_default().push(index);
        } else {
            first_miss.insert(key, index);
            misses.push((index, key));
        }
    }

    // Sources out of the pending batch (owned strings already in hand), then
    // the pure fan-out: no shared mutable state but the StringTable's
    // interning lock.
    let sources: Vec<&str> = changed.iter().map(|(_, _, text)| text.as_str()).collect();

    // Warm-start materialization (#482): each matched payload comes off the
    // staging map — the keyed removal is the content-hash verification —
    // and materializes against the live tables. Grouped by key so a
    // same-content batch (one payload, several consumers) removes it once;
    // the extras clone the payload (identical bytes at two paths, rare and
    // bounded by the batch). Grow-only: ingestion seeds recomputes, it
    // never touches residency (#377). The block runs before any resolution:
    // materialization interns the payload paths into the file table, and
    // that intern order — materialize first, resolve second — is what pins
    // the persisted file-id assignment.
    let mut warmed: Vec<(usize, FactsKey, UnitFacts)> = Vec::with_capacity(warm.len());
    if !warm.is_empty() {
        // One registry per load: materialize runs per entry, and a
        // per-entry `CodeRegistry::builtin()` would rebuild the ~270-entry
        // map for every snapshot file (the #269 F2 anti-pattern).
        let codes = CodeRegistry::builtin();
        let mut by_key: FxHashMap<FactsKey, Vec<usize>> = FxHashMap::default();
        for (index, key) in warm {
            by_key.entry(key).or_default().push(index);
        }
        for (key, slots) in by_key {
            let payload = state
                .warm_start
                .remove(&snapshot_key(key))
                .expect("warm key checked at classification");
            let mut slots = slots.into_iter();
            let first = slots.next().expect("grouped slots are non-empty");
            let (first_payload, rest) = if slots.len() > 0 {
                (payload.clone(), Some(payload))
            } else {
                (payload, None)
            };
            warmed.push((
                first,
                key,
                materialize_warm(
                    first_payload,
                    state,
                    sources[first],
                    changed[first].1,
                    &codes,
                ),
            ));
            if let Some(template) = rest {
                for index in slots {
                    warmed.push((
                        index,
                        key,
                        materialize_warm(
                            template.clone(),
                            state,
                            sources[index],
                            changed[index].1,
                            &codes,
                        ),
                    ));
                }
            }
        }
    }
    // Unclaimed staging entries stand (#1387): the lookup key IS the
    // content hash, so a stale entry can never serve a file whose content
    // moved — it waits for a later batch whose content reverts to the staged
    // identity (an undo is exactly that hit), and leaves only when claimed
    // above, shed by the budget's tier-3, or replaced by the next ingest.
    // The old single-shot drain dropped precisely these revert hits.

    // Sequential URI resolution over the shared FileTable: each template
    // resolves for its own file, then each sibling derives from the
    // pre-resolution template — diagnostics re-attributed, directives
    // re-resolved — under the same sequential pass (resolution interns).
    // The existence face is the post-batch world: files already in
    // residency plus this batch (whose facts insert into residency only
    // after this pass; removals left it at the drain). Field borrows stay
    // disjoint: the closure reads residency, the resolver writes files.
    let batch: FxHashSet<FileId> = changed.iter().map(|(file, _, _)| *file).collect();

    // The fan-out runs in bounded chunks of [`APPLY_FANOUT_CHUNK`] misses:
    // each chunk computes in parallel, resolves, and flushes the deferred
    // cache vectors before the next chunk computes. The batch products —
    // one facts face, one scanned face, and one capture per miss — live for
    // a single chunk instead of the whole apply (the cold-apply transient
    // shrinks by the chunking factor), while everything observable stays in
    // the unchunked order: chunks consume `misses` consecutively, so the
    // resolution pass, the cache-insertion sequence, and the LRU caps'
    // eviction order are exactly the unchunked ones. The resolver is
    // per-chunk because its memos freeze the analyzed world (`residency` +
    // `batch`, both unchanged across chunks) — a fresh instance answers
    // identically and only re-pays the memo warm-up — and dropping it
    // releases the `&mut FileTable` borrow the cache flush needs.
    for chunk in misses.chunks(APPLY_FANOUT_CHUNK) {
        let computed: Vec<(
            usize,
            FactsKey,
            UnitFacts,
            ScannedFace,
            dartq_index::FileCapture,
        )> = chunk
            .par_iter()
            .map(|(index, key)| {
                let (facts, capture, face) = compute_unit(
                    changed[*index].0,
                    changed[*index].1,
                    sources[*index],
                    versions[*index],
                    &state.strings,
                    &state.grants(),
                );
                (*index, *key, facts, face, capture)
            })
            .collect();
        let mut to_cache: Vec<(FactsKey, Arc<UnitFacts>)> = Vec::with_capacity(computed.len());
        // Scanned faces wait for the resolver's `&mut FileTable` borrow to
        // end (same deferred-insert discipline as `to_cache`).
        let mut scanned_to_cache: Vec<(FactsKey, ScannedFace)> = Vec::with_capacity(computed.len());
        {
            let resident = |id: FileId| state.residency.contains_key(&id) || batch.contains(&id);
            let mut resolver = UriResolver::new(
                &state.uri_packages,
                &state.sdk,
                &state.declared_variables,
                &mut state.files,
                &resident,
            );
            for (index, key, mut facts, face, capture) in computed {
                // The scan face persists under the same key (batch siblings
                // and cross-batch hits share it): the semdiag fold and the
                // index captures parse through these tokens instead of
                // re-scanning every member of every dirty library (the
                // audit's per-analyze re-scan multiplication).
                scanned_to_cache.push((key, face));
                // The cache holds the pure face, pre-resolution: resolution
                // is a per-file step, so caching its results would serve one
                // file's targets (and URI diagnostics) to every
                // same-content successor.
                to_cache.push((key, Arc::new(UnitFacts::clone(&facts))));
                for sibling in siblings.remove(&index).unwrap_or_default() {
                    let derived = derive_facts(
                        &facts,
                        changed[sibling].0,
                        &mut resolver,
                        &state.declared_variables,
                    );
                    slots[sibling] = Some((Arc::new(derived), true, None));
                }
                resolve_unit(
                    &mut facts,
                    changed[index].0,
                    &mut resolver,
                    &state.declared_variables,
                );
                slots[index] = Some((Arc::new(facts), false, Some(Arc::new(capture))));
            }
        }
        for (key, face) in scanned_to_cache {
            state.cache_scanned(key, face);
        }
        for (key, facts) in to_cache {
            state.cache_facts(key, facts);
        }
    }
    // Cross-batch hits (#303): the served face was computed for another
    // file's identity — its diagnostics point there and its directives
    // resolved against that file's path. Re-run the per-file derivation
    // (same helper as the batch siblings) so the hit lands fully
    // re-attributed: the AstKey triple is path-free by contract,
    // making this a hit-site duty.
    // The warm-start re-derivation (#482) rides the same resolver: the
    // materialized face attributes and resolves against the snapshot's
    // *saved* file; every baked diagnostic belongs to the unit, so
    // re-attributing all of them to the consuming file is exact, and the
    // URI diagnostic tail rebuilds one-for-one with the re-resolved coded
    // directives (resolve_unit's construction invariant: exactly one pushed
    // diagnostic per coded directive, after every other diagnostic) — a
    // same-content file at another path re-resolves against its own
    // directory.
    let mut warm_to_cache: Vec<(FactsKey, Arc<UnitFacts>)> = Vec::with_capacity(warmed.len());
    {
        let resident = |id: FileId| state.residency.contains_key(&id) || batch.contains(&id);
        let mut resolver = UriResolver::new(
            &state.uri_packages,
            &state.sdk,
            &state.declared_variables,
            &mut state.files,
            &resident,
        );
        for (index, facts) in hits {
            let derived = derive_facts(
                &facts,
                changed[index].0,
                &mut resolver,
                &state.declared_variables,
            );
            slots[index] = Some((Arc::new(derived), true, None));
        }
        for (index, key, mut facts) in warmed {
            // Re-attribution re-clones only rows pointing elsewhere (the S4
            // dedup: shared rows are immutable); a same-path materialization
            // keeps every handle.
            let file = changed[index].0;
            for diagnostic in &mut facts.diagnostics {
                if diagnostic.file != file {
                    let mut row = (**diagnostic).clone();
                    row.file = file;
                    *diagnostic = Arc::new(row);
                }
            }
            let uri_tail = facts
                .directives
                .iter()
                .filter(|directive| directive.code.is_some())
                .count();
            facts
                .diagnostics
                .truncate(facts.diagnostics.len() - uri_tail);
            // The materialized face is pure and pre-resolution — cache it
            // like a computed face (cross-profile source-stats sharing,
            // #1298 T2): the line index and identity it carries cost one
            // build per content per session, and a later batch with the
            // same content takes the facts-cache hit instead of
            // re-materializing.
            warm_to_cache.push((key, Arc::new(UnitFacts::clone(&facts))));
            resolve_unit(
                &mut facts,
                changed[index].0,
                &mut resolver,
                &state.declared_variables,
            );
            slots[index] = Some((Arc::new(facts), true, None));
        }
    }
    for (key, facts) in warm_to_cache {
        state.cache_facts(key, facts);
    }

    changed
        .iter()
        .zip(slots)
        .map(|((file, _, _), slot)| {
            let (facts, cache_hit, capture) = slot.expect("every changed file ends with facts");
            (*file, facts, cache_hit, capture)
        })
        .collect()
}

/// The snapshot entry identity of a facts key (#482): the same
/// `(content, feature, semantic)` triple in the snapshot crate's wire
/// form — the staging map's key.
fn snapshot_key(key: FactsKey) -> EntryKey {
    EntryKey {
        content_hash: *key.content.as_bytes(),
        feature_fingerprint: key.feature_fingerprint,
        semantic_version: key.semantic_version,
    }
}

/// Materializes one warm-start payload against the live tables (#482):
/// re-interns the string slice into the engine's table, mints file ids
/// through the engine's own file table, and rebuilds the line index from
/// the batch source. `content` is the batch's own content id — the same
/// bytes' hash the apply gate already computed, so the snapshot path never
/// re-hashes the source. The post-resolution directive targets and
/// attributed diagnostics re-derive per consumer under the shared resolver
/// (the re-derivation pass in [`recompute_facts`]).
fn materialize_warm(
    payload: crate::state::persist::PersistedFacts,
    state: &mut EngineState,
    source: &str,
    content: ContentId,
    codes: &CodeRegistry,
) -> UnitFacts {
    payload.materialize(source, content, &state.strings, codes, &mut |path| {
        state.files.intern(path)
    })
}

/// The per-file derivation over a pure (content, feature set) face, shared
/// by batch siblings (#272) and cross-batch facts-cache hits (#303): clone
/// the template and re-run the two file-specific steps — diagnostics
/// re-attributed to `file`, directives re-resolved against its own path.
/// The template must be pre-resolution (one URI diagnostic per coded
/// directive; resolving twice would double them).
fn derive_facts(
    template: &UnitFacts,
    file: FileId,
    resolver: &mut UriResolver<'_>,
    declared: &FxHashMap<String, String>,
) -> UnitFacts {
    // The template clone is now a handle copy per row (the S4 dedup);
    // re-attribution re-clones only the rows pointing at another file —
    // the same byte traffic the former full deep clone paid, and nothing
    // more when the hit lands at its own path.
    let mut derived = UnitFacts::clone(template);
    for diagnostic in &mut derived.diagnostics {
        if diagnostic.file != file {
            let mut row = (**diagnostic).clone();
            row.file = file;
            *diagnostic = Arc::new(row);
        }
    }
    resolve_unit(&mut derived, file, resolver, declared);
    derived
}

/// The file's language version: the owning package's version, or the
/// default when the file belongs to no package — the
/// engine's single source of truth for file → package attribution, shared by
/// the apply recompute and the semantic-seam fold.
pub(crate) fn language_version_for(
    packages: &IdentityTable,
    files: &FileTable,
    file: FileId,
) -> LanguageVersion {
    files
        .path(file)
        .and_then(|path| packages.containing(path))
        .and_then(|identity| identity.language_version)
        .map_or(
            DEFAULT_LANGUAGE_VERSION,
            dartq_pub::LanguageVersion::into_inner,
        )
}

/// The borrowed per-file grant faces: everything the feature-set merge
/// consults, bundled so the per-file lanes pass one reference instead of
/// four disjoint state fields ([`EngineState::grants`] builds it).
pub(crate) struct Grants<'a> {
    /// The memoized grant tables and SDK-library reverse map.
    pub tables: &'a FeatureGrants,
    /// The SDK's parsed `allowed_experiments.json`.
    pub experiments: &'a AllowedExperiments,
    /// File → owning package attribution.
    pub packages: &'a IdentityTable,
    /// Interned paths.
    pub files: &'a FileTable,
}

impl Grants<'_> {
    /// The feature set `file` analyzes under at `version` — the version
    /// baseline with the owner's allowed-experiment grants merged in (audit
    /// #708 P2-1). A no-grant engine returns the baseline unchanged, so the
    /// empty-whitelist corpora stay byte-identical to the pre-grant face.
    pub(crate) fn granted(&self, file: FileId, version: LanguageVersion) -> FeatureSet {
        self.tables
            .granted(self.experiments, self.packages, self.files, file, version)
    }
}

/// The facts/scanned cache key of a resident file: the `AstKey` triple
/// derived from the file's content and its granted feature
/// fingerprint. The single derivation both the apply recompute and
/// the drained consumers share, so a cache entry is always found under the
/// key it was stored under.
pub(crate) fn facts_key(grants: &Grants<'_>, file: FileId, content: ContentId) -> FactsKey {
    let version = language_version_for(grants.packages, grants.files, file);
    FactsKey {
        content,
        feature_fingerprint: grants.granted(file, version).fingerprint(),
        semantic_version: FACTS_SEMANTIC_VERSION,
    }
}

/// The pure per-file pipeline: the shared scan face ([`scan_unit`]) — its
/// lexical diagnostics ride into the facts — then
/// ONE multiplexed parse — parse-recovery diagnostics, element collection,
/// and index capture consume the same event stream (the parse-once
/// fan-out; the analyze-side semdiag fold keeps its own graph-dependent
/// parse) — then the directive diet walk. Returns the facts plus the
/// file's capture and the scan face (tokens + settled feature set) — the
/// face persists in the scanned cache so the analyze-phase consumers parse
/// these tokens instead of re-scanning the file.
fn compute_unit(
    file: FileId,
    content: ContentId,
    text: &str,
    version: LanguageVersion,
    strings: &StringTable,
    grants: &Grants<'_>,
) -> (UnitFacts, dartq_index::FileCapture, ScannedFace) {
    let (feature_set, settled_version, scanned, mut diagnostics) =
        scan_unit(text, version, file, grants);
    // The scan face's lexical rows ride into the facts (the CLI check face
    // extends them the same way). Unmatched begin groups are NOT scanner
    // rows: the scanner records the `(begin, synthetic close)` pairs on the
    // token table and the parse below reports them at their post-parse
    // positions (the official `UnmatchedToken` translation reads
    // `begin.endGroup.charOffset` after the parse), so they arrive in the
    // parse-phase segment. Extended before the parse so the vec keeps
    // pipeline-phase order — scan, parse-recovery (unmatched groups
    // included), duplicate — leaving resolve_unit's URI rows the tail the
    // warm-start rebuild slices. No dedup and no filtering: the output face
    // sorts canonically, and row identity is oracle-adjudicated (the
    // unmatched pass dedupes only identical rows among itself, the official
    // downstream behavior).
    diagnostics.extend(scanned.diagnostics);
    // The parse-once fan-out: recovery diagnostics, the element model, and
    // the index capture each observe the single parse's events, in the
    // order they would in a solo parse. Parsing is the dominant per-file
    // cost after scanning, so three full passes over the same tokens
    // collapse to one.
    let mut diag = DiagCollector::new(file);
    let mut decl = dartq_semantic::DeclarationCollector::new(text, &scanned.tokens, strings);
    let mut capture = dartq_index::Capturer::new(file, text, &scanned.tokens, strings);
    Parser::new(
        text,
        &scanned.tokens,
        ParserFeatures::from_feature_set(feature_set),
        // Real three-consumer fan-out (recovery diagnostics, element model,
        // index capture over one parse): the nested mux stays, borrowed.
        &mut MultiplexListener::new(
            &mut diag,
            &mut MultiplexListener::new(&mut decl, &mut capture),
        ),
    )
    .parse_unit();
    diagnostics.extend(diag.finish());
    // The single allocation of the element model: every later consumer
    // (duplicate bake, api signature, facts clones, the semdiag fold)
    // shares this `Arc` (#308).
    let elements = Arc::new(decl.finish());
    let capture = capture.finish();
    diagnostics.extend(duplicate_diagnostics(
        file,
        &elements,
        strings,
        &capture.index,
    ));
    let (directives, directive_region) = extract_directives(text, &scanned.tokens, strings);
    let face = directive_face(text, &scanned.tokens, directive_region);

    let facts = UnitFacts {
        content,
        api_signature: unit_signature(&elements, &face, strings),
        directives,
        elements,
        // The rows move into their refcounted store form once here (the
        // S4 dedup, #1501): every later consumer — the per-library
        // results, the facts-cache clones — holds handles, never copies.
        diagnostics: diagnostics.into_iter().map(Arc::new).collect(),
        line_index: scanned.line_index,
    };
    let scanned_face = ScannedFace {
        tokens: scanned.tokens,
        features: feature_set,
        version: settled_version,
    };
    (facts, capture, scanned_face)
}

/// The file's API signature: the canonical element-face
/// hash folded with the directive-region lexemes, one blake3 pass over both
/// faces. Computed once per changed file inside the parallel recompute and
/// cached on the facts — never a per-query cost.
fn unit_signature(
    elements: &dartq_semantic::FileElements,
    directive_face: &[u8],
    strings: &StringTable,
) -> dartq_semantic::ApiSignature {
    elements.api_signature_with_directives(strings, directive_face)
}

/// The directive-region lexemes: every token the diet walk consumed
/// (script tag, directives with their URIs, prefixes, `deferred`,
/// show/hide combinator names, library and part-of names), source order.
/// This is the directive face of [`unit_signature`] — an export-combinator
/// or membership edit changes the file's contribution to its importers'
/// namespaces, so it must flip the gate even when the element face stands.
/// Resolution outcomes are deliberately absent: a target's residency is the
/// *other* file's face, and importing it must not re-seed this file's
/// importers.
///
/// Framing is a length prefix (audit #398 F12): per lexeme, the byte length
/// as a little-endian `u64`, then the lexeme's UTF-8 bytes. The face is
/// injective over the lexeme sequence — the retired `\u{1}` separator
/// aliased whenever a lexeme itself carried U+0001 (a raw control character
/// inside a directive's string literal is legal source). The face is only
/// ever hashed, so bare bytes suffice.
fn directive_face(text: &str, tokens: &TokenTable, region_end: usize) -> Vec<u8> {
    let mut face = Vec::new();
    for i in 0..region_end {
        if tokens.get(i).is_some() {
            let lexeme = tokens.lexeme(text, i);
            face.extend_from_slice(&(lexeme.len() as u64).to_le_bytes());
            face.extend_from_slice(lexeme.as_bytes());
        }
    }
    face
}

/// The shared scan face of the engine's two per-file lanes (the apply
/// recompute and the index capture): scans `text` at the file's `version`
/// and settles the `@dart` override per the scanner's consumer contract
/// (official `source_loader.dart:1194–1209` — a differing override
/// re-derives the config and rescans; the scanner reports only the last
/// header-window match, so at most one rescan settles). Returns the settled
/// feature set with the settled version it was derived at — the version the
/// lint fan-out's registration gates consult — plus the scan and the
/// override warnings.
///
/// The feature set carries the file's allowed-experiment grants on top of
/// the version baseline (official `feature_set_provider.dart:58-77`), and
/// the override re-gates the same grants at the override version
/// (`scanner.dart:127`, `featureSetForOverriding.restrictToVersion`) — not
/// a bare re-derivation, so a package's whitelist survives its own
/// `// @dart` pin-down.
///
/// An override above the latest known version is officially rejected, not
/// clamped (official scanner `_languageVersionChanged`): the
/// `invalid_language_version_override` `STATIC_WARNING` pins at the comment
/// lexeme and the override is dropped (`_overrideVersion = null`) — the
/// feature set stays the granted package/default-derived one and no rescan
/// runs. The warning rides back for the diagnostics face to bake; the
/// index-capture face drops it (it produces no diagnostics).
pub(crate) fn scan_unit(
    text: &str,
    version: LanguageVersion,
    file: FileId,
    grants: &Grants<'_>,
) -> (FeatureSet, LanguageVersion, ScanResult, Vec<Diagnostic>) {
    let mut feature_set = grants.granted(file, version);
    let mut settled = version;
    let mut scanned = scan(text, scan_config(feature_set), file);
    let mut warnings = Vec::new();
    if let Some(over) = scanned.language_version_override {
        if over.is_above_latest() {
            // The generated table's const row (audit #269 F2): a per-emit
            // `CodeRegistry::builtin()` build would reconstruct a ~270-entry
            // map for one known code. The GREATER-variant message text stays
            // at this emit site (the generated row keeps the AT_SIGN
            // template — dartq-diagnostics code.rs preamble).
            let code = dartq_diagnostics::INVALID_LANGUAGE_VERSION_OVERRIDE;
            let span = scanned
                .language_version_override_span
                .expect("scanner records the comment span with every override");
            warnings.push(Diagnostic::new(
                code.dtype.severity(),
                code,
                format!(
                    "The language version override can't specify a version greater than \
                     the latest known language version: {DEFAULT_LANGUAGE_VERSION}."
                ),
                file,
                span,
                None,
            ));
        } else {
            // The floor is a verifier check, not a scanner drop
            // (`error_verifier.dart:5787-5809`): the override stays
            // effective — the settled version and the feature grants move
            // to it exactly like the in-range case — and the ERROR rides
            // beside it.
            if over.is_below_floor() {
                let code = dartq_diagnostics::ILLEGAL_LANGUAGE_VERSION_OVERRIDE;
                let span = scanned
                    .language_version_override_span
                    .expect("scanner records the comment span with every override");
                warnings.push(Diagnostic::new(
                    code.dtype.severity(),
                    code,
                    format!("The language version must be {MIN_LANGUAGE_VERSION_CONSTRAINT}."),
                    file,
                    span,
                    Some(
                        "Try removing the language version override and migrating the code."
                            .to_owned(),
                    ),
                ));
            }
            if over != version {
                settled = over;
                feature_set = grants.granted(file, over);
                scanned = scan(text, scan_config(feature_set), file);
            }
        }
    }
    (feature_set, settled, scanned, warnings)
}

/// The scanner's two lexical gates, from the feature set in force.
fn scan_config(feature_set: FeatureSet) -> ScanConfig {
    ScanConfig {
        enable_triple_shift: feature_set.is_enabled(Feature::TripleShift),
        enable_augmentations: feature_set.is_enabled(Feature::Augmentations),
    }
}

/// Fills the resolution columns of a unit's directives and bakes the
/// URI-layer diagnostics (the only resolution diagnostics that are a pure
/// function of the file's own content — everything cross-file belongs to the
/// semantic seam).
///
/// `declared` is the context's conditional-evaluation map (`-D name=value`):
/// configurable imports/exports select the first matching clause against it
/// (the shared `resolve_conditional` decision), falling back to the primary
/// URI. Under the oracle-default empty map every condition misses, so the
/// no-`-D` face is unchanged.
fn resolve_unit(
    facts: &mut UnitFacts,
    file: FileId,
    resolver: &mut UriResolver<'_>,
    declared: &FxHashMap<String, String>,
) {
    // The engine extracts no `@docImport`s at M1, so every resolvable kind
    // maps to the resolution face's `DirectiveKind::ImportExport`; a
    // docImport extraction only gains a branch here. Authority-carrying
    // `dart:` URIs need no pre-validation either: they compose to an empty
    // library name, fail closed in `SdkSnapshot::uri_to_path`, and surface as
    // `uri_does_not_exist` — the oracle's own outcome for the same input
    // (`internalMapDartUri` parses the `dart:` prefix as the library name and
    // finds no such library).
    for directive in &mut facts.directives {
        // A name-form `part of a.b;` carries its target in `name`, never in
        // a URI: the oracle binds it by library-name matching (analyzer
        // `PartOfNameFileKind`, file_state.dart:2162+), so the URI layer has
        // nothing to resolve and emits nothing — pushing the null URI
        // through `resolve` would code a bogus `uri_with_interpolation` at
        // offset 0. The interpolation-split string form still goes through
        // (that is exactly the `uri_with_interpolation` face).
        if directive.kind == DirectiveKind::PartOf && directive.name.is_some() {
            continue;
        }
        if matches!(
            directive.kind,
            DirectiveKind::Import
                | DirectiveKind::Export
                | DirectiveKind::Part
                | DirectiveKind::PartOf
        ) {
            let (uri, _span) = selected_uri(directive, declared);
            let UriResolution { target, code, .. } =
                resolver.resolve(file, uri, dartq_resolution::DirectiveKind::ImportExport);
            directive.target = target;
            directive.code = code;
        }
    }
    for directive in &facts.directives {
        if let Some(code) = directive.code {
            let (uri, span) = selected_uri(directive, declared);
            facts.diagnostics.push(Arc::new(uri_diagnostic(
                code,
                uri.unwrap_or_default(),
                span,
                file,
            )));
        }
    }
}

/// The URI string a directive's resolution selects against `declared`, with
/// its own literal span: the first configuration the declared variables
/// match (the resolution crate's `resolve_conditional` — source order,
/// exact-value equality, primary fallback), else the primary. The span
/// follows the selection so a selected branch's missing-target diagnostic
/// lands on the branch literal — the oracle reports `state.selectedUri`
/// (library_analyzer.dart:698-717), never the primary's. Unmatched branch
/// URIs are never resolved here (the oracle never reads them), so a missing
/// *unselected* target stays silent.
fn selected_uri<'a>(
    directive: &'a DirectiveFacts,
    declared: &FxHashMap<String, String>,
) -> (Option<&'a str>, SourceRange) {
    debug_assert_eq!(
        directive.configurations.len(),
        directive.configuration_spans.len(),
        "configuration spans are index-locked to the clauses"
    );
    match resolve_conditional(&directive.configurations, declared) {
        SelectedUri::Primary => (directive.uri.as_deref(), directive.uri_span),
        SelectedUri::Configuration(index) => (
            Some(directive.configurations[index].uri.as_ref()),
            directive.configuration_spans[index],
        ),
    }
}

/// Whether `template` carries any `{N}` placeholder with `N` above zero —
/// the single-argument `replace("{0}", …)` emit would leave such a template
/// partially literal (audit #269 F5).
fn has_placeholder_above_zero(template: &str) -> bool {
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            let digits = bytes[i + 1..]
                .iter()
                .take_while(|&&b| b.is_ascii_digit())
                .count();
            if digits > 0 && bytes.get(i + 1 + digits) == Some(&b'}') {
                if digits > 1 || bytes[i + 1] != b'0' {
                    return true;
                }
                i += digits + 2;
                continue;
            }
        }
        i += 1;
    }
    false
}

/// One URI-layer diagnostic, built entirely from the resolution registry's
/// descriptor: the family decides TYPE and severity (severity derives from
/// the dtype — no separate severity map), the published flag rides into the
/// code info (JSON `documentation`), and the verbatim correction is carried
/// through (`uri_does_not_exist` family rows have one).
pub(crate) fn uri_diagnostic(
    code: dartq_resolution::ResolutionCode,
    uri: &str,
    range: SourceRange,
    file: FileId,
) -> Diagnostic {
    let info = code.info();
    debug_assert!(
        !has_placeholder_above_zero(info.message),
        "{}: template carries placeholders beyond {{0}}",
        info.name
    );
    Diagnostic::new(
        info.dtype.severity(),
        CodeInfo::new(info.name, info.machine, info.dtype, info.has_published),
        info.message.replace("{0}", uri),
        file,
        range,
        info.correction.map(str::to_owned),
    )
}

/// One directive-combinator static warning — the two-placeholder sibling of
/// [`uri_diagnostic`]: the `undefined_hidden_name`/`undefined_shown_name`
/// templates carry `{0}` (the target library's URI spelling) and `{1}` (the
/// written combinator name); the rest of the descriptor-driven shape is
/// identical (the family decides TYPE and severity, the published flag and
/// the verbatim correction ride along).
pub(crate) fn combinator_diagnostic(
    info: dartq_resolution::ResolutionCodeInfo,
    library_uri: &str,
    name: &str,
    range: SourceRange,
    file: FileId,
) -> Diagnostic {
    Diagnostic::new(
        info.dtype.severity(),
        CodeInfo::new(info.name, info.machine, info.dtype, info.has_published),
        info.message
            .replace("{0}", library_uri)
            .replace("{1}", name),
        file,
        range,
        info.correction.map(str::to_owned),
    )
}

/// One duplicate-directive static warning — the placeholder-free sibling of
/// [`uri_diagnostic`]: the `duplicate_import`/`duplicate_export`/
/// `duplicate_hidden_name`/`duplicate_shown_name` templates carry no `{N}`
/// slots, so the message is the verbatim template; the rest of the
/// descriptor-driven shape is identical (the family decides TYPE and
/// severity, the published flag and the verbatim correction ride along).
pub(crate) fn duplicate_diagnostic(
    info: dartq_resolution::ResolutionCodeInfo,
    range: SourceRange,
    file: FileId,
) -> Diagnostic {
    Diagnostic::new(
        info.dtype.severity(),
        CodeInfo::new(info.name, info.machine, info.dtype, info.has_published),
        info.message.to_owned(),
        file,
        range,
        info.correction.map(str::to_owned),
    )
}

/// Collects parse events into engine diagnostics. `handle_recoverable_error`
/// rides the parser's registry messages; `handle_experiment_not_enabled`
/// renders its bare feature flag through the parser's
/// `experiment_not_enabled_message` selection (the analyzer's
/// `getExperimentNotEnabledMessage` face: shipped flags take
/// `experiment_not_enabled`, off-by-default flags the `_off_by_default`
/// wording, both parameterized with the yaml flag name and anchored at the
/// report's begin token).
struct DiagCollector {
    file: FileId,
    diags: Vec<Diagnostic>,
}

impl DiagCollector {
    fn new(file: FileId) -> Self {
        Self {
            file,
            diags: Vec::new(),
        }
    }

    /// The parse-recovery diagnostics, in arrival order.
    fn finish(self) -> Vec<Diagnostic> {
        self.diags
    }
}

impl ParserListener for DiagCollector {
    fn handle_recoverable_error(&mut self, message: Message, start: TokenSpan, _end: TokenSpan) {
        self.diags.push(Diagnostic::new(
            Severity::Error,
            message.code,
            message.text,
            self.file,
            start.range,
            message.correction,
        ));
    }

    fn handle_experiment_not_enabled(
        &mut self,
        flag: dartq_parser::ExperimentalFlag,
        begin: TokenSpan,
        _end: TokenSpan,
    ) {
        let message = dartq_parser::experiment_not_enabled_message(flag);
        self.diags.push(Diagnostic::new(
            Severity::Error,
            message.code,
            message.text,
            self.file,
            begin.range,
            message.correction,
        ));
    }
}

/// Folds the collector's capture-time duplicate log into the unit's baked
/// diagnostics as the frozen `duplicate_definition` row: ERROR at the second
/// declaration's name token, setters named by their bare identifier (the
/// same construction `dartq_semantic::diagnose` renders — the shared
/// [`dartq_semantic::duplicate_definition_diagnostics`] is the single row
/// shape, this bake is the single source the semdiag fold defers to). The
/// log is a pure function of the unit's content and feature set, so it
/// bakes beside the syntax diagnostics; a wired `SemanticDiagnoser` that
/// runs `diagnose` must not re-fold the same entries (its adapter owes the
/// dedup to this face). The first-declaration context row resolves from
/// the capture's declaration rows ([`capture_first_decl`]); a
/// non-declaration face (named formals) degrades to the plain row.
fn duplicate_diagnostics(
    file: FileId,
    elements: &dartq_semantic::FileElements,
    strings: &StringTable,
    capture: &dartq_index::FileIndex,
) -> Vec<Diagnostic> {
    dartq_semantic::duplicate_definition_diagnostics(file, elements, strings, |name, duplicate| {
        capture_first_decl(capture, name, duplicate)
    })
}

/// The capture-grade first-declaration lookup for the duplicate log: the
/// equal-name run carries every declaration's own name-token span with its
/// container anchor, so the duplicate's row pins the scope and the run's
/// earliest same-scope row is the kept declaration. Named formals have no
/// rows (`capture.rs` — the formals table is #119's) and a top-level
/// function's type-parameter list carries no container anchor, so those
/// faces degrade to the plain row rather than a foreign anchor.
fn capture_first_decl(
    index: &dartq_index::FileIndex,
    name: StringId,
    duplicate: SourceRange,
) -> Option<SourceRange> {
    let rows = index.by_name(name)?;
    let dup_row = rows
        .iter()
        .find(|row| row.range.offset == duplicate.offset && row.range.length == duplicate.length)?;
    if dup_row.kind == dartq_index::DeclKind::TypeParam
        && dup_row.container == dartq_index::DeclRow::NONE
    {
        return None;
    }
    let same_scope = |row: &dartq_index::DeclRow| {
        row.container == dup_row.container
            && row.unnamed_extension_member == dup_row.unnamed_extension_member
            && (dup_row.kind != dartq_index::DeclKind::TypeParam
                || row.kind == dartq_index::DeclKind::TypeParam)
    };
    // The run is `(name, offset)`-ascending: the first same-scope row is
    // the kept declaration, which every later duplicate anchors at.
    rows.iter()
        .find(|row| row.range.offset < duplicate.offset && same_scope(row))
        .map(|row| row.range)
}

// ---------------------------------------------------------------------------
// Directive diet walk
//
// The parser consumes directives diet-style until its directive milestone,
// and emits no directive events. The engine therefore walks the directive
// region of the token table itself: after an optional script tag, metadata,
// and `;`s, `library`/`import`/`export`/`part` keyword tokens open directive
// shapes consumed to their `;`; any other token ends the region (the first
// declaration). Malformed shapes (a directive with no string) yield a
// directive with `uri: None` and no diagnostic — under-reporting, matching
// the "never false-positive" bias until the parser milestone owns these.
// ---------------------------------------------------------------------------

/// Walks the directive region of a token table, returning the directives and
/// the region end (the index of the first non-directive token — the boundary
/// [`directive_face`] hashes over).
fn extract_directives(
    text: &str,
    tokens: &TokenTable,
    strings: &StringTable,
) -> (Vec<DirectiveFacts>, usize) {
    let mut directives = Vec::new();
    let mut i = 0;
    while let Some(token) = tokens.get(i) {
        match token.kind {
            TokenKind::Semicolon | TokenKind::ScriptTag => i += 1,
            TokenKind::At => i = skip_annotation(tokens, i + 1),
            TokenKind::Keyword(Keyword::Library) => {
                i = walk_library(text, tokens, i, strings, &mut directives);
            }
            TokenKind::Keyword(Keyword::Import) => {
                i = walk_import_export(
                    text,
                    tokens,
                    i,
                    strings,
                    &mut directives,
                    DirectiveKind::Import,
                );
            }
            TokenKind::Keyword(Keyword::Export) => {
                i = walk_import_export(
                    text,
                    tokens,
                    i,
                    strings,
                    &mut directives,
                    DirectiveKind::Export,
                );
            }
            TokenKind::Keyword(Keyword::Part) => {
                i = walk_part(text, tokens, i, strings, &mut directives);
            }
            _ => break,
        }
    }
    let region_end = i;
    // Late directives: the official parser recovers an `import`/`export`/
    // `part` that follows a declaration (`directive_after_declaration`,
    // directive_context.dart:50-104) and keeps it in the unit's directive
    // list, so its namespace effect stands. A top-level (brace-depth 0)
    // directive keyword followed by a string literal is that recovered
    // shape; the region end above still bounds the API-face hash.
    let mut depth: u32 = 0;
    while let Some(token) = tokens.get(i) {
        match token.kind {
            TokenKind::OpenCurly => depth += 1,
            TokenKind::CloseCurly => depth = depth.saturating_sub(1),
            TokenKind::Keyword(Keyword::Import | Keyword::Export | Keyword::Part)
                if depth == 0 && kind_at(tokens, i + 1) == Some(TokenKind::String) =>
            {
                let kind = match token.kind {
                    TokenKind::Keyword(Keyword::Import) => Some(DirectiveKind::Import),
                    TokenKind::Keyword(Keyword::Export) => Some(DirectiveKind::Export),
                    _ => None,
                };
                let next = match kind {
                    Some(kind) => walk_import_export(text, tokens, i, strings, &mut directives, kind),
                    None => walk_part(text, tokens, i, strings, &mut directives),
                };
                i = next.max(i + 1);
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    (directives, region_end)
}

/// Reads the URI literal starting at `i`. Adjacent string literals
/// concatenate into one URI (`import 'li' 'b/a.dart';` resolves `lib/a.dart`).
/// The oracle's assembly is the directive uri's `StringLiteral.stringValue`
/// (`file_state.dart:1017,1147`): `AdjacentStringsImpl._appendStringValue`
/// concatenates the per-segment values, each segment decoding its own
/// raw/cooked body (ast.dart:195-200), and any interpolation segment makes
/// the whole literal shapeless (`StringInterpolationImpl._appendStringValue`
/// throws → `null`, ast.dart:51023). Returns the uri text, its UTF-16 span
/// (the whole adjacent region; zero when shapeless), and the index just past
/// the literal region.
fn read_uri(text: &str, tokens: &TokenTable, i: usize) -> (Option<Box<str>>, SourceRange, usize) {
    let interpolated = |kind: Option<TokenKind>| {
        matches!(
            kind,
            Some(
                TokenKind::StringInterpolationExpression | TokenKind::StringInterpolationIdentifier
            )
        )
    };
    if kind_at(tokens, i) != Some(TokenKind::String) {
        return (
            None,
            SourceRange {
                offset: 0,
                length: 0,
            },
            i,
        );
    }
    // Concatenate the run of adjacent plain `String` segments. An
    // undecodable segment (malformed escape) leaves the region shapeless —
    // under-report, never a false positive.
    let mut j = i;
    let mut value = String::new();
    let mut decodable = true;
    let span_offset = tokens.get(i).expect("kind_at checked").utf16_offset;
    // Always overwritten by the first iteration (segment `i` is a `String`).
    let mut span_end = span_offset;
    while let Some(TokenKind::String) = kind_at(tokens, j) {
        let lexeme = tokens.lexeme(text, j);
        let token = tokens.get(j).expect("kind_at checked");
        span_end = token.utf16_offset + utf16_len_of(lexeme);
        match string_literal_value(lexeme) {
            Some(segment) => value.push_str(&segment),
            None => decodable = false,
        }
        j += 1;
    }
    if interpolated(kind_at(tokens, j)) {
        // Skip the whole segment chain: `String` pieces and `${ … }` groups
        // (group-aware, so a `;` inside the interpolation cannot mislead).
        loop {
            match kind_at(tokens, j) {
                Some(TokenKind::String | TokenKind::StringInterpolationIdentifier) => j += 1,
                Some(TokenKind::StringInterpolationExpression) => {
                    j = tokens.matching_end(j).map_or(j + 1, |end| end + 1);
                }
                _ => break,
            }
        }
        return (
            None,
            SourceRange {
                offset: 0,
                length: 0,
            },
            j,
        );
    }
    let span = SourceRange {
        offset: span_offset,
        length: span_end - span_offset,
    };
    (decodable.then(|| value.into_boxed_str()), span, j)
}

fn kind_at(tokens: &TokenTable, i: usize) -> Option<TokenKind> {
    tokens.get(i).map(|token| token.kind)
}

/// Whether `kind` can spell part of an annotation name: identifiers and
/// built-in identifiers (`dynamic`) only — reserved/pseudo keywords would
/// swallow the next declaration.
fn is_name_token(kind: TokenKind) -> bool {
    match kind {
        TokenKind::Identifier => true,
        TokenKind::Keyword(keyword) => keyword.style() == KeywordStyle::BuiltIn,
        _ => false,
    }
}

/// Skips `@qualifiedName[(args)]` starting after the `@`.
fn skip_annotation(tokens: &TokenTable, mut i: usize) -> usize {
    while kind_at(tokens, i).is_some_and(is_name_token)
        || kind_at(tokens, i) == Some(TokenKind::Period)
    {
        i += 1;
    }
    if kind_at(tokens, i) == Some(TokenKind::OpenParen) {
        i = tokens.matching_end(i).map_or(i, |end| end + 1);
    }
    i
}

/// A dotted name (`a.b.c`) of identifier-ish tokens, consumed to the `;`.
/// Returns the joined spelling and the index just past the `;`.
fn walk_dotted_name(text: &str, tokens: &TokenTable, mut i: usize) -> (Option<Box<str>>, usize) {
    let mut name = String::new();
    loop {
        match kind_at(tokens, i) {
            Some(TokenKind::Semicolon) | None => return (Some(name.into_boxed_str()), i + 1),
            Some(kind) if is_name_token(kind) => {
                if !name.is_empty() {
                    name.push('.');
                }
                name.push_str(tokens.lexeme(text, i));
                i += 1;
            }
            Some(TokenKind::Period) => i += 1,
            _ => return (Some(name.into_boxed_str()), i + 1),
        }
    }
}

/// `library [a.b.c];`
fn walk_library(
    text: &str,
    tokens: &TokenTable,
    start: usize,
    _strings: &StringTable,
    out: &mut Vec<DirectiveFacts>,
) -> usize {
    let (name, end) = walk_dotted_name(text, tokens, start + 1);
    out.push(DirectiveFacts {
        kind: DirectiveKind::Library,
        uri: None,
        uri_span: SourceRange {
            offset: 0,
            length: 0,
        },
        target: ResolvedTarget::Missing,
        code: None,
        prefix: None,
        deferred: false,
        combinators: Combinators::unfiltered(),
        name,
        configurations: Box::default(),
        configuration_spans: Box::default(),
    });
    end
}

/// `import`/`export`: uri, conditional-uri clauses (`if (test) uri`),
/// `[deferred] [as p]` (import shapes only in practice, tolerated on both),
/// combinator list, `;`.
fn walk_import_export(
    text: &str,
    tokens: &TokenTable,
    start: usize,
    strings: &StringTable,
    out: &mut Vec<DirectiveFacts>,
    kind: DirectiveKind,
) -> usize {
    let (uri, uri_span, mut i) = read_uri(text, tokens, start + 1);
    let mut prefix = None;
    let mut deferred = false;
    let mut clauses: Vec<(CombinatorClause, Box<[SourceRange]>)> = Vec::new();
    let mut configurations: Vec<Configuration> = Vec::new();
    let mut configuration_spans: Vec<SourceRange> = Vec::new();
    loop {
        match kind_at(tokens, i) {
            None | Some(TokenKind::Eof) => break,
            Some(TokenKind::Semicolon) => {
                i += 1;
                break;
            }
            Some(TokenKind::Keyword(Keyword::If)) => {
                // The conditional-uri clauses join the directive from any
                // clause position: the official import recovery
                // (parseImportRecovery :1022-1026) re-parses them into the
                // same configurations, so a late `if` still forks resolution.
                i = walk_conditional_uri(
                    text,
                    tokens,
                    i,
                    &mut configurations,
                    &mut configuration_spans,
                );
            }
            Some(TokenKind::Keyword(Keyword::Deferred)) => {
                deferred = true;
                i += 1;
            }
            Some(TokenKind::Keyword(Keyword::As)) => {
                if let Some(next) = kind_at(tokens, i + 1)
                    && (is_name_token(next) || matches!(next, TokenKind::Keyword(_)))
                {
                    prefix = Some(strings.intern(tokens.lexeme(text, i + 1)));
                    i += 2;
                    continue;
                }
                i += 1;
            }
            Some(TokenKind::Keyword(Keyword::Show | Keyword::Hide)) => {
                let hide = kind_at(tokens, i) == Some(TokenKind::Keyword(Keyword::Hide));
                i += 1;
                let mut names = Vec::new();
                let mut name_spans: Vec<SourceRange> = Vec::new();
                while let Some(next) = kind_at(tokens, i) {
                    match next {
                        TokenKind::Identifier => {
                            let lexeme = tokens.lexeme(text, i);
                            names.push(strings.intern(lexeme));
                            // The written token's span rides with it — the
                            // per-token combinator-warning anchor
                            // (dead_code_verifier.dart:148-158).
                            let token = tokens.get(i).expect("kind_at checked");
                            name_spans.push(SourceRange {
                                offset: token.utf16_offset,
                                length: utf16_len_of(lexeme),
                            });
                            i += 1;
                        }
                        TokenKind::Comma => i += 1,
                        _ => break,
                    }
                }
                let clause = if hide {
                    CombinatorClause::Hide(names.into())
                } else {
                    CombinatorClause::Show(names.into())
                };
                clauses.push((clause, name_spans.into()));
            }
            _ => i += 1,
        }
    }
    out.push(DirectiveFacts {
        kind,
        uri,
        uri_span,
        target: ResolvedTarget::Missing,
        code: None,
        prefix,
        deferred,
        combinators: Combinators::with_spans(clauses, strings),
        name: None,
        configurations: configurations.into(),
        configuration_spans: configuration_spans.into(),
    });
    i
}

/// One diet-walk `if (dottedName (== 'string')?) 'uri'` clause, mirroring the
/// official `parseConditionalUri` decision order (`parser_impl.dart:1095)`:
/// the paren group (a missing `(` reads the bare name the official's
/// `insertParens` would wrap, :1102-1109), the dotted name, the optional
/// `==` value, the group close (garbage inside a real group is jumped over
/// to the closer, :1124-1136), then the candidate URI.
///
/// A clause is recorded only when its whole shape reads clean and both
/// strings decode — the diet walk's under-report bias (never a false
/// positive): a malformed clause contributes no configuration, which under
/// selection resolves the primary URI, and the parser face carries the
/// diagnostics for the same source. Every return path advances past `if`,
/// so the walk stays terminating.
fn walk_conditional_uri(
    text: &str,
    tokens: &TokenTable,
    if_at: usize,
    configurations: &mut Vec<Configuration>,
    configuration_spans: &mut Vec<SourceRange>,
) -> usize {
    let mut i = if_at + 1; // past `if`
    let mut name = String::new();
    let mut value: Option<Box<str>> = None;
    let mut clean = true;

    if kind_at(tokens, i) == Some(TokenKind::OpenParen) {
        let close = tokens.matching_end(i);
        let interior_end = close.unwrap_or(i + 1);
        let mut j = i + 1;
        // dottedName: identifier ('.' identifier)* — a trailing or leading
        // period (a synthetic official name) leaves the clause malformed.
        let mut expect_name = true;
        while let Some(kind) = kind_at(tokens, j) {
            if expect_name && is_name_token(kind) {
                if !name.is_empty() {
                    name.push('.');
                }
                name.push_str(tokens.lexeme(text, j));
                expect_name = false;
                j += 1;
            } else if !expect_name && kind == TokenKind::Period {
                expect_name = true;
                j += 1;
            } else {
                break;
            }
        }
        clean &= !expect_name;
        // Optional `== value`: the same literal read as the candidate URI
        // (adjacent segments concatenate; interpolation stays shapeless).
        if kind_at(tokens, j) == Some(TokenKind::EqEq) {
            j += 1;
            let (read, _, value_next) = read_uri(text, tokens, j);
            match read {
                Some(decoded) => {
                    value = Some(decoded);
                    j = value_next;
                }
                None => clean = false,
            }
        }
        // Garbage between the condition and the real closer: the clause
        // shape is broken (the official reports and jumps to the group end).
        clean &= j == interior_end;
        i = close.map_or(interior_end, |end| end + 1);
    } else {
        // Missing `(`: the official's insertParens (:1102-1109) stands in an
        // EMPTY synthetic condition name — a clause that can never match a
        // declared variable — and the real tokens shift past the synthetic
        // `)` into the value/URI reads. Recording nothing here is the same
        // resolution outcome (primary) under the under-report bias.
        clean = false;
    }

    // The candidate URI after the group (official :1155).
    let (uri, span, next) = read_uri(text, tokens, i);
    if clean
        && !name.is_empty()
        && let Some(uri) = uri
    {
        configurations.push(Configuration::new(&name, value.as_deref(), &uri));
        configuration_spans.push(span);
    }
    next
}

/// `part 'u';` and `part of 'u' | a.b.c;`
fn walk_part(
    text: &str,
    tokens: &TokenTable,
    start: usize,
    _strings: &StringTable,
    out: &mut Vec<DirectiveFacts>,
) -> usize {
    if kind_at(tokens, start + 1) == Some(TokenKind::Keyword(Keyword::Of)) {
        if kind_at(tokens, start + 2) == Some(TokenKind::String) {
            let (uri, uri_span, mut i) = read_uri(text, tokens, start + 2);
            if kind_at(tokens, i) == Some(TokenKind::Semicolon) {
                i += 1;
            }
            out.push(DirectiveFacts {
                kind: DirectiveKind::PartOf,
                uri,
                uri_span,
                target: ResolvedTarget::Missing,
                code: None,
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
                name: None,
                configurations: Box::default(),
                configuration_spans: Box::default(),
            });
            return i;
        }
        let (name, end) = walk_dotted_name(text, tokens, start + 2);
        out.push(DirectiveFacts {
            kind: DirectiveKind::PartOf,
            uri: None,
            uri_span: SourceRange {
                offset: 0,
                length: 0,
            },
            target: ResolvedTarget::Missing,
            code: None,
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
            name,
            configurations: Box::default(),
            configuration_spans: Box::default(),
        });
        return end;
    }
    let (uri, uri_span, mut i) = read_uri(text, tokens, start + 1);
    if kind_at(tokens, i) == Some(TokenKind::Semicolon) {
        i += 1;
    }
    out.push(DirectiveFacts {
        kind: DirectiveKind::Part,
        uri,
        uri_span,
        target: ResolvedTarget::Missing,
        code: None,
        prefix: None,
        deferred: false,
        combinators: Combinators::unfiltered(),
        name: None,
        configurations: Box::default(),
        configuration_spans: Box::default(),
    });
    i
}

/// Strips quotes (raw and cooked, single/double, single/triple) from a
/// complete string-literal lexeme and processes escapes of cooked bodies.
/// Returns `None` for anything that is not one complete literal.
fn string_literal_value(literal: &str) -> Option<String> {
    let is_raw = literal.starts_with('r');
    let body = if is_raw { &literal[1..] } else { literal };
    let body = if let Some(rest) = body.strip_prefix("\"\"\"") {
        rest.strip_suffix("\"\"\"")?
    } else if let Some(rest) = body.strip_prefix("'''") {
        rest.strip_suffix("'''")?
    } else if let Some(rest) = body.strip_prefix('"') {
        rest.strip_suffix('"')?
    } else {
        body.strip_prefix('\'')?.strip_suffix('\'')?
    };
    if is_raw {
        return Some(body.to_owned());
    }
    unescape(body)
}

/// Dart cooked-string escape processing (`\n`-style short escapes, `\xHH`,
/// `\uHHHH`, `\u{H…}`, `\<char>` passthrough, and backslash-newline elision
/// in multiline bodies).
fn unescape(body: &str) -> Option<String> {
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        if c != '\\' {
            out.push(c);
            continue;
        }
        match *chars.get(i)? {
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            'f' => out.push('\u{0C}'),
            'b' => out.push('\u{08}'),
            't' => out.push('\t'),
            'v' => out.push('\u{0B}'),
            'x' => {
                let hi = *chars.get(i + 1)?;
                let lo = *chars.get(i + 2)?;
                let digits = format!("{hi}{lo}");
                i += 2;
                out.push(char::from_u32(u32::from_str_radix(&digits, 16).ok()?)?);
            }
            'u' => {
                i += 1;
                let digits = if chars.get(i) == Some(&'{') {
                    let start = i + 1;
                    let mut end = start;
                    while end < chars.len() && chars.get(end) != Some(&'}') {
                        end += 1;
                    }
                    if end == chars.len() {
                        return None;
                    }
                    i = end;
                    chars[start..end].iter().collect::<String>()
                } else {
                    if i + 4 > chars.len() {
                        return None;
                    }
                    let digits = chars[i..i + 4].iter().collect::<String>();
                    i += 3;
                    digits
                };
                out.push(char::from_u32(u32::from_str_radix(&digits, 16).ok()?)?);
            }
            '\n' => {}
            '\r' => {
                if chars.get(i + 1) == Some(&'\n') {
                    i += 1;
                }
            }
            other => out.push(other),
        }
        i += 1;
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Library-graph delta + cascade closure
// ---------------------------------------------------------------------------

/// The formation-relevant projection of one file's facts: the part flag plus
/// the raw resolved directive targets. Residency-independent — targets are
/// interned at facts-compute time whether or not they are resident — and
/// duplicate-keeping: one entry per directive occurrence, because
/// reverse-edge multiplicity is per occurrence.
struct UnitFace {
    part: bool,
    /// Targets of `part` directives, source order (the claim offers a
    /// defining file makes).
    part_targets: Vec<FileId>,
    /// Targets of `import`/`export` directives, source order (the reverse
    /// edges a file contributes).
    import_targets: Vec<FileId>,
    /// Targets of URI-form `part of` directives, source order (the claim
    /// side a part file makes; name-form part-ofs carry no URI and land
    /// nowhere). Only ever populated on part files.
    part_of_targets: Vec<FileId>,
}

impl UnitFace {
    fn of(facts: &UnitFacts) -> Self {
        let mut face = Self {
            part: facts.is_part(),
            part_targets: Vec::new(),
            import_targets: Vec::new(),
            part_of_targets: Vec::new(),
        };
        for directive in &facts.directives {
            if let ResolvedTarget::Resolved(target) = directive.target {
                match directive.kind {
                    DirectiveKind::Part => face.part_targets.push(target),
                    DirectiveKind::Import | DirectiveKind::Export => {
                        face.import_targets.push(target);
                    }
                    DirectiveKind::PartOf => face.part_of_targets.push(target),
                    DirectiveKind::Library => {}
                }
            }
        }
        face
    }
}

/// Re-resolves the directive faces of residents whose missing/existing
/// verdicts the batch's file-set move just flipped (#2100): the intake
/// resolution pass ran only over the batch's own files, so a resident
/// holding a directive into a created or removed file would otherwise keep
/// its pre-move `target`/`code` columns — and its URI diagnostic tail —
/// forever.
///
/// The holder set is read out of the maintained target→sources
/// projections ([`EngineState::import_sources`],
/// [`EngineState::part_offers`], [`EngineState::part_of_sources`]): a file
/// target interns its `FileId` at intake whether or not the file exists,
/// so the rows for the set-moving files name exactly the residents that
/// can relink — bounded by the live directive set, never a residency scan.
/// Both indexes still carry pre-batch faces here (the delta's `reindex`
/// runs below), which is precisely the holder set: batch members are
/// excluded (they re-resolved fresh at intake) and removals are gone from
/// residency.
///
/// Per holder the facts clone pays the URI-tail truncate +
/// [`resolve_unit`] rebuild — the same idiom the warm-start
/// materialization uses (one tail row per coded directive, after every
/// other diagnostic). A holder whose directives re-resolve identically
/// (a same-directory creation nobody imported) is not rewritten and not
/// reported. Returns the `(file, pre-relink facts)` pairs so the delta
/// below can fold them into the changed-file faces: a relink can move a
/// directive's resolved `FileId` (the generated-file fallback flips which
/// path a `package:` URI lands on), and only the paired face diff keeps
/// the indexes and reverse edges exact.
fn relink_directives(
    state: &mut EngineState,
    recomputed: &[RecomputedFacts],
    removed: &[FileId],
    old_facts: &FxHashMap<FileId, Arc<UnitFacts>>,
) -> Vec<(FileId, Arc<UnitFacts>)> {
    // The set-moving faces: files that entered residency this batch (no
    // previous facts) and every removal. A pure content edit moves no
    // existence verdict — short-circuit before touching the indexes.
    let set_moved = !removed.is_empty()
        || recomputed
            .iter()
            .any(|(file, _, _, _)| !old_facts.contains_key(file));
    if !set_moved {
        return Vec::new();
    }

    let changed_set: FxHashSet<FileId> =
        recomputed.iter().map(|(file, _, _, _)| *file).collect();
    let mut holders: FxHashSet<FileId> = FxHashSet::default();
    let mut gather = |state: &EngineState, target: FileId| {
        for &source in state
            .import_sources
            .row(target)
            .iter()
            .chain(state.part_offers.row(target))
            .chain(state.part_of_sources.row(target))
        {
            if !changed_set.contains(&source) && state.residency.contains_key(&source) {
                holders.insert(source);
            }
        }
    };
    for (file, _, _, _) in recomputed {
        if !old_facts.contains_key(file) {
            gather(state, *file);
        }
    }
    for file in removed {
        gather(state, *file);
    }
    if holders.is_empty() {
        return Vec::new();
    }

    // The post-batch world: residency already holds this batch (removals
    // left at the drain, changes admitted above) — the same existence face
    // the intake pass resolves against. The resolver borrow ends before
    // the residency rewrite below (field-disjoint: `files` mutates under
    // the resolver while the `resident` closure reads residency).
    let mut relinked: Vec<(FileId, Arc<UnitFacts>)> = Vec::new();
    {
        let resident = |id: FileId| state.residency.contains_key(&id);
        let mut resolver = UriResolver::new(
            &state.uri_packages,
            &state.sdk,
            &state.declared_variables,
            &mut state.files,
            &resident,
        );
        let mut rewritten: Vec<(FileId, UnitFacts)> = Vec::new();
        for holder in holders {
            let previous = Arc::clone(&state.residency[&holder]);
            let mut facts = (*previous).clone();
            // The URI diagnostic tail rebuilds one-for-one with the
            // re-resolved coded directives: `resolve_unit`'s construction
            // invariant is exactly one pushed row per coded directive,
            // after every other diagnostic — the warm-start slice.
            let uri_tail = facts
                .directives
                .iter()
                .filter(|directive| directive.code.is_some())
                .count();
            facts
                .diagnostics
                .truncate(facts.diagnostics.len() - uri_tail);
            resolve_unit(&mut facts, holder, &mut resolver, &state.declared_variables);
            let moved = facts
                .directives
                .iter()
                .zip(&previous.directives)
                .any(|(new, old)| (new.target, new.code) != (old.target, old.code));
            if moved {
                rewritten.push((holder, facts));
                relinked.push((holder, previous));
            }
        }
        // The resolver's `&mut FileTable` borrow ends before residency
        // writes; rewrite afterwards so the `resident` closure never
        // observes a half-relinked world (and the memos freeze it anyway).
        for (file, facts) in rewritten {
            state.admit_residency(file, Arc::new(facts));
        }
    }
    // Deterministic order: the hash gather visits rows unordered, and the
    // delta below consumes the list positionally — a sorted face keeps the
    // accumulator writes (and the outcome face) byte-stable.
    relinked.sort_unstable_by_key(|(file, _)| *file);
    relinked
}

/// Applies the identity-keyed library-graph delta for one batch and derives
/// the post-closure dirty set.
///
/// The changed set is known (the gate passed it), so maintenance touches
/// only what moved:
///
/// 1. The raw projection indexes ([`EngineState::part_offers`],
///    [`EngineState::import_sources`]) take the batch's paired
///    subtract/add per file face, and the affected claim-key set is
///    collected: the part files themselves when changed, plus every target a
///    changed or removed defining file offered (old AND new faces — a
///    library-name edit flips `may_claim` while the offer set stands).
/// 2. Claim resolution runs over the affected keys only, against the final
///    offer index. Resolution itself stays deterministic-total (the
///    path-smallest qualifying candidate), so restricting WHICH part files
///    resolve never forks precedence; ownership entries are written only
///    where the resolved owner differs.
/// 3. Libraries whose key file lost its tenant retire precisely
///    ([`LibraryTable::retire`]); their results and surfaces go with them.
/// 4. The reverse edges rewrite as one paired event per file — old face
///    subtracted, new face added, zero map operations when a stable owner
///    keeps an identical target set (the body-edit common case).
/// 5. Every touched library recomputes its [`LibraryInfo`] from current
///    facts and membership (the affected component's full recompute);
///    untouched libraries keep theirs byte-exact. Identity comparison of
///    old against new drives the dirty set, replacing the full rebuild's
///    workspace-wide result scan.
/// 6. Every identity-moved library's cached result drops (g485 W2): a
///    recorded run survives only while the identity that produced it
///    stands, which is the survival half of the post-closure reuse gate.
///
/// Formation semantics are unchanged from the full rebuild: defining files
/// own libraries keyed on their canonical URI, part files join the
/// path-smallest qualifying claimant or fall back to singletons, and every
/// resident file belongs to exactly one library. Returns the invalidated
/// set and whether any library was retired.
#[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
fn maintain_and_close(
    state: &mut EngineState,
    changed: &[(FileId, ContentId, SourceText)],
    removed: &[FileId],
    relinked: &[(FileId, Arc<UnitFacts>)],
    old_facts: &FxHashMap<FileId, Arc<UnitFacts>>,
    sig_changed: &[FileId],
) -> (Vec<LibraryId>, bool) {
    const NO_TARGETS: &[FileId] = &[];
    // Two distinct "changed" notions: `changed_set` is the own-event-ran
    // set — content-changed files plus relinked residents, whose paired
    // face events below re-drive ownership, edges, and claim resolution.
    // `content_changed` is the subset that actually re-parsed — the only
    // files whose membership moves may seed the importer closure (the
    // doctrine `close` documents: an unchanged file's claim flip never
    // seeds, and a relinked file is content-unchanged by construction).
    let content_changed: FxHashSet<FileId> =
        changed.iter().map(|(file, _, _)| *file).collect();
    let changed_set: FxHashSet<FileId> = content_changed
        .iter()
        .copied()
        .chain(relinked.iter().map(|(file, _)| *file))
        .collect();
    let removed_set: FxHashSet<FileId> = removed.iter().copied().collect();
    // Files that entered residency this batch — the same newness test the
    // relink pass uses (no `old_facts` entry means nothing to subtract).
    // `was_resident` below needs it to answer PRE-apply residency: the live
    // map at that point is post-batch, so an entry created this apply would
    // otherwise look like an edge that already existed, and the paired
    // delta would miss the add (the relinked-holder hole #2100: the holder's
    // own retarget saw the created target on both sides and never added
    // the reverse edge the entering-residency pass skips it for).
    let new_residents: FxHashSet<FileId> = changed
        .iter()
        .filter(|(file, _, _)| !old_facts.contains_key(file))
        .map(|(file, _, _)| *file)
        .collect();
    // Old and new faces. A file entering residency here has no old face —
    // nothing to subtract. Relinked residents join as changed faces too:
    // their pre-relink facts are the OLD face the delta subtracts (a
    // target FileId flip under the generated-file fallback moves real
    // edges; a pure code flip diffs empty and touches nothing).
    let mut changed_faces: Vec<(FileId, Option<UnitFace>, UnitFace)> = changed
        .iter()
        .map(|(file, _, _)| {
            let new = UnitFace::of(&state.residency[file]);
            let old = old_facts.get(file).map(|facts| UnitFace::of(facts));
            (*file, old, new)
        })
        .collect();
    changed_faces.extend(relinked.iter().map(|(file, old)| {
        (*file, Some(UnitFace::of(old)), UnitFace::of(&state.residency[file]))
    }));
    let removed_faces: Vec<(FileId, UnitFace)> = removed
        .iter()
        .map(|file| {
            let facts = old_facts
                .get(file)
                .expect("a removed file's previous facts were captured at the gate");
            (*file, UnitFace::of(facts))
        })
        .collect();

    // Phase 1 — projection-index deltas and the affected claim-key set.
    // Offers come only from defining files (claims are only ever made by
    // them); import sources from every file. Both indexes are raw, so the
    // maintenance is a pure face diff.
    let mut affected_parts: FxHashSet<FileId> = FxHashSet::default();
    for (file, old, new) in &changed_faces {
        let old_defining_offers: &[FileId] = match old {
            Some(face) if !face.part => &face.part_targets,
            _ => NO_TARGETS,
        };
        let new_defining_offers: &[FileId] = if new.part {
            NO_TARGETS
        } else {
            &new.part_targets
        };
        affected_parts.extend(old_defining_offers.iter().copied());
        affected_parts.extend(new_defining_offers.iter().copied());
        if old.as_ref().is_some_and(|face| face.part) || new.part {
            affected_parts.insert(*file);
        }
        reindex(
            &mut state.part_offers,
            *file,
            old_defining_offers,
            new_defining_offers,
        );
        let old_imports: &[FileId] = match old {
            Some(face) => &face.import_targets,
            None => NO_TARGETS,
        };
        reindex(
            &mut state.import_sources,
            *file,
            old_imports,
            &new.import_targets,
        );
        let old_part_ofs: &[FileId] = match old {
            Some(face) => &face.part_of_targets,
            None => NO_TARGETS,
        };
        reindex(
            &mut state.part_of_sources,
            *file,
            old_part_ofs,
            &new.part_of_targets,
        );
    }
    for (file, face) in &removed_faces {
        if !face.part {
            affected_parts.extend(face.part_targets.iter().copied());
            reindex(
                &mut state.part_offers,
                *file,
                &face.part_targets,
                NO_TARGETS,
            );
        }
        reindex(
            &mut state.import_sources,
            *file,
            &face.import_targets,
            NO_TARGETS,
        );
        reindex(
            &mut state.part_of_sources,
            *file,
            &face.part_of_targets,
            NO_TARGETS,
        );
    }

    // Phase 2 — ownership decisions. Resolution reads the FINAL offer index,
    // so every offer delta above has landed; owners mint through the arena's
    // URI keys, which keeps a defining file's library id stable across edits
    // of its content (the URI is path-derived). `library_keys` records the
    // key file of every interned id so freshly formed slots (no info yet)
    // know their defining file at the recompute.
    let mut library_keys: FxHashMap<LibraryId, FileId> = FxHashMap::default();
    let mut movers: Vec<(FileId, Option<LibraryId>, LibraryId)> = Vec::new();
    // The canonical URI is a pure function of the interned path and the
    // package table: compose every uncached changed file's spelling in
    // parallel up front, so the serial ownership loop below only reads the
    // memo (a cold walk otherwise composes 10^5 URIs on one thread).
    {
        let pending: Vec<(FileId, std::path::PathBuf)> = changed
            .iter()
            .map(|(file, _, _)| *file)
            .filter(|file| !state.canonical_uris.contains_key(file))
            .filter_map(|file| state.files.path(file).map(|path| (file, path.to_path_buf())))
            .collect();
        let packages = &state.uri_packages;
        let composed: Vec<(FileId, Arc<str>)> = pending
            .par_iter()
            .map(|(file, path)| {
                (
                    *file,
                    Arc::from(dartq_resolution::compose_canonical_uri(packages, path)),
                )
            })
            .collect();
        state.canonical_uris.extend(composed);
    }
    // Unchanged part files whose claim may have moved. Raw offers can name
    // defining files (they claim nothing) and removed files (nothing left to
    // resolve); both skip.
    for file in &affected_parts {
        let Some(facts) = state.residency.get(file) else {
            continue;
        };
        if changed_set.contains(file) || !facts.is_part() {
            continue;
        }
        let new_owner = match resolve_claim(state, *file) {
            Some(claimant) => intern_library_key(state, claimant, &mut library_keys),
            None => intern_library_key(state, *file, &mut library_keys),
        };
        let old_owner = state.library_of_file.get(file).copied();
        if old_owner != Some(new_owner) {
            state.library_of_file.insert(*file, new_owner);
            movers.push((*file, old_owner, new_owner));
        }
    }
    // Changed files take their formation owner directly; removed files leave
    // the membership map, their old owners riding on to retirement and the
    // closure seeds.
    let mut changed_owners: Vec<(FileId, Option<LibraryId>, LibraryId)> =
        Vec::with_capacity(changed_faces.len());
    for (file, _, new) in &changed_faces {
        let new_owner = if new.part {
            match resolve_claim(state, *file) {
                Some(claimant) => intern_library_key(state, claimant, &mut library_keys),
                None => intern_library_key(state, *file, &mut library_keys),
            }
        } else {
            intern_library_key(state, *file, &mut library_keys)
        };
        let old_owner = state.library_of_file.get(file).copied();
        if old_owner != Some(new_owner) {
            movers.push((*file, old_owner, new_owner));
        }
        state.library_of_file.insert(*file, new_owner);
        changed_owners.push((*file, old_owner, new_owner));
    }
    let mut removed_owners: Vec<Option<LibraryId>> = Vec::with_capacity(removed_faces.len());
    for (file, _) in &removed_faces {
        removed_owners.push(state.library_of_file.remove(file));
    }

    // Phase 3 — precise retirement: a library survives exactly when its key
    // file still hands formation its URI (resident, and owned by this
    // library — a defining key file that turned part either re-keys the same
    // URI as an orphan singleton or moved to a claimant; a removed one
    // leaves no tenant). All interns of the batch precede this, so a
    // recycled id cannot alias a same-batch formation (#374).
    let mut candidates: FxHashSet<LibraryId> = FxHashSet::default();
    for (_, old_owner, new_owner) in changed_owners.iter().chain(movers.iter()) {
        if Some(*new_owner) != *old_owner
            && let Some(owner) = old_owner
        {
            candidates.insert(*owner);
        }
    }
    for owner in removed_owners.iter().flatten() {
        candidates.insert(*owner);
    }
    let retired: Vec<LibraryId> = candidates
        .into_iter()
        .filter(|library| !library_survives(state, *library))
        .collect();
    for library in &retired {
        state.libraries.retire(*library);
        state.results.remove(library);
        state.clear_surface(*library);
    }

    // Phase 4 — the affected component set and the reverse-edge rewrite.
    // Every library whose membership, edges, or identity inputs the batch
    // touched recomputes; everything else keeps its exact state.
    let mut affected: FxHashSet<LibraryId> = FxHashSet::default();
    for (_, old_owner, new_owner) in changed_owners.iter().chain(movers.iter()) {
        if let Some(owner) = old_owner {
            affected.insert(*owner);
        }
        affected.insert(*new_owner);
    }
    for owner in removed_owners.iter().flatten() {
        affected.insert(*owner);
    }
    // A removed target's importers lose their dependency (the target is no
    // longer resident) — captured before the row drops.
    for (file, _) in &removed_faces {
        for edge in state.importers.row(*file) {
            affected.insert(edge.library);
        }
    }
    // A target whose owning library id moved flips the dependency column of
    // every library that imports it.
    for (file, old_owner, new_owner) in &movers {
        if old_owner.is_some() && changed_set.contains(file) {
            continue; // covered by the changed-file pass below
        }
        if *old_owner != Some(*new_owner) {
            for edge in state.importers.row(*file) {
                affected.insert(edge.library);
            }
        }
    }
    for (file, old_owner, new_owner) in &changed_owners {
        if *old_owner != Some(*new_owner) {
            for edge in state.importers.row(*file) {
                affected.insert(edge.library);
            }
        }
    }
    // A part target's residency flip moves the offering libraries'
    // part-target identity.
    for file in removed.iter().copied().chain(
        changed_faces
            .iter()
            .filter(|(_, old, _)| old.is_none())
            .map(|(file, _, _)| *file),
    ) {
        let offerers = state.part_offers.row(file);
        for &offerer in offerers {
            if let Some(&library) = state.library_of_file.get(&offerer) {
                affected.insert(library);
            }
        }
    }
    // A changed part file's membership spans every matching library, not
    // just the ownership winner: each offerer's library recomputes so the
    // name-form join re-reads the part's current facts into its content.
    // The Phase 5 identity comparison keeps a membership-stable library
    // byte-exact (no info write, no dirty), so the over-approximation costs
    // one identity compare per touched library and never forks the
    // invalidated set.
    for file in &affected_parts {
        let offerers = state.part_offers.row(*file);
        for &offerer in offerers {
            if let Some(&library) = state.library_of_file.get(&offerer) {
                affected.insert(library);
            }
        }
    }
    // A file entering residency gains the reverse edges of every unchanged
    // source that already pointed at it (the sources' own events carried
    // nothing — their directive faces never moved). Changed and mover
    // sources re-add their edges through their own events below. The movers'
    // ids as a set: membership is asked per unchanged source, and a linear
    // scan over the movers vector is quadratic on mass delete-recreate
    // batches (a local — nothing lingers past this loop's scope).
    let moved_set: FxHashSet<FileId> = movers.iter().map(|(moved, _, _)| *moved).collect();
    for (file, old, _) in &changed_faces {
        if old.is_some() {
            continue;
        }
        let sources = state.import_sources.row(*file);
        for &source in sources {
            if changed_set.contains(&source) || moved_set.contains(&source) {
                continue;
            }
            let Some(&owner) = state.library_of_file.get(&source) else {
                continue;
            };
            // The occurrence count reads the directives directly — the
            // exact same multiset `UnitFace::of(facts).import_targets`
            // carries for `file` (import/export resolved targets, source
            // order, duplicates kept; order is irrelevant to a count)
            // without materializing the face's two vectors per pair.
            let occurrences = state.residency.get(&source).map_or(0, |facts| {
                facts
                    .directives
                    .iter()
                    .filter(|directive| {
                        matches!(
                            directive.kind,
                            DirectiveKind::Import | DirectiveKind::Export
                        )
                    })
                    .filter_map(|directive| match directive.target {
                        ResolvedTarget::Resolved(target) => Some(target),
                        _ => None,
                    })
                    .filter(|target| target == file)
                    .count()
            });
            for _ in 0..occurrences {
                add_edge(&mut state.importers, *file, owner);
            }
            affected.insert(owner);
        }
    }
    // One paired edge event per file: the old face subtracted, the new face
    // added, multiplicity exact. Formation only ever created edges between
    // resident endpoints, so both sides filter to the same notion: an edge
    // can exist exactly for a target resident PRE-apply — the post-batch
    // map minus this batch's entrants, plus its removals — and a new edge
    // exactly for a target resident now. A stable owner over an identical
    // effective target set performs zero map operations.
    let was_resident = |target: FileId| {
        removed_set.contains(&target)
            || (state.residency.contains_key(&target) && !new_residents.contains(&target))
    };
    for ((_, old, new), (_, old_owner, new_owner)) in changed_faces.iter().zip(&changed_owners) {
        let old_imports: Vec<FileId> = match old {
            Some(face) => face
                .import_targets
                .iter()
                .copied()
                .filter(|target| was_resident(*target))
                .collect(),
            None => Vec::new(),
        };
        let new_imports: Vec<FileId> = new
            .import_targets
            .iter()
            .copied()
            .filter(|target| state.residency.contains_key(target))
            .collect();
        retarget_edges(
            &mut state.importers,
            *old_owner,
            &old_imports,
            *new_owner,
            &new_imports,
        );
    }
    // Removals all precede the entry drops: two removed files may import
    // each other, and each removal must see the other's edge still counted.
    for ((file, _), owner) in removed_faces.iter().zip(&removed_owners) {
        if let Some(owner) = owner {
            // The face is gone from residency; its old targets ride on the
            // captured old facts, gated to the pre-apply residency (an edge
            // to a never-resident target never existed).
            let targets = old_facts
                .get(file)
                .map(|facts| UnitFace::of(facts).import_targets)
                .unwrap_or_default();
            for target in targets {
                if was_resident(target) {
                    remove_edge(&mut state.importers, target, *owner);
                }
            }
        }
    }
    for (file, _) in &removed_faces {
        state.importers.remove_row(*file);
    }
    for (file, old_owner, new_owner) in &movers {
        if changed_set.contains(file) {
            continue; // the changed-file event above moved these edges
        }
        // An unchanged mover keeps its directive face; both sides carry the
        // same targets, filtered per side (a target created this batch had
        // no old edge, a removed one keeps no new edge), re-owned.
        let targets = UnitFace::of(&state.residency[file]).import_targets;
        let old_effective: Vec<FileId> = targets
            .iter()
            .copied()
            .filter(|target| was_resident(*target))
            .collect();
        let new_effective: Vec<FileId> = targets
            .iter()
            .copied()
            .filter(|target| state.residency.contains_key(target))
            .collect();
        retarget_edges(
            &mut state.importers,
            *old_owner,
            &old_effective,
            *new_owner,
            &new_effective,
        );
    }

    // Phase 5 — the affected components' full recompute: rebuild each
    // touched library's info from current facts and membership. The
    // old-vs-new identity comparison drives the dirty set, so the full
    // rebuild's workspace-wide result scan disappears; a library whose
    // identity stood is left byte-exact (no info write at all). Retired ids
    // skip: they left the live set this batch, and retirement and formation
    // are disjoint (candidates are pre-apply live ids, formed ids come from
    // fresh interns or the freelist).
    let retired_set: FxHashSet<LibraryId> = retired.iter().copied().collect();
    let mut identity_moved: FxHashSet<LibraryId> = FxHashSet::default();
    let mut targets_moved: FxHashSet<LibraryId> = FxHashSet::default();
    for library in &affected {
        if retired_set.contains(library) {
            continue;
        }
        let previous = state.libraries.info(*library).map(|info| {
            (
                info.defining,
                info.content.clone(),
                info.deps.clone(),
                info.part_targets.clone(),
            )
        });
        if let Some((defining, content, deps, part_targets)) = previous {
            let fresh = build_library_info(state, *library, defining);
            if fresh.part_targets != part_targets {
                targets_moved.insert(*library);
            }
            if fresh.content != content || fresh.deps != deps || fresh.part_targets != part_targets
            {
                identity_moved.insert(*library);
                state.libraries.set_info(*library, fresh);
            }
        } else {
            // Formed this batch (a fresh intern or a recycled slot): no
            // previous identity to compare — everything about it is new,
            // so it is dirty by construction.
            let defining = library_keys
                .get(library)
                .copied()
                .expect("an info-less live library was formed by this batch");
            let fresh = build_library_info(state, *library, defining);
            identity_moved.insert(*library);
            state.libraries.set_info(*library, fresh);
        }
    }
    // Relinked holders' owner libraries (#2100): a relink moves the
    // holder's OUTPUT face — the URI diagnostic tail — without necessarily
    // moving any folded identity input (a pure `code` flip folds into no
    // `LibraryInfo` column). The cached result still carries the pre-relink
    // rows, so the owner must be treated as identity-moved: drop the
    // recorded run and enter the dirty set. No `set_info` write — the
    // standing info is byte-correct; only the result's freshness broke.
    for (file, _) in relinked {
        if let Some(&owner) = state.library_of_file.get(file)
            && state.libraries.info(owner).is_some()
        {
            identity_moved.insert(owner);
        }
    }

    let invalidated = close(
        state,
        sig_changed,
        &movers,
        &content_changed,
        &removed_owners,
        &affected,
        &identity_moved,
        &targets_moved,
    );
    // The reuse-survival half of the fingerprint key (g485 W2): a cached
    // result stays reusable only while the library's identity has stood
    // since it was recorded, so every identity move drops the record
    // outright — the post-closure gate then never has to re-derive identity
    // freshness. Running after `close` keeps its result-presence tests (the
    // resultless touched channel, the targets_moved propagation gate)
    // reading the pre-drop facts they were specified against.
    for library in &identity_moved {
        state.results.remove(library);
    }
    (invalidated, !retired.is_empty())
}

/// The file's canonical URI, memoized ([`EngineState::canonical_uris`]): a
/// pure function of the immutable interned path, so a miss recomputes
/// exactly the string a previous residency held and no library identity can
/// fork across an eviction gap (audit #398 F10). `canonical_uri` never probes
/// existence (the state.rs face contract), so the resident filter is inert
/// — the documented no-probe closure.
fn canonical_uri_memo(state: &mut EngineState, file: FileId) -> Arc<str> {
    if let Some(uri) = state.canonical_uris.get(&file) {
        return Arc::clone(uri);
    }
    let path = state
        .files
        .path(file)
        .map_or_else(std::path::PathBuf::new, std::path::Path::to_path_buf);
    let uri = Arc::from(state.uri_resolver(&|_| false).canonical_uri(&path));
    state.canonical_uris.insert(file, Arc::clone(&uri));
    uri
}

/// Interns the library keyed on `file`'s canonical URI — its defining or
/// singleton formation key — recording the key file so a freshly formed
/// arena slot knows its defining file at the recompute.
fn intern_library_key(
    state: &mut EngineState,
    file: FileId,
    library_keys: &mut FxHashMap<LibraryId, FileId>,
) -> LibraryId {
    let uri = canonical_uri_memo(state, file);
    let id = state.libraries.intern(&uri);
    library_keys.entry(id).or_insert(file);
    id
}

/// Interned-path byte order — the claim-precedence total order (the oracle
/// picks the path-smallest claimant, file_state.dart:2176-2192). Stable
/// across the incremental and full arms because paths are the `FileTable`'s
/// own universe (#287): `FileId` interning order and canonical-URI bytes both
/// reorder under mid-sequence file creation and percent-encoding.
fn path_key(state: &EngineState, file: FileId) -> &[u8] {
    state
        .files
        .path(file)
        .map_or(&[][..], |path| path.as_os_str().as_encoded_bytes())
}

/// Claim resolution for one resident part file over the offer index: the
/// path-smallest defining file whose `may_claim` holds against current
/// facts. Deterministic-total over the candidate set, so restricting which
/// part files resolve (the affected key set) never forks precedence; two
/// candidates cannot tie because interned paths are unique per file.
fn resolve_claim(state: &EngineState, part: FileId) -> Option<FileId> {
    let mut winner: Option<FileId> = None;
    for &candidate in state.part_offers.row(part) {
        let library_name = declared_library_name(&state.residency[&candidate]);
        if may_claim(state, candidate, library_name, part)
            && winner.is_none_or(|current| path_key(state, candidate) < path_key(state, current))
        {
            winner = Some(candidate);
        }
    }
    winner
}

/// The one-library formation recomputed from current facts — the affected
/// component's full recompute. Membership follows the maintained ownership
/// map (a defining file's `part` targets claimed by this library, directive
/// order), content folds the member contents, and deps plus unclaimed part
/// targets scan the members' directives. The construction is the full
/// rebuild's, verbatim; the delta only decides which libraries run it.
fn build_library_info(
    state: &mut EngineState,
    library: LibraryId,
    defining: FileId,
) -> LibraryInfo {
    let uri = canonical_uri_memo(state, defining);
    let defining_facts = &state.residency[&defining];
    let library_name = declared_library_name(defining_facts);
    // Membership, not ownership: a name-form part joins EVERY library whose
    // declared name matches (`may_claim` is the per-library include test),
    // so a second equally-named library still sees the part's members in its
    // namespace. The path-smallest `library_of_file` winner stays the
    // attribution owner and is irrelevant here.
    //
    // The walk is breadth-first over each member's own part directives: a
    // depth-2 chain (`root ← mid ← leaf`, leaf's part-of binding to mid)
    // joins the leaf through the attached middle — the oracle's library
    // builds transitively (a `g()` declared in the leaf resolves bare in
    // the root), the URI-form claim testing each hop against its DIRECT
    // includer (`may_claim(state, member, …)`).
    let mut members: Vec<FileId> = Vec::new();
    let mut claimed: FxHashSet<FileId> = FxHashSet::default();
    members.push(defining);
    claimed.insert(defining);
    let mut hop = 0;
    while hop < members.len() {
        let member = members[hop];
        hop += 1;
        let mut joined: Vec<FileId> = Vec::new();
        for directive in &state.residency[&member].directives {
            if directive.kind != DirectiveKind::Part {
                continue;
            }
            let ResolvedTarget::Resolved(target) = directive.target else {
                continue;
            };
            if target == defining
                || !state.residency.contains_key(&target)
                || !claimed.insert(target)
            {
                continue;
            }
            if may_claim(state, member, library_name, target) {
                joined.push(target);
            } else {
                claimed.remove(&target);
            }
        }
        joined.sort_unstable_by_key(|file| path_key(state, *file));
        members.extend(joined);
    }
    let content: Box<[ContentId]> = members
        .iter()
        .map(|member| state.residency[member].content)
        .collect();
    let mut deps: Vec<LibraryId> = Vec::new();
    let mut part_targets: Vec<FileId> = Vec::new();
    for member in &members {
        for directive in &state.residency[member].directives {
            let ResolvedTarget::Resolved(target) = directive.target else {
                continue;
            };
            match directive.kind {
                DirectiveKind::Import | DirectiveKind::Export => {
                    if let Some(&dep) = state.library_of_file.get(&target)
                        && dep != library
                    {
                        deps.push(dep);
                    }
                }
                DirectiveKind::Part => {
                    // Part directives resolving to a resident file outside
                    // membership: neither content nor deps sees them, so
                    // their residency flip rides on the identity explicitly
                    // (removing such a target must re-bake
                    // `uri_does_not_exist` against the absent URI). The
                    // member test is the same per-library `may_claim` join
                    // the parts list uses, so a member never double-rides.
                    if state.residency.contains_key(&target)
                        && target != defining
                        && !claimed.contains(&target)
                    {
                        part_targets.push(target);
                    }
                }
                DirectiveKind::PartOf | DirectiveKind::Library => {}
            }
        }
    }
    deps.sort_unstable();
    deps.dedup();
    part_targets.sort_unstable_by_key(|file| file.index());
    part_targets.dedup();
    // `parts` is membership minus the defining file (`members` carries both).
    let parts: Vec<FileId> = members[1..].to_vec();
    LibraryInfo {
        // The memo's allocation, shared: a refcount per formed library,
        // never a copy of the URI bytes.
        uri,
        name: library_name.unwrap_or_default().into(),
        defining,
        parts,
        content,
        deps,
        part_targets: part_targets.into(),
    }
}

/// The tenancy test: the library's key file is resident and still owned by
/// this library — exactly the formation condition for its URI key (a
/// defining key, or the orphan-singleton key when the key file is an
/// unclaimed part).
fn library_survives(state: &EngineState, library: LibraryId) -> bool {
    let Some(info) = state.libraries.info(library) else {
        return false;
    };
    state.residency.contains_key(&info.defining)
        && state.library_of_file.get(&info.defining) == Some(&library)
}

/// Paired set-index maintenance (the raw offer/source projections): the
/// file's old target set subtracted, the new one added; an unchanged face
/// touches nothing. Empty rows drop, so the indexes stay bounded by the
/// live directive set. Rows stay sorted by [`FileId`]: every consumer either
/// collects the row wholesale or reduces over it, so the sorted order is a
/// strict upgrade on the old hash order, never a semantic input.
fn reindex(
    index: &mut TargetRows<FileId>,
    source: FileId,
    old_targets: &[FileId],
    new_targets: &[FileId],
) {
    if old_targets == new_targets {
        return;
    }
    let old_set: FxHashSet<FileId> = old_targets.iter().copied().collect();
    let new_set: FxHashSet<FileId> = new_targets.iter().copied().collect();
    for target in old_set.difference(&new_set) {
        index.edit_row(*target, |row| {
            if let Ok(position) = row.binary_search(&source) {
                row.remove(position);
            }
        });
    }
    for target in new_set.difference(&old_set) {
        index.edit_row(*target, |row| {
            if let Err(position) = row.binary_search(&source) {
                row.insert(position, source);
            }
        });
    }
}

/// One reverse-edge insert. The adjacency counts multiplicity: two members
/// of one library importing the same target is a count of two, so one of
/// them re-targeting away leaves the edge exact. Rows stay sorted by
/// [`LibraryId`].
fn add_edge(importers: &mut TargetRows<ImportEdge>, target: FileId, library: LibraryId) {
    importers.edit_row(target, |row| {
        match row.binary_search_by_key(&library, |edge| edge.library) {
            Ok(position) => row[position].count += 1,
            Err(position) => row.insert(position, ImportEdge { library, count: 1 }),
        }
    });
}

/// The paired remove: decrements the multiplicity and drops entries as they
/// empty, so no zero-count ghost survives. An absent edge is a delta
/// asymmetry — debug builds trap on it.
fn remove_edge(importers: &mut TargetRows<ImportEdge>, target: FileId, library: LibraryId) {
    debug_assert!(
        importers
            .row(target)
            .iter()
            .any(|edge| edge.library == library),
        "removing an absent reverse edge (delta asymmetry)"
    );
    importers.edit_row(target, |row| {
        if let Ok(position) = row.binary_search_by_key(&library, |edge| edge.library) {
            row[position].count -= 1;
            if row[position].count == 0 {
                row.remove(position);
            }
        }
    });
}

/// Rewrites one file's outgoing reverse edges from its old face to its new
/// face. A stable owner over an unchanged target set performs zero map
/// operations (the body-edit common case); an ownership move re-owns every
/// edge; otherwise the multiset difference moves only the changed
/// directives. Every removal is paired with the add that created the edge —
/// an asymmetry surfaces as a ghost edge, which the incremental-vs-full
/// oracle gate and the debug assert in [`remove_edge`] both catch.
fn retarget_edges(
    importers: &mut TargetRows<ImportEdge>,
    old_owner: Option<LibraryId>,
    old_targets: &[FileId],
    new_owner: LibraryId,
    new_targets: &[FileId],
) {
    match old_owner {
        Some(owner) if owner == new_owner => {
            if old_targets == new_targets {
                return;
            }
            let (dropped, added) = multiset_delta(old_targets, new_targets);
            for target in dropped {
                remove_edge(importers, target, owner);
            }
            for target in added {
                add_edge(importers, target, owner);
            }
        }
        owner => {
            if let Some(owner) = owner {
                for target in old_targets {
                    remove_edge(importers, *target, owner);
                }
            }
            for target in new_targets {
                add_edge(importers, *target, new_owner);
            }
        }
    }
}

/// Multiset difference per multiplicity: what `old` holds beyond `new`, and
/// vice versa (directive order is irrelevant to edge counts).
fn multiset_delta(old: &[FileId], new: &[FileId]) -> (Vec<FileId>, Vec<FileId>) {
    let count = |targets: &[FileId]| {
        let mut counts: FxHashMap<FileId, u32> = FxHashMap::default();
        for target in targets {
            *counts.entry(*target).or_insert(0) += 1;
        }
        counts
    };
    let mut new_counts = count(new);
    let mut dropped = Vec::new();
    for target in old {
        match new_counts.get_mut(target) {
            Some(count) if *count > 0 => *count -= 1,
            _ => dropped.push(*target),
        }
    }
    let mut old_counts = count(old);
    let mut added = Vec::new();
    for target in new {
        match old_counts.get_mut(target) {
            Some(count) if *count > 0 => *count -= 1,
            _ => added.push(*target),
        }
    }
    (dropped, added)
}

/// The name a defining file's `library` directive declares, `None` when the
/// directive is absent or unnamed — the oracle's `LibraryFileKind.name`
/// (file_state.dart:1831-1834). An unnamed library matches no name-form part.
pub(crate) fn declared_library_name(facts: &UnitFacts) -> Option<&str> {
    facts.directives.iter().find_map(|directive| {
        if directive.kind == DirectiveKind::Library {
            directive.name.as_deref().filter(|name| !name.is_empty())
        } else {
            None
        }
    })
}

/// Whether a library defined by `defining_file` includes the part file
/// `target` — the per-library include test that doubles as the claim
/// qualifier `resolve_claim` picks the path-smallest winner over, mirroring
/// the oracle's per-kind `isPartOf`:
///
/// * Name form — the includer's declared library name equals the part-of
///   name and both files live in the same package (analyzer
///   `PartOfNameFileKind::isPartOf`, file_state.dart:2222-2225; the analyzer
///   matches names globally over sibling-directory discovery; the M1 engine
///   bounds the scan to one package). Inclusion is per library: the element
///   layer links a name-matched part into EVERY matching library (upstream
///   Dart SDK source, pkg/analyzer/lib/src/dart/element/element.dart
///   `PartIncludeWithFile.includedPart` is per-library `isPartOf`, linked
///   per library by library_analyzer.dart `_buildPartInclude`) — the
///   path-smallest winner only picks the attribution owner, never an
///   exclusive membership.
/// * URI form — the part file's own `part of` URI must resolve back to the
///   defining file (`PartOfUriKnownFileKind::isPartOf`,
///   file_state.dart:2262-2265); the include alone is no proof. Exclusive by
///   construction: the URI resolves to exactly one defining file. An
///   unresolvable URI (`PartOfUriUnknownFileKind`, file_state.dart:2283-2291)
///   or a shapeless one (interpolated — no resolvable string) stays
///   unclaimed.
pub(crate) fn may_claim(
    state: &EngineState,
    defining_file: FileId,
    library_name: Option<&str>,
    target: FileId,
) -> bool {
    state.residency[&target]
        .part_of()
        .is_some_and(|part_of| match part_of.name.as_deref() {
            Some(name) => library_name == Some(name) && same_package(state, defining_file, target),
            None => part_of.target == ResolvedTarget::Resolved(defining_file),
        })
}

/// Whether both files carry the same package identity (or both none — the
/// bound keeps a part-of name from claiming across packages).
fn same_package(state: &EngineState, a: FileId, b: FileId) -> bool {
    let owner = |file: FileId| {
        state
            .files
            .path(file)
            .and_then(|path| state.packages.containing(path))
    };
    owner(a) == owner(b)
}

/// The post-closure dirty set:
/// touched libraries whose identity moved or that have no cached result,
/// plus the importer closure fanned out **only from the propagation seeds**
/// — the owners of `sig_changed` files, both live owners of every changed
/// file whose membership moved (the losing owner of a `part of` detach, the
/// claim transfer's winner), libraries with a cached result whose
/// part-target identity moved (a claim flip between competitor libraries,
/// or a part target's residency flip), and the live former owners of
/// removed files. From a seed the closure passes unconditionally through
/// every importer (the re-export chain D←L←M: M's face moves, L re-exports
/// it unchanged, D must still recompute); a library that is only
/// identity-stale (a body edit under a stable face) recomputes alone and
/// does not propagate. A retired former owner needs no seed: its direct
/// importers go stale by identity (the dropped dependency), and their own
/// faces did not move.
///
/// The seed categories are the full rebuild's, exactly — the closure input
/// defines the invalidated SET, which the oracle pins per step, so an extra
/// seed would over-dirty just as a missing one under-dirties. In particular
/// an unchanged part file's claim flip seeds only through the surviving
/// endpoints' part-target moves (a newly formed winner stays unseeded: its
/// importers dirty by identity and do not propagate), and a result-less
/// library never seeds.
#[expect(clippy::too_many_arguments)] // each argument is a distinct invalidation face; a bundling struct would only relocate the list
fn close(
    state: &mut EngineState,
    sig_changed: &[FileId],
    movers: &[(FileId, Option<LibraryId>, LibraryId)],
    // Content-changed files only — the membership-move seed gate's notion
    // of "changed". Relinked residents are content-unchanged and must not
    // widen it (a relinked file's claim flip seeds through the endpoints'
    // part-target moves, never through this filter).
    content_changed: &FxHashSet<FileId>,
    removed_owners: &[Option<LibraryId>],
    touched: &FxHashSet<LibraryId>,
    identity_moved: &FxHashSet<LibraryId>,
    targets_moved: &FxHashSet<LibraryId>,
) -> Vec<LibraryId> {
    let mut dirty: FxHashSet<LibraryId> = FxHashSet::default();
    let mut queued: FxHashSet<LibraryId> = FxHashSet::default();
    let mut work: Vec<LibraryId> = Vec::new();
    // Cancelled-analyze leftovers carry forward (#428): filtering on live
    // info is the post-delta live set. A leftover invalidated by a
    // dependency's face move is identity-fresh (the touched pass below would
    // drop it), and the retired ids among the leftovers must not re-enter —
    // analyze unwraps every queued id's info, and a freelist-recycled id
    // (#230) aliases a fresh tenant that, if it reached here at all, is
    // result-less and already dirty. Leftovers seed `dirty` only, never the
    // fan-out: the cancelled run's dirty set was the full transitive closure
    // of its own close, so every still-invalid importer is already among
    // them.
    for library in std::mem::take(&mut state.invalidated) {
        if state.libraries.info(library).is_some() {
            dirty.insert(library);
        }
    }
    // Untouched libraries keep their identity by the maintenance invariant,
    // so staleness is a touched-set question: moved identity, or no result
    // at all (a freshly formed library has nothing cached to reuse). The
    // pass is live-gated exactly like the full rebuild's iteration over
    // `libraries.live()`: a retired id sits in the touched set as a former
    // owner, its result dropped alongside its info at retirement — without
    // the gate its "resultless" shape would resurrect a dead id into the
    // dirty set, and analyze unwraps every queued id's info (#230).
    for library in touched {
        if state.libraries.info(*library).is_none() {
            continue;
        }
        if identity_moved.contains(library) || !state.results.contains_key(library) {
            dirty.insert(*library);
        }
    }
    // Signature-gated propagation seeds. Identity-stale seeds are already in
    // `dirty` by construction (a signature move implies a content move; a
    // membership move implies an identity move), so seeding only starts the
    // fan-out, deduplicated by `queued` so each library propagates once.
    // Retired owners are skipped: their importers went stale through the
    // dropped dependency instead.
    for file in sig_changed {
        if let Some(&library) = state.library_of_file.get(file)
            && state.libraries.info(library).is_some()
            && queued.insert(library)
        {
            work.push(library);
        }
        // A part's signature is part of EVERY member library's face, not
        // just the ownership winner's: the name-form membership links the
        // part into each matching library, so each including library fans
        // out. `may_claim` is the membership predicate, so a dangling
        // include's library carries no member face and stays unseeded —
        // the seed set stays exactly the full rebuild's recompute span.
        for &offerer in state.part_offers.row(*file) {
            let Some(&holder) = state.library_of_file.get(&offerer) else {
                continue;
            };
            let name = declared_library_name(&state.residency[&offerer]);
            if may_claim(state, offerer, name, *file)
                && state.libraries.info(holder).is_some()
                && queued.insert(holder)
            {
                work.push(holder);
            }
        }
    }
    // Membership-change seeds: changed files whose ownership moved (both
    // live owners), and libraries whose part-target identity moved — the
    // transfer shape, gated on a cached result exactly like the full
    // rebuild's pass over the live set.
    for (file, old_owner, new_owner) in movers {
        if !content_changed.contains(file) {
            continue;
        }
        for owner in [*old_owner, Some(*new_owner)].into_iter().flatten() {
            if state.libraries.info(owner).is_some() && queued.insert(owner) {
                work.push(owner);
            }
        }
    }
    for owner in removed_owners.iter().flatten().copied() {
        if state.libraries.info(owner).is_some() && queued.insert(owner) {
            work.push(owner);
        }
    }
    for library in targets_moved {
        if state.results.contains_key(library)
            && state.libraries.info(*library).is_some()
            && queued.insert(*library)
        {
            work.push(*library);
        }
    }
    while let Some(library) = work.pop() {
        let info = state.libraries.info(library).expect("seed is live");
        for file in std::iter::once(info.defining).chain(info.parts.iter().copied()) {
            for edge in state.importers.row(file) {
                dirty.insert(edge.library);
                if queued.insert(edge.library) {
                    work.push(edge.library);
                }
            }
        }
    }
    let mut invalidated: Vec<LibraryId> = dirty.into_iter().collect();
    invalidated.sort_unstable();
    invalidated
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use dartq_pub::{LockFile, PackageConfig, Pubspec, build_graph};
    use dartq_sdk::SdkSnapshot;

    use super::*;

    /// The `allowed_experiments.json` shape the fixture's synthetic SDK
    /// ships: an empty whitelist (the zero-behavior baseline), the known
    /// `enhanced-parts` grant (#708 P2-1), or that grant plus a fabricated
    /// future experiment — the version-skew degrade face (#1304).
    enum Whitelist {
        Empty,
        Known,
        Skewed,
    }

    /// A throwaway one-package workspace ("t") with a synthetic VM SDK —
    /// the in-module twin of `tests/common/mod.rs`, which lib tests cannot
    /// import. Shared with the pipeline module's stateful cache tests (the
    /// same in-module twin discipline). The analyzed files themselves are
    /// overlay-only (the `Engine::set_content` discipline); only the pub/SDK
    /// scaffolding touches disk.
    pub(crate) struct Fixture {
        pub(crate) root: PathBuf,
        config_dir: PathBuf,
        config: Arc<PackageConfig>,
        packages: IdentityTable,
    }

    impl Fixture {
        pub(crate) fn new() -> Self {
            Self::new_inner(Whitelist::Empty)
        }

        /// The same workspace with `enhanced-parts` granted to package `t`
        /// and to the SDK's default library set, at language version 3.7 —
        /// above the experiment's 3.6 release, so the grant sticks at the
        /// package version (audit #708 P2-1).
        fn new_with_grants() -> Self {
            Self::new_inner(Whitelist::Known)
        }

        /// The grants workspace whose whitelist also names a fabricated
        /// future experiment — the version-skew degrade face (#1304).
        fn new_with_skewed_grants() -> Self {
            Self::new_inner(Whitelist::Skewed)
        }

        fn new_inner(whitelist: Whitelist) -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let root = std::env::temp_dir().join(format!(
                "dartq-engine-l1-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&root);
            let (sdk_constraint, sdk_floor, config_version, experiments_json) = match whitelist {
                Whitelist::Empty => (
                    "^3.5.0",
                    "3.5.0",
                    "3.5",
                    r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
                ),
                Whitelist::Known => (
                    "^3.7.0",
                    "3.7.0",
                    "3.7",
                    r#"{"version": 1, "experimentSets": {"sdkExperiments": ["enhanced-parts"]}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {"t": {"experimentSet": "sdkExperiments"}}}"#,
                ),
                Whitelist::Skewed => (
                    "^3.7.0",
                    "3.7.0",
                    "3.7",
                    r#"{"version": 1, "experimentSets": {"sdkExperiments": ["enhanced-parts", "future-flag"]}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {"t": {"experimentSet": "sdkExperiments"}}}"#,
                ),
            };
            let write = |relative: &str, content: &str| {
                let path = root.join(relative);
                std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
                std::fs::write(path, content).expect("file");
            };
            write(
                "pubspec.yaml",
                &format!("name: t\nversion: 0.1.0\nenvironment:\n  sdk: {sdk_constraint}\n"),
            );
            write(
                "pubspec.lock",
                &format!("packages: {{}}\nsdks:\n  dart: \">={sdk_floor} <4.0.0\"\n"),
            );
            write(
                ".dart_tool/package_config.json",
                &format!(
                    r#"{{"configVersion": 2, "packages": [{{"name": "t", "rootUri": "../", "languageVersion": "{config_version}"}}], "generator": "pub"}}"#
                ),
            );
            write("sdk/version", "3.13.1\n");
            write(
                "sdk/lib/libraries.json",
                r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#,
            );
            write(
                "sdk/lib/_internal/allowed_experiments.json",
                experiments_json,
            );
            // Canonicalize before the pub faces resolve it (the #287
            // spelling universe — macOS /var vs /private/var).
            let root = root.canonicalize().expect("fixture root resolves");
            let config_dir = root.join(".dart_tool");
            let config = Arc::new(
                PackageConfig::parse(
                    &std::fs::read_to_string(config_dir.join("package_config.json"))
                        .expect("config"),
                )
                .expect("valid config"),
            );
            let lock = LockFile::parse(&format!(
                "packages: {{}}\nsdks:\n  dart: \">={sdk_floor} <4.0.0\"\n"
            ))
            .expect("valid lock");
            let root_pubspec = Pubspec::parse(&format!(
                "name: t\nversion: 0.1.0\nenvironment:\n  sdk: {sdk_constraint}\n"
            ))
            .expect("valid pubspec");
            let graph = build_graph(
                &config,
                &lock,
                &root_pubspec,
                &std::collections::BTreeMap::new(),
                None,
                &config_dir,
            )
            .expect("graph");
            let packages = IdentityTable::build(&config, &graph, &config_dir).expect("table");
            Self {
                root,
                config_dir,
                config,
                packages,
            }
        }

        pub(crate) fn state(&self) -> EngineState {
            self.state_with(FxHashMap::default())
        }

        /// The state over a custom declared-variables context — the
        /// conditional-import fork axis (#512).
        fn state_with(&self, declared: FxHashMap<String, String>) -> EngineState {
            EngineState::new(
                self.packages.clone(),
                &self.config,
                &self.config_dir,
                SdkSnapshot::load(&self.root.join("sdk"), None).expect("synthetic SDK loads"),
                crate::memory_store(),
                "vm".to_owned(),
                declared,
                &[],
            )
            .expect("engine state")
        }
    }

    /// Enqueues a content overlay (the `Engine::set_content` discipline:
    /// intern, append to the order on first sight, last write wins).
    pub(crate) fn set(state: &mut EngineState, path: &Path, text: &str) -> FileId {
        let file = state.files.intern(path);
        if !state.pending.contains_key(&file) {
            state.pending_order.push(file);
        }
        state
            .pending
            .insert(file, PendingChange::Set(SourceText::owned(text.to_owned())));
        file
    }

    /// Enqueues a removal (the `Engine::remove_file` discipline).
    fn remove(state: &mut EngineState, file: FileId) {
        if !state.pending.contains_key(&file) {
            state.pending_order.push(file);
        }
        state.pending.insert(file, PendingChange::Remove);
    }

    /// Claim precedence (audit #398 F7): two libraries `part`-including the
    /// same name-form part file race on the Pass A order. The claimants are
    /// chosen so canonical-URI byte order disagrees with interned-path byte
    /// order (`a b.dart` vs `a!b.dart`: space sorts below `!` as a path, but
    /// its `%20` sorts above it in a URI) — the winner must be the
    /// path-smaller library, the oracle's path-smallest-claimant rule
    /// (file_state.dart:2176-2192).
    #[test]
    fn ambiguous_part_claim_follows_path_order_not_canonical_uri_order() {
        let fixture = Fixture::new();
        let mut state = fixture.state();
        let lib = fixture.root.join("lib");
        let claimant = |state: &mut EngineState, name: &str| {
            set(state, &lib.join(name), "library dup;\npart 'p.dart';\n")
        };
        let spaced = claimant(&mut state, "a b.dart");
        let banged = claimant(&mut state, "a!b.dart");
        let part = set(&mut state, &lib.join("p.dart"), "part of dup;\n");
        apply(&mut state, &mut |_| {});

        // Precondition: the two orders genuinely disagree, otherwise this
        // test pins nothing.
        let path_bytes = |file: FileId| {
            state.files.path(file).map_or(Vec::new(), |path| {
                path.as_os_str().as_encoded_bytes().to_vec()
            })
        };
        assert!(
            path_bytes(spaced) < path_bytes(banged),
            "space < '!' as path bytes"
        );
        assert!(
            state.canonical_uris[&banged] < state.canonical_uris[&spaced],
            "canonical-URI order must disagree with path order for the pin to bite"
        );

        let lib_spaced = state.library_of_file[&spaced];
        let lib_banged = state.library_of_file[&banged];
        assert_ne!(
            lib_spaced, lib_banged,
            "two defining libraries stay distinct"
        );
        assert_eq!(
            state.library_of_file[&part], lib_spaced,
            "the path-smaller claimant wins the ambiguous part"
        );
        assert_eq!(
            state
                .libraries
                .info(lib_spaced)
                .expect("live")
                .parts
                .as_slice(),
            &[part]
        );
    }

    /// Name-form membership is per-library, not the exclusive claim: two
    /// libraries declaring the same name both carry the part in their member
    /// sets (the oracle's element layer links a name-matched part into every
    /// matching library — upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/element/element.dart
    /// `PartIncludeWithFile.includedPart`, linked per library by
    /// library_analyzer.dart `_buildPartInclude`), so each library's
    /// namespace sees the part's members. The attribution ownership entry
    /// stays the path-smallest claimant's.
    #[test]
    fn name_form_part_joins_every_matching_library() {
        let fixture = Fixture::new();
        let mut state = fixture.state();
        let lib = fixture.root.join("lib");
        let first = set(
            &mut state,
            &lib.join("n1.dart"),
            "library dup;\npart 'p.dart';\nint f() => 1;\n",
        );
        let second = set(
            &mut state,
            &lib.join("n2.dart"),
            "library dup;\npart 'p.dart';\nint h() => 1;\n",
        );
        let part = set(
            &mut state,
            &lib.join("p.dart"),
            "part of dup;\nint g() => 1;\n",
        );
        apply(&mut state, &mut |_| {});

        let lib_first = state.library_of_file[&first];
        let lib_second = state.library_of_file[&second];
        assert_ne!(
            lib_first, lib_second,
            "two defining libraries stay distinct"
        );
        assert_eq!(
            state.library_of_file[&part], lib_first,
            "the path-smallest claimant stays the attribution owner"
        );
        for library in [lib_first, lib_second] {
            assert_eq!(
                state
                    .libraries
                    .info(library)
                    .expect("live")
                    .parts
                    .as_slice(),
                &[part],
                "every matching library includes the part as a member"
            );
        }
    }

    /// Conditional imports fork end-to-end (#512): the diet walk captures
    /// the `if` clauses, resolution selects the first clause the context's
    /// declared variables match (source order), and only the SELECTED
    /// candidate is read — an unmatched branch's missing target stays
    /// silent, a selected one is diagnosed at its own literal.
    #[test]
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    fn conditional_import_forks_by_declared_variables() {
        let fixture = Fixture::new();
        let lib = fixture.root.join("lib");
        let source = "import 'a.dart' if (dart.library.io) 'b.dart' if (x == 'y') 'c.dart';\n";
        let c_offset = source.find("'c.dart'").expect("candidate literal");

        // The oracle-default empty context: every condition misses, the
        // primary URI resolves, no URI diagnostic fires.
        let mut state = fixture.state_with(FxHashMap::default());
        let main = set(&mut state, &lib.join("main.dart"), source);
        let a = set(&mut state, &lib.join("a.dart"), "library a;\n");
        apply(&mut state, &mut |_| {});
        let facts = &state.residency[&main];
        assert_eq!(facts.directives[0].target, ResolvedTarget::Resolved(a));
        assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);

        // `dart.library.io` declared: the FIRST matching clause wins in
        // source order (the later `x == 'y'` matches too) and the branch
        // target forks away from the primary — the daemon's multi-context
        // fork observability.
        let declared: FxHashMap<String, String> = [("dart.library.io", "true"), ("x", "y")]
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect();
        let mut state = fixture.state_with(declared);
        let main = set(&mut state, &lib.join("main.dart"), source);
        let b = set(&mut state, &lib.join("b.dart"), "library b;\n");
        apply(&mut state, &mut |_| {});
        let facts = &state.residency[&main];
        assert_eq!(facts.directives[0].target, ResolvedTarget::Resolved(b));
        // The unmatched `c.dart` is absent from disk — and silent: only the
        // selected URI is ever read (library_analyzer.dart:698-717).
        assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);

        // Declared but non-matching values (`io=false` misses the bare
        // condition's exact `"true"` test): the second clause selects the
        // missing `c.dart`. A file target lands on its interned id with the
        // missing-target code riding the `code` column — `ResolvedTarget`
        // is `Missing` only for the non-file faces (dart:/other-scheme/
        // undecodable) — and the `uri_does_not_exist` diagnostic
        // attributes to the branch's own literal, not the primary's.
        let declared: FxHashMap<String, String> = [("dart.library.io", "false"), ("x", "y")]
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect();
        let mut state = fixture.state_with(declared);
        let main = set(&mut state, &lib.join("main.dart"), source);
        apply(&mut state, &mut |_| {});
        let facts = &state.residency[&main];
        let c = state
            .files
            .try_file_id(&lib.join("c.dart"))
            .expect("the selected branch target is interned by resolution");
        assert_eq!(facts.directives[0].target, ResolvedTarget::Resolved(c));
        let diagnostic = &facts.diagnostics[0];
        assert_eq!(diagnostic.code.name, "uri_does_not_exist");
        assert!(
            diagnostic.message.contains("'c.dart'"),
            "{}",
            diagnostic.message
        );
        assert_eq!(diagnostic.range.offset, c_offset as u32);
        assert_eq!(diagnostic.range.length, "'c.dart'".len() as u32);
    }

    /// The parser's registry-verbatim correctionMessage rides through the
    /// `DiagCollector` into engine diagnostics (#727): the `base_enum`
    /// modifier row carries "Try removing the keyword 'base'." unmodified.
    #[test]
    fn parse_diagnostic_carries_the_registry_correction() {
        let fixture = Fixture::new();
        let mut state = fixture.state();
        let main = set(
            &mut state,
            &fixture.root.join("lib/main.dart"),
            "base enum E { a }\n",
        );
        apply(&mut state, &mut |_| {});
        let facts = &state.residency[&main];
        let diagnostic = facts
            .diagnostics
            .iter()
            .find(|d| d.code.name == "base_enum")
            .expect("the base modifier row fires");
        assert_eq!(diagnostic.message, "Enums can't be declared to be 'base'.");
        assert_eq!(
            diagnostic.correction.as_deref(),
            Some("Try removing the keyword 'base'."),
        );
    }

    /// The feature-gate face renders the parser's bare
    /// `handle_experiment_not_enabled` payload (#908): the fixture package
    /// sits at 3.5, below dot-shorthands' 3.10, so a `.a` shorthand reports
    /// the shipped-feature wording once, anchored at the dot (the analyzer's
    /// deduped head+context anchor; probed 3.13.1 against a `// @dart=3.9`
    /// file — same selection, `getExperimentNotEnabledMessage`).
    #[test]
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    fn experiment_gate_reports_at_the_shorthand_dot() {
        let fixture = Fixture::new();
        let mut state = fixture.state();
        let source = "enum E { a }\nvoid main() { var x = E.a ==.a; }\n";
        let main = set(&mut state, &fixture.root.join("lib/main.dart"), source);
        apply(&mut state, &mut |_| {});
        let facts = &state.residency[&main];
        let dot = source.rfind(".a").expect("shorthand dot") as u32;
        let gate = facts
            .diagnostics
            .iter()
            .find(|d| d.code.name == "experiment_not_enabled")
            .expect("the dot-shorthands gate fires at 3.5");
        assert_eq!(
            gate.message,
            "This requires the 'dot-shorthands' language feature to be enabled."
        );
        assert_eq!(gate.range.offset, dot);
        assert_eq!(gate.range.length, 1);
    }

    /// Bounded canonical-URI memo (audit #398 F10): a file leaving residency
    /// drops its memo; a returning file recomputes the identical string from
    /// its immutable interned path, so the memo tracks the resident set
    /// instead of growing monotonically.
    #[test]
    fn canonical_uri_memo_evicts_with_residency_and_recomputes_identically() {
        let fixture = Fixture::new();
        let mut state = fixture.state();
        let lib = fixture.root.join("lib");
        let main = set(&mut state, &lib.join("main.dart"), "void main() {}\n");
        let helper = set(&mut state, &lib.join("helper.dart"), "int helper() => 1;\n");
        let extra = set(&mut state, &lib.join("extra.dart"), "int extra() => 2;\n");
        apply(&mut state, &mut |_| {});
        assert_eq!(state.canonical_uris.len(), state.residency.len());
        let extra_uri = state.canonical_uris[&extra].clone();

        remove(&mut state, extra);
        apply(&mut state, &mut |_| {});
        assert!(
            !state.canonical_uris.contains_key(&extra),
            "memo evicted at residency exit"
        );
        assert_eq!(state.canonical_uris.len(), state.residency.len());
        assert!(state.canonical_uris.contains_key(&main));
        assert!(state.canonical_uris.contains_key(&helper));

        // The returning file's memo is byte-identical to the evicted one:
        // library identities keyed on it must not fork across the gap.
        set(&mut state, &lib.join("extra.dart"), "int extra() => 3;\n");
        apply(&mut state, &mut |_| {});
        assert_eq!(state.canonical_uris[&extra], extra_uri);
        assert_eq!(state.canonical_uris.len(), state.residency.len());
    }

    /// The directive face framing (audit #398 F12): per lexeme, a
    /// little-endian `u64` byte length followed by the lexeme's UTF-8 bytes
    /// — self-delimiting, injective over the lexeme sequence.
    #[test]
    fn directive_face_is_length_prefixed() {
        let text = "import 'a.dart';\n";
        let fixture = Fixture::new();
        let mut state = fixture.state();
        let file = state.files.intern(Path::new("/face.dart"));
        let grants = state.grants();
        let (_, _, scanned, warnings) = scan_unit(text, DEFAULT_LANGUAGE_VERSION, file, &grants);
        assert!(warnings.is_empty());
        let strings = StringTable::new();
        let (_, region_end) = extract_directives(text, &scanned.tokens, &strings);
        let face = directive_face(text, &scanned.tokens, region_end);

        let mut expected = Vec::new();
        for lexeme in ["import", "'a.dart'", ";"] {
            expected.extend_from_slice(&(lexeme.len() as u64).to_le_bytes());
            expected.extend_from_slice(lexeme.as_bytes());
        }
        assert_eq!(face, expected);
    }

    /// The duplicate bake carries the oracle's first-definition context
    /// row: the capture-grade resolver ([`capture_first_decl`]) anchors at
    /// the kept declaration's name token.
    #[test]
    fn duplicate_bake_carries_the_first_definition_context() {
        let text = "var x = 0;\nvar x = 1;\n";
        let fixture = Fixture::new();
        let mut state = fixture.state();
        let file = state.files.intern(Path::new("/dup.dart"));
        let grants = state.grants();
        let strings = StringTable::new();
        let (feature_set, _, scanned, _) = scan_unit(text, DEFAULT_LANGUAGE_VERSION, file, &grants);
        let features = ParserFeatures::from_feature_set(feature_set);
        let mut decl = dartq_semantic::DeclarationCollector::new(text, &scanned.tokens, &strings);
        let capture = dartq_index::capture(file, text, &scanned.tokens, features, &strings);
        Parser::new(text, &scanned.tokens, features, &mut decl).parse_unit();
        let elements = decl.finish();
        let rows = duplicate_diagnostics(file, &elements, &strings, &capture.index);
        assert_eq!(rows.len(), 1);
        let context = rows[0]
            .context
            .as_ref()
            .expect("the first-definition context row");
        assert_eq!(&*context[0].message, "The first definition of this name.");
        assert_eq!(
            context[0].range.offset,
            u32::try_from("var ".len()).unwrap()
        );
        assert_eq!(context[0].range.length, 1);
    }

    /// A raw U+0001 inside a directive's string literal (legal source — the
    /// scanner folds control bytes into the string body) must stay framed by
    /// its length prefix: the face is not the retired `\u{1}`-separated join,
    /// which such a lexeme could alias into another token sequence.
    #[test]
    fn control_character_lexeme_does_not_alias_the_separator_join() {
        let text = "import 'a\u{1}b.dart';\n";
        let fixture = Fixture::new();
        let mut state = fixture.state();
        let file = state.files.intern(Path::new("/face.dart"));
        let grants = state.grants();
        let (_, _, scanned, _) = scan_unit(text, DEFAULT_LANGUAGE_VERSION, file, &grants);
        let strings = StringTable::new();
        let (_, region_end) = extract_directives(text, &scanned.tokens, &strings);
        let face = directive_face(text, &scanned.tokens, region_end);

        let retired = "import\u{1}'a\u{1}b.dart'\u{1};\u{1}".as_bytes().to_vec();
        assert_ne!(face, retired, "the separator join is retired");

        let mut expected = Vec::new();
        for lexeme in ["import", "'a\u{1}b.dart'", ";"] {
            expected.extend_from_slice(&(lexeme.len() as u64).to_le_bytes());
            expected.extend_from_slice(lexeme.as_bytes());
        }
        assert_eq!(
            face, expected,
            "the raw byte rides inside its framed lexeme"
        );
    }

    /// Allowed-experiment grants merge into the per-file feature set (audit
    /// #708 P2-1; oracle `feature_set_provider.dart:58-77`): the package's
    /// whitelist rides the version baseline, an SDK-owned file reads its
    /// library's grant even under a package root, an unattributed file
    /// stays on the plain baseline, the memo returns the identical set on
    /// repeat derivation (and the facts key carries the granted
    /// fingerprint), and a `// @dart` override re-gates the same grants at
    /// the override version (`scanner.dart:127`) instead of dropping them.
    #[test]
    fn experiment_grants_merge_and_memoize_identity() {
        let fixture = Fixture::new_with_grants();
        let mut state = fixture.state();
        let file = set(
            &mut state,
            &fixture.root.join("lib/granted.dart"),
            "var x = 1;\n",
        );
        // Under the SDK tree but inside the workspace root: the dart: branch
        // of the oracle wins over the package walk.
        let sdk_file = set(
            &mut state,
            &fixture.root.join("sdk/lib/core/bool.dart"),
            "var s = 1;\n",
        );
        // Outside every package: the unattributed baseline.
        let outside = set(
            &mut state,
            &fixture.root.parent().expect("temp parent").join("out.dart"),
            "var o = 1;\n",
        );
        apply(&mut state, &mut |_| {});

        let version = language_version_for(&state.packages, &state.files, file);
        assert_eq!(version, LanguageVersion { major: 3, minor: 7 });
        let merged = state.grants().granted(file, version);
        assert!(
            merged.is_enabled(Feature::EnhancedParts),
            "the package grant sticks at 3.7 (release 3.6)"
        );

        let sdk_version = language_version_for(&state.packages, &state.files, sdk_file);
        let sdk_features = state.grants().granted(sdk_file, sdk_version);
        assert!(
            sdk_features.is_enabled(Feature::EnhancedParts),
            "the sdk-default grant reaches an SDK library file"
        );

        let outside_features = state.grants().granted(outside, DEFAULT_LANGUAGE_VERSION);
        assert!(
            !outside_features.is_enabled(Feature::EnhancedParts),
            "no owner, no grant"
        );

        // Memo identity: the repeat derivation of the same (owner, version)
        // is the identical set.
        assert_eq!(state.grants().granted(file, version), merged);

        // The facts key carries the granted fingerprint, so the cached and
        // re-derived faces cannot alias across the lanes.
        let content = state.residency[&file].content;
        let key = facts_key(&state.grants(), file, content);
        assert_eq!(key.feature_fingerprint, merged.fingerprint());

        // The override re-gates the grants (`restrictToVersion`), it does
        // not re-derive a bare baseline: enabled above the release, and a
        // pin below it lands exactly on the plain pinned baseline.
        let (upgraded, _, _, warnings) = scan_unit(
            "// @dart = 3.8\nvar y = 2;\n",
            version,
            file,
            &state.grants(),
        );
        assert!(warnings.is_empty());
        assert!(upgraded.is_enabled(Feature::EnhancedParts));
        let (pinned, _, _, _) = scan_unit(
            "// @dart = 3.5\nvar z = 3;\n",
            version,
            file,
            &state.grants(),
        );
        assert_eq!(
            pinned.fingerprint(),
            FeatureSet::for_language_version(LanguageVersion { major: 3, minor: 5 }).fingerprint(),
            "pinned below the 3.6 release: the plain 3.5 baseline"
        );
        assert_ne!(
            upgraded.fingerprint(),
            pinned.fingerprint(),
            "the grant re-gates at the override version"
        );
    }

    /// The version-skew degrade (#1304): a whitelist naming a fabricated
    /// future experiment keeps the lenient grant — no abort, the known
    /// experiment sticks (#1293's posture) — and reports exactly one
    /// `ExperimentDegrade` carrying the name, the SDK version, and the
    /// known-only posture, once across both owners that derived the skewed
    /// whitelist; a second apply re-deriving both owners over fresh
    /// content emits nothing.
    #[test]
    fn unknown_experiment_degrades_with_one_event() {
        let fixture = Fixture::new_with_skewed_grants();
        let mut state = fixture.state();
        let package_file = set(
            &mut state,
            &fixture.root.join("lib/granted.dart"),
            "var x = 1;\n",
        );
        let _sdk_file = set(
            &mut state,
            &fixture.root.join("sdk/lib/core/bool.dart"),
            "var s = 1;\n",
        );
        let mut events = Vec::new();
        apply(&mut state, &mut |event| events.push(event));

        // (a) The lenient grant stands: the known experiment sticks at the
        // package version, the unknown one is skipped without an abort.
        let version = language_version_for(&state.packages, &state.files, package_file);
        assert!(
            state
                .grants()
                .granted(package_file, version)
                .is_enabled(Feature::EnhancedParts),
            "the known grant sticks under the skew"
        );

        // (b) Exactly one event per unknown name, once across both owners
        // and the whole run.
        let skews: Vec<(String, Vec<String>, &str)> = events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::ExperimentDegrade {
                    sdk_version,
                    unknown,
                    granted,
                } => Some((sdk_version.clone(), unknown.clone(), *granted)),
                _ => None,
            })
            .collect();
        assert_eq!(skews.len(), 1, "one degrade per unknown name: {events:?}");
        assert_eq!(skews[0].0, "3.13.1", "the SDK version that named it");
        assert_eq!(skews[0].1, ["future-flag".to_owned()]);
        assert_eq!(skews[0].2, KNOWN_ONLY_GRANT);

        // The second analysis re-derives both owners over fresh content;
        // the memo plus the report set keep it silent.
        let mut again = Vec::new();
        set(
            &mut state,
            &fixture.root.join("lib/other.dart"),
            "var y = 2;\n",
        );
        set(
            &mut state,
            &fixture.root.join("sdk/lib/core/int.dart"),
            "var t = 3;\n",
        );
        apply(&mut state, &mut |event| again.push(event));
        assert!(
            again
                .iter()
                .all(|event| !matches!(event, EngineEvent::ExperimentDegrade { .. })),
            "no duplicate degrade: {again:?}"
        );
    }

    /// The no-grant fixture stays exactly on the version baseline — the
    /// zero-behavior-change guarantee the empty-whitelist differential
    /// depends on.
    #[test]
    fn empty_grants_leave_the_plain_baseline() {
        let fixture = Fixture::new();
        let mut state = fixture.state();
        let file = set(&mut state, &fixture.root.join("lib/a.dart"), "var x = 1;\n");
        apply(&mut state, &mut |_| {});
        let version = language_version_for(&state.packages, &state.files, file);
        let granted = state.grants().granted(file, version);
        assert_eq!(
            granted.fingerprint(),
            FeatureSet::for_language_version(version).fingerprint(),
            "no grant set anywhere in the fixture: baseline byte-for-byte"
        );
    }

    /// The settled per-file face reads back through the accessor (#722
    /// seam): a `// @dart = 2.19` library inside the 3.x package settles to
    /// the pinned baseline (patterns closed) while its unpinned neighbor
    /// keeps the package posture, and a not-yet-resident file answers
    /// `None` — the commands layer's fallback signal.
    #[test]
    fn at_dart_override_settles_the_per_file_accessor() {
        let fixture = Fixture::new();
        let mut state = fixture.state();
        let lib = fixture.root.join("lib");
        let pinned = set(
            &mut state,
            &lib.join("pinned.dart"),
            "// @dart = 2.19\nvoid f() {}\n",
        );
        let modern = set(&mut state, &lib.join("modern.dart"), "void g() {}\n");
        assert!(
            state.settled_face(pinned).is_none(),
            "interned but not resident: nothing settled yet"
        );
        apply(&mut state, &mut |_| {});

        let settled = state
            .settled_face(pinned)
            .expect("a resident file has a persisted face");
        assert!(
            !settled.features.is_enabled(Feature::Patterns),
            "the 2.19 pin closes patterns inside the 3.x package"
        );
        assert_eq!(
            settled.features.fingerprint(),
            FeatureSet::for_language_version(LanguageVersion {
                major: 2,
                minor: 19
            })
            .fingerprint(),
            "no grants in this fixture: the plain pinned baseline"
        );
        let baseline = state
            .settled_face(modern)
            .expect("a resident file has a persisted face");
        assert!(
            baseline.features.is_enabled(Feature::Patterns),
            "the unpinned neighbor keeps the package baseline"
        );
    }

    /// The official override floor (`analysis_options.dart:183-185`,
    /// `>= 2.12.0`; upstream test `illegal_language_version_override_test.
    /// dart:29-34`): a below-floor `// @dart` settles to the written version
    /// — a verifier row, not a scanner drop (`error_verifier.dart:5787-5809`)
    /// — and rides the `illegal_language_version_override` ERROR at the
    /// comment lexeme; 2.12 and above stay clean.
    #[test]
    fn below_floor_override_settles_and_reports() {
        let text = "// @dart = 2.9\nint a = 0;\n";
        let fixture = Fixture::new();
        let mut state = fixture.state();
        let file = state.files.intern(Path::new("/floor.dart"));
        let grants = state.grants();
        let (features, settled, _, warnings) =
            scan_unit(text, DEFAULT_LANGUAGE_VERSION, file, &grants);
        assert_eq!(settled, LanguageVersion::new(2, 9));
        assert_eq!(
            features.fingerprint(),
            FeatureSet::for_language_version(LanguageVersion::new(2, 9)).fingerprint(),
            "the override stays effective: features re-derive at the written version"
        );
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code.name, "illegal_language_version_override");
        assert_eq!(warnings[0].severity, Severity::Error);
        assert_eq!(warnings[0].range.offset, 0);
        assert_eq!(warnings[0].range.length, 14);
        assert_eq!(
            warnings[0].message,
            "The language version must be >=2.12.0."
        );
        assert_eq!(
            warnings[0].correction.as_deref(),
            Some("Try removing the language version override and migrating the code.")
        );

        let (_, settled, _, warnings) = scan_unit(
            "// @dart = 2.12\nvar x = 1;\n",
            DEFAULT_LANGUAGE_VERSION,
            file,
            &grants,
        );
        assert_eq!(settled, LanguageVersion::new(2, 12));
        assert!(warnings.is_empty(), "the floor itself is allowed");
    }

    /// One library's structural row: id, defining file, parts, deps, and
    /// part targets.
    type GraphShapeRow = (LibraryId, FileId, Vec<FileId>, Vec<LibraryId>, Vec<FileId>);

    /// The structural graph face of the live library set: membership,
    /// defining file, parts, deps, and part targets — everything the delta
    /// must keep exact except the per-member content identity (which moves
    /// on every member edit by design).
    fn graph_shape(state: &EngineState) -> Vec<GraphShapeRow> {
        let mut shape: Vec<_> = state
            .libraries
            .iter_infos()
            .map(|(id, info)| {
                (
                    id,
                    info.defining,
                    info.parts.clone(),
                    info.deps.clone(),
                    info.part_targets.to_vec(),
                )
            })
            .collect();
        shape.sort_unstable_by_key(|(id, _, _, _, _)| *id);
        shape
    }

    /// Body-edit invariant (delta discipline): an edit that changes no
    /// directive performs ZERO graph mutations — membership, reverse edges,
    /// the URI memo, and every library's structural identity stand
    /// byte-exact — while the owning library still invalidates (its content
    /// identity moved), and nothing else dirties.
    #[test]
    fn body_edit_without_directive_change_performs_zero_graph_mutations() {
        let fixture = Fixture::new();
        let mut state = fixture.state();
        let lib = fixture.root.join("lib");
        set(&mut state, &lib.join("a.dart"), "int a() => 1;\n");
        let main = set(
            &mut state,
            &lib.join("main.dart"),
            "import 'a.dart';\nint mainValue = 1;\n",
        );
        apply(&mut state, &mut |_| {});

        let owner = state.library_of_file[&main];
        let membership = state.library_of_file.clone();
        let importers = state.importers.clone();
        let uris = state.canonical_uris.clone();
        let offers = state.part_offers.clone();
        let sources = state.import_sources.clone();
        let shape = graph_shape(&state);

        // The carry-forward contract (#428): an apply replaces the dirty
        // set, but the NEXT close seeds the previous set back as leftovers
        // until an analyze consumes it. The cold apply left both libraries
        // invalidated; drain them so this pin isolates the body-edit delta
        // itself instead of the leftover union a real client's analyze
        // would have consumed between applies.
        state.invalidated.clear();
        set(
            &mut state,
            &lib.join("main.dart"),
            "import 'a.dart';\nint mainValue = 2;\n",
        );
        apply(&mut state, &mut |_| {});

        assert_eq!(state.library_of_file, membership, "membership untouched");
        assert_eq!(state.importers, importers, "reverse edges untouched");
        assert_eq!(state.canonical_uris, uris, "URI memo untouched");
        assert_eq!(state.part_offers, offers, "offer index untouched");
        assert_eq!(state.import_sources, sources, "source index untouched");
        assert_eq!(graph_shape(&state), shape, "structural identity untouched");
        assert_eq!(
            state.invalidated,
            vec![owner],
            "the owning library still invalidates through its moved content identity"
        );
    }

    /// The survival half of the fingerprint reuse key (g485 W2): a recorded
    /// result drops at the very apply that moves its library's identity —
    /// including the body-edit case the L2 gate keeps cascade-clean — so
    /// the post-closure gate never sees a record whose member contents
    /// moved underneath it.
    #[test]
    fn identity_move_drops_the_recorded_result_at_the_apply() {
        let fixture = Fixture::new();
        let mut state = fixture.state();
        let lib = fixture.root.join("lib");
        let a = set(&mut state, &lib.join("a.dart"), "int a() => 1;\n");
        apply(&mut state, &mut |_| {});
        let mut graph: Option<crate::pipeline::ResolvedGraph> = None;
        let cancel = crate::cancel::CancellationToken::new();
        crate::pipeline::analyze(&mut state, &mut graph, None, &[], &mut |_| {}, &cancel)
            .expect("cold analyze");
        let owner = state.library_of_file[&a];
        assert!(state.results.contains_key(&owner), "the run is recorded");

        // A body edit: the content moves, the API face stands, the library
        // invalidates alone — and the record goes with the identity move.
        set(&mut state, &lib.join("a.dart"), "int a() => 2;\n");
        apply(&mut state, &mut |_| {});
        assert_eq!(
            state.invalidated,
            vec![owner],
            "the owning library still invalidates"
        );
        assert!(
            !state.results.contains_key(&owner),
            "the identity move dropped the recorded run"
        );
    }

    /// Reverse-edge symmetry: re-targeting an import drops the old edge
    /// exactly (entry removed, no zero-count ghost) and adds the new one —
    /// and the whole reverse-edge map holds only that one exact edge.
    #[test]
    fn import_retarget_updates_importers_symmetrically() {
        let fixture = Fixture::new();
        let mut state = fixture.state();
        let lib = fixture.root.join("lib");
        set(&mut state, &lib.join("a.dart"), "int a() => 1;\n");
        set(&mut state, &lib.join("b.dart"), "int b() => 2;\n");
        let main = set(&mut state, &lib.join("main.dart"), "import 'a.dart';\n");
        apply(&mut state, &mut |_| {});

        let owner = state.library_of_file[&main];
        let a = state
            .files
            .try_file_id(&lib.join("a.dart"))
            .expect("a.dart interned");
        let b = state
            .files
            .try_file_id(&lib.join("b.dart"))
            .expect("b.dart interned");
        assert_eq!(
            state.importers.row(a),
            &[ImportEdge {
                library: owner,
                count: 1
            }]
        );

        set(&mut state, &lib.join("main.dart"), "import 'b.dart';\n");
        apply(&mut state, &mut |_| {});

        assert!(
            !state.importers.has(a),
            "the old edge is gone without a ghost: {:?}",
            state.importers.row(a)
        );
        assert_eq!(
            state.importers.row(b),
            &[ImportEdge {
                library: owner,
                count: 1
            }],
            "the new edge carries the importing library"
        );
        assert_eq!(state.importers.len(), 1, "no other reverse edge exists");
        assert_eq!(state.library_of_file[&main], owner, "the owner id stands");
    }

    /// Row-level CSR equivalence (#1779): the compact projections read back
    /// exactly what the paired delta wrote — edge multiplicity counts each
    /// member's contribution, rows iterate in sorted key order, emptied rows
    /// drop outright, and the residency-flip fan-out seeds edges from
    /// `import_sources` without any directive-face move on the source side.
    #[test]
    fn csr_rows_track_multiplicity_order_and_residency_flips() {
        let fixture = Fixture::new();
        let mut state = fixture.state();
        let lib = fixture.root.join("lib");
        // Two members of one library both importing `t.dart`: the edge's
        // multiplicity must read 2 (the map's `count` column), and the two
        // importers' rows must agree.
        let target = set(&mut state, &lib.join("t.dart"), "int t() => 0;\n");
        let main = set(
            &mut state,
            &lib.join("main.dart"),
            "import 't.dart';\npart 'p.dart';\n",
        );
        set(
            &mut state,
            &lib.join("p.dart"),
            "part of 'main.dart';\nimport 't.dart';\n",
        );
        apply(&mut state, &mut |_| {});

        let owner = state.library_of_file[&main];
        assert_eq!(
            state.importers.row(target),
            &[ImportEdge {
                library: owner,
                count: 2
            }],
            "one edge per source member, multiplicity folded"
        );
        assert_eq!(
            state.import_sources.row(target).len(),
            2,
            "both source files project"
        );

        // Sorted-order pin: a second library importing the same target lands
        // in `LibraryId` order, regardless of hash placement — the read the
        // importer closure consumes wholesale.
        let other = set(&mut state, &lib.join("other.dart"), "import 't.dart';\n");
        apply(&mut state, &mut |_| {});
        let other_owner = state.library_of_file[&other];
        let mut sorted = [(owner, 2u32), (other_owner, 1u32)];
        sorted.sort_unstable_by_key(|(library, _)| *library);
        assert_eq!(
            state.importers.row(target),
            &[
                ImportEdge {
                    library: sorted[0].0,
                    count: sorted[0].1
                },
                ImportEdge {
                    library: sorted[1].0,
                    count: sorted[1].1
                },
            ],
            "two libraries' edges share one row, sorted by id"
        );

        // One member dropping its import decrements that library's edge to
        // 1 (no ghost entry at 0 — emptied elements leave the row).
        set(&mut state, &lib.join("p.dart"), "part of 'main.dart';\n");
        apply(&mut state, &mut |_| {});
        let expected: Vec<ImportEdge> = sorted
            .into_iter()
            .map(|(library, count)| ImportEdge {
                library,
                count: if library == owner { count - 1 } else { count },
            })
            .collect();
        assert_eq!(
            state.importers.row(target),
            expected.as_slice(),
            "the surviving edges keep their counts"
        );

        // Residency flip: removing `t.dart` drops its rows everywhere; a
        // re-add re-seeds the edge from `import_sources` alone (the sources'
        // faces never moved).
        remove(&mut state, target);
        apply(&mut state, &mut |_| {});
        assert!(
            !state.importers.has(target),
            "the removed target's row dropped"
        );
        assert!(
            state.import_sources.row(target).len() == 2,
            "raw sources persist across the residency flip"
        );
        set(&mut state, &lib.join("t.dart"), "int t() => 1;\n");
        apply(&mut state, &mut |_| {});
        let readded = state
            .files
            .try_file_id(&lib.join("t.dart"))
            .expect("re-interned");
        let reseeded: Vec<ImportEdge> = state.importers.row(readded).to_vec();
        assert_eq!(
            reseeded.len(),
            2,
            "the flip re-seeded one edge per standing source: {reseeded:?}"
        );
    }

    /// Structural retirement: deleting a library's last file retires the
    /// library — its infos entry drops, the membership entry goes, and the
    /// cached result and reference surface go with it (no unbounded growth).
    #[test]
    fn deleting_the_last_file_of_a_library_drops_its_infos_entry() {
        let fixture = Fixture::new();
        let mut state = fixture.state();
        let lib = fixture.root.join("lib");
        let main = set(&mut state, &lib.join("main.dart"), "void main() {}\n");
        apply(&mut state, &mut |_| {});

        let owner = state.library_of_file[&main];
        assert!(state.libraries.info(owner).is_some(), "the library is live");

        remove(&mut state, main);
        apply(&mut state, &mut |_| {});

        assert!(
            state.libraries.info(owner).is_none(),
            "the infos entry dropped with its tenant"
        );
        assert!(!state.library_of_file.contains_key(&main));
        assert!(
            state.libraries.live().all(|live| live != owner),
            "the id is not live"
        );
        assert!(
            !state.results.contains_key(&owner),
            "the cached result went with the retirement"
        );
        assert!(
            state.surfaces().all(|(id, _)| id != owner),
            "the reference surface went with the retirement"
        );
    }

    /// The capture arena's absence semantics (the dense-id slots): a
    /// removal drops the capture outright, and re-adding the same path
    /// reuses its id's `None` slot — the file id is immortal, so the
    /// slot is refilled, never re-keyed. The refill lands with the
    /// analyze-phase recapture: the re-added bytes hit the facts cache
    /// (no parse ran at the apply), so the capture queues exactly like
    /// the cache-served paths, and the drain fills the same id's slot.
    #[test]
    fn removing_a_file_drops_its_capture_and_the_slot_is_reusable() {
        let fixture = Fixture::new();
        let mut state = fixture.state();
        let lib = fixture.root.join("lib");
        let main = set(&mut state, &lib.join("main.dart"), "void main() {}\n");
        apply(&mut state, &mut |_| {});
        let captured = state.capture(main).expect("the apply stored a capture");
        assert_eq!(captured.file, main, "the capture carries its own file");

        remove(&mut state, main);
        apply(&mut state, &mut |_| {});
        assert!(
            state.capture(main).is_none(),
            "the capture dropped with residency"
        );

        set(&mut state, &lib.join("main.dart"), "void main() {}\n");
        apply(&mut state, &mut |_| {});
        // The re-add served its facts from the cache, so the capture waits
        // for the analyze-phase recapture (the apply-side store only rides
        // a parse).
        crate::pipeline::drain_captures(&mut state);
        let recaptured = state.capture(main).expect("the re-added file recaptured");
        assert_eq!(recaptured.file, main, "the same id's slot was refilled");
    }

    /// Strict-mode resolution determinism (the options lookup contract):
    /// the deepest governing entry wins, same-length sibling prefixes
    /// never interfere with each other's files, and not the `options` map's insertion (iteration) order. The containing keys
    /// of a path are its ancestors — pairwise nested — so an
    /// equal-depth tie cannot exist; this pins the invariant against a
    /// future scan or index that reintroduces order dependence.
    #[test]
    fn strict_modes_pin_the_deepest_entry_regardless_of_insertion_order() {
        use crate::analysis_options::AnalysisOptions;
        use dartq_semantic::StrictModes;

        let fixture = Fixture::new();
        let base = fixture.root.join("pkg");
        let nested = base.join("nested");
        // `sibling` matches `base`'s component count but never contains
        // its files; its flags differ so any leakage is visible.
        let sibling = fixture.root.join("sibling");
        let options = |strict: bool| {
            let mut options = AnalysisOptions::default();
            options.strict_casts = Some(strict);
            options.strict_raw_types = (!strict).then_some(true);
            Arc::new(options)
        };

        let built = |insertion_reversed: bool| {
            let mut state = fixture.state();
            let mut entries = vec![
                (base.clone(), options(true)),
                (nested.clone(), options(false)),
                (sibling.clone(), options(true)),
            ];
            if insertion_reversed {
                entries.reverse();
            }
            for (dir, options) in entries {
                state.options.insert(dir, options);
            }
            let under_base = set(&mut state, &base.join("f.dart"), "var x = 0;\n");
            let under_nested = set(&mut state, &nested.join("g.dart"), "var y = 0;\n");
            (state, under_base, under_nested)
        };

        for insertion_reversed in [false, true] {
            let (state, under_base, under_nested) = built(insertion_reversed);
            // Deepest entry wins: `base` governs its own file (the
            // fixture root's entry also contains it, shallower), `nested`
            // overrides `base` for its own.
            assert_eq!(
                state.strict_modes_for(under_base),
                StrictModes {
                    casts: true,
                    inference: false,
                    raw_types: false,
                },
                "the base entry governs, the sibling's flags never leak"
            );
            assert_eq!(
                state.strict_modes_for(under_nested),
                // The nested entry's own flags win wholesale: `options(false)`
                // carries `strict_casts = Some(false)` AND
                // `strict_raw_types = Some(true)` (the leak-visibility
                // design above), so both columns answer from `nested` —
                // `base`'s `raw_types = None` baseline must not leak in.
                StrictModes {
                    casts: false,
                    inference: false,
                    raw_types: true,
                },
                "the nested entry overrides its ancestor's"
            );
            // Repeated resolution agrees, every time.
            let first = state.strict_modes_for(under_base);
            for _ in 0..4 {
                assert_eq!(state.strict_modes_for(under_base), first);
            }
        }
    }

    /// Warm-start staging survives unclaimed applies (#1387): a batch that
    /// matches no staged entry leaves the staging map standing — the lookup
    /// key IS the content hash, so a stale entry can never serve a moved
    /// file, and the entries an apply does claim are removed at the claim.
    /// The old single-shot drain dropped precisely the later-batch hits
    /// (a file reverting to staged content). The warm arm here is a fresh
    /// engine state, the real daemon shape: empty caches, staged payloads.
    #[test]
    fn warm_start_retains_unclaimed_entries_until_claimed_or_shed() {
        let fixture = Fixture::new();
        let mut source = fixture.state();
        let lib = fixture.root.join("lib");
        let a = set(&mut source, &lib.join("a.dart"), "int shared() => 1;\n");
        apply(&mut source, &mut |_| {});

        // The real ingest shape: `a`'s facts projected under its live key
        // (the key is path-free, so another file with the same bytes and
        // package can claim it) plus the same key with its hash corrupted —
        // an entry no batch can ever claim.
        let content = source.residency[&a].content;
        let live = snapshot_key(facts_key(&source.grants(), a, content));
        let mut dead = live;
        dead.content_hash[0] ^= 0xff;
        let mut state = fixture.state();
        state.warm_start.insert(
            live,
            crate::state::persist::PersistedFacts::of(
                &source.residency[&a],
                &source.strings,
                &source.files,
            ),
        );
        state.warm_start.insert(
            dead,
            crate::state::persist::PersistedFacts::of(
                &source.residency[&a],
                &source.strings,
                &source.files,
            ),
        );

        // Batch 1: `b` lands with different bytes — nothing claims, and both
        // staged entries stand.
        let _ = set(&mut state, &lib.join("b.dart"), "int mine() => 2;\n");
        apply(&mut state, &mut |_| {});
        assert_eq!(
            state.warm_start.len(),
            2,
            "unclaimed staged entries survive the apply"
        );

        // Batch 2: `b` reverts to the staged bytes — its live key matches
        // the staged identity, the claim removes the entry, and the payload
        // serves the recompute. The unclaimable entry stands until the
        // tier-3 shed or the next ingest.
        let _ = set(&mut state, &lib.join("b.dart"), "int shared() => 1;\n");
        apply(&mut state, &mut |_| {});
        assert_eq!(state.warm_start.len(), 1, "the claim removed its entry");
        assert!(
            state.warm_start.contains_key(&dead),
            "the unclaimable entry stands until shed"
        );
    }

    /// The chunked cold-apply fan-out ([`APPLY_FANOUT_CHUNK`]): a batch
    /// larger than one chunk lands every miss with its own resolved face,
    /// and a same-content straddler classified into the second chunk still
    /// dedupes into template + derived sibling — the sibling derives from
    /// the template's pre-resolution face with its own path attribution, so
    /// chunking moves only the in-flight bound, never the batch's
    /// observable face.
    #[test]
    fn multi_chunk_apply_lands_every_miss_and_dedupes_across_chunks() {
        let fixture = Fixture::new();
        let mut state = fixture.state();
        let lib = fixture.root.join("lib");
        // One chunk of distinct misses plus a tail in the second chunk; the
        // straddler repeats the first file's bytes from inside the second
        // chunk (same key → sibling of chunk 1's template, never a miss).
        // The unresolved relative import gives every face a URI row, so the
        // derivation's re-attribution is visible.
        let total = APPLY_FANOUT_CHUNK + 4;
        let mut files = Vec::with_capacity(total);
        for i in 0..total - 1 {
            let text = format!("import 'missing_{i}.dart';\nint f{i}() => {i};\n");
            files.push(set(&mut state, &lib.join(format!("u{i:05}.dart")), &text));
        }
        files.push(set(
            &mut state,
            &lib.join("straddle.dart"),
            "import 'missing_0.dart';\nint f0() => 0;\n",
        ));
        apply(&mut state, &mut |_| {});

        assert_eq!(
            state.residency.len(),
            total,
            "every miss across both chunks landed in residency"
        );
        let template = &state.residency[&files[0]];
        let straddler = &state.residency[&files[total - 1]];
        assert_eq!(
            straddler.diagnostics.len(),
            template.diagnostics.len(),
            "the straddling sibling derived the template's row count"
        );
        for row in &straddler.diagnostics {
            assert_eq!(
                row.file,
                files[total - 1],
                "the derived row re-attributed to the straddler's own file"
            );
        }
        assert_eq!(
            straddler.api_signature, template.api_signature,
            "same bytes, same baked signature across the chunk boundary"
        );
    }

    /// Scanner lexical diagnostics ride through the engine's analyze path
    /// into the unit facts (#1325): `compute_unit` extends the unit's
    /// diagnostics with the scan face's rows, so the `$`-EOF unterminated
    /// family surfaces here with the scanner-pinned anchors (the
    /// `bare_dollar` probe in the scanner's string suite —
    /// `missing_identifier` one unit past the `$`, the unterminated row
    /// back at the quote, both at length 1).
    #[test]
    fn scanner_diagnostics_reach_the_unit_facts() {
        let fixture = Fixture::new();
        let mut state = fixture.state();
        let main = set(&mut state, &fixture.root.join("lib/main.dart"), "'$\n");
        apply(&mut state, &mut |_| {});
        let facts = &state.residency[&main];

        let missing = facts
            .diagnostics
            .iter()
            .find(|d| d.code.name == "missing_identifier")
            .expect("the $-form's missing_identifier row flows through");
        assert_eq!(missing.range.offset, 2);
        assert_eq!(missing.range.length, 1);
        assert_eq!(missing.message, "Expected an identifier.");

        let unterminated = facts
            .diagnostics
            .iter()
            .find(|d| d.code.name == "unterminated_string_literal")
            .expect("the unterminated string row flows through");
        assert_eq!(unterminated.range.offset, 1);
        assert_eq!(unterminated.range.length, 1);
        assert_eq!(unterminated.message, "Unterminated string literal.");
    }

    /// The scanner's illegal-character rows flow through the analyze path
    /// too (#1325): non-ASCII in identifier position fires one
    /// `illegal_character` per character, pinned at its own UTF-16 unit
    /// (the scanner's `illegal_non_ascii_identifier_recovery` pins mirrored
    /// at the engine boundary).
    #[test]
    fn illegal_character_rows_reach_the_unit_facts() {
        let fixture = Fixture::new();
        let mut state = fixture.state();
        let main = set(
            &mut state,
            &fixture.root.join("lib/main.dart"),
            "var 한글;\n",
        );
        apply(&mut state, &mut |_| {});
        let facts = &state.residency[&main];

        let rows: Vec<_> = facts
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "illegal_character")
            .collect();
        assert_eq!(
            rows.len(),
            2,
            "one row per character: {:?}",
            facts.diagnostics
        );
        assert_eq!(rows[0].range.offset, 4);
        assert_eq!(rows[1].range.offset, 5);
        for row in &rows {
            assert_eq!(row.range.length, 1);
        }
    }
}
