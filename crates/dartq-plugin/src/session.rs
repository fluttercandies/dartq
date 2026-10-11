//! The core-side plugin session: handshake state machine as pure functions
//! over frames (testable without spawning any process).
//!
//! Lifecycle (PRD §14, wire contract v1):
//!
//! 1. Core sends `initialize` (its protocol version + supported capabilities).
//! 2. Plugin replies with its version + advertised capabilities. The reply
//!    version must equal [`PROTOCOL_VERSION`], else the session fails with
//!    [`SessionError::VersionMismatch`]. The effective capability set is the
//!    **intersection** of both sides.
//! 3. While `Ready`: the plugin may send `notification/diagnostic` frames and
//!    the core may send `request/fixes` (only if `fix` was negotiated).
//! 4. Core sends `shutdown`; the plugin's reply moves the session to
//!    [`SessionState::Terminated`].
//!
//! Decisions pinned here (documented in the crate docs as the wire contract):
//!
//! - A diagnostic notification **before** initialize completes is rejected
//!   with [`SessionError::UnexpectedFrame`] — never queued. Queueing would
//!   hide a protocol violation and add an unbounded buffer; the host tears
//!   the plugin down instead (PRD §14.3).
//! - A diagnostic arriving while the shutdown reply is awaited
//!   ([`SessionState::AwaitingShutdownReply`]) is queued, not rejected: the
//!   host wrote it before it could have seen the shutdown request, so it is
//!   legitimate in-flight traffic crossing the request on the wire. After
//!   the reply ([`SessionState::Terminated`]) a diagnostic is a violation
//!   again.
//! - Any `Err` from [`Session::handle`] is fatal: the caller must discard the
//!   session and kill the plugin process. The session never resynchronizes.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::capability::{Capability, CapabilitySet};
use crate::frame::{
    Frame, METHOD_DIAGNOSTIC_NOTIFICATION, METHOD_ENGINE_DIAGNOSTICS, METHOD_INITIALIZE,
    METHOD_REQUEST_FIXES, METHOD_SHUTDOWN, PROTOCOL_VERSION,
};

/// Session lifecycle states, driven by [`Session::handle`] and the
/// `send_*` builders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    /// Created; `initialize` has not been sent yet.
    Fresh,
    /// `initialize` has been sent; its reply is awaited.
    AwaitingInitializeReply,
    /// Handshake complete; work requests and diagnostics may flow.
    Ready,
    /// `shutdown` has been sent; its reply is awaited.
    AwaitingShutdownReply,
    /// Shutdown reply received; the session is finished.
    Terminated,
}

impl SessionState {
    /// Stable name used in error messages.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            SessionState::Fresh => "Fresh",
            SessionState::AwaitingInitializeReply => "AwaitingInitializeReply",
            SessionState::Ready => "Ready",
            SessionState::AwaitingShutdownReply => "AwaitingShutdownReply",
            SessionState::Terminated => "Terminated",
        }
    }
}

/// What one inbound frame did to the session.
#[derive(Clone, Debug, PartialEq)]
pub enum Incoming {
    /// The `initialize` reply was accepted; carries the effective
    /// (intersected) capability set.
    Initialized(CapabilitySet),
    /// A `notification/diagnostic` produced by the plugin.
    Diagnostic(PluginDiagnostic),
    /// The reply to a `request/fixes`; carries the correlation id the
    /// runtime awaits on.
    Fixes {
        /// The request id this reply completes.
        id: u64,
        /// The parsed reply payload.
        reply: FixesReply,
    },
    /// The `shutdown` reply was accepted; the session is over.
    ShutdownComplete,
}

