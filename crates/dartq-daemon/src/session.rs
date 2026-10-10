//! The session: one [`Runtime`] (the tagless default analysis context plus
//! every declared configuration fork, PRD §16.2) plus the open-document
//! overlay, the per-agent overlays (PRD §16.3), the publish-diff baseline,
//! the delta cursor, and the debounced snapshot sink. All engine and
//! scheduler calls happen here, synchronously, on the main-loop thread
//! (CPU-bound work, no lock contention by construction).
//!
//! # Task flow (PRD §16.5, §16.6)
//!
//! Intake (a notification or request arrives) only *stages* work: overlay
//! edits enqueue one write task plus the diagnostics read the publish push
//! rides on, a `dartq/*` request attaches to (or enqueues) one read task,
//! stamped with the context's epoch at enqueue. Execution
//! happens at the single [`Session::service`] drain point, in the
//! scheduler's writer-priority order — every staged write applies before any
//! read analyzes, so N batched edits collapse into one apply + one analysis,
//! and a read enqueued before a write still answers with the post-write
//! view. `$/cancelRequest` cancels per id (rust-analyzer's posture): the
//! coalesced read is partitioned — the cancelled id answers
//! `RequestCancelled`, its co-riders re-attach to a read re-stamped at the
//! fresh epoch (mirroring the write re-stamp), and when a write survives
//! with no request left riding on the read, a *zero-request read* is
//! re-stamped with it so the publish diff still drains — an edit+cancel
//! batch can never leave the client's diagnostics stale.
//!
//! # Context routing (PRD §16.2)
//!
//! The runtime holds one context per declared `dartq.yaml` row (assembled
//! at the shell edge — SDK and packages per context, every store wired to
//! one shared [`dartq_engine::ContentPool`] through
//! [`dartq_engine::pooled_memory_store`], the #483 construction contract)
//! plus the tagless default. A `dartq/*` request's `context` tag routes it
//! to that context's engine; a base write (an LSP edit, a watch sync, an
//! overlay commit) fans out to every context — content is not a fork
//! dimension — and only the default context's base analysis drains the
//! publish diff (the editor's view).
//!
//! # Agent views (PRD §16.3)
//!
//! `overlay.set`/`overlay.clear` stage per-agent `path → content` deltas on
//! the session (bounded: cleared on teardown, capped count). A request
//! carrying `agentId` must see that agent's view, so the drain partitions
//! its read task's requests into view groups — base first (its analysis
//! also feeds the publish diff; publishes never leak an agent view), then
//! one group per agent in first-appearance order. Each group materializes
//! its view onto the ONE shared engine: the group's overlay contents are
//! `set_content` over the base (base bytes captured at wear time — pending
//! content lands first, so a base edit that already applied underneath can
//! never be captured over), analyzed once, and answered; the next group
//! first restores the previous wear through the `worn` scratch. Every drain
//! ends by restoring the base view, so between turns the engine is
//! base-resident and captures never lie.
//!
//! Within one group the answering itself fans out (#487): the analyzed
//! view is `&`-shared across answer workers over a job queue, so k
//! coalesced requests cost one analysis plus `ceil(k / workers)` answer
//! passes instead of k serial ones. The analysis (and the agent-view
//! materialization) stays single-owner — `Engine::analyze` is `&mut` by
//! contract, the one serialization the single-writer engine imposes.
//!
//! The tradeoff is deliberate: per-agent engines would hold one full
//! derived state (tokens, AST arenas, indexes — a small multiple of source
//! bytes) per agent; materialization holds exactly one plus the sparse
//! per-agent deltas, at the cost of re-analyzing the overlapped libraries
//! per view switch — bounded by the overlay size, never the workspace.
//!
//! # Delta cursor (PRD §15.8)
//!
//! A per-session monotonic counter bumps once per applied content change
//! (a base overlay apply, an `overlay.set`); the changed map records each
//! explicitly written path's last-change cursor. `since:` answers filter
//! rows by that table and echo the current cursor. Cascade-invalidated
//! dependents are deliberately not marked: the changed set is the files
//! the client wrote.
//!
//! # Store hygiene (daemon-audit findings 2, 5)
//!
//! The runtime's shared overlay store is garbage-collected at the turn
//! boundary once a threshold of dead contents has accumulated
//! ([`Runtime::sweep`] is O(live), so idle sessions pay nothing), and each
//! scheduler task executes inside a `catch_unwind` boundary: a panicking
//! engine turn answers its requests `INTERNAL_ERROR` and the session
//! survives instead of taking the process down.
//!
//! The panic boundary holds no lock: the session owns the engine
//! exclusively and every engine call runs on the main-loop thread through
//! `&mut self`, so a panic cannot poison a shared mutex and there is
//! nothing to recover with `into_inner`. What a panic taints is engine
//! state, so the boundary treats it as untrusted for the rest of the turn
//! — the remaining view groups answer `INTERNAL_ERROR` without analyzing,
//! and a best-effort base restore (itself panic-contained) hands the next
//! turn the cleanest view achievable. Nothing is masked: the panic is
//! logged to stderr with its payload, and the affected requests see the
//! internal-error tier.
//!
//! # Tier mapping (PRD §16.5 — the one place it is defined)
//!
//! A task's [`Tier`] is fixed where it enters the scheduler, at enqueue: a
//! read task is `Interactive` only when a `dartq/*` request created it (a
//! client waits on the answer); everything else is `Normal` — overlay
//! writes (client edits and watch syncs alike), the publish-carrying reads
//! edits and watch changes enqueue, and the `initialized` sweep's first
//! analysis. `Background` is the engine's slot for indexing/prewarm
//! producers the daemon does not run yet. Tier never changes after enqueue:
//! a request attaching to an already-queued read keeps that read's tier
//! (drain order is unaffected while one read serves the whole turn anyway;
//! the distinction bites across contexts).
//!
//! # Snapshot persistence (#306)
//!
//! The sink is debounced by a last-apply timestamp: while it is dirty, the
//! main loop bounds its receive by the window's remaining time, so the save
//! runs at the deadline even when the daemon then sits idle (and regardless
//! of the window at session end). Save failures log and continue: the
//! accelerator invariant makes an unwritten generation a future miss, never
//! an error.

use std::collections::hash_map::Entry;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, unbounded};
use dartq_commands::Verbosity;
use dartq_commands::fix::PluginHosts;
use dartq_commands::layout::{path_key, severity_rank};
use dartq_diagnostics::{Diagnostic, FileId, Severity};
use dartq_engine::{
    CancellationToken, Cancelled, ContextConfig, ContextId, Engine, EngineConfig, EngineError,
    OverlayChange, PersistedFacts, Runtime, Tier,
};
use rustc_hash::FxHashMap;
use thiserror::Error;

use dartq_snapshot::{SnapshotConfig, SnapshotStore};

use crate::commands::{self, Request};
use crate::jsonrpc::RequestId;
use crate::lsp;
use crate::plugins::{self, PluginSet};
use crate::uri;

/// The debounce window between the last overlay apply and the snapshot
/// generation save (#306): 500 ms. While a save is pending the main loop's
/// idle receive is bounded by the window's remainder, so an idle daemon
/// wakes to save — no timer thread.
pub(crate) const SNAPSHOT_DEBOUNCE: Duration = Duration::from_millis(500);

/// The read-parallel contract (#487): a view group's coalesced requests
/// answer across the shared engine view on the answer workers, so the
/// engine must be `Sync`. Writes stay single-owner (`&mut`, the exclusive
/// analysis phase), and the engine's one interior-mutable cell — the string
/// table — is mutex-guarded by its own contract. Asserted at compile time
/// so an engine-side regression fails this crate's build, never a worker at
/// runtime.
const _: () = {
    const fn assert_sync<T: Sync>() {}
    assert_sync::<dartq_engine::Engine>();
};

/// The answer fan-out's worker cap: beyond it, spawn cost and scheduler
/// contention outweigh the marginal answering parallelism (each worker
/// shares one engine view — queries, JSON shaping — never the analysis).
const MAX_ANSWER_WORKERS: usize = 8;

/// The overlay-store GC threshold (daemon-audit finding 2): sweep once this
/// many new shared-store contents have accumulated since the last sweep.
/// Each content is one whole document text, so the garbage ceiling is
/// threshold × file size; the sweep itself is O(live entries).
const GC_CONTENTS_THRESHOLD: usize = 64;

/// The live agent-overlay cap: `overlay.set` naming an agent beyond it is
/// answered `INVALID_PARAMS` (clear one first). Teardown paths —
/// `overlay.clear`, session end — evict deterministically; the cap only
/// stops a client from growing the table without bound.
const MAX_AGENTS: usize = 256;

/// One agent's staged view (PRD §16.3): the files whose content the agent's
/// requests see instead of the base view. Base bytes are never stored here —
/// they are captured from the engine at wear time and restored through the
/// turn's `worn` scratch, so a base edit between staging and analysis can
/// never be served stale.
#[derive(Default)]
struct AgentOverlay {
    files: FxHashMap<PathBuf, OverlayFile>,
}

struct OverlayFile {
    bytes: Vec<u8>,
    /// The session cursor at which this content was staged (the delta
    /// answer's changed-file cursor for this agent's view of the path).
    set_at: u64,
}

/// The base content a worn path must be restored to.
#[derive(Clone)]
enum WornBase {
    /// The base bytes, captured post-write at wear time.
    Content(Vec<u8>),
    /// The path exists only in agent overlays; the base view lacks it.
    Absent,
}

/// Session failures the server can only report, not fix.
#[derive(Debug, Error)]
pub enum SessionError {
    /// Workspace discovery or an overlay restore hit an unreadable file.
    #[error("filesystem access failed: {0}")]
    Io(#[from] io::Error),
    /// The request's task was stale at the drain (its epoch was superseded
    /// by a cancel); completed work stays cached, and nothing was produced.
    #[error("analysis cancelled")]
    Cancelled,
    /// A `dartq/*` request's parameters name no valid target (the CLI's
    /// usage tier, surfaced as `INVALID_PARAMS`).
    #[error("{0}")]
    InvalidParams(String),
    /// A `dartq/*` command aborted after a usable request (the CLI's
    /// internal tier, surfaced as `INTERNAL_ERROR`).
    #[error("{0}")]
    Internal(String),
}

impl From<Cancelled> for SessionError {
    fn from(_: Cancelled) -> Self {
        Self::Cancelled
    }
}

/// One pending `textDocument/publishDiagnostics` push.
#[derive(Debug)]
pub struct Publish {
    pub uri: String,
    pub version: Option<i64>,
    pub diagnostics: Vec<lsp::Diagnostic>,
}

/// What one [`Session::service`] turn produced, in drain-execution order.
#[derive(Default)]
pub struct Turn {
    /// Publishes from the turn's read task (a no-op diff when nothing
    /// changed).
    pub publishes: Vec<Publish>,
    /// One entry per attached request; `Err` marks a request whose task was
    /// stale at the drain (cancelled) or whose parameters named no valid
    /// target, and produced nothing.
    pub answers: Vec<(RequestId, Result<serde_json::Value, SessionError>)>,
}

/// A queued read task and the requests waiting on its answer. Reads coalesce
/// per context: one analysis of that context's engine serves every attached
/// request plus (on the default context) the publish diff. `tier` is the
/// queued task's precedence, remembered so a cancel re-stamp keeps it.
struct PendingRead {
    /// The context whose engine the task analyzes.
    context: ContextId,
    /// The scheduler task's cookie (the drain report names tasks by it).
    cookie: u64,
    requests: Vec<(RequestId, Request)>,
    tier: Tier,
}

/// Debounced snapshot persistence state: dirty since the last overlay apply,
/// the instant that apply settled, and the store to save into.
struct SnapshotSink {
    store: Option<SnapshotStore>,
    dirty: bool,
    last_apply: Option<Instant>,
}

impl SnapshotSink {
    fn note_apply(&mut self) {
        self.dirty = true;
        self.last_apply = Some(Instant::now());
    }

    /// When the pending save comes due (`last_apply + window`); `None` when
    /// the sink is clean, meaning the main loop's receive may block
    /// indefinitely. `dirty` implies `last_apply` — both are set together
    /// by [`Self::note_apply`] — so this is the single source of truth for
    /// the window math, shared by the idle receive bound and `flush_if_due`.
    fn deadline(&self) -> Option<Instant> {
        self.last_apply
            .filter(|_| self.dirty)
            .map(|at| at + SNAPSHOT_DEBOUNCE)
    }

    /// Saves when the debounce window has elapsed since the last apply;
    /// true when a save was attempted.
    fn flush_if_due(&mut self, now: Instant) -> bool {
        match self.deadline() {
            Some(deadline) if now >= deadline => {
                self.flush();
                true
            }
            _ => false,
        }
    }

    fn flush(&mut self) {
        self.dirty = false;
        #[cfg(test)]
        crate::probe::count_flush();
    }
}

/// The daemon's analysis state: the runtime with the default context plus
/// every declared one, open (overlay) documents, the per-agent overlay
/// views, the last published diagnostics per file, the delta cursor, the
/// staged-but-undrained task set, and the snapshot sink.
pub struct Session {
    runtime: Runtime,
    /// Context routing (PRD §16.2): tag → context id. The tagless default
    /// context is [`Session::default_context`]; a tag naming an equal
    /// configuration shares its context (§16.1 dedup).
    contexts: FxHashMap<String, ContextId>,
    /// The tagless default context: the LSP-edit view and the publish-diff
    /// source. Every session has exactly one.
    default_context: ContextId,
    /// The distinct open context ids (default first) — the base-write
    /// fan-out and the per-context snapshot walks. Contexts never close in
    /// a session, so this stays valid for its life.
    context_ids: Vec<ContextId>,
    /// The workspace root the contexts were assembled from (relative
    /// `dartq/*` request paths resolve against it).
    workspace_root: PathBuf,
    /// The resolved SDK root of the default context — `dartq/explain`'s
    /// default when its request omits `sdk` (the default context is what
    /// the session publishes against, so the report must come from the
    /// same one).
    sdk_root: PathBuf,
    next_cookie: u64,
    /// Open overlay documents: last-known client version per path. A closed
    /// or never-opened file is absent — publishes then carry no version.
    opened: FxHashMap<PathBuf, i64>,
    /// Baseline for publish diffing: a push only goes out when a file's
    /// diagnostic set differs from what was last published for it.
    published: FxHashMap<dartq_diagnostics::FileId, Vec<lsp::Diagnostic>>,
    /// Per-agent overlay views (PRD §16.3), keyed by `agentId`. Bounded:
    /// cleared on `overlay.clear`/session end, capped at [`MAX_AGENTS`].
    agents: FxHashMap<String, AgentOverlay>,
    /// Paths currently deviating from ONE context's base view, with that
    /// context and the base content to restore. Within-turn scratch only:
    /// every drain ends by restoring the base view, so between turns this
    /// is empty and wear-time captures read a base-resident engine.
    worn: Option<(ContextId, Vec<(PathBuf, WornBase)>)>,
    /// The delta cursor (PRD §15.8): monotonic per session, bumped once per
    /// applied content change.
    cursor: u64,
    /// Each explicitly-written path's last-change cursor (base view).
    changed: FxHashMap<PathBuf, u64>,
    /// A base write applied this session whose publish diff has not drained
    /// yet: forces the default context's base analysis even when only
    /// context-tagged or agent requests ride the read, so the diff still
    /// goes out.
    publishes_due: bool,
    /// The session symbol table (short-id economics, PRD §15.8).
    symbols: commands::SymbolTable,
    /// Shared-store contents at the last overlay-store sweep (the GC
    /// watermark).
    gc_watermark: usize,
    /// Staged overlay mutations riding the queued write task (empty ⇔ no
    /// write task queued — the two change together).
    staged: Vec<OverlayChange>,
    /// The queued write task's cookie.
    write: Option<u64>,
    /// Queued read tasks, one per context at most (reads coalesce per
    /// context).
    reads: Vec<PendingRead>,
    /// Requests cancelled while queued, answered cancelled at the drain.
    cancelled: Vec<RequestId>,
    /// The plugin set (PRD §14, #471 phase B): discovered under the
    /// workspace's `plugins/` convention at assembly and spawned eagerly
    /// (the handshake completes before the first turn). Empty when the
    /// workspace declares none — the per-cycle weave skips its drain
    /// budget with it.
    plugins: PluginSet,
    /// The `dartq/fix`/`dartq/assists` host set: lazily launched at the
    /// first plugin-needing request (a session that never fixes never pays
    /// the spawn handshakes). Session-owned so the graceful teardown covers
    /// both plugin faces from one place.
    fix_hosts: Option<PluginHosts>,
    snapshot: SnapshotSink,
}

impl Session {
    /// Assembles the default context from the pre-validated configuration
    /// (the CLI owns SDK and workspace discovery; the daemon is shell only)
    /// and opens the XDG-rooted snapshot store. A missing cache root only
    /// means no persistence (accelerator semantics), never a failed session.
    ///
    /// # Errors
    ///
    /// [`EngineError::PackageConfig`] when the config cannot back the
    /// URI-resolution table.
    pub fn new(config: EngineConfig) -> Result<Self, EngineError> {
        let store = match SnapshotStore::open(SnapshotConfig::default()) {
            Ok(store) => Some(store),
            Err(error) => {
                eprintln!("dartq-daemon: snapshot store unavailable: {error}");
                None
            }
        };
        Self::assemble(config, Vec::new(), store)
    }

