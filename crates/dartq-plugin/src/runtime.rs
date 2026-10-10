//! The core-side plugin runtime: owns one Dart host process and drives the
//! wire contract against it (PRD §14.2), with the §14.3 isolation metrics
//! wired at every enforcement point.
//!
//! # Face
//!
//! [`PluginRuntime::spawn`] launches the host, performs the `initialize`
//! handshake (within the manifest's request budget), and hands back a handle.
//! From there the engine wiring is two calls per analyze cycle:
//!
//! - [`PluginRuntime::feed`] pushes the engine's merged diagnostics to the
//!   plugin as one `notification/engine_diagnostics` batch (its diagnostic
//!   context); [`FileTable`] resolves [`FileId`]s to the wire's path strings.
//! - [`PluginRuntime::take_diagnostics`] drains the diagnostics the plugin
//!   emitted back (`notification/diagnostic`) for the caller to merge into
//!   its output. Plugin findings keep their wire shape here: engine
//!   [`CodeInfo`]s are `&'static`-interned, so namespacing plugin codes into
//!   the engine's diagnostic set is the daemon/CLI output layer's job
//!   (PRD §14 fix sources), not the transport's.
//! - [`PluginRuntime::request_fixes`] is the fix face: it sends
//!   `request/fixes` and awaits the reply under the manifest's callback
//!   budget (§14.3 CPU row, default 5 s). An overrun kills the child and
//!   consumes the restart budget, surfacing
//!   [`RuntimeError::CallbackTimeout`]. The analyze push has no reply to
//!   await, so the monitor enforces the same budget on the demand itself:
//!   the clock [`PluginRuntime::feed`] opens must close with produced
//!   diagnostics before `callback_ms` elapses.
//!
//! [`PluginRuntime::stats`] is the §14.3 observability face (peak RSS, frame
//! and byte counters, guard drops, restart count) the daemon/CLI read.
//! [`crash_diagnostic`] converts a terminal [`RuntimeError`] into the
//! `dartq/`-namespace [`Diagnostic`] the output layer merges (PRD §14.3:
//! a plugin failure is one tool diagnostic, never an engine failure).
//!
//! # Discovery
//!
//! [`crate::discovery`] turns an on-disk workspace into spawn-ready
//! manifests (the `<workspace>/plugins/*/dartq_plugin.json` convention plus
//! explicit override paths); each hit launches through
//! [`SpawnSpec::dart`].
//!
//! # PRD §14.3 conformance
//!
//! | Dimension | Mechanism here |
//! | --- | --- |
//! | Memory | RSS monitor thread (100 ms tick) samples the child; Linux reads the kernel's `VmHWM` high-water mark, macOS `proc_pid_rusage` (`ri_resident_size`, SDK-header-pinned ABI); exceeding the manifest's `memory_budget_mb` kills the child. |
//! | CPU | `request/fixes` awaits its reply under the manifest's `callback_ms`; an overrun kills the child and consumes the restart budget. The analyze demand is enforced against the same budget: feeding engine diagnostics opens a clock in the monitor, closed when the child produces diagnostics (or dies); a demand still open past `callback_ms` kills the child through the same path. |
//! | IPC | `initialize` and `shutdown` run under the manifest's `request_ms`; every stdin write is bounded by the same deadline (writer thread). |
//! | Crash | Child death never propagates: the runtime relaunches within the manifest's retry budget (counted in `stats.restarts`), then reports [`RuntimeError::Crashed`] — the terminal form names the exhausted budget. |
//! | Frame guard | Lines beyond [`MAX_FRAME_SIZE`] are dropped before buffering (counted) and tear the session down — the transport never resynchronizes. |
//!
//! # Threading (std threads only, per the workspace runtime policy)
//!
//! Per generation (one child process): a reader thread owns the child's
//! stdout, the [`Session`] state machine (behind a mutex shared with the
//! handle), and the pending-diagnostics buffer; a monitor thread samples RSS
//! and enforces the memory budget; a writer thread owns the child's stdin
//! and is the only process writing to it — the handle (`&mut self`) feeds it
//! frames and awaits per-write acks, which bounds every write without ever
//! blocking the handle on a full pipe. All shared counters are atomics; the
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

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dartq_diagnostics::{
    DARTQ_PLUGIN_CRASHED, DARTQ_PLUGIN_TIMEOUT, Diagnostic, Severity as EngineSeverity, SourceRange,
};
use dartq_source::{FileId, FileTable};
use thiserror::Error;

