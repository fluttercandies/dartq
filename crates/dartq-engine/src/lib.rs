//! The dartq engine facade: one analysis context, two-phase content
//! updates, and topologically ordered library recompute (engine-api-spec
//! §1, incremental-l1-spec §3).
//!
//! # Shape (§1.1–§1.5)
//!
//! * One [`Engine`] binds the immutable context — content store, SDK
//!   snapshot, package identity table, and the pub config the URI layer
//!   resolves against. Context change = new Engine.
//! * [`Engine::set_content`] / [`Engine::remove_file`] only enqueue into the
//!   pending overlay (official `changeFile`: enqueue, never compute);
//!   [`Engine::apply_pending`] is the barrier that runs the ContentId gate,
//!   the per-file facts recompute, and the L1 cascade.
//! * [`Engine::analyze`] applies as its fixed first phase, recomputes dirty
//!   libraries in import-edge topological order on the batch fan-out, and
//!   returns the merged, canonically sorted diagnostic set.
//! * [`EngineEvent`]s arrive on a synchronous `FnMut` callback, in work
//!   order; the returned diagnostics are in canonical order — the two are
//!   deliberately distinct (§5 risk 1).
//!
//! # Module layout (§1.6)
//!
//! `state` (residency + overlay + reverse edges), `l1` (apply), `pipeline`
//! (analyze), `budget` (the §17.1 memory-budget runtime), `store` (the §17.2
//! L3 content-addressed store), `event`, `cancel`, `query` (the `query
//! symbol` face, PRD
//! §20.3), `semdiag` (the default semantic diagnoser, #236),
//! `analysis_options` (the merged options per governing directory, PRD §12), `context` +
//! `scheduler` (the multi-context runtime substrate — contexts, overlays,
//! scheduling, cancellation — PRD §16).
//!
//! # The semantic diagnostics seam (#120/#121)
//!
//! The engine assembles syntax (scan + parse recovery) and URI-layer
//! diagnostics itself. Semantic diagnosis — `undefined_*`, `wrong_number_*`,
//! the part/part-of family — plugs in through [`SemanticDiagnoser`]: the
//! trait lives here so the engine needs no dependency edge back upward, and
//! the engine ships the default implementor ([`semdiag::Diagnoser`], #236),
//! which folds `dartq_semantic::diagnose_with` over the [`LibraryView`] faces.
//! With no diagnoser configured the engine under-reports exactly those
//! families and nothing else.

pub mod analysis_options;
mod budget;
pub mod cancel;
pub mod context;
pub mod event;
pub mod l1;
pub mod pipeline;
pub mod query;
pub mod scheduler;
pub mod semdiag;
pub mod state;
pub mod store;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dartq_diagnostics::Diagnostic;
use dartq_index::{FileCapture, LibraryIndex};
use dartq_language::FeatureSet;
use dartq_package::PackageTable;
use dartq_pub::{PackageConfig, PubError};
use dartq_sdk::SdkSnapshot;
use dartq_snapshot::{
    Entry, EntryKey, GenerationHeader, RemapTables, SNAPSHOT_FORMAT_VERSION, SaveError, Snapshot,
    SnapshotStore,
};
use dartq_source::{
    FileId, FileTable, LineIndex, MemorySourceStore, PooledSourceStore, SourceStore, StringTable,
};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use thiserror::Error;

pub use cancel::{CancellationToken, Cancelled};
pub use context::{ContextConfig, ContextId, ContextKey, OverlayChange, Runtime};
pub use dartq_source::ContentPool;
pub use event::{EngineEvent, RevalidateGate};
pub use query::{DeclHit, DeclKind, RefHit, RefKind, SymbolAnswer};
pub use scheduler::{DrainReport, Task, Tier};
pub use state::persist::PersistedFacts;
pub use state::{DirectiveFacts, DirectiveKind, FeatureGrants, LibraryId, ScannedFace, UnitFacts};

/// The default SDK target segment (`libraries.json` panel) `dart:` URI
/// resolution dispatches against — the `dart analyze` posture. Every
/// [`EngineConfig`] constructor spells it unless the context selects
/// otherwise; the constant is the single source of the default.
pub const DEFAULT_TARGET: &str = "vm";

/// Everything immutable across the engine's lifetime (§1.1): the M1 slice
/// of PRD §5.4's six context fields — store, SDK, packages — plus the pub
/// config + config directory the URI-resolution table derives from. The
/// fork dimensions the context salt pins (#482) — target segment, declared
/// variables — are first-class fields (#483) and ride the config into the
/// context key.
//
// The declared-variables map is the face (its FxHasher is part of the
// engine's identity, not a generalization axis).
pub struct EngineConfig {
    /// Content-addressed byte storage. The engine takes ownership of the
    /// store handle; byte-level sharing across engines/contexts is opt-in
    /// through one [`ContentPool`] ([`pooled_memory_store`]): stores wired
    /// to the same pool hold every distinct content exactly once between
    /// them — the multi-context store face (§16.1's shared content store,
    /// the M3 note this field's doc drafted). The handle itself stays
    /// per-engine, so reads keep the zero-copy `get` contract and each
    /// engine's own amortized GC path. [`memory_store`] is the standalone
    /// default.
    pub store: Box<dyn SourceStore + Send + Sync>,
    /// SDK snapshot (PRD §7); loading hard-errors on a crippled SDK before
    /// construction (dartq-sdk doc).
    pub sdk: SdkSnapshot,
    /// Package identity table over one resolution (PRD §8); carries the
    /// per-package language versions the engine derives per-file feature
    /// sets from.
    pub packages: PackageTable,
    /// The parsed `package_config.json` the identity table was built from —
    /// the URI-resolution face re-derives its lib-dir index from it.
    pub pub_config: Arc<PackageConfig>,
    /// The directory holding `package_config.json` (normally `.dart_tool/`).
    pub config_dir: PathBuf,
    /// The semantic diagnostics plug point (#120/#121); `None` keeps the
    /// engine's syntax + URI-layer set (under-reporting contract).
    pub semdiag: Option<Arc<dyn SemanticDiagnoser>>,
    /// The SDK target segment `dart:` URI resolution dispatches against
    /// (the `libraries.json` panel — `vm`, `js`, …). [`DEFAULT_TARGET`]
    /// (`vm`) preserves the `dart analyze` posture; the per-context value
    /// forks both the context key and the snapshot generation salt.
    pub target: String,
    /// Declared variables (`-D name=value`) the conditional-import
    /// (configurable URI) directives evaluate against. Empty by default —
    /// the no-`-D` oracle context under which every configurable directive
    /// resolves its primary URI; the per-context value forks both the
    /// context key and the snapshot generation salt.
    pub declared_variables: FxHashMap<String, String>,
}