/// Session-logic failures (codec failures surface earlier, at
/// [`crate::decode_line`]). Any variant is fatal for the session.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SessionError {
    /// The frame's method is not part of protocol v1.
    #[error("unknown method `{method}` in state {state}")]
    UnknownMethod {
        /// The offending method name.
        method: String,
        /// The state the frame arrived in.
        state: &'static str,
    },
    /// The plugin answered `initialize` with a foreign protocol version.
    #[error("plugin speaks protocol version {got}; core supports {supported}")]
    VersionMismatch {
        /// The version the plugin replied with.
        got: u32,
        /// The version the core speaks ([`PROTOCOL_VERSION`]).
        supported: u32,
    },
    /// A notification carried an id; notifications have no reply channel.
    #[error("`notification/diagnostic` frames must not carry an id")]
    NotificationWithId,
    /// A syntactically valid frame that the state machine does not accept.
    #[error("frame `{method}` is not valid in state {state}")]
    UnexpectedFrame {
        /// The frame's method.
        method: String,
        /// The state the frame arrived in.
        state: &'static str,
    },
    /// A reply whose id matches no request the core has outstanding.
    #[error("reply id {id} matches no outstanding request")]
    UnknownReplyId {
        /// The unmatched id.
        id: u64,
    },
    /// A `request/fixes` was attempted without a negotiated `fix` capability.
    #[error("`request/fixes` requires the `{0}` capability, which was not negotiated")]
    CapabilityNotNegotiated(Capability),
    /// The plugin answered a request with an error payload
    /// (`params.error`).
    #[error("plugin returned an error for `{method}`: {message}")]
    RemoteError {
        /// The request method that failed remotely.
        method: String,
        /// The plugin's error message.
        message: String,
    },
    /// A known method with a payload that does not match the wire schema.
    #[error("invalid `{method}` payload: {reason}")]
    InvalidPayload {
        /// The method whose payload failed to parse.
        method: String,
        /// What was wrong (includes the offending field name).
        reason: String,
    },
}

/// A diagnostic flowing over the wire. Plugin-emitted diagnostics ride
/// `notification/diagnostic` (plugin → core); engine diagnostics are pushed
/// as `notification/engine_diagnostics` batches (core → plugin), so both
/// directions share this one row shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginDiagnostic {
    /// Path of the file the diagnostic refers to.
    pub file: String,
    /// Diagnostic severity.
    pub severity: Severity,
    /// The plugin's diagnostic code (e.g. `my_plugin/avoid_sync_io`).
    pub code: String,
    /// Human-readable message.
    pub message: String,
    /// UTF-16 offset of the highlighted range; defaults to 0.
    #[serde(default)]
    pub offset: u64,
    /// Length of the highlighted range in UTF-16 units; defaults to 0.
    #[serde(default)]
    pub length: u64,
}

/// Severity vocabulary of plugin diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// An error diagnostic.
    Error,
    /// A warning diagnostic.
    Warning,
    /// An informational diagnostic.
    Info,
}

/// Builds the core → plugin `notification/engine_diagnostics` batch carrying
/// the engine's current diagnostics (one frame per analyze cycle). A
/// notification: the plugin never replies to it.
#[must_use]
pub fn engine_diagnostics_frame(diagnostics: &[PluginDiagnostic]) -> Frame {
    let mut params = Map::new();
    params.insert("diagnostics".into(), json!(diagnostics));
    Frame::notification(METHOD_ENGINE_DIAGNOSTICS, params)
}

/// The reply payload of `request/fixes`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct FixesReply {
    /// Fixes applicable to the requested location, best first.
    pub fixes: Vec<PrioritizedChange>,
}

/// One proposed source change with a priority ordering.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct PrioritizedChange {
    /// Lower priority numbers sort first (analyzer convention).
    pub priority: i32,
    /// Human-readable title of the change.
    pub message: String,
    /// The concrete edits; empty for a message-only suggestion.
    #[serde(default)]
    pub edits: Vec<SourceEdit>,
}

/// One contiguous text replacement.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SourceEdit {
    /// UTF-16 offset the replacement starts at.
    pub offset: u64,
    /// UTF-16 length being replaced.
    pub length: u64,
    /// The replacement text.
    #[serde(default)]
    pub replacement: String,
}