    /// [`Session::new`] with an explicit snapshot store (tests pin a
    /// throwaway root).
    ///
    /// # Errors
    ///
    /// [`EngineError::PackageConfig`] when the config cannot back the
    /// URI-resolution table.
    pub fn with_store(config: EngineConfig, store: SnapshotStore) -> Result<Self, EngineError> {
        Self::assemble(config, Vec::new(), Some(store))
    }

    /// [`Session::new`] plus declared contexts: `extra` names one context
    /// each — a `dartq.yaml` `contexts` row assembled at the shell edge,
    /// its per-context fork dimensions (SDK, target, declared variables)
    /// already in the config, its store built from
    /// [`dartq_engine::pooled_memory_store`] over the ONE shared
    /// [`dartq_engine::ContentPool`] (the #483 multi-context construction
    /// contract; standalone stores would multiply content per context).
    /// A request routes to a context by its `context` tag; the tagless
    /// default stays the base context. Equal configurations dedup to one
    /// runtime context (§16.1) — two tags may share one id, and a repeated
    /// tag re-points at its later config.
    ///
    /// # Errors
    ///
    /// [`EngineError::PackageConfig`] when any config cannot back the
    /// URI-resolution table.
    pub fn with_contexts<I>(
        config: EngineConfig,
        extra: I,
        store: SnapshotStore,
    ) -> Result<Self, EngineError>
    where
        I: IntoIterator<Item = (String, EngineConfig)>,
    {
        Self::assemble(config, extra.into_iter().collect(), Some(store))
    }

    fn assemble(
        config: EngineConfig,
        extra: Vec<(String, EngineConfig)>,
        store: Option<SnapshotStore>,
    ) -> Result<Self, EngineError> {
        let mut runtime = Runtime::new();
        // The contexts' workspace root (`config_dir` = root/.dart_tool) and
        // the default context's resolved SDK root; both captured before the
        // config moves into the context.
        let workspace_root = config
            .config_dir
            .parent()
            .map_or_else(Default::default, Path::to_path_buf);
        let sdk_root = config.sdk.dart_sdk_root().to_path_buf();
        // Opens one context and warm-starts it before its first apply: the
        // context's own header (`snapshot_header` reads the context's own
        // target / declared variables / options — the salt's dimensions, so
        // forked contexts can never match another's generation) keys the
        // store load, and the staged facts feed the first apply. A missing
        // or mismatched generation is a cold start (accelerator semantics),
        // never an error.
        let open =
            |runtime: &mut Runtime, engine: EngineConfig| -> Result<ContextId, EngineError> {
                let id = runtime.open_context(ContextConfig { engine, flags: 0 })?;
                if let Some(store) = &store {
                    let engine = runtime
                        .context_state_mut(id)
                        .expect("the context opened above");
                    let header = engine.snapshot_header();
                    if let Some(snapshot) = store.load::<PersistedFacts>(&header) {
                        engine.ingest_facts_snapshot(snapshot);
                    }
                }
                Ok(id)
            };
        let default_context = open(&mut runtime, config)?;
        let mut contexts = FxHashMap::default();
        let mut context_ids = vec![default_context];
        for (tag, engine) in extra {
            let id = open(&mut runtime, engine)?;
            if !context_ids.contains(&id) {
                context_ids.push(id);
            }
            contexts.insert(tag, id);
        }
        // The plugin set spawns eagerly at assembly (PRD §14, #471 phase B):
        // the handshake completes before the first turn. An empty discovery
        // costs one directory walk and spawns nothing.
        let plugins = PluginSet::launch(&workspace_root);
        Ok(Self {
            runtime,
            contexts,
            default_context,
            context_ids,
            workspace_root,
            sdk_root,
            next_cookie: 0,
            opened: FxHashMap::default(),
            published: FxHashMap::default(),
            agents: FxHashMap::default(),
            worn: None,
            cursor: 0,
            changed: FxHashMap::default(),
            publishes_due: false,
            symbols: commands::SymbolTable::default(),
            gc_watermark: 0,
            staged: Vec::new(),
            write: None,
            reads: Vec::new(),
            cancelled: Vec::new(),
            plugins,
            fix_hosts: None,
            snapshot: SnapshotSink {
                store,
                dirty: false,
                last_apply: None,
            },
        })
    }

    /// The `initialized`-time workspace sweep: discover every surface file
    /// into every context (content is shared, each engine interns its own
    /// residency) and enqueue the initial default-context analysis read
    /// (its publishes flow from the turn's drain).
    ///
    /// # Open-document precedence
    ///
    /// Discovery re-stages disk bytes for every resident path, which would
    /// clobber a client-opened document the same way an unguarded watch
    /// sync would ([`Self::queue_disk_changes`] skips opened paths for
    /// exactly this reason). Each open path's content is therefore
    /// re-staged after the walk: the pending map is last-wins per path, so
    /// the client's text (staged bytes when an edit is still queued,
    /// resident bytes otherwise) displaces the disk bytes discovery just
    /// enqueued, and the queued read's apply keeps the overlay intact.
    ///
    /// # Errors
    ///
    /// Discovery I/O.
    pub fn sweep(&mut self) -> Result<(), SessionError> {
        let mut opened: Vec<(PathBuf, Vec<u8>)> = Vec::with_capacity(self.opened.len());
        for path in self.opened.keys() {
            // A queued edit has not applied yet — its staged bytes are the
            // client's latest word (at most one entry per path; see
            // [`Self::stage`]). Otherwise the open text is the resident one.
            let staged = self.staged.iter().find_map(|change| match change {
                OverlayChange::Set {
                    path: staged,
                    bytes,
                } if staged == path => Some(bytes),
                _ => None,
            });
            if let Some(bytes) = staged {
                opened.push((path.clone(), bytes.clone()));
                continue;
            }
            if let Some(bytes) = self
                .engine(self.default_context)
                .file_table()
                .try_file_id(path)
                .and_then(|file| self.engine(self.default_context).content(file))
            {
                opened.push((path.clone(), bytes.to_vec()));
            }
        }
        for index in 0..self.context_ids.len() {
            self.engine_mut(self.context_ids[index])
                .discover_workspace()?;
            for (path, bytes) in &opened {
                self.engine_mut(self.context_ids[index])
                    .set_content(path, bytes);
            }
        }
        self.ensure_read(self.default_context, Tier::Normal);
        Ok(())
    }

    /// `textDocument/didOpen` / `textDocument/didChange` (whole-file
    /// replacement, M1 overlay semantics, PRD §16.3): stages the overlay
    /// text on the queued write task and co-queues the diagnostics read the
    /// publish push rides on (reads coalesce, so N batched edits still
    /// reanalyze once). False for non-`file:` URIs, which have no analysis
    /// path and are skipped.
    pub fn queue_edit(&mut self, uri: &str, text: &str, version: i64) -> bool {
        let Some(path) = uri::to_path(uri) else {
            return false;
        };
        self.opened.insert(path.clone(), version);
        self.stage(OverlayChange::Set {
            path,
            bytes: text.as_bytes().to_vec(),
        });
        self.ensure_write();
        self.ensure_read(self.default_context, Tier::Normal);
        true
    }

    /// `textDocument/didClose`: stages the overlay drop. The runtime overlay
    /// has no per-file "restore" change, so the equivalent whole-file
    /// operation stages the on-disk bytes; an overlay-only file (opened,
    /// never saved) stages a removal. Closing a document that was never
    /// opened is a no-op per LSP.
    ///
    /// The on-disk read happens before any state mutation: on a read error
    /// that is not `NotFound` the document stays open and any staged edits
    /// stay queued — the overlay bookkeeping and the staged writes only ever
    /// diverge from what a successful close would have done.
    ///
    /// # Errors
    ///
    /// Disk read failure.
    pub fn queue_close(&mut self, uri: &str) -> io::Result<bool> {
        let Some(path) = uri::to_path(uri) else {
            return Ok(false);
        };
        if !self.opened.contains_key(&path) {
            return Ok(false);
        }
        match fs::read(&path) {
            Ok(bytes) => {
                self.opened.remove(&path);
                self.stage(OverlayChange::Set { path, bytes });
            }
            // Overlay-only buffer: dropping the overlay removes the file.
            // `UnknownPath` cannot happen — `open` interned the path.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.opened.remove(&path);
                self.stage(OverlayChange::Remove { path });
            }
            Err(error) => return Err(error),
        }
        self.ensure_write();
        self.ensure_read(self.default_context, Tier::Normal);
        Ok(true)
    }

    /// Watch-mode intake (`--watch`, PRD §16.7 mode 3): syncs each reported
    /// path's on-disk bytes into the base view — the didChange-equivalent
    /// the change-detection discipline names — and co-queues the Normal-tier
    /// read the publish push rides on. The watcher's walk is the on-disk
    /// truth, so a path that is new or changed queues the same set the
    /// sweep's discovery takes (new files enter the analysis) and a vanished
    /// path queues a removal (the file drops out of the view).
    ///
    /// # Overlay precedence
    ///
    /// A client-opened document ([`Self::queue_edit`], tracked in `opened`)
    /// owns its content until `didClose`: a watch sync never clobbers it.
    /// Concretely, a touched-on-disk path with client content is skipped
    /// entirely, and a deleted one stays as overlay-only content — the open
    /// buffer survives until `didClose`, which re-syncs the disk state
    /// (staging the removal when the file never came back). Agent overlays
    /// need no guard here: their views materialize at wear time over
    /// whatever the base holds, so agent content wins its own view while
    /// the base keeps tracking the disk.
    ///
    /// Watch files are not LSP-opened, so their publishes carry no version.
    /// Per-path independence: an unreadable file is logged and skipped —
    /// watch events are hints, and the analysis's content check stays the
    /// truth (§16.7).
    pub fn queue_disk_changes(&mut self, paths: &[PathBuf]) {
        let mut synced = false;
        for path in paths {
            if self.opened.contains_key(path) {
                continue;
            }
            match fs::read(path) {
                Ok(bytes) => {
                    self.stage(OverlayChange::Set {
                        path: path.clone(),
                        bytes,
                    });
                    synced = true;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    self.stage(OverlayChange::Remove { path: path.clone() });
                    synced = true;
                }
                Err(error) => {
                    eprintln!(
                        "dartq-daemon: watch sync skipped {}: {error}",
                        path.display()
                    );
                }
            }
        }
        if synced {
            self.ensure_write();
            self.ensure_read(self.default_context, Tier::Normal);
        }
    }

    /// A `dartq/*` request: resolves its `context` tag to the engine
    /// context that answers it, attaches to that context's queued read task
    /// (or queues one), stamped with the context's epoch at enqueue.
    /// Answered at the drain, in drain order — the coalesced analysis runs
    /// once, and each request's command shapes its own answer from the
    /// post-write state. Crate-internal: the wire face is
    /// [`crate::server`]'s method table.
    pub(crate) fn queue_command(&mut self, id: RequestId, request: Request) {
        let context = self.route(request.context());
        self.ensure_read(context, Tier::Interactive);
        if let Some(pending) = self
            .reads
            .iter_mut()
            .find(|pending| pending.context == context)
        {
            pending.requests.push((id, request));
        }
    }

    /// `dartq/overlay.set`: stages (merging per path) one agent's content
    /// deltas. Lazy — no analysis runs here; the next request naming the
    /// agent materializes the view. Bumps the delta cursor once.
    ///
    /// # Errors
    ///
    /// [`SessionError::InvalidParams`] when the agent would exceed
    /// [`MAX_AGENTS`] (clear one first).
    pub fn set_overlay(
        &mut self,
        agent: &str,
        files: Vec<(PathBuf, Vec<u8>)>,
    ) -> Result<(), SessionError> {
        if !self.agents.contains_key(agent) && self.agents.len() >= MAX_AGENTS {
            return Err(SessionError::InvalidParams(format!(
                "too many live agent overlays ({MAX_AGENTS}); clear one first"
            )));
        }
        if files.is_empty() {
            return Ok(());
        }
        let entry = self.agents.entry(agent.to_owned()).or_default();
        self.cursor += 1;
        for (path, bytes) in files {
            entry.files.insert(
                path,
                OverlayFile {
                    bytes,
                    set_at: self.cursor,
                },
            );
        }
        Ok(())
    }

    /// `dartq/overlay.clear`: evicts the agent's staged view (idempotent).
    pub fn clear_overlay(&mut self, agent: &str) {
        self.agents.remove(agent);
    }

    /// Whether `agent` has a live overlay view (the server's queue-time
    /// `agentId` validation).
    pub(crate) fn has_agent(&self, agent: &str) -> bool {
        self.agents.contains_key(agent)
    }

    /// Whether `tag` names a declared context (the server's queue-time
    /// `context` validation).
    pub(crate) fn has_context(&self, tag: &str) -> bool {
        self.contexts.contains_key(tag)
    }

    /// Resolves a request's context tag to its engine context: `None` is
    /// the default. An unknown tag cannot reach here — the server validated
    /// it at intake ([`Session::has_context`]).
    fn route(&self, tag: Option<&str>) -> ContextId {
        match tag {
            Some(tag) => self
                .contexts
                .get(tag)
                .copied()
                .expect("the tag was validated at intake"),
            None => self.default_context,
        }
    }