/// Engine construction and explicit-API failures.
#[derive(Debug, Error)]
pub enum EngineError {
    /// `remove_file` of a path the engine never interned and does not
    /// track (deliberate deviation from the official silent no-op,
    /// incremental-l1-spec §1.2: dartq reports explicitly).
    #[error("unknown path: {0}")]
    UnknownPath(PathBuf),
    /// Runtime addressing of a context that is not open: never opened,
    /// already closed, or a recycled id.
    #[error("unknown context: {0}")]
    UnknownContext(ContextId),
    /// The pub package config cannot back the URI-resolution table
    /// (dartq-pub's shared `resolve` face rejected it: a broken `rootUri`,
    /// duplicate names, a name outside the reader grammar, or a
    /// `packageUri` escaping its root).
    #[error("package config unusable: {0}")]
    PackageConfig(#[from] PubError),
}

/// The analysis driver (§1).
pub struct Engine {
    state: state::EngineState,
    graph: Option<pipeline::ResolvedGraph>,
    semdiag: Option<Arc<dyn SemanticDiagnoser>>,
}

impl Engine {
    /// Binds the context. `dart:` URIs resolve against the config's target
    /// segment ([`EngineConfig::target`], [`DEFAULT_TARGET`] when unset) and
    /// conditional directives against [`EngineConfig::declared_variables`].
    ///
    /// # Errors
    ///
    /// [`EngineError::PackageConfig`] when the config cannot back the
    /// URI-resolution table.
    pub fn new(config: EngineConfig) -> Result<Self, EngineError> {
        let semdiag = config.semdiag;
        let state = state::EngineState::new(
            config.packages,
            &config.pub_config,
            &config.config_dir,
            config.sdk,
            config.store,
            config.target,
            config.declared_variables,
        )?;
        Ok(Self {
            state,
            graph: None,
            semdiag,
        })
    }

    /// Arms (or disarms) the memory-budget runtime (PRD §17.1). A limit
    /// turns on sampled-RSS accounting at the phase and library boundaries
    /// of every later [`Self::analyze`]: over budget, the engine descends
    /// the degradation ladder one tier per boundary — shedding
    /// documented-recomputable caches and reporting each step as a
    /// `dartq/memory_degraded` diagnostic — while diagnostics stay
    /// complete at every tier. `None` (the default) short-circuits every
    /// check: the sampler never runs and the output face is byte-identical
    /// to an unbudgeted run.
    pub fn set_memory_limit(&mut self, limit_mib: Option<u64>) {
        self.state.budget = limit_mib.map(budget::MemoryBudget::new);
    }

    /// Arms (or disarms) the `--trace-events` JSONL sink (PRD §17.4): every
    /// later apply/analyze appends one JSON object per engine event — §17.4
    /// taxonomy names, stable field order, flushed per event so a killed CI
    /// run keeps its tail. `None` (the default) constructs nothing and the
    /// output face is byte-identical to an untraced run.
    pub fn set_trace_sink(&mut self, sink: Option<event::TraceSink>) {
        self.state.trace = sink;
    }

    /// Arms (or re-arms) the L3 content-addressed store (PRD §17.2): the
    /// persistent, cross-session global under `config.root` that every
    /// later apply batch's changed contents persist into (write batch =
    /// puts, then flush — manifest save + cap enforcement). Opening
    /// degrades gracefully — a torn manifest rebuilds by mtime and never
    /// blocks startup — so the only failure is a store directory that
    /// cannot exist; a caller that treats that as a warning keeps
    /// analyzing disarmed. Re-arming replaces the handle (flush the old
    /// store through [`Self::content_store_mut`] first if its books carry
    /// unsaved pins).
    ///
    /// # Errors
    ///
    /// [`store::StoreError::CreateDir`] when the store directory cannot
    /// exist.
    pub fn open_content_store(
        &mut self,
        config: store::StoreConfig,
    ) -> Result<(), store::StoreError> {
        self.state.l3 = Some(store::ContentStore::open(config)?);
        Ok(())
    }

    /// The armed L3 store, read-only face (`None` when disarmed — the
    /// default): lookups, blob reads, and the observability counters the
    /// daemon's diagnostics assemble over.
    #[must_use]
    pub fn content_store(&self) -> Option<&store::ContentStore> {
        self.state.l3.as_ref()
    }

    /// The armed L3 store, mutating face (`None` when disarmed): the
    /// daemon's reference-counting surface — [`store::ContentStore::retain`]
    /// pins a workspace's contents against eviction,
    /// [`store::ContentStore::release`] drops the pin when the workspace
    /// closes, and a final [`store::ContentStore::flush`] persists the
    /// pins.
    pub fn content_store_mut(&mut self) -> Option<&mut store::ContentStore> {
        self.state.l3.as_mut()
    }