use crate::capability::CapabilitySet;
use crate::frame::{Frame, FrameDecoder, MAX_FRAME_SIZE, ProtocolError, encode_frame};
use crate::manifest::Manifest;
use crate::session::{
    FixesReply, Incoming, PluginDiagnostic, Session, SessionError, SessionState, Severity,
    engine_diagnostics_frame,
};
use crate::stats::{PluginStats, PluginStatsSnapshot};

/// How often the isolation monitor samples the child's RSS.
const RSS_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Pending-diagnostics fuse: a plugin that queues more than this many
/// un-drained diagnostics is torn down as a protocol violation instead of
/// growing the buffer without bound — the same `10_000` fuse the parser's
/// `MAX_DIAGNOSTICS` and the scanner's per-file cap enforce.
const MAX_PENDING_DIAGNOSTICS: usize = 10_000;

/// Reply-burst settle window of the drain: after the first diagnostic of a
/// reply arrives, [`PendingDiagnostics::wait_for_burst`] keeps absorbing
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
    /// entrypoint script ([`SpawnSpec::dart`] builds it).
    pub args: Vec<String>,
    /// Working directory of the plugin (its package directory).
    pub current_dir: PathBuf,
    /// The parsed manifest: capabilities and §14.3 budgets.
    pub manifest: Manifest,
    /// Where the child's stderr goes; `Null` keeps a chatty plugin from
    /// filling an unread pipe.
    pub stderr: StdioPolicy,
}

/// The stderr routing of the plugin process.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StdioPolicy {
    /// Discard (default; an unread pipe could block the plugin).
    #[default]
    Null,
    /// Pass through to dartq's own stderr (tests, debugging).
    Inherit,
}

impl StdioPolicy {
    fn stdio(self) -> Stdio {
        match self {
            // `stderr()` is the process handle; `Stdio::inherit()` needs the
            // raw fd — both route to the same stream.
            Self::Null => Stdio::null(),
            Self::Inherit => Stdio::inherit(),
        }
    }
}

impl SpawnSpec {
    /// The manifest contract's default launch: run the manifest's entrypoint
    /// script with `dart` inside the plugin directory.
    #[must_use]
    pub fn dart(plugin_dir: &Path, manifest: Manifest) -> Self {
        Self {
            program: "dart".to_owned(),
            args: vec![manifest.entrypoint.clone()],
            current_dir: plugin_dir.to_path_buf(),
            manifest,
            stderr: StdioPolicy::Null,
        }
    }
}

/// Runtime failures. Any variant other than a successful shutdown leaves the
/// runtime dead or restarting — the engine never loses its own state to a
/// plugin failure (PRD §14.3).
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
    #[error("plugin handshake timed out after {ms} ms")]
    HandshakeTimeout {
        /// The enforced budget in milliseconds.
        ms: u64,
    },
    /// `shutdown` was answered neither in time nor at all.
    #[error("plugin shutdown timed out after {ms} ms")]
    ShutdownTimeout {
        /// The enforced budget in milliseconds.
        ms: u64,
    },
    /// The plugin overran the manifest's `callback_ms` budget on
    /// `request/fixes` (§14.3 CPU row): the child was killed and one restart
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

/// The per-generation diagnostic queue: the reader fills it, the handle
/// drains it, and every push signals [`Self::wait_for_burst`] so the handle
/// parks instead of polling.
#[derive(Debug, Default)]
struct PendingDiagnostics {
    queue: Mutex<Vec<PluginDiagnostic>>,
    signal: Condvar,
}

impl PendingDiagnostics {
    /// Queues one plugin diagnostic and wakes a parked waiter. Past
    /// [`MAX_PENDING_DIAGNOSTICS`] the queue is a protocol violation — a
    /// plugin flooding diagnostics is torn down like any other contract
    /// break. `Err` carries the teardown reason.
    fn push(&self, diagnostic: PluginDiagnostic) -> Result<(), String> {
        let mut queue = self.queue.lock().expect("pending diagnostics lock");
        if queue.len() >= MAX_PENDING_DIAGNOSTICS {
            return Err(format!(
                "plugin exceeded the {MAX_PENDING_DIAGNOSTICS}-diagnostic pending cap"
            ));
        }
        queue.push(diagnostic);
        self.signal.notify_all();
        Ok(())
    }