/// The core side of one plugin session. Pure state over frames: no I/O, no
/// threads — the caller owns the transport.
#[derive(Debug)]
pub struct Session {
    state: SessionState,
    local: CapabilitySet,
    negotiated: Option<CapabilitySet>,
    outstanding: Vec<u64>,
    next_id: u64,
}

impl Session {
    /// Creates a session advertising `local` capabilities to the plugin.
    #[must_use]
    pub fn new(local: CapabilitySet) -> Self {
        Self {
            state: SessionState::Fresh,
            local,
            negotiated: None,
            outstanding: Vec::new(),
            next_id: 1,
        }
    }

    /// The current lifecycle state.
    #[must_use]
    pub const fn state(&self) -> SessionState {
        self.state
    }

    /// The effective capability set once the handshake completed.
    #[must_use]
    pub const fn negotiated(&self) -> Option<CapabilitySet> {
        self.negotiated
    }

    /// Builds the `initialize` request and moves to
    /// [`SessionState::AwaitingInitializeReply`].
    ///
    /// # Errors
    ///
    /// [`SessionError::UnexpectedFrame`] unless the session is
    /// [`SessionState::Fresh`] — the handshake request is sent exactly once.
    pub fn send_initialize(&mut self) -> Result<Frame, SessionError> {
        if self.state != SessionState::Fresh {
            return Err(SessionError::UnexpectedFrame {
                method: METHOD_INITIALIZE.to_owned(),
                state: self.state.as_str(),
            });
        }
        let id = self.take_id();
        let mut params = Map::new();
        params.insert("protocol_version".into(), json!(PROTOCOL_VERSION));
        params.insert("capabilities".into(), json!(self.local.names()));
        self.outstanding.push(id);
        self.state = SessionState::AwaitingInitializeReply;
        Ok(Frame::request(id, METHOD_INITIALIZE, params))
    }

    /// Builds the `shutdown` request and moves to
    /// [`SessionState::AwaitingShutdownReply`].
    ///
    /// # Errors
    ///
    /// [`SessionError::UnexpectedFrame`] unless the session is [`SessionState::Ready`].
    pub fn send_shutdown(&mut self) -> Result<Frame, SessionError> {
        if self.state != SessionState::Ready {
            return Err(SessionError::UnexpectedFrame {
                method: METHOD_SHUTDOWN.to_owned(),
                state: self.state.as_str(),
            });
        }
        let id = self.take_id();
        self.outstanding.push(id);
        self.state = SessionState::AwaitingShutdownReply;
        Ok(Frame::request(id, METHOD_SHUTDOWN, Map::new()))
    }

    /// Builds a `request/fixes` for `file` at the given UTF-16 range.
    ///
    /// # Errors
    ///
    /// [`SessionError::CapabilityNotNegotiated`] unless the session is ready
    /// and the `fix` capability was negotiated; [`SessionError::UnexpectedFrame`]
    /// in any other non-ready state.
    pub fn send_fixes_request(
        &mut self,
        file: &str,
        offset: u64,
        length: u64,
    ) -> Result<Frame, SessionError> {
        if self.state != SessionState::Ready {
            return Err(SessionError::UnexpectedFrame {
                method: METHOD_REQUEST_FIXES.to_owned(),
                state: self.state.as_str(),
            });
        }
        let Some(negotiated) = self.negotiated else {
            return Err(SessionError::CapabilityNotNegotiated(Capability::Fix));
        };
        if !negotiated.contains(Capability::Fix) {
            return Err(SessionError::CapabilityNotNegotiated(Capability::Fix));
        }
        let id = self.take_id();
        let mut params = Map::new();
        params.insert("file".into(), json!(file));
        params.insert("offset".into(), json!(offset));
        params.insert("length".into(), json!(length));
        self.outstanding.push(id);
        Ok(Frame::request(id, METHOD_REQUEST_FIXES, params))
    }

