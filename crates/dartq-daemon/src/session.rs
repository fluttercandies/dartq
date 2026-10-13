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
//! # Fix dispatch off the main loop (PRD §14, §15.6)
//!
//! `dartq/fix` and `dartq/assists` stage like reads — one task per request
//! on the default context's epoch, `Interactive` (a client waits) — so a
//! cancel that advances the epoch voids them for free. At the drain the
//! task does not run inline: the plugin handshakes and the per-hit
//! `request/fixes` round trips (each bounded by the manifests' budgets,
//! their sum unbounded) would hold the loop. The job moves to one worker
//! thread (at most one at a time — the host set is one exclusive request
//! surface); the main loop keeps serving edits, reads, publishes, and
//! cancels, wakes on the worker's completion through a session-owned
//! channel, and posts the answer then. A wall-clock budget
//! ([`FIX_TIME_BUDGET`]) answers the request even when the worker is still
//! running — the slow-plugin worst case degrades to one abandoned dispatch,
//! never a wedged session; the late result is discarded when it lands. A
//! `dartq/fixProgress` notification rides the turn's notification pipeline
//! when the dispatch starts, so a client sees the work begin.
//!
//! The off-loop invariants: a fix job never blocks the select loop (the
//! dispatch returns at the spawn — staging, the progress notice, and the
//! completion drain are the loop's whole duty), and the completion drain
//! ([`Session::complete_fix`] → `absorb_fix`) is the one mutation point
//! for fix state: the worker only mails its [`FixMail`]; a cancel or the
//! [`FIX_TIME_BUDGET`] expiry only flags the in-flight run, and a flagged
//! run's late mail drops without answering. The worker body is itself a
//! panic boundary (#947 F5): a panicking job answers `INTERNAL_ERROR`,
//! logs, and still mails — the slot frees and the host set comes home.
//! Ordering caveat: the epoch
//! pins staging order, not worker execution — a running dispatch reads the
//! workspace from disk through the plugin hosts, so its reads can
//! interleave with the overlay writes of later batches; the answer
//! reflects the disk state the worker observed, not the overlays a
//! concurrent analysis sees.
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
//! state, so the boundary treats it as untrusted: the panicking view group
//! answers its requests `INTERNAL_ERROR`, a best-effort base restore
//! (itself panic-contained) hands the next group the cleanest view
//! achievable, and the turn's remaining groups still analyze and answer —
//! containment is per group (#690), never session-wide. The plugin weave
//! (its only plugin-row exposure) is contained a level finer still: a
//! panic there skips the cycle's findings but keeps the group's engine
//! rows flowing, and it is attributable, so it penalizes immediately.
//! Repeat offenders, and any panic that keeps recurring on the
//! plugin-bearing group ([`PLUGIN_PANIC_LIMIT`] strikes, never reset),
//! disable the whole plugin set for the session through the same
//! one-shot-crash-row degrade the transport's restart-budget exhaustion
//! uses. Nothing is masked: the shell's panic hook reports every panic
//! once (payload, thread, location) and the catching boundary logs its
//! recovery on stderr, so no payload is printed twice; the affected
//! requests see the internal-error tier.
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
use std::path::{Component, Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, unbounded};
use dartq_commands::Verbosity;
use dartq_commands::discover::DiscoverError;
use dartq_commands::fix::{self, PluginHosts};
use dartq_commands::layout::{path_key, severity_rank};
use dartq_commands::workspace::WorkspaceAdvisory;
use dartq_diagnostics::{Diagnostic, FileId, Severity};
use dartq_engine::{
    CancellationToken, Cancelled, ContextConfig, ContextId, Engine, EngineConfig, EngineError,
    OverlayChange, PersistedFacts, Runtime, Tier,
};
use rustc_hash::FxHashMap;
use serde_json::json;
use thiserror::Error;

use dartq_snapshot::{SnapshotConfig, SnapshotStore};

use crate::commands::{self, Request};
use crate::jsonrpc::{Notification, RequestId};
use crate::lsp;
use crate::plugins::{self, PluginSet};
use crate::uri;

/// The debounce window between the last overlay apply and the snapshot
/// generation save (#306): 500 ms. While a save is pending the main loop's
/// idle receive is bounded by the window's remainder, so an idle daemon
/// wakes to save — no timer thread.
pub(crate) const SNAPSHOT_DEBOUNCE: Duration = Duration::from_millis(500);

/// The per-runtime memory budget every session context runs under (PRD
/// §17.1's `memory.limit_mib` default, 2048 in the config sample): over
/// budget, each context's engine degrades tier-by-tier and reports each
/// step as `dartq/memory_degraded` instead of growing without bound.
const DEFAULT_MEMORY_LIMIT_MIB: u64 = 2048;

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

/// The overlay-store GC byte watermark (#947 F7): sweep once this many
/// content bytes have been stored since the last sweep. The entry threshold
/// above bounds garbage by count, but 64 entries of large documents would
/// still be hundreds of MiB resident before any sweep fired — so the byte
/// tally bounds the ceiling too. 256 MiB: far above any real editing
/// window's dead content, so ordinary sessions still sweep on the entry
/// count, while huge-document traffic sweeps on bytes. The #952 inflow caps
/// frame the watermark's meaning — each staged set is bounded (16 MiB per
/// wire frame, 64 MiB per `overlay.set`), so the resident garbage stays
/// under this watermark plus one write batch's bounded inflow.
const GC_BYTES_SINCE_SWEEP: u64 = 256 << 20;

/// The live agent-overlay cap: `overlay.set` naming an agent beyond it is
/// answered `INVALID_PARAMS` (clear one first). Teardown paths —
/// `overlay.clear`, session end — evict deterministically; the cap only
/// stops a client from growing the table without bound.
const MAX_AGENTS: usize = 256;

/// One agent's staged-overlay file ceiling (#947 F3): `overlay.set` merging
/// an agent past it is rejected `INVALID_PARAMS`. 64 files — a whole
/// package's worth of simultaneous agent edits is far below; the bound keeps
/// one agent's staged table (and the commit's clone) small regardless of
/// what the RPC declares.
const MAX_OVERLAY_FILES: usize = 64;

/// One agent's staged-overlay byte ceiling (#947 F3): the sum of staged
/// `file.bytes` past it is rejected `INVALID_PARAMS`. 64 MiB — an order of
/// magnitude above real document traffic (the wire frame cap is 16 MiB per
/// message); the bound caps the staged set and makes `commit_overlay`'s
/// clone-then-write transient provably small.
const MAX_OVERLAY_BYTES: usize = 64 << 20;

/// The session-wide staged-overlay byte ceiling (#962 P3-2): the sum of
/// every live agent's staged `file.bytes` past it is rejected
/// `INVALID_PARAMS`. The per-agent cap alone leaves a 16 GiB theoretical
/// residency ([`MAX_AGENTS`] × [`MAX_OVERLAY_BYTES`]); 256 MiB — the order
/// of the store's GC watermark — bounds the whole staged table to a few
/// hundred MiB worst case, while real agent traffic (KiB-scale documents)
/// never approaches it.
const MAX_OVERLAY_TOTAL_BYTES: usize = 256 << 20;

/// The delta-cursor table's path cap (#947 F8): `changed` grows one entry
/// per distinct written path, and edits merge last-wins, but a session that
/// never deletes files would otherwise grow it for its whole life. Past the
/// cap the lowest cursors evict — those are the rows only a client paging
/// from a very old `since` could still ask about, and the delta contract
/// for them is "re-sync without `since`". `65_536` paths ≈ 10 MiB of
/// bookkeeping at ~150 B/entry, orders above any real editing session.
const MAX_DELTA_PATHS: usize = 65_536;

/// The open-document table's cap (#947 F8): `opened` grows one entry per
/// distinct `didOpen`, and only `didClose` removed it before. Real clients
/// hold tens of open editors; `4_096` is orders above that (≈ 0.5 MiB of
/// bookkeeping), so the least-recently-touched eighth evicts only under a
/// broken-client open storm. An evicted document degrades to on-disk
/// truth: its publishes lose the version, and a watch sync may resync its
/// disk bytes — the documented cost of the bound.
const MAX_OPEN_DOCUMENTS: usize = 4_096;

/// The repeat-offender ladder (#690): unattributable panics on the
/// plugin-bearing group (the default context's base view) tolerate this
/// many strikes before the whole plugin set is disabled for the session.
/// Small by design — a panic re-fires deterministically — and the count
/// never resets, mirroring the transport's never-resetting restart budget.
const PLUGIN_PANIC_LIMIT: u32 = 3;

/// The wall-clock ceiling for one fix/assists dispatch (PRD §14.3's
/// isolation posture at session granularity). The manifests' own budgets
/// (`request_ms × (retries + 1)`, defaults 10 s × 4) bound each plugin
/// round trip; their sum over N plugins × M hits is unbounded, so the
/// session caps the whole dispatch: past the budget the request answers an
/// internal error and the in-flight work is abandoned (its result is
/// discarded when it lands — the host set still returns through the
/// completion mail, so nothing leaks). Generous enough that only a
/// genuinely wedged plugin set ever hits it; finite so the session never
/// hangs on one.
const FIX_TIME_BUDGET: Duration = Duration::from_mins(2);

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

/// One client-opened document's bookkeeping: the version publishes echo,
/// plus the LRU stamp the cap eviction scans (#947 F8).
struct OpenDocument {
    version: i64,
    /// Session-monotonic edit stamp — the most recent `queue_edit` touch.
    touched: u64,
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
    /// Server→client notifications the turn staged (the fix dispatch's
    /// progress notice; the same pipeline the publishes ride).
    pub notifications: Vec<Notification>,
    /// One entry per attached request; `Err` marks a request whose task was
    /// stale at the drain (cancelled) or whose parameters named no valid
    /// target, and produced nothing.
    pub answers: Vec<(RequestId, Result<serde_json::Value, SessionError>)>,
}

/// One read task's per-view request group: the agent name (`None` is the
/// base view) with the view's queued requests.
type ViewGroup = (Option<String>, Vec<(RequestId, Request)>);

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

/// The plugin-dispatch work one `dartq/fix`/`dartq/assists` request staged:
/// the wire parameters, paths already resolved against the workspace root.
#[derive(Debug)]
pub(crate) enum FixJob {
    /// The §15.6 patch documents over the requested paths (built-in lint
    /// rows plus the hosts' proposals).
    Fix {
        paths: Vec<PathBuf>,
        verbosity: Verbosity,
    },
    /// The hosts' location-directed proposals for one file.
    Assists {
        path: PathBuf,
        offset: u64,
        length: u64,
        verbosity: Verbosity,
    },
    /// Test-only (the #947 F5 containment pin): a job that panics inside
    /// the worker. A panicking job cannot be conjured deterministically
    /// from RPC input, and the probe's thread-local trigger seams cannot
    /// reach a worker thread — the job payload itself is the deterministic,
    /// race-free seam.
    #[cfg(test)]
    Panics,
}

impl FixJob {
    /// The wire method the job answers (the progress notice names it).
    fn method(&self) -> &'static str {
        match self {
            Self::Fix { .. } => "dartq/fix",
            Self::Assists { .. } => "dartq/assists",
            #[cfg(test)]
            Self::Panics => "dartq/fix",
        }
    }
}

/// One staged fix job: one request per task (the jobs share no work to
/// coalesce), executed off the main loop at its drain.
struct PendingFix {
    id: RequestId,
    job: FixJob,
    /// The scheduler task's cookie (the drain report names tasks by it).
    cookie: u64,
}

/// A fix dispatch in flight on its worker thread. The slot stays occupied
/// until the worker's completion mail lands — even past the budget (the
/// `expired` answer went out, but the host set is still owned by the
/// worker, so the next dispatch must wait for it) — which is what keeps
/// the hosts a single exclusive surface.
struct FixRun {
    id: RequestId,
    deadline: Instant,
    /// Set once the budget expiry answered; the late completion is then
    /// discarded (no second answer for one request id).
    expired: bool,
    /// Set by a `$/cancelRequest` while the dispatch ran: the worker cannot
    /// be stopped (it owns the host set), so the late completion drops and
    /// the id answers `Cancelled` at the next drain.
    cancelled: bool,
}

/// The worker's completion mail: the host set returns with the outcome.
pub(crate) struct FixMail {
    hosts: PluginHosts,
    outcome: Result<serde_json::Value, SessionError>,
}

/// The worker-side execution of one fix job — the same faces the intake
/// dispatch ran inline before the staging moved them off the main loop.
fn run_fix_job(job: FixJob, hosts: &mut PluginHosts) -> Result<serde_json::Value, SessionError> {
    match job {
        #[cfg(test)]
        FixJob::Panics => panic!("test-armed fix panic"),
        FixJob::Fix { paths, verbosity } => commands::fix_answer(&paths, verbosity, hosts),
        FixJob::Assists {
            path,
            offset,
            length,
            verbosity,
        } => fix::assists(&path, offset, length, hosts)
            .map(|answer| fix::assists_json(&answer, verbosity))
            .map_err(|error| match error {
                DiscoverError::Missing(path) => {
                    SessionError::InvalidParams(format!("Directory or file doesn't exist: {path}"))
                }
                DiscoverError::Io(io) => SessionError::Io(io),
            }),
    }
}

/// The fix worker's body (#947 F5): the job runs inside the same
/// panic-boundary contract as every engine call — a panic answers an
/// internal error and is logged, never kills the thread with the dispatch
/// slot occupied and the host set unmailed. The completion mail goes out in
/// every case, so the slot frees and the hosts come home (ownership of
/// `hosts` stays outside the guarded closure — only its `&mut` borrows in —
/// so even a panic cannot lose the set; a panicking set is torn down
/// kill-only by its `Drop` on the mail's eventual consumption).
fn fix_worker_body(job: FixJob, mut hosts: PluginHosts, done: &Sender<FixMail>) {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_fix_job(job, &mut hosts)
    }))
    .unwrap_or_else(|_| {
        eprintln!("dartq-daemon: fix worker panicked; the dispatch answers internal error");
        Err(SessionError::Internal("the fix dispatch panicked".into()))
    });
    let _ = done.send(FixMail { hosts, outcome });
}

