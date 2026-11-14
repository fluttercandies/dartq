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
//! residency that discovery cannot make resident — its package sits outside
//! the workspace bound, or beneath a dot-prefixed directory discovery never
//! descends — seeds a BFS —
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
//! index, declaration capture) plus the written supertype-clause trees
//! the analyze-time merge consumes. Neither the source bytes nor the scan
//! face are retained: both are pure functions of the file on disk, and
//! the two consumers that needed them (the clause capture and the query
//! faces' source reads) are served by a load-time derived product and an
//! on-demand disk read respectively. This is the #4217 seam: at cockpit's
//! closure scale the retained source text alone accounted for ≈52 MB.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use dartq_index::{FileCapture, Postings, distinct_decl_names};
use dartq_language::LanguageVersion;
use dartq_resolution::{
    DirectiveKind as UriDirectiveKind, PackageTable as UriPackageTable, ResolvedTarget, UriResolver,
};
use dartq_scanner::{ScanConfig, scan};
use dartq_sdk::SdkSnapshot;
use dartq_source::{FileId, FileTable, SourceText, StringTable, normalize_path};
use rayon::prelude::*;
use rustc_hash::FxHashMap;

use crate::l1;
use crate::state::{DirectiveFacts, DirectiveKind, EngineState, UnitFacts};

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
/// declaration-only capture (the name/kind/span rows the query joins
/// resolve cross-package targets against — `rename` on a use whose
/// declaration sits in the dependency must find the declaration to name
/// its edit, and a `query symbol` must list it; the raw reference-site
/// columns are never built, see [`l1::compute_dep_unit`]), the settled
/// language version, the resolved target per directive (index-aligned
/// with [`UnitFacts::directives`]) the graph build maps onto
/// SDK/dep/workspace nodes, and the written supertype-clause trees of the
/// unit's containers (captured once at load —
/// [`l1::capture_dep_clauses`] — the analyze-time merge's input; the
/// source bytes they were derived from are not retained).
pub(crate) struct DepUnit {
    pub facts: Arc<UnitFacts>,
    pub capture: Arc<FileCapture>,
    pub version: LanguageVersion,
    pub targets: Box<[ResolvedTarget]>,
    /// The unit's written supertype/parameter-bound trees, keyed by this
    /// unit's `FileId` — the derived product the analyze-time clause
    /// merge reads instead of re-scanning retained source.
    pub clauses: dartq_semantic::ClauseTrees,
    /// The analyzed source bytes, loaded lazily on first read (the
    /// engine's `content` face dependency arm: query faces like the rename
    /// plan's spelling check read a dep file's text). Every load attempt
    /// is memoized — including a failed one — so a vanished file costs one
    /// failed read, never one per call. A dependency file is immutable for
    /// the engine's life (the pub-cache closure's session-stability
    /// contract), so the lazy read reproduces the bytes the unit was
    /// computed from.
    source: OnceLock<Option<SourceText>>,
}

impl DepUnit {
    /// The one constructor: the loaders differ in how they compute the
    /// pieces, never in the unit's shape.
    pub(crate) fn new(
        facts: Arc<UnitFacts>,
        capture: FileCapture,
        version: LanguageVersion,
        targets: Box<[ResolvedTarget]>,
        clauses: dartq_semantic::ClauseTrees,
    ) -> Self {
        Self {
            facts,
            capture: Arc::new(capture),
            version,
            targets,
            clauses,
            source: OnceLock::new(),
        }
    }

    /// The unit's source bytes; `path` is the interned dependency path
    /// (the loader's own read key). See [`Self::source`].
    pub(crate) fn source_bytes(&self, path: Option<&Path>) -> Option<&[u8]> {
        self.source
            .get_or_init(|| {
                let bytes = std::fs::read(path?).ok()?;
                Some(SourceText::canonical(&bytes))
            })
            .as_ref()
            .map(SourceText::as_bytes)
    }
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