    /// Takes every queued diagnostic (the handle-side drain).
    fn take_all(&self) -> Vec<PluginDiagnostic> {
        std::mem::take(&mut self.queue.lock().expect("pending diagnostics lock"))
    }

    /// Parks until diagnostics arrived and the reply burst settled, or
    /// `deadline` passes (`None`). A non-empty queue on entry skips the
    /// first-row wait and only settles the burst. The deadline bounds the
    /// whole wait exactly — the settle window never extends past it — and a
    /// push wakes the waiter immediately; there is no poll tick.
    fn wait_for_burst(&self, deadline: Instant) -> Option<Vec<PluginDiagnostic>> {
        let mut queue = self.queue.lock().expect("pending diagnostics lock");
        while queue.is_empty() {
            let remaining = deadline.checked_duration_since(Instant::now())?;
            let (woken, _) = self
                .signal
                .wait_timeout(queue, remaining)
                .expect("pending diagnostics lock");
            queue = woken;
        }
        // First row is in. The plugin answers one analyze cycle with
        // contiguous notification frames; hold through a quiet window so
        // the drain hands over the whole burst, never its first line.
        loop {
            let now = Instant::now();
            let settle_end = deadline.min(now + BURST_SETTLE);
            if now >= settle_end {
                break;
            }
            let seen = queue.len();
            let (woken, timed_out) = self
                .signal
                .wait_timeout(queue, settle_end - now)
                .expect("pending diagnostics lock");
            queue = woken;
            if queue.len() > seen {
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
        Some(std::mem::take(&mut queue))
    }
}

/// The analyze demand's wall clock (§14.3 CPU row, analyze path): closed at
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

    /// Closes the demand: diagnostics arrived (the reader) or the generation
    /// died (the kill paths and the monitor's own kill).
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

/// One live plugin: child process, session, isolation monitor, counters.
///
/// Created by [`PluginRuntime::spawn`]; `&mut self` methods make the handle
/// thread the only writer, so the reader thread and monitor never contend
/// with a write.
#[derive(Debug)]
pub struct PluginRuntime {
    spec: SpawnSpec,
    request_timeout: Duration,
    callback_timeout: Duration,
    restart_budget: u32,
    memory_cap_bytes: u64,

    // Per generation (one child process); replaced on every launch.
    session: Arc<Mutex<Session>>,
    commands: Option<Sender<WriterCommand>>,
    acks: mpsc::Receiver<std::io::Result<()>>,
    child: Arc<Mutex<Option<Child>>>,
    events: mpsc::Receiver<ProtocolEvent>,
    pending: Arc<PendingDiagnostics>,
    analyze_clock: Arc<AnalyzeClock>,
    running: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    death: Arc<Mutex<Option<String>>>,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
    monitor: Option<JoinHandle<()>>,
    negotiated: Option<CapabilitySet>,

    // Across generations.
    created: Instant,
    stats: Arc<PluginStats>,
    restarts_used: u32,
}

impl PluginRuntime {
    /// Launches the plugin process and completes the handshake.
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
        let request_timeout = Duration::from_millis(spec.manifest.timeouts.request_ms);
        let callback_timeout = Duration::from_millis(spec.manifest.timeouts.callback_ms);
        let restart_budget = spec.manifest.timeouts.request_retries;
        let memory_cap_bytes = u64::from(spec.manifest.memory_budget_mb) * 1024 * 1024;
        let (events_tx, events) = mpsc::channel();
        drop(events_tx); // placeholder replaced by the first launch
        let mut runtime = Self {
            spec,
            request_timeout,
            callback_timeout,
            restart_budget,
            memory_cap_bytes,
            session: Arc::new(Mutex::new(Session::new(CapabilitySet::all()))),
            commands: None,
            acks: mpsc::channel().1,
            child: Arc::new(Mutex::new(None)),
            events,
            pending: Arc::new(PendingDiagnostics::default()),
            analyze_clock: Arc::new(AnalyzeClock::new()),
            running: Arc::new(AtomicBool::new(false)),
            alive: Arc::new(AtomicBool::new(false)),
            death: Arc::new(Mutex::new(None)),
            reader: None,
            writer: None,
            monitor: None,
            negotiated: None,
            stats: Arc::new(PluginStats::default()),
            created: Instant::now(),
            restarts_used: 0,
        };
        runtime.launch_with_budget()?;
        Ok(runtime)
    }

    /// The capability set negotiated at the last completed handshake.
    #[must_use]
    pub const fn negotiated(&self) -> Option<CapabilitySet> {
        self.negotiated
    }

    /// Pushes the engine's current diagnostics to the plugin as one
    /// `notification/engine_diagnostics` batch — its diagnostic context for
    /// the cycle (PRD §14.2). An empty slice sends nothing.
    ///
    /// A child that died since the last call is restarted here (within
    /// budget) before the push. The push opens the §14.3 analyze clock: the
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
    ) -> Result<(), RuntimeError> {
        if diagnostics.is_empty() {
            return Ok(());
        }
        self.ensure_alive()?;
        let frame = engine_diagnostics_frame(&wire_diagnostics(diagnostics, files));
        self.write_frame(&frame)?;
        // §14.3 CPU row, analyze path: the demand's clock opens with the
        // batch on the wire and closes when the child produces diagnostics
        // (reader) or the generation dies — the monitor kills a demand still
        // open past the callback budget.
        self.analyze_clock.open(self.created);
        Ok(())
    }

