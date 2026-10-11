//! `apply_pending`: the ContentId gate → facts recompute → the
//! identity-keyed library-graph delta → cascade closure
//! (incremental-l1-spec §3.2 steps 1–5).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use dartq_diagnostics::{CodeInfo, CodeRegistry, Diagnostic, Severity};
use dartq_language::{DEFAULT_LANGUAGE_VERSION, Feature, FeatureSet, LanguageVersion};
use dartq_package::PackageTable as IdentityTable;
use dartq_parser::{Message, Parser, ParserFeatures, ParserListener, TokenSpan};
use dartq_resolution::{
    CombinatorClause, Combinators, Configuration, ResolvedTarget, SelectedUri, UriResolution,
    UriResolver, resolve_conditional,
};
use dartq_scanner::{Keyword, KeywordStyle, ScanConfig, ScanResult, TokenKind, TokenTable, scan};
use dartq_snapshot::EntryKey;
use dartq_source::{ContentId, FileId, FileTable, SourceRange, StringTable, utf16_len_of};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::event::{EngineEvent, RevalidateGate};
use crate::state::{
    DirectiveFacts, DirectiveKind, EngineState, FACTS_SEMANTIC_VERSION, FactsKey, LibraryId,
    LibraryInfo, PendingChange, ScannedFace, UnitFacts,
};

/// The outcome of one apply (engine-api-spec §1.2 `AffectedReport` payload).
#[derive(Debug, Default)]
pub(crate) struct ApplyOutcome {
    pub changed: Vec<FileId>,
    pub removed: Vec<FileId>,
    pub invalidated: Vec<LibraryId>,
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
pub(crate) fn apply(
    state: &mut EngineState,
    on_event: &mut dyn FnMut(EngineEvent),
) -> ApplyOutcome {
    if state.pending.is_empty() {
        return ApplyOutcome::empty();
    }

    // Step 1: the ContentId gate (official defers the comparison to here,
    // never to set_content — a rewrite with identical bytes is a legal no-op).
    let order = std::mem::take(&mut state.pending_order);
    let mut pending = std::mem::take(&mut state.pending);
    let mut changed: Vec<(FileId, dartq_source::ContentId, String)> = Vec::new();
    let mut removed: Vec<FileId> = Vec::new();
    let mut gate_hits: Vec<FileId> = Vec::new();
    // Previous facts of every touched file — the OLD face the graph delta
    // subtracts. Captured here because both arms destroy the entries: a
    // removal drops residency, a set overwrites it below.
    let mut old_facts: FxHashMap<FileId, Arc<UnitFacts>> = FxHashMap::default();
    for file in order {
        match pending.remove(&file) {
            Some(PendingChange::Set(text)) => {
                if let Some(previous) = state.residency.get(&file) {
                    old_facts.insert(file, Arc::clone(previous));
                }
                let content = state.store.put(text.as_bytes());
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
                    state.captures.remove(&file);
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
                    removed.push(file);
                }
            }
            None => {}
        }
    }
    debug_assert!(pending.is_empty(), "drained entries must all be consumed");
    let changed_files: Vec<FileId> = changed.iter().map(|(file, _, _)| *file).collect();
    // The recapture queue: every changed file's declaration spans may have
    // moved (a body edit under a stable signature keeps its library clean
    // but shifts offsets), so recapture tracks changed files, not dirty
    // libraries. Gate hits (identical bytes) recapture nothing.
    for file in &changed_files {
        state.mark_captures_dirty(*file);
    }

    on_event(EngineEvent::OverlayApplied {
        changed: changed_files.clone(),
        removed: removed.clone(),
    });
    if let Some(sink) = state.trace.as_mut() {
        sink.overlay_applied(changed_files.len(), removed.len());
    }