    /// The dependency tier's resident bytes, split by family — the
    /// RSS-decomposition extension the dep half of the engine answers to.
    /// `exact` rows count owned columns at capacity; `est` rows compose
    /// the per-unit facts the same way [`memory_file_elements.rs`] does
    /// over public accessors, and stay tagged `est` for the `ArgPool`/`Ty`
    /// residue that lens cannot enumerate.
    pub(crate) fn memory_bytes(&self) -> Vec<(&'static str, u64, &'static str)> {
        use std::mem::{size_of, size_of_val};
        let mut facts_shell = 0usize;
        let mut line_index = 0usize;
        let mut directives = 0usize;
        let mut elements = 0usize;
        let mut captures = 0usize;
        let mut targets = 0usize;
        for unit in self.units.values() {
            let facts = &*unit.facts;
            facts_shell += size_of::<crate::state::UnitFacts>();
            line_index += facts.line_index.line_count() as usize * size_of::<u32>();
            directives += facts.directives.capacity() * size_of::<DirectiveFacts>()
                + facts
                    .directives
                    .iter()
                    .map(|d| d.uri.as_ref().map_or(0, |uri| uri.len()))
                    .sum::<usize>();
            let model = &*facts.elements;
            elements += size_of::<dartq_semantic::FileElements>()
                + model
                    .declarations()
                    .map(|(_, decl)| {
                        size_of::<dartq_source::StringId>()
                            + size_of::<dartq_semantic::TopDecl>()
                            + decl.formals.as_deref().map_or(0, |f| {
                                size_of::<dartq_semantic::model::Formals>()
                                    + size_of_val(&*f.positional)
                                    + size_of_val(&*f.named)
                            })
                    })
                    .sum::<usize>()
                + model
                    .containers()
                    .map(|(_, container)| {
                        size_of::<dartq_source::StringId>()
                            + container
                                .members
                                .iter()
                                .map(|(_, entry)| {
                                    size_of::<dartq_source::StringId>()
                                        + size_of::<dartq_semantic::MemberEntry>()
                                        + size_of_val(&*entry.type_param_names)
                                        + entry.formals.as_deref().map_or(0, |f| {
                                            size_of::<dartq_semantic::model::Formals>()
                                                + size_of_val(&*f.positional)
                                                + size_of_val(&*f.named)
                                        })
                                })
                                .sum::<usize>()
                            + container
                                .ctor_formals
                                .iter()
                                .map(|(_, f)| {
                                    size_of::<dartq_source::StringId>()
                                        + size_of::<dartq_semantic::model::Formals>()
                                        + size_of_val(&*f.positional)
                                        + size_of_val(&*f.named)
                                })
                                .sum::<usize>()
                            + (container.with_clause.len()
                                + container.implements_clause.len()
                                + container.on_clause.len()
                                + container.header_residue.len()
                                + container.enum_constants.len())
                                * size_of::<dartq_source::StringId>()
                            + size_of_val(&*container.primary_formals)
                            + size_of_val(&*container.constructors)
                            + size_of_val(&*container.annotations)
                    })
                    .sum::<usize>()
                + model
                    .extension_faces()
                    .iter()
                    .map(|face| {
                        size_of::<dartq_semantic::model::ExtensionFace>()
                            + face
                                .members
                                .iter()
                                .map(|(_, entry)| {
                                    size_of::<dartq_source::StringId>()
                                        + size_of::<dartq_semantic::MemberEntry>()
                                        + size_of_val(&*entry.type_param_names)
                                        + entry.formals.as_deref().map_or(0, |f| {
                                            size_of::<dartq_semantic::model::Formals>()
                                                + size_of_val(&*f.positional)
                                                + size_of_val(&*f.named)
                                        })
                                })
                                .sum::<usize>()
                    })
                    .sum::<usize>();
            captures += size_of::<dartq_index::FileCapture>() + unit.capture.heap_bytes();
            targets += unit.targets.len() * size_of::<ResolvedTarget>();
        }
        let as_u64 = |b: usize| b as u64;
        vec![
            ("dep_unit_facts_shell", as_u64(facts_shell), "exact"),
            ("dep_unit_line_index", as_u64(line_index), "exact"),
            ("dep_unit_directives", as_u64(directives), "exact"),
            ("dep_unit_element_models", as_u64(elements), "est"),
            ("dep_unit_captures", as_u64(captures), "exact"),
            ("dep_unit_targets", as_u64(targets), "exact"),
            // The clause trees are opaque to the engine (dartq-semantic
            // owns the type and exposes no byte walk), so the row is a
            // count: the product is the containers' written supertype and
            // parameter-bound trees only, none of the source they were
            // parsed from.
            ("dep_clause_tree_units", as_u64(self.units.len()), "count"),
            (
                "dep_map_slots",
                as_u64(self.units.capacity() * (size_of::<(FileId, DepUnit)>() + 1)),
                "exact",
            ),
            (
                "dep_decl_postings",
                as_u64(self.decl_postings.bytes()),
                "exact",
            ),
            (
                "dep_sdk_libraries",
                as_u64(
                    self.sdk_libraries.capacity() * (size_of::<(String, Vec<FileId>)>() + 1)
                        + self
                            .sdk_libraries
                            .values()
                            .map(|v| v.capacity() * size_of::<FileId>() + v.len())
                            .sum::<usize>(),
                ),
                "exact",
            ),
            ("dep_unit_entries", as_u64(self.units.len()), "count"),
            ("dep_sdk_files", as_u64(self.sdk_file_count), "count"),
        ]
    }
}

/// Seeds and drains the dependency closure for the current live libraries.
/// Called at every graph build; cached units reuse, so a warm graph
/// rebuild pays only the seed scan (O(live directives)). The frontier is
/// closure-complete (no depth bound — the oracle's resolution face); the
/// visited set makes it structurally terminating and the cap bounds the
/// resident unit count.
///
/// # Shape
///
/// Three phases, mirroring the workspace-face pipeline's fan-out/serial-
/// merge pattern. Phase one is the discovery walk: the serial per-file
/// half (read, scan, directive extraction, URI resolution) fans out over
/// level frontiers — each worker resolves against a *thread-local*
/// [`FileTable`], so the resolved `uri` faces carry real filesystem paths
/// while their `FileId`s stay local garbage. Phase two replays the
/// recorded edges serially in the original stack order, interning every
/// resolved path into the engine's [`FileTable`] in exactly the sequence
/// the serial loader produced — [`FileId`] numbering is byte-identical to
/// the serial walk, so every downstream order keyed on it (postings,
/// query joins, `dep_files` path sorts) is unchanged. Phase three
/// computes the interface units in parallel ([`l1::compute_dep_unit`])
/// and installs them serially in that same order.
pub(crate) fn load_dep_closure(state: &mut EngineState) {
    if state.residency.is_empty() {
        return;
    }
    let seeds = prepare_dep_closure(state);
    // The A/B seam: `DQ_DEPS_SERIAL=1` runs the original stack walk
    // (`load_dep_unit`'s serial BFS) so one binary diffs the two loaders
    // on identical input — unit contents, interning order, timings.
    if std::env::var_os("DQ_DEPS_SERIAL").is_some() {
        load_serial(state, seeds);
    } else {
        load_parallel(state, seeds);
    }
    dump_dep_units(state);
}