/// The wire form of a request id (the progress notice carries it, so a
/// client can match the notice to its request).
fn id_value(id: &RequestId) -> serde_json::Value {
    match id {
        RequestId::Num(number) => json!(number),
        RequestId::Str(text) => json!(text),
    }
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
    /// The open documents' LRU stamp source ([`Self::next_touch`], #947 F8).
    next_touch: u64,
    /// Open overlay documents: last-known client version per path plus an
    /// LRU touch stamp (bumped on every edit — the eviction scan keeps the
    /// recently-touched documents, #947 F8). A closed or never-opened file
    /// is absent — publishes then carry no version.
    opened: FxHashMap<PathBuf, OpenDocument>,
    /// Baseline for publish diffing: a push only goes out when a file's
    /// diagnostic set differs from what was last published for it.
    published: FxHashMap<dartq_diagnostics::FileId, Vec<lsp::Diagnostic>>,
    /// The workspace assembly's stale-config advisory (#741), pre-built for
    /// the publish channel and taken by the first drain: exactly one push
    /// per session start, never part of the per-file diff baseline (the
    /// config file is not an engine file, so no later cycle can reconcile
    /// it away or republish it).
    workspace_advisory: Option<Publish>,
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
    /// Bounded by [`MAX_DELTA_PATHS`] — past it the oldest cursors evict
    /// and the delta contract sends pre-eviction clients to a re-sync
    /// (#947 F8).
    changed: FxHashMap<PathBuf, u64>,
    /// A base write applied this session whose publish diff has not drained
    /// yet: forces the default context's base analysis even when only
    /// context-tagged or agent requests ride the read, so the diff still
    /// goes out.
    publishes_due: bool,
    /// The session symbol table (short-id economics, PRD §15.8).
    symbols: commands::SymbolTable,
    /// The per-file lint-row memo (#772): rows keyed by content, features,
    /// enable set, package facts, and path, so a quiet turn's lint stage
    /// answers unchanged files from memory and only parses the turn's
    /// dirty set. One per session is sound: `ContentId` is the global
    /// blake3-128 content hash, not per-context sequential, and every
    /// other row input folds into the key — contexts cannot alias.
    lint_memo: dartq_commands::lints::LintMemo,
    /// Shared-store contents at the last overlay-store sweep (the GC
    /// watermark).
    gc_watermark: usize,
    /// Content bytes stored since the last overlay-store sweep — the byte
    /// half of the GC watermark pair (#947 F7): an upper bound on the
    /// store's inflow (identical-content rewrites dedupe, so the true
    /// growth is never larger), reset when the sweep runs.
    gc_bytes_since_sweep: u64,
    /// The byte watermark the sweep fires at ([`GC_BYTES_SINCE_SWEEP`]; the
    /// GC test shrinks it).
    gc_byte_limit: u64,
    /// The two table caps ([`MAX_DELTA_PATHS`] / [`MAX_OPEN_DOCUMENTS`];
    /// the F8 tests shrink them).
    delta_path_cap: usize,
    open_document_cap: usize,
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
    /// The repeat-offender ladder (#690): panics on the plugin-bearing
    /// group — the default context's base view, the only analysis a
    /// plugin's rows ride — counted toward [`PLUGIN_PANIC_LIMIT`]. The
    /// group-level containment bounds each panic to its own group; this
    /// ladder is the session-level answer to one that keeps coming back.
    plugin_path_panics: u32,
    /// The `dartq/fix`/`dartq/assists` host set: `None` until the first fix
    /// worker launches it inside its thread (a session that never fixes
    /// never pays the spawn handshakes, and the main loop never pays them
    /// at all). The set travels into the worker at dispatch and returns
    /// through the completion mail, so it stays one exclusive surface.
    fix_hosts: Option<PluginHosts>,
    /// Staged fix jobs waiting for their drain (one task per request, on
    /// the default context's epoch).
    fixes: Vec<PendingFix>,
    /// The fix dispatch currently running on its worker thread, if any.
    /// One at a time — the host set is one exclusive request surface.
    fix_running: Option<FixRun>,
    /// The fix workers' completion channel. The session owns both ends:
    /// workers get sender clones, the drain collects mails, and the main
    /// loop selects on the receiver so a completion wakes it like intake
    /// (a quiet client still gets its fix answer).
    fix_done: (Sender<FixMail>, Receiver<FixMail>),
    /// The dispatch budget (`FIX_TIME_BUDGET`; test seam overrides it).
    fix_budget: Duration,
    snapshot: SnapshotSink,
}

impl Session {
    /// Assembles the default context from the pre-validated configuration
    /// (the CLI owns SDK and workspace discovery; the daemon is shell only)
    /// and opens the XDG-rooted snapshot store. A missing cache root only
    /// means no persistence (accelerator semantics), never a failed session.
    /// `workspace_advisory` is the assembly's one stale-config advisory (#741);
    /// when present, the session's first drain leads its publishes with the
    /// advisory push.
    ///
    /// # Errors
    ///
    /// [`EngineError::PackageConfig`] when the config cannot back the
    /// URI-resolution table.
    pub fn new(
        config: EngineConfig,
        workspace_advisory: Option<WorkspaceAdvisory>,
    ) -> Result<Self, EngineError> {
        let store = match SnapshotStore::open(SnapshotConfig::default()) {
            Ok(store) => Some(store),
            Err(error) => {
                eprintln!("dartq-daemon: snapshot store unavailable: {error}");
                None
            }
        };
        Self::assemble(config, Vec::new(), store, workspace_advisory)
    }

    /// [`Session::new`] with an explicit snapshot store (tests pin a
    /// throwaway root).
    ///
    /// # Errors
    ///
    /// [`EngineError::PackageConfig`] when the config cannot back the
    /// URI-resolution table.
    pub fn with_store(
        config: EngineConfig,
        store: SnapshotStore,
        workspace_advisory: Option<WorkspaceAdvisory>,
    ) -> Result<Self, EngineError> {
        Self::assemble(config, Vec::new(), Some(store), workspace_advisory)
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
    /// tag re-points at its later config. Results differ from the default
    /// exactly along the fork dimensions: an SDK, target, or declared
    /// variable that changes resolution changes that context's diagnostics
    /// (content itself is shared — a base write fans out to every
    /// context).
    ///
    /// # Advisory ruling
    ///
    /// This constructor takes no [`WorkspaceAdvisory`], deliberately: the
    /// advisory is produced by the shell's single-config workspace
    /// resolution and forwarded by [`Session::new`] — the only production
    /// face that builds sessions today (the CLI daemon shell has no
    /// multi-context flag, so no caller reaches here holding an advisory).
    /// When a multi-context shell face appears, it resolves its advisory
    /// at the shell edge and passes it here — the session-start push (the
    /// exactly-once take in [`Session::service`]) is the default view's
    /// row and stays one per session.
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
        // Advisory ruling: none — see the doc above. The one production
        // construction face (`new`) carries the advisory; this face gains
        // the parameter when a multi-context shell appears.
        Self::assemble(config, extra.into_iter().collect(), Some(store), None)
    }