    /// Enqueues a content override. Only touches the pending overlay — no
    /// derived state is invalidated here. Interns `path` if new and returns
    /// its `FileId`. Last write per path wins; `set_content` then
    /// `remove_file` before apply nets to a no-op for that path.
    ///
    /// The bytes decode as UTF-8 eagerly (the whole pipeline downstream is
    /// `&str`); invalid sequences become U+FFFD. Oracle parity: the analyzer
    /// read layer (`FileContentCache._read`,
    /// pkg/analyzer/lib/src/dart/analysis/file_content_cache.dart) decodes
    /// strictly and on any decode failure marks the file nonexistent
    /// (`exists = false`, empty content) — an undecodable file contributes
    /// zero diagnostics. The `encoding` diagnostic ("Unable to decode bytes
    /// as UTF-8.", pkg/_fe_analyzer_shared/lib/src/messages/diagnostic.g.dart,
    /// scanner-path translation at pkg/analyzer/lib/src/dart/scanner/
    /// translate_error_token.dart) belongs to the compiler's byte-scanner
    /// path and is never reached by `dart analyze`; probed on 3.13.1: FF FE
    /// in a comment, inside a string literal, and in a code position all
    /// yield zero rows. dartq keeps analyzing the lossy text: the scanner
    /// culls U+FFFD silently (`replacement_char_is_skipped_silently` in
    /// dartq-scanner), which matches that output for bytes inside comments
    /// or string literals. Bytes landing in code positions are a known
    /// divergence: the cull can leave a broken token stream (e.g.
    /// `var <FFFD> = 1;` scans as `var = 1;`) that may surface parser
    /// diagnostics where the oracle emits none.
    ///
    /// Exactly one leading UTF-8 BOM is dropped before decode (oracle
    /// read-layer parity: the analyzer read path strips it before scanning,
    /// so all spans are relative to the stripped text). A second BOM is
    /// content and reaches the scanner as an ordinary U+FEFF.
    pub fn set_content(&mut self, path: &Path, bytes: &[u8]) -> FileId {
        let file = self.state.files.intern(path);
        if !self.state.pending.contains_key(&file) {
            self.state.pending_order.push(file);
        }
        let text = String::from_utf8_lossy(strip_one_bom(bytes)).into_owned();
        self.state
            .pending
            .insert(file, state::PendingChange::Set(text));
        file
    }

    /// Enqueues a removal. Unknown path — never interned and not pending —
    /// is an explicit error (official `changeFile` silently no-ops; dartq
    /// reports, incremental-l1-spec §1.2).
    ///
    /// # Errors
    ///
    /// [`EngineError::UnknownPath`] when the path is neither pending nor
    /// resident.
    pub fn remove_file(&mut self, path: &Path) -> Result<(), EngineError> {
        if let Some(file) = self.state.files.try_file_id(path) {
            if !self.state.pending.contains_key(&file) {
                self.state.pending_order.push(file);
            }
            self.state
                .pending
                .insert(file, state::PendingChange::Remove);
            return Ok(());
        }
        Err(EngineError::UnknownPath(path.to_path_buf()))
    }

    /// Barrier: drains the pending queue and runs the L1 cascade
    /// (§3.2 steps 1–5). Events fire only for a non-empty queue.
    pub fn apply_pending(&mut self, on_event: &mut dyn FnMut(EngineEvent)) -> AffectedReport {
        let outcome = l1::apply(&mut self.state, on_event);
        pipeline::flush_l3(&mut self.state, &outcome.changed);
        AffectedReport {
            changed: outcome.changed,
            removed: outcome.removed,
            invalidated: outcome.invalidated,
        }
    }

    /// Phase 1: [`Self::apply_pending`]. Phase 2: recompute dirty libraries
    /// in import-edge topological order; clean libraries keep cached
    /// results. Returns the merged diagnostic set of every live library,
    /// canonically sorted (path bytes, offset, code name) — the superset the
    /// incremental-equals-full oracle contract needs (deviation note in
    /// engine-api-spec: the drafted per-`targets` return cannot satisfy the
    /// byte-equality arm, so `targets` drives only the AnalyzeFile
    /// amplification events).
    ///
    /// # Errors
    ///
    /// [`Cancelled`] when a checkpoint fired: completed libraries keep their
    /// cached results, the rest stay dirty.
    pub fn analyze(
        &mut self,
        targets: &[FileId],
        on_event: &mut dyn FnMut(EngineEvent),
        cancel: &CancellationToken,
    ) -> Result<Vec<Diagnostic>, Cancelled> {
        self.capture_pubspecs();
        let mut diagnostics = pipeline::analyze(
            &mut self.state,
            &mut self.graph,
            self.semdiag.as_deref(),
            targets,
            on_event,
            cancel,
        )?;
        self.append_options_diagnostics(&mut diagnostics);
        Ok(self.apply_severity_options(diagnostics))
    }

    /// Captures the workspace's pubspec files for the pubspec lint face
    /// (#434): the root package's and every workspace member's
    /// `pubspec.yaml` joins the file table with text and line index held
    /// beside it (the options-file shape — reported rows resolve positions
    /// through the [`Engine::content`]/[`Engine::line_index`] fallbacks,
    /// never through Dart residency). Refreshed every analyze pass, so the
    /// captured text is the analyzed one and the lint stage scans exactly
    /// these bytes. One bounded read per package per pass.
    fn capture_pubspecs(&mut self) {
        let table = &self.state.packages;
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Some(root) = table.root().and_then(|name| table.get(name.as_str())) {
            dirs.push(root.root.clone());
        }
        if let Some(members) = table.workspace_members() {
            dirs.extend(members.iter().filter_map(|name| {
                table
                    .get(name.as_str())
                    .map(|identity| identity.root.clone())
            }));
        }
        for dir in dirs {
            let path = dir.join("pubspec.yaml");
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let file = self.state.files.intern(&path);
            let index = dartq_source::LineIndex::new(&text);
            self.state
                .reported
                .insert(file, (Arc::from(text.as_str()), index));
        }
    }

    /// Appends the options-file self-diagnostics (#457) to an output set:
    /// one row per issue recorded during each distinct loaded file's merge,
    /// attributed to the options file itself (the official reports every
    /// options diagnostic on the initial file — `include_walker.dart` reports
    /// include-chain rows at the initial include span, `parse_model` reports
    /// local rows at their nodes). The rows ride the same severity face as
    /// everything else (`analyzer.errors` re-levels or filters them by their
    /// own code names, the official `computeSeverity` scope). The loaded
    /// files are interned here even without issues so the CLI's
    /// `undefined_lint` face can attribute against the same table.
    ///
    /// Cold path: runs only when some options file recorded issues.
    fn append_options_diagnostics(&mut self, diagnostics: &mut Vec<Diagnostic>) {
        // Intern every loaded options file once (the CLI's `undefined_lint`
        // face attributes against the same table), keeping the
        // issue-carrying ones deduplicated by their shared load.
        let mut seen = FxHashSet::default();
        let mut loaded: Vec<(FileId, &analysis_options::AnalysisOptions)> = Vec::new();
        for options in self.state.options.values() {
            if options.file_path.as_os_str().is_empty() {
                continue;
            }
            let file = self.state.files.intern(&options.file_path);
            if !options.issues.is_empty() && seen.insert(Arc::as_ptr(options)) {
                loaded.push((file, options));
            }
        }
        if loaded.is_empty() {
            return;
        }
        let mut rows: Vec<Diagnostic> = loaded
            .into_iter()
            .flat_map(|(file, options)| {
                options
                    .issues
                    .iter()
                    .map(move |issue| issue.to_diagnostic(file))
            })
            .collect();
        rows.sort_by(|a, b| pipeline::compare_canonical(&self.state.files, a, b));
        // The engine rows leave the pipeline canonically sorted; the merged
        // set stays canonical for the rows' (rare, cold) addition.
        diagnostics.extend(rows);
        diagnostics.sort_by(|a, b| pipeline::compare_canonical(&self.state.files, a, b));
    }