/// The shared head of every loader arm: the residency-wins eviction, the
/// SDK unit load, and the seed scan over the live libraries' resolved
/// `package:` import/export targets. Returns the seed [`FileId`]s in
/// discovery order.
fn prepare_dep_closure(state: &mut EngineState) -> Vec<FileId> {
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
    let mut seed_files: Vec<FileId> = Vec::new();
    for library in state.libraries.live() {
        let Some(info) = state.libraries.info(library) else {
            continue;
        };
        for file in std::iter::once(info.defining).chain(info.parts.iter().copied()) {
            let Some(facts) = state.residency.get(&file) else {
                continue;
            };
            for directive in &facts.directives {
                if !matches!(
                    directive.kind,
                    DirectiveKind::Import | DirectiveKind::Export
                ) {
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
                if state.residency.contains_key(&target) || state.deps.units.contains_key(&target) {
                    continue;
                }
                if dep_candidate(state, target) {
                    seed_files.push(target);
                }
            }
        }
    }
    seed_files
}

/// The original serial loader (the `DQ_DEPS_SERIAL` A/B arm and the
/// fallback for frontier pops the probe bound never covered): the stack
/// walk over [`load_dep_unit`].
fn load_serial(state: &mut EngineState, seed_files: Vec<FileId>) {
    let mut queue = seed_files;
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

/// The three-phase parallel loader — see [`load_dep_closure`].
fn load_parallel(state: &mut EngineState, seed_files: Vec<FileId>) {
    // `DQ_DEPS_FULL=1` swaps the interface-only pipeline for the full one:
    // the A/B arm that isolates the lighter tier's win.
    let full = std::env::var_os("DQ_DEPS_FULL").is_some();
    // Phase one — the parallel discovery walk. Each frontier's files read,
    // scan, parse, capture, extract their directives, and resolve the
    // import/export URIs in parallel; a worker's `UriResolver` interns into
    // a thread-local `FileTable` (seeded with the file's own path), so the
    // returned `ResolvedUri` paths are the real filesystem paths while
    // their transient ids stay local. The interface tier's output depends
    // on the engine `FileId` through exactly one column — the capture's
    // `file` attribution — so the unit computes here against a provisional
    // id and the install rewrites that column, keeping the expensive
    // per-file pass fully parallel. `exists` keeps the engine's residency
    // face through the mapping closure.
    let mut probes: FxHashMap<PathBuf, DepProbe> = FxHashMap::default();
    let mut frontier: Vec<PathBuf> = Vec::new();
    {
        let mut seeds_seen = rustc_hash::FxHashSet::default();
        for file in &seed_files {
            if let Some(path) = state.files.path(*file)
                && seeds_seen.insert(path)
            {
                frontier.push(path.to_path_buf());
            }
        }
    }
    // The seen set of every frontier path — a path enters `queued` the
    // moment any level selects it, so siblings that resolve it later in
    // the same level never double-queue it. Frontier entries are canonical
    // spellings (`files.path` output or the workers' `normalize_path`
    // results), so they key the set directly.
    let mut queued: rustc_hash::FxHashSet<PathBuf> = frontier.iter().cloned().collect();
    // The path-level dep-candidate decisions, memoized across the walk: the
    // predicate is a pure function of the static package table, and the
    // replay pays it once per target edge (the flat lib-dir scan is the
    // walk's most repeated lookup). Seeds entered through the same filter,
    // so every path any probe target names has its entry here.
    let mut candidates: FxHashMap<PathBuf, bool> = FxHashMap::default();
    for file in &seed_files {
        if let Some(path) = state.files.path(*file) {
            candidates.insert(path.to_path_buf(), true);
        }
    }
    while !frontier.is_empty() {
        let level: Vec<DepProbe> = frontier
            .par_iter()
            .map(|path| probe_dep(state, path, full))
            .collect();
        let mut next = Vec::new();
        for (path, probe) in frontier.iter().zip(level) {
            for target in &probe.targets {
                let ProbeTarget::Resolved { canonical, .. } = target else {
                    continue;
                };
                if queued.contains(&**canonical) {
                    continue;
                }
                // The push-time filters the serial walk applies inside
                // `load_dep_unit`, minus the `FileId` face: residency and
                // unit membership are enforced on the replay pop (exactly
                // as the serial walk's pop check does), so a resident
                // child's probe is a rare wasted probe, never a wrong
                // edge — and this merge pays no `FileTable` lookup.
                let candidate = *candidates
                    .entry(canonical.to_path_buf())
                    .or_insert_with(|| dep_candidate_path(state, canonical));
                if !candidate {
                    continue;
                }
                // The probe set is bounded by the same file cap the serial
                // walk enforces: a closure wider than the cap must not make
                // the discovery read-and-compute the unbounded tail. A
                // replay pop whose probe the bound cut off falls back to
                // the serial per-file load (see phase two), so the cap's
                // tail stays correct, just unbatched.
                if queued.len() >= DEP_FILE_CAP {
                    continue;
                }
                queued.insert(canonical.to_path_buf());
                next.push(canonical.to_path_buf());
            }
            probes.insert(path.clone(), probe);
        }
        frontier = next;
    }

    // Phase two — the serial interning replay. Popping the recorded edges
    // in the original stack order interns every resolved path into the
    // engine's `FileTable` in exactly the sequence the serial loader
    // produced: `FileId` numbering is byte-identical, and the load-order
    // semantics (pop-time dedup, the cap gate, unreadable targets) follow
    // the serial shape one-to-one. The interning rides `intern_normalized`
    // with the workers' precomputed canonical spellings, so the serial
    // pass performs no filesystem walks of its own.
    let mut order: Vec<FileId> = Vec::new();
    let mut queue: Vec<FileId> = seed_files;
    // The this-walk twin of the pop-time `units` check: units only install
    // in phase three, so the files already ordered stand in for the
    // serial walk's freshly-inserted rows. Files that fail (cap,
    // unreadable) stay unmarked — a duplicate queue entry re-pops them
    // exactly like the serial walk re-attempted its loads.
    let mut loaded: rustc_hash::FxHashSet<FileId> = rustc_hash::FxHashSet::default();
    while let Some(file) = queue.pop() {
        if state.deps.units.contains_key(&file)
            || state.residency.contains_key(&file)
            || loaded.contains(&file)
        {
            continue;
        }
        let dep_count = state.deps.units.len() - state.deps.sdk_file_count + order.len();
        if dep_count >= DEP_FILE_CAP {
            if !state.deps.cap_noted {
                state.deps.cap_noted = true;
                eprintln!(
                    "dartq: dependency interface index capped at {DEP_FILE_CAP} files; \
                     further dependency files are skipped"
                );
            }
            continue;
        }
        let Some(path) = state.files.path(file) else {
            state.deps.load_failures += 1;
            continue;
        };
        // `files.path` returns the interned canonical spelling — the same
        // key the discovery merge stored the probe under.
        let Some(probe) = probes.get_mut(path) else {
            // Beyond the probe bound (a closure wider than the file cap),
            // or a pathless seed: the serial per-file load resolves,
            // interns, and installs in place — the same pop-order
            // interning and failure counting as the original walk. A
            // successful load lands in `deps.units`, so a duplicate queue
            // entry skips on the pop check exactly like the serial walk.
            queue.extend(load_dep_unit(state, file));
            continue;
        };
        if probe.text.is_none() {
            state.deps.load_failures += 1;
            continue;
        }
        loaded.insert(file);
        order.push(file);
        // The intern sequence of `load_dep_unit`: one `files.intern` per
        // resolved directive target, in directive order.
        for target in &mut probe.targets {
            let ProbeTarget::Resolved {
                path: raw,
                canonical,
                id,
            } = target
            else {
                continue;
            };
            let dependency = state.files.intern_normalized(raw, canonical.to_path_buf());
            *id = Some(dependency);
            if state.residency.contains_key(&dependency)
                || state.deps.units.contains_key(&dependency)
            {
                continue;
            }
            // The discovery merge already classified this path (every
            // target canonical passed through it, the seeds through the
            // seed filter); a miss falls back to the predicate itself.
            let candidate = *candidates
                .entry(canonical.to_path_buf())
                .or_insert_with(|| dep_candidate_path(state, canonical));
            if !candidate {
                continue;
            }
            queue.push(dependency);
        }
    }

    // Phase three — the clause-tree capture, then the serial install in
    // DFS order (the order `queue` popped above — the same sequence the
    // serial loader inserted in). The capture fans out over the real
    // interned ids — `ClauseTrees` keys by `FileId`, and phase one
    // computed against worker-local provisional ids — so it runs after
    // phase two's interning and before the install moves each probe's
    // text away. Every unit was computed in phase one; the install
    // rewrites the capture's file attribution to the real `FileId`,
    // registers the unit with its target array, and drops the source
    // bytes: the trees are their only analyze-time consumer's product,
    // and a later query-face read re-derives them on demand.
    let ordered: Vec<(FileId, PathBuf)> = order
        .iter()
        .map(|&file| {
            (
                file,
                state
                    .files
                    .path(file)
                    .expect("queued file interned")
                    .to_path_buf(),
            )
        })
        .collect();
    let clauses: Vec<dartq_semantic::ClauseTrees> = ordered
        .par_iter()
        .map(|(file, path)| {
            let probe = probes
                .get(path)
                .expect("ordered file has a discovery probe");
            let text = probe.text.as_ref().expect("ordered file carries its text");
            let version = probe
                .unit
                .as_ref()
                .expect("ordered file carries its unit")
                .2;
            l1::capture_dep_clauses(
                *file,
                text.as_str(),
                version,
                &state.strings,
                &state.grants(),
            )
        })
        .collect();
    for ((file, path), trees) in ordered.into_iter().zip(clauses) {
        let probe = probes
            .remove(&path)
            .expect("ordered file has a discovery probe");
        let Some((facts, capture, version)) = probe.unit else {
            state.deps.load_failures += 1;
            continue;
        };
        // The resolved target per directive, index-aligned with
        // `facts.directives` — the probe recorded one entry per directive
        // of the same extraction, so the zip is exact.
        debug_assert_eq!(probe.targets.len(), facts.directives.len());
        let mut targets = Vec::with_capacity(probe.targets.len());
        for target in &probe.targets {
            targets.push(match target {
                ProbeTarget::Missing => ResolvedTarget::Missing,
                ProbeTarget::External => ResolvedTarget::External,
                ProbeTarget::Resolved { id, .. } => {
                    ResolvedTarget::Resolved(id.expect("replay interned every target"))
                }
            });
        }
        let mut capture = capture;
        // The one engine-identity column the parallel compute deferred:
        // `compute_dep_unit` ran against the worker-local provisional id.
        capture.file = file;
        state.deps.insert(
            file,
            DepUnit::new(
                Arc::new(facts),
                capture.into_declaration_surface(),
                version,
                targets.into_boxed_slice(),
                trees,
            ),
        );
    }
}

/// The serial loader over freshly prepared state — the test seam for the
/// loader-parity pin: a full serial reload must reproduce the parallel
/// loader's unit faces exactly.
#[cfg(test)]
pub(crate) fn reload_serial_for_test(state: &mut EngineState) -> u64 {
    state.deps = DepIndex::default();
    let seeds = prepare_dep_closure(state);
    load_serial(state, seeds);
    state.deps.load_failures
}

/// [`dep_unit_faces`] for the loader-parity pin.
#[cfg(test)]
pub(crate) fn dep_faces_for_test(state: &EngineState) -> (Vec<(PathBuf, String)>, String) {
    dep_unit_faces(state)
}

/// The `DQ_DEPS_DUMP` verification seam: prints the digests
/// [`dep_unit_faces`] derives, one line per unit sorted by path plus one
/// total line — the serial loader and the parallel loader must emit
/// identical output on identical input.
fn dump_dep_units(state: &EngineState) {
    if std::env::var_os("DQ_DEPS_DUMP").is_none() {
        return;
    }
    let (faces, total) = dep_unit_faces(state);
    for (path, digest) in faces {
        eprintln!("DQ_DEPS_DUMP {} {digest}", path.display());
    }
    eprintln!("DQ_DEPS_DUMP total {total}");
}

/// A deterministic, `FileId`/`StringId`-independent digest of every
/// consumed dep-unit face, per unit and folded — the equality contract the
/// serial loader and the parallel loader must satisfy byte for byte. The
/// faces folded in are exactly the ones the workspace's resolution reads:
/// the element model's own API signature, the directives (kinds, URIs,
/// prefixes, combinators, configurations, names — resolution columns
/// excluded, deps keep them deliberately unresolved), the declaration
/// capture's name/kind/span rows, the resolved targets (mapped back to
/// their canonical paths), the settled version, and the content id. Dead
/// columns (diagnostics, api_signature, doc_imports) are deliberately
/// absent — the interface-only tier never produces them.
///
/// Interned ids resolve to their text before hashing: parallel string
/// interning assigns ids in scheduling order, so the digest equals across
/// runs only on the resolved spellings.
fn dep_unit_faces(state: &EngineState) -> (Vec<(PathBuf, String)>, String) {
    let mut files: Vec<FileId> = state.deps.units.keys().copied().collect();
    files.sort_unstable();
    let mut faces = Vec::with_capacity(files.len());
    let mut total = blake3::Hasher::new();
    for file in files {
        let unit = &state.deps.units[&file];
        let path = state
            .files
            .path(file)
            .unwrap_or_else(|| Path::new(""))
            .to_path_buf();
        // The element model's own canonical face — the strongest single
        // equality check (it renders every declaration, container, member,
        // clause, and annotation shape through sorted resolved text).
        // Derived on the fly: the stored `api_signature` is the engine
        // gate's face and stays a zero placeholder on the interface-only
        // tier.
        let elements = component_digest(0, |field| {
            field(unit.facts.elements.api_signature(&state.strings).as_bytes());
        });
        let directives = component_digest(1, |field| {
            for directive in &unit.facts.directives {
                field(&[directive.kind as u8]);
                field(directive.uri.as_deref().unwrap_or("").as_bytes());
                field(
                    directive
                        .prefix
                        .map(|id| state.strings.resolve(id).as_bytes())
                        .unwrap_or_default(),
                );
                field(&[u8::from(directive.deferred)]);
                // The written combinator sequence with clause boundaries,
                // resolved to text (the struct's interned ids are
                // process-local and order-dependent).
                for (name, hidden, span, clause) in directive.combinators.written_with_clauses() {
                    field(state.strings.resolve(name).as_bytes());
                    field(&[u8::from(hidden)]);
                    field(&span.offset.to_le_bytes());
                    field(&span.length.to_le_bytes());
                    field(&clause.to_le_bytes());
                }
                field(directive.name.as_deref().unwrap_or("").as_bytes());
                for configuration in directive.configurations.iter() {
                    field(configuration.dotted_name.as_bytes());
                    field(configuration.value.as_deref().unwrap_or("").as_bytes());
                    field(configuration.uri.as_bytes());
                }
            }
        });
        let capture = component_digest(2, |field| {
            for decl in unit.capture.index.decls() {
                field(state.strings.resolve(decl.name).as_bytes());
                field(&[decl.kind as u8]);
                field(&[u8::from(decl.unnamed_extension_member)]);
                field(&decl.range.offset.to_le_bytes());
                field(&decl.range.length.to_le_bytes());
                field(&decl.container.to_le_bytes());
                field(&decl.decl_end.to_le_bytes());
                field(&decl.body_open.to_le_bytes());
            }
        });
        let targets = component_digest(3, |field| {
            for target in unit.targets.iter() {
                match target {
                    ResolvedTarget::Missing => field(b"m"),
                    ResolvedTarget::External => field(b"e"),
                    ResolvedTarget::Resolved(id) => field(
                        state
                            .files
                            .path(*id)
                            .unwrap_or_else(|| Path::new(""))
                            .as_os_str()
                            .as_encoded_bytes(),
                    ),
                };
            }
        });
        let stamp = component_digest(4, |field| {
            field(&unit.version.major.to_le_bytes());
            field(&unit.version.minor.to_le_bytes());
            field(unit.facts.content().as_bytes());
            field(path.as_os_str().as_encoded_bytes());
        });
        let digest = format!("{elements}:{directives}:{capture}:{targets}:{stamp}");
        total.update(digest.as_bytes());
        faces.push((path, digest));
    }
    (faces, total.finalize().to_hex().to_string())
}

/// One component of a unit's face digest: a seeded, length-prefixed walk
/// of the component's fields (the [`dep_unit_faces`] attribution lanes).
fn component_digest(seed: u8, write: impl FnOnce(&mut dyn FnMut(&[u8]))) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&[seed]);
    let mut field = |bytes: &[u8]| {
        hasher.update(&(bytes.len() as u32).to_le_bytes());
        hasher.update(bytes);
    };
    write(&mut field);
    hasher.finalize().to_hex().to_string()
}