    fn assemble(
        config: EngineConfig,
        extra: Vec<(String, EngineConfig)>,
        store: Option<SnapshotStore>,
        workspace_advisory: Option<WorkspaceAdvisory>,
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
                let engine = runtime
                    .context_state_mut(id)
                    .expect("the context opened above");
                // The per-runtime budget default (PRD §17.1): every context
                // arms the ladder at assembly; a limit that never bites is
                // indistinguishable from an unlimited run beyond one
                // `Option` test per boundary.
                engine.set_memory_limit(Some(DEFAULT_MEMORY_LIMIT_MIB));
                if let Some(store) = &store {
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
        // The assembly's workspace advisory (#741: the stale-config row, or
        // its #759 missing-config sibling), shaped for the publish channel:
        // an INFO row anchored at the config path — the CLI analyze face's
        // anchor — carrying the assembly's message. The empty range is the
        // row's whole extent: the advisory names a file, not a span.
        let workspace_advisory = workspace_advisory.map(|advisory| Publish {
            uri: uri::from_path(advisory.config_path()),
            version: None,
            diagnostics: vec![lsp::Diagnostic {
                range: lsp::Range {
                    start: lsp::Position {
                        line: 0,
                        character: 0,
                    },
                    end: lsp::Position {
                        line: 0,
                        character: 0,
                    },
                },
                severity: 3,
                code: advisory.code().name.to_owned(),
                source: "dartq".into(),
                message: advisory.message(),
            }],
        });
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
            next_touch: 0,
            opened: FxHashMap::default(),
            published: FxHashMap::default(),
            workspace_advisory,
            agents: FxHashMap::default(),
            worn: None,
            cursor: 0,
            changed: FxHashMap::default(),
            publishes_due: false,
            symbols: commands::SymbolTable::default(),
            lint_memo: dartq_commands::lints::LintMemo::default(),
            gc_watermark: 0,
            gc_bytes_since_sweep: 0,
            gc_byte_limit: GC_BYTES_SINCE_SWEEP,
            delta_path_cap: MAX_DELTA_PATHS,
            open_document_cap: MAX_OPEN_DOCUMENTS,
            staged: Vec::new(),
            write: None,
            reads: Vec::new(),
            cancelled: Vec::new(),
            plugins,
            plugin_path_panics: 0,
            fix_hosts: None,
            fixes: Vec::new(),
            fix_running: None,
            fix_done: unbounded(),
            fix_budget: FIX_TIME_BUDGET,
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
        let touched = self.next_touch();
        self.opened
            .insert(path.clone(), OpenDocument { version, touched });
        self.evict_open_documents();
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

    /// `dartq/fix` / `dartq/assists`: stages one plugin-dispatch job on the
    /// default context's epoch — the same read-task stamping the engine-view
    /// faces take, `Interactive` (a client waits on the answer) — so a
    /// cancel that advances the epoch voids it for free, and a batched write
    /// still applies before it runs. The job executes off the main loop at
    /// the drain (see the module's fix-dispatch notes); the parameters were
    /// decoded and their paths resolved at intake.
    pub(crate) fn queue_fix(&mut self, id: RequestId, job: FixJob) {
        let cookie = self.next_cookie();
        self.runtime
            .enqueue(self.default_context, false, Tier::Interactive, cookie)
            .expect("the default context lives for the session");
        self.fixes.push(PendingFix { id, job, cookie });
    }

    /// `dartq/overlay.set`: stages (merging per path) one agent's content
    /// deltas. Lazy — no analysis runs here; the next request naming the
    /// agent materializes the view. Bumps the delta cursor once.
    ///
    /// Every path is confined to the workspace root before anything lands
    /// ([`confine_to_workspace`] — the write-face trust boundary, #947 F4),
    /// and the agent's post-merge staged set is bounded
    /// ([`MAX_OVERLAY_FILES`] / [`MAX_OVERLAY_BYTES`], #947 F3) and every
    /// agent's combined staging is bounded ([`MAX_OVERLAY_TOTAL_BYTES`],
    /// #962 P3-2). A rejection
    /// is atomic: nothing stages, and a new agent's table entry is not
    /// created.
    ///
    /// # Errors
    ///
    /// [`SessionError::InvalidParams`] when the agent would exceed
    /// [`MAX_AGENTS`] (clear one first), a path escapes the workspace root,
    /// the merged staged set would exceed the per-agent file/byte caps, or
    /// the session-wide staged total would exceed
    /// [`MAX_OVERLAY_TOTAL_BYTES`].
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
        let staged: Vec<(PathBuf, Vec<u8>)> = files
            .into_iter()
            .map(|(path, bytes)| Ok((confine_to_workspace(&self.workspace_root, &path)?, bytes)))
            .collect::<Result<_, SessionError>>()?;
        // The per-agent bounds, computed over the merged view (an incoming
        // path overwrites its staged entry) before anything is inserted.
        let existing = self.agents.get(agent);
        let (mut count, mut total) = existing.map_or((0usize, 0usize), |overlay| {
            (
                overlay.files.len(),
                overlay
                    .files
                    .values()
                    .fold(0usize, |sum, file| sum.saturating_add(file.bytes.len())),
            )
        });
        for (path, bytes) in &staged {
            if let Some(prev) = existing.and_then(|overlay| overlay.files.get(path)) {
                total = total
                    .saturating_sub(prev.bytes.len())
                    .saturating_add(bytes.len());
            } else {
                count += 1;
                total = total.saturating_add(bytes.len());
            }
        }
        if count > MAX_OVERLAY_FILES {
            return Err(SessionError::InvalidParams(format!(
                "agent overlay exceeds the {MAX_OVERLAY_FILES}-file cap ({count}); clear or commit first"
            )));
        }
        if total > MAX_OVERLAY_BYTES {
            return Err(SessionError::InvalidParams(format!(
                "agent overlay exceeds the {} MiB byte cap ({total}); clear or commit first",
                MAX_OVERLAY_BYTES >> 20
            )));
        }
        // The session-wide bound (#962 P3-2), recomputed over every live
        // agent with this request's merged delta applied: staging grows
        // only here, so one check gates all agents and self-heals from any
        // clear, commit, or eviction since the last call. The walk is
        // O(staged files) — lengths only — microseconds at the 256-agent
        // table's cap.
        let others: usize = self
            .agents
            .iter()
            .filter(|(id, _)| id.as_str() != agent)
            .flat_map(|(_, overlay)| overlay.files.values())
            .fold(0usize, |sum, file| sum.saturating_add(file.bytes.len()));
        if others.saturating_add(total) > MAX_OVERLAY_TOTAL_BYTES {
            return Err(SessionError::InvalidParams(format!(
                "session overlay exceeds the {} MiB total cap ({}); clear or commit first",
                MAX_OVERLAY_TOTAL_BYTES >> 20,
                others.saturating_add(total)
            )));
        }
        let entry = self.agents.entry(agent.to_owned()).or_default();
        self.cursor += 1;
        for (path, bytes) in staged {
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
    /// [`SessionError::InvalidParams`] for an unknown agent, or when a
    /// staged path fails the commit-time canonical confinement (#947 F4 —
    /// e.g. a symlink planted in the worktree now points outside the root;
    /// the write itself opens its leaf `O_NOFOLLOW`, so a symlink swapped
    /// in after the check fails the write instead of being followed);
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
        // The clone is bounded by the per-agent byte cap (#947 F3), so the
        // 2× staged-bytes transient peak is small by construction.
        let committed: Vec<(PathBuf, Vec<u8>)> = overlay
            .files
            .iter()
            .map(|(path, file)| (path.clone(), file.bytes.clone()))
            .collect();
        // The write boundary re-checks the confinement canonically (#947
        // F4): staging validated the paths lexically, but a symlink planted
        // in the worktree since could aim a write outside the root.
        let canonical_root = fs::canonicalize(&self.workspace_root)?;
        for (path, _) in &committed {
            ensure_canonical_child(&canonical_root, path)?;
        }
        // Write-back first: nothing merges until every file is on disk.
        for (path, bytes) in &committed {
            write_committed(path, bytes)?;
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
    /// re-stamped at the fresh epoch. When the cancelled request rode the
    /// default context, every task the bump voided re-stamps through
    /// [`Session::restamp_default_tasks`] — the surviving read, the staged
    /// write, and the staged fix jobs (a write survives too: a zero-request
    /// read joins it, so the publish diff the cancelled read was carrying
    /// still runs). Cancels on other contexts' reads re-stamp that context's
    /// surviving read alone — its write and fix jobs ride the default
    /// context's epoch, which the bump never touched.
    ///
    /// Cancelling an unknown or already-answered id is ignored (LSP
    /// §cancelRequest) — a duplicate cancel of a running dispatch whose
    /// answer is still queued counts as already-answered. The staged overlay
    /// edits are *not* cancellable (LSP notifications carry no ids).
    ///
    /// # Panics
    ///
    /// Never in practice: the re-stamps enqueue against open session
    /// contexts, which live for the session.
    pub fn cancel_request(&mut self, id: &RequestId) {
        // A staged fix cancels like a read: the id answers `Cancelled`, the
        // job never dispatches, and the epoch bump voids the default
        // context's queued tasks — the survivors re-stamp below.
        if let Some(position) = self.fixes.iter().position(|pending| pending.id == *id) {
            self.fixes.remove(position);
            self.cancelled.push(id.clone());
            let _ = self.runtime.cancel(self.default_context);
            self.restamp_default_tasks();
            return;
        }
        // A running dispatch cannot be stopped (the worker owns the host
        // set): the late completion drops and the id answers `Cancelled` at
        // the next drain. One already answered past the budget is left
        // alone, and so is a duplicate cancel of the still-queued answer —
        // one entry in `cancelled` per id, or `service()` would answer it
        // twice.
        if let Some(run) = &mut self.fix_running
            && run.id == *id
        {
            if !run.expired && !run.cancelled {
                run.cancelled = true;
                self.cancelled.push(id.clone());
            }
            return;
        }
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
        // The survivor read re-joins the queue before the re-stamp (its
        // stale cookie is replaced there, its surviving requests kept); on
        // the default context a zero-request read still joins, carrying the
        // publish diff the cancelled read owned.
        let context = pending.context;
        let has_survivors = !pending.requests.is_empty();
        if has_survivors || (context == self.default_context && self.write.is_some()) {
            self.reads.push(pending);
        }
        if context == self.default_context {
            // The bump voided every default-context stamp — the staged fix
            // jobs' and the write's included. Left stamped at the dead
            // epoch, a staged fix would drop silently at the drain: never
            // dispatch, never start its budget, never answer, and hold its
            // `fixes` entry forever. The shared restamp re-enqueues them
            // all at the fresh epoch.
            self.restamp_default_tasks();
        } else if has_survivors {
            // Other contexts' tasks keep their (still current) stamps; only
            // the surviving read re-stamps, keeping its tier.
            let index = self.reads.len() - 1;
            let tier = self.reads[index].tier;
            let cookie = self.next_cookie();
            self.runtime
                .enqueue(context, false, tier, cookie)
                .expect("the read's context lives for the session; read re-stamp cannot fail");
            self.reads[index].cookie = cookie;
        }
    }

    /// Re-enqueues every default-context task still staged after a cancel
    /// bumped the epoch — the surviving fix jobs, the coalesced read (its
    /// requests and tier kept), and the pending write. The fresh stamps are
    /// what lets them drain; the bump voided everything enqueued under the
    /// old one. Other contexts' tasks keep their (still current) stamps.
    ///
    /// The single restamp authority for a default-context cancel: both
    /// cancel arms (staged fix, coalesced read) route through here, so no
    /// task class can be left holding a dead epoch's stamp.
    fn restamp_default_tasks(&mut self) {
        for mut pending in std::mem::take(&mut self.fixes) {
            pending.cookie = self.next_cookie();
            self.runtime
                .enqueue(
                    self.default_context,
                    false,
                    Tier::Interactive,
                    pending.cookie,
                )
                .expect("the default context lives for the session; fix re-stamp cannot fail");
            self.fixes.push(pending);
        }
        for index in 0..self.reads.len() {
            if self.reads[index].context != self.default_context {
                continue;
            }
            let cookie = self.next_cookie();
            self.runtime
                .enqueue(self.default_context, false, self.reads[index].tier, cookie)
                .expect("the default context lives for the session; read re-stamp cannot fail");
            self.reads[index].cookie = cookie;
        }
        if self.write.is_some() {
            let cookie = self.next_cookie();
            self.runtime
                .enqueue(self.default_context, true, Tier::Normal, cookie)
                .expect("the default context lives for the session; write re-stamp cannot fail");
            self.write = Some(cookie);
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
        // Paths this turn's write removed (empty on every write-free turn):
        // their publish-baseline entries drop once this turn's diff has run.
        let mut removed: Vec<PathBuf> = Vec::new();
        // The session-start advisory (#741): leads the first drain's
        // publishes — the CLI faces render the advisory ahead of the rows —
        // and `take` keeps it exactly-once. Every turn-producing path runs
        // through here, so the push rides whichever intake wakes the loop
        // first.
        if let Some(advisory) = self.workspace_advisory.take() {
            turn.publishes.push(advisory);
        }
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
                    #[cfg(test)]
                    if crate::probe::take_write_panic() {
                        panic!("test-armed write panic");
                    }
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
                    Ok(()) => {
                        self.snapshot.note_apply();
                        self.cursor += 1;
                        for change in &changes {
                            match change {
                                OverlayChange::Set { path, bytes } => {
                                    // The store's inflow tally (#947 F7):
                                    // identical content dedupes on put, so
                                    // this is an upper bound on real growth.
                                    self.gc_bytes_since_sweep += bytes.len() as u64;
                                    self.changed.insert(path.clone(), self.cursor);
                                }
                                OverlayChange::Remove { path } => {
                                    // The file left the view: its delta cursor
                                    // goes with it (a later re-creation
                                    // re-inserts through the Set arm), and the
                                    // path joins the turn's removals for the
                                    // publish-baseline drop below.
                                    self.changed.remove(path);
                                    removed.push(path.clone());
                                }
                            }
                        }
                        self.evict_delta_paths();
                        self.publishes_due = true;
                    }
                    Err(_) => {
                        eprintln!(
                            "dartq-daemon: overlay apply panicked; the write did not land; \
                             the edits are restaged for the next write task"
                        );
                        // Re-stage the taken changes (last-wins merge with
                        // anything a new edit stages) WITHOUT re-enqueueing
                        // the write task: the next real intake re-queues via
                        // ensure_write and applies the merged latest bytes.
                        // Re-enqueueing here would retry the same panicked
                        // apply every drain — a deterministic panic must not
                        // become a self-driven loop.
                        for change in changes {
                            self.stage(change);
                        }
                    }
                }
            } else if let Some(position) = self
                .fixes
                .iter()
                .position(|pending| pending.cookie == task.cookie)
            {
                // The fix job's epoch verified — its batch's writes applied
                // before it. The job runs off the main loop (see the
                // module's fix-dispatch notes): the dispatch moves to the
                // worker when the single slot is free, and re-stages
                // otherwise — it pops again at the next drain (the
                // completion's or the next batch's) and dispatches then.
                if self.fix_running.is_none() {
                    let pending = self.fixes.remove(position);
                    self.dispatch_fix(pending, &mut turn);
                } else {
                    let cookie = self.next_cookie();
                    self.runtime
                        .enqueue(self.default_context, false, Tier::Interactive, cookie)
                        .expect("the default context lives for the session");
                    self.fixes[position].cookie = cookie;
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
        // The turn's removals leave the publish baseline too: writes apply
        // before reads, so this turn's diff already observed each removal —
        // emitting the clearing push for a diagnosed file — and a file that
        // no longer exists must not keep a baseline entry (watch churn would
        // otherwise accumulate one per deleted file for the session's life).
        // A turn without removals drops nothing.
        for path in removed {
            if let Some(file) = self
                .engine(self.default_context)
                .file_table()
                .try_file_id(&path)
            {
                self.published.remove(&file);
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
        // A completion that landed before teardown reclaims the host set
        // for the graceful handshake; one still in flight returns to a dead
        // receiver and tears down kill-only when the worker finishes (every
        // round trip is manifest-budgeted, so the worker is bounded).
        if let Ok(mail) = self.fix_done.1.try_recv() {
            self.fix_hosts = Some(mail.hosts);
        }
        if let Some(hosts) = self.fix_hosts.take() {
            hosts.shutdown();
        }
    }

    /// Moves one staged fix job onto the worker thread (the single
    /// dispatch slot was free). The host set travels into the worker —
    /// launched there on first use, so the main loop never pays the
    /// handshakes — and returns through the completion mail. The progress
    /// notice rides the turn's notification pipeline. A spawn refusal (only
    /// under resource exhaustion) answers the request here: the failed
    /// closure drops, tearing its host set down kill-only, and nothing ran.
    fn dispatch_fix(&mut self, pending: PendingFix, turn: &mut Turn) {
        let root = self.workspace_root.clone();
        let done = self.fix_done.0.clone();
        let id = pending.id.clone();
        let method = pending.job.method();
        let job = pending.job;
        let hosts = self.fix_hosts.take();
        let spawned = thread::Builder::new()
            .name("dartq-fix".into())
            .spawn(move || {
                fix_worker_body(
                    job,
                    hosts.unwrap_or_else(|| PluginHosts::launch(&root)),
                    &done,
                );
            });
        match spawned {
            Ok(_) => {
                turn.notifications.push(Notification::new(
                    "dartq/fixProgress",
                    json!({ "id": id_value(&id), "method": method }),
                ));
                self.fix_running = Some(FixRun {
                    id,
                    deadline: Instant::now() + self.fix_budget,
                    expired: false,
                    cancelled: false,
                });
            }
            Err(error) => {
                eprintln!("dartq-daemon: fix worker spawn failed: {error}");
                turn.answers.push((
                    id,
                    Err(SessionError::Internal(
                        "fix dispatch failed to start".into(),
                    )),
                ));
            }
        }
    }

    /// The fix dispatch's completion drain: fires the budget expiry,
    /// collects the worker's mail (`mail` is the select-received one;
    /// `try_recv` picks up any behind it), and — when a completion or the
    /// expiry moved state — drains the session, so the answers flow, staged
    /// work publishes, and the next staged fix dispatches. The quiet main
    /// loop calls this with `None` every iteration; with nothing to do the
    /// call is a no-op.
    pub(crate) fn complete_fix(&mut self, mail: Option<FixMail>) -> Turn {
        let mut turn = Turn::default();
        let mut moved = false;
        // The budget ceiling answers even though the worker still owns the
        // hosts; the slot frees when the late mail lands (it is discarded —
        // no second answer for one id).
        if let Some(run) = &mut self.fix_running
            && !run.expired
            && Instant::now() >= run.deadline
        {
            run.expired = true;
            moved = true;
            turn.answers.push((
                run.id.clone(),
                Err(SessionError::Internal(format!(
                    "the fix dispatch exceeded its budget ({:?})",
                    self.fix_budget
                ))),
            ));
        }
        if let Some(mail) = mail {
            self.absorb_fix(mail, &mut turn, &mut moved);
        }
        while let Ok(mail) = self.fix_done.1.try_recv() {
            self.absorb_fix(mail, &mut turn, &mut moved);
        }
        if moved {
            // The completion wakes the loop like a batch arrival: the drain
            // flushes whatever staged (publishes, cancelled answers) and
            // dispatches the next fix.
            let drained = self.service();
            turn.publishes.extend(drained.publishes);
            turn.notifications.extend(drained.notifications);
            turn.answers.extend(drained.answers);
        }
        turn
    }

    /// One completion mail: the host set comes home (the slot frees), and
    /// the answer goes out unless the budget already answered it or the
    /// client cancelled the request.
    fn absorb_fix(&mut self, mail: FixMail, turn: &mut Turn, moved: &mut bool) {
        *moved = true;
        self.fix_hosts = Some(mail.hosts);
        if let Some(run) = self.fix_running.take()
            && !run.expired
            && !run.cancelled
        {
            turn.answers.push((run.id, mail.outcome));
        }
    }

    /// The running dispatch's budget ceiling. `None` once the expiry
    /// answered — nothing is left to time out, and the late mail wakes the
    /// loop through the mailbox.
    pub(crate) fn fix_deadline(&self) -> Option<Instant> {
        self.fix_running
            .as_ref()
            .filter(|run| !run.expired)
            .map(|run| run.deadline)
    }

    /// The fix workers' completion mailbox: the main loop selects on it
    /// next to intake, so a completion wakes a quiet loop.
    pub(crate) fn fix_mailbox(&self) -> &Receiver<FixMail> {
        &self.fix_done.1
    }

    /// The dispatch budget seam (the slow-plugin tests shrink it).
    #[cfg(test)]
    fn set_fix_budget(&mut self, budget: Duration) {
        self.fix_budget = budget;
    }

    /// The GC byte-watermark seam (the F7 test shrinks it).
    #[cfg(test)]
    fn set_gc_byte_limit(&mut self, limit: u64) {
        self.gc_byte_limit = limit;
    }

    /// Content bytes stored since the last sweep (the F7 test's probe:
    /// zero means a sweep ran since the last crossing).
    #[cfg(test)]
    fn gc_bytes_since_sweep(&self) -> u64 {
        self.gc_bytes_since_sweep
    }

    /// The two table caps' seam (the F8 tests shrink them).
    #[cfg(test)]
    fn set_table_caps(&mut self, delta_paths: usize, open_documents: usize) {
        self.delta_path_cap = delta_paths;
        self.open_document_cap = open_documents;
    }

    /// Replaces the session's plugin set (test seam): lets a gate inject a
    /// crafted set — e.g. an already-dead plugin — without going through
    /// the launch path's real child processes. Call before the first drain.
    #[cfg(test)]
    fn inject_plugins(&mut self, set: PluginSet) {
        self.plugins = set;
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
        let mut groups: Vec<ViewGroup> = Vec::new();
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
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.run_group(self.default_context, None, Vec::new(), turn);
            }));
            match outcome {
                // The drain happened (run_group clears `publishes_due` at
                // the publish plan), or the task cancelled — the next write
                // re-arms the diff anyway.
                Ok(()) => self.publishes_due = false,
                Err(_) => {
                    eprintln!(
                        "dartq-daemon: base analysis panicked; the publish diff retries on \
                         the next intake"
                    );
                    self.note_plugin_path_panic(pending.context, None);
                    self.restore_base_view_contained();
                }
            }
        }

        // Containment is per group (#690): a panicking group answers its
        // own requests `INTERNAL_ERROR` and hands the next group a
        // restored base view; the turn's remaining groups still analyze
        // and answer. One group's panic — engine or plugin — never
        // converts the rest of the turn into internal errors or defers
        // the publish diff.
        for (agent, requests) in groups {
            let ids: Vec<RequestId> = requests.iter().map(|(id, _)| id.clone()).collect();
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.run_group(pending.context, agent.as_deref(), requests, turn);
            }));
            if outcome.is_err() {
                eprintln!("dartq-daemon: analysis panicked; answering the group internal error");
                Self::answer_panicked(turn, ids);
                self.note_plugin_path_panic(pending.context, agent.as_deref());
                self.restore_base_view_contained();
            }
        }
    }

