//! AI-first debug session orchestration over the VM Service client:
//! launch/attach, breakpoints (line, entry, conditional), step-and-
//! package stopped snapshots, lazy object-graph expansion, exception
//! pause mode, hot reload/restart, and bounded stdout/stderr capture.
//!
//! Ordering contract (fixed by the VM service): connect, negotiate
//! `getVersion`, subscribe streams, then arm breakpoints. Subscriptions
//! must be live before any breakpoint can fire, so a pause event is
//! never missed between arming and listening.
//!
//! Conditional breakpoints are an orchestration-layer feature: the VM
//! service has no condition parameter on the `addBreakpoint*` RPCs
//! (verified against the protocol spec), so a condition is evaluated
//! with `evaluateInFrame` each time the breakpoint pauses the isolate;
//! a falsy or failing condition auto-resumes. Consecutive auto-resumes
//! are capped so a ping-ponging condition surfaces as a stop instead
//! of hanging the session.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, TrySendError, bounded};
use rustc_hash::FxHashMap;
use serde_json::{Value, json};

use crate::client::{StreamEvent, VmClient};
use crate::error::{Error, Result};
use crate::protocol::{
    Breakpoint, BoundField, Event, EventKind, ExceptionPauseMode, Frame, Instance, InstanceKind,
    IsolateRef, ScriptRef, SentinelKind, Stack, StepOption, ValueOrSentinel, VmObject,
};
use crate::ring::Ring;

/// The VM Service protocol major version this client implements.
const SUPPORTED_MAJOR: u64 = 4;

/// Streams every session subscribes to before arming breakpoints:
/// `Debug` carries pause/breakpoint events, `Isolate` the isolate
/// lifecycle, `Stdout`/`Stderr` process output, `Logging`
/// `dart:developer.log` records.
const STREAMS: [&str; 5] = ["Debug", "Isolate", "Stdout", "Stderr", "Logging"];

/// How many output chunks the stdout/stderr rings retain. A chunk is
/// whatever one pipe read produced, so total bytes are bounded by
/// `capacity × PIPE_READ_SIZE`.
const OUTPUT_CHUNK_CAPACITY: usize = 4096;

/// Bytes requested per pipe read.
const PIPE_READ_SIZE: usize = 8 * 1024;

/// How many pipe events may queue between a reader thread and the
/// session; a full queue drops chunks rather than blocking the child.
const PIPE_QUEUE_CAPACITY: usize = 4096;

/// Bounds for lazy object-graph expansion.
#[derive(Debug, Clone, Copy)]
pub struct ExpandLimits {
    /// Maximum children returned per expansion level.
    pub max_children: usize,
    /// Maximum levels `expand_graph` walks from the root.
    pub max_depth: usize,
}

impl Default for ExpandLimits {
    fn default() -> Self {
        Self {
            max_children: 100,
            max_depth: 3,
        }
    }
}

/// Bounded auto-resume policy for conditional breakpoints.
#[derive(Debug, Clone, Copy)]
pub struct ConditionalResumeLimit {
    /// Consecutive auto-resumes allowed before reporting a stop.
    pub max_auto_resumes: u32,
}

impl Default for ConditionalResumeLimit {
    fn default() -> Self {
        Self {
            max_auto_resumes: 1000,
        }
    }
}

/// Session-wide wiring knobs; the defaults are the production values.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Retained stream-event capacity (ring).
    pub event_capacity: usize,
    /// Retained output chunks per stream (ring).
    pub output_capacity: usize,
    /// Conditional-breakpoint auto-resume ceiling.
    pub conditional_resume_limit: ConditionalResumeLimit,
    /// Frames included in a stopped snapshot's stack summary.
    pub stack_summary_frames: usize,
    /// How long a step/resume waits for the resulting pause.
    pub pause_timeout: Duration,
    /// Default depth/width for lazy object expansion.
    pub expand_limits: ExpandLimits,
    /// Timeout for ordinary RPCs (slow operations lift it per call).
    pub rpc_timeout: Duration,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            event_capacity: 10_000,
            output_capacity: OUTPUT_CHUNK_CAPACITY,
            conditional_resume_limit: ConditionalResumeLimit::default(),
            stack_summary_frames: 20,
            pause_timeout: Duration::from_secs(30),
            expand_limits: ExpandLimits::default(),
            rpc_timeout: Duration::from_secs(30),
        }
    }
}

/// How to start a Dart process under debug.
#[derive(Debug, Clone)]
pub struct LaunchConfig {
    /// Script to run (`dart <script> [args]`).
    pub script: PathBuf,
    /// Arguments passed to the script.
    pub args: Vec<String>,
    /// Extra VM flags placed before the script path.
    pub vm_flags: Vec<String>,
    /// Pause the main isolate at start (`--pause-isolates-on-start`).
    /// The AI-first default is `true`: breakpoints are armed before the
    /// program runs.
    pub pause_on_start: bool,
    /// How long to wait for the service-URI banner.
    pub launch_timeout: Duration,
    /// Working directory for the child.
    pub cwd: Option<PathBuf>,
}

impl LaunchConfig {
    /// A launch configuration for `script` with the AI-first defaults
    /// (paused at start, 30 s banner timeout).
    #[must_use]
    pub fn new(script: impl Into<PathBuf>) -> Self {
        Self {
            script: script.into(),
            args: Vec::new(),
            vm_flags: Vec::new(),
            pause_on_start: true,
            launch_timeout: Duration::from_secs(30),
            cwd: None,
        }
    }
}

/// Where a breakpoint goes.
#[derive(Debug, Clone)]
pub enum BreakpointLocationSpec {
    /// A source line (`addBreakpointWithScriptUri`); works before the
    /// script has an id, which is the AI-first path (an agent knows the
    /// file, not the VM's script handle).
    Line {
        /// `package:`/`file:` URI of the target script.
        script_uri: String,
        /// 1-based line.
        line: u32,
        /// 1-based column, when the line holds several breakpoints.
        column: Option<u32>,
    },
    /// A function entry (`addBreakpointAtEntry`).
    Function {
        /// The `@Function` object id.
        function_id: String,
    },
}

/// A breakpoint request plus its optional orchestration condition.
#[derive(Debug, Clone)]
pub struct BreakpointSpec {
    /// The target isolate; `None` means the session's main isolate.
    pub isolate: Option<String>,
    /// Where to break.
    pub location: BreakpointLocationSpec,
    /// Dart expression evaluated in the top frame on each hit; a falsy
    /// value or an evaluation error auto-resumes. `None` breaks every
    /// time.
    pub condition: Option<String>,
}

/// A breakpoint as the session tracks it: VM identity, condition, and
/// the VM's resolution state (a breakpoint in not-yet-loaded code is
/// unresolved until `BreakpointResolved`).
#[derive(Debug, Clone)]
pub struct BreakpointHandle {
    /// VM breakpoint id (`breakpoints/<n>`).
    pub id: String,
    /// Isolate the breakpoint belongs to.
    pub isolate: String,
    /// The condition, when one was supplied.
    pub condition: Option<String>,
    /// Whether the VM has bound the breakpoint to a concrete location.
    pub resolved: bool,
}

/// One variable, rendered for an agent: name, short value
/// representation, and the object handle for lazy expansion.
#[derive(Debug, Clone)]
pub struct Variable {
    /// Variable name.
    pub name: String,
    /// Short rendering (`"42"`, `"null"`, `Instance of Foo`, a sentinel
    /// spelling, or a container summary).
    pub value: String,
    /// Object handle (instance id) for [`DebugSession::expand`];
    /// `None` for sentinels and id-less values.
    pub handle: Option<String>,
}

/// Where execution is paused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameLocation {
    /// 1-based line.
    pub line: Option<i64>,
    /// 1-based column.
    pub column: Option<i64>,
    /// Script URI.
    pub script_uri: Option<String>,
    /// Script object id.
    pub script_id: Option<String>,
    /// Function name.
    pub function: Option<String>,
}

