//! The `dartq/debug/*` face: launch/attach debugging sessions over the
//! daemon's RPC surface and drive them — breakpoints, stepping, stopped
//! snapshots with variables, and cursor-paged event/output reads.
//!
//! The face is a registry problem, not an analysis problem: every request
//! targets one of the session's live [`DebugManager`] entries and never
//! touches the analysis engine, so the server answers these methods at
//! intake (the overlay faces' posture) instead of staging them onto the
//! read scheduler. The underlying [`dartq_debug::DebugSession`] calls are
//! synchronous — child-process spawn, WebSocket round trips, and bounded
//! waits — and run inline on the main loop; see the server's dispatch
//! notes for the blocking posture.
//!
//! dartq-debug's session types carry no serde derives by design (the
//! crate keeps its wire face at the VM protocol level), so this module
//! owns the daemon-side DTO shapers — pure functions over the session
//! types, unit-testable without a live VM. The protocol-level types it
//! re-exports (`Event` and friends) are serde-derived and embed directly.

use std::path::{Path, PathBuf};

use dartq_debug::{
    BreakpointHandle, BreakpointLocationSpec, BreakpointSpec, DebugSession, EventBatch,
    ExceptionPauseMode, FrameLocation, FrameSummary, LaunchConfig, OutputBatch, SessionEvent,
    StepOption, StoppedReason, StoppedSnapshot, Variable,
};
use rustc_hash::FxHashMap;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::jsonrpc::codes;

/// The `dartq/debug/*` method names — the debug family's closed request
/// set. `pub` so the drift gate (tests/rpc_schema.rs) can pin the served
/// family as one fact with the dispatch table below.
pub const DEBUG_LAUNCH: &str = "dartq/debug/launch";
pub const DEBUG_ATTACH: &str = "dartq/debug/attach";
pub const DEBUG_DETACH: &str = "dartq/debug/detach";
pub const DEBUG_STATUS: &str = "dartq/debug/status";
pub const DEBUG_BREAKPOINT_ADD: &str = "dartq/debug/breakpointAdd";
pub const DEBUG_BREAKPOINT_REMOVE: &str = "dartq/debug/breakpointRemove";
pub const DEBUG_EXCEPTION_PAUSE: &str = "dartq/debug/exceptionPause";
pub const DEBUG_RESUME: &str = "dartq/debug/resume";
pub const DEBUG_PAUSE: &str = "dartq/debug/pause";
pub const DEBUG_STEP: &str = "dartq/debug/step";
pub const DEBUG_SNAPSHOT: &str = "dartq/debug/snapshot";
pub const DEBUG_EVENTS: &str = "dartq/debug/events";
pub const DEBUG_OUTPUT: &str = "dartq/debug/output";

/// Every `dartq/debug/*` request name — the family the server intercepts
/// at intake ahead of the analysis-command arm.
pub const DEBUG_REQUESTS: [&str; 13] = [
    DEBUG_LAUNCH,
    DEBUG_ATTACH,
    DEBUG_DETACH,
    DEBUG_STATUS,
    DEBUG_BREAKPOINT_ADD,
    DEBUG_BREAKPOINT_REMOVE,
    DEBUG_EXCEPTION_PAUSE,
    DEBUG_RESUME,
    DEBUG_PAUSE,
    DEBUG_STEP,
    DEBUG_SNAPSHOT,
    DEBUG_EVENTS,
    DEBUG_OUTPUT,
];

/// Whether `method` is one of the debug faces this module serves.
#[must_use]
pub(crate) fn is_request(method: &str) -> bool {
    DEBUG_REQUESTS.contains(&method)
}

/// A debug face failure, mapped to the wire by [`DebugError::wire`].
#[derive(Debug)]
pub(crate) enum DebugError {
    /// A structural parameter mismatch — the `INVALID_PARAMS` tier every
    /// other face uses.
    InvalidParams(String),
    /// The request named a session id that is not live.
    NoSuchSession(u32),
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
            Self::Vm(error) => (codes::DEBUG_SESSION, format!("DARTQ/DEBUG/VM: {error}")),
        }
    }
}

/// How a managed session came to be — the `status` face's origin field.
enum Origin {
    /// A launched `dart <script>` (the script path as requested).
    Launch(PathBuf),
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
/// daemon [`Session`] and is touched only from the main loop, like
/// every other piece of it.
#[derive(Default)]
pub(crate) struct DebugManager {
    sessions: FxHashMap<u32, ManagedSession>,
    next_id: u32,
}

impl DebugManager {
    /// Registers a live session under the next monotonic id.
    fn register(&mut self, origin: Origin, session: DebugSession) -> u32 {
        self.next_id += 1;
        self.sessions.insert(self.next_id, ManagedSession { origin, session });
        self.next_id
    }