    /// Applies the governing `analysis_options` severity face to an output
    /// set: every row is re-leveled or dropped per its file's
    /// `analyzer.errors` overrides ([`analysis_options::AnalysisOptions::processed_severity`],
    /// the official `computeSeverity`). The engine runs this over its own
    /// analyze output (the diagnostics output face) and the lint stage runs
    /// it over its rows, so each output row passes through exactly once.
    ///
    /// Root attribution is the discovery rule: the longest `options`-keyed
    /// package root containing the file's path; a file under no root (an
    /// overlay outside the packages) has no options and keeps its row.
    #[must_use]
    pub fn apply_severity_options(&self, diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
        // The identity fast path: no root declares an `errors` section, so
        // every row's processed severity is its declared one (cannot-ignore
        // gates ignore-comment suppression, not this face).
        if !self
            .state
            .options
            .values()
            .any(|options| options.has_severity_overrides())
        {
            return diagnostics;
        }
        let mut roots: Vec<&PathBuf> = self.state.options.keys().collect();
        roots.sort_by_key(|root| std::cmp::Reverse(root.components().count()));
        diagnostics
            .into_iter()
            .filter_map(|mut diagnostic| {
                let path = self.state.files.path(diagnostic.file)?;
                let options = roots
                    .iter()
                    .find(|root| path.starts_with(**root))
                    .and_then(|root| self.state.options.get(*root));
                diagnostic.severity = match options {
                    Some(options) => {
                        options.processed_severity(diagnostic.code.name, diagnostic.severity)?
                    }
                    None => diagnostic.severity,
                };
                Some(diagnostic)
            })
            .collect()
    }

    /// The settled per-file feature face (#722): the feature set `file`
    /// analyzes under — the `@dart` override applied, the allowed-experiment
    /// grants merged (`l1::scan_unit`'s settled output) — read from the
    /// scan face the last apply/analyze persisted, never re-derived per
    /// query. `None` when the file is not resident or its face was capped
    /// out of the cache: the caller falls back to its own (package-version)
    /// derivation, the same posture for a file outside the analyzed set.
    #[must_use]
    pub fn settled_features(&self, file: FileId) -> Option<FeatureSet> {
        self.state.settled_face(file).map(|face| face.features)
    }

    /// The `query symbol` face (PRD §20.3): every declaration of `name`
    /// across the workspace plus every resolved reference to those
    /// declarations, answered from the maintained index state (the per-file
    /// captures and per-library reference surfaces analyze keeps fresh at
    /// the `LibraryAnalyzed` point — no recapture per query).
    ///
    /// Read-after-write contract: the answer reflects the latest
    /// [`Engine::analyze`] — callers analyze first, and a query never
    /// applies the overlay, re-analyzes, or otherwise mutates derived state
    /// (the only side effect is interning `name`). Querying before the
    /// first analyze answers an empty set. Names resolve against the
    /// engine's intern table at this boundary; the rows borrow, never
    /// clone.
    #[must_use]
    pub fn query_symbols(&self, name: &str) -> SymbolAnswer<'_> {
        query::symbols(&self.state, name)
    }