/// A compact stack entry for a stopped snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameSummary {
    /// Frame index (0 = top).
    pub index: u32,
    /// Function name.
    pub function: Option<String>,
    /// Position in source.
    pub location: FrameLocation,
}

/// Why the isolate stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoppedReason {
    /// Paused at `--pause-isolates-on-start`.
    Start,
    /// Paused at one or more breakpoints (condition true or absent).
    Breakpoint {
        /// The VM breakpoint ids hit here.
        breakpoints: Vec<String>,
    },
    /// Paused on an exception (mode-dependent).
    Exception,
    /// Paused by an explicit [`DebugSession::pause`].
    Interrupted,
    /// Paused at isolate exit.
    Exit,
    /// Paused after a service request (e.g. reload).
    PostRequest,
    /// The conditional-breakpoint auto-resume budget ran out.
    ConditionalResumeLimit {
        /// How many times the condition auto-resumed in a row.
        auto_resumes: u32,
    },
    /// A pause kind this crate does not model.
    Other(EventKind),
}

/// The one-call state bundle an agent needs after a step or a hit:
/// reason, isolate, top frame position, a bounded stack summary, and
/// the top frame's variables as expandable handles.
#[derive(Debug, Clone)]
pub struct StoppedSnapshot {
    /// Why the isolate stopped.
    pub reason: StoppedReason,
    /// The stopped isolate.
    pub isolate: String,
    /// Top frame position.
    pub location: FrameLocation,
    /// First `stack_summary_frames` frames.
    pub stack: Vec<FrameSummary>,
    /// Locals of the top frame.
    pub variables: Vec<Variable>,
    /// Whether the pause is at an await/yield suspension.
    pub at_async_suspension: bool,
}

/// One stream event as the session recorded it.
#[derive(Debug, Clone)]
pub struct SessionEvent {
    /// Monotonic session cursor (the event's sequence number).
    pub cursor: u64,
    /// VM stream id (`Debug`, `Isolate`, `Stdout`, …).
    pub stream_id: String,
    /// The parsed event.
    pub event: Event,
    /// The raw event JSON, retained only when the typed [`Event`]
    /// does not model the kind's payload — the 10k-deep ring would
    /// otherwise hold ~10k duplicate `Value`s. `Some` for
    /// [`EventKind::Other`] (unknown kinds need their raw form to be
    /// useful at all) and for kinds carrying fields outside the typed
    /// event (`Inspect`, `Gc`, `CpuSamples`, `TimelineEvents`); `None`
    /// for every fully modeled kind.
    pub raw: Option<Value>,
}

/// A page of buffered stream events.
#[derive(Debug, Clone)]
pub struct EventBatch {
    /// Events in arrival order.
    pub events: Vec<SessionEvent>,
    /// Cursor to pass to the next call.
    pub next_cursor: u64,
    /// Events evicted before this page could serve them.
    pub dropped: u64,
}

/// A page of captured process output.
#[derive(Debug, Clone)]
pub struct OutputBatch {
    /// Concatenated bytes, oldest first.
    pub bytes: Vec<u8>,
    /// Cursor to pass to the next call.
    pub next_cursor: u64,
    /// Chunks dropped before this page.
    pub dropped: u64,
}

/// What a pipe reader thread forwards to the session.
enum PipeEvent {
    /// Raw stdout bytes.
    Stdout(Vec<u8>),
    /// Raw stderr bytes.
    Stderr(Vec<u8>),
    /// The service URI found in either stream.
    Banner(String),
    /// The stream ended.
    Eof,
}

/// A live debug session: connection, process (when launched), isolate
/// tracking, breakpoint registry, and bounded buffers.
pub struct DebugSession {
    client: VmClient,
    process: Option<Child>,
    config: SessionConfig,
    /// Known isolates by id.
    isolates: FxHashMap<String, IsolateRef>,
    /// The default target: the first non-system isolate.
    main_isolate: Option<String>,
    /// Breakpoints by VM id.
    breakpoints: FxHashMap<String, BreakpointHandle>,
    /// Most recent pause event per isolate.
    pause_events: FxHashMap<String, Event>,
    /// Stream events ring (sequence == [`SessionEvent::cursor`]).
    events: Ring<SessionEvent>,
    /// Captured stdout chunks.
    stdout: Ring<Vec<u8>>,
    /// Captured stderr chunks.
    stderr: Ring<Vec<u8>>,
    /// Pipe reader feed (`None` when attached, not launched).
    pipe_feed: Option<Receiver<PipeEvent>>,
    /// Consecutive auto-resumes per isolate, without a reported stop.
    ///
    /// Invariant: an entry is *only* cleared when that isolate's own
    /// pause resolves to `PauseOutcome::Report` (a real stop) or when
    /// the isolate exits. A `Resume` event observed on another
    /// isolate, or the resume RPC this session itself issues, must
    /// never touch a sibling isolate's counter — otherwise one
    /// isolate's looping conditional breakpoint would push a
    /// different isolate toward `ConditionalResumeLimit` and surface
    /// a false stop.
    auto_resumes: FxHashMap<String, u32>,
    /// Events the client-level feed dropped because its bounded
    /// channel was full — accumulated into [`EventBatch::dropped`] so
    /// agents see the true gap.
    event_drops_from_client: u64,
    /// The `dropped_events` watermark already folded into
    /// `event_drops_from_client`.
    last_seen_client_drops: u64,
    /// Root-library object id per isolate, resolved lazily by
    /// [`DebugSession::evaluate_in_root_lib`].
    root_libs: FxHashMap<String, String>,
}

impl DebugSession {
    /// Launches `dart` with the VM service enabled, parses the service
    /// URI banner, connects, negotiates the protocol version,
    /// subscribes the session streams, and returns the live session.
    ///
    /// # Errors
    ///
    /// [`Error::DartBinaryNotFound`] when neither `DARTQ_DART_BIN` nor
    /// `PATH` yields a `dart`; [`Error::LaunchTimeout`] when no banner
    /// arrives in time; [`Error::ProcessExited`] when the child dies
    /// first; plus the connection errors of [`DebugSession::attach_with`].
    pub fn launch(config: LaunchConfig) -> Result<Self> {
        Self::launch_with(config, SessionConfig::default())
    }