    // Steps 2–3: facts recompute for gate-passing files (parallel scan/parse/
    // collect, sequential URI resolution), then residency update. The
    // pre-apply signatures are captured first — the L2 gate (PRD §11.3)
    // compares them against the recomputed ones to classify the cascade.
    let recomputed = recompute_facts(state, &changed);
    let old_signatures: FxHashMap<FileId, dartq_semantic::ApiSignature> = recomputed
        .iter()
        .filter_map(|(file, _, _)| {
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
    gates.extend(recomputed.iter().map(|(file, _facts, cache_hit)| {
        (
            *file,
            RevalidateGate::Parsed {
                ast_cache_hit: *cache_hit,
            },
        )
    }));
    for (file, facts, _) in &recomputed {
        state.residency.insert(*file, Arc::clone(facts));
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

    // The L2 classification: a file whose API signature moved (or that is
    // newly resident — no previous signature to match) re-seeds the cascade;
    // a body/comment edit with a stable face does not.
    let sig_changed: Vec<FileId> = recomputed
        .iter()
        .filter(|(file, facts, _)| old_signatures.get(file) != Some(&facts.api_signature))
        .map(|(file, _, _)| *file)
        .collect();

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
    // The closure is signature-gated (L2, PRD §11.3): seeds are the owners of
    // signature-changed files, and both live owners of every file whose
    // membership moved (a `part of` detach strands the losing library's
    // importers on stale facts; a claim transfer hands its part to a
    // competitor library that no other seed reaches). The delta reads the
    // pre-apply owners straight out of `library_of_file` before it writes
    // any membership entry.
    let (invalidated, retired_any) =
        maintain_and_close(state, &changed, &removed, &old_facts, &sig_changed);
    state.invalidated = invalidated.clone();
    // The resolution graph rebuilds only when its inputs moved: a
    // signature-bearing face (element or directive), a removal, or a library
    // retirement. A body edit with a stable face leaves the graph byte-identical.
    state.structure_dirty = retired_any || !removed.is_empty() || !sig_changed.is_empty();
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
    // Amortized store collection: this apply's replaced and removed
    // residency contents are the new dead candidates (gate hits carry none
    // — their put deduplicated); facts-cache evictions tallied themselves
    // during the recompute. Runs after the residency rewrite above, so the
    // replaced contents are already unreferenced if the sweep fires.
    state.collect_garbage(changed.len() + removed.len());
    ApplyOutcome {
        changed: changed_files,
        removed,
        invalidated,
    }
}

/// Recomputes the facts of every gate-passing file: fan-out over the
/// pure per-file work (language version → scan → directive diet walk → parse
/// diagnostics → element collection), then a sequential URI-resolution pass
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
/// Changed files parse **twice** at M1 — once for recovery diagnostics
/// through the engine's listener, once inside `dartq_semantic::collect`,
/// which hardcodes its own listener. The waste is bounded to changed files
/// and is on record as the reason #120/#121 should land a combined listener
/// face.
fn recompute_facts(
    state: &mut EngineState,
    changed: &[(FileId, ContentId, String)],
) -> Vec<(FileId, Arc<UnitFacts>, bool)> {
    // Language version per file: the owning package's version (the engine's
    // single source of truth for file → package attribution).
    let versions: Vec<LanguageVersion> = changed
        .iter()
        .map(|(file, _, _)| language_version_for(&state.packages, &state.files, *file))
        .collect();

    // Cache lookups up front (read-only); misses dispatch to the fan-out
    // below, deduplicated by key so a batch computes each distinct
    // `(content, feature set)` once.
    let mut slots: Vec<Option<(Arc<UnitFacts>, bool)>> = Vec::with_capacity(changed.len());
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
        let key = facts_key(&state.packages, &state.files, changed[index].0, *content);
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
    let computed: Vec<(usize, FactsKey, UnitFacts, ScannedFace)> = misses
        .par_iter()
        .map(|(index, key)| {
            let (facts, face) = compute_unit(
                changed[*index].0,
                changed[*index].1,
                sources[*index],
                versions[*index],
                &state.strings,
            );
            (*index, *key, facts, face)
        })
        .collect();

    // Warm-start materialization (#482): each matched payload comes off the
    // staging map — the keyed removal is the content-hash verification —
    // and materializes against the live tables. Grouped by key so a
    // same-content batch (one payload, several consumers) removes it once;
    // the extras clone the payload (identical bytes at two paths, rare and
    // bounded by the batch). The staging map is single-shot: whatever no
    // changed file claimed was stale against this batch and drops here —
    // stale entries never survive an apply (bounded memory; the next
    // ingest re-seeds). Grow-only: ingestion seeds recomputes, it never
    // touches residency (#377).
    let mut warmed: Vec<(usize, UnitFacts)> = Vec::with_capacity(warm.len());
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
                materialize_warm(first_payload, state, sources[first], &codes),
            ));
            if let Some(template) = rest {
                for index in slots {
                    warmed.push((
                        index,
                        materialize_warm(template.clone(), state, sources[index], &codes),
                    ));
                }
            }
        }
        state.warm_start.clear();
    }

    // Sequential URI resolution over the shared FileTable: each template
    // resolves for its own file, then each sibling derives from the
    // pre-resolution template — diagnostics re-attributed, directives
    // re-resolved — under the same sequential pass (resolution interns).
    // The existence face is the post-batch world: files already in
    // residency plus this batch (whose facts insert into residency only
    // after this pass; removals left it at the drain). Field borrows stay
    // disjoint: the closure reads residency, the resolver writes files.
    let batch: FxHashSet<FileId> = changed.iter().map(|(file, _, _)| *file).collect();
    let resident = |id: FileId| state.residency.contains_key(&id) || batch.contains(&id);
    let mut resolver = UriResolver::new(
        &state.uri_packages,
        &state.sdk,
        &state.target,
        &state.declared_variables,
        &mut state.files,
        &resident,
    );
    let mut to_cache: Vec<(FactsKey, Arc<UnitFacts>)> = Vec::with_capacity(computed.len());
    // Scanned faces wait for the resolver's `&mut FileTable` borrow to end
    // (same deferred-insert discipline as `to_cache`).
    let mut scanned_to_cache: Vec<(FactsKey, ScannedFace)> = Vec::with_capacity(computed.len());
    for (index, key, mut facts, face) in computed {
        // The scan face persists under the same key (batch siblings and
        // cross-batch hits share it): the semdiag fold and the index
        // captures parse through these tokens instead of re-scanning every
        // member of every dirty library (the audit's per-analyze
        // re-scan multiplication).
        scanned_to_cache.push((key, face));
        // The cache holds the pure face, pre-resolution: resolution is a
        // per-file step, so caching its results would serve one file's
        // targets (and URI diagnostics) to every same-content successor.
        to_cache.push((key, Arc::new(UnitFacts::clone(&facts))));
        for sibling in siblings.remove(&index).unwrap_or_default() {
            let derived = derive_facts(
                &facts,
                changed[sibling].0,
                &mut resolver,
                &state.declared_variables,
            );
            slots[sibling] = Some((Arc::new(derived), true));
        }
        resolve_unit(
            &mut facts,
            changed[index].0,
            &mut resolver,
            &state.declared_variables,
        );
        slots[index] = Some((Arc::new(facts), false));
    }
    // Cross-batch hits (#303): the served face was computed for another
    // file's identity — its diagnostics point there and its directives
    // resolved against that file's path. Re-run the per-file derivation
    // (same helper as the batch siblings) so the hit lands fully
    // re-attributed: the AstKey triple is path-free by contract (PRD §5.3),
    // making this a hit-site duty.
    for (index, facts) in hits {
        let derived = derive_facts(
            &facts,
            changed[index].0,
            &mut resolver,
            &state.declared_variables,
        );
        slots[index] = Some((Arc::new(derived), true));
    }
    // Warm-start re-derivation (#482): the materialized face attributes and
    // resolves against the snapshot's *saved* file; every baked diagnostic
    // belongs to the unit, so re-attributing all of them to the consuming
    // file is exact, and the URI diagnostic tail rebuilds one-for-one with
    // the re-resolved coded directives (resolve_unit's construction
    // invariant: exactly one pushed diagnostic per coded directive, after
    // every other diagnostic) — a same-content file at another path
    // re-resolves against its own directory.
    for (index, mut facts) in warmed {
        for diagnostic in &mut facts.diagnostics {
            diagnostic.file = changed[index].0;
        }
        let uri_tail = facts
            .directives
            .iter()
            .filter(|directive| directive.code.is_some())
            .count();
        facts
            .diagnostics
            .truncate(facts.diagnostics.len() - uri_tail);
        resolve_unit(
            &mut facts,
            changed[index].0,
            &mut resolver,
            &state.declared_variables,
        );
        slots[index] = Some((Arc::new(facts), true));
    }
    drop(resolver);
    for (key, face) in scanned_to_cache {
        state.cache_scanned(key, face);
    }
    for (key, facts) in to_cache {
        state.cache_facts(key, facts);
    }

    changed
        .iter()
        .zip(slots)
        .map(|((file, _, _), slot)| {
            let (facts, cache_hit) = slot.expect("every changed file ends with facts");
            (*file, facts, cache_hit)
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
/// the batch source. The post-resolution directive targets and attributed
/// diagnostics re-derive per consumer under the shared resolver (the
/// re-derivation pass in [`recompute_facts`]).
fn materialize_warm(
    payload: crate::state::persist::PersistedFacts,
    state: &mut EngineState,
    source: &str,
    codes: &CodeRegistry,
) -> UnitFacts {
    payload.materialize(source, &state.strings, codes, &mut |path| {
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
    let mut derived = UnitFacts::clone(template);
    for diagnostic in &mut derived.diagnostics {
        diagnostic.file = file;
    }
    resolve_unit(&mut derived, file, resolver, declared);
    derived
}

/// The file's language version: the owning package's version, or the
/// default when the file belongs to no package (engine-api-spec §1.1) — the
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
        .map(dartq_pub::LanguageVersion::into_inner)
        .unwrap_or(DEFAULT_LANGUAGE_VERSION)
}

/// The facts/scanned cache key of a resident file: the `AstKey` triple
/// (PRD §5.3) derived from the file's content and its package-derived
/// feature fingerprint. The single derivation both the apply recompute and
/// the drained consumers share, so a cache entry is always found under the
/// key it was stored under.
pub(crate) fn facts_key(
    packages: &IdentityTable,
    files: &FileTable,
    file: FileId,
    content: ContentId,
) -> FactsKey {
    let version = language_version_for(packages, files, file);
    FactsKey {
        content,
        feature_fingerprint: FeatureSet::for_language_version(version).fingerprint(),
        semantic_version: FACTS_SEMANTIC_VERSION,
    }
}

/// The pure per-file pipeline: the shared scan face ([`scan_unit`]), then
/// directive diet walk, parse diagnostics, element collection. Returns the
/// facts plus the scan face (tokens + settled feature set) — the face
/// persists in the scanned cache so the analyze-phase consumers parse these
/// tokens instead of re-scanning the file.
fn compute_unit(
    file: FileId,
    content: ContentId,
    text: &str,
    version: LanguageVersion,
    strings: &StringTable,
) -> (UnitFacts, ScannedFace) {
    let (feature_set, scanned, mut diagnostics) = scan_unit(text, version, file);
    diagnostics.extend(parse_diagnostics(file, text, &scanned.tokens, feature_set));
    // The single allocation of the element model: every later consumer
    // (duplicate bake, api signature, facts clones, the semdiag fold)
    // shares this `Arc` (#308).
    let elements = Arc::new(dartq_semantic::collect(
        text,
        &scanned.tokens,
        ParserFeatures::from_feature_set(feature_set),
        strings,
    ));
    diagnostics.extend(duplicate_diagnostics(file, &elements, strings));
    let (directives, directive_region) = extract_directives(text, &scanned.tokens, strings);
    let face = directive_face(text, &scanned.tokens, directive_region);

    let facts = UnitFacts {
        content,
        api_signature: unit_signature(&elements, &face, strings),
        directives,
        elements,
        diagnostics,
        line_index: scanned.line_index,
    };
    let scanned_face = ScannedFace {
        tokens: scanned.tokens,
        features: feature_set,
    };
    (facts, scanned_face)
}

/// The file's API signature (PRD §11.3, #282): the canonical element-face
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
/// header-window match, so at most one rescan settles).
///
/// An override above the latest known version is officially rejected, not
/// clamped (official scanner `_languageVersionChanged`): the
/// `invalid_language_version_override` STATIC_WARNING pins at the comment
/// lexeme and the override is dropped (`_overrideVersion = null`) — the
/// feature set stays the package/default-derived one and no rescan runs.
/// The warning rides back for the diagnostics face to bake; the
/// index-capture face drops it (it produces no diagnostics).
pub(crate) fn scan_unit(
    text: &str,
    version: LanguageVersion,
    file: FileId,
) -> (FeatureSet, ScanResult, Vec<Diagnostic>) {
    let mut feature_set = FeatureSet::for_language_version(version);
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
        } else if over != version {
            feature_set = FeatureSet::for_language_version(over);
            scanned = scan(text, scan_config(feature_set), file);
        }
    }
    (feature_set, scanned, warnings)
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
            facts
                .diagnostics
                .push(uri_diagnostic(code, uri.unwrap_or_default(), span, file));
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

/// Parse-recovery diagnostics through the engine's listener (pass 1 of 2;
/// see `recompute_facts` for the two-pass note). The listener moves into the
/// parser (the trait is not object-safe over `&mut` here), so diagnostics
/// flow through a shared `Rc<RefCell>` — the same pattern
/// `dartq_semantic::collect` uses.
fn parse_diagnostics(
    file: FileId,
    text: &str,
    tokens: &TokenTable,
    feature_set: FeatureSet,
) -> Vec<Diagnostic> {
    let diags = Rc::new(RefCell::new(Vec::new()));
    Parser::new(
        text,
        tokens,
        ParserFeatures::from_feature_set(feature_set),
        DiagCollector {
            file,
            diags: Rc::clone(&diags),
        },
    )
    .parse_unit();
    Rc::try_unwrap(diags)
        .expect("parser released its listener")
        .into_inner()
}

/// Collects `handle_recoverable_error` events into engine diagnostics.
/// `handle_experiment_not_enabled` stays a no-op: its payload is a bare
/// feature flag without a code+text pair, and inventing wording here would
/// break oracle text parity — the message-capable path lands with the
/// diagnostic tickets.
struct DiagCollector {
    file: FileId,
    diags: Rc<RefCell<Vec<Diagnostic>>>,
}

impl ParserListener for DiagCollector {
    fn handle_recoverable_error(&mut self, message: Message, start: TokenSpan, _end: TokenSpan) {
        self.diags.borrow_mut().push(Diagnostic::new(
            Severity::Error,
            message.code,
            message.text,
            self.file,
            start.range,
            None,
        ));
    }
}

/// Folds the collector's capture-time duplicate log into the unit's baked
/// diagnostics as the frozen `duplicate_definition` row: ERROR at the second
/// declaration's name token, setters named by their bare identifier (the
/// same construction `dartq_semantic::diagnose` uses — one source of truth
/// is the log, both faces render it identically). The log is a pure function
/// of the unit's content and feature set, so it bakes beside the syntax
/// diagnostics; a wired `SemanticDiagnoser` that runs `diagnose` must not
/// re-fold the same entries (its adapter owes the dedup to this face).
fn duplicate_diagnostics(
    file: FileId,
    elements: &dartq_semantic::FileElements,
    strings: &StringTable,
) -> Vec<Diagnostic> {
    elements
        .duplicates()
        .iter()
        .map(|duplicate| {
            let text = strings.resolve(duplicate.name);
            let display = text.strip_suffix('=').unwrap_or(text);
            Diagnostic::new(
                dartq_semantic::codes::SemanticCode::DuplicateDefinition.severity(),
                dartq_semantic::codes::SemanticCode::DuplicateDefinition.code_info(),
                dartq_semantic::codes::SemanticCode::DuplicateDefinition.message(display, ""),
                file,
                duplicate.range,
                None,
            )
        })
        .collect()
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
    loop {
        let Some(token) = tokens.get(i) else { break };
        match token.kind {
            TokenKind::Eof => break,
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
    (directives, i)
}

/// The token kind at `i`, if any.
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

/// Reads the URI literal starting at `i`. Adjacent string literals
/// concatenate into one URI (`import 'li' 'b/a.dart';` resolves `lib/a.dart`).
/// The oracle's assembly is the directive uri's `StringLiteral.stringValue`
/// (file_state.dart:1017,1147): `AdjacentStringsImpl._appendStringValue`
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
    loop {
        match kind_at(tokens, j) {
            Some(TokenKind::String) => {
                let lexeme = tokens.lexeme(text, j);
                let token = tokens.get(j).expect("kind_at checked");
                span_end = token.utf16_offset + utf16_len_of(lexeme);
                match string_literal_value(lexeme) {
                    Some(segment) => value.push_str(&segment),
                    None => decodable = false,
                }
                j += 1;
            }
            _ => break,
        }
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
                if let Some(next) = kind_at(tokens, i + 1) {
                    if is_name_token(next) || matches!(next, TokenKind::Keyword(_)) {
                        prefix = Some(strings.intern(tokens.lexeme(text, i + 1)));
                        i += 2;
                        continue;
                    }
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
/// official `parseConditionalUri` decision order (parser_impl.dart:1095):
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
}

impl UnitFace {
    fn of(facts: &UnitFacts) -> Self {
        let mut face = Self {
            part: facts.is_part(),
            part_targets: Vec::new(),
            import_targets: Vec::new(),
        };
        for directive in &facts.directives {
            if let ResolvedTarget::Resolved(target) = directive.target {
                match directive.kind {
                    DirectiveKind::Part => face.part_targets.push(target),
                    DirectiveKind::Import | DirectiveKind::Export => {
                        face.import_targets.push(target)
                    }
                    DirectiveKind::PartOf | DirectiveKind::Library => {}
                }
            }
        }
        face
    }
}

/// Applies the identity-keyed library-graph delta for one batch and derives
/// the post-closure dirty set (§3.2 steps 3–5).
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
///
/// Formation semantics are unchanged from the full rebuild: defining files
/// own libraries keyed on their canonical URI, part files join the
/// path-smallest qualifying claimant or fall back to singletons, and every
/// resident file belongs to exactly one library. Returns the invalidated
/// set and whether any library was retired.
fn maintain_and_close(
    state: &mut EngineState,
    changed: &[(FileId, ContentId, String)],
    removed: &[FileId],
    old_facts: &FxHashMap<FileId, Arc<UnitFacts>>,
    sig_changed: &[FileId],
) -> (Vec<LibraryId>, bool) {
    let changed_set: FxHashSet<FileId> = changed.iter().map(|(file, _, _)| *file).collect();
    let removed_set: FxHashSet<FileId> = removed.iter().copied().collect();
    // Old and new faces. A file entering residency here has no old face —
    // nothing to subtract.
    let changed_faces: Vec<(FileId, Option<UnitFace>, UnitFace)> = changed
        .iter()
        .map(|(file, _, _)| {
            let new = UnitFace::of(&state.residency[file]);
            let old = old_facts.get(file).map(|facts| UnitFace::of(facts));
            (*file, old, new)
        })
        .collect();
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
    const NO_TARGETS: &[FileId] = &[];
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
    }

    // Phase 2 — ownership decisions. Resolution reads the FINAL offer index,
    // so every offer delta above has landed; owners mint through the arena's
    // URI keys, which keeps a defining file's library id stable across edits
    // of its content (the URI is path-derived). `library_keys` records the
    // key file of every interned id so freshly formed slots (no info yet)
    // know their defining file at the recompute.
    let mut library_keys: FxHashMap<LibraryId, FileId> = FxHashMap::default();
    let mut movers: Vec<(FileId, Option<LibraryId>, LibraryId)> = Vec::new();
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
        state.library_surfaces.remove(library);
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
    // longer resident) — captured before the entry drops.
    for (file, _) in &removed_faces {
        if let Some(importers) = state.importers.get(file) {
            for library in importers.keys() {
                affected.insert(*library);
            }
        }
    }
    // A target whose owning library id moved flips the dependency column of
    // every library that imports it.
    for (file, old_owner, new_owner) in &movers {
        if old_owner.is_some() && changed_set.contains(file) {
            continue; // covered by the changed-file pass below
        }
        if *old_owner != Some(*new_owner)
            && let Some(importers) = state.importers.get(file)
        {
            for library in importers.keys() {
                affected.insert(*library);
            }
        }
    }
    for (file, old_owner, new_owner) in &changed_owners {
        if *old_owner != Some(*new_owner)
            && let Some(importers) = state.importers.get(file)
        {
            for library in importers.keys() {
                affected.insert(*library);
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
        let offerers: Vec<FileId> = state
            .part_offers
            .get(&file)
            .map(|offers| offers.iter().copied().collect())
            .unwrap_or_default();
        for offerer in offerers {
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
        let sources: Vec<FileId> = state
            .import_sources
            .get(file)
            .map(|sources| sources.iter().copied().collect())
            .unwrap_or_default();
        for source in sources {
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
            let occurrences = state
                .residency
                .get(&source)
                .map(|facts| {
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
                })
                .unwrap_or(0);
            for _ in 0..occurrences {
                add_edge(&mut state.importers, *file, owner);
            }
            affected.insert(owner);
        }
    }
    // One paired edge event per file: the old face subtracted, the new face
    // added, multiplicity exact. Formation only ever created edges between
    // resident endpoints, so both sides filter to the same notion: an edge
    // can exist exactly for a target resident pre-apply (post-update
    // residency plus this batch's removals), and a new edge exactly for a
    // target resident now. A stable owner over an identical effective target
    // set performs zero map operations.
    let was_resident =
        |target: FileId| state.residency.contains_key(&target) || removed_set.contains(&target);
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
        state.importers.remove(file);
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
        match previous {
            Some((defining, content, deps, part_targets)) => {
                let fresh = build_library_info(state, *library, defining);
                if fresh.part_targets != part_targets {
                    targets_moved.insert(*library);
                }
                if fresh.content != content
                    || fresh.deps != deps
                    || fresh.part_targets != part_targets
                {
                    identity_moved.insert(*library);
                    state.libraries.set_info(*library, fresh);
                }
            }
            None => {
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
    }

    let invalidated = close(
        state,
        sig_changed,
        &movers,
        &changed_set,
        &removed_owners,
        &affected,
        &identity_moved,
        &targets_moved,
    );
    (invalidated, !retired.is_empty())
}

/// The file's canonical URI, memoized ([`EngineState::canonical_uris`]): a
/// pure function of the immutable interned path, so a miss recomputes
/// exactly the string a previous residency held and no library identity can
/// fork across an eviction gap (audit #398 F10). canonical_uri never probes
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
/// across the incremental and full arms because paths are the FileTable's
/// own universe (#287): FileId interning order and canonical-URI bytes both
/// reorder under mid-sequence file creation and percent-encoding.
fn path_key<'a>(state: &'a EngineState, file: FileId) -> &'a [u8] {
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
    for candidate in state.part_offers.get(&part).into_iter().flatten() {
        let candidate = *candidate;
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
    let parts: Vec<FileId> = defining_facts
        .directives
        .iter()
        .filter(|directive| directive.kind == DirectiveKind::Part)
        .filter_map(|directive| match directive.target {
            ResolvedTarget::Resolved(target) => Some(target),
            _ => None,
        })
        .filter(|target| *target != defining && state.library_of_file.get(target) == Some(&library))
        .collect();
    let mut members = Vec::with_capacity(parts.len() + 1);
    members.push(defining);
    members.extend_from_slice(&parts);
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
                    // `uri_does_not_exist` against the absent URI).
                    if state.residency.contains_key(&target)
                        && target != defining
                        && state.library_of_file.get(&target) != Some(&library)
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
    LibraryInfo {
        // The memo's allocation, shared: a refcount per formed library,
        // never a copy of the URI bytes.
        uri,
        defining,
        parts: parts.into(),
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
/// touches nothing. Empty entries drop, so the indexes stay bounded by the
/// live directive set.
fn reindex(
    index: &mut FxHashMap<FileId, FxHashSet<FileId>>,
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
        if let Some(sources) = index.get_mut(target) {
            sources.remove(&source);
            if sources.is_empty() {
                index.remove(target);
            }
        }
    }
    for target in new_set.difference(&old_set) {
        index.entry(*target).or_default().insert(source);
    }
}

/// One reverse-edge insert. The adjacency counts multiplicity: two members
/// of one library importing the same target is a count of two, so one of
/// them re-targeting away leaves the edge exact.
fn add_edge(
    importers: &mut FxHashMap<FileId, FxHashMap<LibraryId, u32>>,
    target: FileId,
    library: LibraryId,
) {
    *importers
        .entry(target)
        .or_default()
        .entry(library)
        .or_insert(0) += 1;
}

/// The paired remove: decrements the multiplicity and drops entries as they
/// empty, so no zero-count ghost survives. An absent edge is a delta
/// asymmetry — debug builds trap on it.
fn remove_edge(
    importers: &mut FxHashMap<FileId, FxHashMap<LibraryId, u32>>,
    target: FileId,
    library: LibraryId,
) {
    debug_assert!(
        importers
            .get(&target)
            .is_some_and(|libraries| libraries.contains_key(&library)),
        "removing an absent reverse edge (delta asymmetry)"
    );
    if let Some(libraries) = importers.get_mut(&target) {
        if let Some(count) = libraries.get_mut(&library) {
            *count -= 1;
            if *count == 0 {
                libraries.remove(&library);
            }
        }
        if libraries.is_empty() {
            importers.remove(&target);
        }
    }
}

/// Rewrites one file's outgoing reverse edges from its old face to its new
/// face. A stable owner over an unchanged target set performs zero map
/// operations (the body-edit common case); an ownership move re-owns every
/// edge; otherwise the multiset difference moves only the changed
/// directives. Every removal is paired with the add that created the edge —
/// an asymmetry surfaces as a ghost edge, which the incremental-vs-full
/// oracle gate and the debug assert in [`remove_edge`] both catch.
fn retarget_edges(
    importers: &mut FxHashMap<FileId, FxHashMap<LibraryId, u32>>,
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
fn declared_library_name(facts: &UnitFacts) -> Option<&str> {
    facts.directives.iter().find_map(|directive| {
        if directive.kind == DirectiveKind::Library {
            directive.name.as_deref().filter(|name| !name.is_empty())
        } else {
            None
        }
    })
}

/// Whether `defining_file`'s `part` directive may claim the part file
/// `target`, mirroring the oracle's per-kind `isPartOf`:
///
/// * Name form — the includer's declared library name equals the part-of
///   name and both files live in the same package (analyzer
///   `PartOfNameFileKind::isPartOf`, file_state.dart:2222-2225; the analyzer
///   matches names globally over sibling-directory discovery, the M1 engine
///   bounds the scan to one package, incremental-l1-spec §7.1).
/// * URI form — the part file's own `part of` URI must resolve back to the
///   claiming defining file (`PartOfUriKnownFileKind::isPartOf`,
///   file_state.dart:2262-2265); the include alone is no proof. An
///   unresolvable URI (`PartOfUriUnknownFileKind`, file_state.dart:2283-2291)
///   or a shapeless one (interpolated — no resolvable string) stays
///   unclaimed.
fn may_claim(
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

/// The post-closure dirty set (§3.2 step 5, L2-refined per PRD §11.3):
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
fn close(
    state: &mut EngineState,
    sig_changed: &[FileId],
    movers: &[(FileId, Option<LibraryId>, LibraryId)],
    changed_set: &FxHashSet<FileId>,
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
    }
    // Membership-change seeds: changed files whose ownership moved (both
    // live owners), and libraries whose part-target identity moved — the
    // transfer shape, gated on a cached result exactly like the full
    // rebuild's pass over the live set.
    for (file, old_owner, new_owner) in movers {
        if !changed_set.contains(file) {
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
            if let Some(importers) = state.importers.get(&file) {
                for importer in importers.keys() {
                    dirty.insert(*importer);
                    if queued.insert(*importer) {
                        work.push(*importer);
                    }
                }
            }
        }
    }
    let mut invalidated: Vec<LibraryId> = dirty.into_iter().collect();
    invalidated.sort_unstable();
    invalidated
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use dartq_pub::{LockFile, PackageConfig, Pubspec, build_graph};
    use dartq_sdk::SdkSnapshot;

    use super::*;

    /// A throwaway one-package workspace ("t") with a synthetic VM SDK —
    /// the in-module twin of `tests/common/mod.rs`, which lib tests cannot
    /// import. The analyzed files themselves are overlay-only (the
    /// `Engine::set_content` discipline); only the pub/SDK scaffolding
    /// touches disk.
    struct Fixture {
        root: PathBuf,
        config_dir: PathBuf,
        config: Arc<PackageConfig>,
        packages: IdentityTable,
    }

    impl Fixture {
        fn new() -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let root = std::env::temp_dir().join(format!(
                "dartq-engine-l1-{}-{}",
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
            let lock = LockFile::parse("packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n")
                .expect("valid lock");
            let root_pubspec =
                Pubspec::parse("name: t\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n")
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

        fn state(&self) -> EngineState {
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
            )
            .expect("engine state")
        }
    }

    /// Enqueues a content overlay (the `Engine::set_content` discipline:
    /// intern, append to the order on first sight, last write wins).
    fn set(state: &mut EngineState, path: &Path, text: &str) -> FileId {
        let file = state.files.intern(path);
        if !state.pending.contains_key(&file) {
            state.pending_order.push(file);
        }
        state
            .pending
            .insert(file, PendingChange::Set(text.to_owned()));
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

    /// Conditional imports fork end-to-end (#512): the diet walk captures
    /// the `if` clauses, resolution selects the first clause the context's
    /// declared variables match (source order), and only the SELECTED
    /// candidate is read — an unmatched branch's missing target stays
    /// silent, a selected one is diagnosed at its own literal.
    #[test]
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
        let file = FileTable::new().intern(Path::new("/face.dart"));
        let (_, scanned, warnings) = scan_unit(text, DEFAULT_LANGUAGE_VERSION, file);
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

    /// A raw U+0001 inside a directive's string literal (legal source — the
    /// scanner folds control bytes into the string body) must stay framed by
    /// its length prefix: the face is not the retired `\u{1}`-separated join,
    /// which such a lexeme could alias into another token sequence.
    #[test]
    fn control_character_lexeme_does_not_alias_the_separator_join() {
        let text = "import 'a\u{1}b.dart';\n";
        let file = FileTable::new().intern(Path::new("/face.dart"));
        let (_, scanned, _) = scan_unit(text, DEFAULT_LANGUAGE_VERSION, file);
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

    /// The structural graph face of the live library set: membership,
    /// defining file, parts, deps, and part targets — everything the delta
    /// must keep exact except the per-member content identity (which moves
    /// on every member edit by design).
    fn graph_shape(
        state: &EngineState,
    ) -> Vec<(LibraryId, FileId, Vec<FileId>, Vec<LibraryId>, Vec<FileId>)> {
        let mut shape: Vec<_> = state
            .libraries
            .iter_infos()
            .map(|(id, info)| {
                (
                    id,
                    info.defining,
                    info.parts.to_vec(),
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
            state.importers.get(&a),
            Some(&[(owner, 1)].into_iter().collect())
        );

        set(&mut state, &lib.join("main.dart"), "import 'b.dart';\n");
        apply(&mut state, &mut |_| {});

        assert!(
            state.importers.get(&a).is_none(),
            "the old edge is gone without a ghost: {:?}",
            state.importers.get(&a)
        );
        assert_eq!(
            state.importers.get(&b),
            Some(&[(owner, 1)].into_iter().collect()),
            "the new edge carries the importing library"
        );
        assert_eq!(state.importers.len(), 1, "no other reverse edge exists");
        assert_eq!(state.library_of_file[&main], owner, "the owner id stands");
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
            !state.library_surfaces.contains_key(&owner),
            "the reference surface went with the retirement"
        );
    }
}
