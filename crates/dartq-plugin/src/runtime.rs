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
//!
//! [`PluginRuntime::stats`] is the §14.3 observability face (peak RSS, frame
//! and byte counters, guard drops, restart count) the daemon/CLI read.
//!
//! # PRD §14.3 conformance
//!
//! | Dimension | Mechanism here |
//! | --- | --- |
//! | Memory | RSS monitor thread (100 ms tick) samples the child; Linux reads the kernel's `VmHWM` high-water mark, macOS `proc_pid_rusage` (`ri_resident_size`, SDK-header-pinned ABI); exceeding the manifest's `memory_budget_mb` kills the child. |
//! | CPU / IPC | Per-callback wall-clock enforcement needs the `request/fixes` surface (not exposed on the runtime yet — no fix requests are sent this milestone); `initialize` and `shutdown` already run under the manifest's request deadline. |
//! | Crash | Child death never propagates: the runtime relaunches within the manifest's retry budget (counted in `stats.restarts`), then reports [`RuntimeError::Crashed`]. The `plugin_crashed` diagnostic itself is emitted by the daemon layer, which owns the output merge. |
//! | Frame guard | Lines beyond [`MAX_FRAME_SIZE`] are dropped before buffering (counted) and tear the session down — the transport never resynchronizes. |
//!
//! # Threading (std threads only, per the workspace runtime policy)
//!
//! Per generation (one child process): a reader thread owns the child's
//! stdout, the [`Session`] state machine (behind a mutex shared with the
//! handle), and the pending-diagnostics buffer; a monitor thread samples RSS
//! and enforces the memory budget; the handle thread does every stdin write
//! (`&mut self` makes it the only writer). All shared counters are atomics;
//! the only cross-thread locks are the two leaf mutexes (session, pending)
//! and the child handle, never nested.
//!
//! # Known bounds
//!
//! - A stdin write blocks when the child stops reading and the 64 KiB pipe
//!   fills; write-side timeouts arrive with the daemon's request loop.
//! - `feed` skips unknown [`FileId`]s (a caller-contract violation, same
//!   posture as `FileLayoutProvider`) rather than failing a whole batch.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dartq_diagnostics::{Diagnostic, Severity as EngineSeverity};
use dartq_source::FileTable;
use memchr::memchr;
use thiserror::Error;

use crate::capability::CapabilitySet;
use crate::frame::{Frame, MAX_FRAME_SIZE, ProtocolError, decode_line, encode_frame};
use crate::manifest::Manifest;
use crate::session::{
    Incoming, PluginDiagnostic, Session, SessionError, SessionState, Severity,
    engine_diagnostics_frame,
};
use crate::stats::{PluginStats, PluginStatsSnapshot};

