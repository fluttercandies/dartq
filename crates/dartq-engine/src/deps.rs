//! Lazy, import-driven indexing of dependency libraries outside the
//! analyzed workspace (the pub cache): when a live library's `package:`
//! import resolves to a file no package root's discovery made resident,
//! its interface facts load on demand so the workspace's own rows resolve
//! the same faces the oracle sees — `class DelegatingList implements List`
//! in a dependency becomes a real supertype edge, and the override/
//! assignability families that consult it fire in the workspace files.
//!
//! # Shape
//!
//! The graph build ([`crate::pipeline::build_library_graph`]) walks the
//! live libraries' import/export targets: a resolved target outside
//! residency whose package is not workspace source seeds a bounded BFS —
//! the target file reads from disk, runs the shared per-file pipeline
//! ([`crate::l1::compute_unit`]: scan, one parse, element collection),
//! and its own import/export directives resolve for the next frontier
//! ([`MAX_DEPTH`] hops from the workspace — deep enough for the re-export
//! chains pub entry points are, never deeper). Each loaded file registers
//! as a graph node beside the synthetic `dart:` faces: its containers
//! declare, its edges point at SDK/dep/workspace nodes, and its element
//! model joins the analyze's shared element index — but it is never an
//! engine library, so it yields zero diagnostic rows of its own (the
//! differential counts workspace files only; the dep's job is to make its
//! class/member/clause faces resolvable).
//!
//! # Boundedness
//!
//! Loaded units cache in engine state keyed by the interned canonical
//! path (the [`FileId`] is that path's identity — same key, no second
//! table), capped at [`DEP_FILE_CAP`] files per engine; past the cap the
//! skip is one stderr note. Imports past the depth bound and unreadable
//! targets count in the debug counters. Missing configs, missing files,
//! and unreadable bytes stay silent — no new diagnostics — exactly the
//! posture a broken or absent dependency had before this layer.

use std::path::Path;
use std::sync::Arc;

use dartq_package::SourceCategory;
use dartq_resolution::{DirectiveKind as UriDirectiveKind, ResolvedTarget, UriResolver};
use dartq_source::{FileId, SourceText};
use rustc_hash::FxHashMap;

use crate::l1;
use crate::state::{DirectiveKind, EngineState, ScannedFace, UnitFacts};

/// The total dependency-file cap per engine. Each cached unit is one
/// dep file's facts (element model + directives + scan face) — a few KB
/// for a typical package file — so the cap holds the layer to a bounded
/// slice of the dependency closure however wide the import graph is.
pub(crate) const DEP_FILE_CAP: usize = 512;

/// The depth bound from any workspace file: depth 1 is a workspace
/// import/export target, depth 2 a dep file's own target. Two hops cover
/// the re-export shape of pub entry points (`package:x/x.dart` exporting
/// `src/impl.dart`); a third hop's target is skipped with a counted note.
const MAX_DEPTH: u8 = 2;

/// One indexed dependency file: the interface facts (its diagnostics are
/// dead weight — the file is never an engine library and never folds), the
/// scan face its clause-tree capture parses through, and the resolved
/// target per directive (index-aligned with [`UnitFacts::directives`]) the
/// graph build maps onto SDK/dep/workspace nodes.
pub(crate) struct DepUnit {
    pub facts: Arc<UnitFacts>,
    pub scanned: Arc<ScannedFace>,
    pub targets: Box<[ResolvedTarget]>,
}

/// The lazy dependency cache plus its skip counters — engine-state
/// resident, rebuilt never (the units re-derive only through a fresh
/// engine; pub-cache content is session-stable).
#[derive(Default)]
pub(crate) struct DepIndex {
    /// The indexed units by interned canonical path.
    pub units: FxHashMap<FileId, DepUnit>,
    /// Debug counter: targets past the depth bound.
    pub depth_skips: u64,
    /// Debug counter: unreadable or missing dep files.
    pub load_failures: u64,
    /// The one-shot stderr note's tripwire for cap overflows.
    pub cap_noted: bool,
    /// The loaded SDK libraries by `dart:` name → their files (the entry
    /// first, then its parts), every one a unit in `units`
    /// ([`crate::sdk_units`]).
    pub sdk_libraries: FxHashMap<String, Vec<FileId>>,
    /// How many of `units` are SDK files — exempt from [`DEP_FILE_CAP`].
    pub sdk_file_count: usize,
}