/// One dep file's discovery probe: the canonical text (the store adopts
/// the very bytes the compute ran over), the computed interface unit (the
/// interface tier's output depends on the engine `FileId` only through the
/// capture's file column, which the install rewrites — so the unit
/// computes here, fully parallel), and the resolved target per directive
/// aligned with the extracted directive list. `text` and `unit` are `None`
/// when the file could not be read — the unreadable face the serial loader
/// counts.
struct DepProbe {
    /// The canonical source text, `None` for an unreadable file.
    text: Option<SourceText>,
    /// The computed interface unit, `None` for an unreadable file.
    unit: Option<(UnitFacts, dartq_index::FileCapture, LanguageVersion)>,
    /// The resolved target per directive of the file's own extraction, in
    /// directive order — import/export carry their resolution, every
    /// other family records `Missing` (the serial loader's posture).
    targets: Vec<ProbeTarget>,
}

/// The engine-facing classification of one probed directive target —
/// [`ResolvedTarget`] with the filesystem paths standing in for the engine
/// [`FileId`] until the serial replay interns them.
enum ProbeTarget {
    /// No target (an unresolvable URI, or a non-import/export directive).
    Missing,
    /// An external (`dart:`) library.
    External,
    /// A resolved file target: the raw path the resolution composed (the
    /// serial loader's `files.intern` key) and its canonical spelling (the
    /// frontier/dedup key, precomputed on the worker so the serial phases
    /// pay no filesystem walks). `id` fills at the replay's intern.
    Resolved {
        path: Box<Path>,
        canonical: Box<Path>,
        id: Option<FileId>,
    },
}