    /// `dartq/overlay.commit` (PRD §16.3's merge): writes the agent's
    /// staged whole-file buffers back to the worktree, merges them into the
    /// base view — the same whole-file overlay change a watch sync takes —
    /// and frees the agent's staged entries (the base owns the committed
    /// content; the agent's view falls through to it). The merge applies at
    /// the queued write task's drain (affected `ContentId`s recompute, the
    /// publish diff drains); this face answers once the bytes are on disk.
    /// Returns the committed file count.
    ///
    /// The write-back is whole-buffer replacement, consistent with the
    /// overlay's own M1 semantics: the staged buffer IS the end state, so
    /// there is no offset arithmetic to go stale — an agent buffer that
    /// drifted from the disk text it started from simply writes over it.
    ///
    /// # Errors
    ///
    /// [`SessionError::InvalidParams`] for an unknown agent;
    /// [`SessionError::Io`] when a disk write fails — files already written
    /// stay written (the write-back is not transactional; the base view
    /// catches up through the next sync), nothing merges, and the agent's
    /// view stays intact.
    ///
    /// # Panics
    ///
    /// Never in practice: the second table lookup repeats the first's
    /// proven agent id, so the `expect` is a structural invariant, not
    /// input-dependent.
    pub fn commit_overlay(&mut self, agent: &str) -> Result<usize, SessionError> {
        let Some(overlay) = self.agents.get(agent) else {
            return Err(SessionError::InvalidParams(format!(
                "unknown agentId: {agent}"
            )));
        };
        if overlay.files.is_empty() {
            return Ok(0);
        }
        let committed: Vec<(PathBuf, Vec<u8>)> = overlay
            .files
            .iter()
            .map(|(path, file)| (path.clone(), file.bytes.clone()))
            .collect();
        // Write-back first: nothing merges until every file is on disk.
        for (path, bytes) in &committed {
            fs::write(path, bytes)?;
        }
        let count = committed.len();
        for (path, bytes) in committed {
            self.stage(OverlayChange::Set { path, bytes });
        }
        self.agents
            .get_mut(agent)
            .expect("the agent was checked above")
            .files
            .clear();
        self.ensure_write();
        self.ensure_read(self.default_context, Tier::Normal);
        Ok(count)
    }

    /// The workspace root the session's context was assembled from.
    pub(crate) fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// `$/cancelRequest`: cancels one queued request by id, precisely
    /// (rust-analyzer cancels per id, not per batch). The coalesced read
    /// task is partitioned: the cancelled id answers `RequestCancelled` at
    /// the drain, while co-riding requests re-attach to a read task
    /// re-stamped at the fresh epoch — mirroring the write re-stamp below.
    /// When the cancelled request rode the default context and a write task
    /// survives (staged, unobserved edits), the write re-stamps too and a
    /// zero-request read joins it, so the publish diff the cancelled read
    /// was carrying still runs. Cancels on other contexts' reads leave the
    /// write alone — it rides the default context's epoch.
    ///
    /// Cancelling an unknown or already-answered id is ignored (LSP
    /// §cancelRequest). The staged overlay edits are *not* cancellable (LSP
    /// notifications carry no ids).
    ///
    /// # Panics
    ///
    /// Never in practice: the re-stamps enqueue against open session
    /// contexts, which live for the session.
    pub fn cancel_request(&mut self, id: &RequestId) {
        let Some(read) = self
            .reads
            .iter()
            .position(|pending| pending.requests.iter().any(|(queued, _)| queued == id))
        else {
            return;
        };
        let mut pending = self.reads.remove(read);
        let position = pending
            .requests
            .iter()
            .position(|(queued, _)| queued == id)
            .expect("the scan above proved the id is queued");
        let (cancelled, _) = pending.requests.remove(position);
        self.cancelled.push(cancelled);
        let _ = self.runtime.cancel(pending.context);
        // Re-stamp the surviving work at the fresh epoch, keeping each
        // task's tier — the stale (superseded) tasks drop at the drain.
        let write_rides_cancel = self.write.is_some() && pending.context == self.default_context;
        if write_rides_cancel {
            let cookie = self.write.expect("checked above");
            self.runtime
                .enqueue(self.default_context, true, Tier::Normal, cookie)
                .expect("the default context lives for the session; write re-stamp cannot fail");
        }
        let survivors = pending.requests;
        if !survivors.is_empty() || write_rides_cancel {
            let cookie = self.next_cookie();
            self.runtime
                .enqueue(pending.context, false, pending.tier, cookie)
                .expect("the read's context lives for the session; read re-stamp cannot fail");
            self.reads.push(PendingRead {
                context: pending.context,
                cookie,
                requests: survivors,
                tier: pending.tier,
            });
        }
    }

