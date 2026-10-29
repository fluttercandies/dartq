//! The core-side plugin runtime: owns one Dart host process and drives the
//! wire contract against it (), with the isolation metrics
//! wired at every enforcement point.
//!
//! # Face
//!
//! [`PluginRuntime::spawn`] launches the host, performs the `initialize`
//! handshake (within the manifest's request budget), and hands back a handle;
//! [`PluginRuntime::lazy`] hands back the same handle without launching —
//! the first generation then starts at the first interaction
//! ([`PluginRuntime::ensure_alive`], the same path every restart rides), so
//! a plugin whose capability is never needed never costs a process. From
//! there the engine wiring is two calls per analyze cycle:
//!
//! - [`PluginRuntime::feed`] pushes the engine's merged diagnostics to the
//!   plugin as one `notification/engine_diagnostics` batch (its diagnostic
//!   context); [`FileTable`] resolves [`FileId`]s to the wire's path strings.
//! - [`PluginRuntime::take_output`] drains the diagnostics the plugin
//!   emitted back (`notification/diagnostic`) for the caller to merge into
//!   its output. Plugin findings keep their wire shape here: engine
//!   [`CodeInfo`](dartq_diagnostics::CodeInfo)s are `&'static`-interned, so
//!   namespacing plugin codes into
//!   the engine's diagnostic set is the daemon/CLI output layer's job
//!  , not the transport's.
//! - [`PluginRuntime::request_fixes`] is the fix face: it sends
//!   `request/fixes` and awaits the reply under the manifest's callback
//!   budget. An overrun kills the child and
//!   consumes the restart budget, surfacing
//!   [`RuntimeError::CallbackTimeout`]. The analyze push has no reply to
//!   await, so the monitor enforces the same budget on the demand itself:
//!   the clock [`PluginRuntime::feed`] opens must close with produced
//!   diagnostics before `callback_ms` elapses.
//!
//! [`PluginRuntime::stats`] is the observability face (peak RSS, frame
//! and byte counters, guard drops, restart count) the daemon/CLI read.
//! [`crash_diagnostic`] converts a terminal [`RuntimeError`] into the
//! `dartq/`-namespace [`Diagnostic`] the output layer merges (:
//! a plugin failure is one tool diagnostic, never an engine failure).
//!
//! # Discovery
//!
//! [`crate::discovery`] turns an on-disk workspace into spawn-ready
//! manifests (the `<workspace>/plugins/*/dartq_plugin.json` convention plus
//! explicit override paths); each hit launches through
//! [`SpawnSpec::dart`].
//!
//! # conformance
//!
//! | Dimension | Mechanism here |
//! | --- | --- |
//! | Memory | RSS monitor thread (100 ms tick) samples the child; Linux reads the kernel's `VmHWM` high-water mark, macOS `proc_pid_rusage` (`ri_resident_size`, SDK-header-pinned ABI); exceeding the manifest's `memory_budget_mb` kills the child. The same thread carries the idle-eviction arm: a child idle past the manifest's `idle_evict_ms` (default 5 min, `0` disables; `DARTQ_PLUGIN_IDLE_EVICT_MS` overrides for measurement) is killed to release its resident set and relaunches budget-free at the next interaction — an eviction is a host memory action, never a crash, and is counted in `stats.evictions`. Sampling loss never disarms the monitor: a platform without a sampler (or a live child that keeps failing its samples past a bounded tolerance) disarms only the RSS face — counted in `stats.rss_disarms` — while the thread stays on the analyze clock. |
//! | CPU | `request/fixes` awaits its reply under the manifest's `callback_ms`; an overrun kills the child and consumes the restart budget. The analyze demand is enforced against the same budget: feeding engine diagnostics opens a clock in the monitor, closed when the child produces diagnostics (or dies); a demand still open past `callback_ms` kills the child through the same path. |
//! | IPC | `initialize` and `shutdown` run under the manifest's `request_ms`; every stdin write is bounded by the same deadline (writer thread). |
//! | Crash | Child death never propagates: the runtime relaunches within the manifest's retry budget (counted in `stats.restarts`), then reports [`RuntimeError::Crashed`] — the terminal form names the exhausted budget. |
//! | Frame guard | Both directions: inbound lines beyond [`MAX_FRAME_SIZE`] are dropped before buffering (counted) and tear the session down — the transport never resynchronizes; outbound frames beyond it are rejected at the handle with the same typed error before any write, counted, leaving the child untouched. |
//!
//! # Threading (std threads only, per the workspace runtime policy)
//!
//! Per generation (one child process): a reader thread owns the child's
//! stdout, the [`Session`] state machine (behind a mutex shared with the
//! handle), and the pending-diagnostics buffer; a monitor thread enforces
//! the memory budget (RSS samples), the analyze demand's clock, and the
//! idle-eviction deadline; a writer thread owns the child's stdin
//! and is the only process writing to it — the handle (`&mut self`) feeds it
//! frames and awaits per-write acks, which bounds every write without ever
//! blocking the handle on a full pipe. Under [`StdioPolicy::Capture`] (the
//! default) a stderr drain thread keeps the child's stderr pipe empty and
//! feeds the bounded startup capture, whose text folds into the surfaced
//! startup-failure error. A worker the OS cannot create (thread creation
//! fails under fd or memory exhaustion) takes the ordinary death path —
//! cause recorded in the death slot, child killed, restart budget decides —
//! so a resource-starved host degrades one plugin instead of aborting the
//! daemon. All shared counters are atomics; the
//! only cross-thread locks are the three leaf mutexes (session, pending,
//! child) and the death slot, never nested.
//!
//! # Known bounds
//!
//! - The stdin write deadline is the manifest's `request_ms` — a plugin that
//!   stops draining its input pipe is killed as a protocol violation, but
//!   the kill (like every crash path) restarts lazily at the next
//!   interaction. `request_fixes` alone restarts eagerly, so its
//!   `stats.restarts` consumption is observable at the error site.
//! - `feed` skips unknown [`FileId`]s (a caller-contract violation, same
//!   posture as `FileLayoutProvider`) rather than failing a whole batch.

use std::io::{BufRead, BufReader, Read, Write};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};

use dartq_diagnostics::{
    DARTQ_PLUGIN_CRASHED, DARTQ_PLUGIN_TIMEOUT, Diagnostic, Severity as EngineSeverity, SourceRange,
};
use dartq_source::{FileId, FileTable};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::capability::{Capability, CapabilitySet};
use crate::frame::{Frame, FrameDecoder, MAX_FRAME_SIZE, ProtocolError, encode_frame};
use crate::manifest::{EntrypointKind, Manifest};
use crate::session::{
    CycleConfig, CyclePush, FixesReply, Incoming, PluginDiagnostic, QueryRequest, RuleCensus,
    Session, SessionError, SessionState, Severity, engine_diagnostics_frame, plugin_config_frame,
    query_error_frame, query_reply_frame,
};
use crate::stats::{PluginStats, PluginStatsSnapshot};

/// How often the isolation monitor samples the child's RSS.
const RSS_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Cap on the captured startup stderr: only the tail (the last
/// [`STDERR_CAPTURE_CAP`] bytes) is retained — the end of a failing host's
/// output is what names the failure (the fatal exception, the pub resolution
/// error) — and a bounded capture cannot grow with a chatty host however
/// long the startup takes.
const STDERR_CAPTURE_CAP: usize = 8 * 1024;

/// Read-chunk size of the stderr drain thread: large enough for a burst,
/// small enough that the capture bound dominates the thread's transient
/// memory.
const STDERR_READ_CHUNK: usize = 4096;

/// Teardown join bound: how long a generation teardown waits for the reader
/// or writer after the kill before detaching it. After a kill the threads
/// unwind in microseconds (the Unix group kill closes every pipe write end,
/// so the reader's `fill_buf` returns EOF immediately); the bound only binds
/// when a descendant escaped the group (its own `setsid`, or a non-Unix
/// target where only the direct child dies) and still holds the write end —
/// one second of detach-latency there beats an unbounded hang, and it sits
/// two orders below the smallest budget the teardown runs inside.
const TEARDOWN_JOIN_BOUND: Duration = Duration::from_secs(1);

/// Poll step while waiting out [`TEARDOWN_JOIN_BOUND`] (std has no timed
/// join; `is_finished` + sleep bounds the wait without consuming the
/// handle).
const JOIN_POLL: Duration = Duration::from_millis(5);

/// Whether this build has an RSS sampler at all (see `sample_rss` below).
/// On a target without one the monitor does not exit — the analyze clock
/// keeps its enforcement — and the RSS arm records its disarm in the stats
/// once per generation instead of failing a sample on every tick.
const RSS_SAMPLING_SUPPORTED: bool = cfg!(any(target_os = "linux", target_os = "macos"));

/// Consecutive failed RSS samples a live child's watch rides out before
/// concluding the arm cannot run and disarming it, observably. Three ticks
/// ≈ 300 ms at the poll interval: far above any transient scheduler or
/// syscall hiccup, and a genuinely unreadable target exhausts it in one
/// poll window instead of silently ending the enforcement.
const RSS_SAMPLE_TOLERANCE: u32 = 3;

/// Pending-output fuse: a plugin that queues more than this many un-drained
/// diagnostics *or reverse queries* is torn down as a protocol violation
/// instead of growing the buffer without bound — the same `10_000` fuse the
/// parser's `MAX_DIAGNOSTICS` and the scanner's per-file cap enforce. The
/// two queues share one fuse: both are un-drained reader output, and a
/// plugin flooding either is the same isolation breach.
const MAX_PENDING_OUTPUT: usize = 10_000;

/// Reply-burst settle window of the drain: after the first diagnostic of a
/// reply arrives, [`PendingOutput::wait_for_output`] keeps absorbing
/// rows this long of pipe quiet before handing the batch over. A plugin
/// serves one analyze cycle with contiguous `notification/diagnostic`
/// frames (the host loop writes them back-to-back), so without the window
/// the drain races the reply and returns its first line while the rest is
/// still in flight. The window is orders of magnitude above the
/// sub-millisecond line gaps of a live burst and far below the daemon's
/// per-cycle drain budget; the caller's deadline still caps the whole wait
/// exactly.
const BURST_SETTLE: Duration = Duration::from_millis(5);

/// How a plugin process is launched and what budgets it runs under. The
/// launch policy (the `dart` invocation) is the caller's; the runtime owns
/// protocol, isolation, and restarts.
#[derive(Clone, Debug)]
pub struct SpawnSpec {
    /// Executable, e.g. `dart` (resolved over `PATH`).
    pub program: String,
    /// Arguments after the program; for the manifest contract this is the
    /// entrypoint path ([`SpawnSpec::dart`] builds it — a script, kernel
    /// snapshot, or executable per the manifest's `entrypoint_kind`).
    pub args: Vec<String>,
    /// Working directory of the plugin (its package directory).
    pub current_dir: PathBuf,
    /// The parsed manifest: capabilities and resource budgets.
    pub manifest: Manifest,
    /// Where the child's stderr goes; the default captures it into a bounded
    /// startup buffer so a startup failure surfaces with the host's own
    /// error output instead of a bare "host died".
    pub stderr: StdioPolicy,
    /// Whether this session serves the plugin's `request/query` reverse
    /// requests (T4). Offering the `query` capability at the handshake is
    /// gated on this: a face without a query server (the fix faces) never
    /// advertises it, so a querying producer sees the capability absent
    /// instead of timing out against a core that cannot answer. The analyze
    /// face (the daemon's plugin set) opts in — its cycle pump serves
    /// queries against the session's current engine view.
    pub serve_queries: bool,
}

/// The stderr routing of the plugin process.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StdioPolicy {
    /// Capture into a bounded startup buffer (default): a dedicated drain
    /// thread keeps the pipe empty — a chatty plugin can never block on a
    /// full unread pipe — and the captured tail backs the startup-failure
    /// error text; once the handshake succeeds the capture is discarded and
    /// the thread keeps only draining.
    #[default]
    Capture,
    /// Discard unconditionally.
    Null,
    /// Pass through to dartq's own stderr (tests, debugging).
    Inherit,
}

impl StdioPolicy {
    fn stdio(self) -> Stdio {
        match self {
            Self::Capture => Stdio::piped(),
            Self::Null => Stdio::null(),
            // `stderr()` is the process handle; `Stdio::inherit()` needs the
            // raw fd — both route to the same stream.
            Self::Inherit => Stdio::inherit(),
        }
    }
}

impl SpawnSpec {
    /// The manifest contract's default launch, branched on the manifest's
    /// [`EntrypointKind`] — this is the one place the spawn argv is built:
    ///
    /// - `source` / `kernel`: `dart <entrypoint>`. Both share the argv;
    ///   they differ in what the VM does with the file. A source script is
    ///   compiled by the VM's in-process CFE at startup; a kernel snapshot
    ///   (`.dill`) is recognized by magic number and loaded precompiled —
    ///   no CFE source compile runs in the child (upstream Dart SDK source,
    ///   `runtime/bin/dfe.cc`).
    /// - `exe`: the self-contained `dart compile exe` product is spawned
    ///   directly — no `dart` front end at all. The path is absolutized:
    ///   the executable is resolved against the *parent's* working
    ///   directory on some platforms (Windows) while `current_dir` only
    ///   takes effect inside the child.
    ///
    /// Query serving stays off — a caller with a query server (the daemon's
    /// analyze face) flips [`SpawnSpec::serve_queries`].
    #[must_use]
    pub fn dart(plugin_dir: &Path, manifest: Manifest) -> Self {
        let mut spec = Self {
            program: "dart".to_owned(),
            args: vec![manifest.entrypoint.clone()],
            current_dir: plugin_dir.to_path_buf(),
            manifest,
            stderr: StdioPolicy::Capture,
            serve_queries: false,
        };
        if spec.manifest.entrypoint_kind == EntrypointKind::Exe {
            let exe = plugin_dir.join(&spec.manifest.entrypoint);
            spec.program = std::path::absolute(&exe)
                .unwrap_or(exe)
                .to_string_lossy()
                .into_owned();
            spec.args.clear();
        }
        spec
    }
}