/// The phase-one per-file probe: disk read, the interface compute, the
/// directive extraction, and URI resolution for the walk's next frontier —
/// all pure functions of shared read-only state plus a thread-local
/// [`FileTable`], so the lanes run on rayon workers.
fn probe_dep(state: &EngineState, path: &Path, full: bool) -> DepProbe {
    let text = std::fs::read(path).ok().map(|b| SourceText::canonical(&b));
    // The resolver interns into a thread-local table — its `&mut` borrow
    // rules out sharing `state.files`, and a `FileId` it mints means
    // nothing outside the file. `exists`'s residency arm reads ids back
    // out of that table, so the closure maps them through `local_paths`:
    // the index-locked mirror of the local table's interned spellings,
    // kept in step after every resolve (the container first, then exactly
    // one entry per resolved target — `land`'s single intern per landing).
    // The container rides `intern_normalized` with its own spelling: a
    // frontier path is already canonical, so the local table pays no walk.
    let mut local = FileTable::new();
    let file = local.intern_normalized(path, path.to_path_buf());
    let local_paths = std::cell::RefCell::new(vec![path.to_path_buf()]);
    let local_seen =
        std::cell::RefCell::new(rustc_hash::FxHashSet::from_iter([path.to_path_buf()]));
    let resident = |id: FileId| {
        local_paths
            .borrow()
            .get(id.index() as usize)
            .and_then(|path| state.files.try_file_id(path))
            .is_some_and(|id| state.residency.contains_key(&id))
    };
    let mut unit = None;
    let mut targets = Vec::new();
    if let Some(text) = &text {
        let version = version_for_path(state, path);
        // The grants pass the thread-local table: the probe's `file` id is
        // local, but the derivation reads the file's *path* (owner
        // attribution: SDK-library reverse map, package lookup, options
        // entries), which the local table maps correctly. Against the
        // engine table a local id would name an unrelated file and grant
        // the wrong feature set.
        let grants = state.grants_over(&local);
        let computed = if full {
            let (facts, capture, face) = l1::compute_unit(
                file,
                text.content_id(),
                text.as_str(),
                version,
                &state.strings,
                &grants,
            );
            (facts, capture, face.version)
        } else {
            l1::compute_dep_unit(
                file,
                text.content_id(),
                text.as_str(),
                version,
                &state.strings,
                &grants,
            )
        };
        drop(grants);
        let mut resolver = UriResolver::new(
            &state.uri_packages,
            &state.sdk,
            &state.declared_variables,
            &mut local,
            &resident,
        );
        targets.reserve(computed.0.directives.len());
        for directive in &computed.0.directives {
            if !matches!(
                directive.kind,
                DirectiveKind::Import | DirectiveKind::Export
            ) {
                targets.push(ProbeTarget::Missing);
                continue;
            }
            let resolution = resolver.resolve(
                file,
                directive.uri.as_deref(),
                UriDirectiveKind::ImportExport,
            );
            let target = match resolution.target {
                ResolvedTarget::Missing => ProbeTarget::Missing,
                ResolvedTarget::External => ProbeTarget::External,
                ResolvedTarget::Resolved(_) => {
                    // The local-table id is garbage; the `uri` face carries
                    // the real filesystem path the serial loader interned.
                    match resolution.uri {
                        dartq_resolution::ResolvedUri::File(path)
                        | dartq_resolution::ResolvedUri::Package { file: path, .. }
                        | dartq_resolution::ResolvedUri::Dart { file: path, .. } => {
                            let canonical = normalize_path(&path);
                            // Keep `local_paths` index-locked with the
                            // table: a resolved landing interned exactly one
                            // path — unless a previous directive already
                            // minted it, the mirror grows by one.
                            if local_seen.borrow_mut().insert(canonical.clone()) {
                                local_paths.borrow_mut().push(canonical.clone());
                            }
                            ProbeTarget::Resolved {
                                path: path.into_boxed_path(),
                                canonical: canonical.into_boxed_path(),
                                id: None,
                            }
                        }
                        dartq_resolution::ResolvedUri::Unresolved(_) => ProbeTarget::Missing,
                    }
                }
            };
            targets.push(target);
        }
        unit = Some(computed);
    }
    DepProbe {
        text,
        unit,
        targets,
    }
}

