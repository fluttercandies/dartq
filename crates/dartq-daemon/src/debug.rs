//! The `dartq/debug/*` face: launch/attach debugging sessions over the
//! daemon's RPC surface and drive them — breakpoints, stepping, stopped
//! snapshots with variables, evaluation, hot reload/restart, and
//! cursor-paged event/output reads. Launches run `dart` scripts or
//! `flutter run` apps (Flutter is the primary target platform); attach
//! takes any live VM service URI, including one owned by a running
//! `flutter run`.
//!
//! Every session-bearing face takes an optional `session_id`: absent
//! drives the sole live session (the common shape — one interaction,
//! one session); zero live sessions and several live sessions are
//! machine-coded domain misses, the several case listing every
//! candidate (`#id kind script`) so the caller can pick.
//!
//! The face is a registry problem, not an analysis problem: every request
//! targets one of the session's live [`DebugManager`] entries and never
//! touches the analysis engine. The registry lives on a dedicated worker
//! thread ([`DebugDispatch`], #2587): the server hands each debug request
//! to the worker at intake and the answer arrives on the completion
//! channel — a slow or stuck debug op never holds the main loop, and
//! every other face keeps answering. Every underlying wait is bounded by
//! dartq-debug's `SessionConfig` (connect, handshake, per-RPC, pause).
//!
//! dartq-debug's session types carry no serde derives by design (the
//! crate keeps its wire face at the VM protocol level), so this module
//! owns the daemon-side DTO shapers — pure functions over the session
//! types, unit-testable without a live VM. The protocol-level types it
//! re-exports (`Event` and friends) are serde-derived and embed directly.
//!
//! #2619: a held debug session must not evaporate with the daemon. Every
//! opened session leaves an orphan manifest under the session layout's
//! `debug/` directory (the re-attach trail a later daemon's `status`
//! lists), and the serve loop's exit paths drive the internal
//! [`DEBUG_SHUTDOWN`] face — every held session is detached (its paused
//! isolates resumed) before the daemon dies.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};
use dartq_debug::{
    BreakpointHandle, BreakpointLocationSpec, BreakpointSpec, DebugSession, EventBatch,
    ExceptionPauseMode, FrameLocation, FrameSummary, LaunchConfig, LaunchKind, OutputBatch,
    RELEASE_RESUME_BUDGET, RestartOutcome, SessionEvent, StepOption, StoppedReason,
    StoppedSnapshot, Variable, VmObject,
};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::jsonrpc::{RequestId, Response, codes};

/// The `dartq/debug/*` method names — the debug family's closed request
/// set. `pub` so the drift gate (tests/rpc_schema.rs) can pin the served
/// family as one fact with the dispatch table below.
pub const DEBUG_LAUNCH: &str = "dartq/debug/launch";
pub const DEBUG_ATTACH: &str = "dartq/debug/attach";
pub const DEBUG_DETACH: &str = "dartq/debug/detach";
pub const DEBUG_STOP: &str = "dartq/debug/stop";
pub const DEBUG_STATUS: &str = "dartq/debug/status";
pub const DEBUG_BREAKPOINT_ADD: &str = "dartq/debug/breakpointAdd";
pub const DEBUG_BREAKPOINT_REMOVE: &str = "dartq/debug/breakpointRemove";
pub const DEBUG_EXCEPTION_PAUSE: &str = "dartq/debug/exceptionPause";
pub const DEBUG_RESUME: &str = "dartq/debug/resume";
pub const DEBUG_PAUSE: &str = "dartq/debug/pause";
pub const DEBUG_STEP: &str = "dartq/debug/step";
pub const DEBUG_SNAPSHOT: &str = "dartq/debug/snapshot";
pub const DEBUG_RELOAD: &str = "dartq/debug/reload";
pub const DEBUG_RESTART: &str = "dartq/debug/restart";
pub const DEBUG_EVALUATE: &str = "dartq/debug/evaluate";
pub const DEBUG_EVENTS: &str = "dartq/debug/events";
pub const DEBUG_OUTPUT: &str = "dartq/debug/output";

/// Every `dartq/debug/*` request name — the family the server intercepts
/// at intake ahead of the analysis-command arm.
pub const DEBUG_REQUESTS: [&str; 17] = [
    DEBUG_LAUNCH,
    DEBUG_ATTACH,
    DEBUG_DETACH,
    DEBUG_STOP,
    DEBUG_STATUS,
    DEBUG_BREAKPOINT_ADD,
    DEBUG_BREAKPOINT_REMOVE,
    DEBUG_EXCEPTION_PAUSE,
    DEBUG_RESUME,
    DEBUG_PAUSE,
    DEBUG_STEP,
    DEBUG_SNAPSHOT,
    DEBUG_RELOAD,
    DEBUG_RESTART,
    DEBUG_EVALUATE,
    DEBUG_EVENTS,
    DEBUG_OUTPUT,
];

/// Whether `method` is one of the debug faces this module serves.
#[must_use]
pub(crate) fn is_request(method: &str) -> bool {
    DEBUG_REQUESTS.contains(&method)
}

/// The internal shutdown face (#2619): detach (resume) every live
/// session and mark each orphan manifest released. Deliberately NOT in
/// [`DEBUG_REQUESTS`] — `is_request` is the wire intake gate, so no
/// client can ever route a request here; only the serve loop's own exit
/// paths send it through the worker.
pub(crate) const DEBUG_SHUTDOWN: &str = "dartq/debug/internalShutdown";

/// The internal shutdown job's id — the serve loop's bounded wait picks
/// its own answer out of the completion channel by it (a straggler
/// answer from a disconnected client may sit ahead of it).
pub(crate) const SHUTDOWN_JOB_ID: &str = "debug-shutdown";

/// #2619: the election reap grace must strictly exceed one detach's
/// resume budget — a graceful shutdown's last act (resume every paused
/// isolate) must fit inside the grace before the reaper's SIGKILL
/// escalation lands. Compile-time tie: the two constants cannot drift
/// apart silently (both are whole seconds, so the `as_secs` compare is
/// exact).
const _: () = assert!(
    dartq_session::REAP_GRACE.as_secs() > RELEASE_RESUME_BUDGET.as_secs(),
    "REAP_GRACE must exceed RELEASE_RESUME_BUDGET"
);

/// #2632: the reaper's per-held-session grace share mirrors the release
/// budget exactly — the shutdown ladder is sequential, one budget per
/// held session, and the advertised-count arithmetic in
/// `dartq_session::reap_grace` must size the same budget this crate's
/// ladder spends. The compile-time tie keeps the mirror from drifting.
const _: () = assert!(
    dartq_session::REAP_GRACE_PER_HELD.as_secs() == RELEASE_RESUME_BUDGET.as_secs(),
    "REAP_GRACE_PER_HELD must equal RELEASE_RESUME_BUDGET"
);

/// A debug face failure, mapped to the wire by [`DebugError::wire`].
#[derive(Debug)]
pub(crate) enum DebugError {
    /// A structural parameter mismatch — the `INVALID_PARAMS` tier every
    /// other face uses.
    InvalidParams(String),
    /// The request named a session id that is not live.
    NoSuchSession(u32),
    /// The request named no session and none are live (the sole-session
    /// default has nothing to pick).
    NoSessions,
    /// The request named no session and several are live (the payload
    /// lists every candidate: id, kind, script or ws URI).
    AmbiguousSession(String),
    /// A frame-indexed `evaluate` named a frame the paused isolate
    /// does not have (its stack is empty — an isolate interrupted
    /// while idle).
    NoFrame(u32),
    /// The underlying debug session call failed (transport, VM RPC,
    /// process, or timeout — dartq-debug's typed failure set). Resource
    /// exhaustion (process table, fds, memory) surfaces here as the OS's
    /// own error, not a daemon-side quota — the registry itself is
    /// unbounded.
    Vm(dartq_debug::Error),
}

impl DebugError {
    /// The JSON-RPC `(code, message)` pair. Session-domain failures carry
    /// a `DARTQ/DEBUG/*` machine-code prefix in the message (there is no
    /// per-cause JSON-RPC code space for them; one server-error code
    /// keeps the family distinguishable from the analysis faces').
    pub(crate) fn wire(self) -> (i64, String) {
        match self {
            Self::InvalidParams(message) => (codes::INVALID_PARAMS, message),
            Self::NoSuchSession(id) => (
                codes::DEBUG_SESSION,
                format!("DARTQ/DEBUG/NO_SUCH_SESSION: session {id} is not live"),
            ),
            Self::NoSessions => (
                codes::DEBUG_SESSION,
                "DARTQ/DEBUG/NO_SESSIONS: no debug sessions are live — \
                 launch or attach one first"
                    .to_owned(),
            ),
            Self::AmbiguousSession(candidates) => (
                codes::DEBUG_SESSION,
                format!(
                    "DARTQ/DEBUG/AMBIGUOUS_SESSION: several debug sessions are live \
                     ({candidates}) — name one (CLI `-s <ID>`, MCP `session_id`)"
                ),
            ),
            Self::NoFrame(index) => (
                codes::DEBUG_SESSION,
                format!(
                    "DARTQ/DEBUG/NO_FRAME: the paused isolate has no frame at index {index}; \
                     pause at a breakpoint or evaluate in the root library"
                ),
            ),
            Self::Vm(error) => (codes::DEBUG_SESSION, format!("DARTQ/DEBUG/VM: {error}")),
        }
    }
}

/// How a managed session came to be — the `status` face's origin field.
enum Origin {
    /// A launched process (the entrypoint path as requested, plus the
    /// toolchain that runs it).
    Launch {
        /// The entrypoint script or Flutter target.
        script: PathBuf,
        /// The toolchain (`dart`/`flutter`).
        kind: LaunchKind,
    },
    /// An attached VM service WebSocket URI.
    Attach(String),
}