    /// PRD §17.2 discovery face: package roots (from the identity table)
    /// plus `lib`/`bin`/`test`/`tool` per root and root-level `*.dart`
    /// files; never a blind tree walk. The only built-in exclusion is the
    /// oracle's dot rule, checked on every child path — directories and
    /// files alike (context_root.dart:146-166 `_isExcluded`; the same rule
    /// the CLI walk ships). `build`/`node_modules` are `analysis_options`
    /// concerns (excluded globs), not walk-level ones: the official server
    /// reports *unexcluded* `node_modules` dirs as a performance warning
    /// and advises adding them to the options exclude list
    /// (performance_page.dart:272-354), so they are demonstrably not
    /// excluded by default. The dot rule is not options-overridable — the
    /// oracle checks it before the globs; options excludes compose on top.
    /// Symlink rule, both halves: a symlinked `.dart` file is included when
    /// its target stats as a regular file (dangling links are skipped — the
    /// oracle analyzes linked files too, so skipping them would flood the
    /// differential gate with phantom oracle-only entries), while a
    /// symlinked directory is never descended into (cycle safety). Interns
    /// paths, enqueues the bytes as a `set_content` batch, and reports the
    /// file/LOC counts the metrics hook needs (one pass; LOC counts lines).
    ///
    /// # Errors
    ///
    /// Propagates I/O errors from reading discovered files.
    pub fn discover_workspace(&mut self) -> std::io::Result<DiscoveryStats> {
        const SURFACES: [&str; 4] = ["lib", "bin", "test", "tool"];

        let mut roots: Vec<PathBuf> = self
            .state
            .packages
            .iter()
            .map(|package| package.root.clone())
            .collect();
        roots.sort();
        roots.dedup();

        let mut stats = DiscoveryStats::default();
        for root in &roots {
            // The governing options (PRD §12.2): the root's entry plus every
            // nested options directory under it. Excluded paths leave the
            // analysis set — directories are not descended into, files are
            // not interned — so neither diagnostics nor lints ever see
            // them. The predicate unions each governing entry's globs,
            // anchored to their declaring directories (the official
            // `root.excludedGlobs` accumulation, context_locator.dart
            // :402-409), so a nested file's excludes prune only its subtree.
            let governing: Vec<(PathBuf, Arc<analysis_options::AnalysisOptions>)> = self
                .state
                .options
                .iter()
                .filter(|(key, _)| key.starts_with(root))
                .map(|(key, options)| (key.clone(), Arc::clone(options)))
                .collect();
            let is_excluded = |path: &Path| {
                governing
                    .iter()
                    .any(|(key, options)| path.starts_with(key) && options.is_excluded(path))
            };
            // The four declared surfaces; directly-in-root dart files are
            // collected below, never queued (the walk's `read_dir` on a file
            // is ENOTDIR and would silently drop them).
            let mut queue: Vec<PathBuf> =
                SURFACES.iter().map(|surface| root.join(surface)).collect();
            // Accepts one `.dart` file — the shared tail of the root pass
            // and the walk below. Reading and enqueueing happen after the
            // walk, parallelized over the sorted path list.
            let mut accepted: Vec<PathBuf> = Vec::new();
            if let Ok(entries) = std::fs::read_dir(root) {
                for entry in entries.flatten() {
                    let Ok(file_type) = entry.file_type() else {
                        continue;
                    };
                    let path = entry.path();
                    // The dot rule covers files too: the oracle's
                    // `_isExcluded` checks the basename of the path itself
                    // before its parent components.
                    if entry.file_name().as_encoded_bytes().starts_with(b".") {
                        continue;
                    }
                    if path.extension().is_some_and(|ext| ext == "dart")
                        && (file_type.is_file()
                            || (file_type.is_symlink()
                                && std::fs::metadata(&path).is_ok_and(|meta| meta.is_file())))
                        && !is_excluded(&path)
                    {
                        accepted.push(path);
                    }
                }
            }
            while let Some(dir) = queue.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let Ok(file_type) = entry.file_type() else {
                        continue;
                    };
                    let path = entry.path();
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if file_type.is_dir() {
                        if name.starts_with('.') || is_excluded(&path) {
                            continue;
                        }
                        queue.push(path);
                    } else if path.extension().is_some_and(|ext| ext == "dart")
                        // The dot rule covers files too: the oracle's
                        // `_isExcluded` rejects `.hidden.dart` like any dot
                        // path.
                        && !name.starts_with('.')
                        // `entry.file_type()` reports link-host facts (it does
                        // not follow symlinks): a symlinked `.dart` file is
                        // neither `is_dir` nor `is_file` there, yet the
                        // oracle analyzes it — stat the target
                        // (`fs::metadata` follows the link) and accept
                        // regular files; a dangling link has no target and
                        // is skipped. Symlinked directories are still never
                        // descended (the `is_dir` arm never queues them).
                        // Regular files take the cached-type path without a
                        // syscall.
                        && (file_type.is_file()
                            || (file_type.is_symlink()
                                && std::fs::metadata(&path)
                                    .is_ok_and(|meta| meta.is_file())))
                        && !is_excluded(&path)
                    {
                        accepted.push(path);
                    }
                }
            }
            // `read_dir` yields entries in platform-dependent order, so the
            // serial walk's enqueue order was never stable run-to-run;
            // sorting pins discovery to one deterministic path order.
            accepted.sort();
            // Cold-path I/O: read every accepted file's bytes and count its
            // LOC in parallel — `fs::read` is I/O-bound and rayon's blocking
            // pool absorbs that (the sanctioned parallel form, same as the
            // l1 batch fan-out). The walk and its acceptance predicate stay
            // serial on purpose: the predicate is pure and the walk's
            // `queue` stack is inherently sequential. Reads run in bounded
            // batches of 256, each batch enqueued and dropped before the
            // next one reads: peak transient memory is one batch's bytes,
            // not the whole workspace's (~2x source bytes on a cold load,
            // on top of the pending store's standing 1x). Sorting precedes
            // chunking, so the enqueue order, the byte-stable differential
            // face, and the first-failure-in-sorted-order error surface are
            // unchanged; per file the intern/`set_content` path and the
            // stats are identical to the unbatched form.
            for batch in accepted.chunks(256) {
                let reads: Vec<std::io::Result<(Vec<u8>, u64)>> = batch
                    .par_iter()
                    .map(|path| {
                        std::fs::read(path).map(|bytes| {
                            let loc = u64::from(
                                u32::try_from(bytes.iter().filter(|&&b| b == b'\n').count())
                                    .unwrap_or(u32::MAX),
                            );
                            (bytes, loc)
                        })
                    })
                    .collect();
                // Enqueue in the same sorted order the reads were issued in.
                // The pending overlay is last-wins per path, so content would
                // land identically in any order, but a fixed enqueue order
                // keeps the differential harness byte-stable. A read failure
                // surfaces at its sorted position with everything before it
                // already enqueued — the surfaced error is the first in sorted
                // order (the old serial version surfaced whichever read failed
                // first in platform-dependent read_dir order, itself never
                // stable).
                for (path, read) in batch.iter().zip(reads) {
                    let (bytes, loc) = read?;
                    stats.loc_total += loc;
                    stats.file_count += 1;
                    self.set_content(path, &bytes);
                }
            }
        }
        Ok(stats)
    }

    /// The engine's file table (the CLI's `FileLayoutProvider` backing).
    #[must_use]
    pub fn file_table(&self) -> &FileTable {
        &self.state.files
    }

    /// The context's package identity table — the lint face's package
    /// attribution (`PackageTable::containing`) and its workspace member
    /// set ([`PackageTable::workspace_members`]).
    #[must_use]
    pub fn packages(&self) -> &dartq_package::PackageTable {
        &self.state.packages
    }

    /// The merged analysis options governing `root` (a package root), or
    /// `None` when the root has no options file in its ancestry. The lint
    /// face resolves its enable set from [`analysis_options::AnalysisOptions::lint_rules`];
    /// discovery has already applied [`analysis_options::AnalysisOptions::is_excluded`]
    /// over every governing entry (the root's plus the nested options
    /// directories inside it).
    #[must_use]
    pub fn options_for_root(&self, root: &Path) -> Option<&analysis_options::AnalysisOptions> {
        self.state.options.get(root).map(Arc::as_ref)
    }

    /// The line index of a resident file. Options-file rows resolve through
    /// the options face: their file is never resident as Dart, and the
    /// captured load-time index is the positions source
    /// ([`analysis_options::AnalysisOptions`] layout).
    #[must_use]
    pub fn line_index(&self, file: FileId) -> Option<&LineIndex> {
        if let Some(facts) = self.state.residency.get(&file) {
            return Some(&facts.line_index);
        }
        if let Some((_, index)) = self.state.reported.get(&file) {
            return Some(index);
        }
        let path = self.state.files.path(file)?;
        self.state
            .options
            .values()
            .find(|options| options.file_path == path)
            .and_then(|options| options.layout.as_ref())
            .map(|(_, index)| index)
    }

    /// The resident bytes of a file. Reported non-Dart files (the pubspec
    /// face) and options-file rows resolve through their captured layouts
    /// (see [`Engine::line_index`]).
    #[must_use]
    pub fn content(&self, file: FileId) -> Option<&[u8]> {
        if let Some(facts) = self.state.residency.get(&file) {
            return self.state.store.get(facts.content);
        }
        if let Some((text, _)) = self.state.reported.get(&file) {
            return Some(text.as_bytes());
        }
        let path = self.state.files.path(file)?;
        self.state
            .options
            .values()
            .find(|options| options.file_path == path)
            .and_then(|options| options.layout.as_ref())
            .map(|(text, _)| text.as_bytes())
    }

    /// The resident facts of one file — the baked API signature, the
    /// element model, the directives, the syntax + URI-layer diagnostics,
    /// the line index — or `None` when the file is not resident. The read
    /// face batch queries assemble over (#341's accessor face): a query
    /// reads what analysis already computed instead of re-deriving it.
    #[must_use]
    pub fn file_facts(&self, file: FileId) -> Option<&state::UnitFacts> {
        self.state.residency.get(&file).map(Arc::as_ref)
    }

    /// The maintained capture of a resident file — the declaration surface
    /// plus raw reference sites analyze recaptures per changed file
    /// (index-design-spec §8) — or `None` before the file's first analyze
    /// covers it. The raw-row read face the index-backed queries assemble
    /// over: [`Engine::query_symbols`]'s substrate at row fidelity, the
    /// rows' `StringId`s native to [`Engine::strings`].
    #[must_use]
    pub fn file_capture(&self, file: FileId) -> Option<&FileCapture> {
        self.state.captures.get(&file).map(Arc::as_ref)
    }

    /// Every live library's maintained reference surface, ascending by
    /// library id — rebuilt whole at each `LibraryAnalyzed` point and
    /// dropped with the library at retirement, the fresh partition
    /// [`Engine::query_symbols`] reads.
    pub fn library_surfaces(&self) -> impl Iterator<Item = (LibraryId, &LibraryIndex)> + '_ {
        self.state
            .library_surfaces
            .iter()
            .map(|(library, surface)| (*library, surface))
    }

    /// The engine's intern table — the one table every resident fact's
    /// `StringId`s resolve against. Mutex-guarded interning makes sharing
    /// it the contract: a query reading resident facts resolves (and, on
    /// its own additions, interns) here, exactly the documented
    /// `query_symbols` posture — the only side effect is interning.
    #[must_use]
    pub fn strings(&self) -> &StringTable {
        &self.state.strings
    }

    /// Distinct contents the store currently holds — the entry-count half of
    /// the store's observable memory budget. Under steady editing the count
    /// is bounded by residency + the facts-cache cap + the sweep threshold
    /// (amortized mark-sweep; the daemon memory law).
    #[must_use]
    pub fn stored_contents(&self) -> usize {
        self.state.store.len()
    }

    /// Total bytes the store currently holds — the byte-count half of the
    /// store's observable memory budget.
    #[must_use]
    pub fn stored_bytes(&self) -> u64 {
        self.state.store.len_bytes()
    }

    /// The library owning `file` (defining or part), if resident.
    #[must_use]
    pub fn library_of(&self, file: FileId) -> Option<LibraryId> {
        self.state.library_of_file.get(&file).copied()
    }

    /// The defining file of a live library. Singleton convention
    /// (incremental-oracle-cases §4): a singleton library's sole member is its
    /// defining file — the face the incremental oracle's `invalidated` →
    /// defining-path projection reads through.
    #[must_use]
    pub fn library_defining_file(&self, library: LibraryId) -> Option<FileId> {
        self.state.libraries.info(library).map(|info| info.defining)
    }

    /// Live libraries with their member files (defining first, then parts
    /// in directive order).
    #[must_use]
    pub fn libraries(&self) -> Vec<(LibraryId, Vec<FileId>)> {
        self.state
            .libraries
            .iter_infos()
            .map(|(id, info)| (id, pipeline::member_files(info)))
            .collect()
    }

    /// Library-level import dependency graph, CSR form (GPU posture: flat
    /// `&[u32]` offsets/edges, zero-copy exportable).
    #[must_use]
    pub fn library_import_graph(&self) -> CsrGraph {
        pipeline::library_import_csr(&self.state)
    }

    /// File-level directive reference graph, CSR form (same posture).
    #[must_use]
    pub fn file_reference_graph(&self) -> CsrGraph {
        pipeline::file_reference_csr(&self.state)
    }

    /// The generation header facts snapshots key on (PRD §5.5/§5.6): the
    /// container format version, the engine's `FACTS_SEMANTIC_VERSION`,
    /// the SDK identity, the package-graph fingerprint, and the context
    /// salt (the remaining fork dimensions — target, declared variables,
    /// analysis options). The same header drives `SnapshotStore::load` at
    /// warm start and `save` at flush; callers never derive it by hand.
    #[must_use]
    pub fn snapshot_header(&self) -> GenerationHeader {
        let mut roots: Vec<PathBuf> = self
            .state
            .packages
            .iter()
            .map(|package| package.root.clone())
            .collect();
        roots.sort();
        roots.dedup();
        GenerationHeader {
            format_version: SNAPSHOT_FORMAT_VERSION,
            semantic_version: state::FACTS_SEMANTIC_VERSION,
            sdk_identity: state::persist::sdk_identity(&self.state.sdk, &self.state.target),
            package_graph_fingerprint: state::persist::package_graph_fingerprint(
                &self.state.packages,
            ),
            context_salt: state::persist::context_salt(
                &self.state.target,
                &self.state.declared_variables,
                &roots,
            ),
        }
    }

    /// Writes the resident facts into `store` as one generation — the M3
    /// flush face the daemon calls after analyze. One [`Entry`] per
    /// distinct `(content, feature, semantic)` identity over residency,
    /// under [`Self::snapshot_header`]. The entry key is path-free, so
    /// same-content files share one payload (the path-smallest file's — a
    /// deterministic pick, so an unchanged workspace's re-save leaves
    /// entry frames, and their files, byte-identical). The remap slices
    /// stay empty: the facts payload is self-contained (per-entry string
    /// slices and file paths), so no generation-wide id space is needed.
    ///
    /// # Errors
    ///
    /// Propagates [`SaveError`] — a failed flush is a future miss (the
    /// accelerator invariant), never a correctness loss.
    pub fn save_facts_snapshot(&self, store: &SnapshotStore) -> Result<usize, SaveError> {
        let mut files: Vec<FileId> = self.state.residency.keys().copied().collect();
        files.sort_unstable_by(|a, b| {
            let path = |file: FileId| {
                self.state
                    .files
                    .path(file)
                    .map_or(&[][..], |path| path.as_os_str().as_encoded_bytes())
            };
            path(*a).cmp(path(*b))
        });
        let mut entries: Vec<Entry<PersistedFacts>> = Vec::with_capacity(files.len());
        let mut claimed: FxHashSet<EntryKey> = FxHashSet::default();
        for file in files {
            let facts = &self.state.residency[&file];
            let key = l1::facts_key(&self.state.grants(), file, facts.content());
            let key = EntryKey {
                content_hash: *key.content.as_bytes(),
                feature_fingerprint: key.feature_fingerprint,
                semantic_version: key.semantic_version,
            };
            if !claimed.insert(key) {
                continue;
            }
            entries.push(Entry {
                key,
                payload: PersistedFacts::of(facts, &self.state.strings, &self.state.files),
            });
        }
        let saved = entries.len();
        store.save(&self.snapshot_header(), &RemapTables::default(), &entries)?;
        Ok(saved)
    }

    /// Warm start (the M3 load face): stages a loaded facts generation for
    /// the next apply. Staging is keyed by the entry triple; the apply's
    /// lookup IS the content-hash verification — an entry serves only a
    /// file whose live content id equals the key's hash, and everything
    /// stale drops at that first apply. Ingestion is grow-only: it seeds
    /// recomputes, never touching residency (#377). The loaded header and
    /// remap tables are the store's match gate — consumed by `load`, not
    /// needed here.
    ///
    /// Returns the number of staged entries.
    pub fn ingest_facts_snapshot(&mut self, snapshot: Snapshot<PersistedFacts>) -> usize {
        let staged = snapshot.entries.len();
        self.state.warm_start.extend(
            snapshot
                .entries
                .into_iter()
                .map(|entry| (entry.key, entry.payload)),
        );
        staged
    }
}