    /// [`DebugSession::launch`] with explicit session wiring.
    ///
    /// # Errors
    ///
    /// See [`DebugSession::launch`].
    pub fn launch_with(config: LaunchConfig, settings: SessionConfig) -> Result<Self> {
        let dart = find_dart_binary()?;
        let mut command = Command::new(&dart);
        command
            .arg("--enable-vm-service=0")
            .args(&config.vm_flags);
        if config.pause_on_start {
            command.arg("--pause-isolates-on-start");
        }
        command
            .arg(&config.script)
            .args(&config.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &config.cwd {
            command.current_dir(cwd);
        }
        let mut child = command.spawn().map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::DartBinaryNotFound(dart.display().to_string())
            } else {
                Error::Io(error)
            }
        })?;

        let (tx, rx) = bounded(PIPE_QUEUE_CAPACITY);
        if let Some(stdout) = child.stdout.take() {
            spawn_pipe_reader(stdout, PipeKind::Stdout, tx.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_pipe_reader(stderr, PipeKind::Stderr, tx.clone());
        }
        drop(tx);

        let (service_uri, early_stdout, early_stderr) =
            await_banner(&mut child, &rx, config.launch_timeout)?;
        let ws_uri = service_uri_to_ws(&service_uri)?;
        let client = VmClient::connect(&ws_uri)?;
        let mut session = Self {
            client,
            process: Some(child),
            events: Ring::new(settings.event_capacity),
            stdout: Ring::new(settings.output_capacity),
            stderr: Ring::new(settings.output_capacity),
            config: settings,
            isolates: FxHashMap::default(),
            main_isolate: None,
            breakpoints: FxHashMap::default(),
            pause_events: FxHashMap::default(),
            pipe_feed: Some(rx),
            auto_resumes: FxHashMap::default(),
            event_drops_from_client: 0,
            last_seen_client_drops: 0,
            root_libs: FxHashMap::default(),
        };
        for chunk in early_stdout {
            session.stdout.push(chunk);
        }
        for chunk in early_stderr {
            session.stderr.push(chunk);
        }
        session.handshake()?;
        Ok(session)
    }

    /// Attaches to an already-running VM service endpoint (its `ws`
    /// URI), then performs the handshake.
    ///
    /// # Errors
    ///
    /// [`Error::VersionIncompatible`] when the peer's protocol major
    /// differs; transport errors from the connection; [`Error::Rpc`]
    /// when a session stream cannot be subscribed.
    pub fn attach(ws_uri: &str) -> Result<Self> {
        Self::attach_with(ws_uri, SessionConfig::default())
    }

    /// [`DebugSession::attach`] with explicit session wiring.
    ///
    /// # Errors
    ///
    /// See [`DebugSession::attach`].
    pub fn attach_with(ws_uri: &str, settings: SessionConfig) -> Result<Self> {
        let client = VmClient::connect(ws_uri)?;
        let mut session = Self {
            client,
            process: None,
            events: Ring::new(settings.event_capacity),
            stdout: Ring::new(settings.output_capacity),
            stderr: Ring::new(settings.output_capacity),
            config: settings,
            isolates: FxHashMap::default(),
            main_isolate: None,
            breakpoints: FxHashMap::default(),
            pause_events: FxHashMap::default(),
            pipe_feed: None,
            auto_resumes: FxHashMap::default(),
            event_drops_from_client: 0,
            last_seen_client_drops: 0,
            root_libs: FxHashMap::default(),
        };
        session.handshake()?;
        Ok(session)
    }

    /// Negotiates the protocol version, subscribes the session
    /// streams, and seeds isolate tracking from `getVM`.
    fn handshake(&mut self) -> Result<()> {
        let version = self.client.get_version()?;
        if version.major != SUPPORTED_MAJOR {
            return Err(Error::VersionIncompatible {
                expected_major: SUPPORTED_MAJOR,
                found_major: version.major,
                found_minor: version.minor,
            });
        }
        // Subscribe before anything can pause so no early event is
        // lost; a duplicate subscription (103) is benign when another
        // client already listens on this VM.
        for stream in STREAMS {
            match self.client.stream_listen(stream) {
                Ok(()) | Err(Error::Rpc { code: 103, .. }) => {}
                Err(error) => return Err(error),
            }
        }
        let vm = self.client.get_vm()?;
        for isolate in vm.isolates.into_iter().chain(vm.system_isolates) {
            self.note_isolate(isolate);
        }
        Ok(())
    }

    /// The default isolate id (first non-system isolate) — what
    /// breakpoints and steps target when no isolate is named.
    #[must_use]
    pub fn main_isolate(&self) -> Option<&str> {
        self.main_isolate.as_deref()
    }

    /// Every isolate the session knows about.
    #[must_use]
    pub fn isolates(&self) -> impl Iterator<Item = &IsolateRef> {
        self.isolates.values()
    }

    /// Session configuration in force.
    #[must_use]
    pub fn config(&self) -> &SessionConfig {
        &self.config
    }

    /// Adds a breakpoint. Line breakpoints use
    /// `addBreakpointWithScriptUri` so they can be armed before the
    /// script is loaded; the returned handle starts unresolved when the
    /// VM says so and flips on `BreakpointResolved`.
    ///
    /// # Errors
    ///
    /// [`Error::Rpc`] code 102 when the VM cannot place the breakpoint;
    /// [`Error::Sentinel`] when the isolate is gone.
    pub fn add_breakpoint(&mut self, spec: BreakpointSpec) -> Result<BreakpointHandle> {
        let isolate = self.resolve_isolate(spec.isolate.as_deref())?;
        let breakpoint: Breakpoint = match &spec.location {
            BreakpointLocationSpec::Line {
                script_uri,
                line,
                column,
            } => {
                let mut params = json!({
                    "isolateId": isolate,
                    "scriptUri": script_uri,
                    "line": line,
                });
                if let Some(column) = column {
                    params["column"] = json!(column);
                }
                self.client.call_typed(
                    "addBreakpointWithScriptUri",
                    params,
                    self.config.rpc_timeout,
                )?
            }
            BreakpointLocationSpec::Function { function_id } => self.client.call_typed(
                "addBreakpointAtEntry",
                json!({ "isolateId": isolate, "functionId": function_id }),
                self.config.rpc_timeout,
            )?,
        };
        let handle = BreakpointHandle {
            id: breakpoint.id.clone(),
            isolate,
            condition: spec.condition,
            resolved: breakpoint.resolved,
        };
        self.breakpoints.insert(handle.id.clone(), handle.clone());
        Ok(handle)
    }

    /// Removes a breakpoint by its VM id. Removing an unknown id is a
    /// no-op (the breakpoint is already gone).
    ///
    /// # Errors
    ///
    /// [`Error::Sentinel`] when the isolate is gone; [`Error::Rpc`]
    /// when the VM rejects the removal.
    pub fn remove_breakpoint(&mut self, breakpoint_id: &str) -> Result<()> {
        let Some(handle) = self.breakpoints.remove(breakpoint_id) else {
            return Ok(());
        };
        self.client.call_unit(
            "removeBreakpoint",
            json!({ "isolateId": handle.isolate, "breakpointId": breakpoint_id }),
            self.config.rpc_timeout,
        )
    }

    /// Every breakpoint the session tracks.
    #[must_use]
    pub fn breakpoints(&self) -> impl Iterator<Item = &BreakpointHandle> {
        self.breakpoints.values()
    }

    /// Sets the exception pause mode (`None`/`Unhandled`/`All`).
    ///
    /// # Errors
    ///
    /// [`Error::Sentinel`] when the isolate is gone; transport errors.
    pub fn set_exception_pause_mode(
        &mut self,
        isolate: Option<&str>,
        mode: ExceptionPauseMode,
    ) -> Result<()> {
        let isolate = self.resolve_isolate(isolate)?;
        // `setExceptionPauseMode` is deprecated upstream in favor of
        // `setIsolatePauseMode`; prefer the forward RPC and fall back
        // only for VMs that predate it.
        match self.client.call_unit(
            "setIsolatePauseMode",
            json!({ "isolateId": isolate, "exceptionPauseMode": mode }),
            self.config.rpc_timeout,
        ) {
            Ok(()) => Ok(()),
            Err(Error::Rpc { code, .. }) if code == -32601 || code == 100 => {
                self.client.call_unit(
                    "setExceptionPauseMode",
                    json!({ "isolateId": isolate, "mode": mode }),
                    self.config.rpc_timeout,
                )
            }
            Err(error) => Err(error),
        }
    }

    /// Resumes the isolate and packages everything an agent needs about
    /// the next stop into one [`StoppedSnapshot`].
    ///
    /// `step` of `None` is a plain resume (runs to the next
    /// breakpoint/exception); `Some` single-steps.
    ///
    /// # Errors
    ///
    /// [`Error::Rpc`] code 106 when the isolate is not paused; the
    /// pause wait times out with [`Error::Rpc`] code -1.
    pub fn step_and_package(
        &mut self,
        isolate: Option<&str>,
        step: Option<StepOption>,
    ) -> Result<StoppedSnapshot> {
        let isolate = self.resolve_isolate(isolate)?;
        let mut params = json!({ "isolateId": isolate });
        if let Some(step) = step {
            params["step"] = json!(step.as_str());
        }
        // A stale pause record from before this resume would be
        // mistaken for the resume's result.
        self.pause_events.remove(&isolate);
        self.client
            .call_unit("resume", params, self.config.rpc_timeout)?;
        self.wait_for_pause(&isolate)
    }

    /// Resumes without waiting for the next stop (auto-resume and
    /// fire-and-forget runs).
    ///
    /// # Errors
    ///
    /// [`Error::Sentinel`] when the isolate is gone; transport errors.
    pub fn resume(&mut self, isolate: Option<&str>) -> Result<()> {
        let isolate = self.resolve_isolate(isolate)?;
        self.pause_events.remove(&isolate);
        self.client.call_unit(
            "resume",
            json!({ "isolateId": isolate }),
            self.config.rpc_timeout,
        )
    }

    /// Interrupts a running isolate; the pause arrives as an event.
    ///
    /// # Errors
    ///
    /// [`Error::Sentinel`] when the isolate is gone.
    pub fn pause(&mut self, isolate: Option<&str>) -> Result<()> {
        let isolate = self.resolve_isolate(isolate)?;
        self.client.call_unit(
            "pause",
            json!({ "isolateId": isolate }),
            self.config.rpc_timeout,
        )
    }

    /// Waits on the event feed until `isolate` pauses, then builds
    /// the snapshot. Conditional breakpoints resolve here: a falsy
    /// condition resumes and loops, bounded by the auto-resume
    /// ceiling.
    ///
    /// The wait is event-driven: after each drain the session blocks
    /// on the client's bounded channel for up to 2 ms (or the
    /// remaining deadline, whichever is shorter), instead of polling
    /// on a fixed sleep. A pause event therefore wakes the waiter as
    /// soon as the connection thread delivers it — typical wake
    /// latency is channel-bound, not tick-bound — while the deadline
    /// is still re-checked at least every 2 ms.
    ///
    /// # Errors
    ///
    /// [`Error::Rpc`] code -1 when no pause arrives within the
    /// configured timeout; [`Error::Closed`] when an attached
    /// connection drops while waiting.
    pub fn wait_for_pause(&mut self, isolate: &str) -> Result<StoppedSnapshot> {
        let isolate = isolate.to_owned();
        let deadline = Instant::now() + self.config.pause_timeout;
        /// Upper bound on one blocking wait: short enough that the
        /// deadline is re-checked promptly, long enough that an idle
        /// wait does not spin.
        const WAKE_QUANTUM: Duration = Duration::from_millis(2);
        loop {
            self.poll_events()?;
            if let Some(event) = self.pause_events.get(&isolate).cloned() {
                match self.classify_pause(&event) {
                    PauseOutcome::Report(reason) => {
                        self.auto_resumes.remove(&isolate);
                        return self.snapshot(&isolate, reason, &event);
                    }
                    PauseOutcome::AutoResume => {
                        let consecutive =
                            self.auto_resumes.entry(isolate.clone()).or_insert(0);
                        *consecutive += 1;
                        if *consecutive > self.config.conditional_resume_limit.max_auto_resumes {
                            let reason = StoppedReason::ConditionalResumeLimit {
                                auto_resumes: *consecutive,
                            };
                            self.auto_resumes.remove(&isolate);
                            return self.snapshot(&isolate, reason, &event);
                        }
                        self.pause_events.remove(&isolate);
                        self.client.call_unit(
                            "resume",
                            json!({ "isolateId": isolate }),
                            self.config.rpc_timeout,
                        )?;
                        continue;
                    }
                }
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::Rpc {
                    code: -1,
                    message: format!("timed out waiting for isolate {isolate} to pause"),
                    details: None,
                });
            }
            match self.client.next_event(remaining.min(WAKE_QUANTUM)) {
                Ok(Some(event)) => self.absorb_event(event),
                Ok(None) => {}
                Err(Error::Closed) => {
                    // Attached: the feed is gone, surface it. Launched:
                    // the child may still be writing pipe output or be
                    // about to exit — keep polling until the deadline,
                    // but the channel never re-opens, so fall back to
                    // a sleep instead of spinning on `next_event`.
                    if self.process.is_none() {
                        return Err(Error::Closed);
                    }
                    std::thread::sleep(remaining.min(WAKE_QUANTUM));
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Drains the connection's event feed into the session ring and
    /// updates isolate/breakpoint/pause state; pipe output feeds the
    /// stdout/stderr rings.
    ///
    /// # Errors
    ///
    /// [`Error::Closed`] when an attached connection has dropped (a
    /// launched session treats the same condition as process exit).
    pub fn poll_events(&mut self) -> Result<()> {
        loop {
            match self.client.next_event(Duration::from_millis(0)) {
                Ok(Some(event)) => self.absorb_event(event),
                Ok(None) => break,
                Err(Error::Closed) => {
                    if self.process.is_none() {
                        return Err(Error::Closed);
                    }
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        if let Some(feed) = &self.pipe_feed {
            while let Ok(pipe) = feed.try_recv() {
                match pipe {
                    PipeEvent::Stdout(bytes) => {
                        self.stdout.push(bytes);
                    }
                    PipeEvent::Stderr(bytes) => {
                        self.stderr.push(bytes);
                    }
                    // The launch banner was consumed during launch; a
                    // late duplicate is harmless.
                    PipeEvent::Banner(_) | PipeEvent::Eof => {}
                }
            }
        }
        // Events that never reached the session because the client
        // feed was full still count as lost — fold them into the
        // ring's drop accounting so `events_since` reports the gap.
        let dropped = self.client.dropped_events();
        self.event_drops_from_client += dropped.saturating_sub(self.last_seen_client_drops);
        self.last_seen_client_drops = dropped;
        Ok(())
    }

    /// Stream events at or after `cursor`.
    #[must_use]
    pub fn events_since(&self, cursor: u64) -> EventBatch {
        EventBatch {
            dropped: self
                .events
                .oldest_seq()
                .saturating_sub(cursor)
                .saturating_add(self.event_drops_from_client),
            events: self.events.since(cursor).into_iter().cloned().collect(),
            next_cursor: self.events.pushed(),
        }
    }

    /// Captured stdout at or after `cursor` (chunk sequence space).
    #[must_use]
    pub fn stdout_since(&self, cursor: u64) -> OutputBatch {
        output_batch(&self.stdout, cursor)
    }

    /// Captured stderr at or after `cursor`.
    #[must_use]
    pub fn stderr_since(&self, cursor: u64) -> OutputBatch {
        output_batch(&self.stderr, cursor)
    }

    /// The current stack of `isolate`, limited to `limit` frames.
    ///
    /// # Errors
    ///
    /// [`Error::Sentinel`] when the isolate is gone.
    pub fn stack(&mut self, isolate: Option<&str>, limit: usize) -> Result<Stack> {
        let isolate = self.resolve_isolate(isolate)?;
        self.client.call_typed(
            "getStack",
            json!({ "isolateId": isolate, "limit": limit }),
            self.config.rpc_timeout,
        )
    }

    /// The full isolate object (`getIsolate`).
    ///
    /// # Errors
    ///
    /// [`Error::Sentinel`] when the isolate is gone.
    pub fn isolate(&mut self, isolate: Option<&str>) -> Result<crate::protocol::Isolate> {
        let isolate = self.resolve_isolate(isolate)?;
        self.client.call_typed(
            "getIsolate",
            json!({ "isolateId": isolate }),
            self.config.rpc_timeout,
        )
    }

    /// Every script loaded in `isolate` — line-breakpoint targets and
    /// URI→script mapping for agents.
    ///
    /// # Errors
    ///
    /// [`Error::Sentinel`] when the isolate is gone.
    pub fn scripts(&mut self, isolate: Option<&str>) -> Result<Vec<ScriptRef>> {
        let isolate = self.resolve_isolate(isolate)?;
        let list: crate::protocol::ScriptList = self.client.call_typed(
            "getScripts",
            json!({ "isolateId": isolate }),
            self.config.rpc_timeout,
        )?;
        Ok(list.scripts)
    }

    /// Evaluates `expression` in a stack frame (`frame_index` 0 is the
    /// top frame).
    ///
    /// # Errors
    ///
    /// [`Error::Rpc`] code 113 when the expression fails to compile;
    /// [`Error::Sentinel`] when the isolate is gone.
    pub fn evaluate_in_frame(
        &mut self,
        isolate: Option<&str>,
        frame_index: u32,
        expression: &str,
    ) -> Result<VmObject> {
        let isolate = self.resolve_isolate(isolate)?;
        let value = self.client.call(
            "evaluateInFrame",
            json!({
                "isolateId": isolate,
                "frameIndex": frame_index,
                "expression": expression,
            }),
            self.config.rpc_timeout,
        )?;
        Ok(VmObject::from_value(value))
    }

    /// Evaluates `expression` against `target_id` — a Library, Class,
    /// or Instance object id. Passing an isolate id is an RPC error
    /// (code 113); use [`DebugSession::evaluate_in_root_lib`] for the
    /// common "evaluate in the isolate's root library" case.
    ///
    /// # Errors
    ///
    /// [`Error::Rpc`] code 113 when the expression fails to compile or
    /// `target_id` is not a Library/Class/Instance id.
    pub fn evaluate(
        &mut self,
        isolate: Option<&str>,
        target_id: &str,
        expression: &str,
    ) -> Result<VmObject> {
        let isolate = self.resolve_isolate(isolate)?;
        let value = self.client.call(
            "evaluate",
            json!({
                "isolateId": isolate,
                "targetId": target_id,
                "expression": expression,
            }),
            self.config.rpc_timeout,
        )?;
        Ok(VmObject::from_value(value))
    }

    /// Evaluates `expression` in the isolate's root library — the
    /// convenience form of [`DebugSession::evaluate`] that resolves the
    /// library target for the caller. The root-library id is fetched
    /// once via `getIsolate` and cached per isolate.
    ///
    /// # Errors
    ///
    /// [`Error::Protocol`] when the isolate reports no root library
    /// (an isolate that has not reached `Runnable` yet); otherwise see
    /// [`DebugSession::evaluate`].
    pub fn evaluate_in_root_lib(
        &mut self,
        isolate: Option<&str>,
        expression: &str,
    ) -> Result<VmObject> {
        let isolate_id = self.resolve_isolate(isolate)?;
        let target = match self.root_libs.get(&isolate_id) {
            Some(library_id) => library_id.clone(),
            None => {
                let fetched: crate::protocol::Isolate = self.client.call_typed(
                    "getIsolate",
                    json!({ "isolateId": isolate_id }),
                    self.config.rpc_timeout,
                )?;
                let library_id = fetched.root_lib.map(|library| library.id).ok_or_else(|| {
                    Error::Protocol(format!(
                        "isolate {isolate_id} reports no root library (not Runnable yet?)"
                    ))
                })?;
                self.root_libs.insert(isolate_id.clone(), library_id.clone());
                library_id
            }
        };
        self.evaluate(Some(&isolate_id), &target, expression)
    }

    /// Invokes a method on an instance/class/library.
    ///
    /// # Errors
    ///
    /// [`Error::Rpc`] code 113 on compilation failure;
    /// [`Error::Sentinel`] for expired/collected receivers.
    pub fn invoke(
        &mut self,
        isolate: Option<&str>,
        target_id: &str,
        selector: &str,
        argument_ids: &[String],
    ) -> Result<VmObject> {
        let isolate = self.resolve_isolate(isolate)?;
        let value = self.client.call(
            "invoke",
            json!({
                "isolateId": isolate,
                "targetId": target_id,
                "selector": selector,
                "argumentIds": argument_ids,
            }),
            self.config.rpc_timeout,
        )?;
        Ok(VmObject::from_value(value))
    }

    /// Fetches one object by handle (`getObject`) — the primitive
    /// under [`DebugSession::expand`].
    ///
    /// # Errors
    ///
    /// [`Error::Sentinel`] for expired/collected handles.
    pub fn get_object(&mut self, isolate: Option<&str>, handle: &str) -> Result<VmObject> {
        let isolate = self.resolve_isolate(isolate)?;
        let value = self.client.call(
            "getObject",
            json!({ "isolateId": isolate, "objectId": handle }),
            self.config.rpc_timeout,
        )?;
        match VmObject::from_value(value) {
            // A sentinel means the handle is dead — the caller needs a
            // structured failure, not an object placeholder.
            VmObject::Sentinel(sentinel) => Err(Error::Sentinel {
                kind: sentinel.kind.as_str().to_owned(),
                value_as_string: sentinel.value_as_string,
            }),
            object => Ok(object),
        }
    }

    /// Expands one level of an object handle: fields of a plain
    /// instance, entries of a list/set/map, bounded by
    /// [`ExpandLimits::max_children`]. Never recurses — callers walk
    /// with [`DebugSession::expand_graph`].
    ///
    /// # Errors
    ///
    /// [`Error::Sentinel`] for expired/collected handles;
    /// [`Error::Protocol`] when the object is not expandable.
    pub fn expand(&mut self, isolate: Option<&str>, handle: &str) -> Result<Vec<Variable>> {
        let object = self.get_object(isolate, handle)?;
        let VmObject::Instance(instance) = object else {
            return Err(Error::Protocol(format!(
                "handle {handle} is not an expandable instance"
            )));
        };
        Ok(children_of(&instance, self.config.expand_limits.max_children))
    }

    /// Walks the object graph from `handle` down to `max_depth` levels,
    /// width-limited at every level. The result is flat with dotted
    /// names (`this.cart.items[0]`) — an agent reads it without
    /// re-walking handles.
    ///
    /// # Errors
    ///
    /// Propagates the root [`DebugSession::get_object`] failure; deeper
    /// failures degrade to a `<error>` child row so one dead handle
    /// cannot fail the whole expansion.
    pub fn expand_graph(&mut self, isolate: Option<&str>, handle: &str) -> Result<Vec<Variable>> {
        let limits = self.config.expand_limits;
        let mut out = Vec::new();
        let mut level = self.expand(isolate, handle)?;
        let mut depth = 1;
        while !level.is_empty() {
            let mut next = Vec::new();
            for item in level {
                let child_handle = item.handle.clone();
                let name = item.name.clone();
                out.push(item);
                if depth >= limits.max_depth {
                    continue;
                }
                let Some(child_handle) = child_handle else {
                    continue;
                };
                match self.expand(isolate, &child_handle) {
                    Ok(children) => {
                        for mut child in children {
                            child.name = format!("{name}.{}", child.name);
                            next.push(child);
                        }
                    }
                    Err(error) => out.push(Variable {
                        name: format!("{name}.<error>"),
                        value: error.to_string(),
                        handle: None,
                    }),
                }
            }
            level = next;
            depth += 1;
        }
        Ok(out)
    }

    /// Hot reload (`reloadSources`). Returns whether the VM reported
    /// success.
    ///
    /// # Errors
    ///
    /// [`Error::Rpc`] codes 108/109/110 for reload-state conflicts.
    pub fn reload_sources(&mut self, isolate: Option<&str>, force: bool) -> Result<bool> {
        let isolate = self.resolve_isolate(isolate)?;
        let report: crate::protocol::ReloadReport = self.client.call_typed(
            "reloadSources",
            json!({ "isolateId": isolate, "force": force }),
            // Reloads of large apps legitimately outrun the ordinary
            // RPC timeout.
            self.config.rpc_timeout.max(Duration::from_secs(120)),
        )?;
        Ok(report.success)
    }

    /// Hot restart through the DDS-registered `hotRestart` service
    /// (registered by DDS/DWDS middleware, not by the bare VM service;
    /// a plain VM answers with an RPC error, surfaced unchanged).
    ///
    /// # Errors
    ///
    /// [`Error::Rpc`] when the service is not registered or the
    /// restart fails.
    pub fn hot_restart(&mut self) -> Result<()> {
        self.client.call_unit(
            "hotRestart",
            json!({}),
            self.config.rpc_timeout.max(Duration::from_secs(120)),
        )
    }

    /// Kills the launched process (no-op when attached) and closes the
    /// connection.
    ///
    /// # Errors
    ///
    /// Propagates an unexpected kill failure from the OS.
    pub fn terminate(&mut self) -> Result<()> {
        self.client.close();
        if let Some(mut child) = self.process.take() {
            match child.kill() {
                Ok(()) => {}
                // Already exited — the kill is a no-op.
                Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {}
                Err(error) => return Err(Error::Io(error)),
            }
            let _ = child.wait();
        }
        Ok(())
    }

    /// Records one event: ring entry, isolate table, breakpoint
    /// resolution, pause tracking, WriteEvent output capture.
    fn absorb_event(&mut self, stream: StreamEvent) {
        let stream_id = stream.stream_id.clone();
        let cursor = self.events.pushed();
        self.events.push(SessionEvent {
            cursor,
            stream_id: stream.stream_id,
            event: stream.event.clone(),
            // See `SessionEvent::raw` for the retention rule: modeled
            // kinds drop the raw JSON (the typed event already carries
            // it), unmodeled kinds keep it.
            raw: match stream.event.kind {
                EventKind::Other(_)
                | EventKind::Inspect
                | EventKind::Gc
                | EventKind::CpuSamples
                | EventKind::TimelineEvents => Some(stream.raw),
                _ => None,
            },
        });
        match stream.event.kind {
            EventKind::IsolateStart | EventKind::IsolateRunnable | EventKind::IsolateUpdate => {
                if let Some(reference) = stream.event.isolate {
                    self.note_isolate(reference);
                }
            }
            EventKind::IsolateExit => {
                if let Some(reference) = stream.event.isolate {
                    self.isolates.remove(&reference.id);
                    self.pause_events.remove(&reference.id);
                    self.auto_resumes.remove(&reference.id);
                    if self.main_isolate.as_deref() == Some(reference.id.as_str()) {
                        self.main_isolate = self
                            .isolates
                            .values()
                            .find(|isolate| !isolate.is_system_isolate)
                            .map(|isolate| isolate.id.clone());
                    }
                }
            }
            EventKind::BreakpointResolved | EventKind::BreakpointUpdated => {
                if let Some(breakpoint) = &stream.event.breakpoint
                    && let Some(handle) = self.breakpoints.get_mut(&breakpoint.id)
                {
                    handle.resolved = breakpoint.resolved;
                }
            }
            EventKind::BreakpointRemoved => {
                if let Some(breakpoint) = &stream.event.breakpoint {
                    self.breakpoints.remove(&breakpoint.id);
                }
            }
            EventKind::PauseStart
            | EventKind::PauseBreakpoint
            | EventKind::PauseInterrupted
            | EventKind::PauseException
            | EventKind::PauseExit
            | EventKind::PausePostRequest => {
                if let Some(reference) = &stream.event.isolate {
                    self.pause_events
                        .insert(reference.id.clone(), stream.event.clone());
                }
            }
            EventKind::Resume => {
                if let Some(reference) = &stream.event.isolate {
                    self.pause_events.remove(&reference.id);
                }
            }
            // Attach-mode stdout/stderr: the VM forwards process output
            // as base64 `WriteEvent`s on the Stdout/Stderr streams
            // (launch mode captures the same bytes from the child
            // pipes). The rings stay GLOBAL across isolates — identical
            // semantics to the pipe capture — because the VM multiplexes
            // all isolates onto one stream pair; per-isolate attribution
            // is available in `events_since` via `event.isolate` for
            // whoever needs it.
            EventKind::WriteEvent => {
                use base64::Engine as _;
                if let Some(bytes) = &stream.event.bytes
                    && let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(bytes)
                {
                    // Malformed base64 skips this event only — output
                    // capture is best-effort and must not fail the
                    // absorb.
                    match stream_id.as_str() {
                        "Stdout" => {
                            self.stdout.push(decoded);
                        }
                        "Stderr" => {
                            self.stderr.push(decoded);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    /// Records one isolate and elects the main isolate if none is set.
    fn note_isolate(&mut self, reference: IsolateRef) {
        if self.main_isolate.is_none() && !reference.is_system_isolate {
            self.main_isolate = Some(reference.id.clone());
        }
        self.isolates.insert(reference.id.clone(), reference);
    }

    /// Resolves the target isolate: the explicit id, else the main
    /// isolate.
    fn resolve_isolate(&self, isolate: Option<&str>) -> Result<String> {
        isolate
            .map(str::to_owned)
            .or_else(|| self.main_isolate.clone())
            .ok_or_else(|| {
                Error::Protocol("no isolate available yet (wait for an Isolate event)".into())
            })
    }

    /// Classifies a pause event: report it, or auto-resume it because
    /// every breakpoint at this position has a falsy/failing
    /// condition.
    fn classify_pause(&mut self, event: &Event) -> PauseOutcome {
        match &event.kind {
            EventKind::PauseStart => PauseOutcome::Report(StoppedReason::Start),
            EventKind::PauseException => PauseOutcome::Report(StoppedReason::Exception),
            EventKind::PauseInterrupted => PauseOutcome::Report(StoppedReason::Interrupted),
            EventKind::PauseExit => PauseOutcome::Report(StoppedReason::Exit),
            EventKind::PausePostRequest => PauseOutcome::Report(StoppedReason::PostRequest),
            EventKind::PauseBreakpoint => {
                let mut hits: Vec<String> = event
                    .pause_breakpoints
                    .iter()
                    .map(|breakpoint| breakpoint.id.clone())
                    .collect();
                if let Some(breakpoint) = &event.breakpoint
                    && !hits.contains(&breakpoint.id)
                {
                    hits.push(breakpoint.id.clone());
                }
                // A breakpoint with no condition stops unconditionally
                // (an untracked id — e.g. armed by another client —
                // breaks unconditionally too).
                let unconditional = hits.iter().any(|id| {
                    self.breakpoints
                        .get(id)
                        .is_none_or(|handle| handle.condition.is_none())
                });
                if unconditional || hits.is_empty() {
                    return PauseOutcome::Report(StoppedReason::Breakpoint {
                        breakpoints: hits,
                    });
                }
                // The condition must evaluate against the isolate that
                // actually paused — the event's own isolate ref — not
                // the session's default (main) isolate. A conditional
                // breakpoint hit on a worker isolate evaluated in the
                // main isolate either reads the wrong frame's locals
                // or fails outright (RPC 113).
                let isolate_id = event
                    .isolate
                    .as_ref()
                    .map(|reference| reference.id.clone())
                    .or_else(|| self.main_isolate.clone());
                // Collect the conditions once per pause, not once per
                // hit: the borrow on `self.breakpoints` ends here so
                // `condition_holds` (which calls into the client) can
                // take `&mut self`, and each condition string is cloned
                // exactly once regardless of how many hits share it.
                let conditions: Vec<String> = hits
                    .iter()
                    .filter_map(|id| {
                        self.breakpoints
                            .get(id)
                            .and_then(|handle| handle.condition.clone())
                    })
                    .collect();
                let Some(isolate_id) = isolate_id else {
                    // No isolate on the event and none elected yet —
                    // cannot evaluate, so treat the stop as real.
                    return PauseOutcome::Report(StoppedReason::Breakpoint {
                        breakpoints: hits,
                    });
                };
                // Every hit is conditional: any truthy condition stops.
                for condition in &conditions {
                    if self.condition_holds(&isolate_id, condition) {
                        return PauseOutcome::Report(StoppedReason::Breakpoint {
                            breakpoints: hits,
                        });
                    }
                }
                PauseOutcome::AutoResume
            }
            other => PauseOutcome::Report(StoppedReason::Other(other.clone())),
        }
    }

    /// Evaluates one breakpoint condition in the top frame of the
    /// isolate that actually paused; a `true` Bool or a non-empty
    /// String holds. Evaluation failures count as falsy — a condition
    /// that cannot be evaluated must not trap the program.
    fn condition_holds(&mut self, isolate: &str, condition: &str) -> bool {
        match self.evaluate_in_frame(Some(isolate), 0, condition) {
            Ok(VmObject::Instance(instance)) => match instance.kind {
                InstanceKind::Bool => instance.value_as_string.as_deref() == Some("true"),
                InstanceKind::String => instance
                    .value_as_string
                    .as_deref()
                    .is_some_and(|text| !text.is_empty()),
                InstanceKind::Int | InstanceKind::Double => instance
                    .value_as_string
                    .as_deref()
                    .and_then(|text| text.parse::<f64>().ok())
                    .is_some_and(|number| number != 0.0),
                InstanceKind::Null => false,
                _ => true,
            },
            Ok(_) | Err(_) => false,
        }
    }

    /// Builds the stopped snapshot: position, stack summary, top frame
    /// locals.
    fn snapshot(
        &mut self,
        isolate: &str,
        reason: StoppedReason,
        event: &Event,
    ) -> Result<StoppedSnapshot> {
        let limit = self.config.stack_summary_frames;
        let stack = self.stack(Some(isolate), limit)?;
        let frames = if stack.frames.is_empty() {
            stack.async_causal_frames
        } else {
            stack.frames
        };
        let top = frames.first();
        let location = top.map_or_else(
            || frame_location(event.top_frame.as_ref()),
            |frame| frame_location(Some(frame)),
        );
        let stack_summary = frames
            .iter()
            .map(|frame| FrameSummary {
                index: frame.index,
                function: frame.function.as_ref().and_then(|f| f.name.clone()),
                location: frame_location(Some(frame)),
            })
            .collect();
        let variables = top.map_or_else(Vec::new, |frame| {
            frame
                .vars
                .iter()
                .map(|variable| Variable {
                    name: variable.name.clone(),
                    value: render_value(variable.value.as_ref()),
                    handle: handle_of(variable.value.as_ref()),
                })
                .collect()
        });
        Ok(StoppedSnapshot {
            reason,
            isolate: isolate.to_owned(),
            location,
            stack: stack_summary,
            variables,
            at_async_suspension: event.at_async_suspension,
        })
    }
}

impl Drop for DebugSession {
    /// A dropped session must not leak the child process, the pipe
    /// readers, the connection thread, or the socket — under a batch
    /// workload running many sessions that leak is fatal. `terminate`
    /// is idempotent, so a session terminated by hand drops cleanly.
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}

/// What to do with a pause event.
enum PauseOutcome {
    /// Surface the stop to the caller.
    Report(StoppedReason),
    /// Falsy condition — resume without surfacing.
    AutoResume,
}

/// Which pipe a reader thread serves.
#[derive(Clone, Copy)]
enum PipeKind {
    Stdout,
    Stderr,
}

/// Waits for the service-URI banner while collecting pre-banner output
/// so nothing printed before the banner is lost.
fn await_banner(
    child: &mut Child,
    feed: &Receiver<PipeEvent>,
    timeout: Duration,
) -> Result<(String, Vec<Vec<u8>>, Vec<Vec<u8>>)> {
    let deadline = Instant::now() + timeout;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    loop {
        if let Some(status) = child.try_wait()? {
            return Err(Error::ProcessExited(status.code()));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(Error::LaunchTimeout);
        }
        match feed.recv_timeout(remaining.min(Duration::from_millis(100))) {
            Ok(PipeEvent::Banner(banner)) => return Ok((banner, stdout, stderr)),
            Ok(PipeEvent::Stdout(bytes)) => stdout.push(bytes),
            Ok(PipeEvent::Stderr(bytes)) => stderr.push(bytes),
            Ok(PipeEvent::Eof) => {}
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return Err(Error::LaunchTimeout);
            }
        }
    }
}

/// Reads one child pipe to EOF, forwarding chunks and the first
/// service-URI banner line. A full session queue drops chunks rather
/// than blocking the child — the ring is the bounded record, but the
/// pipe must keep draining or the child would block on write.
fn spawn_pipe_reader(
    mut pipe: impl std::io::Read + Send + 'static,
    kind: PipeKind,
    tx: crossbeam_channel::Sender<PipeEvent>,
) {
    let name = match kind {
        PipeKind::Stdout => "dartq-debug-stdout",
        PipeKind::Stderr => "dartq-debug-stderr",
    };
    let _ = std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let mut buffer = [0_u8; PIPE_READ_SIZE];
            let mut line = Vec::new();
            let mut banner_sent = false;
            loop {
                let read = match pipe.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read,
                };
                let chunk = &buffer[..read];
                if !banner_sent {
                    for byte in chunk {
                        if *byte == b'\n' {
                            if let Some(banner) = banner_in(&line) {
                                banner_sent = true;
                                let _ = tx.try_send(PipeEvent::Banner(banner));
                            }
                            line.clear();
                        } else {
                            line.push(*byte);
                        }
                    }
                }
                let event = match kind {
                    PipeKind::Stdout => PipeEvent::Stdout(chunk.to_vec()),
                    PipeKind::Stderr => PipeEvent::Stderr(chunk.to_vec()),
                };
                match tx.try_send(event) {
                    Ok(()) | Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {}
                }
            }
            let _ = tx.try_send(PipeEvent::Eof);
        });
}

/// Extracts the service URI from a banner line:
/// `The Dart VM service is listening on http://127.0.0.1:PORT/AUTH=/`.
fn banner_in(line: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(line).ok()?;
    let marker = "The Dart VM service is listening on ";
    let start = text.find(marker)? + marker.len();
    let uri = text[start..].split_whitespace().next()?;
    Some(uri.trim_end_matches('\r').to_owned())
}

/// Converts the announced http(s) service URI to its WebSocket form:
/// `http→ws`, `https→wss`, and `ws` appended to the path — the DDS
/// conversion (path segments, then `ws`).
fn service_uri_to_ws(uri: &str) -> Result<String> {
    let parsed = ServiceUri::parse(uri)?;
    let scheme = match parsed.scheme.as_str() {
        "http" | "ws" => "ws",
        "https" | "wss" => "wss",
        other => {
            return Err(Error::BannerParse(format!(
                "unexpected service URI scheme {other}"
            )));
        }
    };
    let path = parsed.path.trim_end_matches('/');
    Ok(format!(
        "{scheme}://{}:{}{path}/ws",
        parsed.host, parsed.port
    ))
}

/// The minimal URI decomposition the banner conversion needs
/// (`scheme://host:port/path`).
struct ServiceUri {
    scheme: String,
    host: String,
    port: u16,
    path: String,
}

impl ServiceUri {
    fn parse(text: &str) -> Result<Self> {
        let (scheme, rest) = text
            .split_once("://")
            .ok_or_else(|| Error::BannerParse(format!("no scheme in {text}")))?;
        let (authority, path) = rest
            .find('/')
            .map_or((rest, ""), |slash| (&rest[..slash], &rest[slash..]));
        let (host, port) = authority
            .rsplit_once(':')
            .ok_or_else(|| Error::BannerParse(format!("no port in service URI {text}")))?;
        let port = port
            .parse::<u16>()
            .map_err(|_| Error::BannerParse(format!("bad port in {text}")))?;
        Ok(Self {
            scheme: scheme.to_owned(),
            host: host.to_owned(),
            port,
            path: path.to_owned(),
        })
    }
}

/// Finds the `dart` executable: `DARTQ_DART_BIN` wins, then `PATH`.
fn find_dart_binary() -> Result<PathBuf> {
    if let Some(explicit) = std::env::var_os("DARTQ_DART_BIN") {
        let path = PathBuf::from(explicit);
        if is_executable(&path) {
            return Ok(path);
        }
        return Err(Error::DartBinaryNotFound(format!(
            "DARTQ_DART_BIN={} is not an executable file",
            path.display()
        )));
    }
    let Some(path_var) = std::env::var_os("PATH") else {
        return Err(Error::DartBinaryNotFound(
            "PATH is unset and DARTQ_DART_BIN is not set".into(),
        ));
    };
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(if cfg!(windows) { "dart.exe" } else { "dart" });
        if is_executable(&candidate) {
            return Ok(candidate);
        }
    }
    Err(Error::DartBinaryNotFound(
        "no `dart` executable on PATH (set DARTQ_DART_BIN to override)".into(),
    ))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Builds an output page from one ring.
fn output_batch(ring: &Ring<Vec<u8>>, cursor: u64) -> OutputBatch {
    let mut bytes = Vec::new();
    for chunk in ring.since(cursor) {
        bytes.extend_from_slice(chunk);
    }
    OutputBatch {
        bytes,
        next_cursor: ring.pushed(),
        dropped: ring.oldest_seq().saturating_sub(cursor),
    }
}

/// Children of one instance, width-limited.
fn children_of(instance: &Instance, max_children: usize) -> Vec<Variable> {
    if !instance.fields.is_empty() {
        return instance
            .fields
            .iter()
            .take(max_children)
            .map(|field| Variable {
                name: field_name(field),
                value: render_value(field.value.as_ref()),
                handle: handle_of(field.value.as_ref()),
            })
            .collect();
    }
    if !instance.associations.is_empty() {
        let mut children = Vec::new();
        for (index, association) in instance.associations.iter().take(max_children).enumerate() {
            children.push(Variable {
                name: format!("[{index}].key"),
                value: render_value(association.key.as_ref()),
                handle: handle_of(association.key.as_ref()),
            });
            children.push(Variable {
                name: format!("[{index}].value"),
                value: render_value(association.value.as_ref()),
                handle: handle_of(association.value.as_ref()),
            });
        }
        return children;
    }
    instance
        .elements
        .iter()
        .take(max_children)
        .enumerate()
        .map(|(index, element)| Variable {
            name: format!("[{index}]"),
            value: render_value(Some(element)),
            handle: handle_of(Some(element)),
        })
        .collect()
}

/// A field's display name: `name` is a string for ordinary fields and
/// an integer for record positional fields; `decl` is the deprecated
/// fallback.
fn field_name(field: &BoundField) -> String {
    match &field.name {
        Some(Value::String(text)) => return text.clone(),
        Some(Value::Number(number)) => return format!("[{number}]"),
        _ => {}
    }
    field
        .decl
        .as_ref()
        .and_then(|decl| decl.get("name"))
        .and_then(Value::as_str)
        .map_or_else(|| "<field>".to_owned(), str::to_owned)
}

/// A short human/agent-readable rendering of one value slot.
fn render_value(value: Option<&ValueOrSentinel>) -> String {
    match value {
        Some(ValueOrSentinel::Instance(instance)) => render_instance(instance),
        Some(ValueOrSentinel::Sentinel(sentinel)) => {
            sentinel.value_as_string.clone().unwrap_or_else(|| {
                let kind = match &sentinel.kind {
                    SentinelKind::Collected => "collected",
                    SentinelKind::Expired => "expired",
                    SentinelKind::NotInitialized => "not initialized",
                    SentinelKind::BeingInitialized => "being initialized",
                    SentinelKind::OptimizedOut => "optimized out",
                    SentinelKind::Free => "free",
                    SentinelKind::Other(kind) => kind.as_str(),
                };
                format!("<{kind}>")
            })
        }
        Some(ValueOrSentinel::Other(_)) | None => "<unknown>".to_owned(),
    }
}

fn render_instance(instance: &Instance) -> String {
    if let Some(text) = &instance.value_as_string {
        return text.clone();
    }
    let class = instance
        .class
        .as_ref()
        .and_then(|class| class.name.clone())
        .unwrap_or_else(|| format!("{:?}", instance.kind));
    match instance.kind {
        InstanceKind::List | InstanceKind::Set => {
            format!("{class} (length {})", instance.length.unwrap_or(0))
        }
        InstanceKind::Map => format!("{class} ({} entries)", instance.length.unwrap_or(0)),
        InstanceKind::Null => "null".to_owned(),
        _ => format!("Instance of {class}"),
    }
}

/// The expandable handle of one value slot (the instance id).
fn handle_of(value: Option<&ValueOrSentinel>) -> Option<String> {
    match value {
        Some(ValueOrSentinel::Instance(instance)) if !instance.id.is_empty() => {
            Some(instance.id.clone())
        }
        _ => None,
    }
}

/// Position of a frame (or of a pause event's top frame when the stack
/// fetch came back empty).
fn frame_location(frame: Option<&Frame>) -> FrameLocation {
    let Some(frame) = frame else {
        return FrameLocation {
            line: None,
            column: None,
            script_uri: None,
            script_id: None,
            function: None,
        };
    };
    let location = frame.location.as_ref();
    FrameLocation {
        line: location.and_then(|location| location.line),
        column: location.and_then(|location| location.column),
        script_uri: location
            .and_then(|location| location.script.as_ref())
            .and_then(|script| script.uri.clone()),
        script_id: location
            .and_then(|location| location.script.as_ref())
            .map(|script| script.id.clone()),
        function: frame.function.as_ref().and_then(|f| f.name.clone()),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::time::{Duration, Instant};

    use super::{PipeEvent, PipeKind, banner_in, service_uri_to_ws, spawn_pipe_reader};

    const BANNER: &str = "The Dart VM service is listening on http://127.0.0.1:8181/AbC=/\n";

    #[test]
    fn banner_line_is_parsed() {
        assert_eq!(
            banner_in(BANNER.trim_end().as_bytes()).as_deref(),
            Some("http://127.0.0.1:8181/AbC=/")
        );
    }

    #[test]
    fn banner_with_carriage_return_is_parsed() {
        assert_eq!(
            banner_in(b"The Dart VM service is listening on http://127.0.0.1:1/x=/\r").as_deref(),
            Some("http://127.0.0.1:1/x=/")
        );
    }

    #[test]
    fn unrelated_lines_are_not_banners() {
        assert_eq!(banner_in(b"hello"), None);
        assert_eq!(banner_in(b""), None);
    }

    #[test]
    fn service_uri_converts_http_to_ws() {
        assert_eq!(
            service_uri_to_ws("http://127.0.0.1:8181/AbC=/").expect("convert"),
            "ws://127.0.0.1:8181/AbC=/ws"
        );
    }

    #[test]
    fn service_uri_converts_https_to_wss() {
        assert_eq!(
            service_uri_to_ws("https://example.test:443/auth=/").expect("convert"),
            "wss://example.test:443/auth=/ws"
        );
    }

    #[test]
    fn service_uri_rejects_unknown_scheme_and_missing_port() {
        assert!(service_uri_to_ws("ftp://127.0.0.1:21/x/").is_err());
        assert!(service_uri_to_ws("http://127.0.0.1/x/").is_err());
        assert!(service_uri_to_ws("127.0.0.1:8181/x/").is_err());
        assert!(service_uri_to_ws("http://127.0.0.1:notaport/x/").is_err());
    }

    #[test]
    fn pipe_reader_forwards_banner_chunks_and_eof() {
        let (tx, rx) = crossbeam_channel::bounded(8);
        let input = format!("{BANNER}hello\n").into_bytes();
        spawn_pipe_reader(Cursor::new(input), PipeKind::Stdout, tx);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut banner = None;
        let mut data = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(remaining) {
                Ok(PipeEvent::Banner(found)) => banner = Some(found),
                Ok(PipeEvent::Stdout(chunk)) => data.extend_from_slice(&chunk),
                Ok(PipeEvent::Eof) => break,
                Ok(PipeEvent::Stderr(_)) => {}
                Err(_) => break,
            }
        }
        assert_eq!(banner.as_deref(), Some("http://127.0.0.1:8181/AbC=/"));
        assert!(data.windows(b"hello".len()).any(|window| window == b"hello"));
    }
}