/// One registry entry: the origin plus the live session. Dropping the
/// entry drops the session, whose `Drop` kills and reaps the child and
/// closes the connection (dartq-debug's teardown contract).
struct ManagedSession {
    origin: Origin,
    session: DebugSession,
}

/// The session registry: monotonic ids (never reused within a daemon
/// life) and an unbounded live set — resource exhaustion (process table,
/// fds, memory) surfaces through `DebugSession::launch`/`attach`'s own
/// error rather than a daemon-side quota, so the ceiling is whatever
/// the host can actually run, not a hardcoded guess. Hangs off the
/// daemon [`Session`] until the first debug request, then lives on the
/// debug worker; the shared `live` counter it keeps in sync is the
/// session's idle probe (#2619 — an attached debug session means the
/// daemon is not idle) and sizes the shutdown wait window.
pub(crate) struct DebugManager {
    sessions: FxHashMap<u32, ManagedSession>,
    next_id: u32,
    live: Arc<AtomicUsize>,
}

impl DebugManager {
    /// The manager keeping `live` in sync with its registry — the
    /// session holds the other handle of the same counter.
    pub(crate) fn sharing(live: Arc<AtomicUsize>) -> Self {
        Self { sessions: FxHashMap::default(), next_id: 0, live }
    }

    /// Registers a live session under the next monotonic id.
    fn register(&mut self, origin: Origin, session: DebugSession) -> u32 {
        self.next_id += 1;
        self.sessions.insert(self.next_id, ManagedSession { origin, session });
        self.live.fetch_add(1, Ordering::Relaxed);
        self.next_id
    }

    /// Drops one session from the registry, keeping the live counter in
    /// step.
    fn remove(&mut self, id: u32) -> Option<ManagedSession> {
        let removed = self.sessions.remove(&id);
        if removed.is_some() {
            self.live.fetch_sub(1, Ordering::Relaxed);
        }
        removed
    }

    /// The named session, or the machine-coded miss.
    fn get(&mut self, id: u32) -> Result<&mut ManagedSession, DebugError> {
        self.sessions.get_mut(&id).ok_or(DebugError::NoSuchSession(id))
    }
}

// ---------------------------------------------------------------------------
// The orphan manifests (#2619)
// ---------------------------------------------------------------------------

/// One debug session's recovery manifest — the trail that outlives the
/// daemon that held the session. Written at launch/attach, deleted at a
/// client-requested detach/terminate, rewritten `released: true` at a
/// graceful daemon shutdown (the app was resumed and runs free). A later
/// daemon's `status` lists the survivors as `orphans` so an agent can
/// re-attach; a successful attach to the same `ws_uri` consumes them.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct SessionManifest {
    /// The VM service WebSocket URI — the re-attach address.
    ws_uri: String,
    /// `launch` or `attach` (how the session came to be).
    kind: String,
    /// When the session opened, UNIX seconds.
    since: u64,
    /// The launched program's pid (launches only; an attach owns no
    /// process).
    #[serde(skip_serializing_if = "Option::is_none")]
    pid: Option<u32>,
    /// `true` once a graceful daemon shutdown resumed the app and let
    /// it run free.
    #[serde(skip_serializing_if = "Option::is_none")]
    released: Option<bool>,
}

/// The manifest directory: `<session layout>/debug/`, one `<id>.json`
/// per session a daemon on this worktree opened. The files must survive
/// their daemon (that is their point), so they live beside the session
/// state, not inside any process.
fn manifests_dir(root: &Path) -> PathBuf {
    dartq_session::Layout::new(root).dir.join("debug")
}

/// Now, in UNIX seconds.
fn unix_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

/// Writes one session's manifest. Best-effort by design: the manifest
/// is recovery aid, never authoritative state — an unwritable tree only
/// means `status` lists one fewer orphan.
fn write_manifest(dir: &Path, id: u32, manifest: &SessionManifest) {
    let _ = std::fs::create_dir_all(dir);
    let bytes = serde_json::to_vec(manifest).unwrap_or_default();
    let _ = std::fs::write(dir.join(format!("{id}.json")), bytes);
}

/// Consumes the manifests of sessions now re-attached: deletes every
/// manifest naming `ws_uri` except `keep`'s own (just rewritten under
/// the fresh id). A new attach takes ownership of the program; the
/// stale trail must not keep listing it as a separate orphan.
fn consume_matching(dir: &Path, ws_uri: &str, keep: u32) {
    for (id, manifest) in load_manifests(dir) {
        if id != keep && manifest.ws_uri == ws_uri {
            delete_manifest(dir, id);
        }
    }
}

/// Deletes one session's manifest (a client-requested detach or
/// terminate — the client owns the trail's end).
fn delete_manifest(dir: &Path, id: u32) {
    let _ = std::fs::remove_file(dir.join(format!("{id}.json")));
}

/// Rewrites one manifest `released: true` (a graceful daemon shutdown
/// resumed the app; it runs free). Best-effort like every manifest
/// write; an unreadable manifest stays as it was.
fn mark_released(dir: &Path, id: u32) {
    if let Some(mut manifest) = load_manifests(dir)
        .into_iter()
        .find(|(manifest_id, _)| *manifest_id == id)
        .map(|(_, manifest)| manifest)
    {
        manifest.released = Some(true);
        write_manifest(dir, id, &manifest);
    }
}

/// Every readable manifest in `dir`, ids ascending. Malformed files are
/// skipped — a corrupt manifest is stale trail, not a failure.
fn load_manifests(dir: &Path) -> Vec<(u32, SessionManifest)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let Some(id) = entry
            .path()
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.parse::<u32>().ok())
        else {
            continue;
        };
        if let Ok(text) = std::fs::read_to_string(entry.path())
            && let Ok(manifest) = serde_json::from_str::<SessionManifest>(&text)
        {
            out.push((id, manifest));
        }
    }
    out.sort_by_key(|(id, _)| *id);
    out
}

/// One candidate line of the ambiguity listing: `#id kind script` for a
/// launch, `#id attach <ws_uri>` for an attach.
fn session_line(managed: &ManagedSession, id: u32) -> String {
    match &managed.origin {
        Origin::Launch { script, kind } => {
            format!("#{id} {} {}", launch_kind_word(*kind), script.display())
        }
        Origin::Attach(uri) => format!("#{id} attach {uri}"),
    }
}

/// Resolves the request's session selector: an explicit id names that
/// session; absent drives the sole live session — the common shape, and
/// the reason `session_id` is optional on every session-bearing face.
/// Zero live sessions and several live sessions are the machine-coded
/// domain misses (the several case lists every candidate so the caller
/// can pick).
fn select_session(manager: &mut DebugManager, requested: Option<u32>) -> Result<u32, DebugError> {
    match requested {
        Some(id) => Ok(id),
        None => {
            let mut ids: Vec<u32> = manager.sessions.keys().copied().collect();
            ids.sort_unstable();
            match ids[..] {
                [] => Err(DebugError::NoSessions),
                [only] => Ok(only),
                _ => {
                    let candidates = ids
                        .iter()
                        .map(|&id| session_line(&manager.sessions[&id], id))
                        .collect::<Vec<_>>()
                        .join("; ");
                    Err(DebugError::AmbiguousSession(candidates))
                }
            }
        }
    }
}

/// One queued debug request bound for the worker.
pub(crate) struct DebugJob {
    pub(crate) id: RequestId,
    pub(crate) method: String,
    pub(crate) params: Value,
    pub(crate) root: PathBuf,
}

/// How many debug requests may queue while the worker executes one —
/// and how many completed answers may wait for the main loop. A full
/// queue answers the structured busy error at intake instead of
/// blocking the sender.
const DEBUG_QUEUE_CAPACITY: usize = 16;

/// The debug faces' executor (#2587): a dedicated thread that owns the
/// [`DebugManager`], so a slow or stuck debug op — an attach against a
/// stalled DDS, a step waiting out its pause bound — never holds the
/// daemon's main loop, and every other face keeps answering. Requests
/// run sequentially in arrival order (debug traffic is inherently
/// sequential per session); answers ride the completion channel back
/// to the main loop, which owns the output. Every underlying wait is
/// bounded by dartq-debug's `SessionConfig`, so the worker always
/// returns to the queue; if it ever dies anyway (a bug), the
/// containment is the debug face alone — the sessions drop with it,
/// later debug requests answer the structured worker-gone error, and
/// a fresh request spawns a fresh worker.
pub(crate) struct DebugDispatch {
    requests: Sender<DebugJob>,
    completions: Receiver<Response>,
}

impl DebugDispatch {
    /// Spawns the worker over `manager`, taking ownership of it for
    /// the dispatch's lifetime (the session hands it over at the first
    /// debug request and never touches it again).
    pub(crate) fn spawn(manager: DebugManager) -> Self {
        let (requests, request_rx) = bounded::<DebugJob>(DEBUG_QUEUE_CAPACITY);
        let (completion_tx, completions) = bounded::<Response>(DEBUG_QUEUE_CAPACITY);
        thread::Builder::new()
            .name("dartq-debug-worker".into())
            .spawn(move || {
                let mut manager = manager;
                // The session lives as long as the request sender; when
                // it drops, the loop ends and the manager tears down
                // here (children reaped, sockets closed) — off the main
                // loop, whatever a teardown ladder costs.
                for job in request_rx {
                    let response = match handle(&mut manager, &job.method, &job.params, &job.root) {
                        Ok(result) => Response::ok(job.id, result),
                        Err(error) => {
                            let (code, message) = error.wire();
                            Response::error(Some(job.id), code, message)
                        }
                    };
                    if completion_tx.send(response).is_err() {
                        break;
                    }
                }
            })
            .expect("the debug worker thread spawns");
        Self { requests, completions }
    }