    /// Feeds one inbound (already decoded) frame into the machine.
    ///
    /// # Errors
    ///
    /// Fatal and typed for every violated contract clause: unknown method,
    /// version mismatch, frame not valid for the current state, unmatched
    /// reply id, missing capability, remote error payload, malformed payload.
    pub fn handle(&mut self, frame: Frame) -> Result<Incoming, SessionError> {
        match frame.id {
            Some(id) => self.handle_reply(frame, id),
            None => self.handle_notification(frame),
        }
    }

    fn handle_notification(&mut self, mut frame: Frame) -> Result<Incoming, SessionError> {
        let state = self.state.as_str();
        match method_kind(&frame.method) {
            MethodKind::Diagnostic => {
                // Diagnostics are legal once the handshake completed. They
                // also ride out the shutdown window: a diagnostic the host
                // wrote before it saw the shutdown request legitimately
                // crosses that request on the wire, and the request-send
                // flip is local — unobservable to the host — so rejecting
                // it would manufacture a crash out of in-flight traffic
                // (the official host tolerates the same window: it sends
                // `plugin.shutdown` and simply waits out the reply). After
                // the reply (`Terminated`) the reader is gone; a diagnostic
                // there is a violation again.
                match self.state {
                    SessionState::Ready | SessionState::AwaitingShutdownReply => {
                        Ok(Incoming::Diagnostic(parse_payload(
                            &frame.method,
                            std::mem::take(&mut frame.params),
                        )?))
                    }
                    _ => Err(SessionError::UnexpectedFrame {
                        method: frame.method,
                        state,
                    }),
                }
            }
            // A request method sent without an id, in any state.
            MethodKind::Initialize | MethodKind::Shutdown | MethodKind::Fixes => {
                Err(SessionError::UnexpectedFrame {
                    method: frame.method,
                    state,
                })
            }
            MethodKind::Unknown => Err(SessionError::UnknownMethod {
                method: frame.method,
                state,
            }),
        }
    }

    fn handle_reply(&mut self, mut frame: Frame, id: u64) -> Result<Incoming, SessionError> {
        let state = self.state.as_str();
        let kind = method_kind(&frame.method);
        match kind {
            MethodKind::Initialize | MethodKind::Shutdown | MethodKind::Fixes => {}
            MethodKind::Diagnostic => return Err(SessionError::NotificationWithId),
            MethodKind::Unknown => {
                return Err(SessionError::UnknownMethod {
                    method: frame.method,
                    state,
                });
            }
        }
        let Some(pos) = self.outstanding.iter().position(|&pending| pending == id) else {
            return Err(SessionError::UnknownReplyId { id });
        };

        if let Some(Value::String(message)) = frame.params.get("error") {
            return Err(SessionError::RemoteError {
                method: frame.method,
                message: message.clone(),
            });
        }

        match (kind, self.state) {
            (MethodKind::Initialize, SessionState::AwaitingInitializeReply) => {
                let effective = self.accept_initialize(&frame)?;
                self.outstanding.remove(pos);
                self.negotiated = Some(effective);
                self.state = SessionState::Ready;
                Ok(Incoming::Initialized(effective))
            }
            (MethodKind::Fixes, SessionState::Ready) => {
                let reply: FixesReply =
                    parse_payload(&frame.method, std::mem::take(&mut frame.params))?;
                self.outstanding.remove(pos);
                Ok(Incoming::Fixes { id, reply })
            }
            (MethodKind::Shutdown, SessionState::AwaitingShutdownReply) => {
                self.outstanding.remove(pos);
                self.state = SessionState::Terminated;
                Ok(Incoming::ShutdownComplete)
            }
            _ => Err(SessionError::UnexpectedFrame {
                method: frame.method,
                state,
            }),
        }
    }