/// Seeds and drains the dependency closure for the current live libraries.
/// Called at every graph build; cached units reuse, so a warm graph
/// rebuild pays only the seed scan (O(live directives)).
pub(crate) fn load_dep_closure(state: &mut EngineState) {
    if state.residency.is_empty() {
        return;
    }
    // Residency wins: a dep file the caller later fed as content (a daemon
    // opening a pub-cache path) left the dep face — its stale cached unit
    // would double-register the file's graph node.
    state
        .deps
        .units
        .retain(|file, _| !state.residency.contains_key(file));
    crate::sdk_units::load_sdk_units(state);
    let mut queue: Vec<(FileId, u8)> = Vec::new();
    for library in state.libraries.live() {
        let Some(info) = state.libraries.info(library) else {
            continue;
        };
        for file in std::iter::once(info.defining).chain(info.parts.iter().copied()) {
            let Some(facts) = state.residency.get(&file) else {
                continue;
            };
            for directive in &facts.directives {
                if !matches!(directive.kind, DirectiveKind::Import | DirectiveKind::Export) {
                    continue;
                }
                let ResolvedTarget::Resolved(target) = directive.target else {
                    continue;
                };
                // Workspace-entry shape only: a `package:` URI naming a
                // non-workspace package whose file discovery never made
                // resident. Relative targets and workspace-package files
                // keep the pre-existing postures.
                if !directive
                    .uri
                    .as_deref()
                    .is_some_and(|uri| uri.starts_with("package:"))
                {
                    continue;
                }
                if state.residency.contains_key(&target) || state.deps.units.contains_key(&target)
                {
                    continue;
                }
                if dep_candidate(state, target) {
                    queue.push((target, 1));
                }
            }
        }
    }
    while let Some((file, depth)) = queue.pop() {
        if state.deps.units.contains_key(&file) || state.residency.contains_key(&file) {
            continue;
        }
        if state.deps.units.len() - state.deps.sdk_file_count >= DEP_FILE_CAP {
            if !state.deps.cap_noted {
                state.deps.cap_noted = true;
                eprintln!(
                    "dartq: dependency interface index capped at {DEP_FILE_CAP} files; \
                     further dependency files are skipped"
                );
            }
            continue;
        }
        queue.extend(load_dep_unit(state, file, depth));
    }
}

/// Whether `file` may join the dependency index: anything that is not
/// workspace source. Path-category packages (the root, members, path
/// dependencies) are workspace source — their files enter residency
/// through discovery, and one that did not (an excluded file) keeps that
/// posture instead of silently re-entering through the dep face. Hosted/
/// git cache content and SDK-provided packages — and paths under no
/// package root at all — index.
fn dep_candidate(state: &EngineState, file: FileId) -> bool {
    let Some(path) = state.files.path(file) else {
        return false;
    };
    match state.packages.containing(path) {
        Some(identity) => identity.category != SourceCategory::Path,
        None => true,
    }
}

/// Loads one dep file: disk read, the shared per-file pipeline, directive
/// resolution for the graph edges and the next frontier. Returns the
/// within-bound targets not already resident or indexed. Every failure
/// mode (no path, unreadable, missing bytes) counts and skips silently.
fn load_dep_unit(state: &mut EngineState, file: FileId, depth: u8) -> Vec<(FileId, u8)> {
    let load_failure = |state: &mut EngineState| {
        state.deps.load_failures += 1;
        Vec::new()
    };
    let Some(path) = state.files.path(file).map(Path::to_path_buf) else {
        return load_failure(state);
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return load_failure(state);
    };
    let text = SourceText::canonical(&bytes);
    let content = state.store.put_text(&text);
    let version = l1::language_version_for(&state.packages, &state.files, file);
    let (facts, _capture, scanned) = l1::compute_unit(
        file,
        content,
        text.as_str(),
        version,
        &state.strings,
        &state.grants(),
    );
    let facts = Arc::new(facts);
    // Phase 1 — the dep file's directive targets, resolved while the
    // resolver holds its borrows (the facts are deliberately
    // pre-resolution — resolving into them would bake URI diagnostics a
    // dep file must never report).
    let resident = |id: FileId| state.residency.contains_key(&id);
    let mut resolver = UriResolver::new(
        &state.uri_packages,
        &state.sdk,
        &state.declared_variables,
        &mut state.files,
        &resident,
    );
    let mut targets = Vec::with_capacity(facts.directives.len());
    for directive in &facts.directives {
        if !matches!(directive.kind, DirectiveKind::Import | DirectiveKind::Export) {
            targets.push(ResolvedTarget::Missing);
            continue;
        }
        let resolution =
            resolver.resolve(file, directive.uri.as_deref(), UriDirectiveKind::ImportExport);
        targets.push(resolution.target);
    }
    drop(resolver);
    // Phase 2 — the depth-bounded frontier over the resolved targets: the
    // whole-state checks (residency, cache membership, workspace-package
    // classification) run with the resolver's borrows gone.
    let mut next = Vec::new();
    for dependency in targets.iter().filter_map(|target| match target {
        ResolvedTarget::Resolved(file) => Some(*file),
        _ => None,
    }) {
        if state.residency.contains_key(&dependency) || state.deps.units.contains_key(&dependency)
        {
            continue;
        }
        if !dep_candidate(state, dependency) {
            continue;
        }
        if depth >= MAX_DEPTH {
            state.deps.depth_skips += 1;
        } else {
            next.push((dependency, depth + 1));
        }
    }
    let scanned = Arc::new(ScannedFace {
        tokens: scanned.tokens,
        features: scanned.features,
        version: scanned.version,
    });
    state.deps.units.insert(
        file,
        DepUnit {
            facts,
            scanned,
            targets: targets.into_boxed_slice(),
        },
    );
    next
}