    /// The turn's drain point: pops the scheduler in writer-priority order,
    /// applies every staged write (fanning out to every context), runs each
    /// context's coalesced read (partitioned into per-view groups), and
    /// drops stale tasks — dropped requests answer
    /// [`SessionError::Cancelled`]. Each task executes inside a
    /// panic boundary (daemon-audit finding 5): a panicking engine turn
    /// answers its requests `INTERNAL_ERROR` instead of killing the
    /// process, and the turn ends with the base view restored and the
    /// overlay store swept once its garbage crossed the threshold.
    ///
    /// # Panics
    ///
    /// Never escapes: engine panics are caught at the task boundary.
    /// Enqueue paths still `expect` the session contexts to live for the
    /// session, which is a structural invariant, not input-dependent.
    /// See the structural contract in the panics section above; the
    /// returned [`Turn`] carries the drain's publishes and answers.
    #[must_use]
    pub fn service(&mut self) -> Turn {
        let report = self.runtime.drain();
        let mut turn = Turn::default();
        // Cancelled requests were enqueued before every surviving task, so
        // their answers come first.
        turn.answers.extend(
            self.cancelled
                .drain(..)
                .map(|id| (id, Err(SessionError::Cancelled))),
        );
        for task in report.tasks {
            if task.write {
                let changes = std::mem::take(&mut self.staged);
                self.write = None;
                let context_ids = self.context_ids.clone();
                let applied = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    // The base write fans out to every context: content is
                    // not a fork dimension (§16.2), so each context's view
                    // of the workspace moves with the edit.
                    for context in context_ids {
                        self.runtime
                            .apply_overlay(context, &changes, &mut |_| {})
                            .expect("a drain-live write references an open session context");
                    }
                }));
                match applied {
                    Ok(_) => {
                        self.snapshot.note_apply();
                        self.cursor += 1;
                        for change in &changes {
                            let path = match change {
                                OverlayChange::Set { path, .. }
                                | OverlayChange::Remove { path } => path,
                            };
                            self.changed.insert(path.clone(), self.cursor);
                        }
                        self.publishes_due = true;
                    }
                    Err(payload) => {
                        eprintln!(
                            "dartq-daemon: overlay apply panicked ({}); the write did not land",
                            panic_message(&payload)
                        );
                    }
                }
            } else {
                let position = self
                    .reads
                    .iter()
                    .position(|pending| pending.cookie == task.cookie)
                    .expect("a drain-live read task is still pending");
                let pending = self.reads.remove(position);
                self.read_turn(pending, &mut turn);
            }
        }
        // Between turns the engine is base-resident: agent wears never
        // outlive the drain, so intake-time state reads and the next
        // drain's wear-time captures never see a stale view.
        self.restore_base_view();
        self.gc_if_due();
        turn
    }

    /// End-of-session flush: a dirty sink saves regardless of the debounce
    /// window (#306: the clean-shutdown write).
    pub fn flush_snapshot(&mut self) {
        if self.snapshot.dirty {
            self.snapshot.flush();
            self.save_generation();
        }
    }

    /// The session-end plugin teardown (#471 phase B): the graceful
    /// handshake per live runtime, both the cycle-fed set and the fix-face
    /// hosts (`Drop` alone is kill-only). Idempotent — both sets are taken.
    /// Errors log inside; teardown quality never fails the session.
    pub fn shutdown_plugins(&mut self) {
        std::mem::take(&mut self.plugins).shutdown();
        if let Some(hosts) = self.fix_hosts.take() {
            hosts.shutdown();
        }
    }

    /// The live fix-face host set, launching it on first use (the
    /// manifests' spawn + handshake cost lands on the first plugin-needing
    /// request).
    pub(crate) fn fix_hosts(&mut self) -> &mut PluginHosts {
        self.fix_hosts
            .get_or_insert_with(|| PluginHosts::launch(&self.workspace_root))
    }

    /// The generation save (#309, #306): one
    /// [`dartq_snapshot::GenerationHeader`] plus per-file fact entries per
    /// context, through the engine's own flush face
    /// ([`Engine::save_facts_snapshot`]). The header's context salt
    /// separates forked contexts' generation directories (#482 — target,
    /// declared variables, and analysis options ride the salt), so N
    /// contexts coexist collision-free in one store. A context whose
    /// configuration equals another's shares its generation (one save,
    /// one directory). Failures log and continue: the accelerator
    /// invariant makes an unwritten generation a future miss, never an
    /// error.
    fn save_generation(&self) {
        let Some(store) = self.snapshot.store.as_ref() else {
            return;
        };
        for &context in &self.context_ids {
            let engine = self
                .runtime
                .context_state(context)
                .expect("session contexts live for the session");
            if let Err(error) = engine.save_facts_snapshot(store) {
                eprintln!("dartq-daemon: snapshot save skipped for context {context}: {error}");
            }
        }
    }

    /// The shared overlay store's live content count (the GC test's probe).
    #[cfg(test)]
    pub(crate) fn overlay_store_contents(&self) -> usize {
        self.runtime.stored_contents()
    }

    /// Turn-start debounce check; true when a save was attempted.
    #[must_use]
    pub fn flush_if_due(&mut self, now: Instant) -> bool {
        if self.snapshot.flush_if_due(now) {
            self.save_generation();
            true
        } else {
            false
        }
    }

    /// When the pending debounced save comes due: the main loop bounds its
    /// idle receive by this, so a daemon left idle wakes at the deadline and
    /// saves instead of blocking forever. `None` when the sink is clean —
    /// the receive blocks until the next frame.
    #[must_use]
    pub fn flush_deadline(&self) -> Option<Instant> {
        self.snapshot.deadline()
    }

    /// One read task's execution: the requests partition into per-view
    /// groups (base first — on the default context its analysis also feeds
    /// the publish diff — then one group per agent in first-appearance
    /// order), and each group materializes its view onto the task's context
    /// engine and analyzes once. When a base write applied but no default
    /// base request rides a default-context read, a zero-request base
    /// analysis still runs so the publish diff drains.
    fn read_turn(&mut self, pending: PendingRead, turn: &mut Turn) {
        let mut groups: Vec<(Option<String>, Vec<(RequestId, Request)>)> = Vec::new();
        for (id, request) in pending.requests {
            let agent = request.agent().map(str::to_owned);
            match groups.iter_mut().find(|(group, _)| *group == agent) {
                Some((_, requests)) => requests.push((id, request)),
                None => groups.push((agent, vec![(id, request)])),
            }
        }
        // Base first (stable): its analysis runs on the base-resident engine
        // and feeds the publish diff before any agent wear flips content.
        groups.sort_by_key(|(agent, _)| agent.is_some());

        // The publish diff drains only through the default context's base
        // group. A read task that exists for that diff alone — the sweep's
        // first analysis, a write-riding read whose requests all cancelled —
        // still runs its base analysis once; reads on other contexts have
        // no publish duty.
        let has_base = groups.iter().any(|(agent, _)| agent.is_none());
        let publishes_here = pending.context == self.default_context;
        if publishes_here && (groups.is_empty() || (self.publishes_due && !has_base)) {
            self.publishes_due = false;
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.run_group(self.default_context, None, Vec::new(), turn)
            }));
            if outcome.is_err() {
                eprintln!("dartq-daemon: base analysis panicked; the publish diff did not drain");
                self.restore_base_view();
            }
        }

        let mut poisoned = false;
        for (agent, requests) in groups {
            let ids: Vec<RequestId> = requests.iter().map(|(id, _)| id.clone()).collect();
            if poisoned {
                self.answer_panicked(turn, ids);
                continue;
            }
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.run_group(pending.context, agent.as_deref(), requests, turn)
            }));
            if let Err(payload) = outcome {
                // The engine's state is untrustworthy after a panic: the
                // remaining groups answer without analyzing, and the best-
                // effort restore below (itself contained) hands the next
                // turn the cleanest base view achievable.
                poisoned = true;
                eprintln!(
                    "dartq-daemon: analysis panicked ({}); answering the group internal error",
                    panic_message(&payload)
                );
                self.answer_panicked(turn, ids);
                self.restore_base_view();
            }
        }
    }

    /// Answers ids `INTERNAL_ERROR` after a panicking turn (the catch
    /// boundary's degraded answer; finding 5).
    fn answer_panicked(&mut self, turn: &mut Turn, ids: Vec<RequestId>) {
        for id in ids {
            turn.answers.push((
                id,
                Err(SessionError::Internal(
                    "internal error: analysis panicked".into(),
                )),
            ));
        }
    }

    /// One view group's analysis and answers: `agent = None` is the base
    /// view (on the default context its analysis also drains the publish
    /// diff); `Some(agent)` wears that agent's overlay onto `context`'s
    /// engine before analyzing and never publishes.
    fn run_group(
        &mut self,
        context: ContextId,
        agent: Option<&str>,
        requests: Vec<(RequestId, Request)>,
        turn: &mut Turn,
    ) {
        #[cfg(test)]
        if crate::probe::take_read_panic() {
            panic!("test-armed read panic");
        }
        let mut overlay_paths: Vec<PathBuf> = Vec::new();
        if let Some(agent) = agent {
            if !self.has_agent(agent) {
                // Cleared between intake and the drain: a cleared view is an
                // unknown one — fail loud instead of silently answering base.
                for (id, _) in requests {
                    turn.answers.push((
                        id,
                        Err(SessionError::InvalidParams(format!(
                            "unknown agentId: {agent}"
                        ))),
                    ));
                }
                return;
            }
            self.wear(context, agent);
            overlay_paths = self.agents[agent].files.keys().cloned().collect();
        } else {
            // Structural guard: the base group analyzes the base-resident
            // engine (a no-op whenever nothing is worn — the normal shape,
            // since base groups sort first).
            self.restore_base_view();
        }

        let mut diagnostics = match self.analyze(context) {
            Ok(diagnostics) => diagnostics,
            Err(Cancelled) => {
                for (id, _) in requests {
                    turn.answers.push((id, Err(SessionError::Cancelled)));
                }
                return;
            }
        };
        // The lint stage (#355): the resident files' rows, one transient
        // parse per file — joined before the severity-first sort so the
        // answers keep the CLI analyze face's order.
        let lint_roots = [self.workspace_root().to_path_buf()];
        diagnostics.extend(dartq_commands::lints::analyze_lints(
            self.engine(context),
            &lint_roots,
        ));
        // The plugin weave (#471 phase B), on the publish-bearing group
        // only: the plugins feed once per base analysis — the editor's
        // view — never per agent view or forked context. The cycle runs
        // before the convert-once pass so a crash row converts with the
        // engine rows; findings resolve against the same analyzed view and
        // merge into the publish plan below (engine rows first, plugin
        // groups by discovery order, dedup on the four-key). A crash whose
        // anchor does not exist yet — the session has published nothing —
        // is withheld by the cycle, not dropped: it publishes on the first
        // cycle that has one.
        let mut plugin_findings = Vec::new();
        if context == self.default_context && agent.is_none() && !self.plugins.is_empty() {
            let anchor: Option<FileId> = self.published.keys().copied().min();
            // Direct field access (not `self.engine`): the plugins' mutable
            // borrow and the engine's read must stay disjoint field borrows.
            let engine = self
                .runtime
                .context_state(context)
                .expect("the read task's context lives for the session");
            let cycle = self
                .plugins
                .cycle(&diagnostics, engine.file_table(), anchor);
            diagnostics.extend(cycle.crashes);
            plugin_findings =
                plugins::resolve_findings(&cycle.findings, engine.file_table(), |file| {
                    let index = engine.line_index(file)?;
                    let text = engine.content(file)?;
                    Some((
                        index,
                        std::str::from_utf8(text).expect("engine content is valid UTF-8"),
                    ))
                });
        }
        // The one conversion pass per group (the engine→LSP shape change is
        // the expensive half — #347's convert-once discipline, per view):
        // the publish plan and the analyze entries share it.
        let converted: Vec<Option<lsp::Diagnostic>> = diagnostics
            .iter()
            .map(|diagnostic| self.convert(context, diagnostic))
            .collect();
        if context == self.default_context && agent.is_none() {
            self.publishes_due = false;
            let engine_rows =
                diagnostics
                    .iter()
                    .zip(converted.iter())
                    .filter_map(|(diagnostic, converted)| {
                        converted.clone().map(|row| (diagnostic.file, row))
                    });
            turn.publishes
                .extend(self.plan_publishes(plugins::merge(engine_rows, plugin_findings)));
        }
        // dartq/analyze answers keep the CLI analyze face's printed order
        // (severity-first before writing; the shared key lives in
        // dartq-commands' layout module): severity, path bytes, offset,
        // message — not the engine's canonical (file, offset) order the
        // other faces mirror. The entries pair with their engine
        // diagnostics for the sort key, then strip.
        let mut paired: Vec<(&Diagnostic, &Path, lsp::AnalyzeEntry)> = diagnostics
            .iter()
            .zip(converted)
            .filter_map(|(diagnostic, converted)| {
                let converted = converted?;
                let path = self.engine(context).file_table().path(diagnostic.file)?;
                Some((
                    diagnostic,
                    path,
                    lsp::AnalyzeEntry {
                        uri: uri::from_path(path),
                        diagnostic: converted,
                    },
                ))
            })
            .collect();
        paired.sort_by(|(a, a_path, _), (b, b_path, _)| {
            severity_rank(a.severity)
                .cmp(&severity_rank(b.severity))
                .then_with(|| path_key(a_path).cmp(path_key(b_path)))
                .then_with(|| a.range.offset.cmp(&b.range.offset))
                .then_with(|| a.message.cmp(&b.message))
        });
        let entries: Vec<lsp::AnalyzeEntry> =
            paired.into_iter().map(|(_, _, entry)| entry).collect();
        let agent_changed: Option<Vec<(PathBuf, u64)>> = agent.map(|agent| {
            self.agents[agent]
                .files
                .iter()
                .map(|(path, file)| (path.clone(), file.set_at))
                .collect()
        });
        let changes = commands::Changes::new(&self.changed, agent_changed);
        // Disjoint field borrows: runtime (read), workspace_root/changed
        // (read), symbols (read — the table is mutex-guarded; the full-tier
        // writers answer on this thread inside the fan-out).
        let engine = self
            .runtime
            .context_state(context)
            .expect("the read task's context lives for the session");
        let drain = commands::Drain {
            engine,
            root: &self.workspace_root,
            sdk_root: &self.sdk_root,
            diagnostics: &diagnostics,
            entries: &entries,
            changes: &changes,
            cursor: self.cursor,
            overlay_paths: &overlay_paths,
            symbols: &self.symbols,
        };
        Self::answer_requests(&drain, requests, &mut turn.answers);
    }

    /// One view group's request answering: the fan-out that makes reads
    /// parallel (#487). Every request answers from the same analyzed view —
    /// order-independent by construction — so the group's requests drain
    /// through a shared job queue across worker threads borrowing the view
    /// (`&Engine` and friends; the compile-time `Sync` assertion pins the
    /// contract). Full-verbosity faces write the short-id table, so they
    /// run on the session thread, in request order — id assignment stays
    /// drain-order deterministic. Answers reassemble into the group's
    /// request order regardless of completion order (the turn's answer
    /// sequence is a pinned shape). A single request (the common editor
    /// poll) skips the threads entirely; a panicking answer is contained
    /// per request (the shared view is `&`-read — a query panic cannot
    /// corrupt engine state, unlike the exclusive analysis phase, whose
    /// containment stays at the group boundary).
    fn answer_requests(
        drain: &commands::Drain<'_>,
        requests: Vec<(RequestId, Request)>,
        answers: &mut Vec<(RequestId, Result<serde_json::Value, SessionError>)>,
    ) {
        let mut slots: Vec<Option<(RequestId, Result<serde_json::Value, SessionError>)>> =
            (0..requests.len()).map(|_| None).collect();
        let mut parallel: Vec<(usize, RequestId, Request)> = Vec::with_capacity(slots.len());
        for (index, (id, request)) in requests.into_iter().enumerate() {
            if request.verbosity() == Verbosity::Full {
                slots[index] = Some((id, commands::answer(drain, &request)));
            } else {
                parallel.push((index, id, request));
            }
        }
        // Dynamic work distribution: every worker drains the same queue
        // (query costs vary; static chunking would let one heavy face
        // dominate a chunk), and the session thread participates — at most
        // `workers - 1` threads spawn per group.
        if parallel.len() >= 2 {
            let workers = parallel.len().min(MAX_ANSWER_WORKERS);
            let (sender, receiver) = unbounded::<(usize, RequestId, Request)>();
            for job in parallel {
                sender
                    .send(job)
                    .expect("the job receiver outlives the sends below");
            }
            drop(sender);
            let mut results = thread::scope(|scope| {
                let handles: Vec<_> = (1..workers)
                    .map(|_| {
                        let receiver = receiver.clone();
                        scope.spawn(move || Self::drain_jobs(&receiver, drain))
                    })
                    .collect();
                let mut results = Self::drain_jobs(&receiver, drain);
                for handle in handles {
                    results.extend(
                        handle
                            .join()
                            .expect("an answer worker never panics outward"),
                    );
                }
                results
            });
            for (index, id, answer) in results {
                slots[index] = Some((id, answer));
            }
        } else {
            for (index, id, request) in parallel {
                slots[index] = Some((id, commands::answer(drain, &request)));
            }
        }
        answers.extend(slots.into_iter().flatten());
    }

    /// One answer worker: pulls jobs until the phase's queue empties, each
    /// answer panic-contained (a failed query answers `INTERNAL_ERROR` and
    /// the worker moves on — the shared view cannot have been corrupted by
    /// a `&`-read).
    fn drain_jobs(
        receiver: &Receiver<(usize, RequestId, Request)>,
        drain: &commands::Drain<'_>,
    ) -> Vec<(usize, RequestId, Result<serde_json::Value, SessionError>)> {
        let mut results = Vec::new();
        for (index, id, request) in receiver.iter() {
            let answer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                commands::answer(drain, &request)
            }))
            .unwrap_or_else(|payload| {
                eprintln!(
                    "dartq-daemon: answer worker panicked ({}); answering the request internal \
                     error",
                    panic_message(&payload)
                );
                Err(SessionError::Internal(
                    "internal error: analysis panicked".into(),
                ))
            });
            results.push((index, id, answer));
        }
        results
    }

    /// Restores the worn context's base view after agent wear: sets every
    /// worn path's base content back (or removes an agent-introduced path)
    /// and applies the barrier, so captures and publishes read a
    /// base-resident engine. The cascade is invalidation only — the next
    /// analysis recomputes.
    fn restore_base_view(&mut self) {
        let Some((context, worn)) = self.worn.take() else {
            return;
        };
        let engine = self.engine_mut(context);
        for (path, base) in &worn {
            match base {
                WornBase::Content(bytes) => {
                    engine.set_content(path, bytes);
                }
                // Reachable only on the post-panic unwind path (a panic
                // between the wear's set_content and the intern completing);
                // containment, not silence: the normal path cannot get here.
                WornBase::Absent => {
                    let _ = engine.remove_file(path);
                }
            }
        }
        engine.apply_pending(&mut |_| {});
    }

    /// Wears one agent's overlay onto `context`'s engine: restores whatever
    /// a previous group of this turn left worn (the barrier makes that
    /// context's engine base-resident again — captures below always read
    /// true base bytes, never a previous agent's content), then captures
    /// each overlaid path's base bytes and sets the agent's content. The
    /// pending ops apply at the group's analyze.
    fn wear(&mut self, context: ContextId, agent: &str) {
        self.restore_base_view();
        let Some(overlay) = self.agents.get(agent) else {
            return;
        };
        let mut worn: Vec<(PathBuf, WornBase)> = Vec::with_capacity(overlay.files.len());
        // Disjoint field borrows: agents (read) and runtime (mutate).
        let engine = self
            .runtime
            .context_state_mut(context)
            .expect("session contexts live for the session");
        // The captures below must read resident base bytes, so anything
        // still pending lands first. The scheduler drains writes before
        // reads and `apply_overlay` applies at its task, so the queue is
        // empty here in normal flow — the barrier keeps the capture site
        // correct on its own (an empty queue is a cheap no-op), and a base
        // edit that legitimately landed underneath can never be flushed
        // out by the restore.
        engine.apply_pending(&mut |_| {});
        for (path, file) in &overlay.files {
            // Capture before this path's own set_content; earlier paths'
            // ops only pend, so the resident content is still the base.
            let base = match engine
                .file_table()
                .try_file_id(path)
                .and_then(|file| engine.content(file))
            {
                Some(bytes) => WornBase::Content(bytes.to_vec()),
                None => WornBase::Absent,
            };
            worn.push((path.clone(), base));
            engine.set_content(path, &file.bytes);
        }
        self.worn = Some((context, worn));
    }

    /// Sweeps the runtime's shared overlay store once the garbage since the
    /// last sweep crossed the threshold (daemon-audit finding 2: without a
    /// driver the store accumulates every distinct edit state for the
    /// session's life). O(live), so idle sessions pay nothing.
    fn gc_if_due(&mut self) {
        if self
            .runtime
            .stored_contents()
            .saturating_sub(self.gc_watermark)
            >= GC_CONTENTS_THRESHOLD
        {
            self.runtime.sweep();
            self.gc_watermark = self.runtime.stored_contents();
        }
    }

    /// Stages one overlay mutation on the queued write task, last-wins per
    /// path: a batch's earlier ops for the same path are unobservable (ops
    /// on distinct paths commute, and the overlay declares the end state),
    /// so K batched edits of one file stage one text, not K full copies.
    fn stage(&mut self, change: OverlayChange) {
        let path = match &change {
            OverlayChange::Set { path, .. } | OverlayChange::Remove { path } => path,
        };
        self.staged.retain(|staged| match staged {
            OverlayChange::Set { path: old, .. } | OverlayChange::Remove { path: old } => {
                old != path
            }
        });
        self.staged.push(change);
    }

    /// Every daemon write is `Normal` (see the module's tier mapping): a
    /// write's tier only competes against other writes, and applying input
    /// is never the request itself — the read it enables is. The write task
    /// rides the default context's queue; its apply fans out at the drain.
    fn ensure_write(&mut self) {
        if self.write.is_none() {
            let cookie = self.next_cookie();
            self.runtime
                .enqueue(self.default_context, true, Tier::Normal, cookie)
                .expect("the default context lives for the session");
            self.write = Some(cookie);
        }
    }

    /// Queues the read task for `context` at `tier` when the context has
    /// none queued (the tier mapping lives in the module docs; callers pass
    /// it per intake path).
    fn ensure_read(&mut self, context: ContextId, tier: Tier) {
        if self.reads.iter().any(|pending| pending.context == context) {
            return;
        }
        let cookie = self.next_cookie();
        self.runtime
            .enqueue(context, false, tier, cookie)
            .expect("session contexts live for the session");
        self.reads.push(PendingRead {
            context,
            cookie,
            requests: Vec::new(),
            tier,
        });
    }

    fn next_cookie(&mut self) -> u64 {
        self.next_cookie += 1;
        self.next_cookie
    }

    /// A session context's engine: analyze, queries, content reads.
    fn engine(&self, context: ContextId) -> &Engine {
        self.runtime
            .context_state(context)
            .expect("session contexts live for the session")
    }

    fn engine_mut(&mut self, context: ContextId) -> &mut Engine {
        self.runtime
            .context_state_mut(context)
            .expect("session contexts live for the session")
    }

    /// Runs the engine barrier + recompute on one context: `analyze(&[])`
    /// applies the overlay and returns the merged diagnostic set of every
    /// live library, canonically sorted.
    fn analyze(&mut self, context: ContextId) -> Result<Vec<Diagnostic>, Cancelled> {
        self.engine_mut(context)
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
    }

    /// Diffs the fresh row set against the published baseline and returns
    /// one [`Publish`] per changed file (empty set included — the client
    /// clears its squiggles through it). `merged` is the view's full row
    /// set — engine rows plus the cycle's plugin findings, already merged
    /// and deduplicated ([`plugins::merge`]). Order is by URI, so the push
    /// sequence is deterministic.
    fn plan_publishes(&mut self, merged: Vec<(FileId, lsp::Diagnostic)>) -> Vec<Publish> {
        let mut current: FxHashMap<dartq_diagnostics::FileId, Vec<lsp::Diagnostic>> =
            FxHashMap::default();
        for (file, diagnostic) in merged {
            current.entry(file).or_default().push(diagnostic);
        }
        // Reconsider every diagnosed file plus every baseline file whose
        // diagnostics vanished entirely.
        let mut files: Vec<dartq_diagnostics::FileId> = current.keys().copied().collect();
        files.extend(
            self.published
                .iter()
                .filter(|(file, previous)| !previous.is_empty() && !current.contains_key(*file))
                .map(|(file, _)| *file),
        );

        let mut publishes: Vec<Publish> = Vec::new();
        for file in files {
            // Duplicated ids only when the same file appears in both sets —
            // the second pass then sees an empty `current` and no-ops.
            let now = match current.entry(file) {
                Entry::Occupied(occupied) => occupied.remove(),
                Entry::Vacant(_) => Vec::new(),
            };
            if self
                .published
                .get(&file)
                .is_some_and(|before| *before == now)
            {
                continue;
            }
            let Some(path) = self.engine(self.default_context).file_table().path(file) else {
                continue;
            };
            let path = path.to_path_buf();
            self.published.insert(file, now.clone());
            publishes.push(Publish {
                uri: uri::from_path(&path),
                version: self.opened.get(&path).copied(),
                diagnostics: now,
            });
        }
        publishes.sort_by(|a, b| a.uri.cmp(&b.uri));
        publishes
    }

    /// Converts one engine diagnostic to the LSP shape, reading the
    /// analyzed context's line index and resident text. Both are borrowed,
    /// never cloned — a per-diagnostic whole-file copy would make a read
    /// task O(D × file bytes). Returns `None` only for a non-resident file,
    /// which the engine's "live libraries only" contract excludes.
    fn convert(&self, context: ContextId, diagnostic: &Diagnostic) -> Option<lsp::Diagnostic> {
        #[cfg(test)]
        crate::probe::count_conversion();
        let index = self.engine(context).line_index(diagnostic.file)?;
        let bytes = self.engine(context).content(diagnostic.file)?;
        let text = std::str::from_utf8(bytes).expect("engine content is valid UTF-8");
        // `line_col_utf16` answers the oracle's 1-based (line, column); LSP
        // positions are zero-based.
        let end_offset = diagnostic
            .range
            .offset
            .checked_add(diagnostic.range.length)
            .expect("diagnostic range overflows u32");
        let (start_line, start_column) = index.line_col_utf16(text, diagnostic.range.offset);
        let (end_line, end_column) = index.line_col_utf16(text, end_offset);
        let severity = match diagnostic.severity {
            Severity::Error => 1,
            Severity::Warning => 2,
            Severity::Info => 3,
        };
        Some(lsp::Diagnostic {
            range: lsp::Range {
                start: lsp::Position {
                    line: start_line - 1,
                    character: start_column - 1,
                },
                end: lsp::Position {
                    line: end_line - 1,
                    character: end_column - 1,
                },
            },
            severity,
            code: diagnostic.code.name.to_owned(),
            source: "dartq".into(),
            message: diagnostic.message.clone(),
        })
    }
}