/// Runtime failures. Any variant other than a successful shutdown leaves the
/// runtime dead or restarting — the engine never loses its own state to a
/// plugin failure.
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// The plugin process could not be started.
    #[error("cannot spawn plugin process `{program}`: {source}")]
    Spawn {
        /// The executable attempted.
        program: String,
        /// The underlying spawn error.
        #[source]
        source: std::io::Error,
    },
    /// The plugin did not answer `initialize` within the request budget.
    #[error("plugin handshake timed out after {ms} ms{}", stderr_note(stderr))]
    HandshakeTimeout {
        /// The enforced budget in milliseconds.
        ms: u64,
        /// The host's stderr captured during the failed startup (empty when
        /// the host wrote nothing or the policy was not
        /// [`StdioPolicy::Capture`]).
        stderr: String,
    },
    /// `shutdown` was answered neither in time nor at all.
    #[error("plugin shutdown timed out after {ms} ms")]
    ShutdownTimeout {
        /// The enforced budget in milliseconds.
        ms: u64,
    },
    /// The plugin overran the manifest's `callback_ms` budget on
    /// `request/fixes`: the child was killed and one restart
    /// consumed; the terminal form when the budget is spent is
    /// [`RuntimeError::Crashed`].
    #[error("plugin `request/fixes` timed out after {ms} ms (callback budget)")]
    CallbackTimeout {
        /// The enforced callback budget in milliseconds.
        ms: u64,
    },
    /// The plugin violated the wire contract or died; after the restart
    /// budget is exhausted this is terminal.
    #[error("plugin crashed: {reason}")]
    Crashed {
        /// What ended the session (protocol message or exit description).
        reason: String,
    },
    /// A wire-frame codec failure.
    #[error(transparent)]
    Wire(#[from] ProtocolError),
    /// A session state-machine failure.
    #[error(transparent)]
    Protocol(#[from] SessionError),
    /// Transport I/O on the plugin's pipes.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Reader-thread protocol outcomes the handle waits on.
#[derive(Debug)]
enum ProtocolEvent {
    /// The `initialize` reply was accepted.
    Initialized(CapabilitySet),
    /// The `request/fixes` reply with the correlation id it completes.
    FixesReply { id: u64, reply: FixesReply },
    /// The reader queued one or more of the plugin's reverse
    /// `request/query` frames: the fixes/shutdown wait loops wake on this
    /// to pump them (serve, or decline when no server exists) instead of
    /// letting a querying producer stall its own reply past the budget.
    QueriesQueued,
    /// The `shutdown` reply was accepted.
    ShutdownComplete,
    /// The session ended abnormally (protocol violation or lost stream).
    /// A pure signal — the cause lives in the authoritative death slot,
    /// which the reader fills before sending this event.
    Dead,
}

/// One command to the per-generation writer thread (the only stdin writer).
enum WriterCommand {
    /// Write one encoded frame (the trailing newline included) and ack the
    /// outcome.
    Write(Vec<u8>),
    /// Drop the stdin handle: the EOF that ends the host's read loop during
    /// a graceful shutdown.
    Close,
}

/// The per-generation reader output queue: plugin diagnostics and reverse
/// queries share one bounded queue and one signal — the handle drains it at
/// its interaction points, and every push wakes a parked waiter so the
/// handle parks instead of polling.
#[derive(Debug, Default)]
struct PendingOutput {
    queues: Mutex<PendingQueues>,
    signal: Condvar,
}

/// The queued halves of [`PendingOutput`].
#[derive(Debug, Default)]
struct PendingQueues {
    diagnostics: Vec<PluginDiagnostic>,
    queries: Vec<PendingQuery>,
}

impl PendingOutput {
    /// Queues one plugin diagnostic and wakes a parked waiter. Past
    /// [`MAX_PENDING_OUTPUT`] combined un-drained rows the queue is a
    /// protocol violation — a plugin flooding output is torn down like any
    /// other contract break. `Err` carries the teardown reason.
    fn push(&self, diagnostic: PluginDiagnostic) -> Result<(), String> {
        let mut queues = self.queues.lock();
        if queues.diagnostics.len() + queues.queries.len() >= MAX_PENDING_OUTPUT {
            return Err(format!(
                "plugin exceeded the {MAX_PENDING_OUTPUT}-row pending cap"
            ));
        }
        queues.diagnostics.push(diagnostic);
        self.signal.notify_all();
        Ok(())
    }

    /// Queues one reverse query (the plugin's own correlation id) and wakes
    /// a parked waiter. Queries do **not** close the analyze demand's
    /// clock — a query round trip eats the plugin's own callback budget
    ///: the clock closes when diagnostics are produced.
    /// Shares the diagnostics fuse (see [`PendingOutput::push`]).
    fn push_query(&self, query: PendingQuery) -> Result<(), String> {
        let mut queues = self.queues.lock();
        if queues.diagnostics.len() + queues.queries.len() >= MAX_PENDING_OUTPUT {
            return Err(format!(
                "plugin exceeded the {MAX_PENDING_OUTPUT}-row pending cap"
            ));
        }
        queues.queries.push(query);
        self.signal.notify_all();
        Ok(())
    }

    /// Takes every queued diagnostic (the diagnostics-only drain).
    fn take_all(&self) -> Vec<PluginDiagnostic> {
        std::mem::take(&mut self.queues.lock().diagnostics)
    }

    /// How many reverse queries are queued (a peek — the queue is left for
    /// [`PendingOutput::take_queries`], `serve_queries`'s handle).
    fn queued_queries(&self) -> usize {
        self.queues.lock().queries.len()
    }

    /// Takes every queued reverse query, leaving diagnostics queued.
    fn take_queries(&self) -> Vec<PendingQuery> {
        std::mem::take(&mut self.queues.lock().queries)
    }

    /// Parks until diagnostics arrived and the reply burst settled, or a
    /// reverse query arrived (a query wakes the wait immediately, no settle:
    /// the plugin is stalled awaiting its answer), or `deadline` passes
    /// (`None`). `Some` carries the queued diagnostics and how many reverse
    /// queries are queued (a peek — the queries stay queued for
    /// `serve_queries`, the one consumer that can answer them). The deadline
    /// bounds the whole wait exactly — the settle window never extends past
    /// it.
    fn wait_for_output(&self, deadline: Instant) -> Option<(Vec<PluginDiagnostic>, usize)> {
        let mut queues = self.queues.lock();
        while queues.diagnostics.is_empty() && queues.queries.is_empty() {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return None;
            };
            self.signal.wait_for(&mut queues, remaining);
        }
        if queues.diagnostics.is_empty() {
            // Queries only: no burst to settle — hand the diagnostics over
            // now; the queries stay queued for the pump.
            return Some((
                std::mem::take(&mut queues.diagnostics),
                queues.queries.len(),
            ));
        }
        // First diagnostic row is in. The plugin answers one analyze cycle
        // with contiguous notification frames; hold through a quiet window
        // so the drain hands over the whole burst, never its first line.
        // Reverse queries arriving inside the window ride along.
        loop {
            let now = Instant::now();
            let settle_end = deadline.min(now + BURST_SETTLE);
            if now >= settle_end {
                break;
            }
            let seen = queues.diagnostics.len();
            let timed_out = self.signal.wait_for(&mut queues, settle_end - now);
            if queues.diagnostics.len() > seen {
                // More of the burst arrived (possibly while the notify was
                // racing this wait): settle again from now.
                continue;
            }
            if timed_out.timed_out() {
                // Quiet for the whole window: the burst is complete.
                break;
            }
            // Spurious wake: keep waiting within the same window.
        }
        Some((
            std::mem::take(&mut queues.diagnostics),
            queues.queries.len(),
        ))
    }
}

/// Bounded tail-of-stderr sink shared with one generation's stderr drain
/// thread. Keeps the last [`STDERR_CAPTURE_CAP`] bytes read (the end of a
/// failing host's output names the failure), drops earlier bytes from the
/// front and counts them, and stops retaining once the handshake succeeded —
/// the drain thread then only keeps the pipe empty. Memory is bounded by the
/// cap however chatty the host's startup is.
#[derive(Debug)]
struct StderrCapture {
    /// Whether bytes are still retained; false from the readiness verdict on.
    retain: AtomicBool,
    buf: Mutex<StderrBuf>,
}

#[derive(Debug, Default)]
struct StderrBuf {
    bytes: Vec<u8>,
    /// Bytes dropped from the front once the cap was exceeded.
    dropped_front: u64,
}

impl StderrCapture {
    fn new() -> Self {
        Self {
            retain: AtomicBool::new(true),
            buf: Mutex::new(StderrBuf::default()),
        }
    }

    /// Absorbes one read from the drain thread. No-op once sealed.
    fn push(&self, chunk: &[u8]) {
        if !self.retain.load(Ordering::Relaxed) {
            return;
        }
        let mut buf = self.buf.lock();
        buf.bytes.extend_from_slice(chunk);
        let excess = buf.bytes.len().saturating_sub(STDERR_CAPTURE_CAP);
        if excess > 0 {
            buf.dropped_front = buf
                .dropped_front
                .saturating_add(u64::try_from(excess).unwrap_or(u64::MAX));
            buf.bytes.drain(..excess);
        }
    }

    /// Readiness verdict reached: discard what was captured and stop
    /// retaining (the drain thread keeps emptying the pipe so the host can
    /// never block on a full one).
    fn seal(&self) {
        self.retain.store(false, Ordering::Relaxed);
        let mut buf = self.buf.lock();
        buf.bytes.clear();
        buf.dropped_front = 0;
    }

    /// The captured text with a front-truncation marker when bytes were
    /// dropped; `None` when nothing was retained.
    fn take_text(&self) -> Option<String> {
        let buf = self.buf.lock();
        if buf.bytes.is_empty() {
            return None;
        }
        let mut text = String::new();
        if buf.dropped_front > 0 {
            use std::fmt::Write as _;
            let _ = writeln!(
                text,
                "[{} earlier bytes truncated; showing the last {}]",
                buf.dropped_front,
                buf.bytes.len()
            );
        }
        text.push_str(&String::from_utf8_lossy(&buf.bytes));
        Some(text)
    }
}

/// The `; plugin stderr: …` suffix that folds a captured host stderr into a
/// failure message — empty when nothing was captured, so unaffected errors
/// keep their exact text.
fn stderr_note(stderr: &str) -> String {
    if stderr.is_empty() {
        String::new()
    } else {
        format!("; plugin stderr: {}", stderr.trim_end())
    }
}

/// The analyze demand's wall clock: closed at
/// birth, opened by [`PluginRuntime::feed`] when a batch goes on the wire,
/// closed when the child produces diagnostics or the generation dies. The
/// stored value is the runtime's age in milliseconds at open time
/// ([`AnalyzeClock::CLOSED`] when closed), so the monitor reads the open
/// demand's age against one shared reference instant without a second
/// atomic.
#[derive(Debug)]
struct AnalyzeClock {
    open_at_millis: AtomicU64,
}

impl AnalyzeClock {
    const CLOSED: u64 = u64::MAX;

    fn new() -> Self {
        Self {
            open_at_millis: AtomicU64::new(Self::CLOSED),
        }
    }

    /// Opens the demand unless already open. A re-feed while open must not
    /// refresh the clock: a plugin that acknowledges every batch without
    /// ever producing would otherwise push the budget out forever and the
    /// monitor could never fire.
    fn open(&self, created: Instant) {
        let _ = self.open_at_millis.compare_exchange(
            Self::CLOSED,
            elapsed_millis(created),
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
    }

    /// Closes the demand: diagnostics arrived (the reader), the stream ended
    /// (the reader's exit — nothing can follow), or the generation was killed
    /// (the kill paths and the monitor's own kill).
    fn close(&self) {
        self.open_at_millis.store(Self::CLOSED, Ordering::Relaxed);
    }

    /// The open demand's age, `None` when closed.
    fn open_for(&self, created: Instant) -> Option<Duration> {
        let open_at = self.open_at_millis.load(Ordering::Relaxed);
        (open_at != Self::CLOSED)
            .then(|| Duration::from_millis(elapsed_millis(created).saturating_sub(open_at)))
    }
}

/// Milliseconds since `created`, clamped below the [`AnalyzeClock::CLOSED`]
/// sentinel.
fn elapsed_millis(created: Instant) -> u64 {
    u64::try_from(created.elapsed().as_millis()).unwrap_or(AnalyzeClock::CLOSED - 1)
}

/// One queued reverse query: the plugin's correlation id plus the parsed
/// request. The caller answers it through [`PluginRuntime::serve_queries`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingQuery {
    /// The plugin's correlation id for this request (its own id space).
    pub id: u64,
    /// The parsed request payload.
    pub request: QueryRequest,
}

/// A reverse-query server: answers one pending query with its reply params
/// (`Err` answers the wire's error channel).
type QueryServer<'a> =
    &'a mut dyn FnMut(&PendingQuery) -> Result<serde_json::Map<String, Value>, String>;

/// One drain of the reader's output queue: the plugin diagnostics the
/// caller merges, plus the count of reverse queries still queued — the
/// caller answers those through [`PluginRuntime::serve_queries`], the one
/// consumer that can write their replies.
#[derive(Debug, Default)]
pub struct CycleDrain {
    /// The diagnostics emitted since the last drain, in arrival order.
    pub diagnostics: Vec<PluginDiagnostic>,
    /// How many reverse queries are queued, awaiting `serve_queries`.
    pub pending_queries: usize,
}

/// One live plugin: child process, session, isolation monitor, counters.
///
/// Created by [`PluginRuntime::spawn`] (eager: first generation at
/// construction) or [`PluginRuntime::lazy`] (first generation at the first
/// interaction); `&mut self` methods make the handle
/// thread the only writer, so the reader thread and monitor never contend
/// with a write.
#[derive(Debug)]
pub struct PluginRuntime {
    spec: SpawnSpec,
    request_timeout: Duration,
    callback_timeout: Duration,
    restart_budget: u32,
    memory_cap_bytes: u64,
    /// Idle-eviction deadline (`None` = disabled): the manifest's
    /// `idle_evict_ms` (`0` disables), clamped up to the runtime's own
    /// interaction window (`request_ms + callback_ms`) so the monitor's
    /// eviction arm can never fire inside an in-flight request — every
    /// interaction touches the clock at entry and concludes within that
    /// window. `DARTQ_PLUGIN_IDLE_EVICT_MS` overrides the manifest value.
    idle_evict: Option<Duration>,

    // Per generation (one child process); replaced on every launch.
    session: Arc<Mutex<Session>>,
    commands: Option<Sender<WriterCommand>>,
    acks: mpsc::Receiver<std::io::Result<()>>,
    child: Arc<Mutex<Option<Child>>>,
    events: mpsc::Receiver<ProtocolEvent>,
    pending: Arc<PendingOutput>,
    analyze_clock: Arc<AnalyzeClock>,
    running: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    /// Set by the monitor's eviction arm before it kills the child: the
    /// generation is dead but the runtime is *resumable* — the next
    /// interaction relaunches free of the restart-budget charge (an
    /// eviction is a host memory action, not a crash).
    evicted: Arc<AtomicBool>,
    /// Milliseconds since `created` at the last interaction (the monitor's
    /// idle clock); touched at every interaction entry and at each launch.
    last_interaction_ms: Arc<AtomicU64>,
    death: Arc<Mutex<Option<String>>>,
    stderr_capture: Arc<StderrCapture>,
    stderr_drain: Option<JoinHandle<()>>,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
    monitor: Option<JoinHandle<()>>,
    negotiated: Option<CapabilitySet>,

    // Across generations.
    created: Instant,
    stats: Arc<PluginStats>,
    restarts_used: u32,
    /// Whether the first generation was already launched: the initial
    /// launch rides [`Self::ensure_alive`] for the lazy form and must run
    /// regardless of the restart budget (the budget prices relaunches).
    launched_once: bool,

    // Config-push dedup: the last pushed config's params and the restart
    // count it was pushed at. A relaunched child starts with no config
    // state, so a restart invalidates the dedup and forces a re-push.
    pushed_config: Option<Map<String, Value>>,
    pushed_at_restarts: u32,
}

impl PluginRuntime {
    /// Launches the plugin process and completes the handshake eagerly.
    ///
    /// The eager form for faces whose first use immediately follows
    /// construction (the fix face). The daemon's analyze face builds
    /// [`Self::lazy`] handles instead: no sidecar process exists until its
    /// capability is first needed, so an unneeded plugin never costs its
    /// resident set.
    ///
    /// A process that dies before answering is relaunched within the
    /// manifest's retry budget (each attempt counted in
    /// [`PluginStatsSnapshot::restarts`]); once the budget is spent the last
    /// failure surfaces.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Spawn`] when the program cannot start,
    /// [`RuntimeError::HandshakeTimeout`] when `initialize` is unanswered in
    /// time, [`RuntimeError::Crashed`] when the process dies or breaks the
    /// wire contract, plus the codec/session error variants.
    pub fn spawn(spec: SpawnSpec) -> Result<Self, RuntimeError> {
        let mut runtime = Self::construct(spec);
        runtime.launched_once = true;
        runtime.launch_with_budget()?;
        Ok(runtime)
    }

    /// Builds the runtime handle **without launching**: no sidecar process
    /// exists until its capability is first needed. The first interaction —
    /// [`Self::feed`], [`Self::wait_output`], [`Self::take_output`],
    /// [`Self::request_fixes`], or an explicit [`Self::ensure_alive`] —
    /// launches the first generation through the same budgeted path every
    /// restart rides. A readiness concern is accepted by construction: the
    /// first request pays the spawn + handshake (budgeted, seconds-scale),
    /// never a boot-time prewarm.
    #[must_use]
    pub fn lazy(spec: SpawnSpec) -> Self {
        Self::construct(spec)
    }

    /// The shared constructor body: budgets resolved, per-generation state
    /// at its empty placeholders, no threads, no child.
    fn construct(spec: SpawnSpec) -> Self {
        let request_timeout = Duration::from_millis(spec.manifest.timeouts.request_ms);
        let callback_timeout = Duration::from_millis(spec.manifest.timeouts.callback_ms);
        let restart_budget = spec.manifest.timeouts.request_retries;
        let memory_cap_bytes = u64::from(spec.manifest.memory_budget_mb) * 1024 * 1024;
        let idle_evict = idle_evict_timeout(&spec.manifest)
            .map(|timeout| timeout.max(request_timeout + callback_timeout));
        let (events_tx, events) = mpsc::channel();
        drop(events_tx); // placeholder replaced by the first launch
        Self {
            spec,
            request_timeout,
            callback_timeout,
            restart_budget,
            memory_cap_bytes,
            idle_evict,
            session: Arc::new(Mutex::new(Session::new(CapabilitySet::all()))),
            commands: None,
            acks: mpsc::channel().1,
            child: Arc::new(Mutex::new(None)),
            events,
            pending: Arc::new(PendingOutput::default()),
            analyze_clock: Arc::new(AnalyzeClock::new()),
            running: Arc::new(AtomicBool::new(false)),
            alive: Arc::new(AtomicBool::new(false)),
            evicted: Arc::new(AtomicBool::new(false)),
            last_interaction_ms: Arc::new(AtomicU64::new(0)),
            death: Arc::new(Mutex::new(None)),
            stderr_capture: Arc::new(StderrCapture::new()),
            stderr_drain: None,
            reader: None,
            writer: None,
            monitor: None,
            negotiated: None,
            stats: Arc::new(PluginStats::default()),
            created: Instant::now(),
            restarts_used: 0,
            launched_once: false,
            pushed_config: None,
            pushed_at_restarts: 0,
        }
    }

    /// The capability set negotiated at the last completed handshake.
    #[must_use]
    pub const fn negotiated(&self) -> Option<CapabilitySet> {
        self.negotiated
    }

    /// The plugin's registration census (the `initialize` reply's `rules`
    /// array), empty until the handshake completed. The caller resolves the
    /// per-cycle enablement against it ([`CyclePush`]).
    #[must_use]
    pub fn rule_census(&self) -> Vec<RuleCensus> {
        self.session.lock().rule_census().to_vec()
    }

    /// Pushes one analyze cycle to the plugin: the resolved config as
    /// `notification/plugin_config` (only when it changed since the last
    /// push — a relaunched child always re-receives it, its config state
    /// died with the old generation), then the engine's current diagnostics
    /// plus the cycle's analyzed file set as one
    /// `notification/engine_diagnostics` batch.
    ///
    /// A cycle with no diagnostics and no analyzed files sends nothing: the
    /// batch would carry no diagnostic context and the rules would have no
    /// files to run over. A cycle with analyzed files but an empty
    /// diagnostic set still pushes — the rules must run over clean files
    /// too, and their findings are the cycle's whole output.
    ///
    /// A child that died since the last call is restarted here (within
    /// budget) before the push. The push opens the analyze clock: the
    /// child must produce diagnostics (or die) within the manifest's
    /// `callback_ms`, or the monitor kills it through the ordinary crash
    /// path.
    ///
    /// # Errors
    ///
    /// Restart/transport failures, per [`RuntimeError`].
    pub fn feed(
        &mut self,
        diagnostics: &[Diagnostic],
        files: &FileTable,
        push: &CyclePush,
    ) -> Result<(), RuntimeError> {
        if diagnostics.is_empty() && push.analyzed.is_empty() {
            return Ok(());
        }
        self.ensure_alive()?;
        // The config frame rides only real change: identical configs
        // compare equal through their wire params, and a fresh generation
        // (restart count moved) always re-receives the current config.
        let config_params = if push.config.is_empty() {
            None
        } else {
            Some(config_params(&push.config))
        };
        if config_params != self.pushed_config || self.pushed_at_restarts != self.restarts_used {
            if let Some(params) = config_params {
                self.write_frame(&plugin_config_frame(&push.config))?;
                self.pushed_config = Some(params);
                self.pushed_at_restarts = self.restarts_used;
            } else {
                // The config emptied: drop the cache so a later non-empty
                // config re-pushes instead of comparing equal to the stale
                // entry.
                self.pushed_config = None;
            }
        }
        let frame = engine_diagnostics_frame(&wire_diagnostics(diagnostics, files), &push.analyzed);
        self.write_frame(&frame)?;
        // CPU budget row, analyze path: the demand's clock opens with the
        // batch on the wire and closes when the child produces diagnostics
        // (reader) or the generation dies — the monitor kills a demand still
        // open past the callback budget.
        self.analyze_clock.open(self.created);
        Ok(())
    }

    /// Drains the reader's diagnostics queue: the diagnostics the plugin
    /// emitted since the last call (`notification/diagnostic`), in arrival
    /// order. Reverse queries are **not** consumed — [`CycleDrain::pending_queries`]
    /// reports how many are queued, and [`PluginRuntime::serve_queries`]
    /// answers them. The caller merges the diagnostics into its output
    /// (namespacing the codes is the output layer's job).
    ///
    /// # Errors
    ///
    /// Restart/transport failures, per [`RuntimeError`].
    pub fn take_output(&mut self) -> Result<CycleDrain, RuntimeError> {
        self.ensure_alive()?;
        Ok(CycleDrain {
            diagnostics: self.pending.take_all(),
            pending_queries: self.pending.queued_queries(),
        })
    }

    /// Whether the analyze demand is still open — `feed` wrote an
    /// `engine_diagnostics` batch the child has yet to answer with a
    /// diagnostic (or die trying). The caller uses this to distinguish
    /// "the drain completed because the plugin quiesced" from "the drain's
    /// budget expired while the plugin was still working": rows of the
    /// second kind land in `pending` later and re-feed on the next cycle.
    /// Read-only; the monitor's own enforcement runs off the same clock.
    #[must_use]
    pub fn analyze_demand_open(&self) -> bool {
        self.analyze_clock.open_for(self.created).is_some()
    }

    /// The manifest's `callback_ms` — the plugin's declared budget for one
    /// analyze demand (and one `request/fixes` round trip). The drain's
    /// outer bound is this budget, matching upstream's `await` of the
    /// plugin's response; a plugin that overruns it is killed by the
    /// monitor through the same path the fixes face uses.
    #[must_use]
    pub const fn callback_timeout(&self) -> Duration {
        self.callback_timeout
    }

    /// [`Self::take_output`] with a wait: parks on the reader's push signal
    /// until diagnostics arrived (holding through one `BURST_SETTLE` quiet
    /// window so a synchronous reply — one analyze cycle answered with
    /// contiguous `notification/diagnostic` frames — drains as the whole
    /// burst, never as its first line) or a reverse query arrived (returned
    /// immediately, no settle — the plugin is stalled awaiting its answer).
    /// The caller's deadline bounds the whole wait exactly (the daemon's
    /// per-cycle drain budget stays exact); there is no poll tick. A child
    /// death during the wait restarts it, within budget: the parked phase
    /// may run to the deadline on a dead child, and the final alive gate
    /// performs the restart before the empty drain returns.
    ///
    /// # Errors
    ///
    /// Restart budget exhaustion and transport failures, per [`RuntimeError`].
    pub fn wait_output(&mut self, timeout: Duration) -> Result<CycleDrain, RuntimeError> {
        // Eager restart of a child that died since the last interaction, so
        // the wait below runs against a live generation (the crash-path
        // posture: restart at the caller's interaction, never lazily).
        self.ensure_alive()?;
        let deadline = Instant::now() + timeout;
        if let Some((diagnostics, pending_queries)) = self.pending.wait_for_output(deadline) {
            return Ok(CycleDrain {
                diagnostics,
                pending_queries,
            });
        }
        // Deadline: one final alive-gated drain — a push racing the expiry is
        // still honored, and a death during the wait gets its restart (or its
        // error) here rather than at the next interaction.
        self.ensure_alive()?;
        let (diagnostics, pending_queries) = self
            .pending
            .wait_for_output(Instant::now())
            .unwrap_or((Vec::new(), 0));
        Ok(CycleDrain {
            diagnostics,
            pending_queries,
        })
    }

    /// Answers the plugin's queued reverse `request/query` frames through
    /// `server`, one reply frame per query under the same correlation id
    /// ([`query_reply_frame`]; a server that declines with `Err` answers the
    /// wire's error channel via [`query_error_frame`] — the session stays
    /// up). Queries are served in arrival order; a write failure surfaces
    /// and leaves the remaining queries queued for the next call. Queries
    /// that arrive while this call runs are served at the caller's next
    /// pump.
    ///
    /// # Errors
    ///
    /// Transport failures per [`RuntimeError`]; the server's own failures
    /// ride the wire as error replies, not through this result.
    pub fn serve_queries(&mut self, server: QueryServer<'_>) -> Result<usize, RuntimeError> {
        let queries = self.pending.take_queries();
        let count = queries.len();
        for query in &queries {
            match server(query) {
                Ok(params) => self.write_frame(&query_reply_frame(query.id, params))?,
                Err(message) => self.write_frame(&query_error_frame(query.id, &message))?,
            }
        }
        Ok(count)
    }

    /// Declines the plugin's queued reverse queries with error replies —
    /// the posture of a face whose session negotiated `query` but has no
    /// server at this interaction (a querying producer fails immediately
    /// with a typed message instead of stalling its own reply past the
    /// callback budget).
    ///
    /// # Errors
    ///
    /// Transport failures per [`RuntimeError`].
    pub fn decline_queued_queries(&mut self) -> Result<(), RuntimeError> {
        let queries = self.pending.take_queries();
        for query in &queries {
            self.write_frame(&query_error_frame(
                query.id,
                "the `query` face has no server on this session (queries are only served on \
                 the analyze face)",
            ))?;
        }
        Ok(())
    }

    /// Sends `request/fixes` for `file` at the given UTF-16 range and awaits
    /// the reply under the manifest's `callback_ms` budget.
    /// Requires the `fix` capability to have been negotiated.
    ///
    /// A producer that queries while its fixes are computed stalls its own
    /// reply, so the wait pumps the plugin's reverse queries: this
    /// signature-less form has no query server and **declines** every query
    /// queued while the reply is awaited (an immediate typed error reply on
    /// the wire — the querying producer fails fast instead of timing out);
    /// [`Self::request_fixes_serving`] is the serving form for faces that
    /// can answer.
    ///
    /// A child that died since the last call is restarted here (within
    /// budget) before the request. An overrun kill consumes one restart and
    /// relaunches before the error surfaces, so the next call meets a fresh
    /// child; a spent restart budget turns the failure into the terminal
    /// [`RuntimeError::Crashed`].
    ///
    /// # Errors
    ///
    /// [`RuntimeError::CallbackTimeout`] when the reply misses the callback
    /// budget (child killed and restarted), [`RuntimeError::Crashed`] when
    /// the child dies or breaks the contract mid-request, session errors for
    /// a non-ready session or an unnegotiated `fix` capability, plus
    /// transport failures.
    pub fn request_fixes(
        &mut self,
        file: &str,
        offset: u64,
        length: u64,
    ) -> Result<FixesReply, RuntimeError> {
        self.request_fixes_inner(file, offset, length, None)
    }

    /// [`Self::request_fixes`] with a query server: the plugin's reverse
    /// queries queued while the fixes reply is awaited are answered through
    /// `server` (`Err` answers the wire's error channel — the session stays
    /// up). This is the form a query-capable face drives producers with;
    /// the plain form declines instead.
    ///
    /// # Errors
    ///
    /// Per [`Self::request_fixes`]; the server's own failures ride the wire
    /// as error replies, not through this result.
    pub fn request_fixes_serving(
        &mut self,
        file: &str,
        offset: u64,
        length: u64,
        server: QueryServer<'_>,
    ) -> Result<FixesReply, RuntimeError> {
        self.request_fixes_inner(file, offset, length, Some(server))
    }

    fn request_fixes_inner(
        &mut self,
        file: &str,
        offset: u64,
        length: u64,
        mut server: Option<QueryServer<'_>>,
    ) -> Result<FixesReply, RuntimeError> {
        self.ensure_alive()?;
        let frame = self
            .session
            .lock()
            .send_fixes_request(file, offset, length)?;
        let request_id = frame.id.expect("fixes requests carry an id");
        self.write_frame(&frame)?;

        let callback_ms = self.callback_timeout.as_millis();
        let deadline = Instant::now() + self.callback_timeout;
        loop {
            match self
                .events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(ProtocolEvent::FixesReply { id, reply }) => {
                    if id != request_id {
                        return Err(RuntimeError::Crashed {
                            reason: format!(
                                "`request/fixes` reply id {id} does not match the outstanding \
                                 request {request_id}"
                            ),
                        });
                    }
                    return Ok(reply);
                }
                // The plugin queried while its fixes were being computed:
                // pump now, or the producer stalls its own reply past the
                // budget. A queued query the pump cannot answer here gets
                // the immediate error reply (the session stays up).
                Ok(ProtocolEvent::QueriesQueued) => match server.as_deref_mut() {
                    Some(server) => {
                        self.serve_queries(server)?;
                    }
                    None => self.decline_queued_queries()?,
                },
                // The death slot is authoritative (the reader records the
                // cause before sending the event; killers record before
                // killing), so build the error from it — which also adds the
                // uniform restart-budget suffix at the accounting point. A
                // closed event channel is the same death observed from the
                // reader side.
                Ok(ProtocolEvent::Dead) | Err(RecvTimeoutError::Disconnected) => {
                    return Err(self.crash_error());
                }
                Ok(ProtocolEvent::Initialized(_)) => {
                    return Err(RuntimeError::Crashed {
                        reason: "plugin re-handshaked mid-session".to_owned(),
                    });
                }
                Ok(ProtocolEvent::ShutdownComplete) => {
                    return Err(RuntimeError::Crashed {
                        reason: "plugin answered shutdown before request/fixes".to_owned(),
                    });
                }
                Err(RecvTimeoutError::Timeout) => {
                    // CPU budget row: a callback that overruns its budget is a
                    // misbehaving child — kill it and consume one restart so the
                    // consumption is observable at this error site.
                    let reason = format!(
                        "`request/fixes` was not answered within the {callback_ms} ms callback \
                         budget"
                    );
                    *self.death.lock() = Some(reason);
                    self.kill_child();
                    self.ensure_alive()?;
                    return Err(RuntimeError::CallbackTimeout {
                        ms: u64::try_from(callback_ms).unwrap_or(u64::MAX),
                    });
                }
            }
        }
    }

    /// The isolation metrics snapshot.
    #[must_use]
    pub fn stats(&self) -> PluginStatsSnapshot {
        self.stats.snapshot()
    }

    /// Records one analyze cycle's interaction cost with this plugin — the
    /// caller's feed-plus-drain wall time and whether the drain budget
    /// expired before the plugin quiesced ([`PluginStatsSnapshot::degraded_cycles`]).
    /// The per-plugin overhead attribution face (counters only; the absent
    /// path never constructs a runtime, so no timer arms without plugins).
    pub fn observe_cycle(&self, busy: Duration, degraded: bool) {
        self.stats.observe_cycle(busy, degraded);
    }

    /// The graceful teardown: `shutdown` request, reply awaited under the
    /// request budget, then the handle closes the plugin's stdin (the host's
    /// graceful exit signal) and kills the child before joining the
    /// generation, so the teardown never waits past that budget for a plugin
    /// to exit. Consumes the runtime — after `shutdown` there is no session
    /// left (wire contract).
    ///
    /// A child already dead when `shutdown` is called surfaces its crash.
    ///
    /// A plugin diagnostic still in flight when the request goes out (the
    /// host wrote it before seeing the request) arrives while the reply is
    /// awaited; it is queued and dropped with the runtime — tolerated, never
    /// a crash.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::ShutdownTimeout`] (child killed), [`RuntimeError::Crashed`]
    /// (dead or protocol-violating plugin), session/transport variants.
    pub fn shutdown(mut self) -> Result<(), RuntimeError> {
        if self.evicted.load(Ordering::Relaxed) {
            // An evicted generation is already dead and reaped (the
            // monitor's eviction arm): no handshake exists to run. Close
            // the writer and join what remains — a clean teardown, no
            // shutdown error.
            self.join_generation();
            return Ok(());
        }
        if !self.alive.load(Ordering::Relaxed) {
            self.reap_child();
            return Err(self.crash_error());
        }
        let frame = self.session.lock().send_shutdown()?;
        self.write_frame(&frame)?;

        // One budget for the whole wait: a crossing query re-awaits within
        // the same deadline, so a chatty teardown cannot extend it.
        let deadline = self.request_deadline();
        let outcome = loop {
            match self
                .events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(ProtocolEvent::ShutdownComplete) => break Ok(()),
                // Authoritative death slot, as in `request_fixes` (see there);
                // a closed channel is the same death from the reader side.
                Ok(ProtocolEvent::Dead) | Err(RecvTimeoutError::Disconnected) => {
                    break Err(self.crash_error());
                }
                Ok(ProtocolEvent::Initialized(_)) => {
                    break Err(RuntimeError::Crashed {
                        reason: "plugin re-handshaked mid-session".to_owned(),
                    });
                }
                // Unreachable through the session machine (shutdown is sent
                // only from Ready with the fixes reply already consumed); a
                // stray reply is a contract break like any other.
                Ok(ProtocolEvent::FixesReply { .. }) => {
                    break Err(RuntimeError::Crashed {
                        reason: "unsolicited request/fixes reply".to_owned(),
                    });
                }
                // A query crossing the shutdown request was queued (the
                // session machine tolerates the window); the session is
                // terminating and the query dies with the runtime — no
                // answer is coming, which the host's own teardown absorbs.
                // Keep waiting for the reply within the same deadline.
                Ok(ProtocolEvent::QueriesQueued) => {}
                Err(RecvTimeoutError::Timeout) => {
                    self.kill_child();
                    break Err(RuntimeError::ShutdownTimeout {
                        ms: u64::try_from(self.request_timeout.as_millis()).unwrap_or(u64::MAX),
                    });
                }
            }
        };
        if outcome.is_ok() {
            // Session over (wire contract). Closing stdin first is the
            // graceful signal: a well-behaved host ends its read loop on the
            // EOF and exits on its own, no kill needed. The writer thread
            // owns the handle, so the close is a command to it.
            if let Some(commands) = &self.commands
                && commands.send(WriterCommand::Close).is_ok()
            {
                // The writer is idle here (the last write acked); the deadline
                // only keeps a pathological ack from stalling teardown.
                let _ = self.acks.recv_timeout(
                    self.request_deadline()
                        .saturating_duration_since(Instant::now()),
                );
            }
            // Kill before joining: the reply is acked and the session is
            // Terminated, so nothing of value is left in the child, but a
            // plugin that hangs in a cleanup handler never closes its stdout
            // — the reader below stays parked in `fill_buf` — and both the
            // join and the reap would wait forever, wedging the caller past
            // any budget. The kill closes the pipes and ends the reader
            // deterministically (and reaps the child, a no-op when it already
            // exited on the stdin EOF).
            self.kill_child();
            self.join_generation();
        }
        outcome
    }

    // --- generation management ---

    fn launch_with_budget(&mut self) -> Result<(), RuntimeError> {
        let mut outcome = self.launch_once();
        while outcome.is_err() && self.restarts_used < self.restart_budget {
            self.teardown_generation();
            self.stats.restart();
            self.restarts_used += 1;
            outcome = self.launch_once();
        }
        outcome
    }

    /// Ensures a live generation, launching on demand — the single path
    /// every interaction's first launch and every restart rides.
    ///
    /// Three states, in order: a live child is a clock touch and an atomic
    /// load; an evicted generation (the monitor's idle arm) relaunches
    /// budget-free — an eviction is a host memory action, not a crash; and
    /// a dead generation relaunches within the restart budget, whose
    /// exhaustion is terminal. The very first launch (the lazy form) runs
    /// regardless of that budget: the budget prices *relaunches*, and the
    /// eager form already launches once at `request_retries: 0`.
    ///
    /// # Errors
    ///
    /// Restart-budget exhaustion and launch failures, per [`RuntimeError`].
    pub fn ensure_alive(&mut self) -> Result<(), RuntimeError> {
        if self.alive.load(Ordering::Relaxed) {
            self.touch();
            return Ok(());
        }
        if self.evicted.load(Ordering::Relaxed) {
            // Join the evicted generation's threads (the monitor already
            // killed and reaped the child), then relaunch fresh. The config
            // cache drops too: the new generation has no config state, and
            // an eviction — unlike a crash — does not move `restarts_used`,
            // so the feed-side dedup would otherwise suppress the re-push.
            self.teardown_generation();
            self.pushed_config = None;
            return self.launch_with_budget();
        }
        if !self.launched_once {
            self.launched_once = true;
            return self.launch_with_budget();
        }
        if self.restarts_used >= self.restart_budget {
            return Err(self.crash_error());
        }
        self.teardown_generation();
        self.stats.restart();
        self.restarts_used += 1;
        self.launch_with_budget()
    }

    /// Stamps the idle clock (milliseconds since `created`): the monitor's
    /// eviction arm measures inactivity against this. Called at every
    /// interaction entry (via this path — `feed`, `wait_output`,
    /// `take_output`, and `request_fixes` all open with
    /// [`Self::ensure_alive`]) and at each launch.
    fn touch(&self) {
        self.last_interaction_ms
            .store(elapsed_millis(self.created), Ordering::Relaxed);
    }

    /// The capabilities the core offers at the handshake: everything the
    /// protocol defines, minus `query` when this session has no query
    /// server — negotiation must honestly represent what this session can
    /// serve, so a querying producer on a fix face sees the capability
    /// absent at the host side instead of stalling against a core that
    /// cannot answer.
    fn local_capabilities(serve_queries: bool) -> CapabilitySet {
        let mut caps = CapabilitySet::all();
        if !serve_queries {
            caps.remove(Capability::Query);
        }
        caps
    }

    /// Spawns one child generation and completes its handshake.
    fn launch_once(&mut self) -> Result<(), RuntimeError> {
        let mut command = Command::new(&self.spec.program);
        command
            .args(&self.spec.args)
            .current_dir(&self.spec.current_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(self.spec.stderr.stdio());
        // Unix: the child leads its own process group (pgid == its pid), so
        // every kill below can signal the whole group. A plugin that spawns
        // descendants inheriting its stdout would otherwise keep the pipe's
        // write end open past a direct kill — no EOF, and the generation's
        // reader stays parked in `fill_buf` forever.
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn().map_err(|source| RuntimeError::Spawn {
            program: self.spec.program.clone(),
            source,
        })?;
        let pid = child.id();
        // Both pipes were just piped: the invariants hold by construction.
        let stdin = child.stdin.take().expect("stdin piped");
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take();

        // Fresh per-generation state (the previous generation's Arcs stay
        // with its threads, which wind down on their own flags). Installed
        // BEFORE the worker threads: a thread the OS cannot create below
        // then has the full generation shape to tear down — the death slot
        // to record the cause in and the child to kill — the ordinary
        // death path instead of aborting the host.
        self.session = Arc::new(Mutex::new(Session::new(Self::local_capabilities(
            self.spec.serve_queries,
        ))));
        let (events_tx, events_rx) = mpsc::channel();
        self.events = events_rx;
        self.pending = Arc::new(PendingOutput::default());
        self.analyze_clock = Arc::new(AnalyzeClock::new());
        self.running = Arc::new(AtomicBool::new(true));
        self.alive = Arc::new(AtomicBool::new(true));
        self.evicted = Arc::new(AtomicBool::new(false));
        self.last_interaction_ms = Arc::new(AtomicU64::new(elapsed_millis(self.created)));
        self.death = Arc::new(Mutex::new(None));
        self.negotiated = None;
        *self.child.lock() = Some(child);

        // Startup stderr face: `Capture` pipes the host's stderr into the
        // bounded capture, drained by its own thread (the host can never
        // fill and stall on it); `Null`/`Inherit` leave no capture. Fresh
        // per generation, like every other per-generation field.
        self.stderr_capture = Arc::new(StderrCapture::new());
        self.stderr_drain = match stderr {
            Some(stderr) => Some(
                spawn_stderr_drain(stderr, Arc::clone(&self.stderr_capture))
                    .map_err(|source| self.thread_spawn_failed("stderr drain", source))?,
            ),
            None => None,
        };

        let (commands, commands_rx) = mpsc::channel();
        let (acks, acks_rx) = mpsc::channel();
        self.commands = Some(commands);
        self.acks = acks_rx;
        let handles = GenerationHandles {
            session: Arc::clone(&self.session),
            pending: Arc::clone(&self.pending),
            events: events_tx,
            clock: Arc::clone(&self.analyze_clock),
            running: Arc::clone(&self.running),
            alive: Arc::clone(&self.alive),
            evicted: Arc::clone(&self.evicted),
            last_interaction_ms: Arc::clone(&self.last_interaction_ms),
            death: Arc::clone(&self.death),
            stats: Arc::clone(&self.stats),
        };
        let reader = spawn_reader(stdout, handles.clone())
            .map_err(|source| self.thread_spawn_failed("reader", source))?;
        self.reader = Some(reader);
        let writer = spawn_writer(stdin, commands_rx, acks)
            .map_err(|source| self.thread_spawn_failed("writer", source))?;
        self.writer = Some(writer);
        let monitor = spawn_monitor(
            handles,
            pid,
            self.memory_cap_bytes,
            self.callback_timeout,
            self.idle_evict,
            self.created,
            Arc::clone(&self.child),
        )
        .map_err(|source| self.thread_spawn_failed("monitor", source))?;
        self.monitor = Some(monitor);

        // Handshake: one `initialize`, reply awaited under the request budget.
        let init = self.session.lock().send_initialize()?;
        self.write_frame(&init)?;
        let handshake = match self.events.recv_timeout(
            self.request_deadline()
                .saturating_duration_since(Instant::now()),
        ) {
            Ok(ProtocolEvent::Initialized(caps)) => {
                self.negotiated = Some(caps);
                Ok(())
            }
            // Authoritative death slot, as in `request_fixes` (see there);
            // a closed channel is the same death from the reader side.
            Ok(ProtocolEvent::Dead) | Err(RecvTimeoutError::Disconnected) => {
                Err(self.crash_error())
            }
            Ok(ProtocolEvent::ShutdownComplete) => Err(RuntimeError::Crashed {
                reason: "plugin answered shutdown before initialize".to_owned(),
            }),
            // Unreachable through the session machine (no fixes request is
            // outstanding during the handshake); a stray reply is a contract
            // break like any other.
            Ok(ProtocolEvent::FixesReply { .. }) => Err(RuntimeError::Crashed {
                reason: "unsolicited request/fixes reply".to_owned(),
            }),
            // Unreachable before the handshake completes (nothing has been
            // fed, and no query is negotiable yet).
            Ok(ProtocolEvent::QueriesQueued) => Err(RuntimeError::Crashed {
                reason: "plugin queried before the handshake completed".to_owned(),
            }),
            Err(RecvTimeoutError::Timeout) => {
                self.kill_child();
                Err(RuntimeError::HandshakeTimeout {
                    ms: u64::try_from(self.request_timeout.as_millis()).unwrap_or(u64::MAX),
                    stderr: String::new(),
                })
            }
        };
        match handshake {
            // Readiness verdict reached: stop retaining — the drain thread
            // keeps emptying the pipe, but nothing more is captured. The
            // fresh generation starts its idle window now.
            Ok(()) => {
                self.stderr_capture.seal();
                self.touch();
                Ok(())
            }
            Err(error) => Err(self.attach_startup_stderr(error)),
        }
    }

    /// Stops the monitor, reaps the child, joins the reader and writer — for
    /// the generation being replaced. Must run before a new launch overwrites
    /// the per-generation fields. The writer's command channel is dropped
    /// first so a parked writer exits even when the kill cannot unblock it.
    ///
    /// The joins are bounded: the kill closes the pipes deterministically
    /// (on Unix the group kill reaches descendants that inherited them), but
    /// a descendant that escaped the group keeps the write end open and the
    /// reader parked with no EOF. Waiting unbounded would turn a budgeted
    /// enforcement kill into a permanent hang; past the bound the
    /// thread is detached and the relaunch proceeds — see [`join_bounded`]
    /// for why detaching is safe.
    fn teardown_generation(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        self.commands = None;
        self.kill_child();
        // The generation's stderr drain ends with the child (its pipe closes
        // on death); the bound keeps a wedged drain from stalling a relaunch.
        if let Some(drain) = self.stderr_drain.take() {
            join_bounded(drain);
        }
        if let Some(reader) = self.reader.take() {
            join_bounded(reader);
        }
        if let Some(writer) = self.writer.take() {
            join_bounded(writer);
        }
        // Monitors exit within one poll tick; they hold only their own
        // generation's Arcs and are detached here.
        self.monitor = None;
    }

    fn join_generation(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        self.commands = None;
        if let Some(drain) = self.stderr_drain.take() {
            let _ = drain.join();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        if let Some(monitor) = self.monitor.take() {
            let _ = monitor.join();
        }
    }

    /// Kills the child and reaps it (the `wait` is what prevents zombies).
    /// Also flips `alive`: the killer knows the death deterministically, the
    /// reader's EOF only later — and a write into the dead child's pipe must
    /// never be attempted.
    fn kill_child(&mut self) {
        // The demand ends with the generation: a killed child can no longer
        // produce diagnostics, and a stale-open clock must not outlive it.
        self.analyze_clock.close();
        self.alive.store(false, Ordering::Relaxed);
        let child = self.child.lock().take();
        if let Some(mut child) = child {
            kill_and_reap(&mut child);
        }
    }

    fn reap_child(&mut self) {
        let child = self.child.lock().take();
        if let Some(mut child) = child {
            let _ = child.wait();
        }
    }

    fn crash_error(&self) -> RuntimeError {
        let death = self.death.lock().clone();
        let mut reason = death.unwrap_or_else(|| "plugin process is no longer alive".to_owned());
        if self.restarts_used >= self.restart_budget {
            reason = format!(
                "{reason}; restart budget of {} exhausted",
                self.restart_budget
            );
        }
        RuntimeError::Crashed { reason }
    }

    /// The death path for a per-generation worker whose thread the OS could
    /// not create (fd or memory exhaustion): record the cause
    /// first-writer-wins in the fresh generation's death slot, kill the
    /// child, and surface the ordinary crash — [`Self::launch_with_budget`]
    /// prices the relaunch from the restart budget, and only its exhaustion
    /// is terminal. A host too starved to create one thread degrades exactly
    /// like a plugin that will not start: one surfaced tool diagnostic,
    /// never an abort of the daemon.
    fn thread_spawn_failed(&mut self, thread: &str, source: std::io::Error) -> RuntimeError {
        let mut slot = self.death.lock();
        if slot.is_none() {
            *slot = Some(format!("{thread} thread could not be spawned: {source}"));
        }
        drop(slot);
        self.kill_child();
        self.crash_error()
    }

    /// Binds the captured startup stderr to a startup failure. Waits
    /// (bounded) for the drain thread's EOF — the host is dead at this
    /// point, so its stderr pipe closes and the thread exits in
    /// microseconds, carrying every byte the host wrote — then folds the
    /// captured text into the surfaced error: a startup failure never
    /// reports a bare "host died" where the host itself said why (the
    /// official plugin manager's failure report does the same for its
    /// subprocesses). Errors that carry no startup stderr pass through
    /// unchanged.
    fn attach_startup_stderr(&mut self, error: RuntimeError) -> RuntimeError {
        if let Some(drain) = self.stderr_drain.take() {
            join_bounded(drain);
        }
        let Some(stderr) = self.stderr_capture.take_text() else {
            return error;
        };
        match error {
            RuntimeError::Crashed { reason } => RuntimeError::Crashed {
                reason: format!("{reason}{}", stderr_note(&stderr)),
            },
            RuntimeError::HandshakeTimeout { ms, .. } => {
                RuntimeError::HandshakeTimeout { ms, stderr }
            }
            error => error,
        }
    }

    fn request_deadline(&self) -> Instant {
        Instant::now() + self.request_timeout
    }

    /// Sends one frame through the writer thread and awaits its ack under
    /// the request deadline. The only stdin writer is the writer thread, so
    /// a child that stops draining its pipe blocks that thread — never the
    /// handle — and the deadline turns the stall into a protocol violation:
    /// the child is killed, the death recorded, and the restart happens at
    /// the caller's next interaction (like every crash path).
    ///
    /// # Errors
    ///
    /// Codec failures, [`RuntimeError::Crashed`] on a write deadline overrun
    /// or a dead writer, and transport I/O errors acked by the writer.
    fn write_frame(&mut self, frame: &Frame) -> Result<(), RuntimeError> {
        let mut bytes = encode_frame(frame)?;
        bytes.push(b'\n');
        let byte_len = bytes.len();
        // Outbound twin of the inbound [`FrameDecoder`] guard, with the same
        // raw-wire accounting (content plus terminator) and the same typed
        // error: what passes here is exactly what the child's decoder accepts.
        // An over-limit batch sent anyway would kill the child's decoder, and
        // re-feeding it after the relaunch would kill the next child too — a
        // deterministic restart loop burning the whole budget into a crash
        // that blames the plugin. Rejecting on this side names the cause,
        // counts the drop, and leaves the innocent child running.
        if byte_len > MAX_FRAME_SIZE {
            self.stats.frame_dropped();
            return Err(ProtocolError::TooLarge {
                limit: MAX_FRAME_SIZE,
            }
            .into());
        }
        let commands = self.commands.as_ref().expect("writer present while alive");
        if commands.send(WriterCommand::Write(bytes)).is_err() {
            // The writer exited: the generation is winding down.
            return Err(self.crash_error());
        }
        match self.acks.recv_timeout(self.request_timeout) {
            Ok(Ok(())) => {
                self.stats.frame_out();
                self.stats
                    .bytes_out(u64::try_from(byte_len).unwrap_or(u64::MAX));
                Ok(())
            }
            Ok(Err(e)) => Err(e.into()),
            Err(RecvTimeoutError::Timeout) => {
                let reason = format!(
                    "plugin stopped reading its input pipe (stdin write exceeded the {} ms \
                     request budget)",
                    self.request_timeout.as_millis()
                );
                *self.death.lock() = Some(reason);
                self.kill_child();
                Err(self.crash_error())
            }
            Err(RecvTimeoutError::Disconnected) => Err(self.crash_error()),
        }
    }
}

impl Drop for PluginRuntime {
    fn drop(&mut self) {
        self.kill_child();
        self.running.store(false, Ordering::Relaxed);
    }
}

/// Kills one generation's child and reaps it. On Unix the child leads its
/// own process group (spawned with `process_group(0)`), so the negative-pid
/// `SIGKILL` takes its descendants down with it — a descendant that
/// inherited the child's stdout would otherwise hold the pipe's write end
/// open past a direct kill, leaving the generation's reader parked in
/// `fill_buf` with no EOF. The direct kill stays as the fallback everywhere
/// (and is a no-op once the group kill already landed); the `wait` reaps the
/// child so nothing leaks. Shared by every kill site — the handle-side
/// `kill_child` and the monitor's budget arms — so no enforcement path can
/// leave a survivor holding the pipes.
fn kill_and_reap(child: &mut Child) {
    #[cfg(unix)]
    {
        // A real pid fits `pid_t` (i32) on every supported Unix.
        if let Ok(pid) = i32::try_from(child.id()) {
            // Safety: `kill(2)` only signals; the negative pid targets the
            // child's own process group, never ours.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Joins `handle`, giving the worker [`TEARDOWN_JOIN_BOUND`] to exit; past
/// the bound the handle is dropped and the thread detaches. Detaching is
/// safe because every worker holds only its generation's
/// [`GenerationHandles`] Arcs, and [`PluginRuntime::launch_once`] replaces
/// all of them with fresh state before the next generation interacts: a
/// detached reader/writer unwinds on its own copies when its pipe closes,
/// and its late `Dead` event lands in the old generation's event channel —
/// whose receiver is dropped by the relaunch — or fails harmlessly. The one
/// cross-generation Arc (`stats`) is append-only atomics, so a late byte
/// count from the dying generation is correct accounting, not corruption.
/// The cost of a detached thread is bounded by the surviving descendant's
/// lifetime: one parked thread and one pipe fd.
fn join_bounded(handle: JoinHandle<()>) {
    let deadline = Instant::now() + TEARDOWN_JOIN_BOUND;
    while !handle.is_finished() {
        if Instant::now() >= deadline {
            return; // detach: the thread winds down on its own Arcs
        }
        std::thread::sleep(JOIN_POLL);
    }
    let _ = handle.join();
}

// --- threads ---

/// The Arc'd per-generation state the worker threads share with the runtime.
/// Cloning only bumps refcounts; each spawn site moves the bundle (or a
/// clone) instead of threading eight separate parameters.
#[derive(Clone)]
struct GenerationHandles {
    session: Arc<Mutex<Session>>,
    pending: Arc<PendingOutput>,
    events: Sender<ProtocolEvent>,
    clock: Arc<AnalyzeClock>,
    running: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    evicted: Arc<AtomicBool>,
    last_interaction_ms: Arc<AtomicU64>,
    death: Arc<Mutex<Option<String>>>,
    stats: Arc<PluginStats>,
}

/// Spawns the per-generation reader: the only reader of the child's stdout
/// and the driver of the [`Session`] state machine behind the shared mutex.
/// Returns `Err` when the OS cannot create the thread — the caller routes
/// that through [`PluginRuntime::thread_spawn_failed`], never an abort.
fn spawn_reader(stdout: ChildStdout, h: GenerationHandles) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("dartq-plugin-reader".to_owned())
        .spawn(move || {
            let outcome = run_reader(
                BufReader::new(stdout),
                &h.session,
                &h.pending,
                &h.events,
                &h.stats,
                &h.clock,
            );
            // The stream is over: no diagnostic can ever arrive on this
            // generation, so the demand's clock closes here — the natural
            // -death twin of the kill paths. Left open, a caller that idles
            // past the callback budget hands the monitor an open clock on a
            // corpse and its budget arm fires, overwriting the death reason
            // recorded below.
            h.clock.close();
            h.alive.store(false, Ordering::Relaxed);
            if let Err(reason) = outcome {
                // First-writer-wins on the death slot: every deterministic
                // killer (monitor budget arms, request/write deadlines)
                // records the cause BEFORE flipping `alive`/killing, so this
                // EOF is only the downstream observation of that kill and
                // must not clobber the authoritative record. A genuine
                // plugin-initiated close still lands — no prior writer.
                let mut slot = h.death.lock();
                if slot.is_none() {
                    *slot = Some(reason);
                }
                drop(slot);
                let _ = h.events.send(ProtocolEvent::Dead);
            }
        })
}

/// Reads frames until the stream ends or the contract breaks. `Ok(())` only
/// when the plugin answered `shutdown` (the clean exit).
///
/// Framing is delegated to [`FrameDecoder`] — the single codec authority,
/// with the same raw-byte bound and terminator handling as the Dart host —
/// so this loop only chunks bytes and drives the session.
fn run_reader(
    mut stdout: impl BufRead,
    session: &Mutex<Session>,
    pending: &PendingOutput,
    events: &Sender<ProtocolEvent>,
    stats: &PluginStats,
    clock: &AnalyzeClock,
) -> Result<(), String> {
    let mut decoder = FrameDecoder::default();
    loop {
        let chunk = match stdout.fill_buf() {
            Ok(chunk) => chunk,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.to_string()),
        };
        if chunk.is_empty() {
            // EOF: a buffered partial frame means the stream died mid-frame;
            // otherwise the clean end is a session that saw `shutdown`.
            if decoder.has_partial() {
                return Err("stream ended mid-frame".to_owned());
            }
            let terminated = session.lock().state() == SessionState::Terminated;
            return if terminated {
                Ok(())
            } else {
                Err("plugin process closed the connection".to_owned())
            };
        }
        let chunk_len = chunk.len();
        stats.bytes_in(u64::try_from(chunk_len).unwrap_or(u64::MAX));
        let frames = match decoder.push(chunk) {
            Ok(frames) => frames,
            Err(ProtocolError::TooLarge { .. }) => {
                stats.frame_dropped();
                return Err(format!(
                    "frame exceeds the {MAX_FRAME_SIZE}-byte frame limit"
                ));
            }
            Err(e) => return Err(e.to_string()),
        };
        stdout.consume(chunk_len);
        for frame in frames {
            stats.frame_in();
            let incoming = session.lock().handle(frame);
            match incoming {
                Ok(Incoming::Initialized(caps)) => {
                    let _ = events.send(ProtocolEvent::Initialized(caps));
                }
                Ok(Incoming::Diagnostic(diagnostic)) => {
                    // CPU budget row: produced diagnostics close the analyze
                    // demand's clock.
                    clock.close();
                    pending.push(diagnostic)?;
                }
                Ok(Incoming::ShutdownComplete) => {
                    let _ = events.send(ProtocolEvent::ShutdownComplete);
                    return Ok(());
                }
                Ok(Incoming::Fixes { id, reply }) => {
                    let _ = events.send(ProtocolEvent::FixesReply { id, reply });
                }
                Ok(Incoming::Query { id, request }) => {
                    // The reverse direction: queue for the handle's next
                    // pump and wake its wait loops. A query does not close
                    // the analyze clock — the round trip eats the plugin's
                    // own callback budget.
                    pending.push_query(PendingQuery { id, request })?;
                    let _ = events.send(ProtocolEvent::QueriesQueued);
                }
                Err(e) => return Err(e.to_string()),
            }
        }
    }
}

/// Spawns the per-generation stdin writer: the only thread writing to the
/// child, so a full pipe blocks it instead of the runtime handle, and the
/// handle-side deadline in [`PluginRuntime::write_frame`] bounds every write.
/// Returns `Err` when the OS cannot create the thread — the caller routes
/// that through [`PluginRuntime::thread_spawn_failed`], never an abort.
fn spawn_writer(
    mut stdin: ChildStdin,
    commands: mpsc::Receiver<WriterCommand>,
    acks: Sender<std::io::Result<()>>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("dartq-plugin-writer".to_owned())
        .spawn(move || {
            while let Ok(command) = commands.recv() {
                match command {
                    WriterCommand::Write(bytes) => {
                        let outcome = stdin.write_all(&bytes).and_then(|()| stdin.flush());
                        let alive = outcome.is_ok();
                        // Unbounded channel: the ack never blocks the writer,
                        // so a failed write still lets the thread exit even
                        // when the handle stopped listening.
                        let _ = acks.send(outcome);
                        if !alive {
                            return; // pipe broken; the generation is over
                        }
                    }
                    // Graceful shutdown: dropping the handle is the EOF that
                    // ends the host's read loop.
                    WriterCommand::Close => {
                        drop(stdin);
                        let _ = acks.send(Ok(()));
                        return;
                    }
                }
            }
        })
}

/// Spawns the per-generation stderr drain: the only reader of the child's
/// stderr, so the host can write as much as it likes without ever blocking
/// on a full pipe. Everything read is offered to the bounded capture (which
/// discards once the handshake succeeded); the thread ends at the pipe's
/// EOF — the child's death — and its bounded join at a failure site is what
/// makes the final bytes deterministic in the surfaced error. Returns `Err`
/// when the OS cannot create the thread — the caller routes that through
/// [`PluginRuntime::thread_spawn_failed`], never an abort.
fn spawn_stderr_drain(
    stderr: ChildStderr,
    capture: Arc<StderrCapture>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("dartq-plugin-stderr".to_owned())
        .spawn(move || {
            let mut stderr = BufReader::new(stderr);
            let mut chunk = [0u8; STDERR_READ_CHUNK];
            loop {
                match stderr.read(&mut chunk) {
                    Ok(0) => return, // EOF: the host's stderr is closed
                    Ok(n) => capture.push(&chunk[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => return, // pipe gone with the host
                }
            }
        })
}

/// The monitor's RSS arm: armed at birth when the platform has a sampler,
/// disarmed observably when sampling proves impossible. A sample can fail
/// transiently on a healthy child (a racing proc read, an interrupted
/// syscall), so the arm tolerates [`RSS_SAMPLE_TOLERANCE`] consecutive
/// failures — any delivered sample resets the count — and only then gives
/// up. A disarm covers the RSS face alone; the monitor thread keeps running
/// the analyze clock either way.
#[derive(Debug)]
struct RssWatch {
    armed: bool,
    consecutive_failures: u32,
}

impl RssWatch {
    fn new() -> Self {
        Self {
            armed: RSS_SAMPLING_SUPPORTED,
            consecutive_failures: 0,
        }
    }

    /// Whether a sample should be taken at all.
    const fn armed(&self) -> bool {
        self.armed
    }

    /// A delivered sample: the failure tolerance resets.
    fn sampled(&mut self) {
        self.consecutive_failures = 0;
    }

    /// One failed sample on a live child. Returns `true` exactly once — when
    /// the tolerance is spent and the arm disarms — so the caller records
    /// the disarm in the stats at that point.
    fn failed(&mut self) -> bool {
        if !self.armed {
            return false;
        }
        self.consecutive_failures += 1;
        if self.consecutive_failures > RSS_SAMPLE_TOLERANCE {
            self.armed = false;
            return true;
        }
        false
    }
}

/// Spawns the per-generation isolation monitor: the RSS budget arm, the
/// analyze demand's clock, and the idle-eviction deadline (see the module
/// docs' conformance table). Returns `Err` when the OS cannot create the
/// thread — the caller routes that through
/// [`PluginRuntime::thread_spawn_failed`], never an abort: an unenforced
/// generation must not exist, and the restart budget decides what does.
fn spawn_monitor(
    h: GenerationHandles,
    pid: u32,
    memory_cap_bytes: u64,
    callback_timeout: Duration,
    idle_evict: Option<Duration>,
    created: Instant,
    child: Arc<Mutex<Option<Child>>>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("dartq-plugin-monitor".to_owned())
        .spawn(move || {
            // The RSS arm disarming (no sampler on this platform, or
            // sampling this child keeps failing) must never take the thread
            // down: the analyze-clock arm below is the other half of the
            // budget enforcement, and a monitor that quit on its first bad
            // sample would silently drop both budgets for the generation.
            // Disarms are counted in the stats instead.
            let mut rss = RssWatch::new();
            if !rss.armed() {
                h.stats.rss_disarm();
            }
            while h.running.load(Ordering::Relaxed) {
                // CPU budget row, analyze path: a fed demand that has produced
                // no diagnostics for longer than the callback budget is the
                // same isolation breach as the memory row. Checked before the
                // RSS sample because unsampled targets report no RSS at all —
                // the budget must hold everywhere. Record the cause so the
                // crash diagnostic names the timeout, flip `alive` so the
                // next interaction restarts instead of writing into the
                // dying pipe, kill, and close the clock (the demand ends
                // with the generation).
                if let Some(open_for) = h.clock.open_for(created)
                    && open_for > callback_timeout
                {
                    // First-writer-wins on the death slot, as in the reader's
                    // EOF: between the clock read above and this lock a
                    // genuine death may already have recorded its cause. This
                    // arm then only finishes the corpse (the kill below is a
                    // no-op reap) and must not re-attribute the death to the
                    // budget.
                    let mut slot = h.death.lock();
                    if slot.is_none() {
                        *slot = Some(format!(
                            "no diagnostics were produced within the {} ms \
                             analyze callback budget",
                            callback_timeout.as_millis()
                        ));
                    }
                    drop(slot);
                    h.alive.store(false, Ordering::Relaxed);
                    if let Some(mut child) = child.lock().take() {
                        kill_and_reap(&mut child);
                    }
                    h.clock.close();
                    break;
                }
                // A dead generation is not a sampling failure: the reader's
                // EOF (the ordinary death path) ends the monitor here, before
                // the sample, so dead-child read failures cannot pollute the
                // disarm counter.
                if !h.alive.load(Ordering::Relaxed) {
                    break;
                }
                if rss.armed() {
                    match sample_rss(pid) {
                        // A zero sample (process gone) never raises the peak.
                        Some(rss_bytes) if rss_bytes > 0 => {
                            rss.sampled();
                            h.stats.observe_rss(rss_bytes);
                            if rss_bytes > memory_cap_bytes {
                                // memory budget enforcement: over-budget child
                                // dies. Record the cause so the crash diagnostic
                                // can name the memory row, and flip `alive` so
                                // the next interaction restarts instead of
                                // writing into the dying pipe; the reader's EOF
                                // turns this into the ordinary crash path.
                                *h.death.lock() = Some(format!(
                                    "RSS sample {rss_bytes} exceeded the {memory_cap_bytes}-byte \
                                     memory budget"
                                ));
                                h.alive.store(false, Ordering::Relaxed);
                                if let Some(mut child) = child.lock().take() {
                                    kill_and_reap(&mut child);
                                }
                                break;
                            }
                        }
                        Some(_) => {}
                        None => {
                            if rss.failed() {
                                // Tolerance spent on a live child: the memory
                                // row is unenforceable for the rest of this
                                // generation. Record it and keep the clock
                                // armed.
                                h.stats.rss_disarm();
                            }
                        }
                    }
                }
                // Idle eviction: a generation idle past the deadline holds
                // its resident set for nothing — kill it here (no new
                // thread, no async: this monitor tick is the deadline
                // check) and let the next interaction relaunch budget-free.
                // The deadline was clamped above every interaction budget at
                // construction, so no request can be in flight when it
                // fires; the re-read closes the one residual window where a
                // just-started interaction's touch has not become visible
                // to this thread yet.
                if let Some(idle_timeout) = idle_evict {
                    let last = h.last_interaction_ms.load(Ordering::Relaxed);
                    if Duration::from_millis(elapsed_millis(created).saturating_sub(last))
                        >= idle_timeout
                        && h.last_interaction_ms.load(Ordering::Relaxed) == last
                    {
                        // Resumable death: `evicted` flips before the kill
                        // so every later observer (ensure_alive, shutdown)
                        // takes the budget-free resume path, and the death
                        // slot stays untouched — an eviction never surfaces
                        // a crash row and never consumes the restart
                        // budget.
                        h.evicted.store(true, Ordering::Relaxed);
                        h.stats.evict();
                        h.alive.store(false, Ordering::Relaxed);
                        if let Some(mut child) = child.lock().take() {
                            kill_and_reap(&mut child);
                        }
                        break;
                    }
                }
                std::thread::sleep(RSS_POLL_INTERVAL);
            }
        })
}

/// The runtime's idle-eviction timeout: the manifest's `idle_evict_ms`
/// (default `DEFAULT_IDLE_EVICT_MS`, 5 minutes; `0` disables) with the
/// `DARTQ_PLUGIN_IDLE_EVICT_MS` environment override taking precedence when
/// it parses as a non-negative integer — the measurement harness exercises
/// eviction at short timeouts without editing plugin manifests. A malformed
/// override falls back to the manifest value (the manifest is the
/// configuration of record; the env var is a measurement knob).
fn idle_evict_timeout(manifest: &Manifest) -> Option<Duration> {
    let ms = std::env::var("DARTQ_PLUGIN_IDLE_EVICT_MS")
        .ok()
        .and_then(|text| text.parse::<u64>().ok())
        .unwrap_or(manifest.idle_evict_ms);
    (ms > 0).then(|| Duration::from_millis(ms))
}

/// The child's resident set in bytes, `None` once the process is gone.
///
/// The two samplers are deliberately **not** symmetric, and the recorded
/// peak says so: Linux reads the kernel's monotone high-water mark, so no
/// allocation spike can escape it; macOS reads an instantaneous value, so a
/// spike fitting between two [`RSS_POLL_INTERVAL`] polls is invisible and
/// `stats.peak_rss_bytes` is a lower bound there. The budget enforcement
/// inherits the same shape — exact on Linux, sampled on macOS.
///
/// - Linux: `/proc/<pid>/status` `VmHWM`, the kernel's exact high-water mark.
/// - macOS: `proc_pid_rusage` `ri_resident_size` (`rusage_info_v0`, ABI
///   pinned against the SDK's `libproc.h`/`sys/resource.h`; flavor
///   `RUSAGE_INFO_V0` == 0).
/// - Other targets: unsampled (the counter stays 0 and the arm is disarmed,
///   counted, in [`PluginStatsSnapshot::rss_disarms`]; documented in
///   [`PluginStatsSnapshot`]).
#[cfg(target_os = "linux")]
fn sample_rss(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let hwm = status.lines().find(|line| line.starts_with("VmHWM:"))?;
    let kb: u64 = hwm["VmHWM:".len()..]
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    Some(kb * 1024)
}

#[cfg(target_os = "macos")]
fn sample_rss(pid: u32) -> Option<u64> {
    /// `struct rusage_info_v0` (`sys/resource.h`): 16 UUID bytes then ten
    /// `uint64_t` fields; `ri_resident_size` is field 8. Field names are the
    /// SDK header's, pinned by the ABI — not a naming choice.
    #[expect(clippy::struct_field_names)] // field names are the SDK header's, pinned by the ABI
    #[repr(C)]
    struct RUsageInfoV0 {
        ri_uuid: [u8; 16],
        ri_user_time: u64,
        ri_system_time: u64,
        ri_pkg_idle_wkups: u64,
        ri_interrupt_wkups: u64,
        ri_pageins: u64,
        ri_wired_size: u64,
        ri_resident_size: u64,
        ri_phys_footprint: u64,
        ri_proc_start_abstime: u64,
        ri_proc_exit_abstime: u64,
    }
    unsafe extern "C" {
        fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut RUsageInfoV0) -> i32;
    }
    let mut info = RUsageInfoV0 {
        ri_uuid: [0; 16],
        ri_user_time: 0,
        ri_system_time: 0,
        ri_pkg_idle_wkups: 0,
        ri_interrupt_wkups: 0,
        ri_pageins: 0,
        ri_wired_size: 0,
        ri_resident_size: 0,
        ri_phys_footprint: 0,
        ri_proc_start_abstime: 0,
        ri_proc_exit_abstime: 0,
    };
    // Safety: `info` is a valid, correctly laid-out `rusage_info_v0` buffer;
    // the call only fills it for our own child's pid.
    let ok = unsafe { proc_pid_rusage(i32::try_from(pid).ok()?, 0, std::ptr::from_mut(&mut info)) };
    (ok == 0).then_some(info.ri_resident_size)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn sample_rss(_pid: u32) -> Option<u64> {
    None
}

// --- wire conversion ---

/// The config frame's `params`, in wire form — also the config-dedup key
/// (map equality is key-independent, so identical configs compare equal
/// whatever the iteration order).
fn config_params(config: &CycleConfig) -> Map<String, Value> {
    plugin_config_frame(config).params
}

/// Maps engine diagnostics onto the wire row shape, resolving file paths
/// through the engine's file table. Diagnostics with unknown `FileId`s are
/// skipped (caller-contract violation; see the module docs).
fn wire_diagnostics(diagnostics: &[Diagnostic], files: &FileTable) -> Vec<PluginDiagnostic> {
    diagnostics
        .iter()
        .filter_map(|d| {
            let path = files.path(d.file)?;
            Some(PluginDiagnostic {
                file: path.to_string_lossy().into_owned(),
                severity: match d.severity {
                    EngineSeverity::Error => Severity::Error,
                    EngineSeverity::Warning => Severity::Warning,
                    EngineSeverity::Info => Severity::Info,
                },
                code: d.code.name.to_owned(),
                message: d.message.clone().into_owned(),
                offset: u64::from(d.range.offset),
                length: u64::from(d.range.length),
            })
        })
        .collect()
}

/// Converts a runtime failure into the `dartq/`-namespace engine diagnostic
/// the output layer merges (: a plugin failure costs one tool
/// diagnostic, never engine state). Deadline overruns map to
/// `dartq/plugin_timeout`, everything else — process death, wire
/// violations, restart-budget exhaustion, spawn failures — to
/// `dartq/plugin_crashed`, with the underlying cause in the message.
///
/// The diagnostic points at `anchor`, a `FileId` the caller chooses (plugin
/// failures are tool-level, not file-level). Plugin *findings* stay
/// wire-form (`PluginDiagnostic`): engine `CodeInfo`s are `&'static`-interned,
/// so namespacing runtime-owned plugin codes is the daemon merge's job, not
/// this conversion's.
#[must_use]
pub fn crash_diagnostic(plugin: &str, error: &RuntimeError, anchor: FileId) -> Diagnostic {
    let (code, reason) = match error {
        RuntimeError::HandshakeTimeout { ms, stderr } => (
            DARTQ_PLUGIN_TIMEOUT,
            format!(
                "handshake did not complete within {ms} ms{}",
                stderr_note(stderr)
            ),
        ),
        RuntimeError::ShutdownTimeout { ms } => (
            DARTQ_PLUGIN_TIMEOUT,
            format!("shutdown did not complete within {ms} ms"),
        ),
        RuntimeError::CallbackTimeout { ms } => (
            DARTQ_PLUGIN_TIMEOUT,
            format!("`request/fixes` was not answered within {ms} ms (callback budget)"),
        ),
        RuntimeError::Crashed { reason } => (DARTQ_PLUGIN_CRASHED, reason.clone()),
        RuntimeError::Spawn { .. } | RuntimeError::Wire(_) => {
            (DARTQ_PLUGIN_CRASHED, error.to_string())
        }
        // Session state-machine failures and pipe errors surface only through
        // the killed-session paths above in practice; as a diagnostic they
        // are still crashes.
        RuntimeError::Protocol(e) => (DARTQ_PLUGIN_CRASHED, e.to_string()),
        RuntimeError::Io(e) => (DARTQ_PLUGIN_CRASHED, e.to_string()),
    };
    Diagnostic::new(
        EngineSeverity::Error,
        code,
        format!("plugin `{plugin}`: {reason}"),
        anchor,
        SourceRange {
            offset: 0,
            length: 0,
        },
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn analyze_clock_opens_once_and_closes() {
        let clock = AnalyzeClock::new();
        assert_eq!(
            clock.open_for(Instant::now()),
            None,
            "closed at birth: nothing was fed"
        );

        // A runtime "created" 500 ms ago; feeding opens the demand against
        // that reference instant. Storage is whole milliseconds
        // (ms-truncated at open and at read), so the first read may
        // legitimately be 0 — this pin tests the storage contract, not ns
        // precision. Every open-state read passes the SAME reference the
        // monitor holds (`created`): a fresh `Instant::now()` would measure
        // elapsed ≈ 0 against the stored open-at and read 0 forever.
        let created = Instant::now()
            .checked_sub(Duration::from_millis(500))
            .expect("clock goes back 500 ms");
        clock.open(created);
        let first = clock.open_for(created).expect("open after the feed");

        // The age advances on the ms grid as the runtime ages.
        std::thread::sleep(Duration::from_millis(2));
        let aged = clock.open_for(created).expect("still open");
        assert!(
            aged >= first + Duration::from_millis(2),
            "the age tracks the runtime clock on the ms grid: {aged:?} after {first:?}"
        );

        // A re-feed while open must not refresh the clock — a plugin that
        // acks every batch without producing would otherwise reset the
        // budget forever and the monitor could never fire. Read across the
        // re-feed: a refresh would drop the age back toward 0.
        clock.open(created);
        let after = clock
            .open_for(created)
            .expect("still open after the re-feed");
        assert!(
            after >= aged,
            "the first open wins: {after:?} must not be younger than {aged:?}"
        );

        clock.close();
        assert_eq!(
            clock.open_for(Instant::now()),
            None,
            "closed when diagnostics arrive or the generation dies"
        );
    }

    #[test]
    fn rss_watch_tolerates_consecutive_failures_then_disarms_once() {
        let mut watch = RssWatch::new();
        if !RSS_SAMPLING_SUPPORTED {
            // This build has no sampler: the arm starts disarmed, records
            // nothing further, and the monitor runs on the clock alone.
            assert!(!watch.armed());
            assert!(!watch.failed(), "a disarmed watch reports nothing");
            return;
        }
        assert!(watch.armed(), "armed at birth on a sampling platform");
        for _ in 0..RSS_SAMPLE_TOLERANCE {
            assert!(
                !watch.failed(),
                "a failure inside the tolerance keeps the arm"
            );
            assert!(watch.armed());
        }
        // One delivered sample resets the count: only *consecutive* failures
        // can spend the tolerance — a flaky sampler never accumulates a disarm.
        watch.sampled();
        for _ in 0..RSS_SAMPLE_TOLERANCE {
            assert!(!watch.failed());
        }
        assert!(watch.failed(), "the tick past the tolerance disarms");
        assert!(!watch.armed(), "the RSS face is off after the disarm");
        assert!(!watch.failed(), "the disarm is reported exactly once");
    }

    #[test]
    fn stderr_capture_keeps_only_the_bounded_tail_and_seals_on_success() {
        let capture = StderrCapture::new();
        // Front bytes past the cap are dropped (and counted), the tail and a
        // truncation marker survive — memory is bounded by the cap.
        capture.push(b"front line\n".as_slice());
        let flood: Vec<u8> = (0..STDERR_CAPTURE_CAP + 100)
            .map(|i| b'0' + u8::try_from(i % 10).expect("digit fits u8"))
            .collect();
        capture.push(&flood);
        let text = capture.take_text().expect("captured text survives");
        assert!(
            text.len() <= STDERR_CAPTURE_CAP + 128,
            "bounded by the cap (plus the marker): {} bytes",
            text.len()
        );
        assert!(text.contains("truncated"), "front loss is marked: {text}");
        assert!(!text.contains("front line"), "front bytes are gone");
        assert!(
            text.ends_with("678901"),
            "the tail is kept verbatim: …{text}"
        );

        // Readiness verdict reached: the capture is discarded and nothing
        // more is retained (the drain thread keeps only emptying the pipe).
        capture.seal();
        assert!(
            capture.take_text().is_none(),
            "success discards the capture"
        );
        capture.push(b"late noise");
        assert!(
            capture.take_text().is_none(),
            "post-verdict writes are not retained"
        );
    }

    #[test]
    fn spawn_spec_argv_follows_the_entrypoint_kind() {
        let manifest_for = |kind: &str, entry: &str| {
            Manifest::parse(&format!(
                r#"{{"name": "n", "version": "1.0.0", "entrypoint": "{entry}", "entrypoint_kind": "{kind}"}}"#
            ))
.expect("parse")
        };
        let dir = Path::new("/plugins/dcl");

        // Source and kernel share the `dart <entrypoint>` argv: a kernel
        // snapshot differs in what the VM does with the file (magic-number
        // sniff, direct load), not in how it is spawned.
        for kind in ["source", "kernel"] {
            let spec = SpawnSpec::dart(dir, manifest_for(kind, "bin/serve"));
            assert_eq!(spec.program, "dart");
            assert_eq!(spec.args, vec!["bin/serve".to_owned()]);
            assert_eq!(spec.current_dir, dir);
        }

        // The exe form drops the dart front end: the precompiled executable
        // is the program (absolutized — resolution must not depend on the
        // child's cwd change ordering), with no args.
        let spec = SpawnSpec::dart(dir, manifest_for("exe", "bin/serve.exe"));
        let program = Path::new(&spec.program);
        assert!(program.is_absolute(), "exe program absolutized: {spec:?}");
        assert!(program.ends_with("bin/serve.exe"), "{spec:?}");
        assert!(spec.args.is_empty());
        assert_eq!(spec.current_dir, dir);
    }

    #[test]
    fn thread_spawn_failure_takes_the_death_path_and_never_aborts() {
        // Test-form rationale: an OS thread-creation failure has no
        // portable, deterministic in-process trigger (`Builder::spawn` fails
        // only on real fd/memory exhaustion, and exhausting host limits to
        // force one is flaky and resource-hostile), so this pin targets the
        // shared failure arm all four spawn sites route through — each
        // site's wiring is the `Err => thread_spawn_failed` mapping checked
        // by the compiler, and the restart-budget ride is the ordinary
        // `launch_with_budget` path the crash integration tests exercise.
        let manifest = Manifest::parse(
            r#"{"name": "n", "version": "1.0.0", "entrypoint": "x", "timeouts": {"request_retries": 0}}"#,
        )
        .expect("parse");
        let mut runtime = PluginRuntime::lazy(SpawnSpec::dart(Path::new("/plugins/dcl"), manifest));
        // Shape of a live generation at the spawn sites (launch_once sets
        // the flag before any worker thread is created).
        runtime.alive.store(true, Ordering::Relaxed);

        // The failure arm records the cause, kills the (here absent) child
        // without panicking, and surfaces the crash form carrying the reason
        // and the spent budget — one tool diagnostic at exhaustion, never a
        // host abort.
        let error = runtime.thread_spawn_failed(
            "reader",
            std::io::Error::new(std::io::ErrorKind::OutOfMemory, "host thread limit"),
        );
        let RuntimeError::Crashed { reason } = &error else {
            panic!("the death path surfaces the crash form, got {error:?}");
        };
        assert!(
            reason.contains("reader thread could not be spawned")
                && reason.contains("host thread limit"),
            "the reason names the thread and the io error: {reason}"
        );
        assert!(
            reason.contains("restart budget of 0 exhausted"),
            "the budget accounting is uniform with every death: {reason}"
        );
        assert!(
            !runtime.alive.load(Ordering::Relaxed),
            "the generation is dead; the relaunch rides the next interaction"
        );

        // First-writer-wins, as on every death-slot recorder: a second
        // failure on the same generation must not clobber the cause.
        let _ = runtime.thread_spawn_failed(
            "writer",
            std::io::Error::new(std::io::ErrorKind::OutOfMemory, "again"),
        );
        let RuntimeError::Crashed { reason } = runtime.crash_error() else {
            panic!("the death slot still surfaces the crash form");
        };
        assert!(
            reason.contains("reader thread") && !reason.contains("writer"),
            "the first recorded cause stands: {reason}"
        );
    }

    #[test]
    fn crash_diagnostic_namespaces_deadline_and_death_failures() {
        let anchor = {
            let mut files = FileTable::new();
            files.intern(Path::new("lib/main.dart"))
        };
        let timeout =
            crash_diagnostic("slow", &RuntimeError::CallbackTimeout { ms: 5_000 }, anchor);
        assert_eq!(timeout.code, DARTQ_PLUGIN_TIMEOUT);
        assert_eq!(timeout.code.name, "dartq/plugin_timeout");
        assert_eq!(timeout.severity, EngineSeverity::Error);
        assert_eq!(timeout.file, anchor);
        assert!(
            timeout.message.contains("slow") && timeout.message.contains("5000"),
            "message names the plugin and the budget: {}",
            timeout.message
        );

        let crashed = crash_diagnostic(
            "dies",
            &RuntimeError::Crashed {
                reason: "plugin process closed the connection".to_owned(),
            },
            anchor,
        );
        assert_eq!(crashed.code, DARTQ_PLUGIN_CRASHED);
        assert_eq!(crashed.code.name, "dartq/plugin_crashed");
        assert!(crashed.message.contains("closed the connection"));
    }

    #[test]
    fn reader_skips_blank_lines_and_decodes_crlf_frames() {
        let init_reply = r#"{"schema_version":1,"id":1,"method":"initialize","params":{"protocol_version":1,"capabilities":["lint"]}}"#;
        let diagnostic = r#"{"schema_version":1,"id":null,"method":"notification/diagnostic","params":{"file":"lib/main.dart","severity":"warning","code":"p/lint","message":"framing","offset":1,"length":2}}"#;
        // One stray blank line between two valid frames, the second
        // CRLF-terminated: framing noise the single codec authority
        // (`FrameDecoder::push`, shared with the Dart host) normalizes — the
        // reader must not tear the session down over it.
        let stream = format!("{init_reply}\n\n{diagnostic}\r\n");

        let session = Mutex::new({
            let mut session = Session::new(CapabilitySet::all());
            session.send_initialize().expect("arm the handshake");
            session
        });
        let pending = PendingOutput::default();
        let (events, rx) = mpsc::channel();
        let stats = PluginStats::default();

        let outcome = run_reader(
            Cursor::new(stream.into_bytes()),
            &session,
            &pending,
            &events,
            &stats,
            &AnalyzeClock::new(),
        );

        // The session was alive at EOF (never Terminated): the exact death
        // reason distinguishes a clean stream end from a framing teardown.
        assert_eq!(
            outcome,
            Err("plugin process closed the connection".to_owned()),
            "a blank line must not surface as a protocol violation"
        );
        assert!(
            matches!(rx.try_recv(), Ok(ProtocolEvent::Initialized(_))),
            "the handshake reply was processed across the blank line"
        );
        let drained = pending.queues.lock();
        assert_eq!(
            drained.diagnostics.len(),
            1,
            "the CRLF-terminated frame decoded"
        );
        assert_eq!(drained.diagnostics[0].code, "p/lint");
    }

    #[test]
    fn reader_queues_reverse_queries_and_signals_the_handle() {
        // The reverse direction over the reader harness: a `request/query`
        // frame routes through the session machine into the pending queue,
        // and the wait loops learn about it through `QueriesQueued`.
        let init_reply = r#"{"schema_version":1,"id":1,"method":"initialize","params":{"protocol_version":1,"capabilities":["query"]}}"#;
        let query = r#"{"schema_version":1,"id":42,"method":"request/query","params":{"kind":"element_at","file":"lib/a.dart","offset":3}}"#;
        let stream = format!("{init_reply}\n{query}\n");

        let session = Mutex::new({
            let mut session = Session::new(CapabilitySet::all());
            session.send_initialize().expect("arm the handshake");
            session
        });
        let pending = PendingOutput::default();
        let (events, rx) = mpsc::channel();
        let stats = PluginStats::default();

        // The stream ends without a shutdown: the ordinary EOF error, after
        // both frames were consumed.
        let outcome = run_reader(
            Cursor::new(stream.into_bytes()),
            &session,
            &pending,
            &events,
            &stats,
            &AnalyzeClock::new(),
        );
        assert_eq!(
            outcome,
            Err("plugin process closed the connection".to_owned())
        );
        assert!(matches!(rx.try_recv(), Ok(ProtocolEvent::Initialized(_))));
        assert!(matches!(rx.try_recv(), Ok(ProtocolEvent::QueriesQueued)));
        let queries = pending.take_queries();
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].id, 42);
        assert_eq!(
            queries[0].request.kind,
            crate::session::QueryKind::ElementAt
        );
        assert_eq!(queries[0].request.file.as_deref(), Some("lib/a.dart"));
    }

    #[test]
    fn pending_output_overflow_tears_the_session_down() {
        // The reader-side fuse: the 10_001st un-drained row (diagnostics and
        // reverse queries share one cap) is a protocol violation — same
        // 10_000 fuse as the parser's `MAX_DIAGNOSTICS`, not unbounded
        // queue growth.
        let init_reply = r#"{"schema_version":1,"id":1,"method":"initialize","params":{"protocol_version":1,"capabilities":["lint"]}}"#;
        let diagnostic = r#"{"schema_version":1,"id":null,"method":"notification/diagnostic","params":{"file":"f","severity":"warning","code":"c","message":"m"}}"#;
        let mut stream = format!("{init_reply}\n");
        for _ in 0..=MAX_PENDING_OUTPUT {
            stream.push_str(diagnostic);
            stream.push('\n');
        }

        let session = Mutex::new({
            let mut session = Session::new(CapabilitySet::all());
            session.send_initialize().expect("arm the handshake");
            session
        });
        let pending = PendingOutput::default();
        let (events, _rx) = mpsc::channel();
        let stats = PluginStats::default();

        let outcome = run_reader(
            Cursor::new(stream.into_bytes()),
            &session,
            &pending,
            &events,
            &stats,
            &AnalyzeClock::new(),
        );

        let reason = outcome.expect_err("overflow must be a protocol violation");
        assert!(
            reason.contains(&MAX_PENDING_OUTPUT.to_string()),
            "the reason names the fuse: {reason}"
        );
        let drained = pending.queues.lock();
        assert_eq!(
            drained.diagnostics.len(),
            MAX_PENDING_OUTPUT,
            "the queue stopped exactly at the cap"
        );
    }

    #[test]
    fn reverse_queries_share_the_output_fuse_with_diagnostics() {
        let pending = PendingOutput::default();
        let request = QueryRequest {
            kind: crate::session::QueryKind::Symbol,
            name: Some("x".to_owned()),
            file: None,
            offset: None,
        };
        // One diagnostic rides under the cap together with the queries: the
        // drain must never mix the halves.
        pending.push(row("context")).expect("under the cap");
        for i in 0..MAX_PENDING_OUTPUT - 1 {
            pending
                .push_query(PendingQuery {
                    id: u64::try_from(i).expect("fits"),
                    request: request.clone(),
                })
                .expect("under the cap");
        }
        let err = pending
            .push(row("over"))
            .expect_err("the combined fuse fires across both queues");
        assert!(err.contains(&MAX_PENDING_OUTPUT.to_string()));

        // take_queries takes only the query half; diagnostics survive it.
        let queries = pending.take_queries();
        assert_eq!(queries.len(), MAX_PENDING_OUTPUT - 1);
        assert_eq!(queries[0].id, 0);
        assert_eq!(pending.take_all().len(), 1, "the diagnostic survived");
        assert!(pending.take_queries().is_empty(), "queries were taken");
    }

    #[test]
    fn wait_output_wakes_on_the_reader_signal_not_the_deadline() {
        let pending = Arc::new(PendingOutput::default());
        let reader = {
            let pending = Arc::clone(&pending);
            std::thread::spawn(move || {
                // A "reader" pushes one diagnostic after a short delay. A
                // missing signal would burn the whole 5 s deadline and blow
                // the elapsed bound below.
                std::thread::sleep(Duration::from_millis(50));
                pending
                    .push(PluginDiagnostic {
                        file: "lib/main.dart".to_owned(),
                        severity: Severity::Warning,
                        code: "p/lint".to_owned(),
                        message: "signaled".to_owned(),
                        offset: 0,
                        length: 0,
                    })
                    .expect("under the cap");
            })
        };
        let start = Instant::now();
        let (batch, pending_queries) = pending
            .wait_for_output(start + Duration::from_secs(5))
            .expect("the push woke the wait");
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "woke on the push signal, long before the deadline"
        );
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].code, "p/lint");
        assert_eq!(pending_queries, 0);
        reader.join().expect("reader joins");
    }

    #[test]
    fn wait_output_returns_queued_rows_at_the_deadline() {
        let pending = PendingOutput::default();
        pending.push(row("racing")).expect("under the cap");
        let start = Instant::now();
        // A non-empty queue skips the first-row wait and only settles the
        // burst; the settle window (5 ms) bounds this far below the old
        // full-deadline shape.
        let (batch, pending_queries) = pending
            .wait_for_output(start + Duration::from_millis(50))
            .expect("the queued row skips the park");
        assert!(start.elapsed() < Duration::from_millis(50));
        assert_eq!(batch.len(), 1);
        assert_eq!(pending_queries, 0);

        // An empty queue rides to the deadline and reports the expiry —
        // `wait_output`'s final alive gate turns `None` into the restart.
        let empty = PendingOutput::default();
        let start = Instant::now();
        assert!(
            empty
                .wait_for_output(start + Duration::from_millis(50))
                .is_none(),
            "the deadline bounds the wait"
        );
        assert!(start.elapsed() >= Duration::from_millis(40));
    }

    #[test]
    fn wait_output_drains_the_whole_reply_not_its_first_line() {
        let pending = Arc::new(PendingOutput::default());
        let reader = {
            let pending = Arc::clone(&pending);
            std::thread::spawn(move || {
                pending.push(row("first")).expect("under the cap");
                // Well inside the settle window: part of the same reply
                // burst, exactly like the contiguous frames a live plugin
                // writes back after serving one analyze cycle.
                std::thread::sleep(BURST_SETTLE / 2);
                pending.push(row("second")).expect("under the cap");
            })
        };
        let (batch, pending_queries) = pending
            .wait_for_output(Instant::now() + Duration::from_secs(5))
            .expect("the burst arrived");
        assert_eq!(batch.len(), 2, "both rows of the burst, one drain");
        assert_eq!(batch[0].message, "first");
        assert_eq!(batch[1].message, "second");
        assert_eq!(pending_queries, 0);
        reader.join().expect("reader joins");
    }

    #[test]
    fn a_reverse_query_wakes_the_wait_without_the_burst_settle() {
        // A plugin stalled awaiting its query answer must not wait out the
        // diagnostics settle window: the query wakes the drain immediately.
        let pending = Arc::new(PendingOutput::default());
        let reader = {
            let pending = Arc::clone(&pending);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(30));
                pending
                    .push_query(PendingQuery {
                        id: 7,
                        request: QueryRequest {
                            kind: crate::session::QueryKind::ElementAt,
                            name: None,
                            file: Some("lib/a.dart".to_owned()),
                            offset: Some(3),
                        },
                    })
                    .expect("under the cap");
            })
        };
        let start = Instant::now();
        let (batch, pending_queries) = pending
            .wait_for_output(start + Duration::from_secs(5))
            .expect("the query woke the wait");
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "the query woke the wait immediately"
        );
        assert_eq!(batch.len(), 0);
        // The query was only peeked: it stays queued for `serve_queries`,
        // the one consumer that can answer it.
        assert_eq!(pending_queries, 1);
        let queued = pending.take_queries();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].id, 7);
        assert_eq!(queued[0].request.kind, crate::session::QueryKind::ElementAt);
        reader.join().expect("reader joins");
    }

    #[test]
    fn a_row_after_the_settle_window_lands_in_the_next_drain() {
        let pending = Arc::new(PendingOutput::default());
        let reader = {
            let pending = Arc::clone(&pending);
            std::thread::spawn(move || {
                pending.push(row("first")).expect("under the cap");
                // Far beyond the settle window: a separate reply the drain
                // must not wait for.
                std::thread::sleep(BURST_SETTLE * 4);
                pending.push(row("late")).expect("under the cap");
            })
        };
        let start = Instant::now();
        let (batch, _) = pending
            .wait_for_output(start + Duration::from_secs(5))
            .expect("the first row arrived");
        assert_eq!(batch.len(), 1, "the window closed on a quiet pipe");
        assert_eq!(batch[0].message, "first");

        let (late, _) = pending
            .wait_for_output(start + Duration::from_secs(5))
            .expect("the late row arrived");
        assert_eq!(late.len(), 1);
        assert_eq!(late[0].message, "late");
        reader.join().expect("reader joins");
    }

    /// One queued diagnostic for the burst tests.
    fn row(message: &str) -> PluginDiagnostic {
        PluginDiagnostic {
            file: "lib/main.dart".to_owned(),
            severity: Severity::Warning,
            code: "p/lint".to_owned(),
            message: message.to_owned(),
            offset: 0,
            length: 0,
        }
    }

    #[test]
    fn wire_conversion_maps_fields_and_skips_unknown_files() {
        let mut files = FileTable::new();
        let source = std::env::temp_dir().join("dartq_plugin_wire_test.dart");
        let known = files.intern(&source);
        // The table normalizes paths (existing ancestors canonicalize — the
        // macOS `/tmp` symlink included), so the wire carries the table's
        // stored spelling, not the interned spelling.
        let normalized = files.path(known).expect("interned").to_path_buf();
        // A forgotten id is a permanent hole: exactly the "unknown FileId"
        // the conversion must skip.
        files.intern(Path::new("removed.dart"));
        let hole = files
            .forget(Path::new("removed.dart"))
            .expect("forget returns the id it severed");
        let diagnostics = vec![
            Diagnostic::new(
                EngineSeverity::Warning,
                dartq_diagnostics::URI_DOES_NOT_EXIST,
                "Target of URI doesn't exist: 'a.dart'.",
                known,
                dartq_diagnostics::SourceRange {
                    offset: 12,
                    length: 8,
                },
                None,
            ),
            Diagnostic::new(
                EngineSeverity::Error,
                dartq_diagnostics::URI_DOES_NOT_EXIST,
                "unknown file",
                hole,
                dartq_diagnostics::SourceRange {
                    offset: 0,
                    length: 0,
                },
                None,
            ),
        ];
        let wire = wire_diagnostics(&diagnostics, &files);
        assert_eq!(wire.len(), 1, "unknown FileId is skipped");
        assert_eq!(wire[0].file, normalized.to_string_lossy());
        assert_eq!(wire[0].severity, Severity::Warning);
        assert_eq!(wire[0].code, "uri_does_not_exist");
        assert_eq!(wire[0].offset, 12);
        assert_eq!(wire[0].length, 8);
    }
}
