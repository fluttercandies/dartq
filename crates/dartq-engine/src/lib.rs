//! The dartq engine facade: one analysis context, two-phase content
//! updates, and topologically ordered library recompute.
//!
//! # Shape
//!
//! * One [`Engine`] binds the immutable context — content store, SDK
//!   snapshot, package identity table, and the pub config the URI layer
//!   resolves against. Context change = new Engine.
//! * [`Engine::set_content`] / [`Engine::remove_file`] only enqueue into the
//!   pending overlay (official `changeFile`: enqueue, never compute);
//!   [`Engine::apply_pending`] is the barrier that runs the `ContentId` gate,
//!   the per-file facts recompute, and the L1 cascade.
//! * [`Engine::analyze`] applies as its fixed first phase, recomputes dirty
//!   libraries in import-edge topological order on the batch fan-out, and
//!   returns the merged, canonically sorted diagnostic set.
//! * [`EngineEvent`]s arrive on a synchronous `FnMut` callback, in work
//!   order; the returned diagnostics are in canonical order — the two are
//!   deliberately distinct.
//!
//! # Module layout
//!
//! `state` (residency + overlay + reverse edges), `l1` (apply), `pipeline`
//! (analyze), `cycle`, `budget`
//! (the memory-budget runtime), `store` (the
//! L3 content-addressed store), `event`, `cancel`, `query` (the `query
//! symbol` face), `semdiag` (the default semantic diagnoser, #236),
//! `reachability` (the per-declaration entry-reachability pass, #1199),
//! `analysis_options`, `context` +
//! `scheduler` (the multi-context runtime substrate — contexts, overlays,
//! scheduling, cancellation —).
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
mod cycle;
mod deps;
pub mod event;
pub mod l1;
pub mod pipeline;
pub mod query;
pub mod reachability;
pub mod scheduler;
pub mod sdk_extension_faces;
pub mod sdk_faces_gen;
mod sdk_units;
pub mod semdiag;
pub mod state;
pub mod store;
mod warm;

use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError};
use std::time::SystemTime;

use dartq_diagnostics::Diagnostic;
use dartq_index::{FileCapture, LibraryIndex};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_package::PackageTable;
use dartq_pub::{PackageConfig, PubError};
use dartq_sdk::SdkSnapshot;
use dartq_snapshot::{
    Entry, EntryKey, GenerationHeader, RemapTables, SNAPSHOT_FORMAT_VERSION, SaveError, Snapshot,
    SnapshotStore,
};
use dartq_source::{
    FileId, FileTable, LineIndex, MemorySourceStore, MmapSourceStore, OpenError,
    PooledMappedSourceStore, PooledSourceStore, SourceStore, SourceText, StringTable,
    saturate_slot_u32,
};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use thiserror::Error;

pub use cancel::{CancellationToken, Cancelled};
pub use context::{ContextConfig, ContextId, ContextKey, OverlayChange, Runtime};
pub use dartq_source::{ContentPool, MappedContentPool};
pub use event::{EngineEvent, RevalidateGate};
pub use query::{
    CompletionAnswer, CompletionItem, CompletionOrigin, DeclHit, DeclKind, PositionAnswer, RefHit,
    RefKind, ReferenceKind, SymbolAnswer, UseHit,
};
pub use reachability::Reachability;
pub use scheduler::{DrainReport, Task, Tier};
pub use state::persist::PersistedFacts;
pub use state::{DirectiveFacts, DirectiveKind, FeatureGrants, LibraryId, ScannedFace, UnitFacts};
pub use warm::{WarmStats, WarmStep};

pub use budget::MemoryStatus;

/// The default SDK target segment (`libraries.json` panel) `dart:` URI
/// resolution dispatches against — the `dart analyze` posture. Every
/// [`EngineConfig`] constructor spells it unless the context selects
/// otherwise; the constant is the single source of the default.
pub const DEFAULT_TARGET: &str = "vm";

/// The warm-start staging map's per-ingest entry cap. One staged payload is
/// the persisted facts face, 2–4 KB owned bytes (est., the memory audit's
/// F8 band), so the cap holds the staging map to ≈ 0.25–0.5 GB even for a
/// pathological snapshot — an earlier bound than the armed memory budget's
/// 2 GiB tier-3 ladder. A 10⁵-identity workspace (99,598 entries at the
/// measured face) stages whole; identities past the cap skip staging and
/// their files cold-recompute — staging is an accelerator, so the output
/// face is byte-identical either way. Head entries win (snapshot order =
/// the saving pass's path order), keeping the pick deterministic.
const WARM_START_STAGING_CAP: usize = 1 << 17;

/// Discovery's load chunk in files: one chunk's read results are in flight
/// at a time, each chunk enqueued before the next loads. Sized to keep
/// every worker busy across ~128 32-path normalization slices while the
/// result vector (text lease + LOC + normalized path, ~80 B/row plus the
/// path) stays ~1 MB; the texts themselves live in the pending overlay
/// until apply regardless of chunking.
const DISCOVERY_LOAD_CHUNK: usize = 4096;

/// Everything immutable across the engine's lifetime: the M1 slice
/// of the six context fields — store, SDK, packages — plus the pub
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
    /// them — the multi-context store face (the shared content store,
    /// the M3 note this field's doc drafted). The handle itself stays
    /// per-engine, so reads keep the zero-copy `get` contract and each
    /// engine's own amortized GC path. [`memory_store`] is the standalone
    /// default.
    pub store: Box<dyn SourceStore + Send + Sync>,
    /// SDK snapshot; loading hard-errors on a crippled SDK before
    /// construction (dartq-sdk doc).
    pub sdk: SdkSnapshot,
    /// Package identity table over one resolution; carries the
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
    /// The CLI `--enable-experiment` flags (#938) — the context-level
    /// feature base every package file's feature set starts from; an
    /// options chain declaring its own `enable-experiment` section replaces
    /// it for that subtree (the official `_applyEnableExperiments`
    /// assignment). Raw spellings; unknown names are the CLI's usage tier,
    /// expired ones decode to nothing (the official expiry posture). Empty
    /// by default — the no-flag oracle context.
    pub experiments: Vec<String>,
}