/// The file's language version from its path alone — the worker-side twin
/// of [`l1::language_version_for`] (its `FileTable` lookup replaced by the
/// already-known path), for the parallel probe, which runs before the
/// engine id exists.
fn version_for_path(state: &EngineState, path: &Path) -> LanguageVersion {
    state
        .packages
        .containing(path)
        .and_then(|identity| identity.language_version)
        .map_or(
            dartq_language::DEFAULT_LANGUAGE_VERSION,
            dartq_pub::LanguageVersion::into_inner,
        )
}

/// Whether `file` may join the dependency index: anything discovery cannot
/// make resident. The bound is the same one discovery walks — a package
/// root at or beneath the workspace root is analyzed surface (its files
/// enter residency through discovery, and one that did not — an
/// options-excluded file — keeps that posture instead of silently
/// re-entering through the dep face); a package outside the bound (the
/// pub cache, a git checkout, an SDK-provided package, a path dependency
/// pointing elsewhere) and a path under no package root at all index.
///
/// The dot-prefixed arm closes the one place the two bounds disagreed:
/// discovery skips every dot directory, so a package root beneath one (the
/// chunked-conformance shape's `.aux/<package>` roots — resolved into the
/// context's `package:` map but never walked) is analyzed surface
/// discovery can never reach. Without the index its interface never loads,
/// and the resolution face reports the imports that DO resolve there as
/// `uri_does_not_exist` plus an undefined-name cascade the oracle never
/// emits (the oracle resolves such a target as a dependency library).
fn dep_candidate(state: &EngineState, file: FileId) -> bool {
    let Some(path) = state.files.path(file) else {
        return false;
    };
    dep_candidate_path(state, path)
}

/// The path-level half of [`dep_candidate`] — the discovery walk runs it
/// on a resolved child path before any [`FileTable`] interning.
fn dep_candidate_path(state: &EngineState, path: &Path) -> bool {
    match state.packages.containing(path) {
        Some(identity) => {
            !identity.root.starts_with(state.packages.workspace_root())
                || !discovery_reaches(path, state.packages.workspace_root())
        }
        None => true,
    }
}

/// Whether the discovery walk reaches `path` from the workspace bound: it
/// descends no dot-prefixed directory and takes no dot-named file
/// (`dartq-engine`'s `walk`, the oracle's `_isExcluded` dot rule), so any
/// dot component below the bound puts the file outside the resident world.
fn discovery_reaches(path: &Path, bound: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(bound) else {
        return false;
    };
    relative
        .components()
        .all(|component| !component.as_os_str().to_string_lossy().starts_with('.'))
}