    /// Answers a panicking group's ids `INTERNAL_ERROR` (the catch
    /// boundary's degraded answer).
    fn answer_panicked(turn: &mut Turn, ids: Vec<RequestId>) {
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

        let Ok(mut diagnostics) = self.analyze_with_lints(context) else {
            for (id, _) in requests {
                turn.answers.push((id, Err(SessionError::Cancelled)));
            }
            return;
        };
        let mut plugin_findings = Vec::new();
        if self.carries_plugins(context, agent) {
            plugin_findings = self.weave_plugins(context, &mut diagnostics);
        }
        // The one conversion pass per group (the engine→LSP shape change is
        // the expensive half — #347's convert-once discipline, per view):
        // the publish plan and the analyze entries share it.
        let converted: Vec<Option<lsp::Diagnostic>> = diagnostics
            .iter()
            .map(|diagnostic| self.convert(context, diagnostic))
            .collect();
        if context == self.default_context && agent.is_none() {
            self.publish_plan(&diagnostics, &converted, plugin_findings, turn);
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

    /// The group's analysis: the engine turn plus the resident files' lint
    /// rows (#355) — memoized (#772), so a quiet turn parses only the
    /// turn's changed files — joined before the caller's severity-first
    /// sort so the answers keep the CLI analyze face's order. The context
    /// engine is read through `runtime` directly so the memo field borrows
    /// disjointly.
    fn analyze_with_lints(&mut self, context: ContextId) -> Result<Vec<Diagnostic>, Cancelled> {
        let mut diagnostics = self.analyze(context)?;
        let lint_roots = [self.workspace_root().to_path_buf()];
        let engine = self
            .runtime
            .context_state(context)
            .expect("session contexts live for the session");
        diagnostics.extend(dartq_commands::lints::analyze_lints(
            engine,
            &lint_roots,
            &mut self.lint_memo,
        ));
        Ok(diagnostics)
    }

    /// The plugin weave (#471 phase B) for the publish-bearing group: the
    /// plugins feed once per base analysis — the editor's view — never per
    /// agent view or forked context. The cycle runs before the
    /// convert-once pass so a crash row converts with the engine rows, and
    /// findings resolve against the same analyzed view; the caller merges
    /// them into the publish plan (engine rows first, plugin groups by
    /// discovery order, dedup on the four-key). A crash whose anchor does
    /// not exist yet — the session has published nothing — is withheld by
    /// the cycle, not dropped: it publishes on the first cycle that has
    /// one. Contained a level finer than the group (#690): this is the
    /// group's only plugin-row exposure, so a panic here is attributable
    /// and penalizes immediately — while the group's engine rows still
    /// convert, publish, and answer. The skipped cycle's rows re-feed on
    /// the next cycle; a crash recorded by the penalty surfaces there
    /// through the normal one-shot path.
    fn weave_plugins(
        &mut self,
        context: ContextId,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Vec<plugins::PluginFinding> {
        let anchor: Option<FileId> = self.published.keys().copied().min();
        // The governing root options file, resolved before the closure: the
        // resolver reads the workspace root through a whole-`self` method,
        // which would defeat the closure's disjoint field capture (the
        // plugins' mutable borrow must stay disjoint from the engine's
        // read). Returns an owned path, so the borrow ends here.
        let options_file =
            dartq_engine::analysis_options::AnalysisOptions::find_for_dir(self.workspace_root());
        // Direct field access (not `self.engine`): the plugins' mutable
        // borrow and the engine's read must stay disjoint field borrows.
        let engine = self
            .runtime
            .context_state(context)
            .expect("the read task's context lives for the session");
        let mut plugin_findings = Vec::new();
        let woven = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            #[cfg(test)]
            if crate::probe::take_weave_panic() {
                panic!("test-armed plugin weave panic");
            }
            // The cycle context: the analyzed set (the context's resident
            // libraries — the files the plugin's rules run over), the
            // client's unsaved text for the opened documents among them
            // (the overlays; unopened files read from disk), and the
            // governing root options file (the `plugins:` rule configs).
            // The enumeration runs only when a plugin set exists; the vec
            // build is the per-cycle cost of hosting rules.
            let analyzed: Vec<String> = engine
                .libraries()
                .into_iter()
                .flat_map(|(_, files)| files)
                .filter_map(|file| {
                    engine
                        .file_table()
                        .path(file)
                        .map(|path| path.to_string_lossy().into_owned())
                })
                .collect();
            let overlays: Vec<(String, String)> = self
                .opened
                .keys()
                .filter_map(|path| {
                    let path_string = path.to_string_lossy().into_owned();
                    let file = engine.file_table().try_file_id(path)?;
                    let text = engine.content(file)?;
                    Some((
                        path_string,
                        std::str::from_utf8(text)
                            .expect("engine content is valid UTF-8")
                            .to_owned(),
                    ))
                })
                .collect();
            let options = options_file.as_deref();
            // The T4 reverse-query server: the plugins' `request/query`
            // pulls answer against this context's engine view — the same
            // analyzed state the cycle's findings resolve against.
            let query_server =
                |query: &dartq_plugin::PendingQuery| plugins::serve_query(engine, query);
            let cycle_context = plugins::CycleContext {
                analyzed: &analyzed,
                overlays: &overlays,
                options_file: options,
                query_server: Some(&query_server),
            };
            let cycle =
                self.plugins
                    .cycle(diagnostics, engine.file_table(), anchor, &cycle_context);
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
        }));
        if woven.is_err() {
            eprintln!("dartq-daemon: the plugin cycle panicked; this cycle's findings are skipped");
            self.penalize_plugins("a panic in the plugin cycle");
        }
        plugin_findings
    }

    /// The base group's publish plan: engine rows merged with the plugin
    /// findings into the turn's publishes. The crash row is one-shot: it
    /// rides this push but never the published-state baseline, or every
    /// later clean cycle would republish the file to reconcile it away. The
    /// marker is code+source — plugin codes are free-form, so a plugin
    /// emitting the crash code under its own source keeps its rows and its
    /// baseline. The diff drains here: future writes re-arm it. Kept after
    /// the plan so a panic mid-plan leaves the diff due — the next intake
    /// retries it (the containment posture, #690).
    fn publish_plan(
        &mut self,
        diagnostics: &[Diagnostic],
        converted: &[Option<lsp::Diagnostic>],
        plugin_findings: Vec<plugins::PluginFinding>,
        turn: &mut Turn,
    ) {
        let engine_rows =
            diagnostics
                .iter()
                .zip(converted.iter())
                .filter_map(|(diagnostic, converted)| {
                    converted.clone().map(|row| (diagnostic.file, row))
                });
        let merged = plugins::merge(engine_rows, plugin_findings);
        let retained: Vec<_> = merged
            .iter()
            .filter(|(_, row)| !Self::is_engine_crash_row(row))
            .cloned()
            .collect();
        turn.publishes.extend(self.plan_publishes(merged, retained));
        self.publishes_due = false;
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
            let results = thread::scope(|scope| {
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
        for (index, id, request) in receiver {
            let answer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                commands::answer(drain, &request)
            }))
            .unwrap_or_else(|_| {
                eprintln!(
                    "dartq-daemon: answer worker panicked; answering the request internal error"
                );
                Err(SessionError::Internal(
                    "internal error: analysis panicked".into(),
                ))
            });
            results.push((index, id, answer));
        }
        results
    }

    /// True when this view group is the plugin-bearing one: the default
    /// context's base view — the only analysis the plugin weave (and thus a
    /// plugin's rows) rides. Agent views and forked contexts never see it.
    fn carries_plugins(&self, context: ContextId, agent: Option<&str>) -> bool {
        context == self.default_context && agent.is_none() && !self.plugins.is_empty()
    }

    /// One panic on the plugin-bearing group (#690's repeat-offender
    /// ladder): counted toward [`PLUGIN_PANIC_LIMIT`], never reset — the
    /// transport's restart budget never resets either (PRD §14.3), and a
    /// panic that recurs is deterministic enough to re-fire every cycle.
    /// At the limit the whole plugin set is disabled for the session: the
    /// group-level containment bounds each panic to its own group, this is
    /// the session-level answer to one that keeps coming back. The panic
    /// may be the engine's, not a plugin's — with attribution below group
    /// granularity unavailable cheaply, disabling the set is the accepted
    /// conservative posture (the audit's fallback trigger).
    fn note_plugin_path_panic(&mut self, context: ContextId, agent: Option<&str>) {
        if !self.carries_plugins(context, agent) {
            return;
        }
        self.plugin_path_panics = self.plugin_path_panics.saturating_add(1);
        if self.plugin_path_panics >= PLUGIN_PANIC_LIMIT {
            self.penalize_plugins(&format!("{PLUGIN_PANIC_LIMIT} repeated analysis panics"));
        }
    }

    /// Disables the plugin set for the session: each live plugin drops
    /// kill-only (the restart-budget-exhaustion posture) and emits its
    /// one-shot `dartq/plugin_crashed` row on the next cycle through the
    /// crash machinery — anchored, withheld until an anchor exists, never
    /// repeated. Logged once; later calls find nothing live.
    fn penalize_plugins(&mut self, cause: &str) {
        if self.plugins.disable_for_session(cause) {
            eprintln!("dartq-daemon: every plugin disabled for the session: {cause}");
        }
    }

    /// The post-panic base restore, contained (the module's containment
    /// contract): the engine state is untrusted at this point, so a restore
    /// that itself panics must not escape the turn and kill the remaining
    /// groups — it is logged and the turn moves on.
    fn restore_base_view_contained(&mut self) {
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.restore_base_view()));
        if outcome.is_err() {
            eprintln!("dartq-daemon: the post-panic base restore panicked; continuing without it");
        }
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
    /// last sweep crossed a watermark (daemon-audit finding 2: without a
    /// driver the store accumulates every distinct edit state for the
    /// session's life): the entry count (small documents) or the byte tally
    /// (#947 F7 — a few huge documents must not sit under the entry
    /// threshold). O(live), so idle sessions pay nothing.
    fn gc_if_due(&mut self) {
        let contents = self.runtime.stored_contents();
        if self.gc_bytes_since_sweep >= self.gc_byte_limit
            || contents.saturating_sub(self.gc_watermark) >= GC_CONTENTS_THRESHOLD
        {
            self.runtime.sweep();
            self.gc_watermark = self.runtime.stored_contents();
            self.gc_bytes_since_sweep = 0;
        }
    }

    /// The delta-cursor table's cap eviction (#947 F8): past the cap, keep
    /// the newest 7/8 by cursor and drop the rest — the rows only a client
    /// paging from a very old `since` could still ask about. An evicted
    /// path filters out of delta answers like a never-changed file; the
    /// contract sends such clients to a re-sync. O(n log n) per trigger,
    /// one trigger per cap/8 insertions — amortized O(log n) per write.
    fn evict_delta_paths(&mut self) {
        let keep = self.delta_path_cap * 7 / 8;
        if self.changed.len() <= self.delta_path_cap {
            return;
        }
        let mut by_cursor: Vec<(u64, PathBuf)> = self
            .changed
            .iter()
            .map(|(path, &cursor)| (cursor, path.clone()))
            .collect();
        by_cursor.sort_unstable();
        for (_, path) in &by_cursor[..by_cursor.len() - keep] {
            self.changed.remove(path);
        }
    }

    /// The open-document table's cap eviction (#947 F8): past the cap, keep
    /// the 7/8 most-recently-touched documents and drop the rest. Only a
    /// broken client's open storm reaches here (real clients hold tens of
    /// editors); an evicted document degrades to on-disk truth — publishes
    /// lose the version and a watch sync may resync its disk bytes.
    fn evict_open_documents(&mut self) {
        let keep = self.open_document_cap * 7 / 8;
        if self.opened.len() <= self.open_document_cap {
            return;
        }
        let mut by_touch: Vec<(u64, PathBuf)> = self
            .opened
            .iter()
            .map(|(path, document)| (document.touched, path.clone()))
            .collect();
        by_touch.sort_unstable();
        for (_, path) in &by_touch[..by_touch.len() - keep] {
            self.opened.remove(path);
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

    /// The open documents' LRU clock (#947 F8): one stamp per edit, so the
    /// cap eviction's scan always has a recency order to keep.
    fn next_touch(&mut self) -> u64 {
        self.next_touch = self.next_touch.wrapping_add(1);
        self.next_touch
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

    /// The one-shot plugin crash row's exact marker: the crash CODE under the
    /// engine's own `source` tag (the convert pass stamps every engine row
    /// `dartq`). Code alone is not exact — plugin codes are free-form, so a
    /// plugin emitting the crash code under its own source must keep its rows
    /// in the retained baseline.
    fn is_engine_crash_row(row: &lsp::Diagnostic) -> bool {
        row.code == dartq_diagnostics::DARTQ_PLUGIN_CRASHED.name && row.source == "dartq"
    }

    /// Diffs the fresh row set against the published baseline and returns
    /// one [`Publish`] per changed file (empty set included — the client
    /// clears its squiggles through it). `merged` is the view's full row
    /// set — engine rows plus the cycle's plugin findings, already merged
    /// and deduplicated ([`plugins::merge`]). Order is by URI, so the push
    /// sequence is deterministic.
    /// Plans the publish pushes for one base analysis: diffs the merged
    /// rows against the published-state baseline and emits one push per
    /// changed file (plus baseline files whose rows vanished entirely).
    ///
    /// `retained` is the state future cycles reproduce — recorded as the
    /// new baseline. It differs from `merged` exactly on one-shot rows
    /// (the plugin crash row): those ride the push but never the baseline,
    /// so the next clean cycle diffs quiet against the retained state
    /// instead of reconciling the one-shot row away (which would
    /// republish the file forever after).
    fn plan_publishes(
        &mut self,
        merged: Vec<(FileId, lsp::Diagnostic)>,
        retained: Vec<(FileId, lsp::Diagnostic)>,
    ) -> Vec<Publish> {
        let mut current: FxHashMap<dartq_diagnostics::FileId, Vec<lsp::Diagnostic>> =
            FxHashMap::default();
        for (file, diagnostic) in merged {
            current.entry(file).or_default().push(diagnostic);
        }
        let mut record: FxHashMap<dartq_diagnostics::FileId, Vec<lsp::Diagnostic>> =
            FxHashMap::default();
        for (file, diagnostic) in retained {
            record.entry(file).or_default().push(diagnostic);
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
            let baseline = match record.entry(file) {
                Entry::Occupied(occupied) => occupied.remove(),
                Entry::Vacant(_) => Vec::new(),
            };
            self.published.insert(file, baseline);
            publishes.push(Publish {
                uri: uri::from_path(&path),
                version: self.opened.get(&path).map(|document| document.version),
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
            message: diagnostic.message.clone().into_owned(),
        })
    }
}

/// Confines an RPC-supplied write path to the workspace root (#947 F4):
/// relative paths join the root, then the root prefix is stripped
/// component-wise and the remainder resolved lexically — any path outside
/// the root (an absolute path elsewhere, or enough `..` to climb out) is
/// rejected. The overlay stores the returned normalized absolute path, so
/// the commit-side check ([`ensure_canonical_child`]) never sees a `..`
/// component.
fn confine_to_workspace(root: &Path, path: &Path) -> Result<PathBuf, SessionError> {
    let escape = || {
        SessionError::InvalidParams(format!(
            "overlay path escapes the workspace root: {}",
            path.display()
        ))
    };
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    // Component-wise prefix strip, so a sibling directory can never pass by
    // sharing a textual prefix with the root.
    let Ok(rest) = joined.strip_prefix(root) else {
        return Err(escape());
    };
    // Resolve the remainder lexically; a `..` at its floor is the escape.
    let mut stack: Vec<Component<'_>> = Vec::new();
    for component in rest.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if stack.pop().is_none() {
                    return Err(escape());
                }
            }
            component => stack.push(component),
        }
    }
    let mut confined = root.to_path_buf();
    for component in stack {
        confined.push(component.as_os_str());
    }
    Ok(confined)
}