    /// The named session, or the machine-coded miss.
    fn get(&mut self, id: u32) -> Result<&mut ManagedSession, DebugError> {
        self.sessions.get_mut(&id).ok_or(DebugError::NoSuchSession(id))
    }
}

// The faces' parameter payloads (`#[serde(default)]` keeps every field
// optional beyond the contract-critical ones, the analysis faces'
// convention).

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LaunchParams {
    script: PathBuf,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    vm_flags: Vec<String>,
    #[serde(default = "default_pause_on_start")]
    pause_on_start: bool,
    cwd: Option<PathBuf>,
}

/// The AI-first default: pause at start, so breakpoints arm before the
/// program runs.
fn default_pause_on_start() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachParams {
    ws_uri: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionParams {
    session_id: u32,
}

/// The `status` payload: `sessionId` optional — absent lists every
/// session.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusParams {
    #[serde(default)]
    session_id: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IsolateParams {
    session_id: u32,
    #[serde(default)]
    isolate: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BreakpointAddParams {
    session_id: u32,
    #[serde(default)]
    isolate: Option<String>,
    location: BreakpointLocationParams,
    #[serde(default)]
    condition: Option<String>,
}

/// One breakpoint target, mirroring [`BreakpointLocationSpec`]: a source
/// line (`script` + `line`, optional `column`) or a function entry
/// (`functionId`).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
struct BreakpointRemoveParams {
    session_id: u32,
    breakpoint_id: String,
    #[serde(default)]
    isolate: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExceptionPauseParams {
    session_id: u32,
    #[serde(default)]
    isolate: Option<String>,
    mode: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StepParams {
    session_id: u32,
    #[serde(default)]
    isolate: Option<String>,
    option: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotParams {
    session_id: u32,
    isolate: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventsParams {
    session_id: u32,
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
#[serde(rename_all = "camelCase")]
struct OutputParams {
    session_id: u32,
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

/// Relative request paths resolve against the session workspace root (the
/// analysis faces' rule).
fn resolve(root: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

/// Answers one `dartq/debug/*` request against the session's registry.
/// Every session call is synchronous (a child process, WebSocket round
/// trips, bounded waits) and runs inline on the caller's thread; the
/// waits are bounded by dartq-debug's own `SessionConfig` (30 s banner,
/// 30 s per RPC, 30 s pause waits), so no call blocks forever.
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
            let mut config = LaunchConfig::new(resolve(root, decoded.script.clone()));
            config.args = decoded.args;
            config.vm_flags = decoded.vm_flags;
            config.pause_on_start = decoded.pause_on_start;
            config.cwd = decoded.cwd.map(|cwd| resolve(root, cwd));
            let session = DebugSession::launch(config).map_err(DebugError::Vm)?;
            let id = manager.register(Origin::Launch(decoded.script), session);
            Ok(session_opened_row(id, &manager.get(id).expect("just registered").session))
        }
        DEBUG_ATTACH => {
            let decoded =
                serde_json::from_value::<AttachParams>(params.clone()).map_err(invalid)?;
            if decoded.ws_uri.is_empty() {
                return Err(DebugError::InvalidParams("wsUri must be a non-empty string".into()));
            }
            let session = DebugSession::attach(&decoded.ws_uri).map_err(DebugError::Vm)?;
            let id = manager.register(Origin::Attach(decoded.ws_uri), session);
            Ok(session_opened_row(id, &manager.get(id).expect("just registered").session))
        }
        DEBUG_DETACH => {
            let decoded =
                serde_json::from_value::<SessionParams>(params.clone()).map_err(invalid)?;
            // Removing drops the session; its Drop tears the child and
            // the connection down.
            manager
                .sessions
                .remove(&decoded.session_id)
                .ok_or(DebugError::NoSuchSession(decoded.session_id))?;
            Ok(json!({ "ok": true }))
        }
        DEBUG_STATUS => {
            let decoded =
                serde_json::from_value::<StatusParams>(params.clone()).map_err(invalid)?;
            match decoded.session_id {
                None => {
                    // Deterministic order: ids ascending, regardless of
                    // the map's layout.
                    let mut ids: Vec<u32> = manager.sessions.keys().copied().collect();
                    ids.sort_unstable();
                    Ok(Value::Array(
                        ids.into_iter()
                            .map(|id| {
                                let managed = manager.get(id).expect("keyed id");
                                status_row(id, managed)
                            })
                            .collect(),
                    ))
                }
                Some(id) => {
                    let managed = manager.get(id)?;
                    let mut row = status_row(id, managed);
                    row["breakpoints"] = Value::Array(
                        managed.session.breakpoints().map(breakpoint_row).collect(),
                    );
                    Ok(row)
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
                             {functionId} (function entry)"
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
            let managed = manager.get(decoded.session_id)?;
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
            let managed = manager.get(decoded.session_id)?;
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
            let managed = manager.get(decoded.session_id)?;
            managed
                .session
                .set_exception_pause_mode(decoded.isolate.as_deref(), mode)
                .map_err(DebugError::Vm)?;
            Ok(json!({ "ok": true }))
        }
        DEBUG_RESUME | DEBUG_PAUSE => {
            let decoded =
                serde_json::from_value::<IsolateParams>(params.clone()).map_err(invalid)?;
            let managed = manager.get(decoded.session_id)?;
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
            let managed = manager.get(decoded.session_id)?;
            let snapshot = managed
                .session
                .step_and_package(decoded.isolate.as_deref(), Some(option))
                .map_err(DebugError::Vm)?;
            Ok(snapshot_row(&snapshot))
        }
        DEBUG_SNAPSHOT => {
            let decoded =
                serde_json::from_value::<SnapshotParams>(params.clone()).map_err(invalid)?;
            let managed = manager.get(decoded.session_id)?;
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
            let managed = manager.get(decoded.session_id)?;
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
            let managed = manager.get(decoded.session_id)?;
            managed.session.poll_events().map_err(DebugError::Vm)?;
            let batch = if stderr {
                managed.session.stderr_since(decoded.cursor, decoded.max_bytes)
            } else {
                managed.session.stdout_since(decoded.cursor, decoded.max_bytes)
            };
            Ok(output_row(&batch))
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
        "sessionId": id,
        "isolateIds": user_isolate_ids(session),
        "mainIsolate": session.main_isolate(),
    })
}

/// One `status` row: the origin (script or wsUri), the isolate set, and
/// the breakpoint tally.
fn status_row(id: u32, managed: &ManagedSession) -> Value {
    let session = &managed.session;
    let mut row = json!({
        "sessionId": id,
        "mainIsolate": session.main_isolate(),
        "isolateIds": user_isolate_ids(session),
        "breakpointCount": session.breakpoints().count(),
    });
    match &managed.origin {
        Origin::Launch(script) => row["script"] = json!(script.display().to_string()),
        Origin::Attach(uri) => row["wsUri"] = json!(uri),
    }
    row
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
        "atAsyncSuspension": snapshot.at_async_suspension,
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
            json!({ "kind": "conditionalResumeLimit", "autoResumes": auto_resumes })
        }
        StoppedReason::Other(kind) => json!({ "kind": "other", "eventKind": kind.as_str() }),
    }
}

/// A frame position.
fn frame_location_row(location: &FrameLocation) -> Value {
    json!({
        "line": location.line,
        "column": location.column,
        "scriptUri": location.script_uri,
        "scriptId": location.script_id,
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
        "nextCursor": batch.next_cursor,
        "dropped": batch.dropped,
    })
}

/// One recorded stream event.
fn event_row(event: &SessionEvent) -> Value {
    let mut row = json!({
        "cursor": event.cursor,
        "streamId": event.stream_id,
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
        "nextCursor": batch.next_cursor,
        "dropped": batch.dropped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dartq_debug::Event;

    /// The `launch` payload decodes with its AI-first defaults and
    /// rejects a missing script (the only required field).
    #[test]
    fn launch_params_decode_with_defaults_and_reject_missing_script() {
        let decoded: LaunchParams =
            serde_json::from_value(json!({ "script": "bin/main.dart" })).expect("decodes");
        assert_eq!(decoded.script, PathBuf::from("bin/main.dart"));
        assert!(decoded.args.is_empty());
        assert!(decoded.vm_flags.is_empty());
        assert!(decoded.pause_on_start);
        assert!(decoded.cwd.is_none());

        let decoded: LaunchParams = serde_json::from_value(json!({
            "script": "bin/main.dart",
            "args": ["--fast"],
            "vmFlags": ["--enable-asserts"],
            "pauseOnStart": false,
            "cwd": "bin"
        }))
        .expect("decodes");
        assert_eq!(decoded.args, ["--fast"]);
        assert_eq!(decoded.vm_flags, ["--enable-asserts"]);
        assert!(!decoded.pause_on_start);
        assert_eq!(decoded.cwd, Some(PathBuf::from("bin")));

        assert!(serde_json::from_value::<LaunchParams>(json!({ "args": [] })).is_err());
        assert!(serde_json::from_value::<LaunchParams>(json!({ "script": 7 })).is_err());
    }

    /// A breakpoint location decodes in both shapes — the line form and
    /// the function-entry form — and the half-filled line form is the
    /// explicit rejection (a script without a line arms nothing).
    #[test]
    fn breakpoint_location_params_decode_line_and_function_shapes() {
        let line: BreakpointAddParams = serde_json::from_value(json!({
            "sessionId": 1,
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
            "sessionId": 1,
            "location": { "functionId": "functions/42" }
        }))
        .expect("decodes");
        assert_eq!(function.location.function_id.as_deref(), Some("functions/42"));
        assert!(function.location.script.is_none());
        assert_eq!(function.location.line, None);

        // A script without a line decodes (the fields are individually
        // optional) — the half-filled line form is rejected at dispatch,
        // below, where the location is assembled.
        let half: BreakpointAddParams = serde_json::from_value(json!({
            "sessionId": 1,
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
            serde_json::from_value(json!({ "sessionId": 1 })).expect("decodes");
        assert_eq!(events.cursor, 0);
        assert_eq!(events.limit, default_event_limit());

        let output: OutputParams =
            serde_json::from_value(json!({ "sessionId": 1, "stream": "stdout" }))
                .expect("decodes");
        assert_eq!(output.cursor, 0);
        assert_eq!(output.max_bytes, default_max_bytes());
        assert!(
            serde_json::from_value::<OutputParams>(json!({ "sessionId": 1, "stream": "stdin" }))
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
            assert_eq!(row["location"]["scriptUri"], "package:app/main.dart");
            assert_eq!(row["stack"][0]["index"], 0);
            assert_eq!(row["stack"][0]["location"]["function"], "main");
            assert_eq!(row["variables"][0]["name"], "count");
            assert_eq!(row["variables"][0]["value"], "42");
            assert_eq!(row["variables"][0]["handle"], "objects/9");
            assert_eq!(row["atAsyncSuspension"], true);
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
        assert_eq!(other["reason"]["eventKind"], "WriteEvent");
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
        assert_eq!(row["streamId"], "Debug");
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
        assert_eq!(row["nextCursor"], 9);
        assert_eq!(row["dropped"], 2);
    }

    /// The dispatch wiring is hermetically exercisable without a live
    /// session: the empty registry lists empty, every sessionId-bearing
    /// face answers the machine-coded miss, and bad params answer the
    /// shared `INVALID_PARAMS` tier.
    #[test]
    fn dispatch_answers_unknown_session_and_empty_status() {
        let mut manager = DebugManager::default();
        let root = Path::new("/workspace");

        let answer = handle(&mut manager, DEBUG_STATUS, &json!({}), root).expect("status");
        assert_eq!(answer, json!([]));

        for (method, params) in [
            (DEBUG_DETACH, json!({ "sessionId": 1 })),
            (DEBUG_RESUME, json!({ "sessionId": 1 })),
            (DEBUG_PAUSE, json!({ "sessionId": 1, "isolate": "isolates/1" })),
            (DEBUG_SNAPSHOT, json!({ "sessionId": 1, "isolate": "isolates/1" })),
            (DEBUG_EVENTS, json!({ "sessionId": 1 })),
            (DEBUG_OUTPUT, json!({ "sessionId": 1, "stream": "stdout" })),
            (DEBUG_BREAKPOINT_REMOVE, json!({ "sessionId": 1, "breakpointId": "breakpoints/1" })),
            (DEBUG_EXCEPTION_PAUSE, json!({ "sessionId": 1, "mode": "none" })),
            (DEBUG_STEP, json!({ "sessionId": 1, "option": "into" })),
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
        // word, an empty wsUri, and a half-filled line location (a script
        // without a line names no position).
        for (method, params) in [
            (DEBUG_LAUNCH, json!({})),
            (DEBUG_STEP, json!({ "sessionId": 1, "option": "sideways" })),
            (DEBUG_OUTPUT, json!({ "sessionId": 1, "stream": "stdin" })),
            (DEBUG_ATTACH, json!({ "wsUri": "" })),
            (DEBUG_EXCEPTION_PAUSE, json!({ "sessionId": 1, "mode": "sometimes" })),
            (
                DEBUG_BREAKPOINT_ADD,
                json!({ "sessionId": 1, "location": { "script": "package:app/main.dart" } }),
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
}