/// Loads one dep file in the original serial shape — kept live as the
/// `DQ_DEPS_SERIAL` A/B arm: disk read, the shared per-file pipeline,
/// directive resolution for the graph edges and the next frontier.
/// Returns the not-already-indexed targets. Every failure mode (no path,
/// unreadable, missing bytes) counts and skips silently.
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
    // The source bytes are never adopted into the store (the module doc's
    // retention posture): the unit's own fields carry every analyze-time
    // face, and the query source read re-derives lazily.
    let content = text.content_id();
    let version = l1::language_version_for(&state.packages, &state.files, file);
    let (facts, capture, scanned) = l1::compute_unit(
        file,
        content,
        text.as_str(),
        version,
        &state.strings,
        &state.grants(),
    );
    let clauses = l1::capture_dep_clauses(
        file,
        text.as_str(),
        scanned.version,
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
        if !matches!(
            directive.kind,
            DirectiveKind::Import | DirectiveKind::Export
        ) {
            targets.push(ResolvedTarget::Missing);
            continue;
        }
        let resolution = resolver.resolve(
            file,
            directive.uri.as_deref(),
            UriDirectiveKind::ImportExport,
        );
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
        if state.residency.contains_key(&dependency) || state.deps.units.contains_key(&dependency) {
            continue;
        }
        if !dep_candidate(state, dependency) {
            continue;
        }
        next.push(dependency);
    }
    state.deps.insert(
        file,
        DepUnit::new(
            facts,
            capture.into_declaration_surface(),
            scanned.version,
            targets.into_boxed_slice(),
            clauses,
        ),
    );
    next
}

/// The `--impacted` selection's directive graph: which of `entries` (the
/// suites a test run could execute) reach any of `seeds` (the changed
/// files) through `import`/`export`/`part` edges. Test selection needs
/// import reachability, not element models: Dart code can only use what a
/// directive brings in, so a suite whose directive closure misses every
/// changed file cannot observe the change. This is the selection most
/// test-impact tools run, and it needs no semantic analysis — one scan and
/// one directive walk per visited file, nothing retained but the edges.
///
/// # Walk
///
/// Forward BFS from the entries, level-synchronous: each frontier's files
/// read, scan, and extract their directives in parallel (the pure per-file
/// half, shared read-only state); the URIs then resolve on the calling
/// thread through the engine's own [`UriResolver`] (the same resolution
/// the analysis runs: package universes, relative composition, generated
/// faces). A conditional import/export contributes every branch, so the
/// graph holds for any `-D` environment. Only files of **transparent**
/// packages are walked (see [`transparent_packages`]); a target in an
/// opaque package is an edge sink, never read. The reverse pass then
/// floods from the seeds over the inverted edges (CSR, `u32` ids), and the
/// entries it marks are the answer.
///
/// # Soundness of the opaque tier
///
/// A pub-cache or SDK-provided package is immutable for one resolution
/// (its identity — version, content hash, ref — rides the lock and the
/// package config, which the selection's resolution stamp witnesses), so
/// nothing inside it can change without the whole run going unbounded.
/// What it *could* do is relay a change: a cache package whose code imports
/// an editable package (a `dependency_overrides` path override is the one
/// way pub lets that happen). Such a package declares the editable one in
/// its pubspec, so it is transparent, and so is every package declaring a
/// transparent one — the fixpoint keeps every relay walked.
///
/// # Cost
///
/// Memory: the local path table (one interned path per visited or targeted
/// file), one `(u32, u32)` per directive edge, and the per-node bit sets;
/// file texts live only for their own scan. Time: one read + scan per
/// walked file, parallel per level; dependency closures never load.
///
/// # Errors
///
/// [`crate::EngineError::PackageConfig`] when the package config cannot
/// back the URI-resolution table (the same refusal engine construction
/// raises).
pub fn import_reach(
    config: &crate::EngineConfig,
    entries: &[PathBuf],
    seeds: &[PathBuf],
) -> Result<ImportReach, crate::EngineError> {
    let uri_packages = UriPackageTable::new_with_context_roots(
        &config.pub_config,
        &config.config_dir,
        &config.context_roots,
    )?;
    Ok(walk_reach(
        &config.packages,
        &uri_packages,
        &config.sdk,
        entries,
        seeds,
    ))
}

/// The answer of [`import_reach`].
#[derive(Debug)]
pub struct ImportReach {
    /// The entries whose directive closure reaches a seed, in entry order.
    pub reached: Vec<PathBuf>,
    /// How many files the walk read and scanned — the work bound: local
    /// (transparent) files only, never a dependency package's.
    pub scanned: usize,
}

/// [`import_reach`] over the assembled resolution tables.
fn walk_reach(
    packages: &dartq_package::PackageTable,
    uri_packages: &UriPackageTable,
    sdk: &SdkSnapshot,
    entries: &[PathBuf],
    seeds: &[PathBuf],
) -> ImportReach {
    let transparent = transparent_packages(packages, seeds);
    let walked = |path: &Path| {
        packages
            .containing(path)
            .is_none_or(|identity| transparent.contains(identity.name.as_str()))
    };
    let mut files = FileTable::new();
    let strings = StringTable::new();
    let declared = FxHashMap::default();
    let entry_ids: Vec<FileId> = entries.iter().map(|path| files.intern(path)).collect();
    let mut visited: Vec<bool> = Vec::new();
    let mark = |visited: &mut Vec<bool>, file: FileId| -> bool {
        let index = file.index() as usize;
        if index >= visited.len() {
            visited.resize(index + 1, false);
        }
        !std::mem::replace(&mut visited[index], true)
    };
    let mut frontier: Vec<FileId> = entry_ids
        .iter()
        .copied()
        .filter(|&file| mark(&mut visited, file))
        .collect();
    let mut edges: Vec<(u32, u32)> = Vec::new();
    let mut scanned = 0usize;
    while !frontier.is_empty() {
        scanned += frontier.len();
        let table = &files;
        let uris: Vec<Vec<Box<str>>> = frontier
            .par_iter()
            .map(|&file| directive_uris(table, &strings, file))
            .collect();
        let resident = |_: FileId| false;
        let mut resolver = UriResolver::new(uri_packages, sdk, &declared, &mut files, &resident);
        let mut next = Vec::new();
        for (&file, uris) in frontier.iter().zip(&uris) {
            for uri in uris {
                let ResolvedTarget::Resolved(target) = resolver
                    .resolve(file, Some(&**uri), UriDirectiveKind::ImportExport)
                    .target
                else {
                    continue;
                };
                edges.push((file.index(), target.index()));
                if resolver.file_table().path(target).is_some_and(&walked)
                    && mark(&mut visited, target)
                {
                    next.push(target);
                }
            }
        }
        frontier = next;
    }
    let seeds = seeds.iter().filter_map(|seed| files.try_file_id(seed));
    let hit = flood_reverse(files.len(), &edges, seeds);
    let reached = entries
        .iter()
        .zip(&entry_ids)
        .filter(|(_, id)| hit[id.index() as usize])
        .map(|(path, _)| path.clone())
        .collect();
    ImportReach { reached, scanned }
}

