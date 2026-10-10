//! `apply_pending`: the ContentId gate → facts recompute → directive diff →
//! library reformation → reverse-edge closure (incremental-l1-spec §3.2
//! steps 1–5).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use dartq_diagnostics::{CodeInfo, CodeRegistry, Diagnostic, Severity};
use dartq_language::{DEFAULT_LANGUAGE_VERSION, Feature, FeatureSet, LanguageVersion};
use dartq_package::PackageTable as IdentityTable;
use dartq_parser::{Message, Parser, ParserFeatures, ParserListener, TokenSpan};
use dartq_resolution::{CombinatorClause, Combinators, ResolvedTarget, UriResolution, UriResolver};
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
    for file in order {
        match pending.remove(&file) {
            Some(PendingChange::Set(text)) => {
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
                if state.residency.remove(&file).is_some() {
                    // The maintained index inputs drop with residency: a
                    // removed file's capture must not serve a later query.
                    state.captures.remove(&file);
                    // The canonical-URI memo drops with residency too
                    // (bounded, audit #398 F10 — the map previously grew
                    // monotonically with every path ever resident). Eviction
                    // is safe because `reform_libraries` is its only reader:
                    // it lazily recomputes the entry for every resident file
                    // before reading it, and the recompute is a pure function
                    // of the file's immutable interned path (the package
                    // table and config dir are fixed at state assembly), so
                    // a returning file reproduces the identical string and
                    // no library identity can fork across the gap.
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
    }

    // The L2 classification: a file whose API signature moved (or that is
    // newly resident — no previous signature to match) re-seeds the cascade;
    // a body/comment edit with a stable face does not.
    let sig_changed: Vec<FileId> = recomputed
        .iter()
        .filter(|(file, facts, _)| old_signatures.get(file) != Some(&facts.api_signature))
        .map(|(file, _, _)| *file)
        .collect();

    // Steps 4–5: library reformation, reverse-edge rebuild, dirty seeds,
    // importer closure. The closure output *replaces* the phase-2 work queue
    // (state.rs: "post-closure dirty set of the last apply; consumed by
    // analyze") — assignment, never a union. A cancelled analyze may leave
    // unfinished libraries queued; close() seeds those leftovers into its
    // dirty set, filtered to the post-reform live set: the identity diff
    // alone cannot see closure-stale leftovers — a library invalidated
    // because a DEPENDENCY's face moved carries that staleness nowhere in
    // its own identity (that is why cancel is a token, not an identity diff)
    // — so dropping such a leftover would serve its stale cached result
    // forever, and incremental would diverge from a full recompute
    // permanently. Assignment still preserves the invariant
    // `state.invalidated ⊆ live()`; a union would resurrect ids retired by
    // an intervening remove, and analyze unwraps every queued id's info.
    //
    // The closure is signature-gated (L2, PRD §11.3): seeds are the owners of
    // signature-changed files, the (still live) former owners of removed
    // files, and both live owners of any changed file whose membership moved;
    // everything else stays clean unless its own identity moved. The
    // pre-reform owners are captured before reform rebuilds
    // `library_of_file` — after the rebuild only the *new* owner of a changed
    // file is reachable, which is the detach hole (a `part of` edit strands
    // the losing library's importers on stale facts) and the transfer hole
    // (a claimant renaming away hands its part to a competitor library that
    // no seed reaches).
    let removed_owners: Vec<Option<LibraryId>> = removed
        .iter()
        .map(|file| state.library_of_file.get(file).copied())
        .collect();
    let changed_pre_owners: Vec<(FileId, Option<LibraryId>)> = changed
        .iter()
        .map(|(file, _, _)| (*file, state.library_of_file.get(file).copied()))
        .collect();
    let (invalidated, retired_any) =
        reform_and_close(state, &sig_changed, &changed_pre_owners, &removed_owners);
    state.invalidated = invalidated.clone();
    // The resolution graph rebuilds only when its inputs moved: a
    // signature-bearing face (element or directive), a removal, or a library
    // retirement. A body edit with a stable face leaves the graph byte-identical.
    state.structure_dirty = retired_any || !removed.is_empty() || !sig_changed.is_empty();
    on_event(EngineEvent::CascadeInvalidated {
        libraries: invalidated.clone(),
    });
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
            let derived = derive_facts(&facts, changed[sibling].0, &mut resolver);
            slots[sibling] = Some((Arc::new(derived), true));
        }
        resolve_unit(&mut facts, changed[index].0, &mut resolver);
        slots[index] = Some((Arc::new(facts), false));
    }
    // Cross-batch hits (#303): the served face was computed for another
    // file's identity — its diagnostics point there and its directives
    // resolved against that file's path. Re-run the per-file derivation
    // (same helper as the batch siblings) so the hit lands fully
    // re-attributed: the AstKey triple is path-free by contract (PRD §5.3),
    // making this a hit-site duty.
    for (index, facts) in hits {
        let derived = derive_facts(&facts, changed[index].0, &mut resolver);
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
        resolve_unit(&mut facts, changed[index].0, &mut resolver);
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
fn derive_facts(template: &UnitFacts, file: FileId, resolver: &mut UriResolver<'_>) -> UnitFacts {
    let mut derived = UnitFacts::clone(template);
    for diagnostic in &mut derived.diagnostics {
        diagnostic.file = file;
    }
    resolve_unit(&mut derived, file, resolver);
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
fn resolve_unit(facts: &mut UnitFacts, file: FileId, resolver: &mut UriResolver<'_>) {
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
            let UriResolution { target, code, .. } = resolver.resolve(
                file,
                directive.uri.as_deref(),
                dartq_resolution::DirectiveKind::ImportExport,
            );
            directive.target = target;
            directive.code = code;
        }
    }
    for directive in &facts.directives {
        if let Some(code) = directive.code {
            facts.diagnostics.push(uri_diagnostic(
                code,
                directive.uri.as_deref().unwrap_or_default(),
                directive.uri_span,
                file,
            ));
        }
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

/// `import`/`export`: uri, `[deferred] [as p]` (import shapes only in
/// practice, tolerated on both), combinator list, `;`.
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
    let mut clauses: Vec<CombinatorClause> = Vec::new();
    loop {
        match kind_at(tokens, i) {
            None | Some(TokenKind::Eof) => break,
            Some(TokenKind::Semicolon) => {
                i += 1;
                break;
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
                while let Some(next) = kind_at(tokens, i) {
                    match next {
                        TokenKind::Identifier => {
                            names.push(strings.intern(tokens.lexeme(text, i)));
                            i += 1;
                        }
                        TokenKind::Comma => i += 1,
                        _ => break,
                    }
                }
                clauses.push(if hide {
                    CombinatorClause::Hide(names.into())
                } else {
                    CombinatorClause::Show(names.into())
                });
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
        combinators: Combinators::new(clauses, strings),
        name: None,
    });
    i
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
// Library reformation + cascade closure
// ---------------------------------------------------------------------------

/// Rebuilds the library set from residency, recomputes reverse import edges,
/// and derives the post-closure dirty set: seeds are libraries with no
/// cached result or a changed identity — member contents, dependency set,
/// and the unclaimed part targets (§3.2 steps 3–5). Old and new identities
/// are exact lists, so every membership, edge, or part-target residency
/// change shows up as a difference on the library that owns it; no separate
/// directive diff is needed.
///
/// The importer closure over that set is signature-gated (L2, PRD §11.3):
/// it fans out only from `sig_changed` files' owners and the live former
/// owners of removed files. Returns the invalidated set and whether any
/// library was retired.
fn reform_and_close(
    state: &mut EngineState,
    sig_changed: &[FileId],
    changed_pre_owners: &[(FileId, Option<LibraryId>)],
    removed_owners: &[Option<LibraryId>],
) -> (Vec<LibraryId>, bool) {
    let retired_any = reform_libraries(state);
    let invalidated = close(state, sig_changed, changed_pre_owners, removed_owners);
    (invalidated, retired_any)
}

/// Rebuilds `libraries`, `library_of_file`, and `importers` from residency.
///
/// Formation: defining files (no `part of`) each own a library keyed on
/// their canonical URI; their `part` targets that are themselves part files
/// join in directive order (first claim wins on conflicts, precedence by
/// interned path bytes — the oracle's path-smallest claimant;
/// `duplicate_part` is a diagnostic ticket's business). Part
/// files claimed by nobody become singleton libraries. Every resident file
/// belongs to exactly one library (engine-api-spec §1.2). Returns whether
/// any library was retired (a structure-graph input).
fn reform_libraries(state: &mut EngineState) -> bool {
    // Canonical URIs are a pure function of the interned path; memoize per
    // file (a file's path never changes once interned).
    let mut resident: Vec<FileId> = state.residency.keys().copied().collect();
    resident.sort_unstable_by_key(|file| file.index());
    for file in &resident {
        if !state.canonical_uris.contains_key(file) {
            let path = state
                .files
                .path(*file)
                .map_or_else(std::path::PathBuf::new, std::path::Path::to_path_buf);
            // canonical_uri never probes existence (state.rs face contract):
            // the resident filter is inert here, pass the documented no-probe
            // closure (pre-#287 this call site had no filter at all).
            let uri = state.uri_resolver(&|_| false).canonical_uri(&path);
            state.canonical_uris.insert(*file, Arc::from(uri));
        }
    }

    // Pass A: defining libraries. Claim precedence is the interned-path byte
    // order (the oracle picks the path-smallest claiming library,
    // file_state.dart:2176-2192). Both prior keyings would fork that winner:
    // `FileId` interning order differs between the incremental and full arms
    // when files are created mid-sequence, and canonical-URI bytes reorder
    // percent-encoded paths (a space sorts below `!` as a path, above it as
    // `%20` in a URI) — so the sort keys on the path itself, the FileTable's
    // own universe (#287), stable across arms.
    let mut defining: Vec<(FileId, LibraryId)> = Vec::new();
    let mut is_part: FxHashSet<FileId> = FxHashSet::default();
    for file in &resident {
        if state.residency[file].is_part() {
            is_part.insert(*file);
        } else {
            let id = state.libraries.intern(&state.canonical_uris[file]);
            defining.push((*file, id));
        }
    }
    defining.sort_unstable_by(|a, b| {
        let path_bytes = |file: FileId| {
            state
                .files
                .path(file)
                .map_or(&[][..], |path| path.as_os_str().as_encoded_bytes())
        };
        path_bytes(a.0).cmp(path_bytes(b.0))
    });

    // Pass B: attach parts (directive order, first claim wins). Belonging
    // follows the part-of form (analyzer file_state.dart:2087-2266): a
    // URI-form part file belongs to the includer its `part of` URI resolves
    // to (`PartOfUriKnownFileKind`); a name-form part file joins an includer
    // only when the includer's declared library name equals the part-of name
    // (`PartOfNameFileKind::isPartOf`, file_state.dart:2222-2225), bounded to
    // one package — the M1 stand-in for the oracle's sibling-directory
    // candidate discovery (incremental-l1-spec §7.1). A name-form file with
    // no qualifying includer stays unclaimed and falls through to the orphan
    // singletons of Pass C (the oracle's `library` = null face).
    let mut claims: FxHashMap<FileId, LibraryId> = FxHashMap::default();
    let mut parts_of: FxHashMap<LibraryId, Vec<FileId>> = FxHashMap::default();
    for (defining_file, lib) in &defining {
        let library_name = declared_library_name(&state.residency[defining_file]);
        for directive in &state.residency[defining_file].directives {
            if directive.kind != DirectiveKind::Part {
                continue;
            }
            if let ResolvedTarget::Resolved(target) = directive.target
                && is_part.contains(&target)
                && state.residency.contains_key(&target)
                && !claims.contains_key(&target)
                && may_claim(state, *defining_file, library_name, target)
            {
                claims.insert(target, *lib);
                parts_of.entry(*lib).or_default().push(target);
            }
        }
    }

    // Pass C: orphaned part files become singleton libraries.
    for file in &resident {
        if is_part.contains(file) && !claims.contains_key(file) {
            let id = state.libraries.intern(&state.canonical_uris[file]);
            defining.push((*file, id));
        }
    }

    // File membership (every resident file, exactly one library).
    let mut library_of_file: FxHashMap<FileId, LibraryId> = FxHashMap::default();
    for (file, lib) in &defining {
        library_of_file.insert(*file, *lib);
    }
    for (file, lib) in &claims {
        library_of_file.insert(*file, *lib);
    }

    // Pass D: content identity (defining + parts, directive order) and
    // dependency set (resolved import/export targets' owning libraries).
    let mut infos: Vec<(LibraryId, LibraryInfo)> = Vec::with_capacity(defining.len());
    for (file, lib) in &defining {
        let parts = parts_of
            .get(lib)
            .into_iter()
            .flatten()
            .copied()
            .collect::<Vec<_>>();
        let mut members: Vec<FileId> = vec![*file];
        members.extend_from_slice(&parts);
        let content: Box<[ContentId]> = members
            .iter()
            .map(|member| state.residency[member].content)
            .collect();
        let mut deps: Vec<LibraryId> = Vec::new();
        for member in &members {
            for directive in &state.residency[member].directives {
                if matches!(
                    directive.kind,
                    DirectiveKind::Import | DirectiveKind::Export
                ) && let ResolvedTarget::Resolved(target) = directive.target
                    && let Some(dep) = library_of_file.get(&target)
                    && *dep != *lib
                {
                    deps.push(*dep);
                }
            }
        }
        deps.sort_unstable();
        deps.dedup();
        // Part directives resolving to a resident file outside membership:
        // neither content (membership) nor deps (import/export only) sees
        // them, so their residency flip is carried by the identity
        // explicitly. Without it, removing such a target leaves the library
        // clean while its cached result predates the URI going dangling —
        // incremental diverges from a full recompute, which bakes
        // `uri_does_not_exist` against the absent target.
        let mut part_targets: Vec<FileId> = Vec::new();
        for member in &members {
            for directive in &state.residency[member].directives {
                if directive.kind == DirectiveKind::Part
                    && let ResolvedTarget::Resolved(target) = directive.target
                    && state.residency.contains_key(&target)
                    // O(1) membership: the defining file itself and every
                    // target this library claimed are members; any other
                    // resident target is an unclaimed part target.
                    && target != *file
                    && claims.get(&target).copied() != Some(*lib)
                {
                    part_targets.push(target);
                }
            }
        }
        part_targets.sort_unstable_by_key(|file| file.index());
        part_targets.dedup();
        infos.push((
            *lib,
            LibraryInfo {
                // The memo's allocation, shared: a refcount per defining
                // library per apply, never a copy of the URI bytes.
                uri: Arc::clone(&state.canonical_uris[file]),
                defining: *file,
                parts: parts.into(),
                content,
                deps,
                part_targets: part_targets.into(),
            },
        ));
    }

    // Reverse edges from the new directive set.
    let mut importers: FxHashMap<FileId, FxHashSet<LibraryId>> = FxHashMap::default();
    for (file, lib) in &library_of_file {
        for directive in &state.residency[file].directives {
            if matches!(
                directive.kind,
                DirectiveKind::Import | DirectiveKind::Export
            ) && let ResolvedTarget::Resolved(target) = directive.target
                && state.residency.contains_key(&target)
            {
                importers.entry(target).or_default().insert(*lib);
            }
        }
    }

    // Write back: retire gone libraries (their results go with them),
    // replace infos, and swap the derived maps.
    let live: FxHashSet<LibraryId> = defining.iter().map(|(_, lib)| *lib).collect();
    let retired = state.libraries.retire_except(&live);
    let retired_any = !retired.is_empty();
    for id in retired {
        state.results.remove(&id);
        // The reference surface goes with the retired library; a returning
        // (recycled) id rebuilds it at its next LibraryAnalyzed point.
        state.library_surfaces.remove(&id);
    }
    for (id, info) in infos {
        state.libraries.set_info(id, info);
    }
    state.library_of_file = library_of_file;
    state.importers = importers;
    retired_any
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
/// libraries with no cached result or changed identity, plus the importer
/// closure fanned out **only from the propagation seeds** — the owners of
/// `sig_changed` files, both live owners of a changed file whose membership
/// moved (the pre-reform owner is the losing side of a `part of` detach),
/// libraries whose part-target identity moved (a claim flip between
/// competitor libraries), and the live former owners of removed files. From
/// a seed the closure passes unconditionally through every importer (the
/// re-export chain D←L←M: M's face moves, L re-exports it unchanged, D must
/// still recompute); a library that is only identity-stale (a body edit
/// under a stable face) recomputes alone and does not propagate. A retired
/// former owner needs no seed: its direct importers go stale by identity
/// (the dropped dependency), and their own faces did not move.
fn close(
    state: &mut EngineState,
    sig_changed: &[FileId],
    changed_pre_owners: &[(FileId, Option<LibraryId>)],
    removed_owners: &[Option<LibraryId>],
) -> Vec<LibraryId> {
    let mut dirty: FxHashSet<LibraryId> = FxHashSet::default();
    let mut queued: FxHashSet<LibraryId> = FxHashSet::default();
    let mut work: Vec<LibraryId> = Vec::new();
    // Cancelled-analyze leftovers carry forward (#428): reform_libraries has
    // already run, so filtering on live info is the post-reform live set. A
    // leftover invalidated by a dependency's face move is identity-fresh (the
    // pass below would drop it), and the retired ids among the leftovers must
    // not re-enter — analyze unwraps every queued id's info, and a freelist-
    // recycled id (#230) aliases a fresh tenant that, if it reached here at
    // all, is result-less and already dirty. Leftovers seed `dirty` only,
    // never the fan-out: the cancelled run's dirty set was the full
    // transitive closure of its own close, so every still-invalid importer
    // is already among them.
    for lib in std::mem::take(&mut state.invalidated) {
        if state.libraries.info(lib).is_some() {
            dirty.insert(lib);
        }
    }
    for lib in state.libraries.live() {
        let info = state.libraries.info(lib).expect("live library has info");
        let stale = match state.results.get(&lib) {
            None => true,
            Some(result) => {
                result.content != info.content
                    || result.deps != info.deps
                    || result.part_targets != info.part_targets
            }
        };
        if stale {
            dirty.insert(lib);
        }
        // Membership-change seed (transfer shape): the part-target identity
        // moves exactly when a resolved `part` target flipped claim state or
        // residency. The gaining library's own files are unchanged and the
        // moved member carries no signature, so no other seed reaches it —
        // yet its namespace changed, and its identity-clean importers would
        // keep stale facts.
        if let Some(result) = state.results.get(&lib)
            && result.part_targets != info.part_targets
            && queued.insert(lib)
        {
            work.push(lib);
        }
    }
    // Signature-gated propagation seeds. All categories are identity-stale
    // by construction (a signature move implies a content move; a membership
    // move implies an identity move), so the identity pass above already
    // invalidated them — seeding only starts the fan-out, deduplicated by
    // `queued` so each library propagates once. Retired owners are skipped:
    // their importers went stale through the dropped dependency instead.
    for file in sig_changed {
        if let Some(&lib) = state.library_of_file.get(file)
            && state.libraries.info(lib).is_some()
            && queued.insert(lib)
        {
            work.push(lib);
        }
    }
    // Membership-change seed (detach shape): a changed file whose owner
    // differs between the pre- and post-reform membership maps moved between
    // libraries. The losing owner is identity-stale (it dropped a member)
    // but identity-clean importers ride its seed; the winning owner is
    // usually already seeded through its member's own signature move.
    for (file, pre_owner) in changed_pre_owners {
        let post_owner = state.library_of_file.get(file).copied();
        if post_owner != *pre_owner {
            for owner in [*pre_owner, post_owner].into_iter().flatten() {
                if state.libraries.info(owner).is_some() && queued.insert(owner) {
                    work.push(owner);
                }
            }
        }
    }
    for owner in removed_owners.iter().flatten().copied() {
        if state.libraries.info(owner).is_some() && queued.insert(owner) {
            work.push(owner);
        }
    }
    while let Some(lib) = work.pop() {
        let info = state.libraries.info(lib).expect("seed is live");
        for file in std::iter::once(info.defining).chain(info.parts.iter().copied()) {
            if let Some(importer_set) = state.importers.get(&file) {
                for importer in importer_set {
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
            EngineState::new(
                self.packages.clone(),
                &self.config,
                &self.config_dir,
                SdkSnapshot::load(&self.root.join("sdk"), None).expect("synthetic SDK loads"),
                crate::memory_store(),
                "vm".to_owned(),
                FxHashMap::default(),
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
}