/// Engine construction and explicit-API failures.
#[derive(Debug, Error)]
pub enum EngineError {
    /// `remove_file` of a path the engine never interned and does not
    /// track (deliberate deviation from the official silent no-op:
    /// dartq reports explicitly).
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

/// The analysis driver.
pub struct Engine {
    state: state::EngineState,
    graph: Option<pipeline::ResolvedGraph>,
    semdiag: Option<Arc<dyn SemanticDiagnoser>>,
    /// The captured pubspecs' mtimes at capture time — the read guard for
    /// [`Engine::capture_pubspecs`]: a pubspec is re-read only when its
    /// mtime moved since the last capture.
    pubspec_mtimes: FxHashMap<PathBuf, SystemTime>,
    /// The captured Android manifests' mtimes at capture time — the read
    /// guard for the `<pkg>/android/AndroidManifest.xml` mount
    /// [`Engine::capture_pubspec_at`] runs beside each pubspec (the
    /// oracle's driver walks every `AndroidManifest.xml` under the
    /// analysis roots the same way it walks pubspecs —
    /// `analyzer_cli/lib/src/driver.dart` :346, `_collectFiles`
    /// :427-448). The pass emits only when the governing options enable
    /// `analyzer.optional-checks.chrome-os-manifest-checks`.
    manifest_mtimes: FxHashMap<PathBuf, SystemTime>,
    /// The discovery loads' disk stamps (mtime, size) by normalized path —
    /// the read guard for [`Engine::refresh_workspace`] (#2596): a source
    /// is re-read only when its stamp moved since the last load. Seeded by
    /// [`Engine::discover_workspace`] and kept current by every refresh
    /// load and removal. A filesystem that reports no mtime records
    /// `None` and the file re-reads every pass (the pubspec guard's
    /// posture).
    source_stamps: FxHashMap<PathBuf, Option<(SystemTime, u64)>>,
    /// The assembly's config directory (normally `.dart_tool/` under the
    /// workspace root) — the context root the snapshot generation salt
    /// folds. A pubspec-less workspace's package table is empty, so this
    /// is the header dimension keeping two byte-identical bare trees on
    /// distinct generations (#2274): same path, same salt; a different
    /// tree forks.
    context_root: PathBuf,
    /// The nested-pubspec scan's already-probed directories, tagged with
    /// the residency epoch their scan covered (the generation-tag memo —
    /// the moving counter is [`state::EngineState::residency_epoch`]). A
    /// directory is probed once per engine, so a daemon turn's cost is
    /// O(new dirs), not O(workspace); a pubspec created later in an
    /// already-scanned directory is picked up by the next fresh run (the
    /// oracle's own workspace discovery snapshots per context build the
    /// same way).
    pubspec_dirs_scanned: FxHashMap<PathBuf, u64>,
    /// The residency epoch the nested-pubspec scan last covered. A turn
    /// whose [`state::EngineState::residency_epoch`] still equals this
    /// grew no new resident files, so it cannot surface an unscanned
    /// directory — the turn's whole resident-set walk is skipped in O(1).
    nested_scan_epoch: u64,
    /// Test instrumentation: how many scan passes actually ran (the gate
    /// opened). The warm-turn pin asserts it stays put.
    #[cfg(test)]
    nested_scan_passes: u32,
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
            &config.experiments,
        )?;
        Ok(Self {
            state,
            graph: None,
            semdiag,
            context_root: config.config_dir,
            pubspec_mtimes: FxHashMap::default(),
            manifest_mtimes: FxHashMap::default(),
            source_stamps: FxHashMap::default(),
            pubspec_dirs_scanned: FxHashMap::default(),
            nested_scan_epoch: 0,
            #[cfg(test)]
            nested_scan_passes: 0,
        })
    }

    /// Arms (or disarms) the memory-budget runtime. A limit
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

    /// Arms (or disarms) the `--trace-events` JSONL sink: every
    /// later apply/analyze appends one JSON object per engine event —
    /// stable taxonomy names, stable field order, flushed per event so a killed CI
    /// run keeps its tail. `None` (the default) constructs nothing and the
    /// output face is byte-identical to an untraced run.
    pub fn set_trace_sink(&mut self, sink: Option<event::TraceSink>) {
        self.state.trace = sink;
    }

    /// Drops the standing condensation and marks the structure dirty, so
    /// the next analyze re-derives it wholesale — every cycle folds fresh,
    /// the pre-salvage shape. The rebuilt values are
    /// byte-identical either way (the salvage moves fold cost only), which
    /// makes this the wholesale baseline the incremental benchmarks A/B the
    /// salvaged curve against.
    /// Calling it outside that measurement posture is meaningless: the
    /// mapping is pure derived data, rebuilt at the structure gate.
    #[doc(hidden)]
    pub fn clear_condensation(&mut self) {
        self.state.condensation = crate::cycle::Condensation::default();
        self.state.structure_dirty = true;
    }

    /// Arms (or re-arms) the L3 content-addressed store: the
    /// persistent, cross-session global under `config.root` that every
    /// later apply batch's changed contents persist into (write batch =
    /// puts, then flush — journal append + amortized compaction + cap
    /// enforcement). Opening
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
        config: &store::StoreConfig,
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
    /// `pkg/analyzer/lib/src/dart/analysis/file_content_cache.dart`) decodes
    /// strictly and on any decode failure marks the file nonexistent
    /// (`exists = false`, empty content) — an undecodable file contributes
    /// zero diagnostics. The `encoding` diagnostic ("Unable to decode bytes
    /// as UTF-8.", pkg/_`fe_analyzer_shared/lib/src/messages/diagnostic.g.dart`,
    /// scanner-path translation at pkg/analyzer/lib/src/dart/scanner/
    /// `translate_error_token.dart`) belongs to the compiler's byte-scanner
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
        self.set_source(path, SourceText::canonical(bytes))
    }

    /// Enqueues pre-canonicalized text (the overlay face): `text` is already
    /// the engine's canonical form — one leading BOM stripped, invalid UTF-8
    /// lossily decoded — and is stored as-is, with no second decode (a
    /// leading BOM in already-canonical text stays content). The lease's
    /// content id is the identity residency derives on apply.
    pub(crate) fn set_source(&mut self, path: &Path, text: SourceText) -> FileId {
        let file = self.state.files.intern(path);
        self.queue_source(file, text);
        file
    }

    /// [`Engine::set_source`] with the path normalization computed by the
    /// caller (`normalized == dartq_source::normalize_path(path)`): the
    /// discovery batch derives it on the read fan-out, so the serial id pass
    /// pays no `realpath`.
    fn set_source_normalized(&mut self, path: &Path, normalized: PathBuf, text: SourceText) {
        let file = self.state.files.intern_normalized(path, normalized);
        self.queue_source(file, text);
    }

    fn queue_source(&mut self, file: FileId, text: SourceText) {
        if !self.state.pending.contains_key(&file) {
            self.state.pending_order.push(file);
        }
        self.state
            .pending
            .insert(file, state::PendingChange::Set(text));
    }

    /// Enqueues a removal. Unknown path — never interned and not pending —
    /// is an explicit error (official `changeFile` silently no-ops; dartq
    /// reports explicitly).
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

    /// Records the session live-path ceiling trip (#1887): one residency
    /// degrade row naming the ceiling, anchored at `anchor` — the refused
    /// batch's representative path. A path the session never interned is
    /// interned for the row's anchor only: an id, never content or
    /// residency, so the refusal stays a refusal. At-most-once emission is
    /// the caller's discipline (the daemon trips the row once per session);
    /// the log's (file, face) dedup holds regardless.
    pub fn note_live_path_ceiling(&mut self, anchor: &Path) {
        let file = self
            .state
            .files
            .try_file_id(anchor)
            .unwrap_or_else(|| self.state.files.intern(anchor));
        self.state.residency_degrades.record_ceiling(file);
    }

    /// Barrier: drains the pending queue and runs the L1 cascade
    ///. Events fire only for a non-empty queue.
    pub fn apply_pending(&mut self, on_event: &mut dyn FnMut(EngineEvent)) -> AffectedReport {
        let outcome = l1::apply(&mut self.state, on_event);
        pipeline::flush_l3(&mut self.state, &outcome.changed);
        // The batch's moved files join the structure gate's refresh input:
        // an out-of-analyze apply leaves the analyze's own applied sets
        // empty, and without this record the gate would reuse a standing
        // graph that lacks the batch's formed libraries (the daemon's
        // write face — every overlay apply lands here). Relinked residents
        // ride the same accumulator: their facts rewrote inside the apply,
        // and a resolved-target FileId flip moves the folded digest face.
        self.state
            .structure_changed
            .extend(outcome.changed.iter().copied());
        self.state
            .structure_changed
            .extend(outcome.relinked.iter().copied());
        self.state
            .structure_removed
            .extend(outcome.removed.iter().copied());
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
    /// byte-equality arm, so `targets` drives only the `AnalyzeFile`
    /// amplification events). The owned twin of [`Self::analyze_shared`]:
    /// the same pipeline, the rows cloned out of the standing buffer.
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
        )?
        .as_ref()
        .clone();
        self.capture_nested_pubspecs();
        // The settle's arming point (#2739): a completed analyze arms the
        // `Tier::Background` warm lane — O(1), the plan's items
        // materialize at the first `warm_step`. A cancelled run skipped
        // the `?` above, so a partial settle arms nothing.
        self.state.arm_warm();
        let mut rows = self.option_issue_rows();
        rows.extend(self.validator_issue_rows());
        if !rows.is_empty() {
            // The engine rows leave the pipeline canonically sorted; the
            // merged set stays canonical for the rows' (rare, cold) addition.
            diagnostics.extend(rows);
            diagnostics.sort_by(|a, b| pipeline::compare_canonical(&self.state.files, a, b));
        }
        Ok(self.apply_severity_options(diagnostics))
    }

    /// The shared-output twin of [`Self::analyze`] (#1387): the identical
    /// contract and faces, with the merged rows handed over as one
    /// refcounted `Arc<Vec<Diagnostic>>` instead of an owned clone. A cycle
    /// in which no contributor re-analyzed — the interactive keystroke's
    /// dominant shape — returns a bump on the standing buffer, so the
    /// per-cycle cost no longer carries a clone per diagnostic row; the
    /// holding caller keeps the rows valid across cycles and gives that up
    /// only at the next rebuild that cannot move them (the splice's
    /// full-merge arm). The options-file and severity faces run exactly as
    /// in [`Self::analyze`]; when neither has anything to apply (the common
    /// case) the Arc returns untouched.
    ///
    /// # Errors
    ///
    /// [`Cancelled`] when a checkpoint fired: completed libraries keep their
    /// cached results, the rest stay dirty.
    pub fn analyze_shared(
        &mut self,
        targets: &[FileId],
        on_event: &mut dyn FnMut(EngineEvent),
        cancel: &CancellationToken,
    ) -> Result<Arc<Vec<Diagnostic>>, Cancelled> {
        self.capture_pubspecs();
        let shared = pipeline::analyze(
            &mut self.state,
            &mut self.graph,
            self.semdiag.as_deref(),
            targets,
            on_event,
            cancel,
        )?;
        self.capture_nested_pubspecs();
        // The settle's arming point (#2739), as in [`Self::analyze`].
        self.state.arm_warm();
        let mut rows = self.option_issue_rows();
        rows.extend(self.validator_issue_rows());
        if rows.is_empty() && !self.severity_overrides_present() {
            return Ok(shared);
        }
        let mut diagnostics = shared.as_ref().clone();
        if !rows.is_empty() {
            diagnostics.extend(rows);
            diagnostics.sort_by(|a, b| pipeline::compare_canonical(&self.state.files, a, b));
        }
        Ok(Arc::new(self.apply_severity_options(diagnostics)))
    }

    /// Captures the workspace's pubspec files for the pubspec lint face
    /// (#434): the root package's and every workspace member's
    /// `pubspec.yaml` joins the file table with text and line index held
    /// beside it (the options-file shape — reported rows resolve positions
    /// through the [`Engine::content`]/[`Engine::line_index`] fallbacks,
    /// never through Dart residency). Guarded by mtime: one stat per
    /// package per pass, one read per pubspec whose mtime actually moved
    /// since the last capture, so the captured text is still the analyzed
    /// one and the lint stage scans exactly these bytes (a failed read
    /// records no mtime, so it retries the next pass — the pre-guard
    /// posture for that file). The nested package boundaries join through
    /// [`Self::capture_nested_pubspecs`], after the pass that makes their
    /// files resident.
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
            self.capture_pubspec_at(&dir);
        }
    }

    /// Captures the nested package boundaries' pubspecs for the pubspec
    /// lint face: a folder-local `pubspec.yaml` under a package root is a
    /// package boundary for everything below it (upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/workspace/pub.dart` `findPackageFor` :341-351
    /// — the workspace walks a file's ancestors for the nearest
    /// folder-local pubspec and roots a package there), and the oracle's
    /// driver lints every `pubspec.yaml` under the analysis roots — the
    /// CLI adds each walked file and the driver's `_kindOf` classifies the
    /// path `FileKind.pubspec`. dartq derives the candidate directories
    /// from the resident files' ancestry (the same universe the analysis
    /// covers) and mounts each boundary's pubspec through the same
    /// mtime-guarded reported-set shape as
    /// [`Self::capture_pubspecs`].
    ///
    /// Runs after the pipeline's apply (residency current), so a fresh
    /// engine's first pass sees the files discovery just made resident.
    /// The pass is gated on residency growth
    /// ([`state::EngineState::residency_epoch`]): only a newly resident
    /// file can surface an unscanned directory, so a turn without growth
    /// — the daemon's warm keystroke — skips the resident-set walk
    /// entirely (O(1)), and a growth turn pays one walk over the resident
    /// set to pick up its new directories. The epoch-tagged per-directory
    /// memo ([`Self::pubspec_dirs_scanned`]) keeps that walk's work at
    /// O(new dirs) — a directory is probed once per engine, never
    /// re-statted.
    fn capture_nested_pubspecs(&mut self) {
        // The epoch gate: no residency growth since the last covered pass
        // means no new directories can exist — the walk below would
        // re-hash every resident path for nothing.
        let epoch = self.state.residency_epoch;
        if epoch == self.nested_scan_epoch {
            return;
        }
        self.nested_scan_epoch = epoch;
        #[cfg(test)]
        {
            self.nested_scan_passes += 1;
        }
        let table = &self.state.packages;
        let mut mounted: FxHashSet<PathBuf> = FxHashSet::default();
        if let Some(root) = table.root().and_then(|name| table.get(name.as_str())) {
            mounted.insert(root.root.clone());
        }
        if let Some(members) = table.workspace_members() {
            mounted.extend(members.iter().filter_map(|name| {
                table
                    .get(name.as_str())
                    .map(|identity| identity.root.clone())
            }));
        }
        // The not-yet-scanned directories, one entry per resident file's
        // parent: the walk below stats each exactly once per engine. The
        // `queued` frontier dedupes at push time — a fresh directory
        // holding many new files queues (and walks its chain) once. It
        // exists only within the growth pass and is bounded by the new
        // directories, never by the resident set.
        let mut to_scan: Vec<PathBuf> = Vec::new();
        {
            let mut queued: FxHashSet<&Path> = FxHashSet::default();
            for file in self.state.residency.keys() {
                let Some(path) = self.state.files.path(*file) else {
                    continue;
                };
                let Some(dir) = path.parent() else {
                    continue;
                };
                if !self.pubspec_dirs_scanned.contains_key(dir) && queued.insert(dir) {
                    to_scan.push(dir.to_path_buf());
                }
            }
        }
        let mut nested: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
        for start in to_scan {
            // The containing package root cuts the walk: everything below
            // it is the boundary search's reach, everything above is the
            // workspace's (the root's own pubspec is already mounted). A
            // bare-floor workspace (`PackageTable::basic` with no self
            // package — the oracle's `BasicWorkspace` over an analyzed
            // directory) attributes no directory to a package, so the walk
            // bound is the analyzed directory itself: the oracle still
            // lints every `pubspec.yaml` under it (the CLI driver walks
            // the whole context root, `analyzer_cli/lib/src/driver.dart`
            // `_collectFiles` :427-448).
            let Some(bound) = self
                .state
                .packages
                .containing(&start)
                .map(|identity| identity.root.clone())
                .or_else(|| {
                    let root = self.state.packages.workspace_root();
                    start.starts_with(root).then(|| root.to_path_buf())
                })
            else {
                continue;
            };
            let mut current = start;
            loop {
                // Membership still decides (a directory is probed once per
                // engine); the recorded tag is the residency epoch this
                // walk covered the directory at.
                if self
                    .pubspec_dirs_scanned
                    .insert(current.clone(), epoch)
                    .is_some()
                {
                    break; // an earlier start scanned this chain's head
                }
                if !mounted.contains(&current) && current.join("pubspec.yaml").is_file() {
                    nested.insert(current.clone());
                }
                let Some(parent) = current.parent() else {
                    break;
                };
                if !parent.starts_with(&bound) {
                    break;
                }
                current = parent.to_path_buf();
            }
        }
        for dir in nested {
            self.capture_pubspec_at(&dir);
        }
    }

    /// The mtime-guarded mount of one directory's `pubspec.yaml` into the
    /// reported set — the body both captures share (the doc on
    /// [`Self::capture_pubspecs`] carries the contract). The sibling
    /// `android/AndroidManifest.xml` mounts through the same guard (the
    /// oracle analyzes it wherever it sits under an analysis root —
    /// `analyzer_cli/lib/src/driver.dart` :346-370); the lint face reads
    /// it back through [`Self::captured_manifests`].
    fn capture_pubspec_at(&mut self, dir: &Path) {
        let path = dir.join("pubspec.yaml");
        if let Ok(modified) = std::fs::metadata(&path).and_then(|meta| meta.modified())
            && self.pubspec_mtimes.get(&path) != Some(&modified)
            && let Ok(text) = std::fs::read_to_string(&path)
        {
            self.pubspec_mtimes.insert(path.clone(), modified);
            let file = self.state.files.intern(&path);
            let index = dartq_source::LineIndex::new(&text);
            self.state
                .reported
                .insert(file, (Arc::from(text.as_str()), index));
        }
        let manifest = dir.join("android").join("AndroidManifest.xml");
        if let Ok(modified) = std::fs::metadata(&manifest).and_then(|meta| meta.modified())
            && self.manifest_mtimes.get(&manifest) != Some(&modified)
            && let Ok(text) = std::fs::read_to_string(&manifest)
        {
            self.manifest_mtimes.insert(manifest.clone(), modified);
            let file = self.state.files.intern(&manifest);
            let index = dartq_source::LineIndex::new(&text);
            self.state
                .reported
                .insert(file, (Arc::from(text.as_str()), index));
        }
    }

    /// Every `pubspec.yaml` the captures mounted — the root package's,
    /// every workspace member's, and every nested package boundary's —
    /// in sorted order. The pubspec lint face's enumeration; the face
    /// reads each back through [`Engine::content`] (a pubspec deleted
    /// after its capture keeps serving its last read text, the same
    /// post-delete posture the resident file set has).
    #[must_use]
    pub fn captured_pubspecs(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self.pubspec_mtimes.keys().cloned().collect();
        paths.sort();
        paths
    }

    /// Every `AndroidManifest.xml` the captures mounted — the package
    /// roots', members', and nested boundaries' `android/` siblings — in
    /// sorted order. The manifest lint face's enumeration; emission stays
    /// gated on the governing options' `chrome-os-manifest-checks` (the
    /// face's judgment, not this one's).
    #[must_use]
    pub fn captured_manifests(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self.manifest_mtimes.keys().cloned().collect();
        paths.sort();
        paths
    }

    /// Builds the options-file self-diagnostics rows (#457): one row per
    /// issue recorded during each distinct loaded file's merge, attributed
    /// to the options file itself (the official reports every options
    /// diagnostic on the initial file — `include_walker.dart` reports
    /// include-chain rows at the initial include span, `parse_model` reports
    /// local rows at their nodes). Every loaded options file is interned on
    /// each call — even the issue-free ones — so the CLI's `undefined_lint`
    /// face can attribute against the same table; callers append the
    /// returned rows and re-sort canonically (the engine rows leave the
    /// pipeline sorted). Rows emit in options-file path order, one row set
    /// per distinct loaded path — locally canonical, never hash order.
    /// Empty in the common case.
    fn option_issue_rows(&mut self) -> Vec<Diagnostic> {
        // Intern every loaded options file once (the CLI's `undefined_lint`
        // face attributes against the same table), keeping the
        // issue-carrying ones keyed by their loaded path.
        let mut loaded: Vec<(FileId, &PathBuf, &analysis_options::AnalysisOptions)> = Vec::new();
        for (root, options) in self.state.options.iter() {
            if options.file_path.as_os_str().is_empty() {
                continue;
            }
            let file = self.state.files.intern(&options.file_path);
            if !options.issues.is_empty() {
                loaded.push((file, root, options));
            }
        }
        // One row set per distinct options-file path, emitted in path order
        // (#1851): entries sharing a load alias to one path, and equal paths
        // from distinct loads tie-break to the lowest governing root — a
        // unique map key, so the winner never depends on hash iteration
        // order.
        loaded
            .sort_unstable_by(|a, b| a.2.file_path.cmp(&b.2.file_path).then_with(|| a.1.cmp(b.1)));
        loaded.dedup_by(|a, b| a.2.file_path == b.2.file_path);
        loaded
            .into_iter()
            .flat_map(|(file, _, options)| {
                options
                    .issues
                    .iter()
                    .map(move |issue| issue.to_diagnostic(file))
            })
            .collect()
    }

    /// The pubspec/AndroidManifest validator face's rows: every captured
    /// pubspec runs the seven upstream validators
    /// ([`analysis_options::pubspec_issues`]) and every captured
    /// `android/AndroidManifest.xml` runs the manifest validator
    /// ([`analysis_options::manifest_issues`]) — the oracle's engagement
    /// phase, analysis time: the CLI driver validates each walked pubspec
    /// and manifest alongside the Dart files of the same batch (upstream
    /// Dart SDK source, `pkg/analyzer_cli/lib/src/driver.dart` :301-344,
    /// :346-381). Manifest rows gate on the governing options'
    /// `chrome-os-manifest-checks` (driver.dart :358 — the file's own
    /// options, resolved by the same deepest-ancestor rule the severity
    /// face uses), default off (`analysis_options.dart` :54). Each
    /// pubspec's rows pass the `IgnoreInfo.forYaml` suppression face the
    /// same engagement applies (upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/pubspec/pubspec_validator.dart` :80-83; the
    /// manifest branch carries none). Callers append the returned rows and
    /// re-sort canonically; the severity face re-levels them with the rest
    /// of the output (driver.dart :322, :374). Empty in the common case.
    fn validator_issue_rows(&mut self) -> Vec<Diagnostic> {
        let mut rows = Vec::new();
        for path in self.captured_pubspecs() {
            let Some(dir) = path.parent() else { continue };
            let file = self.state.files.intern(&path);
            let Some((text, index)) = self.state.reported.get(&file) else {
                continue;
            };
            let issues = analysis_options::pubspec_issues(text, dir);
            if issues.is_empty() {
                continue;
            }
            let ignores = YamlIgnores::parse(text);
            for issue in issues {
                let (line, _) = index.line_col_utf16(text, issue.offset);
                if ignores.ignored(line, issue.code.name) {
                    continue;
                }
                rows.push(issue.to_diagnostic(file));
            }
        }
        for path in self.captured_manifests() {
            let check_manifest = path
                .ancestors()
                .find_map(|dir| self.state.options.get(dir))
                .and_then(|options| options.chrome_os_manifest_checks)
                .unwrap_or(false);
            if !check_manifest {
                continue;
            }
            let file = self.state.files.intern(&path);
            let Some((text, _)) = self.state.reported.get(&file) else {
                continue;
            };
            for issue in analysis_options::manifest_issues(text, true) {
                rows.push(issue.to_diagnostic(file));
            }
        }
        rows
    }

    /// Whether any governing options entry declares an `analyzer.errors`
    /// severity override — the identity gate both output faces take before
    /// re-leveling rows.
    fn severity_overrides_present(&self) -> bool {
        self.state
            .options
            .values()
            .any(|options| options.has_severity_overrides())
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
        if !self.severity_overrides_present() {
            return diagnostics;
        }
        diagnostics
            .into_iter()
            .filter_map(|mut diagnostic| {
                let path = self.state.files.path(diagnostic.file)?;
                // The discovery rule as a lookup: a path prefix is unique
                // per depth, so the deepest ancestor holding loaded
                // options is exactly the longest `options`-keyed root
                // containing the file's path — one hash probe per ancestor
                // level instead of a scan over every root per row.
                let options = path.ancestors().find_map(|dir| self.state.options.get(dir));
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

    /// The settled per-file language version (#1053): the version the
    /// [`Engine::settled_features`] set was derived at — the `@dart`
    /// override when one applies, else the package-derived version. The
    /// lint fan-out mounts it onto the unit for the version-gated rule
    /// registrations. `None` under the same conditions as
    /// [`Engine::settled_features`] (not resident, or the face capped out
    /// of the cache): the caller falls back to its own package/default
    /// derivation.
    #[must_use]
    pub fn settled_version(&self, file: FileId) -> Option<LanguageVersion> {
        self.state.settled_face(file).map(|face| face.version)
    }

    /// The `query symbol` face: every declaration of `name`
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

    /// The fuzzy discovery face: the workspace's declarations whose names
    /// fuzzy-match `query`, best-first (the dartq-index `fuzzy` module's
    /// ranking spec — exact prefix, then word-boundary camelCase, then
    /// contiguous substring, then sparse subsequence; ties by name length,
    /// then name bytes), capped at that module's result cap. References
    /// are not joined: discovery names symbols, the exact faces resolve
    /// their uses.
    ///
    /// Read-after-write contract: [`Self::query_symbols`]'s, verbatim — a
    /// pure read of the latest analyze's state that interns nothing (the
    /// exact lookup's one side effect does not exist here; the query folds
    /// to a scratch buffer). Querying before the first analyze answers an
    /// empty set.
    #[must_use]
    pub fn query_symbols_fuzzy(&self, query: &str) -> SymbolAnswer<'_> {
        query::symbols_fuzzy(&self.state, query)
    }

    /// The `query callers` face: every declaration whose body invokes
    /// `name` — the caller→callee walk over the maintained index state,
    /// each invocation site projected to its enclosing declaration (the
    /// rust-analyzer call-hierarchy two-step: use rows first, enclosing
    /// declaration second). One row per caller symbol however many of its
    /// sites invoke the name; rows borrow the engine's interned spellings.
    ///
    /// [`query`]'s module docs and `query::callers` carry the freshness,
    /// answer-domain, and cost contracts; like [`Self::query_symbols`] it
    /// is a pure read of the latest analyze's state, its only side effect
    /// interning `name`.
    #[must_use]
    pub fn query_callers(&self, name: &str) -> Vec<DeclHit<'_>> {
        query::callers(&self.state, name)
    }

    /// The override-family face: every container that declares `name`
    /// connected to the container at `file` through the inheritance
    /// closure, computed as a fixpoint over both directions (each joined
    /// member contributes its own declaring supertypes and subtypes, so
    /// second branches join too) — the whole hierarchy a member rename
    /// edits, an equivalence class whatever member the anchor names. The
    /// member anchor is the `(file, container spelling, lookup name)`
    /// triple the cursor faces resolve (a declaration hit's container or a
    /// use hit's target container); `query::override_family` carries the
    /// freshness, answer-domain, and cost contracts. Like
    /// [`Self::query_symbols`] it is a pure read of the latest analyze's
    /// state, its only side effect interning the two spellings; the
    /// answer's container spellings borrow, never clone.
    #[must_use]
    pub fn override_family<'a>(
        &'a self,
        file: FileId,
        container: &str,
        name: &str,
    ) -> Vec<(FileId, &'a str)> {
        let container = self.state.strings.intern(container);
        let name = self.state.strings.intern(name);
        query::override_family(&self.state, (file, container), name)
    }

    /// The position-resolution face: what the maintained index rows hold
    /// under `offset` (UTF-16 code units, the engine's external
    /// convention) in `file` — the shared substrate the
    /// cursor-interactive faces (goto-def, hover, rename, completion
    /// context) resolve through. [`query`]'s module docs and
    /// `query::resolve_at` carry the freshness, answer-domain, and cost
    /// contracts; like [`Self::query_symbols`] it is a pure read of the
    /// latest analyze's state, with no side effects at all.
    #[must_use]
    pub fn resolve_at(&self, file: FileId, offset: u32) -> Option<PositionAnswer<'_>> {
        query::resolve_at(&self.state, file, offset)
    }

    /// The built-in completion face: the candidates the maintained index
    /// proves visible at `offset` (UTF-16 code units, the engine's
    /// external convention) in `file`, prefix-filtered by the identifier
    /// fragment immediately before the cursor. There is no plugin escape —
    /// the plugin protocol rejects a `completions` capability by
    /// construction — so this face is the whole completion story, and its
    /// candidate domain is the resolution graph at M1 fidelity: the file's
    /// own library-scope declarations, the distinct target names of its
    /// own resolved references, and the import prefixes it arms. Members,
    /// locals, and other names-table residents offer no candidate (the
    /// same boundary [`Engine::resolve_at`] documents); a member-access
    /// cursor answers the empty set.
    ///
    /// [`query`]'s module docs and `query::complete_at` carry the
    /// freshness, answer-domain, word-extraction, and cost contracts;
    /// like [`Self::resolve_at`] it is a pure read of the latest
    /// analyze's state — the word extraction reads the analyzed
    /// snapshot's own text — with no side effects at all.
    #[must_use]
    pub fn complete_at(&self, file: FileId, offset: u32) -> Option<CompletionAnswer<'_>> {
        query::complete_at(&self.state, file, offset)
    }

    /// discovery face: every package root (from the identity table),
    /// walked whole — each root-level `*.dart` file and every non-dot,
    /// non-excluded subdirectory, the oracle's included-folder walk
    /// (context_root.dart `_includedFilesInFolder`). A bare no-pubspec
    /// context carries no package identity, so its single root is the
    /// analyzed directory itself ([`state::context_roots`]) — the oracle's
    /// `BasicWorkspace` context root, walked whole. A nested package root
    /// is walked by its own pass, never by its parent's, so each file is
    /// discovered once under its own package. The only built-in exclusion is the
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
        let roots = state::context_roots(&self.state.packages);

        let mut stats = DiscoveryStats::default();
        // Every package root, so a parent's walk stops at a nested root:
        // the nested package's files belong to the nested walk (each file
        // is discovered exactly once, attributed to its own package).
        let all_roots: FxHashSet<PathBuf> = roots.iter().cloned().collect();
        // Phase 1 — every root's walk fans out at once (each is pure,
        // independent directory I/O). The per-root walks used to run one
        // after another, each closing on its own sort and read barrier: on
        // a many-package workspace (the 10⁵-file corpus carries 402 roots)
        // that serialized hundreds of short phases whose read batches held
        // only a handful of parallel slices. The roots' accepted lists come
        // back in the sorted root order, each list sorted — exactly the
        // order the serial per-root loop enqueued in.
        let options = &self.state.options;
        let accepted: Vec<Vec<PathBuf>> = roots
            .par_iter()
            .map(|root| discover_root(options, root, &all_roots))
            .collect();
        let accepted: Vec<PathBuf> = accepted.into_iter().flatten().collect();
        // Phase 2 — one global load over the whole ordered list: small
        // files owned, large ones mapped (`open_source`), LOC counted, and
        // the path normalization (one `realpath` per distinct parent per
        // 32-path slice, `normalize_paths`) riding the same jobs, so the
        // serial id pass below mints ids syscall-free. The loads run in
        // chunks of `DISCOVERY_LOAD_CHUNK`, each chunk enqueued before the
        // next loads: the bound caps the in-flight result vector, while the
        // texts themselves are retained by the pending overlay until apply
        // whatever the chunking (the mapping store then shares the leases
        // into the CAS table by refcount at apply — the text never copies
        // off the page cache; an owned store copies once at apply's
        // `put_text`). Per file the intern/enqueue path and the stats are
        // identical to the per-root form.
        for chunk in accepted.chunks(DISCOVERY_LOAD_CHUNK) {
            let reads: Vec<Result<(SourceText, u64, PathBuf), OpenError>> = chunk
                .par_chunks(32)
                .flat_map_iter(|slice| {
                    let refs: Vec<&Path> = slice.iter().map(PathBuf::as_path).collect();
                    let normalized = dartq_source::normalize_paths(&refs);
                    slice.iter().zip(normalized).map(|(path, normalized)| {
                        dartq_source::open_source(path).map(|text| {
                            let loc = u64::from(
                                u32::try_from(memchr::memchr_iter(b'\n', text.as_bytes()).count())
                                    .unwrap_or(u32::MAX),
                            );
                            (text, loc, normalized)
                        })
                    })
                })
                .collect();
            // Enqueue in the sorted order the loads were issued in. The
            // pending overlay is last-wins per path, so content would land
            // identically in any order, but a fixed enqueue order keeps the
            // differential harness byte-stable. A load failure surfaces at
            // its sorted position with everything before it already
            // enqueued — the surfaced error is the first in sorted order.
            // The span-capacity rejections are not failures: the file is
            // recorded for the notice face and skipped, and the walk
            // continues (an over-capacity file must cost a notice, not the
            // run).
            for (path, read) in chunk.iter().zip(reads) {
                match read {
                    Ok((text, loc, normalized)) => {
                        stats.loc_total += loc;
                        stats.file_count += 1;
                        self.set_source_normalized(path, normalized.clone(), text);
                        // The refresh guard's stamp (#2596): recorded
                        // here so the first refresh after discovery is a
                        // pure stat pass.
                        self.source_stamps.insert(normalized, stamp(path));
                    }
                    Err(OpenError::TooLarge { .. }) => stats.oversized.push(path.clone()),
                    Err(OpenError::Io(error)) => return Err(error),
                }
            }
        }
        Ok(stats)
    }

    /// The stat-guarded workspace refresh (#2596): re-walks the discovery
    /// roots and picks up what moved on disk since the last load — a
    /// changed file re-reads, a new file loads, a vanished file removes.
    /// Sources whose (mtime, size) stamp is unchanged are not touched, so
    /// a quiet tree pays one parallel directory walk plus one stat per
    /// file and zero reads (the pubspec guard's posture, widened to the
    /// source set; the lazy-boot resident lost its accidental freshness
    /// surface when the eager per-connection sweep retired, and this is
    /// its deliberate replacement — called from the daemon's drain seam
    /// ahead of every engine-work turn, so an out-of-band edit is visible
    /// to the next analyze).
    ///
    /// The reload stages through the pending overlay exactly like
    /// discovery, so an overlay change staged by the same turn for the
    /// same path still wins (last write in the pending map), and an
    /// unchanged re-read is content-deduped at apply.
    ///
    /// The universe roots themselves stay as assembled: a pub get that
    /// adds a package changes the resolution table, not this walk — the
    /// daemon refreshes its advisory face on config-input moves, and
    /// mid-session re-assembly remains out of scope.
    ///
    /// New files admit under the session's live-path ceiling
    /// (`live_path_cap`, the sync-batch gate's governance held over the
    /// walk): once loading the next unseen path would push the interned
    /// count past the cap, that path — and every further new one — is
    /// skipped this pass and counted in [`RefreshStats::skipped`], retried
    /// by the next pass (removals this pass free room for it). Already
    /// loaded paths reload freely: a re-read replaces content, it never
    /// grows residency.
    ///
    /// # Errors
    ///
    /// [`std::io::Error`] propagated from the root walk's listing (the
    /// same surface [`Engine::discover_workspace`] fails on); per-file
    /// read and stat failures skip the file (the next pass retries — a
    /// transient race is never a failed refresh).
    pub fn refresh_workspace(&mut self, live_path_cap: usize) -> std::io::Result<RefreshStats> {
        let roots = state::context_roots(&self.state.packages);
        let all_roots: FxHashSet<PathBuf> = roots.iter().cloned().collect();
        let options = &self.state.options;
        let accepted: Vec<Vec<PathBuf>> = roots
            .par_iter()
            .map(|root| discover_root(options, root, &all_roots))
            .collect();
        let accepted: Vec<PathBuf> = accepted.into_iter().flatten().collect();

        let mut stats = RefreshStats::default();
        let mut live = self.file_table().len();
        let mut walked: FxHashSet<PathBuf> = FxHashSet::default();
        for chunk in accepted.chunks(DISCOVERY_LOAD_CHUNK) {
            let refs: Vec<&Path> = chunk.iter().map(PathBuf::as_path).collect();
            let normalized = dartq_source::normalize_paths(&refs);
            for (path, normalized) in chunk.iter().zip(normalized) {
                walked.insert(normalized.clone());
                let stamp = stamp(path);
                let known = self.source_stamps.contains_key(&normalized);
                if known && self.source_stamps.get(&normalized) == Some(&stamp) {
                    continue; // unchanged on disk since the last load
                }
                if !known && live >= live_path_cap {
                    // A new path past the session's live-path ceiling:
                    // skipped this pass, retried by the next (the
                    // sync-batch gate's residency governance).
                    stats.skipped += 1;
                    continue;
                }
                match dartq_source::open_source(path) {
                    Ok(text) => {
                        self.set_source_normalized(path, normalized.clone(), text);
                        stats.reloaded += 1;
                        if !known {
                            live += 1;
                        }
                    }
                    // Too-large or unreadable now: drop the stamp so the
                    // next pass retries the load.
                    Err(_) => {
                        self.source_stamps.remove(&normalized);
                        continue;
                    }
                }
                self.source_stamps.insert(normalized, stamp);
            }
        }
        // Removals: every previously stamped path the walk no longer
        // reaches leaves the workspace — stage the removal and forget the
        // stamp. A path the engine never interned cannot remove; stamped
        // implies loaded implies interned, so the miss is defensive only.
        let vanished: Vec<PathBuf> = self
            .source_stamps
            .keys()
            .filter(|path| !walked.contains(*path))
            .cloned()
            .collect();
        for path in vanished {
            self.source_stamps.remove(&path);
            if self.remove_file(&path).is_ok() {
                stats.removed += 1;
            }
        }
        Ok(stats)
    }
}