    /// Drains the diagnostics the plugin emitted since the last call
    /// (`notification/diagnostic`), in arrival order. The caller merges them
    /// into its output (namespacing the codes is the output layer's job).
    ///
    /// # Errors
    ///
    /// Restart/transport failures, per [`RuntimeError`].
    ///
    /// # Panics
    ///
    /// Panics when the internal pending-diagnostics mutex is poisoned — a
    /// thread panicked while holding it, a program bug, not a plugin failure.
    pub fn take_diagnostics(&mut self) -> Result<Vec<PluginDiagnostic>, RuntimeError> {
        self.ensure_alive()?;
        Ok(self.pending.take_all())
    }

    /// [`Self::take_diagnostics`] with a wait: parks on the reader's push
    /// signal until diagnostics arrived, then holds through one
    /// [`BURST_SETTLE`] quiet window so a synchronous reply — one analyze
    /// cycle answered with contiguous `notification/diagnostic` frames —
    /// drains as the whole burst, never as its first line. The caller's
    /// deadline bounds the whole wait exactly (the daemon's per-cycle drain
    /// budget stays exact); there is no poll tick. A child death during the
    /// wait restarts it, within budget: the parked phase may run to the
    /// deadline on a dead child, and the final alive gate performs the
    /// restart before the empty hand returns.
    ///
    /// # Errors
    ///
    /// Restart budget exhaustion and transport failures, per [`RuntimeError`].
    pub fn wait_diagnostics(
        &mut self,
        timeout: Duration,
    ) -> Result<Vec<PluginDiagnostic>, RuntimeError> {
        // Eager restart of a child that died since the last interaction, so
        // the wait below runs against a live generation (the crash-path
        // posture: restart at the caller's interaction, never lazily).
        self.ensure_alive()?;
        let deadline = Instant::now() + timeout;
        match self.pending.wait_for_burst(deadline) {
            Some(batch) => Ok(batch),
            // Deadline: one final alive-gated drain — a push racing the
            // expiry is still honored, and a death during the wait gets its
            // restart (or its error) here rather than at the next
            // interaction.
            None => self.take_diagnostics(),
        }
    }