/// The commit-time half of the write confinement (#947 F4, belt and
/// braces): staging validated the path lexically, but a symlink planted in
/// the worktree between staging and commit could aim the write outside.
/// Canonicalizes the deepest existing ancestor of `path` (following
/// symlinks), re-joins the not-yet-existing tail (staged paths carry no
/// `..`, so the tail cannot re-enter an escape), and requires the result to
/// stay under the already-canonicalized `canonical_root`.
fn ensure_canonical_child(canonical_root: &Path, path: &Path) -> Result<(), SessionError> {
    let mut ancestor = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let canonical = loop {
        match fs::canonicalize(&ancestor) {
            Ok(canonical) => break canonical,
            Err(error) => {
                let Some(name) = ancestor.file_name() else {
                    return Err(SessionError::Io(error));
                };
                tail.push(name.to_os_string());
                if !ancestor.pop() {
                    return Err(SessionError::Io(error));
                }
            }
        }
    };
    let mut full = canonical;
    for name in tail.into_iter().rev() {
        full.push(name);
    }
    if full.starts_with(canonical_root) {
        Ok(())
    } else {
        Err(SessionError::InvalidParams(format!(
            "overlay path escapes the workspace root: {}",
            path.display()
        )))
    }
}

/// The committed buffer's disk write (#962 P3-3): the leaf open carries
/// `O_NOFOLLOW` where the platform defines it. The canonical confinement
/// check and this open are separate filesystem operations, and a plain
/// open follows a symlink swapped onto the final component between them —
/// the buffer would land wherever it aims, inside or outside the root.
/// `O_NOFOLLOW` moves the rejection into the open itself, closing the
/// race the best userspace re-check could only narrow. Parent components
/// stay under the confinement walk: a mid-path swap inside the remaining
/// microsecond window is out of model for the local single-operator trust
/// domain (eliminating it needs per-component `openat`, beyond std).
#[cfg(unix)]
fn write_committed(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .custom_flags(LEAF_OPEN_FLAGS)
        .open(path)?
        .write_all(bytes)
}

/// The non-unix fallback: a plain create-truncate write (the confinement
/// check remains the gate).
#[cfg(not(unix))]
fn write_committed(path: &Path, bytes: &[u8]) -> io::Result<()> {
    fs::write(path, bytes)
}