    fn accept_initialize(&self, frame: &Frame) -> Result<CapabilitySet, SessionError> {
        let got = match frame.params.get("protocol_version") {
            Some(Value::Number(n)) => n.as_u64().and_then(|v| u32::try_from(v).ok()),
            _ => None,
        };
        let got = got.ok_or_else(|| SessionError::InvalidPayload {
            method: frame.method.clone(),
            reason: "`protocol_version` must be a positive integer".to_owned(),
        })?;
        if got != PROTOCOL_VERSION {
            return Err(SessionError::VersionMismatch {
                got,
                supported: PROTOCOL_VERSION,
            });
        }
        let mut plugin_caps = CapabilitySet::empty();
        plugin_caps.insert_wire_names(frame.params.get("capabilities"));
        Ok(plugin_caps.intersect(self.local))
    }

    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

/// Which protocol method a frame name denotes. Copy-able so state-machine
/// matches never borrow the frame's fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MethodKind {
    Initialize,
    Shutdown,
    Fixes,
    Diagnostic,
    Unknown,
}

fn method_kind(name: &str) -> MethodKind {
    match name {
        METHOD_INITIALIZE => MethodKind::Initialize,
        METHOD_SHUTDOWN => MethodKind::Shutdown,
        METHOD_REQUEST_FIXES => MethodKind::Fixes,
        METHOD_DIAGNOSTIC_NOTIFICATION => MethodKind::Diagnostic,
        _ => MethodKind::Unknown,
    }
}