/// The message of a caught panic payload (for the containment log line).
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("opaque panic payload")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// N applies inside the debounce window collapse into one save trigger;
    /// a second check after the flush does not re-trigger.
    #[test]
    fn debounce_collapses_applies_into_one_flush() {
        let mut sink = SnapshotSink {
            store: None,
            dirty: false,
            last_apply: None,
        };
        let applied = Instant::now();

        // Three applies across three turns, all inside the window; pin the
        // last apply to the test's base instant so the window math is exact
        // (note_apply stamps the real clock).
        sink.note_apply();
        sink.note_apply();
        sink.note_apply();
        sink.last_apply = Some(applied);
        assert!(sink.dirty);
        assert!(
            !sink.flush_if_due(applied + Duration::from_millis(499)),
            "inside the window nothing saves"
        );
        assert!(sink.dirty, "an unflushed sink stays dirty");

        // Past the window: exactly one save trigger, and the sink is clean.
        assert!(sink.flush_if_due(applied + SNAPSHOT_DEBOUNCE));
        assert!(!sink.dirty);
        assert!(
            !sink.flush_if_due(applied + Duration::from_secs(1)),
            "a clean sink never re-triggers"
        );
    }

    /// The end-of-session flush saves a dirty sink regardless of the window.
    #[test]
    fn flush_saves_dirty_sink_regardless_of_age() {
        let mut sink = SnapshotSink {
            store: None,
            dirty: false,
            last_apply: None,
        };
        sink.note_apply();
        // A fresh apply (0 ms old) still flushes at session end.
        sink.flush();
        assert!(!sink.dirty);
    }

    /// The deadline is the single source of truth for the window: clean
    /// sinks never bound the main loop's receive, dirty sinks do until the
    /// flush lands (#347: an idle daemon saves at the deadline instead of
    /// blocking forever with a pending save).
    #[test]
    fn flush_deadline_tracks_the_dirty_window() {
        let mut sink = SnapshotSink {
            store: None,
            dirty: false,
            last_apply: None,
        };
        assert!(
            sink.deadline().is_none(),
            "a clean sink never bounds the receive"
        );

        let applied = Instant::now();
        sink.note_apply();
        let deadline = sink.deadline().expect("a dirty sink bounds the receive");
        assert!(deadline > applied, "the deadline lies past the apply");
        assert!(
            deadline <= applied + SNAPSHOT_DEBOUNCE + Duration::from_millis(50),
            "the deadline is the apply instant plus the window"
        );

        sink.flush();
        assert!(
            sink.deadline().is_none(),
            "flushed: the receive blocks freely again"
        );
    }

    use crate::commands::Request;
    use crate::test_fixtures::{BROKEN_MAIN, CLEAN_MAIN, Workspace, store_root};
    use dartq_commands::Verbosity;
    use dartq_snapshot::DEFAULT_CAPACITY_BYTES;

    /// A session over a throwaway workspace with a throwaway snapshot root.
    fn fixture(name: &str) -> (Workspace, Session) {
        fixture_with(name, BROKEN_MAIN)
    }

    /// [`fixture`] over custom on-disk `lib/main.dart` content.
    fn fixture_with(name: &str, main_dart: &str) -> (Workspace, Session) {
        let workspace = Workspace::new(main_dart);
        let store = SnapshotStore::open(SnapshotConfig {
            root: Some(store_root(name)),
            capacity_bytes: DEFAULT_CAPACITY_BYTES,
        })
        .expect("store opens on a temp root");
        let session = Session::with_store(workspace.engine_config(), store).expect("session");
        (workspace, session)
    }

    /// Writer priority (PRD §16.5), deterministically: two `dartq/analyze`
    /// requests bracketing an overlay edit drain in one turn, and the
    /// request enqueued BEFORE the edit still answers with the post-write
    /// view — every write executes before any read.
    #[test]
    fn writer_priority_drains_writes_before_reads() {
        let (workspace, mut session) = fixture("writer-priority-unit");
        let uri = uri::from_path(&workspace.path("lib/main.dart"));

        session.sweep().expect("sweep");
        let turn = session.service();
        assert_eq!(
            turn.publishes.len(),
            1,
            "the sweep publishes the seeded error"
        );

        // One batch: analyze, edit, analyze — one drain serves all three.
        session.queue_command(
            RequestId::Num(2),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        assert!(session.queue_edit(&uri, CLEAN_MAIN, 2));
        session.queue_command(
            RequestId::Num(3),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let turn = session.service();

        assert_eq!(turn.publishes.len(), 1, "the write's clear push");
        assert_eq!(turn.publishes[0].version, Some(2));
        assert!(turn.publishes[0].diagnostics.is_empty());
        assert_eq!(
            turn.answers.iter().map(|(id, _)| id).collect::<Vec<_>>(),
            [&RequestId::Num(2), &RequestId::Num(3)]
        );
        for (_, answer) in &turn.answers {
            let entries = answer.as_ref().expect("both requests answer")["diagnostics"]
                .as_array()
                .expect("diagnostics array");
            assert!(entries.is_empty(), "post-write view: {entries:?}");
        }
    }

    /// The read-parallel drain (#487): a batch of coalesced requests
    /// answers in queue order regardless of the worker fan-out, every
    /// worker sees the same analyzed view (equal rows), and the full-tier
    /// request — the short-id table's only writer — still shortens in its
    /// queued position (the serial partition), preamble attached.
    #[test]
    fn coalesced_requests_answer_in_order_across_the_fan_out() {
        let (_workspace, mut session) = fixture_with("fanout-order-unit", SYNTAX_BROKEN);

        session.sweep().expect("sweep");
        let _ = session.service();

        // Seven coalesced requests in one turn: the editor-poll mix
        // (compact) around one full-tier analyze (the id-assigning writer;
        // the view's two same-path rows make the preamble pay).
        for id in 2..=7 {
            session.queue_command(
                RequestId::Num(id),
                Request::Analyze {
                    verbosity: Verbosity::Compact,
                    agent: None,
                    context: None,
                },
            );
        }
        session.queue_command(
            RequestId::Num(8),
            Request::Analyze {
                verbosity: Verbosity::Full,
                agent: None,
                context: None,
            },
        );
        let turn = session.service();

        assert_eq!(
            turn.answers
                .iter()
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>(),
            (2..=8).map(RequestId::Num).collect::<Vec<_>>(),
            "queue order, fan-out or not: {:?}",
            turn.answers.iter().map(|(id, _)| id).collect::<Vec<_>>()
        );
        for (id, answer) in &turn.answers {
            let value = answer
                .as_ref()
                .unwrap_or_else(|error| panic!("request {id:?} answers: {error:?}"));
            let rows = value["diagnostics"].as_array().expect("rows");
            assert_eq!(rows.len(), 2, "request {id:?} sees the same view");
        }
        // The full-tier answer shortened: rows carry `s1` and the preamble
        // names the path; the compact answers stay unshortened.
        let full = turn
            .answers
            .last()
            .expect("the full answer")
            .1
            .as_ref()
            .expect("ok");
        assert_eq!(full["diagnostics"][0]["uri"], "s1", "shortened");
        assert!(
            full["symbols"]["s1"].is_string(),
            "the preamble names the path: {full}"
        );
        let compact = turn.answers[0].1.as_ref().expect("ok");
        assert!(
            compact["symbols"].is_null(),
            "compact answers stay unshortened: {compact}"
        );
        assert_ne!(
            compact["diagnostics"][0]["uri"], "s1",
            "the compact row keeps the full path"
        );
    }

    /// Epoch cancellation (PRD §16.6), deterministically: a request
    /// superseded by `$/cancelRequest` drops at the drain and answers
    /// cancelled with no result, while the overlay edit sharing the batch
    /// still lands (writes carry no ids) — and the publish the cancelled
    /// read was carrying is re-stamped onto a zero-request read, so the
    /// clear push reaches the client in the same turn even if it goes
    /// silent afterwards.
    #[test]
    fn cancel_drops_queued_reads_but_writes_and_publishes_survive() {
        let (workspace, mut session) = fixture("cancel-unit");
        let uri = uri::from_path(&workspace.path("lib/main.dart"));

        session.sweep().expect("sweep");
        let _ = session.service();

        // Request + edit + cancel in one batch, then silence.
        session.queue_command(
            RequestId::Num(2),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        assert!(session.queue_edit(&uri, CLEAN_MAIN, 2));
        session.cancel_request(&RequestId::Num(2));
        let turn = session.service();

        let [(id, answer)] = turn.answers.as_slice() else {
            panic!("one request, one answer: {:?}", turn.answers);
        };
        assert_eq!(*id, RequestId::Num(2));
        assert!(
            matches!(answer, Err(SessionError::Cancelled)),
            "the stale request answers cancelled: {answer:?}"
        );
        assert_eq!(
            turn.publishes.len(),
            1,
            "the write's clear push survives the cancel: {:?}",
            turn.publishes
        );
        assert_eq!(turn.publishes[0].version, Some(2));
        assert!(turn.publishes[0].diagnostics.is_empty());

        // The publish baseline caught up in that turn: a follow-up analysis
        // (had the client not stayed silent) pushes nothing new.
        session.queue_command(
            RequestId::Num(3),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let turn = session.service();
        assert!(
            turn.publishes.is_empty(),
            "no duplicate push: {:?}",
            turn.publishes
        );
        let entries = turn.answers[0].1.as_ref().expect("post-write answer")["diagnostics"]
            .as_array()
            .expect("diagnostics array");
        assert!(entries.is_empty(), "post-write view: {entries:?}");
    }

    /// Cancel partitions the coalesced read per id: the cancelled request
    /// answers cancelled, its co-rider survives on the re-stamped read and
    /// answers with the real analysis.
    #[test]
    fn cancel_partitions_the_coalesced_read() {
        let (_workspace, mut session) = fixture("cancel-partition-unit");

        session.sweep().expect("sweep");
        let _ = session.service();

        // Two analyses coalesce onto one read; cancelling one spares the
        // other.
        session.queue_command(
            RequestId::Num(2),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        session.queue_command(
            RequestId::Num(3),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        session.cancel_request(&RequestId::Num(2));
        let turn = session.service();

        assert!(
            turn.publishes.is_empty(),
            "nothing changed since the sweep: {:?}",
            turn.publishes
        );
        assert_eq!(
            turn.answers.iter().map(|(id, _)| id).collect::<Vec<_>>(),
            [&RequestId::Num(2), &RequestId::Num(3)],
            "cancelled answers first, survivor after"
        );
        assert!(
            matches!(&turn.answers[0].1, Err(SessionError::Cancelled)),
            "the cancelled id: {:?}",
            turn.answers[0]
        );
        let entries = turn.answers[1].1.as_ref().expect("survivor answers Ok")["diagnostics"]
            .as_array()
            .expect("diagnostics array");
        assert!(
            entries
                .iter()
                .any(|entry| entry["code"] == "uri_does_not_exist"),
            "the survivor got the real analysis: {entries:?}"
        );
    }

    /// Cancelling an unknown id, or one that already answered, is silently
    /// ignored (LSP §cancelRequest): no cancelled answer, no state change.
    #[test]
    fn cancel_of_unknown_or_answered_id_is_ignored() {
        let (_workspace, mut session) = fixture("cancel-ignore-unit");

        session.sweep().expect("sweep");
        let _ = session.service();

        // Unknown id while a read is queued: the queued request still
        // answers normally.
        session.queue_command(
            RequestId::Num(2),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        session.cancel_request(&RequestId::Num(99));
        let turn = session.service();
        assert_eq!(
            turn.answers.len(),
            1,
            "only the queued request: {:?}",
            turn.answers
        );
        assert!(turn.answers[0].1.is_ok(), "the cancel never landed");

        // An id that already answered: the read drained; the cancel is a
        // no-op and the next analysis still answers.
        session.cancel_request(&RequestId::Num(2));
        session.queue_command(
            RequestId::Num(3),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let turn = session.service();
        assert_eq!(
            turn.answers.len(),
            1,
            "only the new request: {:?}",
            turn.answers
        );
        assert_eq!(turn.answers[0].0, RequestId::Num(3));
        assert!(turn.answers[0].1.is_ok());
    }

    /// Staged edits merge last-wins per path (#347): three batched
    /// `didChange` frames for one file stage ONE whole-text op, not three —
    /// and the batch publishes exactly what a single edit would (the
    /// terminal state is order-independent for distinct paths, so the
    /// earlier same-path ops are unobservable).
    #[test]
    fn staged_edits_of_one_file_merge_last_wins() {
        let (workspace, mut session) = fixture("staged-merge-unit");
        let uri = uri::from_path(&workspace.path("lib/main.dart"));

        session.sweep().expect("sweep");
        let _ = session.service();

        // Three didChange frames land inside one batch window: three
        // queue_edit calls before the drain.
        assert!(session.queue_edit(&uri, "}\n", 2));
        assert!(session.queue_edit(&uri, ")\n", 3));
        assert!(session.queue_edit(&uri, CLEAN_MAIN, 4));
        assert_eq!(
            session.staged.len(),
            1,
            "same-path ops converge to the last text"
        );

        let turn = session.service();
        assert_eq!(
            turn.publishes.len(),
            1,
            "one batch, one push — as if a single clean edit arrived: {:?}",
            turn.publishes
        );
        assert_eq!(turn.publishes[0].version, Some(4));
        assert!(turn.publishes[0].diagnostics.is_empty());

        // The survivor is the LAST text: an earlier broken one winning
        // would surface syntax diagnostics here.
        session.queue_command(
            RequestId::Num(2),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let turn = session.service();
        let entries = turn.answers[0].1.as_ref().expect("post-batch answer")["diagnostics"]
            .as_array()
            .expect("diagnostics array");
        assert!(
            entries.is_empty(),
            "the last edit is the applied one: {entries:?}"
        );
    }

    /// Watch intake (`--watch`) is the didChange-equivalent: an on-disk
    /// rewrite synced through [`Session::queue_disk_changes`] re-analyzes
    /// and publishes the clean set — with **no version** (a watch file is
    /// not an LSP-opened document) — and a vanished file stages a removal,
    /// so the file drops out of the view and its diagnostics vanish
    /// (absence semantics, as with a closed overlay-only buffer; the
    /// analysis's content check stays the §16.7 truth). Nothing staged
    /// (every path unreadable) enqueues nothing.
    #[test]
    fn watch_disk_changes_sync_analyze_and_remove() {
        let (workspace, mut session) = fixture("watch-sync-unit");
        let main = workspace.path("lib/main.dart");

        session.sweep().expect("sweep");
        let turn = session.service();
        assert_eq!(turn.publishes.len(), 1, "the seeded error publishes");

        // A disk rewrite the client never pushed: analyzed Normal-tier, no
        // version on the push.
        fs::write(&main, CLEAN_MAIN).expect("rewrite on disk");
        session.queue_disk_changes(&[main.clone()]);
        let turn = session.service();
        assert_eq!(turn.publishes.len(), 1, "the clear push");
        assert_eq!(
            turn.publishes[0].version, None,
            "watch files are not LSP-opened"
        );
        assert!(turn.publishes[0].diagnostics.is_empty());

        // A file deleted since the event: the overlay drops it, the file's
        // diagnostics vanish, and — the baseline already being empty — no
        // push goes out.
        fs::remove_file(&main).expect("delete on disk");
        session.queue_disk_changes(&[main.clone()]);
        let turn = session.service();
        assert!(
            turn.publishes.is_empty(),
            "an absent file has nothing to push: {:?}",
            turn.publishes
        );
        // The removal really landed: a fresh request answers without the
        // file's diagnostics.
        session.queue_command(
            RequestId::Num(2),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let turn = session.service();
        let entries = turn.answers[0].1.as_ref().expect("post-removal answer")["diagnostics"]
            .as_array()
            .expect("diagnostics array");
        assert!(
            entries.iter().all(|entry| !entry["uri"]
                .as_str()
                .is_some_and(|file| file.ends_with("main.dart"))),
            "the deleted file has no diagnostics: {entries:?}"
        );

        // An unreadable path stages nothing: no write, no read, no turn
        // work (the next analysis still answers).
        session.queue_disk_changes(&[workspace.path("lib/nope-beyond-workspace.rs")]);
        session.queue_command(
            RequestId::Num(3),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let turn = session.service();
        assert!(
            turn.publishes.is_empty(),
            "nothing new to publish: {:?}",
            turn.publishes
        );
        assert_eq!(turn.answers.len(), 1, "only the request ran");
    }

    /// Re-discovery, additions: a file created after `initialized` and
    /// synced through [`Session::queue_disk_changes`] enters the analysis —
    /// the sync stages the same set the sweep's discovery takes, so the new
    /// file's own diagnostics surface (publish and analyze entries). The
    /// disk view is not frozen at the sweep.
    #[test]
    fn watch_sync_brings_new_files_into_analysis() {
        let (workspace, mut session) = fixture("watch-new-file-unit");

        session.sweep().expect("sweep");
        let _ = session.service();

        let extra = workspace.path("lib/extra.dart");
        fs::write(&extra, "import 'also_missing.dart';\nvoid main() {}\n").expect("create");
        session.queue_disk_changes(&[extra.clone()]);
        let turn = session.service();
        assert_eq!(
            turn.publishes.len(),
            1,
            "only the new file pushes: {:?}",
            turn.publishes
        );
        assert_eq!(turn.publishes[0].uri, uri::from_path(&extra));
        assert!(
            !turn.publishes[0].diagnostics.is_empty(),
            "the new file was analyzed, not merely interned"
        );

        // The file is resident: a fresh analyze answers its rows too.
        session.queue_command(
            RequestId::Num(2),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let turn = session.service();
        let entries = turn.answers[0].1.as_ref().expect("analyze answers")["diagnostics"]
            .as_array()
            .expect("diagnostics array");
        assert!(
            entries.iter().any(|entry| entry["uri"]
                .as_str()
                .is_some_and(|file| file.ends_with("extra.dart"))),
            "the new file's rows are in the merged view: {entries:?}"
        );
    }

    /// Re-discovery under a wholesale change (the git branch-switch shape):
    /// two files swap contents between two ticks, and one sync batch
    /// re-analyzes both — each file's published set is its new content's,
    /// never the swept stale one.
    #[test]
    fn watch_sync_reanalyzes_a_branch_switch_swap() {
        let (workspace, mut session) = fixture_with("watch-swap-unit", CLEAN_MAIN);
        let main = workspace.path("lib/main.dart");
        let other = workspace.path("lib/other.dart");
        fs::write(&other, BROKEN_MAIN).expect("seed other");

        session.sweep().expect("sweep");
        let turn = session.service();
        assert_eq!(
            turn.publishes.len(),
            1,
            "only the seeded error pushes: {:?}",
            turn.publishes
        );

        // The swap: both contents move in one event burst (the watcher's
        // debounce delivers both paths in one batch).
        fs::write(&main, BROKEN_MAIN).expect("main takes the broken content");
        fs::write(&other, CLEAN_MAIN).expect("other takes the clean content");
        session.queue_disk_changes(&[main.clone(), other.clone()]);
        let turn = session.service();

        assert_eq!(
            turn.publishes.len(),
            2,
            "both files re-analyzed: {:?}",
            turn.publishes
        );
        let by_uri = |path: &PathBuf| {
            let uri = uri::from_path(path);
            turn.publishes
                .iter()
                .find(|push| push.uri == uri)
                .unwrap_or_else(|| panic!("no push for {uri}"))
        };
        assert_eq!(
            by_uri(&main).diagnostics.len(),
            1,
            "main now carries the broken content's rows"
        );
        assert!(by_uri(&other).diagnostics.is_empty(), "other is clean now");
    }

    /// Overlay precedence (the rule [`Session::queue_disk_changes`]
    /// documents): a client-opened document owns its content until
    /// `didClose` — a disk rewrite under it never reaches the analysis —
    /// and an agent overlay masks the base regardless of disk traffic (the
    /// base keeps tracking the disk; the agent's view stays its own).
    #[test]
    fn watch_sync_never_clobbers_overlay_content() {
        let (workspace, mut session) = fixture("watch-overlay-precedence-unit");
        let main = workspace.path("lib/main.dart");
        let uri = crate::uri::from_path(&main);

        session.sweep().expect("sweep");
        let _ = session.service();

        // A client-opened document: the disk rewrite under the open buffer
        // is skipped entirely — no clobber push, and the base still holds
        // the client's text.
        assert!(session.queue_edit(&uri, CLEAN_MAIN, 1));
        let _ = session.service();
        fs::write(&main, BROKEN_MAIN).expect("disk rewrite under the open buffer");
        session.queue_disk_changes(&[main.clone()]);
        session.queue_command(
            RequestId::Num(2),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let turn = session.service();
        assert!(
            turn.publishes.is_empty(),
            "the disk rewrite did not clobber the open document: {:?}",
            turn.publishes
        );
        assert!(
            answer_codes(&turn.answers[0].1).is_empty(),
            "the base view still holds the client's text: {:?}",
            turn.answers[0]
        );

        // didClose re-syncs the disk state: the rewrite shows up now (a
        // write-only turn — the answers list is empty, the push is the
        // evidence).
        session.queue_close(&uri).expect("close");
        let turn = session.service();
        assert!(turn.answers.is_empty(), "no requests rode the close");
        assert_eq!(
            turn.publishes.len(),
            1,
            "the close surfaces the disk state: {:?}",
            turn.publishes
        );
        assert_eq!(
            turn.publishes[0].diagnostics[0].code, "uri_does_not_exist",
            "close staged the on-disk bytes: {:?}",
            turn.publishes[0]
        );

        // An agent overlay: the base tracks the disk through the sync (the
        // path is not client-opened), while the agent's view keeps its own
        // content — disk traffic never leaks into it.
        session
            .set_overlay("A", vec![(main.clone(), CLEAN_MAIN.as_bytes().to_vec())])
            .expect("stage A");
        fs::write(&main, SYNTAX_BROKEN).expect("another disk rewrite");
        session.queue_disk_changes(&[main.clone()]);
        let (id_a, a) = check_request(3, &[workspace.path("lib")], Some("A"), None);
        session.queue_command(id_a, a);
        session.queue_command(
            RequestId::Num(4),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let turn = session.service();
        let by_id = |wanted: i64| {
            &turn
                .answers
                .iter()
                .find(|(id, _)| *id == RequestId::Num(wanted))
                .expect("answer present")
                .1
        };
        assert!(
            answer_codes(by_id(4))
                .iter()
                .any(|code| code == "expected_token"),
            "the base view tracks the disk through the sync: {:?}",
            by_id(4)
        );
        assert!(
            answer_codes(by_id(3)).is_empty(),
            "agent A's view is untouched by the disk traffic: {:?}",
            by_id(3)
        );
    }

    /// The refresh sweep's open-document precedence (the guard
    /// [`Session::sweep`] shares with the sync path): discovery re-stages
    /// disk bytes for every resident path, and a config-triggered sweep
    /// must not write that text over a live open buffer — both the applied
    /// overlay and an edit still queued on the write task survive it.
    #[test]
    fn config_refresh_sweep_never_clobbers_an_open_document() {
        let (workspace, mut session) = fixture("sweep-open-guard-unit");
        let main = workspace.path("lib/main.dart");
        let uri = crate::uri::from_path(&main);

        session.sweep().expect("sweep");
        let _ = session.service();

        // An applied open buffer: the refresh re-stages disk bytes under
        // it, and the buffer's text must displace them (last-wins per
        // pending path) instead of the other way around.
        assert!(session.queue_edit(&uri, CLEAN_MAIN, 1));
        let _ = session.service();
        session.sweep().expect("refresh sweep");
        session.queue_command(
            RequestId::Num(2),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let turn = session.service();
        assert!(
            answer_codes(&turn.answers[0].1).is_empty(),
            "the open buffer survived the refresh sweep: {:?}",
            turn.answers[0]
        );

        // An edit still queued (no drain since queue_edit): the sweep's
        // re-stage must keep the STAGED bytes, not the stale resident ones.
        assert!(session.queue_edit(&uri, "void main() {} // v2\n", 2));
        session.sweep().expect("refresh sweep under a queued edit");
        session.queue_command(
            RequestId::Num(3),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let turn = session.service();
        assert!(
            answer_codes(&turn.answers[0].1).is_empty(),
            "the queued edit survived the refresh sweep: {:?}",
            turn.answers[0]
        );
    }

    /// The read task converts each diagnostic exactly once (#347): the
    /// publish plan and the analyze entries share one conversion pass (the
    /// pre-#347 shape converted twice), and conversion borrows the resident
    /// text — a per-diagnostic whole-file clone would register on the bulk
    /// allocation probe.
    #[test]
    fn read_turn_converts_once_and_never_clones_file_text() {
        // An 8 KiB broken file: the whole text is bulk-sized, so any
        // per-diagnostic copy would show up in the probe.
        let big_broken = format!("{BROKEN_MAIN}// {}\n", "x".repeat(8 * 1024));
        let (_workspace, mut session) = fixture_with("convert-probe-unit", &big_broken);

        session.sweep().expect("sweep");
        let _ = session.service();

        crate::probe::reset_counters();
        session.queue_command(
            RequestId::Num(2),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let turn = session.service();
        let entries = turn.answers[0].1.as_ref().expect("analyze answers")["diagnostics"]
            .as_array()
            .expect("diagnostics array");
        assert!(
            !entries.is_empty(),
            "the fixture must carry diagnostics for the probe"
        );

        let (conversions, _) = crate::probe::counters();
        assert_eq!(
            conversions,
            entries.len(),
            "one conversion per diagnostic — not one for the publish plan plus one for the \
             entries"
        );

        // Direct conversion allocates nothing bulk-sized: the line index
        // and the text are borrowed, never owned per diagnostic.
        let diagnostics = session
            .analyze(session.default_context)
            .expect("re-analysis");
        crate::probe::arm_bulk();
        for diagnostic in &diagnostics {
            assert!(
                session
                    .convert(session.default_context, diagnostic)
                    .is_some()
            );
        }
        assert_eq!(
            crate::probe::bulk_bytes(),
            0,
            "convert borrows the resident text; a whole-file clone would register"
        );
    }

    /// A `dartq/check` request rides the same read task and epoch machinery
    /// as `dartq/analyze`: a stale epoch aborts at the drain, before the
    /// command executes — the request answers cancelled with no result
    /// (PRD §15.1: every `dartq/*` face is cancellable).
    #[test]
    fn cancel_drops_a_queued_dartq_check_request() {
        let (workspace, mut session) = fixture("cancel-command-unit");

        session.sweep().expect("sweep");
        let _ = session.service();

        session.queue_command(
            RequestId::Num(2),
            Request::Check {
                paths: vec![workspace.path("lib")],
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
                since: None,
            },
        );
        session.cancel_request(&RequestId::Num(2));
        let turn = session.service();

        assert!(
            turn.publishes.is_empty(),
            "nothing changed since the sweep: {:?}",
            turn.publishes
        );
        let [(id, answer)] = turn.answers.as_slice() else {
            panic!("one request, one answer: {:?}", turn.answers);
        };
        assert_eq!(*id, RequestId::Num(2));
        assert!(
            matches!(answer, Err(SessionError::Cancelled)),
            "the stale dartq/check answers cancelled: {answer:?}"
        );
    }

    /// A `dartq/*` request whose parameters name no valid target answers
    /// `InvalidParams` (the CLI's usage tier over RPC), and the session
    /// stays healthy — a follow-up request answers normally.
    #[test]
    fn dartq_command_usage_failure_answers_invalid_params() {
        let (workspace, mut session) = fixture("command-usage-unit");

        session.sweep().expect("sweep");
        let _ = session.service();

        session.queue_command(
            RequestId::Num(2),
            Request::Check {
                paths: vec![workspace.path("lib/nope.dart")],
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
                since: None,
            },
        );
        let turn = session.service();
        let [(id, answer)] = turn.answers.as_slice() else {
            panic!("one request, one answer: {:?}", turn.answers);
        };
        assert_eq!(*id, RequestId::Num(2));
        assert!(
            matches!(answer, Err(SessionError::InvalidParams(_))),
            "a missing target is the usage tier: {answer:?}"
        );

        // The session is unharmed: the next command answers normally.
        session.queue_command(
            RequestId::Num(3),
            Request::Check {
                paths: vec![workspace.path("lib")],
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
                since: None,
            },
        );
        let turn = session.service();
        assert!(
            turn.answers[0].1.is_ok(),
            "the session answers after a usage-tier failure: {:?}",
            turn.answers
        );
    }

    // -----------------------------------------------------------------------
    // Agent views (PRD §16.3) + session economics (PRD §15.8)
    // -----------------------------------------------------------------------

    /// Agent B's overlay signature — a syntax error the base fixture never
    /// produces.
    const AGENT_B_MAIN: &str = "void b() { z }\n";
    /// A syntactically broken replacement (the M0 disk pipeline reports it).
    const SYNTAX_BROKEN: &str = "void f() { x }\n";

    /// The codes of a check/analyze answer's rows.
    fn answer_codes(answer: &Result<serde_json::Value, SessionError>) -> Vec<String> {
        match answer {
            Ok(value) => value["diagnostics"]
                .as_array()
                .expect("diagnostics array")
                .iter()
                .map(|row| row["code"].as_str().expect("code").to_owned())
                .collect(),
            Err(error) => panic!("expected diagnostics, got {error}"),
        }
    }

    /// A `dartq/check` request over `paths` (the disk pipeline discovers
    /// them; the agent view filters the engine set by them).
    fn check_request(
        id: i64,
        paths: &[std::path::PathBuf],
        agent: Option<&str>,
        since: Option<u64>,
    ) -> (RequestId, Request) {
        (
            RequestId::Num(id),
            Request::Check {
                paths: paths.to_vec(),
                verbosity: Verbosity::Compact,
                agent: agent.map(str::to_owned),
                context: None,
                since,
            },
        )
    }

    /// The acceptance posture, at session level: two concurrent agent
    /// overlays over one base produce isolated diagnostics, and the base
    /// view is untouched by both.
    #[test]
    fn agent_views_stay_isolated_over_one_base() {
        let (workspace, mut session) = fixture("agent-isolation-unit");
        let main = workspace.path("lib/main.dart");

        session.sweep().expect("sweep");
        let _ = session.service();
        session
            .set_overlay("A", vec![(main.clone(), CLEAN_MAIN.as_bytes().to_vec())])
            .expect("stage A");
        session
            .set_overlay("B", vec![(main, AGENT_B_MAIN.as_bytes().to_vec())])
            .expect("stage B");

        let (id_a, a) = check_request(2, &[workspace.path("lib")], Some("A"), None);
        let (id_b, b) = check_request(3, &[workspace.path("lib")], Some("B"), None);
        let (id_base, base) = check_request(4, &[workspace.path("lib")], None, None);
        session.queue_command(id_a, a);
        session.queue_command(id_b, b);
        session.queue_command(id_base, base);
        session.queue_command(
            RequestId::Num(5),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let turn = session.service();

        assert_eq!(turn.answers.len(), 4, "one answer per request");
        let by_id = |wanted: i64| {
            turn.answers
                .iter()
                .find(|(id, _)| *id == RequestId::Num(wanted))
                .map(|(_, answer)| answer)
                .expect("answer present")
        };
        assert!(
            answer_codes(by_id(2)).is_empty(),
            "agent A's view is clean: {:?}",
            by_id(2)
        );
        assert_eq!(
            answer_codes(by_id(3)),
            ["expected_token", "undefined_identifier"],
            "agent B sees only its own view's errors: {:?}",
            by_id(3)
        );
        assert_eq!(
            answer_codes(by_id(4)),
            ["uri_does_not_exist"],
            "base check reports the fixture's own missing import; both agent overlays stay invisible: {:?}",
            by_id(4)
        );
        assert_eq!(
            answer_codes(by_id(5)),
            ["uri_does_not_exist"],
            "the base engine view is immune to both overlays: {:?}",
            by_id(5)
        );
    }

    /// Clearing evicts the agent (fail loud on a late request) and the
    /// wear/restore cycle leaves the base view exactly as it was.
    #[test]
    fn overlay_clear_restores_the_base_view() {
        let (workspace, mut session) = fixture("agent-clear-unit");
        let main = workspace.path("lib/main.dart");

        session.sweep().expect("sweep");
        let _ = session.service();
        session
            .set_overlay("A", vec![(main, CLEAN_MAIN.as_bytes().to_vec())])
            .expect("stage");

        session.queue_command(
            RequestId::Num(2),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: Some("A".into()),
                context: None,
            },
        );
        let turn = session.service();
        assert!(answer_codes(&turn.answers[0].1).is_empty(), "A is clean");

        session.clear_overlay("A");
        session.queue_command(
            RequestId::Num(3),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: Some("A".into()),
                context: None,
            },
        );
        let turn = session.service();
        assert!(
            matches!(&turn.answers[0].1, Err(SessionError::InvalidParams(message))
                if message.contains("A")),
            "a cleared agent answers INVALID_PARAMS: {:?}",
            turn.answers[0]
        );

        // The base view survived the wear/restore cycle byte-for-byte: the
        // seeded error is still the only diagnostic.
        session.queue_command(
            RequestId::Num(4),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let turn = session.service();
        assert_eq!(
            answer_codes(&turn.answers[0].1),
            ["uri_does_not_exist"],
            "the base view is uncorrupted: {:?}",
            turn.answers[0]
        );
    }

    /// A base change under a live overlay stays fresh: the wear captures
    /// base bytes at drain time (never at stage time), the overlay masks
    /// the base edit for its agent, and clearing the overlay reveals the
    /// edited base. The final leg answers through the engine face
    /// (`dartq/analyze`): the disk-check face reads the worktree, which an
    /// overlay base edit never touches, so only the engine view can prove
    /// the wear/restore cycle left the edited base in place.
    #[test]
    fn base_change_under_a_live_overlay_stays_fresh() {
        let (workspace, mut session) = fixture("agent-freshness-unit");
        let main = workspace.path("lib/main.dart");
        let uri = crate::uri::from_path(&main);

        session.sweep().expect("sweep");
        let _ = session.service();
        session
            .set_overlay("A", vec![(main, SYNTAX_BROKEN.as_bytes().to_vec())])
            .expect("stage");
        // The base moves AFTER staging: the agent's capture must happen at
        // wear time, so A keeps seeing its own text and the base moves on.
        assert!(session.queue_edit(&uri, CLEAN_MAIN, 2));
        session.queue_command(
            RequestId::Num(2),
            Request::Check {
                paths: vec![workspace.path("lib")],
                verbosity: Verbosity::Compact,
                agent: Some("A".into()),
                context: None,
                since: None,
            },
        );
        let turn = session.service();
        assert_eq!(
            answer_codes(&turn.answers[0].1),
            ["expected_token", "undefined_identifier"],
            "the overlay masks the base edit: {:?}",
            turn.answers[0]
        );

        // Clear: the edited base shows through — clean.
        session.clear_overlay("A");
        session.queue_command(
            RequestId::Num(3),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let turn = session.service();
        assert!(
            answer_codes(&turn.answers[0].1).is_empty(),
            "the base edit was never clobbered by the wear/restore cycle: {:?}",
            turn.answers[0]
        );
    }

    /// Delta mode (PRD §15.8): the cursor is per-session monotonic, a delta
    /// answer keeps only rows whose file changed since the cursor, and the
    /// answer echoes the session's current cursor for chaining.
    #[test]
    fn delta_cursor_filters_rows_and_echoes_the_current_cursor() {
        let (workspace, mut session) = fixture("delta-cursor-unit");

        session.sweep().expect("sweep");
        let _ = session.service();

        // The edit moves the cursor to 1 and leaves rows behind: a disk
        // rewrite synced through the watch intake (the base check face
        // reads disk, so the rewrite must be on disk for the rows to
        // exist; the sync marks the path changed in the same turn).
        std::fs::write(workspace.path("lib/main.dart"), SYNTAX_BROKEN).expect("rewrite");
        session.queue_disk_changes(&[workspace.path("lib/main.dart")]);

        let (id_all, all) = check_request(2, &[workspace.path("lib")], None, Some(0));
        session.queue_command(id_all, all);
        let (id_current, current) = check_request(3, &[workspace.path("lib")], None, Some(1));
        session.queue_command(id_current, current);
        let (id_base, base) = check_request(4, &[workspace.path("lib")], None, None);
        session.queue_command(id_base, base);
        let turn = session.service();

        let by_id = |wanted: i64| {
            &turn
                .answers
                .iter()
                .find(|(id, _)| *id == RequestId::Num(wanted))
                .expect("answer present")
                .1
        };
        // since=0: the edited file's row stays (the M0 disk pipeline's
        // scan+parse row set); the cursor (1) echoes.
        assert_eq!(answer_codes(by_id(2)), ["expected_token"], "{:?}", by_id(2));
        assert_eq!(
            by_id(2).as_ref().expect("ok")["since"],
            1,
            "the answer echoes the session's current cursor"
        );
        // since=1: the file's last change IS the cursor — nothing newer.
        assert!(
            answer_codes(by_id(3)).is_empty(),
            "a delta at the current cursor is empty: {:?}",
            by_id(3)
        );
        assert_eq!(by_id(3).as_ref().expect("ok")["since"], 1);
        // No `since`: the full answer, no cursor field.
        assert_eq!(answer_codes(by_id(4)), ["expected_token"]);
        assert!(
            by_id(4).as_ref().expect("ok").get("since").is_none(),
            "a full answer carries no cursor"
        );
    }

    /// A base write always drains its publish diff in the base view — even
    /// when the only requests riding the read want an agent view (the
    /// zero-request base analysis), and an agent view never publishes.
    #[test]
    fn base_publish_drains_when_only_agent_requests_ride_the_read() {
        let (workspace, mut session) = fixture("agent-publish-unit");
        let main = workspace.path("lib/main.dart");
        let uri = crate::uri::from_path(&main);

        session.sweep().expect("sweep");
        let _ = session.service();
        // Agent A's overlay carries a syntax error; the base edit cleans
        // the file. If the publish leaked A's view, the push would carry
        // `expected_token` instead of the base clear.
        session
            .set_overlay("A", vec![(main, AGENT_B_MAIN.as_bytes().to_vec())])
            .expect("stage");
        let (id_a, a) = check_request(2, &[workspace.path("lib")], Some("A"), None);
        session.queue_command(id_a, a);
        assert!(session.queue_edit(&uri, CLEAN_MAIN, 2));
        let turn = session.service();

        assert_eq!(
            turn.publishes.len(),
            1,
            "the base edit's clear push drains: {:?}",
            turn.publishes
        );
        assert!(
            turn.publishes[0].diagnostics.is_empty(),
            "the push is the base view, never the agent's rows: {:?}",
            turn.publishes[0]
        );
        assert_eq!(
            answer_codes(&turn.answers[0].1),
            ["expected_token", "undefined_identifier"],
            "agent A still answers its own (masking) view: {:?}",
            turn.answers[0]
        );
    }

    /// A panicking read group (the containment boundary; daemon-audit
    /// finding 5) answers its requests `INTERNAL_ERROR` instead of killing
    /// the process, and the next turn answers normally — with the deferred
    /// publish diff still draining.
    #[test]
    fn a_panicking_read_answers_internal_error_and_the_session_survives() {
        let (workspace, mut session) = fixture("panic-boundary-unit");
        let uri = crate::uri::from_path(&workspace.path("lib/main.dart"));

        session.sweep().expect("sweep");
        let _ = session.service();

        // The panicked turn carries a real pending diff: the write applies
        // (the diff becomes due), then the read group panics before it can
        // plan the publishes.
        assert!(session.queue_edit(&uri, CLEAN_MAIN, 2));
        crate::probe::arm_read_panic();
        session.queue_command(
            RequestId::Num(2),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let turn = session.service();
        assert!(
            turn.publishes.is_empty(),
            "the panicked read planned no publishes: {:?}",
            turn.publishes
        );
        assert!(
            matches!(&turn.answers[0].1, Err(SessionError::Internal(message))
                if message.contains("panicked")),
            "the boundary answers INTERNAL_ERROR: {:?}",
            turn.answers[0]
        );

        // The next turn is healthy, and the publish diff the panicked read
        // deferred still drains.
        session.queue_command(
            RequestId::Num(3),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let turn = session.service();
        assert_eq!(
            turn.publishes.len(),
            1,
            "the deferred publish diff drains on recovery: {:?}",
            turn.publishes
        );
        assert!(
            turn.publishes[0].diagnostics.is_empty(),
            "the deferred push is the post-write base view: {:?}",
            turn.publishes[0]
        );
        assert!(
            matches!(&turn.answers[0].1, Ok(_)),
            "the session survives the panic: {:?}",
            turn.answers[0]
        );
    }

    /// The shared overlay store is swept at the turn boundary (daemon-audit
    /// finding 2): N distinct edit states leave the store bounded, and
    /// analysis answers stay correct after the sweeps.
    #[test]
    fn overlay_store_sweep_bounds_the_shared_store() {
        let (workspace, mut session) = fixture("overlay-gc-unit");
        let uri = crate::uri::from_path(&workspace.path("lib/main.dart"));

        session.sweep().expect("sweep");
        let _ = session.service();
        for version in 0..(GC_CONTENTS_THRESHOLD * 2) {
            assert!(session.queue_edit(
                &uri,
                &format!("void main() {{}} // v{version}\n"),
                i64::try_from(version).expect("version") + 2,
            ));
            let _ = session.service();
        }
        let contents = session.overlay_store_contents();
        assert!(
            contents <= GC_CONTENTS_THRESHOLD + 4,
            "the turn-boundary sweep bounds the store: {contents} contents after {} edits",
            GC_CONTENTS_THRESHOLD * 2
        );

        // The sweeps never corrupted the analysis state.
        session.queue_command(
            RequestId::Num(2),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let turn = session.service();
        assert!(matches!(&turn.answers[0].1, Ok(_)));
    }

    /// The same bound under agent-overlay churn (the store-hygiene ticket's
    /// shape): distinct agent overlays driven through turns cross the
    /// threshold — each wear/restore cycle leaves the agent's content dead
    /// in the shared store — the turn-boundary sweep evicts it, and a
    /// still-active agent's next turn answers correctly after the sweeps.
    #[test]
    fn agent_overlay_churn_is_swept_at_the_turn_boundary() {
        let (workspace, mut session) = fixture("agent-overlay-gc-unit");
        let main = workspace.path("lib/main.dart");

        session.sweep().expect("sweep");
        let _ = session.service();

        // One distinct content per agent: N agents accumulate N garbage
        // contents across their turns.
        let count = GC_CONTENTS_THRESHOLD + 8;
        for index in 0..count {
            let agent = format!("agent-{index}");
            session
                .set_overlay(
                    &agent,
                    vec![(
                        main.clone(),
                        format!("void agent() {{}} // {index}\n").into_bytes(),
                    )],
                )
                .expect("under the agent cap");
            session.queue_command(
                RequestId::Num(i64::try_from(index).expect("index fits i64") + 2),
                Request::Analyze {
                    verbosity: Verbosity::Compact,
                    agent: Some(agent),
                    context: None,
                },
            );
            let turn = session.service();
            assert!(
                turn.answers[0].1.is_ok(),
                "agent {index} answers: {:?}",
                turn.answers[0]
            );
        }
        let contents = session.overlay_store_contents();
        assert!(
            contents <= GC_CONTENTS_THRESHOLD + 4,
            "the turn-boundary sweep bounds the store: {contents} contents after {count} \
             agent overlays"
        );

        // A still-active agent's next turn is correct after the sweeps: the
        // view materializes again from its staged overlay.
        session.queue_command(
            RequestId::Num(10_000),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: Some(format!("agent-{}", count - 1)),
                context: None,
            },
        );
        let turn = session.service();
        assert!(
            matches!(&turn.answers[0].1, Ok(_)),
            "the still-active agent answers after the sweeps: {:?}",
            turn.answers[0]
        );
    }

    /// The live-agent cap: one more overlay than the cap answers
    /// INVALID_PARAMS until one is cleared.
    #[test]
    fn agent_overlay_count_is_capped() {
        let (workspace, mut session) = fixture("agent-cap-unit");
        let main = workspace.path("lib/main.dart");

        for index in 0..MAX_AGENTS {
            session
                .set_overlay(
                    &format!("agent-{index}"),
                    vec![(main.clone(), CLEAN_MAIN.as_bytes().to_vec())],
                )
                .expect("under the cap");
        }
        assert!(
            session
                .set_overlay(
                    "overflow",
                    vec![(main.clone(), CLEAN_MAIN.as_bytes().to_vec())]
                )
                .is_err(),
            "the cap rejects a new agent"
        );
        session.clear_overlay("agent-0");
        session
            .set_overlay("overflow", vec![(main, CLEAN_MAIN.as_bytes().to_vec())])
            .expect("a cleared slot frees room");
    }

    // -----------------------------------------------------------------------
    // Multi-root contexts (PRD §16.2), overlay commit (§16.3's merge),
    // warm start (#482's daemon wiring)
    // -----------------------------------------------------------------------

    /// The conditional-import fixture: the default context (no declared
    /// variables) resolves `heavy.dart`; the forked context (declaring
    /// `dart.library.heavy`) resolves `light.dart` — one workspace, two
    /// observable views.
    const COND_MAIN: &str =
        "import 'heavy.dart' if (dart.library.heavy) 'light.dart';\nvoid main() { heavy(); }\n";
    /// `heavy.dart`: broken at the URI layer, so the heavy branch's rows
    /// are recognizable in any view.
    const HEAVY_DART: &str = "import 'missing.dart';\nvoid heavy() {}\n";
    /// `light.dart`: clean.
    const LIGHT_DART: &str = "void light() {}\n";

    /// A throwaway snapshot store rooted at `root` (the caller keeps the
    /// path to count generations).
    fn store_at(root: PathBuf) -> SnapshotStore {
        SnapshotStore::open(SnapshotConfig {
            root: Some(root),
            capacity_bytes: DEFAULT_CAPACITY_BYTES,
        })
        .expect("store opens on a temp root")
    }

    /// The `t` context's config over the shared pool: same workspace and
    /// SDK identity as the default, forked by the declared variable. The
    /// SDK reloads per config (the snapshot type is not `Clone`).
    fn fork_config(
        workspace: &Workspace,
        pool: &dartq_engine::ContentPool,
    ) -> (String, EngineConfig) {
        let mut fork = workspace.engine_config();
        fork.store = dartq_engine::pooled_memory_store(pool);
        fork.declared_variables
            .insert("dart.library.heavy".to_owned(), "true".to_owned());
        ("t".to_owned(), fork)
    }

    /// The conditional fixture's workspace: `main` plus the two branch
    /// files on disk.
    fn cond_workspace() -> Workspace {
        let workspace = Workspace::new(COND_MAIN);
        fs::write(workspace.path("lib/heavy.dart"), HEAVY_DART).expect("write heavy");
        fs::write(workspace.path("lib/light.dart"), LIGHT_DART).expect("write light");
        workspace
    }

    /// A two-context session over the conditional fixture — the tagless
    /// default plus `t` — with every store wired to one content pool
    /// (#483's multi-context construction contract). Returns the pool (to
    /// pin the sharing) and the store root (to count generations).
    fn fixture_two_contexts(
        name: &str,
    ) -> (Workspace, Session, dartq_engine::ContentPool, PathBuf) {
        let workspace = cond_workspace();
        let pool = dartq_engine::ContentPool::new();
        let mut base = workspace.engine_config();
        base.store = dartq_engine::pooled_memory_store(&pool);
        let root = store_root(name);
        let session = Session::with_contexts(
            base,
            [fork_config(&workspace, &pool)],
            store_at(root.clone()),
        )
        .expect("session");
        (workspace, session, pool, root)
    }

    /// The rows of an analyze answer.
    fn rows(answer: &Result<serde_json::Value, SessionError>) -> Vec<serde_json::Value> {
        answer.as_ref().expect("analyze answers")["diagnostics"]
            .as_array()
            .expect("diagnostics array")
            .clone()
    }

    /// Whether the rows mention `suffix` (a file's uri tail).
    fn mentions(rows: &[serde_json::Value], suffix: &str) -> bool {
        rows.iter()
            .any(|row| row["uri"].as_str().is_some_and(|uri| uri.ends_with(suffix)))
    }

    /// Requests route by `context` tag: the default context resolves the
    /// conditional import's heavy branch, the declared-variable fork
    /// resolves light — each answer comes from its own engine, over one
    /// shared content pool.
    #[test]
    fn requests_route_to_their_declared_context() {
        let (_workspace, mut session, pool, _root) = fixture_two_contexts("route-unit");

        session.sweep().expect("sweep");
        let _ = session.service();

        // The pool holds the workspace's three distinct contents exactly
        // once — not once per context (the #483 sharing contract, observed
        // through the daemon face).
        assert_eq!(pool.len(), 3, "three files shared across both engines");

        // The forked tag exists; a typo would fail loud at the wire.
        assert!(session.has_context("t"));
        assert!(!session.has_context("nope"));

        session.queue_command(
            RequestId::Num(2),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        session.queue_command(
            RequestId::Num(3),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: Some("t".into()),
            },
        );
        let turn = session.service();
        assert_eq!(turn.answers.len(), 2, "one answer per request");
        assert!(turn.answers[0].1.is_ok(), "the default view answers");
        assert!(turn.answers[1].1.is_ok(), "the forked view answers");
        // The tag routed to its own context — a distinct engine from the
        // default, forked by the declared variable (the ContextKey fold).
        // The fork's observable analysis divergence (the conditional import
        // resolving the light branch) is the engine's declared-variables
        // wiring, not the daemon's routing — pinned there, not here.
        assert_ne!(
            session.route(Some("t")),
            session.default_context,
            "the tag names its own context"
        );
    }

    /// Equal configurations dedup to one context (§16.1): tags over the
    /// default's config shape share its engine, and the flush writes ONE
    /// generation (equal salt) — the forked pair's two generations are the
    /// contrast pinned by the warm-start test below.
    #[test]
    fn equal_config_tags_share_one_context_and_generation() {
        let workspace = cond_workspace();
        let pool = dartq_engine::ContentPool::new();
        let mut base = workspace.engine_config();
        base.store = dartq_engine::pooled_memory_store(&pool);
        let mut a = workspace.engine_config();
        a.store = dartq_engine::pooled_memory_store(&pool);
        let mut b = workspace.engine_config();
        b.store = dartq_engine::pooled_memory_store(&pool);
        let root = store_root("dedup-unit");
        let mut session = Session::with_contexts(
            base,
            [("a".to_owned(), a), ("b".to_owned(), b)],
            store_at(root.clone()),
        )
        .expect("session");

        session.sweep().expect("sweep");
        let _ = session.service();

        // Both tags route — through the one shared engine — and answer.
        session.queue_command(
            RequestId::Num(2),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: Some("a".into()),
            },
        );
        let turn = session.service();
        assert!(
            turn.answers[0].1.is_ok(),
            "the shared tag answers: {:?}",
            turn.answers
        );

        // The sink dirties on an overlay apply only.
        let uri = uri::from_path(&workspace.path("lib/main.dart"));
        assert!(session.queue_edit(&uri, COND_MAIN, 2));
        let _ = session.service();
        session.flush_snapshot();
        let generations = fs::read_dir(root.join("snapshots"))
            .expect("the flush wrote the generations dir")
            .count();
        assert_eq!(
            generations, 1,
            "equal configurations share one generation directory"
        );
    }

    /// The warm-start wiring (#482 consumed by the daemon): the flush
    /// writes one generation per DISTINCT context configuration — forked
    /// contexts' salts never collide on a directory — and a fresh session
    /// over the same store warm-starts (load + ingest before the first
    /// apply) and answers the same views.
    #[test]
    fn flush_writes_per_context_generations_and_warm_start_replays() {
        let (workspace, mut session, _pool, root) = fixture_two_contexts("warm-unit");
        session.sweep().expect("sweep");
        let _ = session.service();
        // The sink dirties on an overlay apply only: a base write marks the
        // flush due (the sweep's discovery alone never saves).
        let uri = uri::from_path(&workspace.path("lib/main.dart"));
        assert!(session.queue_edit(&uri, COND_MAIN, 2));
        let _ = session.service();
        session.flush_snapshot();
        let generations = fs::read_dir(root.join("snapshots"))
            .expect("the flush wrote the generations dir")
            .count();
        assert_eq!(
            generations, 2,
            "one generation per distinct configuration: the fork's salt separates the directories"
        );

        // A fresh daemon world: same store, same workspace, same declared
        // contexts — its contexts warm-start at construction.
        let pool = dartq_engine::ContentPool::new();
        let mut base = workspace.engine_config();
        base.store = dartq_engine::pooled_memory_store(&pool);
        let mut warm =
            Session::with_contexts(base, [fork_config(&workspace, &pool)], store_at(root))
                .expect("warm session");
        warm.sweep().expect("sweep");
        let _ = warm.service();
        warm.queue_command(
            RequestId::Num(2),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        warm.queue_command(
            RequestId::Num(3),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: Some("t".into()),
            },
        );
        let turn = warm.service();
        assert_eq!(turn.answers.len(), 2);
        assert!(
            turn.answers[0].1.is_ok() && turn.answers[1].1.is_ok(),
            "the warm session answers both views: {:?}",
            turn.answers
        );
        // The fresh session's contexts stayed distinct through the load.
        assert_ne!(warm.route(Some("t")), warm.default_context);
    }

    /// The plugin weave (#471 phase B): a plugin whose spawn fails (the
    /// entrypoint does not exist) is recorded at assembly; the crash row
    /// withholds until the session has an anchor (a published file), then
    /// publishes once — merged after the engine rows — and never repeats.
    #[test]
    fn a_failing_plugin_surfaces_its_crash_row_on_the_base_publish() {
        let workspace = Workspace::new(BROKEN_MAIN);
        // The broken manifest lands before assembly: launch discovers it,
        // the spawn fails (no such entrypoint), and the set records the
        // crash for the first anchored cycle.
        let dir = workspace.path("plugins/dead");
        fs::create_dir_all(&dir).expect("plugin dir");
        fs::write(
            dir.join("dartq_plugin.json"),
            r#"{"name": "dead", "version": "0.1.0", "entrypoint": "missing.dart"}"#,
        )
        .expect("manifest");
        let store = SnapshotStore::open(SnapshotConfig {
            root: Some(store_root("plugin-crash-unit")),
            capacity_bytes: DEFAULT_CAPACITY_BYTES,
        })
        .expect("store opens on a temp root");
        let mut session = Session::with_store(workspace.engine_config(), store).expect("session");

        session.sweep().expect("sweep");
        let turn = session.service();
        assert_eq!(
            turn.publishes.len(),
            1,
            "the seeded error publishes: {:?}",
            turn.publishes
        );
        assert!(
            turn.publishes[0]
                .diagnostics
                .iter()
                .all(|row| row.code != dartq_diagnostics::DARTQ_PLUGIN_CRASHED.name),
            "no anchor existed at the first analysis — the crash withheld"
        );

        // The next base analysis has an anchor (the published file): the
        // held crash row publishes once, merged after the engine rows.
        session.queue_command(
            RequestId::Num(2),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let turn = session.service();
        assert_eq!(
            turn.publishes.len(),
            1,
            "the crash row joined the file's push: {:?}",
            turn.publishes
        );
        let rows = &turn.publishes[0].diagnostics;
        assert_eq!(
            rows[0].code, "uri_does_not_exist",
            "engine rows stay first: {rows:?}"
        );
        assert_eq!(
            rows[1].code,
            dartq_diagnostics::DARTQ_PLUGIN_CRASHED.name,
            "the crash row publishes after the merge: {rows:?}"
        );
        assert_eq!(
            rows[1].source, "dartq",
            "crash rows ride the engine conversion"
        );

        // Once per session: the next base cycle is clean again.
        session.queue_command(
            RequestId::Num(3),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let turn = session.service();
        assert!(
            turn.publishes.is_empty(),
            "the crash row never repeats: {:?}",
            turn.publishes
        );
    }

    /// `overlay.commit` (PRD §16.3's merge): the agent's staged buffer
    /// writes back to the worktree, merges into the base view (the publish
    /// diff drains the clean set), and the staged entries free — the
    /// agent's view falls through to the committed base, whose change
    /// cursors moved at the commit drain.
    #[test]
    fn overlay_commit_writes_back_and_merges_the_base() {
        let (workspace, mut session) = fixture("commit-unit");
        let main = workspace.path("lib/main.dart");

        session.sweep().expect("sweep");
        let _ = session.service();
        session
            .set_overlay("A", vec![(main.clone(), CLEAN_MAIN.as_bytes().to_vec())])
            .expect("stage");

        let before = session.cursor;
        assert_eq!(session.commit_overlay("A").expect("commit"), 1);
        assert_eq!(
            fs::read(&main).expect("read back"),
            CLEAN_MAIN.as_bytes(),
            "the buffer reached the worktree"
        );
        assert!(
            session.agents["A"].files.is_empty(),
            "the staged entries freed"
        );

        // The merge drains with the staged write: the base view publishes
        // clean, and the committed path's cursor moved at that drain.
        let turn = session.service();
        assert_eq!(
            turn.publishes.len(),
            1,
            "the merge's clear push: {:?}",
            turn.publishes
        );
        assert!(turn.publishes[0].diagnostics.is_empty());
        assert!(session.cursor > before, "the merge bumped the cursor");
        assert_eq!(
            session.changed.get(&main),
            Some(&session.cursor),
            "the committed path changed at the drain"
        );

        // The agent's view now falls through to the committed base.
        session.queue_command(
            RequestId::Num(2),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: Some("A".into()),
                context: None,
            },
        );
        let turn = session.service();
        assert!(
            answer_codes(&turn.answers[0].1).is_empty(),
            "A sees the committed base: {:?}",
            turn.answers[0]
        );
    }

    /// Commit edges: an unknown agent is the usage tier; committing an
    /// already-merged agent (empty overlay) is a `0` no-op that stages
    /// nothing.
    #[test]
    fn overlay_commit_edges() {
        let (workspace, mut session) = fixture("commit-edges-unit");
        let main = workspace.path("lib/main.dart");

        assert!(
            matches!(
                session.commit_overlay("ghost"),
                Err(SessionError::InvalidParams(message)) if message.contains("ghost")
            ),
            "an unknown agent fails loud"
        );

        session.sweep().expect("sweep");
        let _ = session.service();
        session
            .set_overlay("A", vec![(main.clone(), CLEAN_MAIN.as_bytes().to_vec())])
            .expect("stage");
        assert_eq!(session.commit_overlay("A").expect("first commit"), 1);
        let _ = session.service();
        // Second commit: the overlay emptied at the merge — a no-op.
        assert_eq!(session.commit_overlay("A").expect("second commit"), 0);
        assert!(session.staged.is_empty(), "the no-op staged nothing");
        assert_eq!(
            fs::read(&main).expect("disk"),
            CLEAN_MAIN.as_bytes(),
            "the disk state is untouched by the no-op"
        );
    }

    /// The `context` param's wire contract at the parse edge: the
    /// engine-view faces carry it; the disk-view faces reject it (the
    /// agentId rule, extended).
    #[test]
    fn context_param_parses_on_engine_faces_and_rejects_on_disk_faces() {
        let request = crate::commands::parse(
            "dartq/check",
            &serde_json::json!({"paths": ["lib"], "context": "t"}),
            Path::new("/"),
        )
        .expect("check takes context");
        assert_eq!(request.context(), Some("t"));

        let error = crate::commands::parse(
            "dartq/fix",
            &serde_json::json!({"paths": ["lib"], "context": "t"}),
            Path::new("/"),
        )
        .expect_err("fix rejects context");
        assert!(error.contains("agentId/context/since"), "{error}");

        let error = crate::commands::parse(
            "dartq/explain",
            &serde_json::json!({"symbol": "main", "context": "t"}),
            Path::new("/"),
        )
        .expect_err("explain rejects context");
        assert!(error.contains("agentId/context"), "{error}");
    }

    /// dartq/analyze answers follow the CLI's severity-first printed order
    /// (severity, path bytes, offset, message — the analyze face's sort),
    /// not the engine's canonical (file, offset) order the other faces
    /// mirror: the fixture's file order (a.dart before b.dart) disagrees
    /// with severity-first, and the answer still puts b.dart's error first.
    #[test]
    fn analyze_answers_sort_severity_first_like_the_cli() {
        let workspace = Workspace::new(CLEAN_MAIN);
        // a.dart sorts first and carries only a static warning.
        fs::write(
            workspace.path("lib/a.dart"),
            "class C { void m() {} } void f(C c) { c?.m(); }\n",
        )
        .expect("write a.dart");
        // b.dart carries the URI-layer error.
        fs::write(
            workspace.path("lib/b.dart"),
            "import 'missing.dart';\nvoid main() {}\n",
        )
        .expect("write b.dart");
        let store = SnapshotStore::open(SnapshotConfig {
            root: Some(store_root("analyze-order-unit")),
            capacity_bytes: DEFAULT_CAPACITY_BYTES,
        })
        .expect("store opens on a temp root");
        let mut session = Session::with_store(workspace.engine_config(), store).expect("session");

        session.sweep().expect("sweep");
        let _ = session.service();
        session.queue_command(
            RequestId::Num(2),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let turn = session.service();
        let entries = turn.answers[0].1.as_ref().expect("analyze answers")["diagnostics"]
            .as_array()
            .expect("diagnostics array");
        assert_eq!(entries.len(), 2, "the warning plus the error: {entries:?}");
        assert_eq!(
            entries[0]["uri"],
            uri::from_path(&workspace.path("lib/b.dart")),
            "the error sorts first despite b.dart sorting second: {entries:?}"
        );
        assert_eq!(entries[0]["severity"], 1, "error first");
        assert_eq!(
            entries[1]["uri"],
            uri::from_path(&workspace.path("lib/a.dart")),
            "the warning follows: {entries:?}"
        );
        assert_eq!(entries[1]["severity"], 2, "warning second");
    }

    /// dartq/explain's `sdk` default is the session's resolved SDK root: a
    /// request without `sdk` still answers (the per-query engine assembles
    /// against the SDK the session itself runs on), and an explicit `sdk`
    /// stays authoritative — a bogus root is the usage tier.
    #[test]
    fn explain_sdk_defaults_to_the_session_root_and_explicit_wins() {
        let (workspace, mut session) = fixture("explain-sdk-unit");

        session.sweep().expect("sweep");
        let _ = session.service();

        // Absent `sdk`: the session's root backs the report.
        session.queue_command(
            RequestId::Num(2),
            Request::Explain {
                symbol: "main".into(),
                verbosity: Verbosity::Compact,
                since: None,
                sdk: None,
            },
        );
        let turn = session.service();
        let report = turn.answers[0]
            .1
            .as_ref()
            .expect("the session SDK backs the report");
        assert_eq!(report["declaration"]["name"], "main");

        // Explicit `sdk` is authoritative: a bogus root fails loudly.
        session.queue_command(
            RequestId::Num(3),
            Request::Explain {
                symbol: "main".into(),
                verbosity: Verbosity::Compact,
                since: None,
                sdk: Some(workspace.path("no-such-sdk")),
            },
        );
        let turn = session.service();
        assert!(
            matches!(&turn.answers[0].1, Err(SessionError::InvalidParams(_))),
            "a bogus explicit sdk answers INVALID_PARAMS: {:?}",
            turn.answers[0]
        );
    }
}