/// What one [`Engine::apply_pending`] changed (§1.2).
#[derive(Debug, Default)]
pub struct AffectedReport {
    /// Files whose ContentId differed from residency (gate passed, work done).
    pub changed: Vec<FileId>,
    /// Files removed by this apply.
    pub removed: Vec<FileId>,
    /// Post-closure dirty library set (what analyze must recompute).
    pub invalidated: Vec<LibraryId>,
}

/// Discovery counters (§3 metrics hook, PRD §17.2).
#[derive(Debug, Default)]
pub struct DiscoveryStats {
    /// Discovered `.dart` files.
    pub file_count: usize,
    /// Total lines across the discovered set.
    pub loc_total: u64,
}

/// Flat CSR arrays: `offsets[i]..offsets[i+1]` slice `edges` for node `i`.
/// Both views are plain `&[u32]` — the zero-copy export face for external
/// compute backends (GPU posture ADR, engine-api-spec §3 rider).
#[derive(Debug, Clone, Default)]
pub struct CsrGraph {
    offsets: Vec<u32>,
    edges: Vec<u32>,
}

impl CsrGraph {
    /// Per-node start offsets; `nodes() + 1` entries.
    #[must_use]
    pub fn offsets(&self) -> &[u32] {
        &self.offsets
    }

