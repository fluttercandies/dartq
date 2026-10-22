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
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
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
/// lifecycle, `Logging` `dart:developer.log` records.
const CONTROL_STREAMS: [&str; 3] = ["Debug", "Isolate", "Logging"];

/// Output streams an *attached* session subscribes to. A launched
/// session already owns the child's pipes; subscribing as well would
/// make the embedder forward every write a second time (the
/// `ServiceStreamListenCallback` turns capture on) and duplicate each
/// byte in the rings.
const OUTPUT_STREAMS: [&str; 2] = ["Stdout", "Stderr"];

/// Upper bound on one `WriteEvent` chunk stored in an output ring, so
/// the ring's byte ceiling (`capacity × chunk`) holds in attach mode
/// too — a single `print` of a huge string arrives as one event.
const WRITE_EVENT_CHUNK: usize = PIPE_READ_SIZE;

/// Longest line the pipe reader buffers while scanning for the
/// service banner; longer lines cannot be the banner and are dropped
/// from the scan (never from the captured output).
const BANNER_LINE_LIMIT: usize = 4096;

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
    /// Maximum rows `expand_graph` returns in total. Width and depth
    /// alone allow `max_children^max_depth` rows (one RPC each for the
    /// expandable ones); this budget keeps a wide graph to a bounded
    /// number of `getObject` round-trips.
    pub max_nodes: usize,
}

