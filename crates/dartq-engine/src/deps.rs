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
//! index) plus the source bytes in the content store; the scan face's
//! token table is deliberately NOT retained (≈20 B/token — the dominant
//! per-file cost at closure scale, and a pure function of the bytes): the
//! one consumer that needs tokens (the clause-tree capture) re-scans the
//! retained text on demand.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dartq_index::{FileCapture, Postings, distinct_decl_names};
use dartq_language::LanguageVersion;
use dartq_resolution::{
    DirectiveKind as UriDirectiveKind, PackageTable as UriPackageTable, ResolvedTarget, UriResolver,
};
use dartq_scanner::{ScanConfig, scan};
use dartq_sdk::SdkSnapshot;
use dartq_source::{FileId, FileTable, SourceText, StringTable};
use rayon::prelude::*;
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
        DepUnit {
            facts,
            capture: Arc::new(capture.without_sites()),
            version: scanned.version,
            targets: targets.into_boxed_slice(),
        },
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