/// The import/export/part URIs of `file`, every conditional branch
/// included (interpolated URIs resolve nowhere and are skipped). An
/// unreadable or missing file — a deleted seed's dangling target — has no
/// directives: it is a sink in the graph, its importers' edges still
/// point at it.
fn directive_uris(files: &FileTable, strings: &StringTable, file: FileId) -> Vec<Box<str>> {
    let Some(bytes) = files.path(file).and_then(|path| std::fs::read(path).ok()) else {
        return Vec::new();
    };
    let source = String::from_utf8_lossy(&bytes);
    let text: &str = source.strip_prefix('\u{feff}').unwrap_or(&*source);
    // Directive extraction reads keyword/string/punctuation tokens only;
    // neither lexical gate changes them.
    let config = ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: false,
    };
    let tokens = scan(text, config, file).tokens;
    let mut ignored = Vec::new();
    let (directives, ..) = l1::extract_directives(text, &tokens, strings, file, &mut ignored);
    let mut uris = Vec::new();
    for directive in directives {
        if !matches!(
            directive.kind,
            DirectiveKind::Import | DirectiveKind::Export | DirectiveKind::Part
        ) {
            continue;
        }
        uris.extend(directive.uri);
        for (index, branch) in directive.configurations.into_vec().into_iter().enumerate() {
            if !directive
                .interpolated_configurations
                .contains(&(index as u32))
            {
                uris.push(branch.uri);
            }
        }
    }
    uris
}

/// Marks every node that reaches a seed: the seeds themselves, then their
/// importers over the inverted `(from, to)` edges, flooded once. The
/// inversion is a CSR (counting sort over `to`): `node_count + 1` offsets
/// and one `u32` per edge.
fn flood_reverse(
    node_count: usize,
    edges: &[(u32, u32)],
    seeds: impl Iterator<Item = FileId>,
) -> Vec<bool> {
    let mut offsets = vec![0u32; node_count + 1];
    for &(_, to) in edges {
        offsets[to as usize + 1] += 1;
    }
    for index in 0..node_count {
        offsets[index + 1] += offsets[index];
    }
    let mut cursor = offsets.clone();
    let mut importers = vec![0u32; edges.len()];
    for &(from, to) in edges {
        let slot = &mut cursor[to as usize];
        importers[*slot as usize] = from;
        *slot += 1;
    }
    let mut hit = vec![false; node_count];
    let mut stack: Vec<u32> = Vec::new();
    for seed in seeds {
        if !std::mem::replace(&mut hit[seed.index() as usize], true) {
            stack.push(seed.index());
        }
    }
    while let Some(node) = stack.pop() {
        let range = offsets[node as usize] as usize..offsets[node as usize + 1] as usize;
        for &importer in &importers[range] {
            if !std::mem::replace(&mut hit[importer as usize], true) {
                stack.push(importer);
            }
        }
    }
    hit
}

/// The packages whose files the reach walks: every local (`Path`-category)
/// package, every package owning a seed, and — to a fixpoint — every other
/// package whose pubspec declares (`dependencies` / `dev_dependencies`) a
/// package already in the set: the only shape in which an immutable
/// package's code can import an editable one. A pubspec that cannot be
/// read or parsed counts as declaring a transparent dependency — an
/// unknown relay is walked, never assumed away. Pubspecs read in parallel,
/// once each.
fn transparent_packages<'a>(
    packages: &'a dartq_package::PackageTable,
    seeds: &[PathBuf],
) -> rustc_hash::FxHashSet<&'a str> {
    use dartq_package::SourceCategory;
    let mut transparent: rustc_hash::FxHashSet<&str> = packages
        .iter()
        .filter(|identity| identity.category == SourceCategory::Path)
        .map(|identity| identity.name.as_str())
        .collect();
    transparent.extend(
        seeds
            .iter()
            .filter_map(|seed| packages.containing(seed))
            .map(|identity| identity.name.as_str()),
    );
    // `None` = unreadable: a relay of unknown shape.
    let mut opaque: Vec<(&str, Option<Vec<String>>)> = packages
        .iter()
        .filter(|identity| !transparent.contains(identity.name.as_str()))
        .collect::<Vec<_>>()
        .par_iter()
        .map(|&identity: &&'a dartq_package::PackageIdentity| {
            (
                identity.name.as_str(),
                declared_dependencies(&identity.root.join("pubspec.yaml")).ok(),
            )
        })
        .collect();
    loop {
        let before = opaque.len();
        opaque.retain(|(name, declared)| {
            let relays = declared.as_ref().is_none_or(|declared| {
                declared
                    .iter()
                    .any(|dependency| transparent.contains(dependency.as_str()))
            });
            if relays {
                transparent.insert(*name);
            }
            !relays
        });
        if opaque.len() == before {
            return transparent;
        }
    }
}

/// The dependency names a pubspec declares (`dependencies` and
/// `dev_dependencies` keys) — the only fact the transparency fixpoint
/// reads, so the full pubspec model (constraints, environment) is never
/// built.
fn declared_dependencies(pubspec: &Path) -> io::Result<Vec<String>> {
    let text = std::fs::read_to_string(pubspec)?;
    let yaml: serde_yaml::Value = serde_yaml::from_str(&text).map_err(io::Error::other)?;
    let mut names = Vec::new();
    for section in ["dependencies", "dev_dependencies"] {
        match yaml.get(section) {
            None | Some(serde_yaml::Value::Null) => {}
            Some(serde_yaml::Value::Mapping(map)) => {
                for key in map.keys() {
                    let name = key
                        .as_str()
                        .ok_or_else(|| io::Error::other("non-string dependency name"))?;
                    names.push(name.to_owned());
                }
            }
            Some(_) => return Err(io::Error::other("malformed dependency section")),
        }
    }
    Ok(names)
}