impl Default for ExpandLimits {
    fn default() -> Self {
        Self {
            max_children: 100,
            max_depth: 3,
            max_nodes: 500,
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
    /// Paused after a single step: the VM reports a step as a
    /// `PauseBreakpoint` with no breakpoints.
    Step,
    /// Paused at a conditional breakpoint whose condition failed to
    /// evaluate (it does not compile, or the RPC failed). The stop is
    /// surfaced so a broken condition cannot silently disarm the
    /// breakpoint.
    ConditionError {
        /// The VM breakpoint ids hit here.
        breakpoints: Vec<String>,
        /// The evaluation error.
        message: String,
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
    /// Breakpoints by `(isolate id, VM breakpoint id)`. Breakpoint ids
    /// are numbered per isolate (`breakpoints/1` exists in every
    /// isolate that has one), so the id alone is ambiguous.
    breakpoints: FxHashMap<(String, String), BreakpointHandle>,
    /// Most recent pause event per isolate, with its feed sequence
    /// number (used to ignore pauses that predate a resume).
    pause_events: FxHashMap<String, (u64, Event)>,
    /// Stream events ring (sequence == [`SessionEvent::cursor`]).
    events: Ring<SessionEvent>,
    /// Captured stdout chunks.
    stdout: Ring<Vec<u8>>,
    /// Captured stderr chunks.
    stderr: Ring<Vec<u8>>,
    /// Pipe reader feed (`None` when attached, not launched).
    pipe_feed: Option<Receiver<PipeEvent>>,
    /// Chunks the pipe readers dropped because the feed was full, per
    /// stream (`[stdout, stderr]`), and how many were already folded
    /// into the rings as skipped sequence numbers.
    pipe_drops: Option<Arc<[AtomicU64; 2]>>,
    folded_pipe_drops: [u64; 2],
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
        let drops = Arc::new([AtomicU64::new(0), AtomicU64::new(0)]);
        if let Some(stdout) = child.stdout.take() {
            spawn_pipe_reader(stdout, PipeKind::Stdout, tx.clone(), Arc::clone(&drops))?;
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_pipe_reader(stderr, PipeKind::Stderr, tx.clone(), Arc::clone(&drops))?;
        }
        drop(tx);

        // From here on any failure must not leak the spawned VM.
        let connected = await_banner(&mut child, &rx, config.launch_timeout).and_then(
            |(service_uri, early_stdout, early_stderr)| {
                let client = VmClient::connect(&service_uri_to_ws(&service_uri)?)?;
                Ok((client, early_stdout, early_stderr))
            },
        );
        let (client, early_stdout, early_stderr) = match connected {
            Ok(parts) => parts,
            Err(error) => {
                reap(&mut child);
                return Err(error);
            }
        };
        let mut session = Self::new(client, Some(child), settings);
        session.pipe_feed = Some(rx);
        session.pipe_drops = Some(drops);
        for chunk in early_stdout {
            session.stdout.push(chunk);
        }
        for chunk in early_stderr {
            session.stderr.push(chunk);
        }
        // On failure `session` drops here, and its Drop reaps the child.
        session.handshake(false)?;
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
        let mut session = Self::new(client, None, settings);
        session.handshake(true)?;
        Ok(session)
    }

    fn new(client: VmClient, process: Option<Child>, settings: SessionConfig) -> Self {
        Self {
            client,
            process,
            events: Ring::new(settings.event_capacity),
            stdout: Ring::new(settings.output_capacity),
            stderr: Ring::new(settings.output_capacity),
            config: settings,
            isolates: FxHashMap::default(),
            main_isolate: None,
            breakpoints: FxHashMap::default(),
            pause_events: FxHashMap::default(),
            pipe_feed: None,
            pipe_drops: None,
            folded_pipe_drops: [0; 2],
            auto_resumes: FxHashMap::default(),
            root_libs: FxHashMap::default(),
        }
    }

    /// Negotiates the protocol version, subscribes the session
    /// streams, seeds isolate tracking from `getVM`, and seeds each
    /// isolate's current pause state — an isolate already paused when
    /// the session attaches never re-sends its pause event.
    fn handshake(&mut self, output_streams: bool) -> Result<()> {
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
        let outputs: &[&str] = if output_streams { &OUTPUT_STREAMS } else { &[] };
        for stream in CONTROL_STREAMS.iter().chain(outputs) {
            match self.client.stream_listen(stream) {
                Ok(()) | Err(Error::Rpc { code: 103, .. }) => {}
                Err(error) => return Err(error),
            }
        }
        let vm = self.client.get_vm()?;
        // System isolates are tracked for completeness but are not
        // debug targets; only user isolates get their pause state.
        for isolate in vm.system_isolates {
            self.note_isolate(isolate);
        }
        for isolate in vm.isolates {
            let id = isolate.id.clone();
            self.note_isolate(isolate);
            self.seed_pause_state(&id)?;
        }
        Ok(())
    }

    /// Records the isolate's current pause event (`getIsolate`'s
    /// `pauseEvent`) unless a live event already superseded it. A
    /// collected isolate is simply forgotten.
    fn seed_pause_state(&mut self, isolate: &str) -> Result<()> {
        let fetched: crate::protocol::Isolate = match self.client.call_typed(
            "getIsolate",
            json!({ "isolateId": isolate }),
            self.config.rpc_timeout,
        ) {
            Ok(fetched) => fetched,
            Err(Error::Sentinel { .. }) => {
                self.forget_isolate(isolate);
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        if let Some(root) = fetched.root_lib {
            self.root_libs.insert(isolate.to_owned(), root.id);
        }
        if let Some(event) = fetched.pause_event
            && is_pause_kind(&event.kind)
            && !self.pause_events.contains_key(isolate)
        {
            // Sequence 0: a seeded pause predates every live event.
            self.pause_events.insert(isolate.to_owned(), (0, event));
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
    /// Adding the same location twice returns the same VM breakpoint;
    /// the session keeps one handle per breakpoint and the latest
    /// condition wins.
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
            id: breakpoint.id,
            isolate,
            condition: spec.condition,
            resolved: breakpoint.resolved,
        };
        self.breakpoints.insert(
            (handle.isolate.clone(), handle.id.clone()),
            handle.clone(),
        );
        Ok(handle)
    }

    /// Removes a breakpoint by its VM id from `isolate` (`None` means
    /// the main isolate). Removing an id the session does not track is
    /// a no-op; a breakpoint whose isolate already exited is forgotten
    /// without error (the VM dropped it with the isolate).
    ///
    /// # Errors
    ///
    /// [`Error::Rpc`] when the VM rejects the removal — the handle is
    /// kept, so the caller can retry.
    pub fn remove_breakpoint(&mut self, isolate: Option<&str>, breakpoint_id: &str) -> Result<()> {
        let isolate = self.resolve_isolate(isolate)?;
        let key = (isolate, breakpoint_id.to_owned());
        if !self.breakpoints.contains_key(&key) {
            return Ok(());
        }
        match self.client.call_unit(
            "removeBreakpoint",
            json!({ "isolateId": key.0, "breakpointId": key.1 }),
            self.config.rpc_timeout,
        ) {
            Ok(()) | Err(Error::Sentinel { .. }) => {
                self.breakpoints.remove(&key);
                Ok(())
            }
            Err(error) => Err(error),
        }
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
        // `setIsolatePauseMode` (protocol 3.53); prefer the forward
        // RPC and fall back only for VMs that predate it (-32601
        // "method not found").
        match self.client.call_unit(
            "setIsolatePauseMode",
            json!({ "isolateId": isolate, "exceptionPauseMode": mode }),
            self.config.rpc_timeout,
        ) {
            Err(Error::Rpc { code: -32601, .. }) => self.client.call_unit(
                "setExceptionPauseMode",
                json!({ "isolateId": isolate, "mode": mode }),
                self.config.rpc_timeout,
            ),
            other => other,
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
    /// [`Error::Rpc`] code 106 when the isolate is not paused;
    /// [`Error::Timeout`] when no pause follows within the configured
    /// pause timeout; [`Error::IsolateExited`] when the isolate exits.
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
        let mark = self.resume_marked(&isolate, params)?;
        self.wait_for_pause_after(&isolate, mark)
    }

    /// Resumes without waiting for the next stop (fire-and-forget
    /// runs).
    ///
    /// # Errors
    ///
    /// [`Error::Sentinel`] when the isolate is gone; transport errors.
    pub fn resume(&mut self, isolate: Option<&str>) -> Result<()> {
        let isolate = self.resolve_isolate(isolate)?;
        self.resume_marked(&isolate, json!({ "isolateId": isolate }))
            .map(|_| ())
    }

    /// Issues `resume` and retires the isolate's pause record. Returns
    /// the feed watermark taken just before the request went out:
    /// every event delivered up to then has been folded, so a pause
    /// record at or above the mark is a stop that follows this resume.
    /// The watermark is taken *before* sending because the VM (and DDS
    /// in front of it) gives no ordering guarantee between a response
    /// and the events the request causes.
    fn resume_marked(&mut self, isolate: &str, params: Value) -> Result<u64> {
        self.poll_events()?;
        self.pause_events.remove(isolate);
        let mark = self.events.pushed();
        let value = self.client.call("resume", params, self.config.rpc_timeout)?;
        check_success(&value)?;
        Ok(mark)
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

    /// Waits until `isolate` is paused, then builds the snapshot. An
    /// isolate that is already paused (and not yet resumed through
    /// this session) reports immediately.
    ///
    /// Conditional breakpoints resolve here: a falsy condition resumes
    /// and loops, bounded by the auto-resume ceiling.
    ///
    /// # Errors
    ///
    /// [`Error::Timeout`] when no pause arrives within the configured
    /// pause timeout; [`Error::IsolateExited`] when the isolate exits
    /// first; [`Error::ProcessExited`] when the launched process dies;
    /// [`Error::Closed`] when the connection drops.
    pub fn wait_for_pause(&mut self, isolate: &str) -> Result<StoppedSnapshot> {
        self.wait_for_pause_after(isolate, 0)
    }

    /// The wait loop shared by [`DebugSession::wait_for_pause`] and the
    /// resume paths: only pause records with sequence `>= mark` count.
    ///
    /// The wait is event-driven: after each drain the session blocks
    /// on the client's bounded channel for up to 2 ms (or the
    /// remaining deadline, whichever is shorter), so a pause wakes the
    /// waiter as soon as the connection thread delivers it while the
    /// deadline and process liveness are still re-checked promptly.
    fn wait_for_pause_after(&mut self, isolate: &str, mut mark: u64) -> Result<StoppedSnapshot> {
        /// Upper bound on one blocking wait.
        const WAKE_QUANTUM: Duration = Duration::from_millis(2);
        let deadline = Instant::now() + self.config.pause_timeout;
        loop {
            self.poll_events()?;
            if let Some((seq, event)) = self.pause_events.get(isolate)
                && *seq >= mark
            {
                let event = event.clone();
                match self.classify_pause(isolate, &event) {
                    PauseOutcome::Report(reason) => {
                        self.auto_resumes.remove(isolate);
                        return self.snapshot(isolate, reason, &event);
                    }
                    PauseOutcome::AutoResume => {
                        let limit = self.config.conditional_resume_limit.max_auto_resumes;
                        let consecutive = self.auto_resumes.entry(isolate.to_owned()).or_insert(0);
                        *consecutive += 1;
                        if *consecutive > limit {
                            let reason = StoppedReason::ConditionalResumeLimit {
                                auto_resumes: *consecutive,
                            };
                            self.auto_resumes.remove(isolate);
                            return self.snapshot(isolate, reason, &event);
                        }
                        mark = self.resume_marked(isolate, json!({ "isolateId": isolate }))?;
                        continue;
                    }
                }
            }
            if !self.isolates.contains_key(isolate) {
                return Err(Error::IsolateExited(isolate.to_owned()));
            }
            if let Some(child) = &mut self.process
                && let Some(status) = child.try_wait()?
            {
                // Fold whatever the exiting process still had in
                // flight before reporting.
                self.poll_events()?;
                return Err(Error::ProcessExited(status.code()));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::Timeout(format!(
                    "isolate {isolate} did not pause within {:?}",
                    self.config.pause_timeout
                )));
            }
            match self.client.next_event(remaining.min(WAKE_QUANTUM)) {
                Ok(Some(event)) => self.absorb_event(event),
                Ok(None) => {}
                // Launched: the VM may be mid-exit — the liveness check
                // above reports it within one quantum. The channel never
                // re-opens, so sleep rather than spin.
                Err(Error::Closed) if self.process.is_some() => {
                    std::thread::sleep(remaining.min(WAKE_QUANTUM));
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Drains the connection's event feed into the session ring and
    /// updates isolate/breakpoint/pause state; pipe output feeds the
    /// stdout/stderr rings. Every lost event or output chunk is folded
    /// in as a skipped sequence number, so cursors stay exact.
    ///
    /// # Errors
    ///
    /// [`Error::Closed`] once an attached session's connection is down
    /// and every event it delivered has been drained. A launched
    /// session keeps draining its pipes after the VM exits, so it
    /// reports process death through [`DebugSession::wait_for_pause`]
    /// instead.
    pub fn poll_events(&mut self) -> Result<()> {
        // Read the settle mark *before* draining: every event below it
        // is then either drained below or was dropped by the feed.
        let settled = self.client.settled_events();
        let closed = loop {
            match self.client.next_event(Duration::ZERO) {
                Ok(Some(event)) => self.absorb_event(event),
                Ok(None) => break false,
                Err(Error::Closed) => break true,
                Err(error) => return Err(error),
            }
        };
        if settled > self.events.pushed() {
            self.events.skip(settled - self.events.pushed());
        }
        self.drain_pipes();
        if closed && self.process.is_none() {
            return Err(Error::Closed);
        }
        Ok(())
    }

    /// Moves pipe output into the rings and folds reader-side drops in
    /// as skipped chunk sequence numbers.
    fn drain_pipes(&mut self) {
        if let Some(feed) = &self.pipe_feed {
            while let Ok(pipe) = feed.try_recv() {
                match pipe {
                    PipeEvent::Stdout(bytes) => {
                        self.stdout.push(bytes);
                    }
                    PipeEvent::Stderr(bytes) => {
                        self.stderr.push(bytes);
                    }
                    // The launch banner was consumed during launch.
                    PipeEvent::Banner(_) => {}
                }
            }
        }
        if let Some(drops) = &self.pipe_drops {
            for (index, ring) in [&mut self.stdout, &mut self.stderr].into_iter().enumerate() {
                let total = drops[index].load(Ordering::Relaxed);
                ring.skip(total - self.folded_pipe_drops[index]);
                self.folded_pipe_drops[index] = total;
            }
        }
    }

    /// Stream events at or after `cursor`, at most `limit` of them —
    /// a cursor-based pull an agent can page through without one call
    /// copying the whole ring.
    ///
    /// `dropped` counts the sequence numbers in the page's span
    /// (`cursor..next_cursor`) that can no longer be served: evicted
    /// from the ring or lost to a full feed. Pass `next_cursor` back to
    /// continue; `next_cursor == cursor` means nothing new.
    #[must_use]
    pub fn events_since(&self, cursor: u64, limit: usize) -> EventBatch {
        let mut events = Vec::new();
        let mut next_cursor = self.events.pushed();
        for (seq, event) in self.events.entries_since(cursor) {
            if events.len() == limit {
                next_cursor = seq;
                break;
            }
            events.push(event.clone());
        }
        let start = cursor.min(next_cursor);
        EventBatch {
            dropped: self.events.missing_in(start, next_cursor),
            events,
            next_cursor,
        }
    }

    /// Captured stdout at or after `cursor` (chunk sequence space), at
    /// most `max_bytes` (whole chunks; at least one chunk is returned
    /// when any is available).
    #[must_use]
    pub fn stdout_since(&self, cursor: u64, max_bytes: usize) -> OutputBatch {
        output_batch(&self.stdout, cursor, max_bytes)
    }

    /// Captured stderr at or after `cursor`; see
    /// [`DebugSession::stdout_since`].
    #[must_use]
    pub fn stderr_since(&self, cursor: u64, max_bytes: usize) -> OutputBatch {
        output_batch(&self.stderr, cursor, max_bytes)
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
        self.expand_page(isolate, handle, 0)
    }

    /// One page of an object's children starting at element `offset`
    /// (`List`/`Set`/`Map`/typed data). The VM is asked for at most
    /// [`ExpandLimits::max_children`] entries via `getObject`'s
    /// `offset`/`count`, so a million-element list costs one bounded
    /// response, not a full serialization. When more entries remain a
    /// final `[...]` row says so and carries the next offset in its
    /// value.
    ///
    /// # Errors
    ///
    /// [`Error::Sentinel`] for expired/collected handles;
    /// [`Error::Protocol`] when the object is not expandable.
    pub fn expand_page(
        &mut self,
        isolate: Option<&str>,
        handle: &str,
        offset: usize,
    ) -> Result<Vec<Variable>> {
        let isolate = self.resolve_isolate(isolate)?;
        let max_children = self.config.expand_limits.max_children;
        let value = self.client.call(
            "getObject",
            json!({
                "isolateId": isolate,
                "objectId": handle,
                "offset": offset,
                "count": max_children,
            }),
            self.config.rpc_timeout,
        )?;
        let instance = match VmObject::from_value(value) {
            VmObject::Instance(instance) => instance,
            VmObject::Sentinel(sentinel) => {
                return Err(Error::Sentinel {
                    kind: sentinel.kind.as_str().to_owned(),
                    value_as_string: sentinel.value_as_string,
                });
            }
            _ => {
                return Err(Error::Protocol(format!(
                    "handle {handle} is not an expandable instance"
                )));
            }
        };
        Ok(children_of(&instance, offset, max_children))
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
        let isolate = Some(self.resolve_isolate(isolate)?);
        let isolate = isolate.as_deref();
        let mut out = Vec::new();
        // Each object is expanded once: cyclic graphs (`a.next.prev`)
        // and shared nodes would otherwise re-fetch the same handle
        // `max_children^depth` times.
        let mut visited = rustc_hash::FxHashSet::default();
        visited.insert(handle.to_owned());
        let mut level = self.expand(isolate, handle)?;
        let mut depth = 1;
        while !level.is_empty() {
            let mut next = Vec::new();
            for item in level {
                if out.len() >= limits.max_nodes {
                    out.push(Variable {
                        name: "<truncated>".to_owned(),
                        value: format!("node budget of {} reached", limits.max_nodes),
                        handle: None,
                    });
                    return Ok(out);
                }
                let expand_child = depth < limits.max_depth
                    && item
                        .handle
                        .as_ref()
                        .is_some_and(|child| visited.insert(child.clone()));
                let child_handle = expand_child.then(|| item.handle.clone()).flatten();
                let name = item.name.clone();
                out.push(item);
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
                    // A primitive or opaque child is a leaf, not an
                    // error; only real failures get an error row.
                    Err(Error::Protocol(_)) => {}
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
        // A reload may replace the root library object.
        self.root_libs.clear();
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
            // Already exited: the kill is a no-op on every platform
            // once the child is reaped, and `wait` reaps it.
            if child.try_wait()?.is_none() {
                child.kill()?;
            }
            child.wait()?;
        }
        Ok(())
    }

    /// Records one event: ring entry, isolate table, breakpoint
    /// resolution, pause tracking, WriteEvent output capture.
    fn absorb_event(&mut self, stream: StreamEvent) {
        // Keep the ring's sequence space aligned with the wire: events
        // the bounded feed dropped become skipped numbers, so the hole
        // sits where the loss happened.
        if stream.seq > self.events.pushed() {
            self.events.skip(stream.seq - self.events.pushed());
        }
        let cursor = stream.seq;
        self.apply_event(&stream.stream_id, &stream.event, cursor);
        // See `SessionEvent::raw` for the retention rule: modeled kinds
        // drop the raw JSON (the typed event already carries it),
        // unmodeled kinds keep it. The typed event moves, never clones.
        let raw = match stream.event.kind {
            EventKind::Other(_)
            | EventKind::Inspect
            | EventKind::Gc
            | EventKind::CpuSamples
            | EventKind::TimelineEvents => Some(stream.raw),
            _ => None,
        };
        self.events.push(SessionEvent {
            cursor,
            stream_id: stream.stream_id,
            event: stream.event,
            raw,
        });
    }

    /// Applies one event to the session's derived state: isolate
    /// table, breakpoint resolution, pause tracking, WriteEvent output.
    fn apply_event(&mut self, stream_id: &str, event: &Event, seq: u64) {
        let isolate_id = event.isolate.as_ref().map(|isolate| isolate.id.as_str());
        match &event.kind {
            EventKind::IsolateStart | EventKind::IsolateRunnable | EventKind::IsolateUpdate => {
                if let Some(reference) = &event.isolate {
                    self.note_isolate(reference.clone());
                }
            }
            EventKind::IsolateExit => {
                if let Some(id) = isolate_id {
                    self.forget_isolate(&id.to_owned());
                }
            }
            EventKind::IsolateReload => {
                // Reload events are isolate-group scoped; every cached
                // root library may have been replaced.
                self.root_libs.clear();
            }
            EventKind::BreakpointAdded
            | EventKind::BreakpointResolved
            | EventKind::BreakpointUpdated => {
                if let (Some(id), Some(breakpoint)) = (isolate_id, &event.breakpoint)
                    && let Some(handle) = self
                        .breakpoints
                        .get_mut(&(id.to_owned(), breakpoint.id.clone()))
                {
                    handle.resolved = breakpoint.resolved;
                }
            }
            EventKind::BreakpointRemoved => {
                if let (Some(id), Some(breakpoint)) = (isolate_id, &event.breakpoint) {
                    self.breakpoints
                        .remove(&(id.to_owned(), breakpoint.id.clone()));
                }
            }
            kind if is_pause_kind(kind) => {
                if let Some(id) = isolate_id {
                    self.pause_events.insert(id.to_owned(), (seq, event.clone()));
                }
            }
            EventKind::Resume => {
                if let Some(id) = isolate_id {
                    self.pause_events.remove(id);
                }
            }
            // Attach-mode stdout/stderr: the VM forwards process output
            // as base64 `WriteEvent`s on the Stdout/Stderr streams. The
            // rings stay GLOBAL across isolates — identical semantics to
            // the pipe capture — because the VM multiplexes all
            // isolates onto one stream pair; per-isolate attribution is
            // available in `events_since` via `event.isolate`.
            EventKind::WriteEvent => {
                use base64::Engine as _;
                let ring = match stream_id {
                    "Stdout" => &mut self.stdout,
                    "Stderr" => &mut self.stderr,
                    _ => return,
                };
                let Some(bytes) = &event.bytes else { return };
                match base64::engine::general_purpose::STANDARD.decode(bytes) {
                    // Chunked so the ring's byte ceiling holds for a
                    // single huge write too.
                    Ok(decoded) => {
                        for chunk in decoded.chunks(WRITE_EVENT_CHUNK) {
                            ring.push(chunk.to_vec());
                        }
                    }
                    // Undecodable payload: record the loss as a gap in
                    // the output cursor space rather than dropping it
                    // silently.
                    Err(_) => ring.skip(1),
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

    /// Drops every piece of per-isolate state: the VM discards an
    /// exited isolate's breakpoints and objects, so keeping them would
    /// grow the session for every isolate a long-running VM spawns.
    fn forget_isolate(&mut self, id: &str) {
        self.isolates.remove(id);
        self.pause_events.remove(id);
        self.auto_resumes.remove(id);
        self.root_libs.remove(id);
        self.breakpoints.retain(|(isolate, _), _| isolate != id);
        if self.main_isolate.as_deref() == Some(id) {
            // Deterministic re-election: the lowest-numbered user
            // isolate (the VM numbers isolates in spawn order).
            self.main_isolate = self
                .isolates
                .values()
                .filter(|isolate| !isolate.is_system_isolate)
                .min_by(|a, b| isolate_order(a).cmp(&isolate_order(b)))
                .map(|isolate| isolate.id.clone());
        }
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

    /// Classifies a pause event of `isolate`: report it, or auto-resume
    /// it because every breakpoint at this position has a falsy
    /// condition.
    fn classify_pause(&mut self, isolate: &str, event: &Event) -> PauseOutcome {
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
                // Spec: "while single-stepping, the VM sends a
                // PauseBreakpoint event with no breakpoints".
                if hits.is_empty() {
                    return PauseOutcome::Report(StoppedReason::Step);
                }
                // Collect conditions once; an untracked id (armed by
                // another client) or an unconditional breakpoint stops
                // unconditionally.
                let mut conditions = Vec::with_capacity(hits.len());
                for id in &hits {
                    match self
                        .breakpoints
                        .get(&(isolate.to_owned(), id.clone()))
                        .and_then(|handle| handle.condition.clone())
                    {
                        Some(condition) => conditions.push(condition),
                        None => {
                            return PauseOutcome::Report(StoppedReason::Breakpoint {
                                breakpoints: hits,
                            });
                        }
                    }
                }
                // Every hit is conditional: any truthy condition stops;
                // a condition that cannot be evaluated stops too, with
                // the error, so a typo never silently disarms it.
                for condition in &conditions {
                    match self.condition_holds(isolate, condition) {
                        Ok(true) => {
                            return PauseOutcome::Report(StoppedReason::Breakpoint {
                                breakpoints: hits,
                            });
                        }
                        Ok(false) => {}
                        Err(message) => {
                            return PauseOutcome::Report(StoppedReason::ConditionError {
                                breakpoints: hits,
                                message,
                            });
                        }
                    }
                }
                PauseOutcome::AutoResume
            }
            other => PauseOutcome::Report(StoppedReason::Other(other.clone())),
        }
    }

    /// Evaluates one breakpoint condition in the top frame of the
    /// isolate that actually paused. Truthiness follows the Dart DAP
    /// adapter: `true`, or a non-zero `int`/`double`; everything else
    /// (including `null` and strings) is falsy. `Err` carries why the
    /// condition could not be evaluated.
    fn condition_holds(&mut self, isolate: &str, condition: &str) -> std::result::Result<bool, String> {
        match self.evaluate_in_frame(Some(isolate), 0, condition) {
            Ok(VmObject::Instance(instance)) => Ok(match instance.kind {
                InstanceKind::Bool => instance.value_as_string.as_deref() == Some("true"),
                InstanceKind::Int | InstanceKind::Double => instance
                    .value_as_string
                    .as_deref()
                    .and_then(|text| text.parse::<f64>().ok())
                    .is_some_and(|number| number != 0.0),
                _ => false,
            }),
            Ok(VmObject::Error { kind, message }) => Err(format!("{kind}: {message}")),
            Ok(VmObject::Sentinel(sentinel)) => Err(format!("sentinel {}", sentinel.kind.as_str())),
            Ok(VmObject::Other(_)) => Ok(false),
            Err(error) => Err(error.to_string()),
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
            // Pre-banner output is bounded like the rings: a child that
            // floods output without ever announcing the service keeps
            // only its latest chunks.
            Ok(PipeEvent::Stdout(bytes)) => push_bounded(&mut stdout, bytes),
            Ok(PipeEvent::Stderr(bytes)) => push_bounded(&mut stderr, bytes),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            // Both pipes closed without a banner: the child is exiting.
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                let status = child.wait()?;
                return Err(Error::ProcessExited(status.code()));
            }
        }
    }
}

/// Reads one child pipe to EOF, forwarding chunks and the first
/// service-URI banner line. A full session queue drops chunks rather
/// than blocking the child — the pipe must keep draining or the child
/// would block on write — and every dropped chunk is counted in
/// `drops[kind]` so the session can report the gap.
fn spawn_pipe_reader(
    mut pipe: impl std::io::Read + Send + 'static,
    kind: PipeKind,
    tx: crossbeam_channel::Sender<PipeEvent>,
    drops: Arc<[AtomicU64; 2]>,
) -> Result<()> {
    let (name, slot) = match kind {
        PipeKind::Stdout => ("dartq-debug-stdout", 0),
        PipeKind::Stderr => ("dartq-debug-stderr", 1),
    };
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let mut buffer = vec![0_u8; PIPE_READ_SIZE];
            let mut line = Vec::new();
            let mut banner_pending = true;
            loop {
                let read = match pipe.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => read,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                };
                let chunk = &buffer[..read];
                if banner_pending {
                    for &byte in chunk {
                        if byte == b'\n' {
                            if let Some(banner) = banner_in(&line) {
                                banner_pending = false;
                                // The banner must not be lost to a full
                                // queue: block for it (once per pipe).
                                let _ = tx.send(PipeEvent::Banner(banner));
                                break;
                            }
                            line.clear();
                        } else if line.len() < BANNER_LINE_LIMIT {
                            line.push(byte);
                        }
                    }
                }
                let event = match kind {
                    PipeKind::Stdout => PipeEvent::Stdout(chunk.to_vec()),
                    PipeKind::Stderr => PipeEvent::Stderr(chunk.to_vec()),
                };
                match tx.try_send(event) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {
                        drops[slot].fetch_add(1, Ordering::Relaxed);
                    }
                    // Session gone: keep draining so the child never
                    // blocks on a full pipe while it is being killed.
                    Err(TrySendError::Disconnected(_)) => {}
                }
            }
        })
        .map(|_| ())
        .map_err(Error::Io)
}

/// Appends a pre-banner chunk, evicting the oldest past the ring cap.
fn push_bounded(chunks: &mut Vec<Vec<u8>>, chunk: Vec<u8>) {
    if chunks.len() == OUTPUT_CHUNK_CAPACITY {
        chunks.remove(0);
    }
    chunks.push(chunk);
}

/// Kills and reaps a child whose session could not be established.
fn reap(child: &mut Child) {
    // Best effort: the launch error is what the caller needs; a kill
    // failure here means the child is already gone.
    let _ = child.kill();
    let _ = child.wait();
}

/// Whether `kind` is one of the pause kinds a session tracks.
fn is_pause_kind(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::PauseStart
            | EventKind::PauseBreakpoint
            | EventKind::PauseInterrupted
            | EventKind::PauseException
            | EventKind::PauseExit
            | EventKind::PausePostRequest
    )
}

/// Ordering key for main-isolate re-election: the VM's numeric isolate
/// number, then the id for isolates without one.
fn isolate_order(isolate: &IsolateRef) -> (u64, &str) {
    let number = isolate
        .number
        .as_deref()
        .and_then(|text| text.parse().ok())
        .unwrap_or(u64::MAX);
    (number, isolate.id.as_str())
}

/// Rejects a sentinel answer to an RPC that should return `Success`.
fn check_success(value: &Value) -> Result<()> {
    if value.get("type").and_then(Value::as_str) == Some("Sentinel") {
        return Err(Error::Sentinel {
            kind: value
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("Unknown")
                .to_owned(),
            value_as_string: value
                .get("valueAsString")
                .and_then(Value::as_str)
                .map(str::to_owned),
        });
    }
    Ok(())
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
fn output_batch(ring: &Ring<Vec<u8>>, cursor: u64, max_bytes: usize) -> OutputBatch {
    let mut bytes = Vec::new();
    let mut next_cursor = ring.pushed();
    for (seq, chunk) in ring.entries_since(cursor) {
        if !bytes.is_empty() && bytes.len() + chunk.len() > max_bytes {
            next_cursor = seq;
            break;
        }
        bytes.extend_from_slice(chunk);
    }
    OutputBatch {
        bytes,
        dropped: ring.missing_in(cursor.min(next_cursor), next_cursor),
        next_cursor,
    }
}

/// Children of one instance, width-limited. `offset` is the element
/// index the VM page starts at (it echoes it back as `offset`), so
/// element rows carry absolute indices. When the instance has more
/// entries than were returned, a final `[...]` row reports the next
/// offset to pass to [`DebugSession::expand_page`].
fn children_of(instance: &Instance, offset: usize, max_children: usize) -> Vec<Variable> {
    if !instance.fields.is_empty() {
        let mut children: Vec<Variable> = instance
            .fields
            .iter()
            .take(max_children)
            .map(|field| Variable {
                name: field_name(field),
                value: render_value(field.value.as_ref()),
                handle: handle_of(field.value.as_ref()),
            })
            .collect();
        if instance.fields.len() > max_children {
            children.push(more_row(instance.fields.len() - max_children, None));
        }
        return children;
    }
    let mut children = Vec::new();
    let shown = if !instance.associations.is_empty() {
        let shown = instance.associations.len().min(max_children);
        for (index, association) in instance.associations.iter().take(shown).enumerate() {
            let index = offset + index;
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
        shown
    } else {
        let shown = instance.elements.len().min(max_children);
        children.extend(instance.elements.iter().take(shown).enumerate().map(
            |(index, element)| Variable {
                name: format!("[{}]", offset + index),
                value: render_value(Some(element)),
                handle: handle_of(Some(element)),
            },
        ));
        shown
    };
    // `length` is the full collection size; the page ends at
    // `offset + shown`.
    let total = instance
        .length
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or(offset + shown);
    let next = offset + shown;
    if shown > 0 && total > next {
        children.push(more_row(total - next, Some(next)));
    }
    children
}

/// The trailing "more entries" row of a truncated expansion.
fn more_row(remaining: usize, next_offset: Option<usize>) -> Variable {
    Variable {
        name: "[...]".to_owned(),
        value: next_offset.map_or_else(
            || format!("{remaining} more"),
            |next| format!("{remaining} more (next offset {next})"),
        ),
        handle: None,
    }
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
        let drops = std::sync::Arc::new([
            std::sync::atomic::AtomicU64::new(0),
            std::sync::atomic::AtomicU64::new(0),
        ]);
        spawn_pipe_reader(Cursor::new(input), PipeKind::Stdout, tx, drops).expect("spawn reader");
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut banner = None;
        let mut data = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(remaining) {
                Ok(PipeEvent::Banner(found)) => banner = Some(found),
                Ok(PipeEvent::Stdout(chunk)) => data.extend_from_slice(&chunk),
                Ok(PipeEvent::Stderr(_)) => {}
                // The reader thread exits at EOF and drops its sender.
                Err(_) => break,
            }
        }
        assert_eq!(banner.as_deref(), Some("http://127.0.0.1:8181/AbC=/"));
        assert!(data.windows(b"hello".len()).any(|window| window == b"hello"));
    }
}
