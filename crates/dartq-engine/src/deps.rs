//! Lazy, import-driven indexing of dependency libraries outside the
//! analyzed workspace (the pub cache, SDK-provided packages, path
//! dependencies pointing outside the workspace tree): when a live
//! library's `package:` import resolves to a file no package root's
//! discovery made resident, its interface facts load on demand so the
//! workspace's own rows resolve the same faces the oracle sees — `class
//! DelegatingList implements List` in a dependency becomes a real
//! supertype edge, and the override/assignability families that consult
//! it fire in the workspace files.
//!
//! # Shape
//!
//! The graph build ([`crate::pipeline::build_library_graph`]) walks the
//! live libraries' import/export targets: a resolved target outside
//! residency whose package is outside the workspace bound seeds a BFS —
//! the target file reads from disk, runs the shared per-file pipeline
//! ([`crate::l1::compute_unit`]: scan, one parse, element collection),
//! and its own import/export directives resolve for the next frontier.
//! The walk is closure-complete like the oracle's (the analyzer resolves
//! a `package:` import's whole transitive closure, `dart analyze` never
//! bounds it by hops); termination is structural — the visited set only
//! grows, each queue entry either loads a unit or finds it loaded, and
//! the file cap bounds the open set. Each loaded file registers as a
//! graph node beside the synthetic `dart:` faces: its containers declare,
//! its edges point at SDK/dep/workspace nodes, and its element model
//! joins the analyze's shared element index — but it is never an engine
//! library, so it yields zero diagnostic rows of its own (the
//! differential counts workspace files only; the dep's job is to make its
//! class/member/clause faces resolvable).
//!
//! # Boundedness
//!
//! Loaded units cache in engine state keyed by the interned canonical
//! path (the [`FileId`] is that path's identity — same key, no second
//! table), capped at [`DEP_FILE_CAP`] files per engine; past the cap the
//! skip is one stderr note. Unreadable targets count in the debug
//! counters. Missing configs, missing files, and unreadable bytes stay
//! silent — no new diagnostics — exactly the posture a broken or absent
//! dependency had before this layer. The retained per-unit payload is the
//! interface face (element model, directives, resolved targets, line
//! index) plus the source bytes in the content store; the scan face's
//! token table is deliberately NOT retained (≈20 B/token — the dominant
//! per-file cost at closure scale, and a pure function of the bytes): the
//! one consumer that needs tokens (the clause-tree capture) re-scans the
//! retained text on demand.

use std::path::Path;
use std::sync::Arc;

use dartq_index::{FileCapture, Postings, distinct_decl_names};
use dartq_language::LanguageVersion;
use dartq_resolution::{DirectiveKind as UriDirectiveKind, ResolvedTarget, UriResolver};
use dartq_source::{FileId, SourceText};
use rustc_hash::FxHashMap;

use crate::l1;
use crate::state::{DirectiveKind, EngineState, UnitFacts};

/// The total dependency-file cap per engine. Each cached unit is one dep
/// file's interface face (element model + declaration surface + directives +
/// resolved targets + line index) plus its bytes in the content store — the
/// closure of a real application (a Flutter widget tree's interface closure
/// runs to a couple of thousand files) fits with headroom while the cap
/// keeps the layer a bounded slice of memory however wide the import graph
/// is.
pub(crate) const DEP_FILE_CAP: usize = 4096;

/// One indexed dependency file: the interface facts (its diagnostics are
/// dead weight — the file is never an engine library and never folds), the
/// sites-shed declaration capture (the [`FileCapture::without_sites`]
/// twin: the name/kind/span rows the query joins resolve cross-package
/// targets against — `rename` on a use whose declaration sits in the
/// dependency must find the declaration to name its edit, and a
/// `query symbol` must list it), the settled language version (the
/// clause-tree capture's re-scan key), and the resolved target per
/// directive (index-aligned with [`UnitFacts::directives`]) the graph
/// build maps onto SDK/dep/workspace nodes.
pub(crate) struct DepUnit {
    pub facts: Arc<UnitFacts>,
    pub capture: Arc<FileCapture>,
    pub version: LanguageVersion,
    pub targets: Box<[ResolvedTarget]>,
}