    /// Packed adjacency targets.
    #[must_use]
    pub fn edges(&self) -> &[u32] {
        &self.edges
    }

    /// Number of nodes.
    #[must_use]
    pub fn nodes(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    /// Edge count.
    #[must_use]
    pub fn len_edges(&self) -> usize {
        self.edges.len()
    }
}

/// Read-only view of one library for the semantic diagnostics seam — the
/// context #120/#121 diagnose against (semantic crate's assumption contract
/// documents what these fields guarantee). The per-unit faces a
/// `dartq_semantic::diagnose_with` fold needs ride here: source text
/// ([`LibraryView::store`] + the facts' content ids) and the token table
/// (the persisted scan face of [`LibraryView::scanned`], filled by the
/// apply-phase recompute — a miss re-derives the identical face locally).
pub struct LibraryView<'a> {
    /// The engine library id.
    pub library: LibraryId,
    /// Member files: defining first, then parts in directive order.
    pub files: &'a [FileId],
    /// The interned library graph over the current analysis set.
    pub graph: &'a dartq_resolution::LibraryGraph,
    /// This library's id in [`LibraryView::graph`].
    pub graph_library: dartq_resolution::LibraryId,
    /// The shared string intern table (resolve `StringId`s against it).
    pub strings: &'a dartq_source::StringTable,
    /// Every resident file's facts: directives, element model, syntax
    /// diagnostics, line index. Read-only snapshot.
    pub facts: &'a FxHashMap<FileId, Arc<UnitFacts>>,
    /// The SDK snapshot (`dart:` library support queries).
    pub sdk: &'a SdkSnapshot,
    /// The content store backing the facts' content ids (per-unit source).
    pub store: &'a (dyn dartq_source::SourceStore + Send + Sync),
    /// The package identity table (per-file language version source).
    pub packages: &'a PackageTable,
    /// The interned paths (file → path for the package attribution).
    pub file_table: &'a FileTable,
    /// The memoized per-file experiment grants (audit #708 P2-1): a local
    /// re-scan on a scanned-face miss merges the same whitelist the apply
    /// recompute did, so the re-derived face is identical.
    pub grants: &'a FeatureGrants,
    /// The import namespaces of the current graph (built once per graph
    /// generation).
    pub namespaces: &'a dartq_resolution::Namespaces,
    /// The element models of every resident file, indexed by [`FileId`] —
    /// built once per analyze over the whole residency and shared by every
    /// per-library fold (#775). Entries are the facts' own `Arc`s, so the
    /// share costs refcount bumps, never clones.
    pub index: &'a dartq_semantic::ElementIndex,
    /// The language-strictness modes governing the library (the defining
    /// file's analysis options, `unwrap_or(false)` per flag — the official
    /// off posture): the `analyzer.language.strict-*` config → gate face
    /// (#588 item 4, consumed by `dartq_semantic::diagnose_with`).
    pub strict: dartq_semantic::StrictModes,
    /// The member files' persisted scan faces (token table + settled
    /// feature set, keyed by [`FileId`]); entries exist for cache hits the
    /// engine resolved before the call. A missing entry means the fold
    /// derives the face itself (`l1::scan_unit`) — the map never blocks a
    /// diagnoser that scans its own tokens.
    pub scanned: FxHashMap<FileId, Arc<crate::state::ScannedFace>>,
}