    /// Queues one request. `Some` is the intake-time answer for a
    /// request the worker cannot take: the queue is full (more debug
    /// traffic than one sequential session can chew) or the worker is
    /// gone (see [`DebugDispatch`]'s containment note).
    pub(crate) fn send(&self, job: DebugJob) -> Option<Response> {
        match self.requests.try_send(job) {
            Ok(()) => None,
            Err(TrySendError::Full(DebugJob { id, .. })) => Some(Response::error(
                Some(id),
                codes::DEBUG_SESSION,
                format!(
                    "DARTQ/DEBUG/BUSY: {} requests already queued",
                    DEBUG_QUEUE_CAPACITY
                ),
            )),
            Err(TrySendError::Disconnected(DebugJob { id, .. })) => Some(Response::error(
                Some(id),
                codes::DEBUG_SESSION,
                "DARTQ/DEBUG/WORKER_GONE: the debug worker terminated; retry the request".to_owned(),
            )),
        }
    }

    /// The completion channel the main loop selects on. It lives for
    /// the dispatch's lifetime.
    pub(crate) fn completions(&self) -> &Receiver<Response> {
        &self.completions
    }
}

// The faces' parameter payloads (`#[serde(default)]` keeps every field
// optional beyond the contract-critical ones, the analysis faces'
// convention). `session_id` itself is optional on every session-bearing
// face: absent drives the sole live session ([`select_session`]) — the
// AI-first default, since most interactions hold exactly one session.

#[derive(Deserialize)]
struct LaunchParams {
    script: PathBuf,
    /// `dart` (default) or `flutter`.
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    vm_flags: Vec<String>,
    #[serde(default = "default_pause_on_start")]
    pause_on_start: bool,
    cwd: Option<PathBuf>,
    /// Flutter: the device id (`flutter run -d`).
    #[serde(default)]
    device: Option<String>,
    /// Flutter: the build flavor.
    #[serde(default)]
    flavor: Option<String>,
    /// Flutter: `--dart-define` entries, `KEY=VALUE` strings.
    #[serde(default)]
    dart_defines: Vec<String>,
    /// Flutter: extra `flutter run` arguments passed through.
    #[serde(default)]
    flutter_args: Vec<String>,
}

/// The AI-first default: pause at start, so breakpoints arm before the
/// program runs.
fn default_pause_on_start() -> bool {
    true
}

#[derive(Deserialize)]
struct AttachParams {
    ws_uri: String,
}

/// The `detach`/`stop`/`restart` payload: the selector alone.
#[derive(Deserialize)]
struct SessionParams {
    #[serde(default)]
    session_id: Option<u32>,
}

/// The `status` payload: `session_id` optional — absent lists every
/// session.
#[derive(Deserialize)]
struct StatusParams {
    #[serde(default)]
    session_id: Option<u32>,
}

#[derive(Deserialize)]
struct IsolateParams {
    #[serde(default)]
    session_id: Option<u32>,
    #[serde(default)]
    isolate: Option<String>,
}

#[derive(Deserialize)]
struct BreakpointAddParams {
    #[serde(default)]
    session_id: Option<u32>,
    #[serde(default)]
    isolate: Option<String>,
    location: BreakpointLocationParams,
    #[serde(default)]
    condition: Option<String>,
}

/// One breakpoint target, mirroring [`BreakpointLocationSpec`]: a source
/// line (`script` + `line`, optional `column`) or a function entry
/// (`function_id`).
#[derive(Deserialize)]
struct BreakpointLocationParams {
    #[serde(default)]
    script: Option<String>,
    #[serde(default)]
    line: Option<u32>,
    #[serde(default)]
    column: Option<u32>,
    #[serde(default)]
    function_id: Option<String>,
}

#[derive(Deserialize)]
struct BreakpointRemoveParams {
    #[serde(default)]
    session_id: Option<u32>,
    breakpoint_id: String,
    #[serde(default)]
    isolate: Option<String>,
}

#[derive(Deserialize)]
struct ExceptionPauseParams {
    #[serde(default)]
    session_id: Option<u32>,
    #[serde(default)]
    isolate: Option<String>,
    mode: String,
}

#[derive(Deserialize)]
struct StepParams {
    #[serde(default)]
    session_id: Option<u32>,
    #[serde(default)]
    isolate: Option<String>,
    option: String,
}

/// The `reload` payload.
#[derive(Deserialize)]
struct ReloadParams {
    #[serde(default)]
    session_id: Option<u32>,
    #[serde(default)]
    isolate: Option<String>,
    #[serde(default)]
    force: bool,
}

/// The `restart` payload.
#[derive(Deserialize)]
struct RestartParams {
    #[serde(default)]
    session_id: Option<u32>,
}

/// The `evaluate` payload: a frame index evaluates in that frame of
/// the paused isolate; without one the isolate's root library answers.
#[derive(Deserialize)]
struct EvaluateParams {
    #[serde(default)]
    session_id: Option<u32>,
    #[serde(default)]
    isolate: Option<String>,
    expression: String,
    #[serde(default)]
    frame_index: Option<u32>,
}

#[derive(Deserialize)]
struct SnapshotParams {
    #[serde(default)]
    session_id: Option<u32>,
    isolate: String,
}

#[derive(Deserialize)]
struct EventsParams {
    #[serde(default)]
    session_id: Option<u32>,
    #[serde(default)]
    cursor: u64,
    #[serde(default = "default_event_limit")]
    limit: usize,
}

/// One event page's default width.
fn default_event_limit() -> usize {
    64
}

#[derive(Deserialize)]
struct OutputParams {
    #[serde(default)]
    session_id: Option<u32>,
    stream: String,
    #[serde(default)]
    cursor: u64,
    #[serde(default = "default_max_bytes")]
    max_bytes: usize,
}

/// One output page's default byte budget.
fn default_max_bytes() -> usize {
    4096
}

/// The `exceptionPause` mode spellings.
fn parse_pause_mode(mode: &str) -> Option<ExceptionPauseMode> {
    match mode {
        "none" => Some(ExceptionPauseMode::None),
        "uncaught" => Some(ExceptionPauseMode::Unhandled),
        "all" => Some(ExceptionPauseMode::All),
        _ => None,
    }
}

/// The `step` option spellings (Into/Over/Out — the agent-facing subset
/// of the protocol's five).
fn parse_step_option(option: &str) -> Option<StepOption> {
    match option {
        "into" => Some(StepOption::Into),
        "over" => Some(StepOption::Over),
        "out" => Some(StepOption::Out),
        _ => None,
    }
}

/// The `launch` kind spellings.
fn parse_launch_kind(kind: Option<&str>) -> Result<LaunchKind, DebugError> {
    match kind {
        None | Some("dart") => Ok(LaunchKind::Dart),
        Some("flutter") => Ok(LaunchKind::Flutter),
        Some(other) => Err(DebugError::InvalidParams(format!(
            "kind must be \"dart\" or \"flutter\", got {other:?}"
        ))),
    }
}

/// Splits one `KEY=VALUE` dart-define entry.
fn split_define(entry: &str) -> Result<(String, String), DebugError> {
    entry
        .split_once('=')
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .ok_or_else(|| {
            DebugError::InvalidParams(format!(
                "dart_defines entries must be KEY=VALUE, got {entry:?}"
            ))
        })
}

/// The wire word of a launch kind.
fn launch_kind_word(kind: LaunchKind) -> &'static str {
    match kind {
        LaunchKind::Dart => "dart",
        LaunchKind::Flutter => "flutter",
    }
}