/// The lazy dependency cache plus its skip counters — engine-state
/// resident, rebuilt never (the units re-derive only through a fresh
/// engine; pub-cache content is session-stable).
#[derive(Default)]
pub(crate) struct DepIndex {
    /// The indexed units by interned canonical path.
    pub units: FxHashMap<FileId, DepUnit>,
    /// The declaration-name directory over the units' captures — the dep
    /// tier's twin of the resident `decl_postings`, keyed and consulted the
    /// same way (name → declaring files). Entries unlist when a unit leaves
    /// the index (the residency-wins retain), so the map stays exact.
    pub decl_postings: Postings<FileId>,
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

impl DepIndex {
    /// Registers a loaded unit: the unit under its file, the declaration
    /// names into the postings. The single insert seam both loaders share
    /// (the pub-cache closure and the SDK sources).
    pub(crate) fn insert(&mut self, file: FileId, unit: DepUnit) {
        for name in distinct_decl_names(&unit.capture.index) {
            self.decl_postings.list(name, file);
        }
        self.units.insert(file, unit);
    }

    /// Removes a unit that residency superseded (a file the caller later
    /// fed as content), unlisting its declaration names with it.
    fn remove(&mut self, file: FileId) -> Option<DepUnit> {
        let unit = self.units.remove(&file)?;
        for name in distinct_decl_names(&unit.capture.index) {
            self.decl_postings.unlist(name, file);
        }
        Some(unit)
    }
}

/// Seeds and drains the dependency closure for the current live libraries.
/// Called at every graph build; cached units reuse, so a warm graph
/// rebuild pays only the seed scan (O(live directives)). The frontier is
/// closure-complete (no depth bound — the oracle's resolution face); the
/// visited set makes it structurally terminating and the cap bounds the
/// resident unit count.
pub(crate) fn load_dep_closure(state: &mut EngineState) {
    if state.residency.is_empty() {
        return;
    }
    // Residency wins: a dep file the caller later fed as content (a daemon
    // opening a pub-cache path) left the dep face — its stale cached unit
    // would double-register the file's graph node. The eviction unlists the
    // declaration names with the unit, keeping the postings exact.
    let evicted: Vec<FileId> = state
        .deps
        .units
        .keys()
        .filter(|file| state.residency.contains_key(file))
        .copied()
        .collect();
    for file in evicted {
        state.deps.remove(file);
    }
    crate::sdk_units::load_sdk_units(state);
    let mut queue: Vec<FileId> = Vec::new();
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
                // package whose file discovery never made resident. Relative
                // targets and in-bound package files keep the pre-existing
                // postures.
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
                    queue.push(target);
                }
            }
        }
    }
    while let Some(file) = queue.pop() {
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
        queue.extend(load_dep_unit(state, file));
    }
}

/// Whether `file` may join the dependency index: anything outside the
/// analyzed workspace. The bound is the same one discovery walks — a
/// package root at or beneath the workspace root is analyzed surface
/// (its files enter residency through discovery, and one that did not —
/// an options-excluded file — keeps that posture instead of silently
/// re-entering through the dep face); a package outside the bound (the
/// pub cache, a git checkout, an SDK-provided package, a path dependency
/// pointing elsewhere) and a path under no package root at all index.
fn dep_candidate(state: &EngineState, file: FileId) -> bool {
    let Some(path) = state.files.path(file) else {
        return false;
    };
    match state.packages.containing(path) {
        Some(identity) => !identity.root.starts_with(state.packages.workspace_root()),
        None => true,
    }
}

/// Loads one dep file: disk read, the shared per-file pipeline, directive
/// resolution for the graph edges and the next frontier. Returns the
/// not-already-indexed targets. Every failure mode (no path, unreadable,
/// missing bytes) counts and skips silently.
fn load_dep_unit(state: &mut EngineState, file: FileId) -> Vec<FileId> {
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
    let (facts, capture, scanned) = l1::compute_unit(
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
    // Phase 2 — the frontier over the resolved targets: the whole-state
    // checks (residency, cache membership, workspace-bound classification)
    // run with the resolver's borrows gone.
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
        next.push(dependency);
    }
    state.deps.insert(
        file,
        DepUnit {
            facts,
            capture: Arc::new(capture.without_sites()),
            version: scanned.version,
            targets: targets.into_boxed_slice(),
        },
    );
    next
}