    /// Sends `request/fixes` for `file` at the given UTF-16 range and awaits
    /// the reply under the manifest's `callback_ms` budget (§14.3 CPU row).
    /// Requires the `fix` capability to have been negotiated.
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
    ///
    /// # Panics
    ///
    /// Panics when an internal mutex is poisoned — a thread panicked while
    /// holding it, a program bug, not a plugin failure.
    pub fn request_fixes(
        &mut self,
        file: &str,
        offset: u64,
        length: u64,
    ) -> Result<FixesReply, RuntimeError> {
        self.ensure_alive()?;
        let frame = self
            .session
            .lock()
            .expect("session lock")
            .send_fixes_request(file, offset, length)?;
        let request_id = frame.id.expect("fixes requests carry an id");
        self.write_frame(&frame)?;

        let callback_ms = self.callback_timeout.as_millis();
        let deadline = Instant::now() + self.callback_timeout;
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
                Ok(reply)
            }
            // The death slot is authoritative (the reader records the cause
            // before sending the event; killers record before killing), so
            // build the error from it — which also adds the uniform
            // restart-budget suffix at the accounting point.
            Ok(ProtocolEvent::Dead) => Err(self.crash_error()),
            Ok(ProtocolEvent::Initialized(_)) => Err(RuntimeError::Crashed {
                reason: "plugin re-handshaked mid-session".to_owned(),
            }),
            Ok(ProtocolEvent::ShutdownComplete) => Err(RuntimeError::Crashed {
                reason: "plugin answered shutdown before request/fixes".to_owned(),
            }),
            Err(RecvTimeoutError::Timeout) => {
                // §14.3 CPU row: a callback that overruns its budget is a
                // misbehaving child — kill it and consume one restart so the
                // consumption is observable at this error site.
                let reason = format!(
                    "`request/fixes` was not answered within the {callback_ms} ms callback budget"
                );
                *self.death.lock().expect("death lock") = Some(reason);
                self.kill_child();
                self.ensure_alive()?;
                Err(RuntimeError::CallbackTimeout {
                    ms: u64::try_from(callback_ms).unwrap_or(u64::MAX),
                })
            }
            Err(RecvTimeoutError::Disconnected) => Err(self.crash_error()),
        }
    }

    /// The isolation metrics snapshot (PRD §14.3).
    #[must_use]
    pub fn stats(&self) -> PluginStatsSnapshot {
        self.stats.snapshot()
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
    /// # Errors
    ///
    /// [`RuntimeError::ShutdownTimeout`] (child killed), [`RuntimeError::Crashed`]
    /// (dead or protocol-violating plugin), session/transport variants.
    ///
    /// # Panics
    ///
    /// Panics when an internal mutex is poisoned — a thread panicked while
    /// holding it, a program bug, not a plugin failure.
    pub fn shutdown(mut self) -> Result<(), RuntimeError> {
        if !self.alive.load(Ordering::Relaxed) {
            self.reap_child();
            return Err(self.crash_error());
        }
        let frame = self.session.lock().expect("session lock").send_shutdown()?;
        self.write_frame(&frame)?;

        let outcome = match self.events.recv_timeout(
            self.request_deadline()
                .saturating_duration_since(Instant::now()),
        ) {
            Ok(ProtocolEvent::ShutdownComplete) => Ok(()),
            // Authoritative death slot, as in `request_fixes` (see there).
            Ok(ProtocolEvent::Dead) => Err(self.crash_error()),
            Ok(ProtocolEvent::Initialized(_)) => Err(RuntimeError::Crashed {
                reason: "plugin re-handshaked mid-session".to_owned(),
            }),
            // Unreachable through the session machine (shutdown is sent only
            // from Ready with the fixes reply already consumed); a stray
            // reply is a contract break like any other.
            Ok(ProtocolEvent::FixesReply { .. }) => Err(RuntimeError::Crashed {
                reason: "unsolicited request/fixes reply".to_owned(),
            }),
            Err(RecvTimeoutError::Timeout) => {
                self.kill_child();
                Err(RuntimeError::ShutdownTimeout {
                    ms: u64::try_from(self.request_timeout.as_millis()).unwrap_or(u64::MAX),
                })
            }
            Err(RecvTimeoutError::Disconnected) => Err(self.crash_error()),
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

    fn ensure_alive(&mut self) -> Result<(), RuntimeError> {
        if self.alive.load(Ordering::Relaxed) {
            return Ok(());
        }
        if self.restarts_used >= self.restart_budget {
            return Err(self.crash_error());
        }
        self.teardown_generation();
        self.stats.restart();
        self.restarts_used += 1;
        self.launch_with_budget()
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
        let mut child = command.spawn().map_err(|source| RuntimeError::Spawn {
            program: self.spec.program.clone(),
            source,
        })?;
        let pid = child.id();
        // Both pipes were just piped: the invariants hold by construction.
        let stdin = child.stdin.take().expect("stdin piped");
        let stdout = child.stdout.take().expect("stdout piped");

        // Fresh per-generation state (the previous generation's Arcs stay
        // with its threads, which wind down on their own flags).
        self.session = Arc::new(Mutex::new(Session::new(CapabilitySet::all())));
        let (events_tx, events_rx) = mpsc::channel();
        self.events = events_rx;
        self.pending = Arc::new(PendingDiagnostics::default());
        self.analyze_clock = Arc::new(AnalyzeClock::new());
        self.running = Arc::new(AtomicBool::new(true));
        self.alive = Arc::new(AtomicBool::new(true));
        self.death = Arc::new(Mutex::new(None));
        self.negotiated = None;
        *self.child.lock().expect("child lock") = Some(child);

        let (commands, commands_rx) = mpsc::channel();
        let (acks, acks_rx) = mpsc::channel();
        self.commands = Some(commands);
        self.acks = acks_rx;
        self.reader = Some(spawn_reader(
            stdout,
            Arc::clone(&self.session),
            Arc::clone(&self.pending),
            events_tx,
            Arc::clone(&self.analyze_clock),
            Arc::clone(&self.alive),
            Arc::clone(&self.death),
            Arc::clone(&self.stats),
        ));
        self.writer = Some(spawn_writer(stdin, commands_rx, acks));
        self.monitor = Some(spawn_monitor(
            pid,
            self.memory_cap_bytes,
            self.callback_timeout,
            self.created,
            Arc::clone(&self.analyze_clock),
            Arc::clone(&self.child),
            Arc::clone(&self.running),
            Arc::clone(&self.alive),
            Arc::clone(&self.death),
            Arc::clone(&self.stats),
        ));

        // Handshake: one `initialize`, reply awaited under the request budget.
        let init = self
            .session
            .lock()
            .expect("session lock")
            .send_initialize()?;
        self.write_frame(&init)?;
        match self.events.recv_timeout(
            self.request_deadline()
                .saturating_duration_since(Instant::now()),
        ) {
            Ok(ProtocolEvent::Initialized(caps)) => {
                self.negotiated = Some(caps);
                Ok(())
            }
            // Authoritative death slot, as in `request_fixes` (see there).
            Ok(ProtocolEvent::Dead) => Err(self.crash_error()),
            Ok(ProtocolEvent::ShutdownComplete) => Err(RuntimeError::Crashed {
                reason: "plugin answered shutdown before initialize".to_owned(),
            }),
            // Unreachable through the session machine (no fixes request is
            // outstanding during the handshake); a stray reply is a contract
            // break like any other.
            Ok(ProtocolEvent::FixesReply { .. }) => Err(RuntimeError::Crashed {
                reason: "unsolicited request/fixes reply".to_owned(),
            }),
            Err(RecvTimeoutError::Timeout) => {
                self.kill_child();
                Err(RuntimeError::HandshakeTimeout {
                    ms: u64::try_from(self.request_timeout.as_millis()).unwrap_or(u64::MAX),
                })
            }
            Err(RecvTimeoutError::Disconnected) => Err(self.crash_error()),
        }
    }

    /// Stops the monitor, reaps the child, joins the reader and writer — for
    /// the generation being replaced. Must run before a new launch overwrites
    /// the per-generation fields. The writer's command channel is dropped
    /// first so a parked writer exits even when the kill cannot unblock it.
    fn teardown_generation(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        self.commands = None;
        self.kill_child();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        // Monitors exit within one poll tick; they hold only their own
        // generation's Arcs and are detached here.
        self.monitor = None;
    }

    fn join_generation(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        self.commands = None;
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

    /// Kills the child and reaps it (kill on an exited child is a no-op
    /// error; the `wait` is what prevents zombies). Also flips `alive`: the
    /// killer knows the death deterministically, the reader's EOF only later
    /// — and a write into the dead child's pipe must never be attempted.
    fn kill_child(&mut self) {
        // The demand ends with the generation: a killed child can no longer
        // produce diagnostics, and a stale-open clock must not outlive it.
        self.analyze_clock.close();
        self.alive.store(false, Ordering::Relaxed);
        let child = self.child.lock().expect("child lock").take();
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn reap_child(&mut self) {
        let child = self.child.lock().expect("child lock").take();
        if let Some(mut child) = child {
            let _ = child.wait();
        }
    }

    fn crash_error(&self) -> RuntimeError {
        let death = self.death.lock().expect("death lock").clone();
        let mut reason = death.unwrap_or_else(|| "plugin process is no longer alive".to_owned());
        if self.restarts_used >= self.restart_budget {
            reason = format!(
                "{reason}; restart budget of {} exhausted",
                self.restart_budget
            );
        }
        RuntimeError::Crashed { reason }
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
                *self.death.lock().expect("death lock") = Some(reason);
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

// --- threads ---

#[allow(clippy::needless_pass_by_value)]
fn spawn_reader(
    stdout: ChildStdout,
    session: Arc<Mutex<Session>>,
    pending: Arc<PendingDiagnostics>,
    events: Sender<ProtocolEvent>,
    clock: Arc<AnalyzeClock>,
    alive: Arc<AtomicBool>,
    death: Arc<Mutex<Option<String>>>,
    stats: Arc<PluginStats>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("dartq-plugin-reader".to_owned())
        .spawn(move || {
            let outcome = run_reader(
                BufReader::new(stdout),
                &session,
                &pending,
                &events,
                &stats,
                &clock,
            );
            alive.store(false, Ordering::Relaxed);
            if let Err(reason) = outcome {
                // First-writer-wins on the death slot: every deterministic
                // killer (monitor budget arms, request/write deadlines)
                // records the cause BEFORE flipping `alive`/killing, so this
                // EOF is only the downstream observation of that kill and
                // must not clobber the authoritative record. A genuine
                // plugin-initiated close still lands — no prior writer.
                let mut slot = death.lock().expect("death lock");
                if slot.is_none() {
                    *slot = Some(reason);
                }
                drop(slot);
                let _ = events.send(ProtocolEvent::Dead);
            }
        })
        .expect("spawn reader thread")
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
    pending: &PendingDiagnostics,
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
            let terminated =
                session.lock().expect("session lock").state() == SessionState::Terminated;
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
            let incoming = session.lock().expect("session lock").handle(frame);
            match incoming {
                Ok(Incoming::Initialized(caps)) => {
                    let _ = events.send(ProtocolEvent::Initialized(caps));
                }
                Ok(Incoming::Diagnostic(diagnostic)) => {
                    // §14.3 CPU row: produced diagnostics close the analyze
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
                Err(e) => return Err(e.to_string()),
            }
        }
    }
}

/// Spawns the per-generation stdin writer: the only thread writing to the
/// child, so a full pipe blocks it instead of the runtime handle, and the
/// handle-side deadline in [`PluginRuntime::write_frame`] bounds every write.
fn spawn_writer(
    mut stdin: ChildStdin,
    commands: mpsc::Receiver<WriterCommand>,
    acks: Sender<std::io::Result<()>>,
) -> JoinHandle<()> {
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
        .expect("spawn writer thread")
}

fn spawn_monitor(
    pid: u32,
    memory_cap_bytes: u64,
    callback_timeout: Duration,
    created: Instant,
    clock: Arc<AnalyzeClock>,
    child: Arc<Mutex<Option<Child>>>,
    running: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    death: Arc<Mutex<Option<String>>>,
    stats: Arc<PluginStats>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("dartq-plugin-monitor".to_owned())
        .spawn(move || {
            while running.load(Ordering::Relaxed) {
                // §14.3 CPU row, analyze path: a fed demand that has produced
                // no diagnostics for longer than the callback budget is the
                // same isolation breach as the memory row. Checked before the
                // RSS sample because unsampled targets report no RSS at all —
                // the budget must hold everywhere. Record the cause so the
                // crash diagnostic names the timeout, flip `alive` so the
                // next interaction restarts instead of writing into the
                // dying pipe, kill, and close the clock (the demand ends
                // with the generation).
                if let Some(open_for) = clock.open_for(created) {
                    if open_for > callback_timeout {
                        *death.lock().expect("death lock") = Some(format!(
                            "no diagnostics were produced within the {} ms \
                             analyze callback budget",
                            callback_timeout.as_millis()
                        ));
                        alive.store(false, Ordering::Relaxed);
                        if let Some(mut child) = child.lock().expect("child lock").take() {
                            let _ = child.kill();
                            let _ = child.wait();
                        }
                        clock.close();
                        break;
                    }
                }
                match sample_rss(pid) {
                    // A zero sample (process gone) never raises the peak.
                    Some(rss) if rss > 0 => {
                        stats.observe_rss(rss);
                        if rss > memory_cap_bytes {
                            // §14.3 memory enforcement: over-budget child
                            // dies. Record the cause so the crash diagnostic
                            // can name the memory row, and flip `alive` so
                            // the next interaction restarts instead of
                            // writing into the dying pipe; the reader's EOF
                            // turns this into the ordinary crash path.
                            *death.lock().expect("death lock") = Some(format!(
                                "RSS sample {rss} exceeded the {memory_cap_bytes}-byte \
                                 memory budget"
                            ));
                            alive.store(false, Ordering::Relaxed);
                            if let Some(mut child) = child.lock().expect("child lock").take() {
                                let _ = child.kill();
                                let _ = child.wait();
                            }
                            break;
                        }
                    }
                    Some(_) => {}
                    None => break,
                }
                if !alive.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(RSS_POLL_INTERVAL);
            }
        })
        .expect("spawn monitor thread")
}

/// The child's resident set in bytes, `None` once the process is gone.
///
/// - Linux: `/proc/<pid>/status` `VmHWM`, the kernel's exact high-water mark.
/// - macOS: `proc_pid_rusage` `ri_resident_size` (`rusage_info_v0`, ABI
///   pinned against the SDK's `libproc.h`/`sys/resource.h`; flavor
///   `RUSAGE_INFO_V0` == 0).
/// - Other targets: unsampled (the counter stays 0, documented in
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
    #[allow(clippy::struct_field_names)]
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
                message: d.message.clone(),
                offset: u64::from(d.range.offset),
                length: u64::from(d.range.length),
            })
        })
        .collect()
}