/// Parses a frame's `params` as `T`. The frame is consumed by the state
/// machine, so the map is taken by value — no clone on the wire path.
fn parse_payload<T: serde::de::DeserializeOwned>(
    method: &str,
    params: Map<String, Value>,
) -> Result<T, SessionError> {
    serde_json::from_value(Value::Object(params)).map_err(|e| SessionError::InvalidPayload {
        method: method.to_owned(),
        reason: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Capability;
    use crate::frame::decode_line;

    /// A minimal in-test plugin: builds the reply frames the real Dart host
    /// would send, so the handshake loopback runs without a process.
    struct FakePlugin;

    impl FakePlugin {
        fn reply_initialize(id: u64, caps: &[&str], version: u32) -> Frame {
            let mut params = Map::new();
            params.insert("protocol_version".into(), json!(version));
            params.insert("capabilities".into(), json!(caps));
            Frame::request(id, METHOD_INITIALIZE, params)
        }

        fn diagnostic(file: &str, message: &str) -> Frame {
            let mut params = Map::new();
            params.insert("file".into(), json!(file));
            params.insert("severity".into(), json!("warning"));
            params.insert("code".into(), json!("my_plugin/lint"));
            params.insert("message".into(), json!(message));
            params.insert("offset".into(), json!(3));
            params.insert("length".into(), json!(4));
            Frame::notification(METHOD_DIAGNOSTIC_NOTIFICATION, params)
        }
    }

    #[test]
    fn full_handshake_cycle_negotiates_intersection() {
        let mut session = Session::new(CapabilitySet::all());

        let init = session.send_initialize().expect("send initialize");
        assert_eq!(session.state(), SessionState::AwaitingInitializeReply);
        assert_eq!(init.id, Some(1));
        assert_eq!(
            init.params.get("protocol_version"),
            Some(&json!(PROTOCOL_VERSION))
        );

        let reply = FakePlugin::reply_initialize(init.id.expect("id"), &["lint", "completions"], 1);
        match session.handle(reply).expect("handle") {
            Incoming::Initialized(effective) => {
                assert!(effective.contains(Capability::Lint));
                assert!(!effective.contains(Capability::Fix));
                assert!(!effective.contains(Capability::Assist));
            }
            other => panic!("expected Initialized, got {other:?}"),
        }
        assert_eq!(session.state(), SessionState::Ready);
        assert_eq!(session.negotiated(), Some(effective_set()));

        // A fixes request without the negotiated `fix` capability is refused
        // before any frame is built.
        assert_eq!(
            session
                .send_fixes_request("lib/main.dart", 0, 0)
                .unwrap_err(),
            SessionError::CapabilityNotNegotiated(Capability::Fix)
        );

        // Diagnostics flow while ready.
        let notification = FakePlugin::diagnostic("lib/main.dart", "prefer const");
        match session.handle(notification).expect("handle") {
            Incoming::Diagnostic(diagnostic) => {
                assert_eq!(diagnostic.file, "lib/main.dart");
                assert_eq!(diagnostic.severity, Severity::Warning);
                assert_eq!(diagnostic.code, "my_plugin/lint");
                assert_eq!(diagnostic.offset, 3);
            }
            other => panic!("expected Diagnostic, got {other:?}"),
        }

        // Shutdown ends the session.
        let shutdown = session.send_shutdown().expect("send shutdown");
        assert_eq!(session.state(), SessionState::AwaitingShutdownReply);
        let reply = Frame::request(shutdown.id.expect("id"), METHOD_SHUTDOWN, Map::new());
        assert_eq!(
            session.handle(reply).expect("handle"),
            Incoming::ShutdownComplete
        );
        assert_eq!(session.state(), SessionState::Terminated);
    }

    fn effective_set() -> CapabilitySet {
        let mut caps = CapabilitySet::empty();
        caps.insert(Capability::Lint);
        caps
    }

    #[test]
    fn version_mismatch_is_a_typed_error() {
        let mut session = Session::new(CapabilitySet::all());
        let init = session.send_initialize().expect("send initialize");
        let reply = FakePlugin::reply_initialize(init.id.expect("id"), &["lint"], 2);
        assert_eq!(
            session.handle(reply).unwrap_err(),
            SessionError::VersionMismatch {
                got: 2,
                supported: PROTOCOL_VERSION
            }
        );
    }

    #[test]
    fn diagnostic_before_initialize_completes_is_rejected_not_queued() {
        let mut session = Session::new(CapabilitySet::all());
        let notification = FakePlugin::diagnostic("lib/main.dart", "too early");
        assert_eq!(
            session.handle(notification).unwrap_err(),
            SessionError::UnexpectedFrame {
                method: METHOD_DIAGNOSTIC_NOTIFICATION.to_owned(),
                state: SessionState::Fresh.as_str(),
            }
        );
    }

    #[test]
    fn unknown_method_is_a_typed_error_not_a_panic() {
        let mut session = Session::new(CapabilitySet::all());
        let init = session.send_initialize().expect("send initialize");
        session
            .handle(FakePlugin::reply_initialize(
                init.id.expect("id"),
                &["lint"],
                1,
            ))
            .expect("handshake");

        // A known method with an id that matches no outstanding request...
        let stray = Frame::request(999, METHOD_REQUEST_FIXES, Map::new());
        assert_eq!(
            session.handle(stray).unwrap_err(),
            SessionError::UnknownReplyId { id: 999 }
        );
        // ...and an unknown method (with or without id) is a typed error.
        let exotic = Frame::notification("plugin/getCompletions", Map::new());
        assert_eq!(
            session.handle(exotic).unwrap_err(),
            SessionError::UnknownMethod {
                method: "plugin/getCompletions".to_owned(),
                state: "Ready",
            }
        );
    }

    #[test]
    fn fixes_round_trip_over_the_wire_schema() {
        let mut session = Session::new(CapabilitySet::all());
        let init = session.send_initialize().expect("send initialize");
        session
            .handle(FakePlugin::reply_initialize(
                init.id.expect("id"),
                &["lint", "fix"],
                1,
            ))
            .expect("handshake");

        let request = session
            .send_fixes_request("lib/main.dart", 10, 2)
            .expect("send fixes request");
        let mut wire = crate::encode_frame(&request).expect("encode");
        wire.push(b'\n');
        let decoded = decode_line(&wire).expect("decode");
        assert_eq!(decoded, request);

        let mut fixes = Map::new();
        fixes.insert(
            "fixes".into(),
            json!([{
                "priority": 1,
                "message": "add const",
                "edits": [{"offset": 10, "length": 2, "replacement": "const x"}],
            }]),
        );
        let reply = Frame::request(request.id.expect("id"), METHOD_REQUEST_FIXES, fixes);
        match session.handle(reply).expect("handle") {
            Incoming::Fixes {
                id,
                reply: FixesReply { fixes },
            } => {
                assert_eq!(id, request.id.expect("id"));
                assert_eq!(fixes.len(), 1);
                assert_eq!(fixes[0].message, "add const");
                assert_eq!(fixes[0].edits[0].replacement, "const x");
            }
            other => panic!("expected Fixes, got {other:?}"),
        }
    }

    #[test]
    fn remote_error_and_bad_payloads_are_typed() {
        let mut session = Session::new(CapabilitySet::all());
        let init = session.send_initialize().expect("send initialize");
        let id = init.id.expect("id");

        // Plugin-side failure surfaces as a typed error, never a panic.
        let mut params = Map::new();
        params.insert("error".into(), json!("plugin exploded on startup"));
        let reply = Frame::request(id, METHOD_INITIALIZE, params);
        assert_eq!(
            session.handle(reply).unwrap_err(),
            SessionError::RemoteError {
                method: METHOD_INITIALIZE.to_owned(),
                message: "plugin exploded on startup".to_owned(),
            }
        );

        // Restart the handshake: a reply without protocol_version is invalid.
        let mut session = Session::new(CapabilitySet::all());
        let init = session.send_initialize().expect("send initialize");
        let reply = Frame::request(init.id.expect("id"), METHOD_INITIALIZE, Map::new());
        assert!(matches!(
            session.handle(reply).unwrap_err(),
            SessionError::InvalidPayload { .. }
        ));
    }

    #[test]
    fn diagnostic_crossing_the_shutdown_request_is_queued_not_fatal() {
        let mut session = Session::new(CapabilitySet::all());
        let init = session.send_initialize().expect("send initialize");
        session
            .handle(FakePlugin::reply_initialize(
                init.id.expect("id"),
                &["lint"],
                1,
            ))
            .expect("handshake");
        let shutdown = session.send_shutdown().expect("send shutdown");

        // The host answered the last work batch without having seen the
        // shutdown request: the finding legitimately crosses it on the wire
        // and must ride out the window, not tear the session down.
        let crossing = FakePlugin::diagnostic("lib/main.dart", "in flight");
        assert_eq!(
            session.handle(crossing).expect("crossing diagnostic queues"),
            Incoming::Diagnostic(PluginDiagnostic {
                file: "lib/main.dart".to_owned(),
                severity: Severity::Warning,
                code: "my_plugin/lint".to_owned(),
                message: "in flight".to_owned(),
                offset: 3,
                length: 4,
            })
        );

        // The shutdown reply still completes the session...
        let reply = Frame::request(shutdown.id.expect("id"), METHOD_SHUTDOWN, Map::new());
        assert_eq!(
            session.handle(reply).expect("handle"),
            Incoming::ShutdownComplete
        );
        assert_eq!(session.state(), SessionState::Terminated);

        // ...and after the reply a diagnostic is a violation again: the
        // session is over, the reader is gone.
        let late = FakePlugin::diagnostic("lib/main.dart", "late");
        assert_eq!(
            session.handle(late).unwrap_err(),
            SessionError::UnexpectedFrame {
                method: METHOD_DIAGNOSTIC_NOTIFICATION.to_owned(),
                state: SessionState::Terminated.as_str(),
            }
        );
    }

    #[test]
    fn out_of_order_senders_are_rejected() {
        let mut session = Session::new(CapabilitySet::all());
        assert!(matches!(
            session.send_shutdown().unwrap_err(),
            SessionError::UnexpectedFrame { .. }
        ));
        assert!(matches!(
            session.send_fixes_request("f", 0, 0).unwrap_err(),
            SessionError::UnexpectedFrame { .. }
        ));
        session.send_initialize().expect("send initialize");
        assert!(matches!(
            session.send_initialize().unwrap_err(),
            SessionError::UnexpectedFrame { .. }
        ));
    }
}