/// The leaf-open flags for [`write_committed`]: `O_NOFOLLOW` where this
/// exact value is the platform's (xnu `sys/fcntl.h`; the Linux uapi
/// `asm-generic/fcntl.h` generic bits), zero — no extra flags — elsewhere.
/// mips/sparc/alpha Linux define different values and get zero rather
/// than a wrong bit.
#[cfg(target_os = "macos")]
const LEAF_OPEN_FLAGS: i32 = 0o100;
#[cfg(all(
    target_os = "linux",
    any(
        target_arch = "x86",
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "arm",
        target_arch = "riscv64",
        target_arch = "loongarch64"
    )
))]
const LEAF_OPEN_FLAGS: i32 = 0o400000;
#[cfg(not(any(
    target_os = "macos",
    all(
        target_os = "linux",
        any(
            target_arch = "x86",
            target_arch = "x86_64",
            target_arch = "aarch64",
            target_arch = "arm",
            target_arch = "riscv64",
            target_arch = "loongarch64"
        )
    )
)))]
const LEAF_OPEN_FLAGS: i32 = 0;

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
    use dartq_commands::workspace::StaleConfigAdvisory;
    use dartq_pub::StaleReason;
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
        let session = Session::with_store(workspace.engine_config(), store, None).expect("session");
        (workspace, session)
    }

    /// The stale-config advisory (#741): a session assembled with the
    /// advisory leads its first drain with exactly one publish anchored at
    /// the config path — INFO severity, the dartq/* code, the assembly's
    /// message, no version (the config file is never client-opened) — and
    /// no later drain repeats it; an assembly without the advisory never
    /// publishes the config URI.
    #[test]
    fn stale_config_advisory_publishes_once_at_session_start() {
        let workspace = Workspace::new(BROKEN_MAIN);
        let config_path = workspace.path(".dart_tool/package_config.json");
        let advisory = WorkspaceAdvisory::Stale(StaleConfigAdvisory {
            reasons: vec![StaleReason::MissingLockFile],
            config_path: config_path.clone(),
        });
        let store = SnapshotStore::open(SnapshotConfig {
            root: Some(store_root("stale-advisory-stale")),
            capacity_bytes: DEFAULT_CAPACITY_BYTES,
        })
        .expect("store opens on a temp root");
        let mut session =
            Session::with_store(workspace.engine_config(), store, Some(advisory)).expect("session");

        session.sweep().expect("sweep");
        let turn = session.service();
        let config_uri = uri::from_path(&config_path);
        let pushes: Vec<_> = turn
            .publishes
            .iter()
            .filter(|push| push.uri == config_uri)
            .collect();
        assert_eq!(pushes.len(), 1, "exactly one advisory push");
        assert_eq!(
            turn.publishes[0].uri, config_uri,
            "the advisory leads the turn's publishes"
        );
        let push = pushes[0];
        assert_eq!(push.version, None, "the config file is not client-opened");
        assert_eq!(push.diagnostics.len(), 1, "one advisory row");
        let row = &push.diagnostics[0];
        assert_eq!(row.severity, 3, "INFO — the registry row's severity");
        assert_eq!(row.code, "dartq/package_config_out_of_date");
        assert_eq!(row.source, "dartq");
        assert_eq!(row.range.start.line, 0, "the row names a file, not a span");
        assert!(
            row.message.contains("pubspec.lock is missing"),
            "the message carries the assembly's reasons: {}",
            row.message
        );

        // The push is exactly-once: a later drain (a fresh analyze turn)
        // never carries the config URI again.
        session.queue_command(
            RequestId::Num(1),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let turn = session.service();
        assert!(
            turn.publishes.iter().all(|push| push.uri != config_uri),
            "the advisory never republishes"
        );

        // The fresh face: without the advisory, no config-URI push ever.
        let fresh = Workspace::new(BROKEN_MAIN);
        let store = SnapshotStore::open(SnapshotConfig {
            root: Some(store_root("stale-advisory-fresh")),
            capacity_bytes: DEFAULT_CAPACITY_BYTES,
        })
        .expect("store opens on a temp root");
        let mut session = Session::with_store(fresh.engine_config(), store, None).expect("session");
        session.sweep().expect("sweep");
        let turn = session.service();
        let fresh_uri = uri::from_path(&fresh.path(".dart_tool/package_config.json"));
        assert!(
            turn.publishes.iter().all(|push| push.uri != fresh_uri),
            "a fresh assembly never publishes the advisory"
        );
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

    /// The fix dispatch leaves the main loop (the #682 P2 residual): the
    /// drain stages the dispatch — one `dartq/fixProgress` notice, no
    /// answer — and the answer arrives through the completion drain, after
    /// which the host set is home and the slot is free.
    #[test]
    fn fix_stages_at_drain_and_answers_at_completion() {
        let (workspace, mut session) = fixture("fix-stage-unit");
        let main = workspace.path("lib/main.dart");

        session.sweep().expect("sweep");
        let _ = session.service();

        session.queue_fix(
            RequestId::Num(2),
            FixJob::Fix {
                paths: vec![main],
                verbosity: Verbosity::Compact,
            },
        );
        let turn = session.service();
        assert!(
            turn.answers.is_empty(),
            "the dispatch never holds the drain: {:?}",
            turn.answers
        );
        assert_eq!(turn.notifications.len(), 1, "one progress notice");
        assert_eq!(turn.notifications[0].method, "dartq/fixProgress");
        assert_eq!(
            turn.notifications[0].params.as_ref().expect("params")["id"].as_i64(),
            Some(2),
            "the notice names its request"
        );
        assert!(session.fix_running.is_some(), "the slot is owned");

        let turn = wait_fix(&mut session);
        let [(id, answer)] = turn.answers.as_slice() else {
            panic!("one answer: {:?}", turn.answers);
        };
        assert_eq!(*id, RequestId::Num(2));
        let answer = answer.as_ref().expect("the fix answers");
        assert_eq!(
            answer["schema_version"],
            serde_json::json!(commands::SCHEMA_VERSION)
        );
        assert!(answer["files"].is_array(), "the patch document: {answer}");
        assert!(session.fix_running.is_none(), "the mail freed the slot");
        assert!(session.fix_hosts.is_some(), "the host set came home");
    }

    /// A panic inside the fix worker (#947 F5) answers an internal error
    /// through the completion mail instead of killing the thread with the
    /// slot occupied: the slot frees, the host set comes home, and the next
    /// dispatch runs clean — the crate's containment contract applied to
    /// the off-loop dispatch.
    #[test]
    fn fix_worker_panic_answers_internal_error_and_frees_the_slot() {
        let (workspace, mut session) = fixture("fix-panic-unit");
        let main = workspace.path("lib/main.dart");

        session.sweep().expect("sweep");
        let _ = session.service();

        session.queue_fix(RequestId::Num(2), FixJob::Panics);
        let turn = session.service();
        assert_eq!(
            turn.notifications.len(),
            1,
            "the dispatch left the loop: {:?}",
            turn.notifications
        );
        assert!(session.fix_running.is_some(), "the slot is owned");

        let turn = wait_fix(&mut session);
        let [(id, answer)] = turn.answers.as_slice() else {
            panic!("one answer: {:?}", turn.answers);
        };
        assert_eq!(*id, RequestId::Num(2));
        assert!(
            matches!(answer, Err(SessionError::Internal(message)) if message.contains("panicked")),
            "the panic answered an internal error: {answer:?}"
        );
        assert!(
            session.fix_running.is_none(),
            "the panic mail freed the slot"
        );
        assert!(
            session.fix_hosts.is_some(),
            "the host set came home through the panic mail"
        );

        // The session survives: the next fix dispatches and answers clean.
        session.queue_fix(
            RequestId::Num(3),
            FixJob::Fix {
                paths: vec![main],
                verbosity: Verbosity::Compact,
            },
        );
        let _ = session.service();
        let turn = wait_fix(&mut session);
        assert_eq!(turn.answers.len(), 1);
        assert!(
            turn.answers[0].1.is_ok(),
            "the follow-up fix ran clean: {:?}",
            turn.answers[0]
        );
    }

    /// Cancelling a staged fix voids it like a read (the epoch bump is the
    /// free cancel) while the co-rider sharing the batch still answers —
    /// the cancel's re-stamp kept it.
    #[test]
    fn cancel_voids_a_staged_fix_but_not_its_co_riders() {
        let (workspace, mut session) = fixture("fix-cancel-unit");

        session.sweep().expect("sweep");
        let _ = session.service();

        session.queue_fix(
            RequestId::Num(2),
            FixJob::Fix {
                paths: vec![workspace.path("lib/main.dart")],
                verbosity: Verbosity::Compact,
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

        assert_eq!(turn.answers.len(), 2, "fix cancelled, analyze alive");
        assert_eq!(turn.answers[0].0, RequestId::Num(2));
        assert!(
            matches!(turn.answers[0].1, Err(SessionError::Cancelled)),
            "the staged fix answers cancelled: {:?}",
            turn.answers[0]
        );
        assert_eq!(turn.answers[1].0, RequestId::Num(3));
        assert!(
            turn.answers[1].1.is_ok(),
            "the co-rider survived the re-stamp: {:?}",
            turn.answers[1]
        );
        assert!(
            turn.notifications.is_empty(),
            "no dispatch, no progress notice"
        );
        assert!(session.fix_running.is_none());
        assert!(session.fixes.is_empty(), "the job left with the cancel");
    }

    /// Cancelling a read that co-rides a staged fix on the default context
    /// must not orphan the fix: the epoch bump voids the fix job's stamp
    /// too (it rides the same context), so the shared re-stamp has to cover
    /// it — otherwise the job drops at the drain with its cookie dead
    /// (never dispatches, never answers, holds its `fixes` entry forever).
    #[test]
    fn cancel_of_a_co_riding_read_keeps_the_staged_fix_alive() {
        let (workspace, mut session) = fixture("fix-read-cancel-unit");
        let main = workspace.path("lib/main.dart");

        session.sweep().expect("sweep");
        let _ = session.service();

        session.queue_fix(
            RequestId::Num(2),
            FixJob::Fix {
                paths: vec![main],
                verbosity: Verbosity::Compact,
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
        session.cancel_request(&RequestId::Num(3));
        assert_eq!(
            session.fixes.len(),
            1,
            "the cancel targeted the analyze, not the fix"
        );
        let turn = session.service();

        // The cancelled analyze answers; the fix dispatched at the same
        // drain (the progress notice is the dispatch's signature).
        assert_eq!(
            turn.answers.iter().map(|(id, _)| id).collect::<Vec<_>>(),
            [&RequestId::Num(3)],
            "the cancelled analyze: {:?}",
            turn.answers
        );
        assert!(matches!(turn.answers[0].1, Err(SessionError::Cancelled)));
        assert_eq!(
            turn.notifications.len(),
            1,
            "the fix survived the read cancel: {:?}",
            turn.notifications
        );
        assert!(session.fix_running.is_some(), "the job is on the worker");

        let turn = wait_fix(&mut session);
        let [(id, answer)] = turn.answers.as_slice() else {
            panic!("one answer: {:?}", turn.answers);
        };
        assert_eq!(*id, RequestId::Num(2));
        assert!(
            answer.as_ref().is_ok_and(|value| value["files"].is_array()),
            "the re-stamped fix answers: {answer:?}"
        );
        assert!(session.fixes.is_empty(), "the job left with the dispatch");
    }

    /// A duplicate cancel of a running dispatch is ignored (LSP
    /// §cancelRequest's already-answered rule): the first cancel queued the
    /// answer, the second must not queue another — two `Cancelled` answers
    /// for one id would violate JSON-RPC.
    #[test]
    fn double_cancel_of_a_running_dispatch_answers_once() {
        let (workspace, mut session) = fixture("fix-double-cancel-unit");

        session.sweep().expect("sweep");
        let _ = session.service();

        session.queue_fix(
            RequestId::Num(2),
            FixJob::Fix {
                paths: vec![workspace.path("lib/main.dart")],
                verbosity: Verbosity::Compact,
            },
        );
        let turn = session.service();
        assert!(turn.answers.is_empty());
        assert!(session.fix_running.is_some(), "the dispatch is running");

        session.cancel_request(&RequestId::Num(2));
        session.cancel_request(&RequestId::Num(2));
        let turn = session.service();
        assert_eq!(
            turn.answers.len(),
            1,
            "one cancel, one answer: {:?}",
            turn.answers
        );
        assert_eq!(turn.answers[0].0, RequestId::Num(2));
        assert!(matches!(turn.answers[0].1, Err(SessionError::Cancelled)));

        // The worker's late completion drops (the run was cancelled): the
        // slot frees without a second answer for the id.
        let deadline = Instant::now() + Duration::from_secs(30);
        while session.fix_running.is_some() {
            assert!(Instant::now() < deadline, "the dispatch never finished");
            let turn = session.complete_fix(None);
            assert!(
                turn.answers.is_empty(),
                "a cancelled dispatch never answers: {:?}",
                turn.answers
            );
            thread::sleep(Duration::from_millis(1));
        }
        assert!(session.fix_hosts.is_some(), "the host set came home");
    }

    /// A single cancel of a running dispatch mirrors the budget-expiry
    /// path end to end: exactly one `Cancelled` answer at the next drain,
    /// an empty follow-up drain (one cancel, one answer), a read staged in
    /// the same window answers untouched — this cancel arm never bumps the
    /// epoch, so nothing re-stamps — and the abandoned worker's late mail
    /// drops with the host set coming home.
    #[test]
    fn cancel_of_a_running_dispatch_answers_once_and_keeps_co_riders() {
        let (workspace, mut session) = fixture("fix-run-cancel-unit");
        let main = workspace.path("lib/main.dart");

        session.sweep().expect("sweep");
        let _ = session.service();

        // The dispatch leaves the loop; the cancel lands while the worker
        // owns the slot.
        session.queue_fix(
            RequestId::Num(2),
            FixJob::Fix {
                paths: vec![main],
                verbosity: Verbosity::Compact,
            },
        );
        let turn = session.service();
        assert!(turn.answers.is_empty(), "{:?}", turn.answers);
        assert!(session.fix_running.is_some(), "the dispatch is running");
        session.cancel_request(&RequestId::Num(2));

        // A read queued in the same window keeps its stamp (no epoch bump
        // on this arm) and answers alongside the cancelled id.
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
            turn.answers.iter().map(|(id, _)| id).collect::<Vec<_>>(),
            [&RequestId::Num(2), &RequestId::Num(3)],
            "the cancelled id first, the untouched read after: {:?}",
            turn.answers
        );
        assert!(matches!(turn.answers[0].1, Err(SessionError::Cancelled)));
        assert!(
            turn.answers[1].1.is_ok(),
            "the read never re-stamped, never lost: {:?}",
            turn.answers[1]
        );
        let turn = session.service();
        assert!(
            turn.answers.is_empty(),
            "one cancel, one answer — no duplicate: {:?}",
            turn.answers
        );

        // The late mail drops (the run was cancelled): the slot frees
        // without a second answer, and the host set comes home.
        let deadline = Instant::now() + Duration::from_secs(30);
        while session.fix_running.is_some() {
            assert!(Instant::now() < deadline, "the dispatch never finished");
            let turn = session.complete_fix(None);
            assert!(
                turn.answers.is_empty(),
                "a cancelled dispatch never answers: {:?}",
                turn.answers
            );
            thread::sleep(Duration::from_millis(1));
        }
        assert!(session.fix_hosts.is_some(), "the host set came home");
    }

    /// One dispatch at a time: a fix staging while the slot is owned
    /// re-stages at its drain (no second dispatch), and the completion
    /// drain frees the slot, answers the first, and dispatches the second.
    #[test]
    fn fix_dispatches_serialize_on_the_single_slot() {
        let (workspace, mut session) = fixture("fix-serialize-unit");
        let main = workspace.path("lib/main.dart");

        session.sweep().expect("sweep");
        let _ = session.service();

        session.queue_fix(
            RequestId::Num(2),
            FixJob::Fix {
                paths: vec![main.clone()],
                verbosity: Verbosity::Compact,
            },
        );
        let turn = session.service();
        assert_eq!(turn.notifications.len(), 1, "the first job dispatches");

        // The second fix stages while the slot is owned: its drain
        // re-stages it — no dispatch, no answer.
        session.queue_fix(
            RequestId::Num(3),
            FixJob::Fix {
                paths: vec![main],
                verbosity: Verbosity::Compact,
            },
        );
        let turn = session.service();
        assert!(turn.answers.is_empty(), "{:?}", turn.answers);
        assert!(
            turn.notifications.is_empty(),
            "the second job waits for the slot"
        );

        // The completion frees the slot: the drain rides the same wake,
        // answers the first job, and dispatches the second.
        let turn = wait_fix(&mut session);
        assert_eq!(turn.answers.len(), 1);
        assert_eq!(turn.answers[0].0, RequestId::Num(2));
        assert_eq!(
            turn.notifications.len(),
            1,
            "the second job dispatches at the completion"
        );

        let turn = wait_fix(&mut session);
        assert_eq!(turn.answers.len(), 1);
        assert_eq!(turn.answers[0].0, RequestId::Num(3));
    }

    /// Spins the completion drain until the in-flight dispatch's answer
    /// lands (the worker is off-loop; its mail takes a thread hop).
    fn wait_fix(session: &mut Session) -> Turn {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let turn = session.complete_fix(None);
            if !turn.answers.is_empty() {
                return turn;
            }
            assert!(Instant::now() < deadline, "the fix completion never landed");
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// The slow-plugin worst case (the #682 P2 acceptance): a plugin that
    /// stalls far past the dispatch budget still cannot wedge the session —
    /// the budget answers while the worker runs, reads keep serving, the
    /// abandoned worker's late mail drops without a second answer, the host
    /// set comes home, and the next fix dispatches normally. Runs only with
    /// a `dart` on PATH (the live fixture needs one, like the plugin crate's
    /// `callback_timeout` suite).
    #[test]
    fn fix_budget_answers_a_slow_plugin_and_the_session_survives() {
        let dart = std::process::Command::new("dart")
            .arg("--version")
            .output()
            .is_ok();
        if !dart {
            println!("skipping: no `dart` on PATH, the live fixture cannot run");
            return;
        }
        let host = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dartq-plugin-host");
        let (workspace, mut session) = fixture("fix-budget-unit");
        session.set_fix_budget(Duration::from_millis(300));

        // plugins/slow: the shared slow_plugin.dart fixture (a normal
        // handshake, then a 30 s stall per `request/fixes`). The manifest's
        // own request budget (2 s, no retries) is what ends the abandoned
        // worker; the session budget (300 ms) is what answers the client.
        let plugin = workspace.root.join("plugins/slow");
        fs::create_dir_all(plugin.join("bin")).expect("plugin bin");
        fs::write(
            plugin.join("dartq_plugin.json"),
            r#"{"name": "slow", "version": "0.1.0", "entrypoint": "bin/slow_plugin.dart",
                "capabilities": ["lint", "fix"],
                "timeouts": {"callback_ms": 500, "request_ms": 2000, "request_retries": 0}}"#,
        )
        .expect("manifest");
        fs::write(
            plugin.join("bin/slow_plugin.dart"),
            include_str!("../../dartq-plugin-host/bin/slow_plugin.dart"),
        )
        .expect("fixture script");
        // The plugin's `dart` process resolves packages by walking up to
        // the workspace root's config — extend it with the host package.
        let host_uri = fs::canonicalize(&host).expect("host package exists");
        fs::write(
            workspace.root.join(".dart_tool/package_config.json"),
            format!(
                r#"{{"configVersion": 2, "packages": [
                    {{"name": "fixture", "rootUri": "../", "packageUri": "lib/", "languageVersion": "3.5"}},
                    {{"name": "dartq_plugin_host", "rootUri": "file://{}", "packageUri": "lib/", "languageVersion": "3.10"}}]}}"#,
                host_uri.display()
            ),
        )
        .expect("package config");
        // One lint hit on the face's target (`camel_case_types`, in the
        // default package:lints core set) is what schedules the
        // `request/fixes` round trip the plugin stalls in — the seeded
        // BROKEN_MAIN carries only an engine diagnostic, and with zero
        // lint hits the worker finishes at process-startup speed, answering
        // before the budget ever fires.
        fs::write(
            workspace.path("lib/main.dart"),
            "class a {}\nvoid main() {}\n",
        )
        .expect("lint-hit target");

        session.sweep().expect("sweep");
        let _ = session.service();

        session.queue_fix(
            RequestId::Num(2),
            FixJob::Fix {
                paths: vec![workspace.path("lib/main.dart")],
                verbosity: Verbosity::Compact,
            },
        );
        let turn = session.service();
        assert_eq!(
            turn.notifications.len(),
            1,
            "the dispatch leaves the loop: {:?}",
            turn.notifications
        );

        // The budget answers while the worker still owns the hosts.
        let deadline = Instant::now() + Duration::from_secs(30);
        let budget_answered = loop {
            let turn = session.complete_fix(None);
            if let [(_, answer)] = turn.answers.as_slice() {
                break matches!(answer,
                    Err(SessionError::Internal(message)) if message.contains("budget"));
            }
            assert!(
                Instant::now() < deadline,
                "the budget expiry never answered"
            );
            thread::sleep(Duration::from_millis(10));
        };
        assert!(
            budget_answered,
            "the budget expiry answers an internal error naming the budget"
        );

        // The loop keeps serving reads while the dispatch lingers.
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
            turn.answers[0].1.is_ok(),
            "the session is not wedged: {:?}",
            turn.answers
        );

        // The abandoned worker finishes (its manifest's request budget
        // kills the stall), the late mail drops, the hosts come home, and
        // the next fix dispatches normally.
        let deadline = Instant::now() + Duration::from_secs(60);
        while session.fix_running.is_some() {
            assert!(
                Instant::now() < deadline,
                "the abandoned dispatch never finished"
            );
            let _ = session.complete_fix(None);
            thread::sleep(Duration::from_millis(50));
        }
        session.queue_fix(
            RequestId::Num(4),
            FixJob::Fix {
                paths: vec![workspace.path("lib/main.dart")],
                verbosity: Verbosity::Compact,
            },
        );
        let turn = session.service();
        assert_eq!(
            turn.notifications.len(),
            1,
            "the host set serves again: {:?}",
            turn.notifications
        );
        let turn = wait_fix(&mut session);
        assert_eq!(turn.answers[0].0, RequestId::Num(4));
        assert!(
            turn.answers[0].1.is_ok(),
            "the follow-up fix answers: {:?}",
            turn.answers
        );
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

    /// Pin for the removal face of the two per-file tables: a removed file's
    /// publish-baseline entry (`published`) and delta cursor (`changed`)
    /// drop at the removal turn — watch churn must not accumulate per-file
    /// entries for the session's life. The diagnosed file's clearing push
    /// still rides the removal turn's diff (the baseline drops only after
    /// it), and a turn without removals leaves both tables untouched.
    #[test]
    fn removal_drops_the_publish_baseline_and_delta_cursor() {
        let (workspace, mut session) = fixture("removal-drop-unit");
        let main = workspace.path("lib/main.dart");
        let extra = workspace.path("lib/extra.dart");

        session.sweep().expect("sweep");
        let turn = session.service();
        assert_eq!(turn.publishes.len(), 1, "the seeded error publishes");

        let main_file = session
            .engine(session.default_context)
            .file_table()
            .try_file_id(&main)
            .expect("the seeded file is interned");
        assert!(
            session
                .published
                .get(&main_file)
                .is_some_and(|rows| !rows.is_empty()),
            "the diagnosed file holds a non-empty baseline"
        );

        // A watch-written file enters the delta cursor (the sweep alone
        // never touches it — only explicit writes do).
        fs::write(&extra, CLEAN_MAIN).expect("create on disk");
        session.queue_disk_changes(&[extra.clone()]);
        let _ = session.service();
        assert!(
            session.changed.contains_key(&extra),
            "the explicit write gained a cursor"
        );

        // Deleting both in one turn: the diagnosed file's clearing push
        // rides the diff, then both removed entries leave their tables.
        fs::remove_file(&main).expect("delete on disk");
        fs::remove_file(&extra).expect("delete on disk");
        session.queue_disk_changes(&[main.clone(), extra.clone()]);
        let turn = session.service();
        assert_eq!(
            turn.publishes.len(),
            1,
            "the clearing push for the diagnosed file: {:?}",
            turn.publishes
        );
        assert!(turn.publishes[0].diagnostics.is_empty());
        assert!(
            !session.changed.contains_key(&extra),
            "the removed path left the delta cursor"
        );
        assert!(
            session
                .engine(session.default_context)
                .file_table()
                .try_file_id(&main)
                .is_some_and(|file| !session.published.contains_key(&file)),
            "the removed file's id stays interned (the engine never forgets) \
             while its publish baseline entry is gone"
        );
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

    /// Containment is per group (#690): a panicking base group answers its
    /// own requests `INTERNAL_ERROR` while the turn's agent group still
    /// analyzes and answers — one group's panic no longer poisons the rest
    /// of the turn.
    #[test]
    fn a_panicking_base_group_leaves_the_agent_group_answering() {
        let (workspace, mut session) = fixture("panic-groups-unit");
        let main = workspace.path("lib/main.dart");

        session.sweep().expect("sweep");
        let _ = session.service();
        session
            .set_overlay("A", vec![(main, CLEAN_MAIN.as_bytes().to_vec())])
            .expect("stage A");
        let (id_a, a) = check_request(2, &[workspace.path("lib")], Some("A"), None);
        session.queue_command(id_a, a);
        session.queue_command(
            RequestId::Num(3),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        // Groups sort base first, so the armed trigger fires on the base
        // group; the agent group runs after it in the same turn.
        crate::probe::arm_read_panic();
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
            matches!(by_id(3), Err(SessionError::Internal(message)) if message.contains("panicked")),
            "the panicking base group answers INTERNAL_ERROR: {:?}",
            by_id(3)
        );
        assert!(
            matches!(by_id(2), Ok(_)),
            "the agent group still answered in the same turn: {:?}",
            by_id(2)
        );

        // And the session carries on.
        session.queue_command(
            RequestId::Num(4),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let turn = session.service();
        assert!(
            matches!(&turn.answers[0].1, Ok(_)),
            "the session survives: {:?}",
            turn.answers[0]
        );
    }

    /// A panic in a LATER group (an agent view) leaves the same turn's
    /// publish diff intact: the base group sorts first and drains the
    /// write's diff before the agent group ever runs (#690).
    #[test]
    fn an_agent_group_panic_keeps_the_turns_publish_diff() {
        let (workspace, mut session) = fixture("panic-publish-unit");
        let main = workspace.path("lib/main.dart");
        let uri = crate::uri::from_path(&main);

        session.sweep().expect("sweep");
        let _ = session.service();
        session
            .set_overlay("A", vec![(main, AGENT_B_MAIN.as_bytes().to_vec())])
            .expect("stage A");
        let (id_a, a) = check_request(2, &[workspace.path("lib")], Some("A"), None);
        session.queue_command(id_a, a);
        assert!(session.queue_edit(&uri, CLEAN_MAIN, 2));
        session.queue_command(
            RequestId::Num(3),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        // The agent group is the turn's second (base sorts first): the
        // armed trigger skips past the base group and fires there.
        crate::probe::arm_read_panic_on_group(1);
        let turn = session.service();

        assert_eq!(
            turn.publishes.len(),
            1,
            "the write's diff drained in the same turn: {:?}",
            turn.publishes
        );
        assert!(
            turn.publishes[0].diagnostics.is_empty(),
            "the push is the base view, never the agent's rows: {:?}",
            turn.publishes[0]
        );
        let by_id = |wanted: i64| {
            &turn
                .answers
                .iter()
                .find(|(id, _)| *id == RequestId::Num(wanted))
                .expect("answer present")
                .1
        };
        assert!(
            matches!(by_id(3), Ok(_)),
            "the base group answered before the panic: {:?}",
            by_id(3)
        );
        assert!(
            matches!(by_id(2), Err(SessionError::Internal(message)) if message.contains("panicked")),
            "the panicking agent group answers INTERNAL_ERROR: {:?}",
            by_id(2)
        );
    }

    /// The write path's containment: a panicking apply does not lose the
    /// client's staged edits — they are restaged (last-wins) and the next
    /// real intake's write task applies them. Re-enqueueing immediately
    /// would retry the same panicked apply every drain, so the restage
    /// deliberately waits for new intake instead.
    #[test]
    fn a_panicking_write_restages_the_edits_for_the_next_apply() {
        let (workspace, mut session) = fixture("write-panic-unit");
        let uri = crate::uri::from_path(&workspace.path("lib/main.dart"));

        session.sweep().expect("sweep");
        let _ = session.service();

        // The apply panics; the engine keeps its old view and the edits come
        // back staged.
        assert!(session.queue_edit(&uri, "var client = 1;\n", 2));
        crate::probe::arm_write_panic();
        let turn = session.service();
        assert!(
            turn.publishes.is_empty(),
            "the write did not land, so nothing changed: {:?}",
            turn.publishes
        );
        assert!(
            !session.staged.is_empty(),
            "the edits are restaged, not dropped"
        );
        let main_path = workspace.path("lib/main.dart");
        let main_file = session
            .engine(session.default_context)
            .file_table()
            .try_file_id(main_path.as_path())
            .expect("the swept file is resident");
        assert_eq!(
            session
                .engine(session.default_context)
                .content(main_file)
                .expect("resident content"),
            BROKEN_MAIN.as_bytes(),
            "the panicked apply did not land"
        );

        // The next real intake applies the restaged edit alongside the new
        // one — the client's first word is not lost.
        let second = crate::uri::from_path(&workspace.path("lib/second.dart"));
        assert!(session.queue_edit(&second, "var second = 2;\n", 1));
        let turn = session.service();
        assert!(
            !turn.publishes.is_empty(),
            "the reconciled push drains: {:?}",
            turn.publishes
        );
        let engine = session.engine(session.default_context);
        let main_file = engine
            .file_table()
            .try_file_id(main_path.as_path())
            .expect("the swept file is resident");
        assert_eq!(
            engine.content(main_file).expect("resident content"),
            "var client = 1;\n".as_bytes(),
            "the restaged edit applied with the next write task"
        );
        let second_file = engine
            .file_table()
            .try_file_id(workspace.path("lib/second.dart").as_path())
            .expect("the overlay-only file is interned by its apply");
        assert_eq!(
            engine.content(second_file).expect("overlay content"),
            "var second = 2;\n".as_bytes()
        );
    }

    /// The retained-baseline marker is code+source, not code alone: a
    /// plugin emitting the crash code under its own source keeps its rows
    /// (its baseline must stabilize); only the engine's own row is one-shot.
    #[test]
    fn crash_row_marker_matches_code_and_source() {
        let engine_row = lsp::Diagnostic {
            range: lsp::Range {
                start: lsp::Position {
                    line: 0,
                    character: 0,
                },
                end: lsp::Position {
                    line: 0,
                    character: 0,
                },
            },
            severity: 1,
            code: dartq_diagnostics::DARTQ_PLUGIN_CRASHED.name.to_owned(),
            source: "dartq".into(),
            message: "plugin `echo` crashed".into(),
        };
        assert!(Session::is_engine_crash_row(&engine_row));

        let colliding = lsp::Diagnostic {
            source: "echo".into(),
            ..engine_row.clone()
        };
        assert!(
            !Session::is_engine_crash_row(&colliding),
            "a plugin's own code collision is not a one-shot row"
        );

        let ordinary = lsp::Diagnostic {
            code: "uri_does_not_exist".into(),
            ..engine_row
        };
        assert!(!Session::is_engine_crash_row(&ordinary));
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

    /// The byte watermark (#947 F7): a few large documents sweep the store
    /// even though the entry count never nears the count threshold — 64
    /// entries of huge states was the unbounded-residency shape. The tally
    /// resets only on a sweep, so zero after the last drain means the byte
    /// rule fired (the count threshold could not have: 6 contents < 64).
    #[test]
    fn byte_watermark_sweeps_the_store_below_the_count_threshold() {
        let (workspace, mut session) = fixture("gc-byte-watermark-unit");
        session.set_gc_byte_limit(256);
        session.sweep().expect("sweep");
        let _ = session.service();

        let uri = crate::uri::from_path(&workspace.path("lib/main.dart"));
        for version in 0..6u64 {
            let text = format!("void main() {{}} // v{version} {}", "x".repeat(280));
            assert!(session.queue_edit(&uri, &text, i64::try_from(version).expect("version") + 2,));
            let _ = session.service();
        }
        assert_eq!(
            session.gc_bytes_since_sweep(),
            0,
            "the byte watermark swept at the turn boundary"
        );
        assert!(
            session.overlay_store_contents() <= 4,
            "the byte-driven sweep keeps the store at its live set"
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

    /// The delta table's cap eviction (#947 F8): past the cap the lowest
    /// cursors drop — the rows only a client paging from a very old `since`
    /// could still ask about (the contract sends them to a re-sync) — and
    /// the newest paths survive untouched.
    #[test]
    fn delta_table_evicts_oldest_cursors_past_the_cap() {
        let (_workspace, mut session) = fixture("delta-cap-unit");
        session.set_table_caps(8, 8);

        for index in 0..12u64 {
            let path = PathBuf::from(format!("/w/lib/file_{index}.dart"));
            session.changed.insert(path, index + 1);
        }
        session.evict_delta_paths();

        assert_eq!(session.changed.len(), 7, "the cap's newest 7/8 stay");
        for index in 0..5u64 {
            assert!(
                !session
                    .changed
                    .contains_key(&PathBuf::from(format!("/w/lib/file_{index}.dart"))),
                "cursor {} (the oldest) evicted",
                index + 1
            );
        }
        for index in 5..12u64 {
            assert!(
                session
                    .changed
                    .contains_key(&PathBuf::from(format!("/w/lib/file_{index}.dart"))),
                "cursor {} (recent) kept",
                index + 1
            );
        }
    }

    /// The open-document table's cap eviction (#947 F8), through the real
    /// intake: past the cap the least-recently-touched documents drop, and
    /// an edit refreshes recency — the active document survives an open
    /// storm that evicts its older, idle neighbors.
    #[test]
    fn open_documents_evict_least_recently_touched_past_the_cap() {
        let (workspace, mut session) = fixture("open-cap-unit");
        session.set_table_caps(8, 8);
        let uri = |name: &str| crate::uri::from_path(&workspace.path(name));

        for index in 0..8u64 {
            assert!(session.queue_edit(
                &uri(&format!("lib/doc_{index}.dart")),
                &format!("void d{index}() {{}}\n"),
                i64::try_from(index).expect("version") + 1,
            ));
        }
        assert_eq!(session.opened.len(), 8, "at the cap: nothing evicts");

        // Touch doc_0 (refresh its recency), then open one more — the cap
        // fires, and the coldest two (doc_1, doc_2) evict, not doc_0.
        assert!(session.queue_edit(&uri("lib/doc_0.dart"), "void d0() {}\n", 9));
        assert!(session.queue_edit(&uri("lib/doc_8.dart"), "void d8() {}\n", 1));

        assert_eq!(session.opened.len(), 7, "the cap's newest 7/8 stay");
        assert!(
            session
                .opened
                .contains_key(&workspace.path("lib/doc_0.dart")),
            "the refreshed document survives"
        );
        assert!(
            !session
                .opened
                .contains_key(&workspace.path("lib/doc_1.dart")),
            "the coldest document evicted"
        );
        assert!(
            !session
                .opened
                .contains_key(&workspace.path("lib/doc_2.dart")),
            "the second-coldest document evicted"
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

    /// The per-agent file ceiling (#947 F3): merging past
    /// [`MAX_OVERLAY_FILES`] files is rejected atomically — nothing stages,
    /// and an overwrite that keeps the count at the cap still fits.
    #[test]
    fn agent_overlay_file_count_is_capped() {
        let (workspace, mut session) = fixture("agent-files-cap-unit");
        let files: Vec<(PathBuf, Vec<u8>)> = (0..MAX_OVERLAY_FILES)
            .map(|index| {
                (
                    workspace.path(&format!("lib/f{index}.dart")),
                    b"var x = 1;\n".to_vec(),
                )
            })
            .collect();
        session
            .set_overlay("A", files)
            .expect("the cap fits exactly");

        // One more file rejects; the staged set is untouched.
        let error = session
            .set_overlay(
                "A",
                vec![(workspace.path("lib/extra.dart"), b"var y = 2;\n".to_vec())],
            )
            .expect_err("the cap rejects growth");
        assert!(
            matches!(error, SessionError::InvalidParams(ref message) if message.contains("file cap")),
            "{error}"
        );
        assert_eq!(
            session.agents["A"].files.len(),
            MAX_OVERLAY_FILES,
            "the rejection staged nothing"
        );

        // Overwriting an already-staged path does not grow the count.
        session
            .set_overlay(
                "A",
                vec![(workspace.path("lib/f0.dart"), b"var z = 3;\n".to_vec())],
            )
            .expect("an overwrite stays at the cap");
        assert_eq!(session.agents["A"].files.len(), MAX_OVERLAY_FILES);
    }

    /// The per-agent byte ceiling (#947 F3): staged bytes past
    /// [`MAX_OVERLAY_BYTES`] are rejected `INVALID_PARAMS`; exactly the cap
    /// fits.
    #[test]
    fn agent_overlay_bytes_are_capped() {
        let (workspace, mut session) = fixture("agent-bytes-cap-unit");
        let full = vec![0u8; MAX_OVERLAY_BYTES];
        session
            .set_overlay("A", vec![(workspace.path("lib/big.dart"), full)])
            .expect("exactly the byte cap fits");

        let error = session
            .set_overlay("A", vec![(workspace.path("lib/one.dart"), b"x".to_vec())])
            .expect_err("one byte past the cap rejects");
        assert!(
            matches!(error, SessionError::InvalidParams(ref message) if message.contains("byte cap")),
            "{error}"
        );
        // The rejection is atomic: only the first file staged.
        assert_eq!(session.agents["A"].files.len(), 1);
    }

    /// The session-wide staged-total ceiling (#962 P3-2): every agent may
    /// hold its own per-agent cap, but the sum across agents is capped —
    /// one byte past [`MAX_OVERLAY_TOTAL_BYTES`] rejects atomically for the
    /// staging agent, and clearing an agent frees budget again.
    #[test]
    fn session_overlay_total_is_capped() {
        let (workspace, mut session) = fixture("session-total-cap-unit");
        let agents_at_cap = MAX_OVERLAY_TOTAL_BYTES / MAX_OVERLAY_BYTES;
        for index in 0..agents_at_cap {
            let agent = format!("A{index}");
            let full = vec![0u8; MAX_OVERLAY_BYTES];
            session
                .set_overlay(&agent, vec![(workspace.path("lib/big.dart"), full)])
                .expect("each agent holds its own cap within the session total");
        }

        // One byte past the session total rejects the staging agent only.
        let error = session
            .set_overlay("E", vec![(workspace.path("lib/one.dart"), b"x".to_vec())])
            .expect_err("the session total rejects growth");
        assert!(
            matches!(error, SessionError::InvalidParams(ref message) if message.contains("total cap")),
            "{error}"
        );
        assert!(
            !session.agents.contains_key("E"),
            "the rejection staged nothing"
        );
        assert_eq!(session.agents.len(), agents_at_cap);

        // Clearing an agent frees its budget for the next stager.
        session.clear_overlay("A0");
        session
            .set_overlay("E", vec![(workspace.path("lib/one.dart"), b"x".to_vec())])
            .expect("a cleared agent frees session budget");
    }

    // -----------------------------------------------------------------------
    // Write-face path confinement (#947 F4): overlay staging + commit are
    // the only RPC faces that put bytes on disk, so their paths must stay
    // inside the workspace root. The read faces keep CLI parity.
    // -----------------------------------------------------------------------

    /// Staging rejects any path whose normalized form escapes the workspace
    /// root — absolute or relative — and normalizes in-root detours.
    #[test]
    fn overlay_staging_rejects_paths_outside_the_workspace() {
        let (workspace, mut session) = fixture("overlay-confine-unit");
        let main = workspace.path("lib/main.dart");

        // Absolute escape.
        let error = session
            .set_overlay(
                "A",
                vec![(PathBuf::from("/etc/dartq-escape"), b"x".to_vec())],
            )
            .expect_err("an absolute path outside the root rejects");
        assert!(
            matches!(error, SessionError::InvalidParams(ref message) if message.contains("escapes")),
            "{error}"
        );
        // Relative `..` escape.
        let outside = session.set_overlay(
            "A",
            vec![(workspace.path("../../outside.dart"), b"x".to_vec())],
        );
        assert!(
            matches!(outside, Err(SessionError::InvalidParams(ref message)) if message.contains("escapes")),
            "{outside:?}"
        );
        // Nothing staged by either rejection — the agent table stayed empty.
        assert!(!session.has_agent("A"), "a rejected set stages nothing");

        // An in-root `..` detour normalizes and stages under its real path.
        session
            .set_overlay(
                "A",
                vec![(
                    workspace.path("lib/../lib/main.dart"),
                    CLEAN_MAIN.as_bytes().to_vec(),
                )],
            )
            .expect("an in-root detour stages");
        assert_eq!(session.agents["A"].files.len(), 1);
        assert!(
            session.agents["A"].files.contains_key(&main),
            "the staged path normalized: {:?}",
            session.agents["A"].files.keys().collect::<Vec<_>>()
        );
    }

    /// The commit-time canonical check (#947 F4, belt and braces): a symlink
    /// planted in the worktree between staging and commit must not aim the
    /// write outside the root, even though staging's lexical pass saw it
    /// inside. The refusal leaves the staged view intact and writes nothing.
    #[cfg(unix)]
    #[test]
    fn overlay_commit_rejects_a_symlink_escape() {
        let (workspace, mut session) = fixture("commit-symlink-unit");

        session.sweep().expect("sweep");
        let _ = session.service();

        // A directory outside the workspace, reachable through an in-root
        // symlink.
        let outside = std::env::temp_dir().join(format!(
            "dartq-commit-escape-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("t")
        ));
        fs::create_dir_all(&outside).expect("outside dir");
        let link = workspace.path("link");
        std::os::unix::fs::symlink(&outside, &link).expect("symlink");

        session
            .set_overlay(
                "A",
                vec![(link.join("x.dart"), CLEAN_MAIN.as_bytes().to_vec())],
            )
            .expect("staging is lexical: the symlink is not followed yet");

        let error = session
            .commit_overlay("A")
            .expect_err("the canonical check rejects the escape");
        assert!(
            matches!(error, SessionError::InvalidParams(ref message) if message.contains("escapes")),
            "{error}"
        );
        assert!(
            !outside.join("x.dart").exists(),
            "nothing was written outside the root"
        );
        assert_eq!(
            session.agents["A"].files.len(),
            1,
            "the staged view stays intact on refusal"
        );
        let _ = fs::remove_dir_all(&outside);
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

    /// The plugin weave (#471 phase B): a dead plugin is recorded at
    /// assembly; the crash row withholds until the session has an anchor
    /// (a published file), then publishes once — merged after the engine
    /// rows — and never repeats. The dead set is injected rather than
    /// reached through a manifest on disk: a missing-entrypoint manifest is
    /// *skipped* at discovery (never spawn-failed), and a spawnable one
    /// needs a real child process — both pinned by plugins.rs's own tests.
    /// This gate pins the session side of the weave.
    #[test]
    fn a_failing_plugin_surfaces_its_crash_row_on_the_base_publish() {
        let (_workspace, mut session) = fixture("plugin-crash-unit");
        // The broken plugin lands before the first drain: its spawn failure
        // is already recorded, awaiting the first anchored cycle.
        session.inject_plugins(PluginSet::with_dead_plugin(
            "dead",
            "restart budget of 3 exhausted",
        ));

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

    /// The weave is contained a level finer than the group (#690): a panic
    /// in the plugin cycle skips that cycle's findings but the group's
    /// engine rows still answer and publish — and the penalty's crash row
    /// surfaces on the next cycle through the normal one-shot path, exactly
    /// like a transport-side death.
    #[test]
    fn a_panicking_plugin_cycle_keeps_the_group_alive_and_the_row_comes_once() {
        let (_workspace, mut session) = fixture("weave-panic-unit");
        session.inject_plugins(PluginSet::with_dead_plugin(
            "dead",
            "restart budget of 3 exhausted",
        ));

        session.sweep().expect("sweep");
        let turn = session.service();
        assert!(
            turn.publishes.len() == 1
                && turn.publishes[0]
                    .diagnostics
                    .iter()
                    .all(|row| row.code != dartq_diagnostics::DARTQ_PLUGIN_CRASHED.name),
            "the first cycle withheld the crash (no anchor): {:?}",
            turn.publishes
        );

        // The weave panics at the cycle's entry: no findings, no crash row
        // this cycle — but the group answers, so the engine rows flow.
        crate::probe::arm_weave_panic();
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
            matches!(&turn.answers[0].1, Ok(_)),
            "the group survived the weave panic: {:?}",
            turn.answers[0]
        );
        assert!(
            turn.publishes.is_empty(),
            "the panicking cycle skipped its rows: {:?}",
            turn.publishes
        );

        // The next cycle runs: the anchor exists (turn 1 published), so the
        // held crash row publishes once, merged after the engine rows.
        session.queue_command(
            RequestId::Num(3),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let turn = session.service();
        let rows = &turn
            .publishes
            .first()
            .expect("the crash row publishes on the recovered cycle")
            .diagnostics;
        assert_eq!(
            rows.last().expect("row present").code,
            dartq_diagnostics::DARTQ_PLUGIN_CRASHED.name,
            "the crash row rides the merge: {rows:?}"
        );

        // Once per session: the cycle after is clean again.
        session.queue_command(
            RequestId::Num(4),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let turn = session.service();
        assert!(
            turn.publishes.is_empty(),
            "subsequent turns are clean: {:?}",
            turn.publishes
        );
    }

    /// The repeat-offender ladder (#690): panics on the plugin-bearing
    /// group count one per turn, the count never resets, and the third
    /// strike disables the plugin set (its disable transition is pinned by
    /// plugins.rs's own tests; this gate pins the ladder semantics).
    #[test]
    fn repeated_plugin_path_panics_climb_the_ladder_to_the_penalty() {
        let (_workspace, mut session) = fixture("panic-ladder-unit");
        session.inject_plugins(PluginSet::with_dead_plugin(
            "dead",
            "restart budget of 3 exhausted",
        ));

        session.sweep().expect("sweep");
        let _ = session.service();
        assert_eq!(session.plugin_path_panics, 0);

        for strike in 0..PLUGIN_PANIC_LIMIT {
            crate::probe::arm_read_panic();
            session.queue_command(
                RequestId::Num(i64::from(strike) + 2),
                Request::Analyze {
                    verbosity: Verbosity::Compact,
                    agent: None,
                    context: None,
                },
            );
            let turn = session.service();
            assert!(
                matches!(&turn.answers[0].1, Err(SessionError::Internal(_))),
                "strike {strike} answers INTERNAL_ERROR: {:?}",
                turn.answers[0]
            );
            assert_eq!(
                session.plugin_path_panics,
                strike + 1,
                "the ladder climbs one strike per turn"
            );
        }

        // The count never resets: a healthy cycle leaves it, a further
        // panic keeps climbing (saturating).
        session.queue_command(
            RequestId::Num(5),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let _ = session.service();
        assert_eq!(session.plugin_path_panics, PLUGIN_PANIC_LIMIT);
        crate::probe::arm_read_panic();
        session.queue_command(
            RequestId::Num(6),
            Request::Analyze {
                verbosity: Verbosity::Compact,
                agent: None,
                context: None,
            },
        );
        let _ = session.service();
        assert_eq!(session.plugin_path_panics, PLUGIN_PANIC_LIMIT + 1);
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
        let mut session =
            Session::with_store(workspace.engine_config(), store, None).expect("session");

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