/// Converts a runtime failure into the `dartq/`-namespace engine diagnostic
/// the output layer merges (PRD §14.3: a plugin failure costs one tool
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
        RuntimeError::HandshakeTimeout { ms } => (
            DARTQ_PLUGIN_TIMEOUT,
            format!("handshake did not complete within {ms} ms"),
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
        let created = Instant::now() - Duration::from_millis(500);
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
        let pending = PendingDiagnostics::default();
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
        let drained = pending.queue.lock().expect("pending lock");
        assert_eq!(drained.len(), 1, "the CRLF-terminated frame decoded");
        assert_eq!(drained[0].code, "p/lint");
    }

    #[test]
    fn pending_diagnostics_overflow_tears_the_session_down() {
        // The reader-side fuse: the 10_001st un-drained diagnostic is a
        // protocol violation (same 10_000 fuse as the parser's
        // `MAX_DIAGNOSTICS`), not unbounded queue growth.
        let init_reply = r#"{"schema_version":1,"id":1,"method":"initialize","params":{"protocol_version":1,"capabilities":["lint"]}}"#;
        let diagnostic = r#"{"schema_version":1,"id":null,"method":"notification/diagnostic","params":{"file":"f","severity":"warning","code":"c","message":"m"}}"#;
        let mut stream = format!("{init_reply}\n");
        for _ in 0..=MAX_PENDING_DIAGNOSTICS {
            stream.push_str(diagnostic);
            stream.push('\n');
        }

        let session = Mutex::new({
            let mut session = Session::new(CapabilitySet::all());
            session.send_initialize().expect("arm the handshake");
            session
        });
        let pending = PendingDiagnostics::default();
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
            reason.contains(&MAX_PENDING_DIAGNOSTICS.to_string()),
            "the reason names the fuse: {reason}"
        );
        let drained = pending.queue.lock().expect("pending lock");
        assert_eq!(
            drained.len(),
            MAX_PENDING_DIAGNOSTICS,
            "the queue stopped exactly at the cap"
        );
    }

    #[test]
    fn wait_for_burst_wakes_on_the_reader_signal_not_the_deadline() {
        let pending = Arc::new(PendingDiagnostics::default());
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
        let batch = pending
            .wait_for_burst(start + Duration::from_secs(5))
            .expect("the push woke the wait");
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "woke on the push signal, long before the deadline"
        );
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].code, "p/lint");
        reader.join().expect("reader joins");
    }

    #[test]
    fn wait_for_burst_returns_none_at_the_deadline() {
        let pending = PendingDiagnostics::default();
        let start = Instant::now();
        assert!(
            pending
                .wait_for_burst(start + Duration::from_millis(50))
                .is_none()
        );
        assert!(
            start.elapsed() >= Duration::from_millis(40),
            "the deadline bounds the wait"
        );
    }

    #[test]
    fn wait_for_burst_drains_the_whole_reply_not_its_first_line() {
        let pending = Arc::new(PendingDiagnostics::default());
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
        let batch = pending
            .wait_for_burst(Instant::now() + Duration::from_secs(5))
            .expect("the burst arrived");
        assert_eq!(batch.len(), 2, "both rows of the burst, one drain");
        assert_eq!(batch[0].message, "first");
        assert_eq!(batch[1].message, "second");
        reader.join().expect("reader joins");
    }

    #[test]
    fn a_row_after_the_settle_window_lands_in_the_next_drain() {
        let pending = Arc::new(PendingDiagnostics::default());
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
        let batch = pending
            .wait_for_burst(start + Duration::from_secs(5))
            .expect("the first burst arrived");
        assert_eq!(batch.len(), 1, "the window closed on a quiet pipe");
        assert_eq!(batch[0].message, "first");

        let late = pending
            .wait_for_burst(start + Duration::from_secs(5))
            .expect("the later row arrives");
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