/// One refresh pass's outcome (#2596): how many sources re-read from
/// disk, how many removed, and how many new paths the live-path ceiling
/// refused this pass, everything else having matched its stamp.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RefreshStats {
    /// Sources whose stamp moved (or that were new) and re-read.
    pub reloaded: usize,
    /// Previously loaded sources the walk no longer reaches.
    pub removed: usize,
    /// New paths skipped past the live-path ceiling (retried next pass).
    pub skipped: usize,
}

/// A file's disk stamp: the modification time and size, `None` on a
/// filesystem that reports no mtime (the caller re-reads every pass).
fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// One package root's accepted `.dart` files, sorted: the root's own files
/// plus the parallel walk of every non-dot, non-excluded child directory
/// that is not itself a package root. Reads only the options map, so the
/// roots fan out without sharing the engine across workers.
fn discover_root(
    options: &FxHashMap<PathBuf, Arc<analysis_options::AnalysisOptions>>,
    root: &Path,
    all_roots: &FxHashSet<PathBuf>,
) -> Vec<PathBuf> {
    {
        {
            // The governing options: the root's entry plus every
            // nested options directory under it. Excluded paths leave the
            // analysis set — directories are not descended into, files are
            // not interned — so neither diagnostics nor lints ever see
            // them. The predicate unions each governing entry's globs,
            // anchored to their declaring directories (the official
            // `root.excludedGlobs` accumulation, context_locator.dart
            // :402-409), so a nested file's excludes prune only its subtree.
            let governing: Vec<(PathBuf, Arc<analysis_options::AnalysisOptions>)> = options
                .iter()
                .filter(|(key, _)| key.starts_with(root))
                .map(|(key, options)| (key.clone(), Arc::clone(options)))
                .collect();
            let is_excluded = |path: &Path| {
                governing
                    .iter()
                    .any(|(key, options)| path.starts_with(key) && options.is_excluded(path))
            };
            // The whole root, like the oracle's included-folder walk
            // (context_root.dart `_includedFilesInFolder`: every child folder
            // not excluded by the dot rule or the options globs) — not only
            // the conventional `lib`/`bin`/`test`/`tool` surfaces: a `tests/`
            // or `example/` directory is analyzed too. Directly-in-root dart
            // files are collected below, never queued (the walk's `read_dir`
            // on a file is ENOTDIR and would silently drop them).
            let mut queue: Vec<PathBuf> = std::fs::read_dir(root)
                .map(|entries| {
                    entries
                        .flatten()
                        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
                        .filter(|entry| !entry.file_name().as_encoded_bytes().starts_with(b"."))
                        .map(|entry| entry.path())
                        .filter(|path| !all_roots.contains(path) && !is_excluded(path))
                        .collect()
                })
                .unwrap_or_default();
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
            // The walk fans out per directory over rayon (each subtree's
            // `read_dir` is independent I/O); the per-entry predicate is the
            // serial walk's verbatim, and the sort below pins the order.
            fn walk(
                dir: &Path,
                is_excluded: &(dyn Fn(&Path) -> bool + Sync),
                all_roots: &FxHashSet<PathBuf>,
            ) -> Vec<PathBuf> {
                let Ok(entries) = std::fs::read_dir(dir) else {
                    return Vec::new();
                };
                let mut files = Vec::new();
                let mut subdirs = Vec::new();
                for entry in entries.flatten() {
                    let Ok(file_type) = entry.file_type() else {
                        continue;
                    };
                    let path = entry.path();
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if file_type.is_dir() {
                        if name.starts_with('.') || is_excluded(&path) || all_roots.contains(&path)
                        {
                            continue;
                        }
                        subdirs.push(path);
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
                        && (file_type.is_file()
                            || (file_type.is_symlink()
                                && std::fs::metadata(&path)
                                    .is_ok_and(|meta| meta.is_file())))
                        && !is_excluded(&path)
                    {
                        files.push(path);
                    }
                }
                files.extend(
                    subdirs
                        .par_iter()
                        .flat_map_iter(|sub| walk(sub, is_excluded, all_roots))
                        .collect::<Vec<_>>(),
                );
                files
            }
            accepted.extend(
                std::mem::take(&mut queue)
                    .par_iter()
                    .flat_map_iter(|dir| walk(dir, &is_excluded, all_roots))
                    .collect::<Vec<_>>(),
            );
            // `read_dir` yields entries in platform-dependent order, so the
            // serial walk's enqueue order was never stable run-to-run;
            // sorting pins discovery to one deterministic path order.
            accepted.par_sort_unstable();
            accepted
        }
    }
}

impl Engine {
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
        // A dependency unit's face: a context message anchors there (an
        // overridden member declared in a pub-cache file) and the output
        // writers resolve its line the same way.
        if let Some(unit) = self.state.deps.units.get(&file) {
            return Some(&unit.facts.line_index);
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
        if let Some(unit) = self.state.deps.units.get(&file) {
            return self.state.store.get(unit.facts.content);
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
    /// — or `None` before the file's first analyze
    /// covers it. The raw-row read face the index-backed queries assemble
    /// over: [`Engine::query_symbols`]'s substrate at row fidelity, the
    /// rows' `StringId`s native to [`Engine::strings`].
    #[must_use]
    pub fn file_capture(&self, file: FileId) -> Option<&FileCapture> {
        self.state.capture(file).map(Arc::as_ref)
    }

    /// How many resident files' raw reference sites are mounted in the
    /// site cache — the occupancy hook of the `#1859` LRU (capped
    /// oldest-first; shed files' captures keep their declaration surface
    /// and re-derive sites on the next library build). The
    /// memory-accounting face reads it beside the per-structure byte
    /// getters.
    #[must_use]
    pub fn sites_occupancy(&self) -> usize {
        self.state.sites_occupancy()
    }

    /// Every live library's maintained reference surface, ascending by
    /// library id — rebuilt whole at each `LibraryAnalyzed` point and
    /// dropped with the library at retirement, the fresh partition
    /// [`Engine::query_symbols`] reads.
    pub fn library_surfaces(&self) -> impl Iterator<Item = (LibraryId, &LibraryIndex)> + '_ {
        self.state.surfaces()
    }

    /// The per-declaration reachability map (#1199) — the engine-level
    /// pass the `unreachable_from_main` lint shell mounts on
    /// ([`reachability`] for the entry, edge, and attribution contract).
    /// Lazy and memoized (#1383): the analyze only arms the rebuild, the
    /// first read after it pays the build (salvaged per unit — only the
    /// units whose inputs moved re-derive), and every further read clones
    /// the standing map, so the read-after-write posture stays
    /// [`Engine::query_symbols`]'s: the map reflects the latest completed
    /// analyze. A read that lands after an apply no analyze has absorbed
    /// serves the standing map instead of building over in-flight
    /// residency. `None` before the first analyze. Never panics: a poisoned
    /// memo mutex recovers its guard (the crate's
    /// `unwrap_or_else(PoisonError::into_inner)` policy), and a build over
    /// state whose pairing contracts broke degrades to the salvaged
    /// partial map instead of crashing the read.
    #[must_use]
    pub fn reachability(&self) -> Option<Arc<Reachability>> {
        let mut memo = self
            .state
            .reachability
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !memo.armed || memo.applied {
            // Never armed (no analyze completed), or an apply is in flight:
            // serve whatever the last completed analyze left behind — for a
            // cold engine that is `None`, for a warm one the standing map.
            return memo.map.clone();
        }
        if memo.stale || memo.map.is_none() {
            let prev = memo.map.take();
            // The lazy read's token never fires — the read's contract is
            // to answer, not to yield; the warm lane's token is the live
            // one the caller's cancel source flips.
            let token = CancellationToken::new();
            match crate::reachability::build(&self.state, prev, &token) {
                Ok(map) => {
                    memo.map = Some(map);
                    memo.stale = false;
                }
                // Unreachable under a fresh token; the memo keeps its
                // stale flag and standing map — the read serves what it
                // had rather than failing.
                Err(Cancelled) => {}
            }
        }
        memo.map.clone()
    }

    /// Whether the `Tier::Background` warm lane is armed — the O(1)
    /// readiness probe the scheduler-side caller checks before queueing a
    /// warm task. An analyze's settle arms the lane (see
    /// [`Engine::warm_step`]); the flag is optimistic in that a settle
    /// with nothing to warm still arms, and the first step disarms it.
    #[must_use]
    pub fn warm_pending(&self) -> bool {
        self.state.warm_pending()
    }

    /// One bounded warm chunk — the `Tier::Background` producer
    /// (rust-analyzer's `parallel_prime_caches` posture, [`warm`] module
    /// docs for the targets and mechanics). Runs the plan's next slice:
    /// the reachability build if the memo needs one, else up to
    /// `WARM_STEP_FILES` scanned-face fills. `cancel` is the same
    /// cooperative token the analyze checkpoints ride — a flip stops the
    /// step mid-plan with the remainder still armed.
    ///
    /// Output-neutral by construction: every item the lane fills is a
    /// pure function of the settled state — the same `Arc` a lazy read or
    /// a cache-miss fallback would produce — so warming can only move
    /// work off the critical path, never change an answer.
    pub fn warm_step(&mut self, cancel: &CancellationToken) -> WarmStep {
        self.state.warm_step(cancel)
    }

    /// The cumulative warm counters: items produced (a cached scanned
    /// face, the reachability map) and items visited without work (left
    /// residency, already warm, budget-paused) — the observability face
    /// beside [`Engine::sites_occupancy`] and the memory-report counters.
    #[must_use]
    pub fn warm_stats(&self) -> WarmStats {
        self.state.warm_stats()
    }

    /// The memory posture over the armed budget ladder: the limit, how far
    /// it descended, the deepest tier's machine key, and the ladder's last
    /// phase-boundary RSS sample — the data `dartq/memory` (daemon) and
    /// `--memory-report` (CLI) surface. Read-only: nothing here samples;
    /// the readings are the boundary polls'. An unbudgeted engine answers
    /// the default — the sampler never runs, so the face reports nothing
    /// armed, nothing sampled, nothing degraded.
    #[must_use]
    pub fn memory_status(&self) -> MemoryStatus {
        self.state.memory_status()
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

    /// Bytes the store serves out of read-only file mappings — the
    /// evictable page-cache share of [`stored_bytes`](Self::stored_bytes).
    /// Heap-only stores answer `0`; the serial-phase PSS probe reads this
    /// to split resident-anonymous bytes from file-backed ones.
    #[must_use]
    pub fn stored_mapped_bytes(&self) -> u64 {
        self.state.store.mapped_bytes()
    }

    /// Exact byte footprint of the per-target adjacency indexes (reverse
    /// import edges, part-directive offers, import/export sources) — the S6
    /// graph-maps segment of the RSS decomposition probe. Exact under the
    /// row-level CSR layout: slot arena at reserved capacity plus each live
    /// row's boxed payload.
    #[must_use]
    pub fn graph_maps_bytes(&self) -> u64 {
        self.state.graph_maps_bytes()
    }

    /// The memory decomposition (#2210): `(family, bytes, kind)` rows over
    /// every resident family the engine holds, `kind` one of `exact`
    /// (owned columns at capacity), `est` (per-entry constant × count) or
    /// `count` (an entry count, not bytes). O(residency), read-only; the
    /// analyze face prints it only under the `DARTQ_RSS_DECOMPOSITION`
    /// probe, so a disarmed run never calls it.
    #[doc(hidden)]
    #[must_use]
    pub fn memory_report(&self) -> Vec<(&'static str, u64, &'static str)> {
        self.state.memory_report()
    }

    /// Exact byte footprint of the two postings directories (name →
    /// declaring files, name → libraries with resolved uses) plus the
    /// dependency tier's declaration directory — the S2 extension the RSS
    /// decomposition probe pins: hash slots at capacity plus each live
    /// row's reserved bytes ([`dartq_index::Postings::bytes`]).
    #[doc(hidden)]
    #[must_use]
    pub fn postings_bytes(&self) -> u64 {
        (self.state.decl_postings.bytes()
            + self.state.ref_postings.bytes()
            + self.state.deps.decl_postings.bytes()) as u64
    }

    /// The library owning `file` (defining or part), if resident.
    #[must_use]
    pub fn library_of(&self, file: FileId) -> Option<LibraryId> {
        self.state.library_of_file.get(&file).copied()
    }

    /// The defining file of a live library. Singleton convention
    ///: a singleton library's sole member is its
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

    /// The generation header facts snapshots key on: the
    /// container format version, the engine's `FACTS_SEMANTIC_VERSION`,
    /// the SDK identity, the package-graph fingerprint, and the context
    /// salt (the remaining fork dimensions — the context root, target,
    /// declared variables, analysis options). The same header drives
    /// `SnapshotStore::load` at warm start and `save` at flush; callers
    /// never derive it by hand.
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
                &self.context_root,
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
    /// The flush is O(changed), not O(residency) (#1386): keys the store
    /// already holds under the current tables stamp
    /// ([`SnapshotStore::written_keys`]) are claimed in the manifest but
    /// never projected — within a generation the payload is a pure
    /// function of the key, so their frames on disk are already exact and
    /// the projection, the encode, and the incumbent read are all skipped.
    /// Only facts that moved since the store last recorded their key (and
    /// keys the store has no record of, e.g. a cold store's first flush)
    /// pay [`PersistedFacts::of`].
    ///
    /// # Errors
    ///
    /// Propagates [`SaveError`] — a failed flush is a future miss (the
    /// accelerator invariant), never a correctness loss.
    pub fn save_facts_snapshot(&self, store: &SnapshotStore) -> Result<usize, SaveError> {
        let header = self.snapshot_header();
        let tables = RemapTables::default();
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
        // The manifest walk: one slot per distinct key, in path order —
        // the first occurrence's facts are the deterministic payload pick.
        // Projection is deferred past the store query so only unrecorded
        // keys pay it.
        let mut unique: Vec<(EntryKey, &state::UnitFacts)> = Vec::with_capacity(files.len());
        let mut claimed: FxHashSet<EntryKey> = FxHashSet::default();
        for file in files {
            let facts = &self.state.residency[&file];
            let key = l1::facts_key(&self.state.grants(), file, facts.content());
            let key = EntryKey {
                content_hash: *key.content.as_bytes(),
                feature_fingerprint: key.feature_fingerprint,
                semantic_version: key.semantic_version,
            };
            if claimed.insert(key) {
                unique.push((key, facts));
            }
        }
        let keys: Vec<EntryKey> = unique.iter().map(|(key, _)| *key).collect();
        let written: FxHashSet<EntryKey> = store
            .written_keys(&header, &tables, &keys)
            .into_iter()
            .collect();
        let changed: Vec<Entry<PersistedFacts>> = unique
            .iter()
            .filter(|(key, _)| !written.contains(key))
            .map(|(key, facts)| Entry {
                key: *key,
                payload: PersistedFacts::of(facts, &self.state.strings, &self.state.files),
            })
            .collect();
        store.save_delta(&header, &tables, &keys, &changed)?;
        Ok(keys.len())
    }

    /// Warm start (the M3 load face): stages a loaded facts generation for
    /// the apply's lookups. Staging is keyed by the entry triple; the apply's
    /// lookup IS the content-hash verification — an entry serves only a file
    /// whose live content id equals the key's hash, and a served entry is
    /// claimed (removed) at that moment (#1387): entries no batch claims
    /// stay staged for a later one — a file reverting to staged content is
    /// exactly the undo hit the retention keeps — until the budget's tier-3
    /// shed or the next ingest replaces them. Ingestion never touches
    /// residency (#377). The loaded header and remap tables are the store's
    /// match gate — consumed by `load`, not needed here.
    ///
    /// The ingest *replaces* the staged generation instead of extending it:
    /// every superseded payload (a key the new snapshot does not re-stage)
    /// leaves at the refresh, so repeated ingests can never pile staging
    /// maps under a long-lived daemon — the pile's only other exit was the
    /// armed budget's 2 GiB tier-3, far above the staging face. Retention
    /// is per generation: this ingest's unclaimed entries stand across
    /// applies until claimed, shed, or replaced by the next ingest, which
    /// is the documented exit list. [`WARM_START_STAGING_CAP`] bounds a
    /// single pathological snapshot the same way; identities past the cap
    /// skip staging (an accelerator miss, never an output change).
    ///
    /// Returns the number of staged entries.
    pub fn ingest_facts_snapshot(&mut self, snapshot: Snapshot<PersistedFacts>) -> usize {
        let mut staging: FxHashMap<EntryKey, PersistedFacts> = FxHashMap::default();
        for entry in snapshot.entries.into_iter().take(WARM_START_STAGING_CAP) {
            staging.insert(entry.key, entry.payload);
        }
        let staged = staging.len();
        self.state.warm_start = staging;
        staged
    }
}

/// What one [`Engine::apply_pending`] changed.
#[derive(Debug, Default)]
pub struct AffectedReport {
    /// Files whose `ContentId` differed from residency (gate passed, work done).
    pub changed: Vec<FileId>,
    /// Files removed by this apply.
    pub removed: Vec<FileId>,
    /// Post-closure dirty library set (what analyze must recompute).
    pub invalidated: Vec<LibraryId>,
}

/// Discovery counters.
#[derive(Debug, Default)]
pub struct DiscoveryStats {
    /// Discovered `.dart` files.
    pub file_count: usize,
    /// Total lines across the discovered set.
    pub loc_total: u64,
    /// Discovered-but-rejected `.dart` files: each exceeds the engine's
    /// 32-bit span capacity ([`dartq_source::MAX_SOURCE_BYTES`]) and never
    /// enters the set (not interned, not analyzed — the exclusion
    /// posture). The faces surface these as stderr notices; the counts
    /// above never include them.
    pub oversized: Vec<PathBuf>,
}

/// Flat CSR arrays: `offsets[i]..offsets[i+1]` slice `edges` for node `i`.
/// Both views are plain `&[u32]` — the zero-copy export face for external
/// compute backends.
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
    /// The per-library resolved-type snapshot (#2204 phase A): one
    /// `dartq_types` table + hierarchy + name registry over the whole
    /// residency's element index, built once per analyze beside
    /// `index` and shared by every per-unit fold through the flow layer's
    /// `TypeOps` seam. `None` when the analyze resolved no element index
    /// (the incremental-diagnosis path keeps the name-level default).
    pub types: Option<&'a dartq_semantic::LibraryTypes>,
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
    /// The per-library `class-modifiers` posture (#1294): each live
    /// library's defining file's settled language version (the `@dart`
    /// override applied) resolved through the feature registry
    /// (`class-modifiers`, `enabledIn: 3.0.0`), built once per analyze
    /// beside the element index. Libraries absent from the map answer the
    /// modern posture ([`dartq_resolution::LibraryClassModifiers`]).
    pub class_modifiers: &'a dartq_resolution::LibraryClassModifiers,
    /// The default fold's member-level memo handle (#1384): the engine's
    /// fold map plus this library's identity inputs (graph generation,
    /// cycle fingerprint, strict modes, posture epoch). The shipped
    /// [`semdiag::Diagnoser`] consults it per member; a custom diagnoser
    /// that folds differently simply ignores it. Crate-internal — external
    /// diagnosers consume the view read-only.
    pub(crate) memo: crate::semdiag::MemoHandle<'a>,
    /// The residency-pairing degrade log (stability lane #1459): the fold
    /// records the members it skips on a residency/store pairing miss (a
    /// bounded, deduped row per (file, face) pair). Crate-internal, same
    /// discipline as [`LibraryView::memo`].
    pub(crate) degrades: &'a crate::state::ResidencyDegrades,
    /// The analyze's cancellation token: the fold's member loop polls it
    /// on a [`crate::cancel::Checkpoint`] cadence so a flip mid-library
    /// stops the fold instead of waiting out the whole recompute. The
    /// caller treats a fired token as the run's early exit, so a
    /// diagnoser that returns early on it never publishes a partial
    /// product. Crate-internal, same discipline as
    /// [`LibraryView::memo`].
    pub(crate) cancel: &'a CancellationToken,
}

/// The semantic diagnostics plug point (#120/#121).
///
/// The engine ships the default implementor ([`semdiag::Diagnoser`], folding
/// `dartq_semantic::diagnose_with`); a caller may substitute its own. Per-library
/// calls arrive on the batch fan-out workers, so implementations must be
/// `Send + Sync` and stateless across calls; diagnostics append into `out`
/// and carry no ordering obligations (the engine sorts canonically). Rows
/// are refcounted (#1501): appends cost a bump against rows the engine
/// already owns (the facts' syntax rows, the member-fold memo's products),
/// and freshly built rows wrap once in [`Arc::new`] — the engine's stores
/// share those bytes instead of copying them.
pub trait SemanticDiagnoser: Send + Sync {
    /// Appends this library's semantic diagnostics to `out`. The view
    /// arrives by reference — one call may be followed by
    /// [`Self::member_refs`] over the same view.
    fn diagnose_library(&self, library: &LibraryView<'_>, out: &mut Vec<Arc<Diagnostic>>);

    /// The library's semantically resolved member-access reference rows
    /// (#2618): one handle per member file, read after
    /// [`Self::diagnose_library`] ran for the library (the default
    /// implementor serves them from the same member-fold memo the
    /// diagnostics came from — no re-fold, one hash probe per member). The
    /// rows fold into the library's reference surface beside the
    /// parser-level sites; the default — no rows — is the
    /// custom-diagnoser degradation (a substituting fold that publishes
    /// nothing leaves the surface at its parser-level reach).
    fn member_refs(
        &self,
        _library: &LibraryView<'_>,
    ) -> Vec<(FileId, std::sync::Arc<[dartq_semantic::MemberRef]>)> {
        Vec::new()
    }
}

/// The M1 default store constructor.
#[must_use]
pub fn memory_store() -> Box<dyn SourceStore + Send + Sync> {
    Box::new(MemorySourceStore::new())
}

/// The file-backed CAS store constructor (`MmapSourceStore`): loaded files
/// ride read-only memory mappings — canonical text never copied off the
/// page cache, identical contents held once. The workspace-load default;
/// heap-only deployments (sandboxed test fixtures, overlay-only sessions)
/// keep [`memory_store`].
#[must_use]
pub fn mmap_store() -> Box<dyn SourceStore + Send + Sync> {
    Box::new(MmapSourceStore::new())
}

/// A store wired to `pool`: every distinct content the engine puts lands in
/// the pool's shared storage, so N contexts constructed over one pool hold
/// each content exactly once between them (the shared content store —
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

/// The pooled store's mmap-backed sibling (`PooledMappedSourceStore`): same
/// cross-context dedup contract as [`pooled_memory_store`] — every context
/// wired to one [`MappedContentPool`] holds each distinct content exactly
/// once, through one refcounted slot — but a [`SourceText`] mapping lease
/// is shared into the pool instead of copied, so canonical file text stays
/// on evictable page-cache pages rather than the anonymous heap. The
/// daemon's opt-in shared-store posture; [`pooled_memory_store`] remains
/// the heap-only default.
#[must_use]
pub fn pooled_mmap_store(pool: &MappedContentPool) -> Box<dyn SourceStore + Send + Sync> {
    Box::new(PooledMappedSourceStore::new(pool))
}

/// The YAML-face ignore-comment suppression table (`IgnoreInfo.forYaml`,
/// upstream Dart SDK source,
/// `pkg/analyzer/lib/src/ignore_comments/ignore_info.dart` :145-166):
/// `# ignore: name, name` entries suppress by 1-based line — an entry
/// alone on its line suppresses the NEXT line (:157, `beforeComment`
/// empty) — plus file-wide `# ignore_for_file:` entries. The pubspec
/// validator face and the pubspec lint face share it.
pub struct YamlIgnores {
    on_line: FxHashMap<u32, Vec<String>>,
    for_file: Vec<String>,
}

impl YamlIgnores {
    /// Parses the ignore entries of one YAML file's text.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut info = Self {
            on_line: FxHashMap::default(),
            for_file: Vec::new(),
        };
        for (index, line) in text.lines().enumerate() {
            let line_no = saturate_slot_u32(index) + 1;
            if let Some((before, names)) = yaml_ignore_entry(line, "ignore:") {
                // Alone on the line -> the next line (ignore_info.dart:157).
                let target = if before.trim().is_empty() {
                    line_no + 1
                } else {
                    line_no
                };
                info.on_line.entry(target).or_default().extend(names);
            }
            if let Some((_, names)) = yaml_ignore_entry(line, "ignore_for_file:") {
                info.for_file.extend(names);
            }
        }
        info
    }

    /// Whether a diagnostic named `name` reported on 1-based `line` is
    /// suppressed (`_ignoredAt`, ignore_info.dart:186-212).
    #[must_use]
    pub fn ignored(&self, line: u32, name: &str) -> bool {
        self.for_file
            .iter()
            .any(|entry| entry.eq_ignore_ascii_case(name))
            || self
                .on_line
                .get(&line)
                .is_some_and(|entries| entries.iter().any(|entry| entry.eq_ignore_ascii_case(name)))
    }
}

/// The `#`-commented marker entry of one YAML line: the text before the
/// `#` run, and the name list after the marker. The official regexes'
/// greedy `.*` reads the LAST `#` run the rest still matches from; the
/// scan keeps the last candidate the same way.
fn yaml_ignore_entry<'a>(line: &'a str, marker: &str) -> Option<(&'a str, Vec<String>)> {
    let bytes = line.as_bytes();
    let mut found = None;
    for (i, &b) in bytes.iter().enumerate() {
        if b != b'#' {
            continue;
        }
        let mut j = i;
        while j < bytes.len() && bytes[j] == b'#' {
            j += 1;
        }
        let mut k = j;
        while k < bytes.len() && bytes[k] == b' ' {
            k += 1;
        }
        if line[k..].starts_with(marker) {
            found = Some((&line[..i], &line[k + marker.len()..]));
        }
    }
    found.map(|(before, rest)| {
        (
            before,
            rest.split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .collect(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    use dartq_language::Feature;
    use dartq_pub::{LockFile, Pubspec, build_graph};

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
            experiments: Vec::new(),
        })
        .expect("engine");
        (root, engine)
    }

    /// #2596: the stat-guarded refresh picks up out-of-band disk deltas —
    /// a changed file re-reads, a quiet pass reloads nothing, a new file
    /// loads, a vanished file removes — against the discovery-seeded
    /// stamps.
    #[test]
    fn refresh_workspace_picks_up_disk_deltas() {
        let (root, mut engine) = facade_engine();
        std::fs::create_dir_all(root.join("lib")).expect("lib dir");
        std::fs::write(root.join("lib/main.dart"), "int main() => 1;\n").expect("seed");
        engine.discover_workspace().expect("discover");
        let report = engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");
        assert!(report.is_empty(), "the seeded tree is clean");

        // A quiet pass: nothing moved, nothing reloads.
        let quiet = engine.refresh_workspace(usize::MAX).expect("quiet refresh");
        assert_eq!(quiet, RefreshStats::default(), "stamps match, zero reads");

        // An out-of-band edit: exactly that file reloads and the next
        // analyze carries the error.
        std::fs::write(root.join("lib/main.dart"), "int main() => 1 oops;\n").expect("edit");
        let moved = engine.refresh_workspace(usize::MAX).expect("refresh");
        assert_eq!(moved.reloaded, 1, "only the edited file re-read");
        assert_eq!(moved.removed, 0);
        let report = engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");
        assert!(
            report.iter().any(|row| row.code.name == "expected_token"),
            "the edit surfaces: {report:?}"
        );

        // A new file loads; removing it reverts (and removing the edited
        // file's error path — the revert — reloads it too).
        std::fs::write(root.join("lib/extra.dart"), "int broken(\n").expect("add");
        let added = engine.refresh_workspace(usize::MAX).expect("refresh");
        assert_eq!(added.reloaded, 1, "the new file loads");
        std::fs::remove_file(root.join("lib/extra.dart")).expect("remove");
        std::fs::write(root.join("lib/main.dart"), "int main() => 1;\n").expect("revert");
        let reverted = engine.refresh_workspace(usize::MAX).expect("refresh");
        assert_eq!(reverted.removed, 1, "the vanished file removes");
        assert_eq!(reverted.reloaded, 1, "the reverted file re-reads");
        let report = engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");
        assert!(
            report.is_empty(),
            "the reverted tree is clean again: {report:?}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// #2596: the refresh walk admits new paths under the live-path
    /// ceiling — a new file past the cap skips (counted, no residency),
    /// and the next pass with room loads it; an already-loaded file
    /// reloads past the cap regardless (a re-read replaces, never grows).
    #[test]
    fn refresh_workspace_admits_new_paths_under_the_cap() {
        let (root, mut engine) = facade_engine();
        std::fs::create_dir_all(root.join("lib")).expect("lib dir");
        std::fs::write(root.join("lib/main.dart"), "int main() => 1;\n").expect("seed");
        engine.discover_workspace().expect("discover");
        let live = engine.file_table().len();
        assert!(live >= 1, "the seeded file is interned");

        // One slot of headroom: a second new file stays skipped while the
        // cap pins the table at `live`.
        std::fs::write(root.join("lib/extra.dart"), "int broken(\n").expect("add");
        let skipped = engine.refresh_workspace(live).expect("refresh");
        assert_eq!(skipped.skipped, 1, "the new path past the cap skips");
        assert_eq!(skipped.reloaded, 0);
        assert_eq!(engine.file_table().len(), live, "residency never grew");

        // Room appears (the cap lifts): the same pass shape loads it.
        let loaded = engine.refresh_workspace(live + 1).expect("refresh");
        assert_eq!(loaded.skipped, 0);
        assert_eq!(loaded.reloaded, 1, "the skipped path loads once admitted");
        let report = engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");
        assert!(
            report.iter().any(|row| row.code.name == "expected_token"),
            "the admitted file surfaces its error: {report:?}"
        );

        // A reload of an already-loaded file ignores the cap: content
        // replacement never grows residency.
        std::fs::write(root.join("lib/main.dart"), "int main() => 1 oops;\n").expect("edit");
        let moved = engine.refresh_workspace(live).expect("refresh");
        assert_eq!(moved.reloaded, 1, "a loaded path reloads past the cap");
        assert_eq!(moved.skipped, 0);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A throwaway pubspec-less tree + the synthetic SDK — the bare floor
    /// ([`PackageTable::basic`] over `None`), the shape `dart analyze
    /// <dir>` meets outside any package. The tree probes the walk's
    /// exclusions: `.dot.dart` (the dot rule covers root files) and
    /// `.hidden/c.dart` (a nested hidden directory) must stay undiscovered;
    /// `a.dart` and `sub/b.dart` are the accepted set.
    fn bare_floor_engine() -> (PathBuf, Engine) {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "dartq-engine-bare-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let write = |relative: &str, content: &str| {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(path, content).expect("file");
        };
        write("a.dart", "var a = 1;\n");
        write("sub/b.dart", "var b = 2;\n");
        write(".dot.dart", "var dot = 3;\n");
        write(".hidden/c.dart", "var hidden = 4;\n");
        write("sdk/version", "3.13.1\n");
        write(
            "sdk/lib/libraries.json",
            r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#,
        );
        write(
            "sdk/lib/_internal/allowed_experiments.json",
            r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
        );
        // Canonicalize before discovery resolves it (the #287 spelling
        // universe — macOS /var vs /private/var).
        let root = root.canonicalize().expect("fixture root resolves");
        let engine = Engine::new(EngineConfig {
            store: memory_store(),
            sdk: SdkSnapshot::load(&root.join("sdk"), None).expect("synthetic SDK loads"),
            packages: PackageTable::basic(&root, None).expect("bare table"),
            pub_config: Arc::new(PackageConfig {
                packages: Vec::new(),
                generator: None,
                generator_version: None,
                additional_properties: std::collections::BTreeMap::new(),
            }),
            config_dir: root.join(".dart_tool"),
            semdiag: None,
            target: DEFAULT_TARGET.to_owned(),
            declared_variables: FxHashMap::default(),
            experiments: Vec::new(),
        })
        .expect("engine");
        (root, engine)
    }

    /// The bare floor's discovery face: with no package identity to
    /// enumerate, the context root is the analyzed directory itself and the
    /// walk accepts its `.dart` files — the oracle's `BasicWorkspace`
    /// context-root walk, the face `dart analyze <bare-dir>` runs.
    #[test]
    fn bare_floor_discovery_walks_the_target_tree() {
        let (root, mut engine) = bare_floor_engine();
        let stats = engine.discover_workspace().expect("walk");
        assert_eq!(stats.file_count, 2, "a.dart and sub/b.dart accepted");
        for name in ["a.dart", "sub/b.dart"] {
            assert!(
                engine.file_table().try_file_id(&root.join(name)).is_some(),
                "{name} discovered"
            );
        }
    }

    /// The bare floor's walk keeps the oracle's built-in exclusions: the
    /// dot rule covers files and directories alike, at the root and nested.
    #[test]
    fn bare_floor_discovery_excludes_dot_names() {
        let (root, mut engine) = bare_floor_engine();
        engine.discover_workspace().expect("walk");
        for name in [".dot.dart", ".hidden/c.dart"] {
            assert!(
                engine.file_table().try_file_id(&root.join(name)).is_none(),
                "{name} excluded"
            );
        }
    }

    /// The ingest's stale policy: a fresh generation REPLACES the staged
    /// one (superseded payloads leave at the refresh instead of piling
    /// under a long-lived daemon — the audit's F-B), and a snapshot larger
    /// than [`WARM_START_STAGING_CAP`] stages its head deterministically.
    #[test]
    fn ingest_replaces_the_staged_generation_and_caps_the_snapshot() {
        let (root, mut engine) = facade_engine();
        let file = engine.set_content(&root.join("lib/a.dart"), b"int a() => 1;\n");
        engine.apply_pending(&mut |_| {});
        let payload = PersistedFacts::of(
            &engine.state.residency[&file],
            &engine.state.strings,
            &engine.state.files,
        );
        let key = |hash: u8| EntryKey {
            content_hash: [hash; 16],
            feature_fingerprint: 0,
            semantic_version: 0,
        };
        // Generation 1, staged directly (the ingest's product face).
        engine.state.warm_start.insert(key(1), payload.clone());
        engine.state.warm_start.insert(key(2), payload.clone());

        // Generation 2 carries one fresh key: the superseded generation-1
        // keys leave at the refresh.
        let snapshot = Snapshot {
            header: GenerationHeader {
                format_version: SNAPSHOT_FORMAT_VERSION,
                semantic_version: 0,
                sdk_identity: String::new(),
                package_graph_fingerprint: [0; 16],
                context_salt: [0; 16],
            },
            tables: RemapTables::default(),
            entries: vec![Entry {
                key: key(3),
                payload: payload.clone(),
            }],
        };
        assert_eq!(
            engine.ingest_facts_snapshot(snapshot),
            1,
            "the fresh generation staged its one entry"
        );
        assert!(engine.state.warm_start.contains_key(&key(3)));
        assert!(
            !engine.state.warm_start.contains_key(&key(1))
                && !engine.state.warm_start.contains_key(&key(2)),
            "the superseded generation left at the refresh"
        );

        // The cap: a snapshot one entry past it stages exactly the cap, and
        // the head entries win (deterministic pick; a missing tail entry is
        // an accelerator miss, never an output change).
        let entries: Vec<Entry<PersistedFacts>> = (0..=WARM_START_STAGING_CAP)
            .map(|i| {
                let mut hash = [0u8; 16];
                hash[..8].copy_from_slice(&(i as u64).to_le_bytes());
                Entry {
                    key: EntryKey {
                        content_hash: hash,
                        feature_fingerprint: 0,
                        semantic_version: 0,
                    },
                    payload: payload.clone(),
                }
            })
            .collect();
        let snapshot = Snapshot {
            header: GenerationHeader {
                format_version: SNAPSHOT_FORMAT_VERSION,
                semantic_version: 0,
                sdk_identity: String::new(),
                package_graph_fingerprint: [0; 16],
                context_salt: [0; 16],
            },
            tables: RemapTables::default(),
            entries,
        };
        assert_eq!(
            engine.ingest_facts_snapshot(snapshot),
            WARM_START_STAGING_CAP,
            "the cap staged the snapshot's head, one entry short of the input"
        );
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

    /// The memory decomposition (#2210) after a cold discover + analyze:
    /// every family row is present once, the residency/store faces count
    /// what the run admitted, and the counters move with the workspace.
    #[test]
    fn memory_report_counts_the_admitted_workspace() {
        let (root, mut engine) = facade_engine();
        std::fs::create_dir_all(root.join("lib")).expect("lib dir");
        std::fs::write(
            root.join("lib/a.dart"),
            "import 'b.dart';\nvoid a() { b(); }\n",
        )
        .expect("a");
        std::fs::write(root.join("lib/b.dart"), "void b() {}\n").expect("b");
        let stats = engine.discover_workspace().expect("discover");
        assert!(stats.file_count >= 2);
        engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");
        let report = engine.memory_report();
        let row = |family: &str| {
            let rows: Vec<_> = report
                .iter()
                .filter(|(name, _, _)| *name == family)
                .collect();
            assert_eq!(rows.len(), 1, "{family} reported once");
            rows[0].1
        };
        assert_eq!(row("residency_files"), engine.state.residency.len() as u64);
        assert!(row("residency_files") >= 2);
        assert_eq!(row("store_bytes"), engine.stored_bytes());
        assert!(row("residency_line_index") > 0);
        assert!(
            row("index_captures") > 0,
            "every resident file carries a capture"
        );
        assert!(
            row("index_library_surfaces") > 0,
            "the analyze built surfaces"
        );
        assert_eq!(
            row("index_postings"),
            engine.postings_bytes(),
            "the postings row is the public counter"
        );
        assert!(
            report
                .iter()
                .all(|(_, _, kind)| matches!(*kind, "exact" | "est" | "count"))
        );
    }

    /// The memory-status face over an armed budget: the limit reads back,
    /// a real run's boundary polls leave a sample (unix), and a
    /// no-pressure run stays at tier 0 with no tier key. The unbudgeted
    /// engine answers the default — nothing armed, nothing sampled,
    /// nothing degraded.
    #[test]
    fn memory_status_reports_the_armed_budget() {
        let (root, mut engine) = facade_engine();
        std::fs::create_dir_all(root.join("lib")).expect("lib dir");
        std::fs::write(root.join("lib/a.dart"), "void a() {}\n").expect("a");
        // 1 TiB: armed, but no realistic test RSS crosses it — the
        // posture stays at tier 0 deterministically.
        engine.set_memory_limit(Some(1024 * 1024));
        engine.discover_workspace().expect("discover");
        engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");
        let status = engine.memory_status();
        assert_eq!(status.limit_mib, Some(1024 * 1024));
        assert_eq!(
            status.degraded_tiers, 0,
            "no pressure under the 1 TiB limit"
        );
        assert_eq!(status.degraded_tier, None);
        #[cfg(unix)]
        assert!(
            status.rss_bytes.is_some(),
            "a polled boundary left a sample"
        );

        let (_, engine) = facade_engine();
        assert_eq!(
            engine.memory_status(),
            MemoryStatus::default(),
            "unbudgeted: the sampler never ran"
        );
    }

    /// Discovery over several package roots enqueues exactly the per-root
    /// sorted order concatenated in sorted root order — the order the
    /// serial per-root loop produced before the roots fanned out.
    #[test]
    fn discovery_enqueues_roots_in_sorted_order() {
        let (root, mut engine) = facade_engine();
        for relative in [
            "lib/z.dart",
            "lib/a.dart",
            "test/m_test.dart",
            "tool/t.dart",
        ] {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(&path, "void f() {}\n").expect("file");
        }
        engine.discover_workspace().expect("discover");
        let order: Vec<PathBuf> = engine
            .state
            .pending_order
            .iter()
            .filter_map(|file| engine.state.files.path(*file).map(Path::to_path_buf))
            .filter(|path| path.extension().is_some_and(|ext| ext == "dart"))
            .collect();
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(
            order, sorted,
            "one root: the enqueue order is the sorted walk"
        );
        assert_eq!(order.len(), 4);
    }

    /// The residency-epoch gate on the nested-pubspec scan (#1527): a
    /// turn without residency growth runs no scan pass at all — the warm
    /// daemon keystroke pays O(1) here — while a growth turn reopens the
    /// gate exactly once and still mounts the nested boundary (the memo's
    /// first-stat-per-directory contract holds across the gate). The
    /// `nested_scan_passes` instrumentation counts actual passes.
    #[test]
    fn warm_turns_skip_the_nested_pubspec_scan() {
        let (root, mut engine) = facade_engine();
        let nested_dir = root.join("packages/sub");
        std::fs::create_dir_all(nested_dir.join("lib")).expect("nested dirs");
        std::fs::write(
            nested_dir.join("pubspec.yaml"),
            "name: sub\nversion: 0.1.0\n",
        )
        .expect("nested pubspec");
        // Two files share the fresh directory: the first pass queues and
        // walks it once (the to_scan dedup face).
        engine.set_content(&nested_dir.join("lib/a.dart"), b"void a() {}\n");
        engine.set_content(&nested_dir.join("lib/b.dart"), b"void b() {}\n");
        engine.apply_pending(&mut |_| {});
        assert!(engine.state.residency_epoch > engine.nested_scan_epoch);
        engine.capture_nested_pubspecs();
        assert_eq!(engine.nested_scan_passes, 1);
        assert_eq!(engine.nested_scan_epoch, engine.state.residency_epoch);
        assert!(
            engine
                .captured_pubspecs()
                .contains(&nested_dir.join("pubspec.yaml")),
            "the growth pass mounts the nested boundary"
        );
        let memo = engine.pubspec_dirs_scanned.clone();
        // Warm turn: no residency growth — the whole pass is skipped.
        engine.capture_nested_pubspecs();
        assert_eq!(engine.nested_scan_passes, 1, "warm turn scans nothing");
        assert_eq!(engine.pubspec_dirs_scanned, memo);
        // The next residency growth reopens the gate exactly once.
        engine.set_content(&root.join("lib/c.dart"), b"void c() {}\n");
        engine.apply_pending(&mut |_| {});
        assert!(engine.state.residency_epoch > engine.nested_scan_epoch);
        engine.capture_nested_pubspecs();
        assert_eq!(engine.nested_scan_passes, 2);
    }

    /// The validator face's mount (#2773): a bad pubspec (the
    /// `missing_name` shape) analyzed through the real pipeline emits the
    /// validator's row exactly once, attributed to the pubspec file
    /// itself — the commands-side pubspec pass carries no family rows
    /// anymore, so this face is the row's only source.
    #[test]
    fn pubspec_validator_row_flows_through_analyze_once() {
        let (root, mut engine) = facade_engine();
        std::fs::write(root.join("pubspec.yaml"), "environment:\n  sdk: ^3.5.0\n")
            .expect("pubspec");
        engine.set_content(&root.join("lib/a.dart"), b"void a() {}\n");
        let cancel = CancellationToken::new();
        let events = &mut |_: EngineEvent| {};
        let rows = engine.analyze(&[], events, &cancel).expect("analyze");
        assert_eq!(rows.len(), 1, "exactly one validator row: {rows:?}");
        assert_eq!(rows[0].code.name, "missing_name");
        assert_eq!(
            engine.state.files.path(rows[0].file),
            Some(root.join("pubspec.yaml").as_path()),
        );
    }

    /// The manifest face's gate (#2773): the captured
    /// `android/AndroidManifest.xml` validates only when the governing
    /// options enable `chrome-os-manifest-checks` — off by default (the
    /// oracle's field default, `analysis_options.dart` :54), on for a
    /// root entry carrying it, the `no_touchscreen_feature` row emitting
    /// exactly once when enabled.
    #[test]
    fn manifest_validator_rows_honor_the_gate() {
        let (root, mut engine) = facade_engine();
        std::fs::create_dir_all(root.join("android")).expect("android dir");
        std::fs::write(
            root.join("android/AndroidManifest.xml"),
            "<manifest xmlns:android=\"http://schemas.android.com/apk/res/android\">\n</manifest>\n",
        )
        .expect("manifest");
        engine.set_content(&root.join("lib/a.dart"), b"void a() {}\n");
        let cancel = CancellationToken::new();
        let events = &mut |_: EngineEvent| {};
        let off = engine.analyze(&[], events, &cancel).expect("gate off");
        assert!(
            off.iter().all(|row| row.code.name != "no_touchscreen_feature"),
            "default is off: {off:?}"
        );
        let mut options = analysis_options::AnalysisOptions::default();
        options.chrome_os_manifest_checks = Some(true);
        engine.state.options.insert(root.clone(), Arc::new(options));
        let on = engine.analyze(&[], events, &cancel).expect("gate on");
        assert_eq!(
            on.iter()
                .filter(|row| row.code.name == "no_touchscreen_feature")
                .count(),
            1,
            "exactly one manifest row: {on:?}"
        );
    }

    /// The shared-output face (#1387), end to end through the facade: the
    /// rows are byte-identical to the owned face on every path, and a
    /// zero-dirty cycle hands back the same standing allocation — one
    /// refcount, no row clones (`Arc::ptr_eq`). After an edit the rebuild
    /// runs under both the splice arm (the previous share dropped first, so
    /// the old rows move) and its full-merge fallback (a share still held),
    /// and the shared face stays row-equal to the owned face across both.
    /// The spliced rebuild pins against a fresh engine's cold analyze — a
    /// wholesale merge from scratch, not a re-serve of the standing cache,
    /// which would green-light a truncated splice against its own
    /// truncation.
    #[test]
    fn shared_analyze_matches_the_owned_face_and_shares_the_standing_rows() {
        let (root, mut engine) = facade_engine();
        let lib = root.join("lib");
        // Each file carries its own `dart:math` miss: the synthetic SDK
        // registers only `core`, so the import does not resolve — one
        // `uri_does_not_exist` row per file, so the sharing has rows to save.
        engine.set_content(
            &lib.join("a.dart"),
            b"import 'dart:math' show zork;\nint a() => 1;\n",
        );
        engine.set_content(
            &lib.join("b.dart"),
            b"import 'dart:math' show werm;\nint b() => 2;\n",
        );
        let cancel = CancellationToken::new();
        let events = &mut |_: EngineEvent| {};

        let owned = engine.analyze(&[], events, &cancel).expect("cold");
        assert_eq!(
            owned.len(),
            2,
            "one uri_does_not_exist row per file (the synthetic SDK \
             registers only core, so dart:math misses): {owned:?}"
        );

        // Zero-dirty cycles: the shared face serves the standing buffer.
        let first = engine.analyze_shared(&[], events, &cancel).expect("shared");
        let second = engine
            .analyze_shared(&[], events, &cancel)
            .expect("shared again");
        assert_eq!(first.as_ref(), &owned, "shared rows equal the owned face");
        assert_eq!(second.as_ref(), &owned);
        assert!(
            Arc::ptr_eq(&first, &second),
            "the standing buffer is shared, not re-cloned"
        );
        // The owned face over the same state agrees row-for-row: the shared
        // face's identity path changed nothing observable.
        assert_eq!(engine.analyze(&[], events, &cancel).expect("owned"), *first);

        // An edit rebuilds the merge. With the previous shares still held,
        // the splice cannot move the old rows and falls back to the full
        // merge — the degrade-to-clone arm — and the shared face stays
        // byte-equal to the owned face.
        engine.set_content(
            &lib.join("a.dart"),
            b"import 'dart:math' show zork;\nint a() => 1;\nint extra() => 3;\n",
        );
        let edited_shared = engine
            .analyze_shared(&[], events, &cancel)
            .expect("edited shared");
        let edited_owned = engine.analyze(&[], events, &cancel).expect("edited owned");
        assert_eq!(edited_shared.as_ref(), &edited_owned);
        // Dropping the held shares lets the next rebuild splice. The oracle
        // is a fresh engine's cold analyze of the same final state: a
        // wholesale merge from scratch, with no standing cache to share the
        // splice's failure modes (a re-analyze here would be served from
        // the standing buffer — truncation against truncation).
        drop(first);
        drop(second);
        drop(edited_shared);
        engine.set_content(
            &lib.join("b.dart"),
            b"import 'dart:math' show werm;\nint b() => 2;\nint more() => 4;\n",
        );
        let spliced_shared = engine
            .analyze_shared(&[], events, &cancel)
            .expect("spliced shared");
        let (fresh_root, mut fresh_engine) = facade_engine();
        fresh_engine.set_content(
            &fresh_root.join("lib/a.dart"),
            b"import 'dart:math' show zork;\nint a() => 1;\nint extra() => 3;\n",
        );
        let fresh_b = fresh_engine.set_content(
            &fresh_root.join("lib/b.dart"),
            b"import 'dart:math' show werm;\nint b() => 2;\nint more() => 4;\n",
        );
        let cold = fresh_engine
            .analyze(&[], events, &cancel)
            .expect("oracle cold analyze");
        assert_eq!(
            spliced_shared.as_ref(),
            &cold,
            "the spliced rebuild matches a from-scratch full merge"
        );
        // The re-analyzed contributor's presence pins from the oracle's own
        // rows: b.dart sorts last, so its fresh row — its own `dart:math`
        // miss (the synthetic SDK registers only core, and an unresolved
        // target skips the shown-name check, so no `werm` row exists) — is
        // exactly the tail the weave's exit edge must drain.
        let tail = cold.last().expect("the oracle holds both misses");
        assert_eq!(
            tail.file, fresh_b,
            "the trailing contributor's fresh row is the drained tail: {cold:?}"
        );
    }

    /// The fingerprint-keyed reuse gate (g485 W2), end to end: an API-face
    /// edit that lands *without* an analyze carries its closure forward as
    /// invalidated, and the undo restores every face the closure was
    /// about to serve — so the gated analyze recomputes only the edited
    /// library (its record dropped with its identity move) and keeps the
    /// importer's recorded run (its cycle fingerprint returned). A face
    /// that stays moved recomputes its whole downstream instead: the gate
    /// never fires, the trace stays silent.
    #[test]
    fn undo_restores_the_recorded_run_through_the_fingerprint_gate() {
        let (root, mut engine) = facade_engine();
        let lib = root.join("lib");
        let a_src = "int a() => 1;\n";
        let a_moved = "int a() => 1;\nint extra() => 2;\n";
        let b_src = "import 'a.dart';\nint b() => a();\n";
        let events = &mut |_: EngineEvent| {};
        let cancel = CancellationToken::new();

        engine.set_content(&lib.join("a.dart"), a_src.as_bytes());
        engine.set_content(&lib.join("b.dart"), b_src.as_bytes());
        let baseline = engine
            .analyze(&[], &mut |_: EngineEvent| {}, &cancel)
            .expect("cold");
        assert!(baseline.is_empty(), "the fixture analyzes clean");

        // The face move lands as bare applies (no analyze): the closure
        // carries forward, the edited library's record drops with its
        // identity, the importer's record survives untouched.
        engine.set_content(&lib.join("a.dart"), a_moved.as_bytes());
        engine.apply_pending(&mut |_: EngineEvent| {});
        engine.set_content(&lib.join("a.dart"), a_src.as_bytes());
        engine.apply_pending(events);

        // The gated analyze: one recompute (the edited library), one reuse.
        let trace =
            std::env::temp_dir().join(format!("dartq-engine-w2-undo-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&trace);
        engine.set_trace_sink(Some(event::TraceSink::create(&trace).expect("sink opens")));
        let undone = engine.analyze(&[], events, &cancel).expect("undo analyze");
        engine.set_trace_sink(None);
        assert_eq!(undone, baseline, "the undo serves the recorded runs");
        drop(engine);
        let capture = std::fs::read_to_string(&trace).expect("capture readable");
        assert_eq!(
            capture
                .lines()
                .filter(|line| line.contains("\"event\":\"CycleReused\""))
                .count(),
            1,
            "the standing fingerprint fired once: {capture}"
        );
        assert!(
            capture.contains("\"event\":\"CycleReused\",\"libraries\":1"),
            "exactly the importer's run stood: {capture}"
        );
        assert_eq!(
            capture
                .lines()
                .filter(|line| line.contains("\"event\":\"LibraryAnalyzed\""))
                .filter(|line| line.contains("\"from_cache\":false"))
                .count(),
            1,
            "only the edited library recomputed: {capture}"
        );

        // The negative control: a face that stays moved flips the whole
        // downstream — the importer's recorded digest no longer matches, so
        // both libraries recompute and no reuse row fires.
        let mut engine = facade_engine().1;
        engine.set_content(&lib.join("a.dart"), a_src.as_bytes());
        engine.set_content(&lib.join("b.dart"), b_src.as_bytes());
        engine
            .analyze(&[], &mut |_: EngineEvent| {}, &cancel)
            .expect("cold");
        engine.set_content(&lib.join("a.dart"), a_moved.as_bytes());
        engine.apply_pending(events);
        let trace2 = trace.with_extension("moved.jsonl");
        engine.set_trace_sink(Some(event::TraceSink::create(&trace2).expect("sink opens")));
        engine.analyze(&[], events, &cancel).expect("moved analyze");
        engine.set_trace_sink(None);
        drop(engine);
        let capture2 = std::fs::read_to_string(&trace2).expect("capture readable");
        assert!(
            !capture2.contains("\"event\":\"CycleReused\""),
            "a moved dependency face never reuses: {capture2}"
        );
        assert_eq!(
            capture2
                .lines()
                .filter(|line| line.contains("\"event\":\"LibraryAnalyzed\""))
                .filter(|line| line.contains("\"from_cache\":false"))
                .count(),
            2,
            "the face move recomputed the whole downstream: {capture2}"
        );
        let _ = std::fs::remove_file(&trace);
        let _ = std::fs::remove_file(&trace2);
    }

    /// The multi-apply clobber pin (#1567): a dependency's signature face
    /// move that lands as an out-of-analyze apply, followed by a second
    /// body-only apply before the next analyze, must survive the wait — the
    /// dirty flag means "any signature-bearing face moved since the last
    /// analyze", so applies accumulate and a body-only batch cannot retire
    /// an earlier apply's move. Under the overwrite, the gated analyze
    /// skipped the condensation rebuild and the reuse gate served the
    /// cascade-invalidated importer's pre-move recorded run. Pinned against
    /// a fresh cold engine over the same final contents: the stale serve
    /// would hand back the importer's clean run while the oracle reports the
    /// moved signature's broken shown name.
    #[test]
    fn body_only_apply_between_face_moves_keeps_the_cascade_recompute() {
        let (root, mut engine) = facade_engine();
        let lib = root.join("lib");
        // The face move renames the dependency's declaration while the
        // importer pins its shown name to the old one — so the move
        // genuinely moves the importer's rows (`undefined_shown_name`: the
        // rename drops `dep` from the export scope). A return-type edit
        // would not: the engine reports no row for it, and both the gated
        // and the oracle run would stay empty, pinning nothing.
        let dep_stable = "int dep() => 1;\n";
        let dep_moved = "int dep2() => 1;\n";
        let dep_body = "int dep2() => 11;\n";
        let importer = "import 'dep.dart' show dep;\nint use() => dep();\n";
        let events = &mut |_: EngineEvent| {};
        let cancel = CancellationToken::new();

        engine.set_content(&lib.join("dep.dart"), dep_stable.as_bytes());
        let use_id = engine.set_content(&lib.join("use.dart"), importer.as_bytes());
        let baseline = engine
            .analyze(&[], &mut |_: EngineEvent| {}, &cancel)
            .expect("cold");
        assert!(baseline.is_empty(), "the stable fixture analyzes clean");

        // Apply A: the dependency's declaration face moves (the rename) —
        // the importer cascade-invalidates with
        // its recorded run retained. Apply B: a body-only literal edit.
        engine.set_content(&lib.join("dep.dart"), dep_moved.as_bytes());
        engine.apply_pending(events);
        assert!(
            engine.state.structure_dirty,
            "the face move marks the structure dirty"
        );
        engine.set_content(&lib.join("dep.dart"), dep_body.as_bytes());
        engine.apply_pending(events);
        assert!(
            engine.state.structure_dirty,
            "the body-only apply must not retire the pending face move"
        );

        let warm = engine.analyze(&[], events, &cancel).expect("gated analyze");

        // The oracle: a fresh cold engine over the same final contents, same
        // interning order.
        let (fresh_root, mut fresh) = facade_engine();
        fresh.set_content(&fresh_root.join("lib/dep.dart"), dep_body.as_bytes());
        fresh.set_content(&fresh_root.join("lib/use.dart"), importer.as_bytes());
        let cold = fresh
            .analyze(&[], &mut |_: EngineEvent| {}, &cancel)
            .expect("oracle cold analyze");
        assert!(
            cold.iter().any(|row| row.file == use_id),
            "the moved signature must genuinely move the importer's \
             diagnostics, or this pin proves nothing: {cold:?}"
        );
        // The rows' messages embed each engine's own fixture root: the
        // shown-name warning renders the resolved target's absolute URI
        // (`directive_library_uri`), and warm/cold run over different temp
        // dirs. The comparison projects every field verbatim except that
        // one root prefix, normalized to a placeholder.
        let visible = |rows: &[Diagnostic], root: &Path| -> Vec<_> {
            let uri = format!("file://{}", root.display());
            rows.iter()
                .map(|row| {
                    (
                        row.severity,
                        row.code.name,
                        row.message.replace(uri.as_str(), "<fixture>"),
                        row.file,
                        (row.range.offset, row.range.length),
                        row.correction.clone(),
                    )
                })
                .collect()
        };
        assert_eq!(
            visible(&warm, &root),
            visible(&cold, &fresh_root),
            "the gated analyze recomputes the cascade"
        );
    }

    /// The options-rows face's local determinism (#1851): rows emit in
    /// options-file path order, one row set per distinct loaded path.
    /// Entries sharing a path collapse even when their loads don't share an
    /// Arc, and the winner between distinct loads of one path is the
    /// lowest governing root (a unique map key) — never hash iteration
    /// order.
    #[test]
    fn option_rows_emit_path_sorted_with_path_keyed_dedup() {
        let (root, mut engine) = facade_engine();
        let empty = |root: &Path| {
            PackageConfig::parse(r#"{"configVersion": 2, "packages": []}"#)
                .map(|config| (config, root.to_path_buf()))
                .expect("empty config")
        };
        // One options file loaded twice with different contents: same
        // file_path, distinct Arcs — the tie-break face the shared-Arc
        // alias cannot distinguish. The rewrite swaps the broken include
        // target, so the two loads carry different row messages.
        std::fs::write(
            root.join("analysis_options.yaml"),
            "include: missing.yaml\n",
        )
        .expect("options");
        let (config, config_dir) = empty(&root);
        let first = Arc::new(analysis_options::AnalysisOptions::load(
            &root.join("analysis_options.yaml"),
            &config,
            &config_dir,
        ));
        std::fs::write(
            root.join("analysis_options.yaml"),
            "include: replaced.yaml\n",
        )
        .expect("options");
        let (config, config_dir) = empty(&root);
        let second = Arc::new(analysis_options::AnalysisOptions::load(
            &root.join("analysis_options.yaml"),
            &config,
            &config_dir,
        ));
        // A second distinct path, sorting after the root file's.
        std::fs::create_dir_all(root.join("lib")).expect("lib dir");
        std::fs::write(
            root.join("lib/analysis_options.yaml"),
            "include: also_missing.yaml\n",
        )
        .expect("nested options");
        let (config, config_dir) = empty(&root);
        let nested = Arc::new(analysis_options::AnalysisOptions::load(
            &root.join("lib/analysis_options.yaml"),
            &config,
            &config_dir,
        ));
        assert!(
            !first.issues.is_empty() && !second.issues.is_empty() && !nested.issues.is_empty(),
            "the broken includes must record real rows"
        );

        // Three governing entries: two keys aliasing one path (distinct
        // loads — `root` sorts before `zkey`), one carrying the nested
        // path. The assembled map's synthesized default at `root` is
        // replaced.
        engine.state.options.insert(root.clone(), first);
        engine.state.options.insert(root.join("zkey"), second);
        engine.state.options.insert(root.join("lib"), nested);

        let rows = engine.option_issue_rows();
        assert_eq!(
            rows.len(),
            2,
            "aliased paths collapse to one row set each: {rows:?}"
        );
        // Path-sorted emission: the root file's row set before the lib
        // file's, whatever the map's iteration order was.
        let paths: Vec<_> = rows
            .iter()
            .map(|row| engine.state.files.path(row.file).expect("interned"))
            .collect();
        assert!(paths[0] < paths[1], "rows emit path-sorted: {paths:?}");
        // The dedup winner at the aliased path is the lowest governing
        // root's load — the `missing.yaml` spelling, not the `zkey`
        // entry's `replaced.yaml` rewrite.
        assert!(
            rows[0].message.contains("'missing.yaml'") && !rows[0].message.contains("replaced"),
            "the lowest governing root's load wins the aliased path: {:?}",
            rows[0].message
        );
        assert!(
            rows[1].message.contains("'also_missing.yaml'"),
            "the second path's row set follows: {:?}",
            rows[1].message
        );
    }

    /// A throwaway three-package workspace: the root `t`, an in-tree nested
    /// package `nested` (a path package beneath the workspace bound), and an
    /// out-of-tree path package `external` (a sibling directory, the
    /// fvm/flutter and pub-cache shape) — the fixture the closure-width
    /// posts pin. Returns `(workspace_root, external_root, engine)`.
    fn bounded_workspace_engine() -> (PathBuf, PathBuf, Engine) {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let parent = std::env::temp_dir().join(format!(
            "dartq-engine-bounded-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&parent);
        let write = |base: &Path, relative: &str, content: &str| {
            let path = base.join(relative);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(path, content).expect("file");
        };
        let root = parent.join("ws");
        let external = parent.join("external");
        write(
            &root,
            "pubspec.yaml",
            "name: t\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            &root,
            "pubspec.lock",
            "packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n",
        );
        write(
            &root,
            ".dart_tool/package_config.json",
            r#"{"configVersion": 2, "packages": [
                {"name": "t", "rootUri": "../", "packageUri": "lib/", "languageVersion": "3.5"},
                {"name": "nested", "rootUri": "../pkg/nested", "packageUri": "lib/", "languageVersion": "3.5"},
                {"name": "external", "rootUri": "../../external", "packageUri": "lib/", "languageVersion": "3.5"}
            ], "generator": "pub"}"#,
        );
        write(
            &root,
            "lib/a.dart",
            "import 'package:external/e.dart';\nimport 'package:nested/n.dart';\n\nclass A extends E {}\n",
        );
        write(&root, "pkg/nested/pubspec.yaml", "name: nested\n");
        write(&root, "pkg/nested/lib/n.dart", "class N {}\n");
        write(&external, "pubspec.yaml", "name: external\n");
        write(&external, "lib/e.dart", "class E {\n  void hi() {}\n}\n");
        // The dependency's own test tree: unreachable by import, and never
        // part of the oracle's analysis — the 18k-file closure shape.
        write(&external, "test/never_analyzed.dart", "class InTest {}\n");
        write(&parent, "sdk/version", "3.13.1\n");
        write(
            &parent,
            "sdk/lib/libraries.json",
            r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#,
        );
        write(
            &parent,
            "sdk/lib/_internal/allowed_experiments.json",
            r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
        );
        // Canonicalize before the pub faces resolve (the spelling universe).
        let root = root.canonicalize().expect("workspace root resolves");
        let external = external.canonicalize().expect("external root resolves");
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
            sdk: SdkSnapshot::load(&parent.join("sdk"), None).expect("synthetic SDK loads"),
            packages: PackageTable::build(&config, &graph, &config_dir).expect("table"),
            pub_config: config,
            config_dir,
            semdiag: Some(crate::semdiag::diagnoser()),
            target: DEFAULT_TARGET.to_owned(),
            declared_variables: FxHashMap::default(),
            experiments: Vec::new(),
        })
        .expect("engine");
        (root, external, engine)
    }

    /// The closure-width post (#2595): discovery walks the workspace bound
    /// only — the root package and the in-tree nested package — and never
    /// the out-of-tree dependency package, whatever the config lists. The
    /// oracle's `PackageConfigWorkspace` roots at the innermost
    /// package-config file and its folder walk covers that tree; the
    /// dependency's own files (its `test/` tree included) are resolution
    /// targets, never analysis surface.
    #[test]
    fn discovery_walks_the_workspace_bound_only() {
        let (root, external, mut engine) = bounded_workspace_engine();
        let stats = engine.discover_workspace().expect("discover");
        assert_eq!(
            stats.file_count, 2,
            "a.dart and pkg/nested/lib/n.dart — nothing under the external tree"
        );
        assert!(
            engine
                .file_table()
                .try_file_id(&root.join("lib/a.dart"))
                .is_some()
                && engine
                    .file_table()
                    .try_file_id(&root.join("pkg/nested/lib/n.dart"))
                    .is_some(),
            "the root's and the in-tree nested package's files discovered"
        );
        for path in ["lib/e.dart", "test/never_analyzed.dart"] {
            assert!(
                engine
                    .file_table()
                    .try_file_id(&external.join(path))
                    .is_none(),
                "the out-of-tree dependency's {path} must not be discovered"
            );
        }
    }

    /// The resolution post: the out-of-tree dependency still resolves —
    /// its interface loads lazily through the dep index (import-driven,
    /// row-silent), so the workspace file's rows see the dep's declared
    /// faces with no `uri_does_not_exist` staleness and no dep-tree rows.
    #[test]
    fn out_of_bound_dependencies_resolve_through_the_lazy_index() {
        let (root, external, mut engine) = bounded_workspace_engine();
        engine.discover_workspace().expect("discover");
        let diagnostics = engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze completes");
        let entry = external.join("lib/e.dart");
        assert!(
            engine.state.deps.units.contains_key(
                &engine
                    .state
                    .files
                    .try_file_id(&entry)
                    .expect("interned by the dep load")
            ),
            "the imported dep file loads through the lazy interface index"
        );
        assert!(
            diagnostics.iter().all(|row| {
                engine
                    .state
                    .files
                    .path(row.file)
                    .is_none_or(|path| !path.starts_with(&external))
            }),
            "dependency files stay row-silent: {diagnostics:?}"
        );
        let importer = diagnostics.iter().any(|row| {
            engine
                .state
                .files
                .path(row.file)
                .is_some_and(|path| path == root.join("lib/a.dart"))
                && row.code.name.contains("uri_")
        });
        assert!(
            !importer,
            "the workspace importer reports no URI-layer staleness: {diagnostics:?}"
        );
    }
}