/// Relative request paths resolve against the session workspace root (the
/// analysis faces' rule).
fn resolve(root: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

/// Answers one `dartq/debug/*` request against the registry — executed
/// on the debug worker thread ([`DebugDispatch`]), never the main loop.
/// Every session call is synchronous (a child process, WebSocket round
/// trips, bounded waits) and bounded by dartq-debug's own
/// `SessionConfig` (10 s connect, 30 s handshake budget, 30 s per RPC,
/// 30 s pause waits), so the worker always returns to its queue.
///
/// # Errors
///
/// [`DebugError::InvalidParams`] for a structural mismatch; the session
/// machine codes otherwise.
pub(crate) fn handle(
    manager: &mut DebugManager,
    method: &str,
    params: &Value,
    root: &Path,
) -> Result<Value, DebugError> {
    let invalid = |error: serde_json::Error| DebugError::InvalidParams(error.to_string());
    match method {
        DEBUG_LAUNCH => {
            let decoded =
                serde_json::from_value::<LaunchParams>(params.clone()).map_err(invalid)?;
            if decoded.script.as_os_str().is_empty() {
                return Err(DebugError::InvalidParams(
                    "script must be a non-empty path".into(),
                ));
            }
            let kind = parse_launch_kind(decoded.kind.as_deref())?;
            let resolved = resolve(root, decoded.script.clone());
            let mut config = match kind {
                LaunchKind::Dart => LaunchConfig::new(resolved),
                LaunchKind::Flutter => LaunchConfig::flutter(resolved),
            };
            config.args = decoded.args;
            config.vm_flags = decoded.vm_flags;
            config.pause_on_start = decoded.pause_on_start;
            config.cwd = decoded.cwd.map(|cwd| resolve(root, cwd));
            config.device = decoded.device;
            config.flavor = decoded.flavor;
            config.flutter_args = decoded.flutter_args;
            config.dart_defines = decoded
                .dart_defines
                .iter()
                .map(|entry| split_define(entry))
                .collect::<Result<Vec<_>, _>>()?;
            let session = DebugSession::launch(config).map_err(DebugError::Vm)?;
            let id = manager.register(
                Origin::Launch {
                    script: decoded.script,
                    kind,
                },
                session,
            );
            let session = &manager.get(id).expect("just registered").session;
            let ws_uri = session.ws_uri().to_owned();
            record_manifest(root, id, &ws_uri, "launch", session.child_pid());
            Ok(session_opened_row(id, session))
        }
        DEBUG_ATTACH => {
            let decoded =
                serde_json::from_value::<AttachParams>(params.clone()).map_err(invalid)?;
            if decoded.ws_uri.is_empty() {
                return Err(DebugError::InvalidParams("ws_uri must be a non-empty string".into()));
            }
            let session = DebugSession::attach(&decoded.ws_uri).map_err(DebugError::Vm)?;
            let id = manager.register(Origin::Attach(decoded.ws_uri), session);
            let session = &manager.get(id).expect("just registered").session;
            let ws_uri = session.ws_uri().to_owned();
            // #2619: the fresh attach takes ownership of the program —
            // any orphan manifest naming this ws_uri is consumed.
            record_manifest(root, id, &ws_uri, "attach", None);
            Ok(session_opened_row(id, session))
        }
        DEBUG_DETACH => {
            let decoded =
                serde_json::from_value::<SessionParams>(params.clone()).map_err(invalid)?;
            let id = select_session(manager, decoded.session_id)?;
            let managed = manager.get(id)?;
            // Detach leaves the program running: an attached program
            // was never ours, and a launched child is orphaned to a
            // reaper instead of killed. The ws URI rides the answer so
            // the caller can re-attach (or stop it with `stop`).
            managed.session.detach();
            let ws_uri = managed.session.ws_uri().to_owned();
            manager.remove(id);
            delete_manifest(&manifests_dir(root), id);
            Ok(json!({ "ok": true, "ws_uri": ws_uri }))
        }
        DEBUG_STOP => {
            let decoded =
                serde_json::from_value::<SessionParams>(params.clone()).map_err(invalid)?;
            let id = select_session(manager, decoded.session_id)?;
            let managed = manager.get(id)?;
            managed.session.terminate().map_err(DebugError::Vm)?;
            manager.remove(id);
            delete_manifest(&manifests_dir(root), id);
            Ok(json!({ "ok": true }))
        }
        DEBUG_STATUS => {
            let decoded =
                serde_json::from_value::<StatusParams>(params.clone()).map_err(invalid)?;
            // #2619: status always answers a document — the live
            // sessions plus the orphan manifests left by daemons that
            // are gone (an agent's re-attach trail). Even both-empty is
            // a printed document, never blank output.
            let orphans = load_manifests(&manifests_dir(root))
                .into_iter()
                .filter(|(id, _)| !manager.sessions.contains_key(id))
                .map(|(id, manifest)| orphan_row(id, &manifest))
                .collect::<Vec<_>>();
            match decoded.session_id {
                None => {
                    // Deterministic order: ids ascending, regardless of
                    // the map's layout.
                    let mut ids: Vec<u32> = manager.sessions.keys().copied().collect();
                    ids.sort_unstable();
                    Ok(json!({
                        "v": 1,
                        "sessions": ids
                            .into_iter()
                            .map(|id| {
                                let managed = manager.get(id).expect("keyed id");
                                status_row(id, managed)
                            })
                            .collect::<Vec<_>>(),
                        "orphans": orphans,
                    }))
                }
                Some(id) => {
                    let managed = manager.get(id)?;
                    let mut row = status_row(id, managed);
                    row["breakpoints"] = Value::Array(
                        managed.session.breakpoints().map(breakpoint_row).collect(),
                    );
                    Ok(json!({ "v": 1, "sessions": [row], "orphans": orphans }))
                }
            }
        }
        DEBUG_BREAKPOINT_ADD => {
            let decoded =
                serde_json::from_value::<BreakpointAddParams>(params.clone()).map_err(invalid)?;
            let location = match (&decoded.location.script, decoded.location.line) {
                (Some(script), Some(line)) => BreakpointLocationSpec::Line {
                    script_uri: script.clone(),
                    line,
                    column: decoded.location.column,
                },
                (None, None) => match &decoded.location.function_id {
                    Some(function_id) => BreakpointLocationSpec::Function {
                        function_id: function_id.clone(),
                    },
                    None => {
                        return Err(DebugError::InvalidParams(
                            "location must name {script, line} (line breakpoints) or \
                             {function_id} (function entry)"
                                .into(),
                        ));
                    }
                },
                _ => {
                    return Err(DebugError::InvalidParams(
                        "a line breakpoint needs both script and line".into(),
                    ));
                }
            };
            let id = select_session(manager, decoded.session_id)?;
            let managed = manager.get(id)?;
            let handle = managed
                .session
                .add_breakpoint(BreakpointSpec {
                    isolate: decoded.isolate,
                    location,
                    condition: decoded.condition,
                })
                .map_err(DebugError::Vm)?;
            Ok(breakpoint_row(&handle))
        }
        DEBUG_BREAKPOINT_REMOVE => {
            let decoded =
                serde_json::from_value::<BreakpointRemoveParams>(params.clone()).map_err(invalid)?;
            let id = select_session(manager, decoded.session_id)?;
            let managed = manager.get(id)?;
            managed
                .session
                .remove_breakpoint(decoded.isolate.as_deref(), &decoded.breakpoint_id)
                .map_err(DebugError::Vm)?;
            Ok(json!({ "ok": true }))
        }
        DEBUG_EXCEPTION_PAUSE => {
            let decoded =
                serde_json::from_value::<ExceptionPauseParams>(params.clone()).map_err(invalid)?;
            let mode = parse_pause_mode(&decoded.mode).ok_or_else(|| {
                DebugError::InvalidParams(format!(
                    "mode must be one of none/uncaught/all, got {:?}",
                    decoded.mode
                ))
            })?;
            let id = select_session(manager, decoded.session_id)?;
            let managed = manager.get(id)?;
            managed
                .session
                .set_exception_pause_mode(decoded.isolate.as_deref(), mode)
                .map_err(DebugError::Vm)?;
            Ok(json!({ "ok": true }))
        }
        DEBUG_RESUME | DEBUG_PAUSE => {
            let decoded =
                serde_json::from_value::<IsolateParams>(params.clone()).map_err(invalid)?;
            let id = select_session(manager, decoded.session_id)?;
            let managed = manager.get(id)?;
            let result = if method == DEBUG_RESUME {
                managed.session.resume(decoded.isolate.as_deref())
            } else {
                managed.session.pause(decoded.isolate.as_deref())
            };
            result.map_err(DebugError::Vm)?;
            Ok(json!({ "ok": true }))
        }
        DEBUG_STEP => {
            let decoded = serde_json::from_value::<StepParams>(params.clone()).map_err(invalid)?;
            let option = parse_step_option(&decoded.option).ok_or_else(|| {
                DebugError::InvalidParams(format!(
                    "option must be one of into/over/out, got {:?}",
                    decoded.option
                ))
            })?;
            let id = select_session(manager, decoded.session_id)?;
            let managed = manager.get(id)?;
            let snapshot = managed
                .session
                .step_and_package(decoded.isolate.as_deref(), Some(option))
                .map_err(DebugError::Vm)?;
            Ok(snapshot_row(&snapshot))
        }
        DEBUG_RELOAD => {
            let decoded =
                serde_json::from_value::<ReloadParams>(params.clone()).map_err(invalid)?;
            let id = select_session(manager, decoded.session_id)?;
            let managed = manager.get(id)?;
            // #2622: the answer reports the reload's own facts — success,
            // the isolate count, the wall cost — and a failure rides the
            // same document as `err` instead of a wire error, because the
            // session stays alive and usable after a failed reload (a
            // compile error in the edited sources; dartq-debug's reload
            // contract).
            let started = Instant::now();
            let outcome =
                managed.session.reload_sources(decoded.isolate.as_deref(), decoded.force);
            Ok(reload_row(started, outcome))
        }
        DEBUG_RESTART => {
            let decoded =
                serde_json::from_value::<RestartParams>(params.clone()).map_err(invalid)?;
            let id = select_session(manager, decoded.session_id)?;
            let managed = manager.get(id)?;
            let outcome = managed.session.hot_restart().map_err(DebugError::Vm)?;
            Ok(restart_row(&outcome))
        }
        DEBUG_EVALUATE => {
            let decoded =
                serde_json::from_value::<EvaluateParams>(params.clone()).map_err(invalid)?;
            if decoded.expression.is_empty() {
                return Err(DebugError::InvalidParams(
                    "expression must be a non-empty string".into(),
                ));
            }
            let id = select_session(manager, decoded.session_id)?;
            let managed = manager.get(id)?;
            // A frame-indexed evaluation needs a frame to exist. A
            // pause with no top frame (an isolate interrupted while
            // idle) has an empty stack — the VM would wrap the miss in
            // its generic compile-error text, so the session's tracked
            // pause answers the real condition before any round trip.
            if let Some(frame_index) = decoded.frame_index
                && managed.session.paused_frame_exists(decoded.isolate.as_deref()) == Some(false)
            {
                return Err(DebugError::NoFrame(frame_index));
            }
            let value = match decoded.frame_index {
                Some(frame_index) => managed.session.evaluate_in_frame(
                    decoded.isolate.as_deref(),
                    frame_index,
                    &decoded.expression,
                ),
                None => managed
                    .session
                    .evaluate_in_root_lib(decoded.isolate.as_deref(), &decoded.expression),
            }
            .map_err(DebugError::Vm)?;
            Ok(vm_object_row(&value))
        }
        DEBUG_SNAPSHOT => {
            let decoded =
                serde_json::from_value::<SnapshotParams>(params.clone()).map_err(invalid)?;
            let id = select_session(manager, decoded.session_id)?;
            let managed = manager.get(id)?;
            // The event-pump posture: fold what the connection already
            // delivered before reading (`wait_for_pause` polls again in
            // its own loop).
            managed.session.poll_events().map_err(DebugError::Vm)?;
            let snapshot = managed
                .session
                .wait_for_pause(&decoded.isolate)
                .map_err(DebugError::Vm)?;
            Ok(snapshot_row(&snapshot))
        }
        DEBUG_EVENTS => {
            let decoded =
                serde_json::from_value::<EventsParams>(params.clone()).map_err(invalid)?;
            let id = select_session(manager, decoded.session_id)?;
            let managed = manager.get(id)?;
            managed.session.poll_events().map_err(DebugError::Vm)?;
            let batch = managed.session.events_since(decoded.cursor, decoded.limit);
            Ok(events_row(&batch))
        }
        DEBUG_OUTPUT => {
            let decoded =
                serde_json::from_value::<OutputParams>(params.clone()).map_err(invalid)?;
            let stderr = match decoded.stream.as_str() {
                "stdout" => false,
                "stderr" => true,
                other => {
                    return Err(DebugError::InvalidParams(format!(
                        "stream must be \"stdout\" or \"stderr\", got {other:?}"
                    )));
                }
            };
            let id = select_session(manager, decoded.session_id)?;
            let managed = manager.get(id)?;
            managed.session.poll_events().map_err(DebugError::Vm)?;
            let batch = if stderr {
                managed.session.stderr_since(decoded.cursor, decoded.max_bytes)
            } else {
                managed.session.stdout_since(decoded.cursor, decoded.max_bytes)
            };
            Ok(output_row(&batch))
        }
        // #2619: the serve loop's exit paths own this face exclusively
        // (never in DEBUG_REQUESTS, so no wire request reaches it). Each
        // detach resumes every paused isolate first (dartq-debug's
        // release courtesy, bounded by its own resume budget), the
        // program runs free, and the manifest records exactly that.
        DEBUG_SHUTDOWN => {
            let mut ids: Vec<u32> = manager.sessions.keys().copied().collect();
            ids.sort_unstable();
            let dir = manifests_dir(root);
            let mut released = 0;
            for id in ids {
                if let Some(mut managed) = manager.remove(id) {
                    managed.session.detach();
                    mark_released(&dir, id);
                    released += 1;
                }
            }
            Ok(json!({ "released": released }))
        }
        _ => Err(DebugError::InvalidParams(format!(
            "not a dartq/debug command: {method}"
        ))),
    }
}

// The DTO shapers — pure functions over dartq-debug's session types.

/// The debuggable isolates of a session (the VM's user isolates — system
/// isolates are tracked for completeness but are not debug targets).
fn user_isolate_ids(session: &DebugSession) -> Vec<&str> {
    session
        .isolates()
        .filter(|isolate| !isolate.is_system_isolate)
        .map(|isolate| isolate.id.as_str())
        .collect()
}

/// The `launch`/`attach` answer: the fresh session's id and isolate set.
fn session_opened_row(id: u32, session: &DebugSession) -> Value {
    json!({
        "session_id": id,
        "isolates": user_isolate_ids(session),
        "main": session.main_isolate(),
    })
}

/// The `reload` answer (#2622): `ok`, the isolate count reloaded (the
/// targeted isolate — one; on a Flutter session the tool-orchestrated
/// reload reassembles the whole app behind it and the tool's answer
/// carries no count), the wall milliseconds, and `err` naming the
/// failure when it failed — the VM's own text for a VM error, so the
/// agent sees the compile diagnostics without a second call.
fn reload_row(started: Instant, outcome: Result<bool, dartq_debug::Error>) -> Value {
    let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    match outcome {
        Ok(true) => json!({"ok": true, "isolates_reloaded": 1, "ms": ms}),
        Ok(false) => json!({
            "ok": false,
            "isolates_reloaded": 0,
            "ms": ms,
            "err": "the reload reported failure",
        }),
        Err(error) => json!({
            "ok": false,
            "isolates_reloaded": 0,
            "ms": ms,
            "err": error.to_string(),
        }),
    }
}

/// Writes the fresh session's manifest and consumes the stale trail it
/// supersedes — any prior manifest naming the same ws_uri (a re-attach
/// owns the program now).
fn record_manifest(root: &Path, id: u32, ws_uri: &str, kind: &str, pid: Option<u32>) {
    let dir = manifests_dir(root);
    write_manifest(
        &dir,
        id,
        &SessionManifest {
            ws_uri: ws_uri.to_owned(),
            kind: kind.to_owned(),
            since: unix_s(),
            pid,
            released: None,
        },
    );
    consume_matching(&dir, ws_uri, id);
}

/// One orphan manifest as `status` lists it: where to re-attach, how it
/// came to be, when, and whether the daemon that held it released the
/// app on its way out (`released` rides along only when it did).
fn orphan_row(id: u32, manifest: &SessionManifest) -> Value {
    let mut row = json!({
        "id": id,
        "ws_uri": manifest.ws_uri,
        "kind": manifest.kind,
        "since": manifest.since,
    });
    if manifest.released == Some(true) {
        row["released"] = json!(true);
    }
    row
}

/// One `status` row: the origin (script or ws_uri), the isolate set, and
/// the breakpoint tally.
fn status_row(id: u32, managed: &ManagedSession) -> Value {
    let session = &managed.session;
    let mut row = json!({
        "session_id": id,
        "main": session.main_isolate(),
        "isolates": user_isolate_ids(session),
        "bps": session.breakpoints().count(),
    });
    match &managed.origin {
        Origin::Launch { script, kind } => {
            row["script"] = json!(script.display().to_string());
            row["kind"] = json!(launch_kind_word(*kind));
        }
        Origin::Attach(uri) => row["ws_uri"] = json!(uri),
    }
    row
}

/// A hot-restart outcome: the re-armed breakpoints and the drops.
fn restart_row(outcome: &RestartOutcome) -> Value {
    json!({
        "re_armed": outcome.re_armed.iter().map(breakpoint_row).collect::<Vec<_>>(),
        "dropped": outcome.dropped,
    })
}

/// One evaluation answer: a short rendering plus the lazy-expansion
/// handle where one exists.
fn vm_object_row(object: &VmObject) -> Value {
    match object {
        VmObject::Instance(instance) => {
            let class = instance
                .class
                .as_ref()
                .and_then(|class| class.name.clone())
                .unwrap_or_else(|| "instance".into());
            json!({
                "kind": "instance",
                "class": class,
                "value": instance.value_as_string.clone().unwrap_or_default(),
                "handle": if instance.id.is_empty() { Value::Null } else { json!(instance.id) },
            })
        }
        VmObject::Sentinel(sentinel) => json!({
            "kind": "sentinel",
            "value": sentinel
                .value_as_string
                .clone()
                .unwrap_or_else(|| sentinel.kind.as_str().to_owned()),
        }),
        VmObject::Error { kind, message } => {
            json!({ "kind": "error", "error_kind": kind, "message": message })
        }
        VmObject::Other(raw) => json!({ "kind": "other", "raw": raw }),
    }
}

/// A breakpoint handle as the daemon answers it.
fn breakpoint_row(handle: &BreakpointHandle) -> Value {
    json!({
        "id": handle.id,
        "isolate": handle.isolate,
        "condition": handle.condition,
        "resolved": handle.resolved,
    })
}

/// A stopped snapshot: why, where, the bounded stack summary, and the top
/// frame's variables with their lazy-expansion handles.
fn snapshot_row(snapshot: &StoppedSnapshot) -> Value {
    json!({
        "reason": reason_row(&snapshot.reason),
        "isolate": snapshot.isolate,
        "location": frame_location_row(&snapshot.location),
        "stack": snapshot.stack.iter().map(frame_summary_row).collect::<Vec<_>>(),
        "variables": snapshot.variables.iter().map(variable_row).collect::<Vec<_>>(),
        "at_async_suspension": snapshot.at_async_suspension,
    })
}

/// Why an isolate stopped, as a `kind`-tagged object.
fn reason_row(reason: &StoppedReason) -> Value {
    match reason {
        StoppedReason::Start => json!({ "kind": "start" }),
        StoppedReason::Step => json!({ "kind": "step" }),
        StoppedReason::Exception => json!({ "kind": "exception" }),
        StoppedReason::Interrupted => json!({ "kind": "interrupted" }),
        StoppedReason::Exit => json!({ "kind": "exit" }),
        StoppedReason::PostRequest => json!({ "kind": "postRequest" }),
        StoppedReason::Breakpoint { breakpoints } => {
            json!({ "kind": "breakpoint", "breakpoints": breakpoints })
        }
        StoppedReason::ConditionError {
            breakpoints,
            message,
        } => json!({ "kind": "conditionError", "breakpoints": breakpoints, "message": message }),
        StoppedReason::ConditionalResumeLimit { auto_resumes } => {
            json!({ "kind": "conditionalResumeLimit", "auto_resumes": auto_resumes })
        }
        StoppedReason::Other(kind) => json!({ "kind": "other", "event_kind": kind.as_str() }),
    }
}

/// A frame position.
fn frame_location_row(location: &FrameLocation) -> Value {
    json!({
        "line": location.line,
        "column": location.column,
        "script_uri": location.script_uri,
        "script_id": location.script_id,
        "function": location.function,
    })
}

/// One stack-summary entry.
fn frame_summary_row(frame: &FrameSummary) -> Value {
    json!({
        "index": frame.index,
        "function": frame.function,
        "location": frame_location_row(&frame.location),
    })
}

/// One variable: its name, short value rendering, and the object handle
/// for lazy expansion (`null` for leaves).
fn variable_row(variable: &Variable) -> Value {
    json!({
        "name": variable.name,
        "value": variable.value,
        "handle": variable.handle,
    })
}

/// One event page. Each event carries its kind as a string tag and embeds
/// the serde-derived protocol `Event` verbatim; the raw JSON rides along
/// only for kinds the typed event does not model (dartq-debug's
/// retention rule for `SessionEvent::raw`).
fn events_row(batch: &EventBatch) -> Value {
    json!({
        "events": batch.events.iter().map(event_row).collect::<Vec<_>>(),
        "next_cursor": batch.next_cursor,
        "dropped": batch.dropped,
    })
}

/// One recorded stream event.
fn event_row(event: &SessionEvent) -> Value {
    let mut row = json!({
        "cursor": event.cursor,
        "stream_id": event.stream_id,
        "kind": event.event.kind.as_str(),
        "isolate": event.event.isolate.as_ref().map(|isolate| isolate.id.clone()),
        "event": serde_json::to_value(&event.event).expect("protocol event serializes"),
    });
    if let Some(raw) = &event.raw {
        row["raw"] = raw.clone();
    }
    row
}

/// One output page. The bytes decode lossily (U+FFFD for invalid UTF-8 —
/// the paging convention; the `events` face keeps exact `WriteEvent`
/// payloads for byte-exact needs).
fn output_row(batch: &OutputBatch) -> Value {
    json!({
        "text": String::from_utf8_lossy(&batch.bytes),
        "next_cursor": batch.next_cursor,
        "dropped": batch.dropped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dartq_debug::Event;

    /// The `launch` payload decodes with its AI-first defaults (a dart
    /// launch), carries every Flutter option, and rejects a missing
    /// script (the only required field).
    #[test]
    fn launch_params_decode_with_defaults_and_reject_missing_script() {
        let decoded: LaunchParams =
            serde_json::from_value(json!({ "script": "bin/main.dart" })).expect("decodes");
        assert_eq!(decoded.script, PathBuf::from("bin/main.dart"));
        assert_eq!(decoded.kind.as_deref(), None);
        assert!(decoded.args.is_empty());
        assert!(decoded.vm_flags.is_empty());
        assert!(decoded.pause_on_start);
        assert!(decoded.cwd.is_none());
        assert!(decoded.device.is_none());
        assert!(decoded.flavor.is_none());
        assert!(decoded.dart_defines.is_empty());
        assert!(decoded.flutter_args.is_empty());

        let decoded: LaunchParams = serde_json::from_value(json!({
            "script": "bin/main.dart",
            "args": ["--fast"],
            "vm_flags": ["--enable-asserts"],
            "pause_on_start": false,
            "cwd": "bin"
        }))
        .expect("decodes");
        assert_eq!(decoded.args, ["--fast"]);
        assert_eq!(decoded.vm_flags, ["--enable-asserts"]);
        assert!(!decoded.pause_on_start);
        assert_eq!(decoded.cwd, Some(PathBuf::from("bin")));

        let flutter: LaunchParams = serde_json::from_value(json!({
            "script": "lib/main.dart",
            "kind": "flutter",
            "device": "macos",
            "flavor": "dev",
            "dart_defines": ["PROBE=1"],
            "flutter_args": ["--no-devtools"]
        }))
        .expect("decodes");
        assert_eq!(flutter.kind.as_deref(), Some("flutter"));
        assert_eq!(flutter.device.as_deref(), Some("macos"));
        assert_eq!(flutter.flavor.as_deref(), Some("dev"));
        assert_eq!(flutter.dart_defines, ["PROBE=1"]);
        assert_eq!(flutter.flutter_args, ["--no-devtools"]);

        assert!(serde_json::from_value::<LaunchParams>(json!({ "args": [] })).is_err());
        assert!(serde_json::from_value::<LaunchParams>(json!({ "script": 7 })).is_err());
    }

    /// The launch kind maps its wire words or rejects; a dart-define
    /// splits on the first `=` or answers the shared tier.
    #[test]
    fn launch_kind_and_dart_defines_map_or_reject() {
        assert!(matches!(parse_launch_kind(None), Ok(LaunchKind::Dart)));
        assert!(matches!(
            parse_launch_kind(Some("dart")),
            Ok(LaunchKind::Dart)
        ));
        assert!(matches!(
            parse_launch_kind(Some("flutter")),
            Ok(LaunchKind::Flutter)
        ));
        assert!(parse_launch_kind(Some("Flutter")).is_err());

        assert_eq!(
            split_define("PROBE=1=x").expect("splits on the first ="),
            ("PROBE".to_owned(), "1=x".to_owned())
        );
        assert!(split_define("PROBE").is_err());
        assert_eq!(launch_kind_word(LaunchKind::Flutter), "flutter");
        assert_eq!(launch_kind_word(LaunchKind::Dart), "dart");
    }

    /// A breakpoint location decodes in both shapes — the line form and
    /// the function-entry form — and the half-filled line form is the
    /// explicit rejection (a script without a line arms nothing).
    #[test]
    fn breakpoint_location_params_decode_line_and_function_shapes() {
        let line: BreakpointAddParams = serde_json::from_value(json!({
            "session_id": 1,
            "location": { "script": "package:app/main.dart", "line": 12, "column": 5 },
            "condition": "x > 3"
        }))
        .expect("decodes");
        assert_eq!(line.location.script.as_deref(), Some("package:app/main.dart"));
        assert_eq!(line.location.line, Some(12));
        assert_eq!(line.location.column, Some(5));
        assert_eq!(line.condition.as_deref(), Some("x > 3"));
        assert!(line.location.function_id.is_none());

        let function: BreakpointAddParams = serde_json::from_value(json!({
            "session_id": 1,
            "location": { "function_id": "functions/42" }
        }))
        .expect("decodes");
        assert_eq!(function.location.function_id.as_deref(), Some("functions/42"));
        assert!(function.location.script.is_none());
        assert_eq!(function.location.line, None);

        // A script without a line decodes (the fields are individually
        // optional) — the half-filled line form is rejected at dispatch,
        // below, where the location is assembled.
        let half: BreakpointAddParams = serde_json::from_value(json!({
            "session_id": 1,
            "location": { "script": "package:app/main.dart" }
        }))
        .expect("decodes");
        assert!(half.location.line.is_none());
    }

    /// The enum-spelled params map their wire words or reject with the
    /// tier every face uses; the defaults of the numeric params hold.
    #[test]
    fn step_pause_mode_and_stream_spellings_map_or_reject() {
        assert!(matches!(parse_step_option("into"), Some(StepOption::Into)));
        assert!(matches!(parse_step_option("over"), Some(StepOption::Over)));
        assert!(matches!(parse_step_option("out"), Some(StepOption::Out)));
        assert!(parse_step_option("Into").is_none(), "the wire words are lowercase");

        assert!(matches!(
            parse_pause_mode("uncaught"),
            Some(ExceptionPauseMode::Unhandled)
        ));
        assert!(matches!(parse_pause_mode("all"), Some(ExceptionPauseMode::All)));
        assert!(matches!(parse_pause_mode("none"), Some(ExceptionPauseMode::None)));
        assert!(parse_pause_mode("All").is_none());

        let events: EventsParams =
            serde_json::from_value(json!({ "session_id": 1 })).expect("decodes");
        assert_eq!(events.cursor, 0);
        assert_eq!(events.limit, default_event_limit());

        let output: OutputParams =
            serde_json::from_value(json!({ "session_id": 1, "stream": "stdout" }))
                .expect("decodes");
        assert_eq!(output.cursor, 0);
        assert_eq!(output.max_bytes, default_max_bytes());
        assert!(
            serde_json::from_value::<OutputParams>(json!({ "session_id": 1, "stream": "stdin" }))
                .is_ok(),
            "the stream word is validated at dispatch, not by serde"
        );
    }

    /// The snapshot DTO maps every stop reason to its kind tag and the
    /// position/stack/variable rows to their real fields.
    #[test]
    fn snapshot_row_maps_every_reason_kind() {
        let location = FrameLocation {
            line: Some(12),
            column: Some(3),
            script_uri: Some("package:app/main.dart".into()),
            script_id: Some("scripts/1".into()),
            function: Some("main".into()),
        };
        let reasons = [
            (StoppedReason::Start, "start"),
            (StoppedReason::Step, "step"),
            (StoppedReason::Exception, "exception"),
            (StoppedReason::Interrupted, "interrupted"),
            (StoppedReason::Exit, "exit"),
            (StoppedReason::PostRequest, "postRequest"),
            (
                StoppedReason::Breakpoint {
                    breakpoints: vec!["breakpoints/1".into()],
                },
                "breakpoint",
            ),
            (
                StoppedReason::ConditionError {
                    breakpoints: vec!["breakpoints/1".into()],
                    message: "compile error".into(),
                },
                "conditionError",
            ),
            (
                StoppedReason::ConditionalResumeLimit { auto_resumes: 1001 },
                "conditionalResumeLimit",
            ),
            (
                StoppedReason::Other(dartq_debug::EventKind::PauseBreakpoint),
                "other",
            ),
        ];
        for (reason, kind) in &reasons {
            let snapshot = StoppedSnapshot {
                reason: reason.clone(),
                isolate: "isolates/7".into(),
                location: location.clone(),
                stack: vec![FrameSummary {
                    index: 0,
                    function: Some("main".into()),
                    location: location.clone(),
                }],
                variables: vec![Variable {
                    name: "count".into(),
                    value: "42".into(),
                    handle: Some("objects/9".into()),
                }],
                at_async_suspension: true,
            };
            let row = snapshot_row(&snapshot);
            assert_eq!(row["reason"]["kind"], *kind, "the reason tag: {row}");
            assert_eq!(row["isolate"], "isolates/7");
            assert_eq!(row["location"]["line"], 12);
            assert_eq!(row["location"]["script_uri"], "package:app/main.dart");
            assert_eq!(row["stack"][0]["index"], 0);
            assert_eq!(row["stack"][0]["location"]["function"], "main");
            assert_eq!(row["variables"][0]["name"], "count");
            assert_eq!(row["variables"][0]["value"], "42");
            assert_eq!(row["variables"][0]["handle"], "objects/9");
            assert_eq!(row["at_async_suspension"], true);
        }
        // The reason payloads ride along for the compound kinds.
        let breakpoint = snapshot_row(&StoppedSnapshot {
            reason: StoppedReason::Breakpoint {
                breakpoints: vec!["breakpoints/1".into(), "breakpoints/2".into()],
            },
            isolate: "isolates/7".into(),
            location: location.clone(),
            stack: Vec::new(),
            variables: Vec::new(),
            at_async_suspension: false,
        });
        assert_eq!(
            breakpoint["reason"]["breakpoints"],
            json!(["breakpoints/1", "breakpoints/2"])
        );
        let other = snapshot_row(&StoppedSnapshot {
            reason: StoppedReason::Other(dartq_debug::EventKind::WriteEvent),
            isolate: "isolates/7".into(),
            location,
            stack: Vec::new(),
            variables: Vec::new(),
            at_async_suspension: false,
        });
        assert_eq!(other["reason"]["event_kind"], "WriteEvent");
    }

    /// A leaf variable serializes its absent handle as `null`, and a
    /// breakpoint handle answers its real fields.
    #[test]
    fn variable_and_breakpoint_rows_use_the_real_fields() {
        let leaf = variable_row(&Variable {
            name: "flag".into(),
            value: "null".into(),
            handle: None,
        });
        assert_eq!(leaf, json!({ "name": "flag", "value": "null", "handle": null }));

        let handle = breakpoint_row(&BreakpointHandle {
            id: "breakpoints/3".into(),
            isolate: "isolates/7".into(),
            condition: Some("x > 3".into()),
            resolved: false,
        });
        assert_eq!(
            handle,
            json!({
                "id": "breakpoints/3",
                "isolate": "isolates/7",
                "condition": "x > 3",
                "resolved": false,
            })
        );
    }

    /// A restart answer carries the re-armed breakpoint rows and the
    /// drops; an evaluation answer renders each `VmObject` shape with
    /// its lazy handle.
    #[test]
    fn restart_and_evaluate_rows_shape_the_outcomes() {
        let outcome = RestartOutcome {
            re_armed: vec![BreakpointHandle {
                id: "breakpoints/1".into(),
                isolate: "isolates/9".into(),
                condition: None,
                resolved: true,
            }],
            dropped: vec!["rpc error 102: script gone".into()],
        };
        let row = restart_row(&outcome);
        assert_eq!(row["re_armed"][0]["id"], "breakpoints/1");
        assert_eq!(row["dropped"][0], "rpc error 102: script gone");

        let instance = serde_json::from_value::<dartq_debug::Instance>(json!({
            "type": "@Instance",
            "id": "objects/7",
            "kind": "String",
            "valueAsString": "probe-v1"
        }))
        .expect("instance decodes");
        let row = vm_object_row(&VmObject::Instance(Box::new(instance)));
        assert_eq!(row["kind"], "instance");
        assert_eq!(row["value"], "probe-v1");
        assert_eq!(row["handle"], "objects/7");

        let error = vm_object_row(&VmObject::Error {
            kind: "EvaluationError".into(),
            message: "undefined name".into(),
        });
        assert_eq!(error["kind"], "error");
        assert_eq!(error["message"], "undefined name");
        assert!(vm_object_row(&VmObject::Other(json!({ "type": "Script" })))["raw"]["type"]
            == "Script");
    }

    /// One recorded event answers its kind tag, cursor, and stream, embeds
    /// the protocol event, and carries the raw JSON only when dartq-debug
    /// retained one.
    #[test]
    fn event_row_carries_kind_tag_and_raw() {
        let event: Event =
            serde_json::from_value(json!({ "kind": "PauseBreakpoint", "timestamp": 1729 }))
                .expect("protocol event decodes");
        let mut recorded = SessionEvent {
            cursor: 41,
            stream_id: "Debug".into(),
            event,
            raw: None,
        };
        let row = event_row(&recorded);
        assert_eq!(row["cursor"], 41);
        assert_eq!(row["stream_id"], "Debug");
        assert_eq!(row["kind"], "PauseBreakpoint");
        assert_eq!(row["isolate"], Value::Null);
        assert_eq!(row["event"]["timestamp"], 1729);
        assert!(row.get("raw").is_none(), "a modeled kind keeps no raw copy");

        recorded.raw = Some(json!({ "unmodeled": true }));
        let row = event_row(&recorded);
        assert_eq!(row["raw"]["unmodeled"], true);
    }

    /// An output page decodes its bytes lossily (invalid UTF-8 becomes
    /// U+FFFD, never a failure) and passes the cursors through.
    #[test]
    fn output_row_decodes_lossily_and_passes_cursors() {
        let batch = OutputBatch {
            bytes: b"ok\xffbad".to_vec(),
            next_cursor: 9,
            dropped: 2,
        };
        let row = output_row(&batch);
        assert_eq!(row["text"], "ok\u{fffd}bad");
        assert_eq!(row["next_cursor"], 9);
        assert_eq!(row["dropped"], 2);
    }

    /// The sole-session selector: an absent `session_id` never reaches a
    /// VM against an empty registry — every session-bearing face answers
    /// the machine-coded `NO_SESSIONS` miss naming the way out, and an
    /// explicit id keeps its own `NO_SUCH_SESSION` miss. The several-
    /// sessions listing needs live sessions and is pinned at the process
    /// level (the e2e debug leg's ambiguity check).
    #[test]
    fn absent_session_id_answers_the_machine_coded_misses() {
        let mut manager = DebugManager::sharing(Arc::new(AtomicUsize::new(0)));
        let root = Path::new("/workspace");
        for (method, params) in [
            (DEBUG_DETACH, json!({})),
            (DEBUG_STOP, json!({})),
            (DEBUG_RESUME, json!({})),
            (DEBUG_PAUSE, json!({})),
            (DEBUG_STEP, json!({"option": "into"})),
            (DEBUG_EVALUATE, json!({"expression": "1"})),
            (DEBUG_SNAPSHOT, json!({"isolate": "isolates/1"})),
            (DEBUG_OUTPUT, json!({"stream": "stdout"})),
            (DEBUG_EVENTS, json!({})),
            (DEBUG_RELOAD, json!({})),
            (DEBUG_RESTART, json!({})),
            (DEBUG_EXCEPTION_PAUSE, json!({"mode": "all"})),
            (DEBUG_BREAKPOINT_REMOVE, json!({"breakpoint_id": "breakpoints/1"})),
            (
                DEBUG_BREAKPOINT_ADD,
                json!({"location": {"script": "package:app/main.dart", "line": 1}}),
            ),
        ] {
            let error = handle(&mut manager, method, &params, root)
                .err()
                .unwrap_or_else(|| panic!("{method} must not reach a VM with no session"));
            let (code, message) = error.wire();
            assert_eq!(code, codes::DEBUG_SESSION, "{method}: {message}");
            assert!(
                message.starts_with("DARTQ/DEBUG/NO_SESSIONS: "),
                "{method}: {message}"
            );
            assert!(message.contains("launch or attach"), "{method}: {message}");
        }
        // An explicit id keeps its own miss.
        let (code, message) = handle(&mut manager, DEBUG_RESUME, &json!({"session_id": 9}), root)
            .expect_err("miss")
            .wire();
        assert_eq!(code, codes::DEBUG_SESSION);
        assert!(
            message.starts_with("DARTQ/DEBUG/NO_SUCH_SESSION: "),
            "{message}"
        );
    }

    /// The dispatch wiring is hermetically exercisable without a live
    /// session: the empty registry lists empty, every session_id-bearing
    /// face answers the machine-coded miss, and bad params answer the
    /// shared `INVALID_PARAMS` tier.
    #[test]
    fn dispatch_answers_unknown_session_and_empty_status() {
        let mut manager = DebugManager::sharing(Arc::new(AtomicUsize::new(0)));
        let root = Path::new("/workspace");

        let answer = handle(&mut manager, DEBUG_STATUS, &json!({}), root).expect("status");
        assert_eq!(answer, json!({"v": 1, "sessions": [], "orphans": []}));

        for (method, params) in [
            (DEBUG_DETACH, json!({ "session_id": 1 })),
            (DEBUG_STOP, json!({ "session_id": 1 })),
            (DEBUG_RESUME, json!({ "session_id": 1 })),
            (DEBUG_PAUSE, json!({ "session_id": 1, "isolate": "isolates/1" })),
            (DEBUG_SNAPSHOT, json!({ "session_id": 1, "isolate": "isolates/1" })),
            (DEBUG_EVENTS, json!({ "session_id": 1 })),
            (DEBUG_OUTPUT, json!({ "session_id": 1, "stream": "stdout" })),
            (DEBUG_BREAKPOINT_REMOVE, json!({ "session_id": 1, "breakpoint_id": "breakpoints/1" })),
            (DEBUG_EXCEPTION_PAUSE, json!({ "session_id": 1, "mode": "none" })),
            (DEBUG_STEP, json!({ "session_id": 1, "option": "into" })),
            (DEBUG_RELOAD, json!({ "session_id": 1 })),
            (DEBUG_RESTART, json!({ "session_id": 1 })),
            (DEBUG_EVALUATE, json!({ "session_id": 1, "expression": "1 + 1" })),
        ] {
            let error = handle(&mut manager, method, &params, root)
                .err()
                .unwrap_or_else(|| panic!("{method} must not reach a VM with no session"));
            let (code, message) = error.wire();
            assert_eq!(code, codes::DEBUG_SESSION, "{method}: {message}");
            assert!(
                message.starts_with("DARTQ/DEBUG/NO_SUCH_SESSION: "),
                "{method}: {message}"
            );
        }

        // Param-shape failures answer the shared tier before any session
        // lookup: a missing script, an unknown step option, a bad stream
        // word, an empty ws_uri, and a half-filled line location (a script
        // without a line names no position).
        for (method, params) in [
            (DEBUG_LAUNCH, json!({})),
            (DEBUG_LAUNCH, json!({ "script": "lib/main.dart", "kind": "swift" })),
            (
                DEBUG_LAUNCH,
                json!({ "script": "lib/main.dart", "kind": "flutter", "dart_defines": ["NOVALUE"] }),
            ),
            (DEBUG_STEP, json!({ "session_id": 1, "option": "sideways" })),
            (DEBUG_OUTPUT, json!({ "session_id": 1, "stream": "stdin" })),
            (DEBUG_ATTACH, json!({ "ws_uri": "" })),
            (DEBUG_EXCEPTION_PAUSE, json!({ "session_id": 1, "mode": "sometimes" })),
            (DEBUG_EVALUATE, json!({ "session_id": 1, "expression": "" })),
            (
                DEBUG_BREAKPOINT_ADD,
                json!({ "session_id": 1, "location": { "script": "package:app/main.dart" } }),
            ),
            ("dartq/debug/nope", json!({})),
        ] {
            let error = handle(&mut manager, method, &params, root)
                .err()
                .unwrap_or_else(|| panic!("{method} must reject these params"));
            let (code, _) = error.wire();
            assert_eq!(code, codes::INVALID_PARAMS, "{method}");
        }
    }

    /// #2619: the internal shutdown face is serve-loop property, never
    /// a wire face — `is_request` (the intake gate) and the served
    /// family both refuse it — and an empty registry releases zero.
    #[test]
    fn the_internal_shutdown_face_is_not_wire_reachable_and_releases_zero_when_empty() {
        assert!(!is_request(DEBUG_SHUTDOWN), "the shutdown face is serve-loop only");
        assert!(!DEBUG_REQUESTS.contains(&DEBUG_SHUTDOWN));
        let live = Arc::new(AtomicUsize::new(0));
        let mut manager = DebugManager::sharing(Arc::clone(&live));
        let answer =
            handle(&mut manager, DEBUG_SHUTDOWN, &json!({}), Path::new("/workspace"))
                .expect("shutdown answers");
        assert_eq!(answer, json!({"released": 0}));
        assert_eq!(live.load(Ordering::Relaxed), 0, "the counter stays in step");
    }

    /// #2661: `sessions_released` counts only sessions this daemon
    /// actually detaches — an empty registry answers zero even with an
    /// orphan manifest on disk (an earlier daemon's trail), and the
    /// ladder never rewrites that trail: only a held session's own
    /// manifest is marked released.
    #[test]
    fn shutdown_releases_zero_and_never_touches_orphan_manifests() {
        let root = std::env::temp_dir().join(format!("dqd-shutdown-orphans-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = manifests_dir(&root);
        write_manifest(
            &dir,
            4,
            &SessionManifest {
                ws_uri: "ws://left-behind".into(),
                kind: "launch".into(),
                since: 1_700_000_000,
                pid: Some(111),
                released: None,
            },
        );
        let mut manager = DebugManager::sharing(Arc::new(AtomicUsize::new(0)));
        let answer =
            handle(&mut manager, DEBUG_SHUTDOWN, &json!({}), &root).expect("shutdown answers");
        assert_eq!(answer, json!({"released": 0}), "no live session, nothing released");
        let trail = load_manifests(&dir);
        assert_eq!(trail.len(), 1, "the orphan trail stays");
        assert_eq!(trail[0].1.released, None, "an untouched manifest is never marked released");
        std::fs::remove_dir_all(&root).ok();
    }

    /// #2661: a frame-indexed evaluate against a paused isolate with
    /// an empty stack answers the machine-coded `NO_FRAME` condition —
    /// not the VM's generic compile-error text — with the way out
    /// named in the message.
    #[test]
    fn the_no_frame_miss_names_the_condition_and_the_way_out() {
        let (code, message) = DebugError::NoFrame(0).wire();
        assert_eq!(code, codes::DEBUG_SESSION);
        assert!(
            message.starts_with("DARTQ/DEBUG/NO_FRAME: the paused isolate has no frame at index 0;"),
            "{message}"
        );
        assert!(
            message.contains("pause at a breakpoint or evaluate in the root library"),
            "the message names the way out: {message}"
        );
    }

    /// #2622: the `reload` answer reports its own facts — the isolate
    /// count and the wall cost on success, and a failure as `err` inside
    /// the same document (the session survives a failed reload; a wire
    /// error would imply otherwise).
    #[test]
    fn the_reload_answer_reports_count_wall_ms_and_failures() {
        let success = reload_row(Instant::now(), Ok(true));
        assert_eq!(success["ok"], json!(true));
        assert_eq!(success["isolates_reloaded"], json!(1));
        assert!(success["ms"].as_u64().is_some(), "the wall cost rides: {success}");
        assert!(success.get("err").is_none(), "no failure, no err: {success}");

        let reported = reload_row(Instant::now(), Ok(false));
        assert_eq!(reported["ok"], json!(false));
        assert_eq!(reported["isolates_reloaded"], json!(0));
        assert!(
            reported["err"].as_str().is_some_and(|err| !err.is_empty()),
            "a reported failure names itself: {reported}"
        );

        let failed = reload_row(
            Instant::now(),
            Err(dartq_debug::Error::Rpc {
                code: 108,
                message: "cannot reload".into(),
                details: Some("expected a library".into()),
            }),
        );
        assert_eq!(failed["ok"], json!(false));
        assert_eq!(failed["isolates_reloaded"], json!(0));
        let err = failed["err"].as_str().expect("the VM's own text");
        assert!(
            err.contains("cannot reload") && err.contains("108"),
            "the machine-coded VM failure rides verbatim: {err}"
        );
    }

    /// #2619: the orphan manifest trail round-trips on disk — written
    /// with only the fields it has (pid/released serialize when
    /// present), rewritten `released: true` by a graceful shutdown,
    /// consumed by a re-attach to the same ws_uri, deleted at a
    /// client-requested end.
    #[test]
    fn manifest_trail_round_trips_release_consumption_and_deletion() {
        let dir = std::env::temp_dir().join(format!("dqd-manifests-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let manifest = |ws_uri: &str, pid: Option<u32>| SessionManifest {
            ws_uri: ws_uri.to_owned(),
            kind: "launch".to_owned(),
            since: unix_s(),
            pid,
            released: None,
        };

        write_manifest(&dir, 1, &manifest("ws://a", Some(4242)));
        write_manifest(&dir, 2, &manifest("ws://b", None));
        let loaded = load_manifests(&dir);
        assert_eq!(loaded.len(), 2, "both trails load, ids ascending");
        assert_eq!(loaded[0].0, 1);
        assert_eq!(loaded[0].1.ws_uri, "ws://a");
        assert_eq!(loaded[0].1.pid, Some(4242));
        // The optionals ride the file only when present.
        let launched = std::fs::read_to_string(dir.join("1.json")).expect("manifest");
        assert!(launched.contains("\"pid\":4242"), "{launched}");
        let attached = std::fs::read_to_string(dir.join("2.json")).expect("manifest");
        assert!(!attached.contains("pid") && !attached.contains("released"), "{attached}");

        // The graceful-shutdown rewrite: released true, the rest intact.
        mark_released(&dir, 1);
        let released = &load_manifests(&dir)[0].1;
        assert_eq!(released.released, Some(true));
        assert_eq!(released.ws_uri, "ws://a");
        assert_eq!(released.pid, Some(4242));

        // The re-attach consumption: the fresh session's own manifest
        // stays, the stale trail naming the same ws_uri goes.
        write_manifest(&dir, 7, &manifest("ws://a", None));
        consume_matching(&dir, "ws://a", 7);
        let ids: Vec<u32> = load_manifests(&dir).into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, [2, 7], "only the unrelated trail and the fresh one remain");

        // The client-requested end deletes with the session.
        delete_manifest(&dir, 7);
        let ids: Vec<u32> = load_manifests(&dir).into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, [2]);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// #2619: `status` always answers a document — even both-empty is a
    /// printed document (never blank output for a machine face) — and a
    /// seeded trail lists as orphans: id, ws_uri, kind, since, and
    /// `released` only where the daemon that held it released the app.
    #[test]
    fn status_answers_the_document_and_lists_orphans() {
        let root = std::env::temp_dir().join(format!("dqd-orphans-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("dir");
        let mut manager = DebugManager::sharing(Arc::new(AtomicUsize::new(0)));

        // Empty registry, empty trail: the document, not `[]`.
        let answer = handle(&mut manager, DEBUG_STATUS, &json!({}), &root).expect("status");
        assert_eq!(answer, json!({"v": 1, "sessions": [], "orphans": []}));

        let dir = manifests_dir(&root);
        write_manifest(
            &dir,
            3,
            &SessionManifest {
                ws_uri: "ws://released".into(),
                kind: "attach".into(),
                since: 1_700_000_000,
                pid: None,
                released: Some(true),
            },
        );
        write_manifest(
            &dir,
            5,
            &SessionManifest {
                ws_uri: "ws://held".into(),
                kind: "launch".into(),
                since: 1_700_000_060,
                pid: Some(99),
                released: None,
            },
        );
        let answer = handle(&mut manager, DEBUG_STATUS, &json!({}), &root).expect("status");
        assert_eq!(
            answer["orphans"],
            json!([
                {"id": 3, "ws_uri": "ws://released", "kind": "attach",
                 "since": 1_700_000_000, "released": true},
                {"id": 5, "ws_uri": "ws://held", "kind": "launch",
                 "since": 1_700_000_060},
            ]),
            "the pid stays file-side; the row is the re-attach trail"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