/// How often the isolation monitor samples the child's RSS.
const RSS_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// How often [`PluginRuntime::wait_diagnostics`] re-checks its buffer.
const DIAGNOSTICS_POLL_INTERVAL: Duration = Duration::from_millis(10);

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
    /// Pass through to dartiq's own stderr (tests, debugging).
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
    /// The `shutdown` reply was accepted.
    ShutdownComplete,
    /// The session ended abnormally (protocol violation or lost stream).
    Dead(String),
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
    restart_budget: u32,
    memory_cap_bytes: u64,

    // Per generation (one child process); replaced on every launch.
    session: Arc<Mutex<Session>>,
    stdin: Option<ChildStdin>,
    child: Arc<Mutex<Option<Child>>>,
    events: mpsc::Receiver<ProtocolEvent>,
    pending: Arc<Mutex<Vec<PluginDiagnostic>>>,
    running: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    death: Arc<Mutex<Option<String>>>,
    reader: Option<JoinHandle<()>>,
    monitor: Option<JoinHandle<()>>,
    negotiated: Option<CapabilitySet>,

    // Across generations.
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
        let restart_budget = spec.manifest.timeouts.request_retries;
        let memory_cap_bytes = u64::from(spec.manifest.memory_budget_mb) * 1024 * 1024;
        let (events_tx, events) = mpsc::channel();
        drop(events_tx); // placeholder replaced by the first launch
        let mut runtime = Self {
            spec,
            request_timeout,
            restart_budget,
            memory_cap_bytes,
            session: Arc::new(Mutex::new(Session::new(CapabilitySet::all()))),
            stdin: None,
            child: Arc::new(Mutex::new(None)),
            events,
            pending: Arc::new(Mutex::new(Vec::new())),
            running: Arc::new(AtomicBool::new(false)),
            alive: Arc::new(AtomicBool::new(false)),
            death: Arc::new(Mutex::new(None)),
            reader: None,
            monitor: None,
            negotiated: None,
            stats: Arc::new(PluginStats::default()),
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
    /// budget) before the push.
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
        self.write_frame(&frame)
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
        Ok(std::mem::take(
            &mut self.pending.lock().expect("pending diagnostics lock"),
        ))
    }

    /// [`Self::take_diagnostics`] with a wait: polls until at least one
    /// plugin diagnostic arrived or `timeout` elapsed (empty result). A child
    /// death during the wait restarts it, within budget.
    ///
    /// # Errors
    ///
    /// Restart budget exhaustion and transport failures, per [`RuntimeError`].
    pub fn wait_diagnostics(
        &mut self,
        timeout: Duration,
    ) -> Result<Vec<PluginDiagnostic>, RuntimeError> {
        let deadline = Instant::now() + timeout;
        loop {
            let drained = self.take_diagnostics()?;
            if !drained.is_empty() {
                return Ok(drained);
            }
            if Instant::now() >= deadline {
                return Ok(Vec::new());
            }
            std::thread::sleep(DIAGNOSTICS_POLL_INTERVAL);
        }
    }

    /// The isolation metrics snapshot (PRD §14.3).
    #[must_use]
    pub fn stats(&self) -> PluginStatsSnapshot {
        self.stats.snapshot()
    }

    /// The graceful teardown: `shutdown` request, reply awaited under the
    /// request budget, then the handle closes the plugin's stdin — the
    /// host's read loop ends on that EOF and its process exits, which the
    /// reaping joins on. Consumes the runtime — after `shutdown` there is no
    /// session left (wire contract).
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
            Ok(ProtocolEvent::Dead(reason)) => Err(RuntimeError::Crashed { reason }),
            Ok(ProtocolEvent::Initialized(_)) => Err(RuntimeError::Crashed {
                reason: "plugin re-handshaked mid-session".to_owned(),
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
            // Session over (wire contract). Closing stdin first is what ends
            // the host's read loop and its process; reaping before that would
            // wait on a child that is waiting on our EOF.
            self.stdin = None;
            self.join_generation();
            self.reap_child();
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
        self.pending = Arc::new(Mutex::new(Vec::new()));
        self.running = Arc::new(AtomicBool::new(true));
        self.alive = Arc::new(AtomicBool::new(true));
        self.death = Arc::new(Mutex::new(None));
        self.negotiated = None;
        self.stdin = Some(stdin);
        *self.child.lock().expect("child lock") = Some(child);

        self.reader = Some(spawn_reader(
            stdout,
            Arc::clone(&self.session),
            Arc::clone(&self.pending),
            events_tx,
            Arc::clone(&self.alive),
            Arc::clone(&self.death),
            Arc::clone(&self.stats),
        ));
        self.monitor = Some(spawn_monitor(
            pid,
            self.memory_cap_bytes,
            Arc::clone(&self.child),
            Arc::clone(&self.running),
            Arc::clone(&self.alive),
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
            Ok(ProtocolEvent::Dead(reason)) => Err(RuntimeError::Crashed { reason }),
            Ok(ProtocolEvent::ShutdownComplete) => Err(RuntimeError::Crashed {
                reason: "plugin answered shutdown before initialize".to_owned(),
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

    /// Stops the monitor, reaps the child, joins the reader — for the
    /// generation being replaced. Must run before a new launch overwrites
    /// the per-generation fields.
    fn teardown_generation(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        self.kill_child();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        // Monitors exit within one poll tick; they hold only their own
        // generation's Arcs and are detached here.
        self.monitor = None;
    }

    fn join_generation(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some(monitor) = self.monitor.take() {
            let _ = monitor.join();
        }
    }

    /// Kills the child and reaps it (kill on an exited child is a no-op
    /// error; the `wait` is what prevents zombies).
    fn kill_child(&mut self) {
        let child = self.child.lock().expect("child lock").take();
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.stdin = None;
    }

    fn reap_child(&mut self) {
        let child = self.child.lock().expect("child lock").take();
        if let Some(mut child) = child {
            let _ = child.wait();
        }
        self.stdin = None;
    }

    fn crash_error(&self) -> RuntimeError {
        let reason = self
            .death
            .lock()
            .expect("death lock")
            .clone()
            .unwrap_or_else(|| "plugin process is no longer alive".to_owned());
        RuntimeError::Crashed { reason }
    }

    fn request_deadline(&self) -> Instant {
        Instant::now() + self.request_timeout
    }

    /// The only writer: the handle thread (`&mut self`).
    fn write_frame(&mut self, frame: &Frame) -> Result<(), RuntimeError> {
        let mut bytes = encode_frame(frame)?.into_bytes();
        bytes.push(b'\n');
        let stdin = self.stdin.as_mut().expect("stdin present while alive");
        stdin.write_all(&bytes)?;
        stdin.flush()?;
        self.stats.frame_out();
        self.stats
            .bytes_out(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        Ok(())
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
    pending: Arc<Mutex<Vec<PluginDiagnostic>>>,
    events: Sender<ProtocolEvent>,
    alive: Arc<AtomicBool>,
    death: Arc<Mutex<Option<String>>>,
    stats: Arc<PluginStats>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("dartq-plugin-reader".to_owned())
        .spawn(move || {
            let outcome = run_reader(stdout, &session, &pending, &events, &stats);
            alive.store(false, Ordering::Relaxed);
            if let Err(reason) = outcome {
                *death.lock().expect("death lock") = Some(reason.clone());
                let _ = events.send(ProtocolEvent::Dead(reason));
            }
        })
        .expect("spawn reader thread")
}

/// Reads frames until the stream ends or the contract breaks. `Ok(())` only
/// when the plugin answered `shutdown` (the clean exit).
fn run_reader(
    stdout: ChildStdout,
    session: &Mutex<Session>,
    pending: &Mutex<Vec<PluginDiagnostic>>,
    events: &Sender<ProtocolEvent>,
    stats: &PluginStats,
) -> Result<(), String> {
    let mut reader = std::io::BufReader::new(stdout);
    let mut line = Vec::new();
    loop {
        line.clear();
        match read_bounded_line(&mut reader, &mut line) {
            Ok(LineRead::Eof) => {
                let terminated =
                    session.lock().expect("session lock").state() == SessionState::Terminated;
                return if terminated {
                    Ok(())
                } else {
                    Err("plugin process closed the connection".to_owned())
                };
            }
            Ok(LineRead::Line) => {}
            Ok(LineRead::Oversize) => {
                stats.bytes_in(u64::try_from(line.len()).unwrap_or(u64::MAX));
                stats.frame_dropped();
                return Err(format!(
                    "frame exceeds the {MAX_FRAME_SIZE}-byte frame limit"
                ));
            }
            Ok(LineRead::Unterminated) => {
                stats.bytes_in(u64::try_from(line.len()).unwrap_or(u64::MAX));
                return Err("stream ended mid-frame".to_owned());
            }
            Err(e) => return Err(e.to_string()),
        }
        stats.bytes_in(u64::try_from(line.len()).unwrap_or(u64::MAX));
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line.is_empty() {
            continue;
        }
        let frame = match decode_line(&line) {
            Ok(frame) => frame,
            Err(ProtocolError::TooLarge { .. }) => {
                stats.frame_dropped();
                return Err(format!(
                    "frame exceeds the {MAX_FRAME_SIZE}-byte frame limit"
                ));
            }
            Err(e) => return Err(e.to_string()),
        };
        stats.frame_in();
        let incoming = session.lock().expect("session lock").handle(frame);
        match incoming {
            Ok(Incoming::Initialized(caps)) => {
                let _ = events.send(ProtocolEvent::Initialized(caps));
            }
            Ok(Incoming::Diagnostic(diagnostic)) => {
                pending
                    .lock()
                    .expect("pending diagnostics lock")
                    .push(diagnostic);
            }
            Ok(Incoming::ShutdownComplete) => {
                let _ = events.send(ProtocolEvent::ShutdownComplete);
                return Ok(());
            }
            // The runtime never sends `request/fixes`; a matching reply id
            // cannot exist, so the session machine would have errored first.
            Ok(Incoming::Fixes(_)) => {
                return Err("unsolicited request/fixes reply".to_owned());
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn spawn_monitor(
    pid: u32,
    memory_cap_bytes: u64,
    child: Arc<Mutex<Option<Child>>>,
    running: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    stats: Arc<PluginStats>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("dartq-plugin-monitor".to_owned())
        .spawn(move || {
            while running.load(Ordering::Relaxed) {
                match sample_rss(pid) {
                    // A zero sample (process gone) never raises the peak.
                    Some(rss) if rss > 0 => {
                        stats.observe_rss(rss);
                        if rss > memory_cap_bytes {
                            // §14.3 memory enforcement: over-budget child
                            // dies; the reader's EOF turns this into the
                            // ordinary crash/restart path.
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

// --- bounded line reader ---

/// Outcome of one bounded line read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineRead {
    /// EOF at a line boundary.
    Eof,
    /// One complete line (the caller trims the terminator).
    Line,
    /// [`MAX_FRAME_SIZE`] exceeded before a newline appeared.
    Oversize,
    /// EOF with a partial frame buffered.
    Unterminated,
}

/// Reads up to and including the next `'\n'`, never buffering more than
/// [`MAX_FRAME_SIZE`] + 1 bytes: the guard fires before allocation, keeping
/// host memory bounded whatever the plugin writes.
fn read_bounded_line(reader: &mut impl BufRead, buf: &mut Vec<u8>) -> std::io::Result<LineRead> {
    buf.clear();
    loop {
        // The cap check leads: a buffer already at the limit is an oversize
        // frame even when EOF arrives in the same breath.
        let room = (MAX_FRAME_SIZE + 1) - buf.len();
        if room == 0 {
            return Ok(LineRead::Oversize);
        }
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if available.is_empty() {
            return Ok(if buf.is_empty() {
                LineRead::Eof
            } else {
                LineRead::Unterminated
            });
        }
        let take = available.len().min(room);
        if let Some(pos) = memchr(b'\n', &available[..take]) {
            buf.extend_from_slice(&available[..=pos]);
            reader.consume(pos + 1);
            return Ok(LineRead::Line);
        }
        buf.extend_from_slice(&available[..take]);
        reader.consume(take);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn read_lines(input: &[u8]) -> Vec<(LineRead, String)> {
        let mut reader = Cursor::new(input);
        let mut buf = Vec::new();
        let mut out = Vec::new();
        loop {
            buf.clear();
            match read_bounded_line(&mut reader, &mut buf).expect("read") {
                LineRead::Eof => break,
                read => out.push((read, String::from_utf8(buf.clone()).expect("utf8"))),
            }
        }
        out
    }

    #[test]
    fn bounded_reader_splits_lines_and_flags_broken_ends() {
        let lines = read_lines(b"one\ntwo\r\n\nthree");
        assert_eq!(
            lines,
            vec![
                (LineRead::Line, "one\n".to_owned()),
                (LineRead::Line, "two\r\n".to_owned()),
                (LineRead::Line, "\n".to_owned()),
                (LineRead::Unterminated, "three".to_owned()),
            ]
        );
        assert_eq!(read_lines(b""), Vec::new());
    }

    #[test]
    fn bounded_reader_flags_oversize_before_buffering() {
        let long = vec![b'x'; MAX_FRAME_SIZE + 1];
        let lines = read_lines(&long);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].0, LineRead::Oversize);
        assert_eq!(lines[0].1.len(), MAX_FRAME_SIZE + 1);
    }

    #[test]
    fn wire_conversion_maps_fields_and_skips_unknown_files() {
        let mut files = FileTable::new();
        // An absolute, already-clean spelling: the table normalizes paths
        // (cwd-relative names absolutize), so the expected wire form is the
        // canonical spelling itself.
        let source = std::env::temp_dir().join("dartq_plugin_wire_test.dart");
        let known = files.intern(&source);
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
        assert_eq!(wire[0].file, source.to_string_lossy());
        assert_eq!(wire[0].severity, Severity::Warning);
        assert_eq!(wire[0].code, "uri_does_not_exist");
        assert_eq!(wire[0].offset, 12);
        assert_eq!(wire[0].length, 8);
    }
}