/// The semantic diagnostics plug point (#120/#121).
///
/// The engine ships the default implementor ([`semdiag::Diagnoser`], folding
/// `dartq_semantic::diagnose_with`); a caller may substitute its own. Per-library
/// calls arrive on the batch fan-out workers, so implementations must be
/// `Send + Sync` and stateless across calls; diagnostics append into `out`
/// and carry no ordering obligations (the engine sorts canonically).
pub trait SemanticDiagnoser: Send + Sync {
    /// Appends this library's semantic diagnostics to `out`.
    fn diagnose_library(&self, library: LibraryView<'_>, out: &mut Vec<Diagnostic>);
}

/// The M1 default store constructor (`MemorySourceStore`, §1.1).
#[must_use]
pub fn memory_store() -> Box<dyn SourceStore + Send + Sync> {
    Box::new(MemorySourceStore::new())
}

/// A store wired to `pool`: every distinct content the engine puts lands in
/// the pool's shared storage, so N contexts constructed over one pool hold
/// each content exactly once between them (§16.1's shared content store —
/// the multi-context RSS face). Reads stay zero-copy through the context's
/// own view; each context's GC drops only its own handles, and the pool
/// reclaims storage no context references anymore (reference-counted
/// sweep, amortized on fresh insertions).
///
/// The daemon passes one [`ContentPool`] to every context's
/// [`EngineConfig::store`] this way; standalone engines keep
/// [`memory_store`].
#[must_use]
pub fn pooled_memory_store(pool: &ContentPool) -> Box<dyn SourceStore + Send + Sync> {
    Box::new(PooledSourceStore::new(pool))
}

/// Drops exactly one leading UTF-8 BOM (`EF BB BF`, U+FEFF) — oracle
/// read-layer parity (`.third/dart-sdk` `_fe_analyzer_shared` utf.dart
/// Before-BOM state machine via the analyzer file read): the scanner runs on
/// BOM-less text and every span is relative to the stripped text. Exactly
/// one is dropped; a second BOM is content.
pub(crate) fn strip_one_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    use dartq_language::Feature;
    use dartq_pub::{LockFile, Pubspec, build_graph};

    #[test]
    fn bom_stripped_once() {
        assert_eq!(strip_one_bom("\u{FEFF}var x;".as_bytes()), b"var x;");
        // BOM-less input passes through untouched.
        assert_eq!(strip_one_bom(b"var x;"), b"var x;");
    }

    #[test]
    fn double_bom_leaves_second_as_content() {
        assert_eq!(
            strip_one_bom("\u{FEFF}\u{FEFF}x".as_bytes()),
            // One BOM dropped, the second survives as content (EF BB BF 78).
            "\u{FEFF}x".as_bytes()
        );
    }

    #[test]
    fn bom_only_strips_to_empty() {
        assert_eq!(strip_one_bom("\u{FEFF}".as_bytes()), b"");
    }

    /// A throwaway single-package workspace + synthetic core-only SDK — the
    /// l1/semdiag test fixture shape trimmed to what [`Engine::new`] binds —
    /// backing the public-facade tests.
    fn facade_engine() -> (PathBuf, Engine) {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "dartq-engine-facade-{}-{}",
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
        let root = root.canonicalize().expect("fixture root resolves");
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
            &std::collections::BTreeMap::new(),
            None,
            &config_dir,
        )
        .expect("graph");
        let engine = Engine::new(EngineConfig {
            store: memory_store(),
            sdk: SdkSnapshot::load(&root.join("sdk"), None).expect("synthetic SDK loads"),
            packages: PackageTable::build(&config, &graph, &config_dir).expect("table"),
            pub_config: Arc::clone(&config),
            config_dir,
            semdiag: None,
            target: DEFAULT_TARGET.to_owned(),
            declared_variables: FxHashMap::default(),
        })
        .expect("engine");
        (root, engine)
    }

    /// The public settled-face accessor (#722 seam) through the facade: a
    /// `set_content`-interned but not-yet-applied file answers `None` (not
    /// resident — the caller-fallback signal); after `apply_pending` the
    /// same id answers the persisted face, the package-baseline feature set
    /// for this fixture. Mirrors `l1`'s accessor test through the public
    /// method instead of the internal state.
    #[test]
    fn settled_features_projects_the_persisted_face() {
        let (root, mut engine) = facade_engine();
        let file = engine.set_content(&root.join("lib/a.dart"), b"void f() {}\n");
        assert!(
            engine.settled_features(file).is_none(),
            "interned but not resident: nothing settled yet"
        );
        engine.apply_pending(&mut |_| {});
        let features = engine
            .settled_features(file)
            .expect("a resident file has a persisted face");
        assert!(
            features.is_enabled(Feature::Patterns),
            "the unpinned 3.5 package keeps the baseline posture"
        );
    }
}
