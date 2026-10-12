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
//! 3. While `Ready`: the plugin may send `notification/diagnostic` frames
//!    and `request/query` reverse requests (only if `query` was
//!    negotiated), and the core may send `request/fixes` (only if `fix`
//!    was negotiated). Each method has exactly one requesting direction,
//!    so a frame's method disambiguates request from reply on both sides.
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
use std::collections::BTreeMap;

use crate::capability::{Capability, CapabilitySet};
use crate::frame::{
    Frame, METHOD_DIAGNOSTIC_NOTIFICATION, METHOD_ENGINE_DIAGNOSTICS, METHOD_INITIALIZE,
    METHOD_PLUGIN_CONFIG, METHOD_REQUEST_FIXES, METHOD_REQUEST_QUERY, METHOD_SHUTDOWN,
    PROTOCOL_VERSION,
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
    /// A `request/query` from the plugin (the wire's reverse direction):
    /// the id lives in the plugin's own id space, and the caller answers it
    /// with [`query_reply_frame`] or [`query_error_frame`] under the same
    /// correlation.
    Query {
        /// The plugin's correlation id for this request.
        id: u64,
        /// The parsed request payload.
        request: QueryRequest,
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

/// Registration kind of a plugin rule, mirroring the official registry
/// split (`package:analyzer/src/lint/registry.dart`): lint rules are
/// disabled by default and enabled via rule configs, warning rules are
/// enabled by default and disabled via rule configs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleKind {
    /// Disabled by default; enabled via an explicit rule config.
    Lint,
    /// Enabled by default; disabled via an explicit rule config.
    Warning,
}

/// One diagnostic code of a registered rule, as the census carries it: the
/// lower-case wire name (the identity fixes and dedup key on) and the code's
/// declared severity (the base the core's severity-override resolution
/// re-levels from).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleCodeInfo {
    /// The lower-case code name (the wire's rule-code identity).
    pub name: String,
    /// The code's declared severity.
    pub severity: Severity,
}

/// One rule of a plugin's registration census (the `rules` array of the
/// `initialize` reply): what registered, in registration order. The census
/// is what the core resolves the per-cycle enablement against — without it
/// the `lint` capability is an uninterpretable claim.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleCensus {
    /// The rule's name (matched case-insensitively, like the official
    /// registry's lower-case keys).
    pub name: String,
    /// Registration kind (the default-enabled face).
    pub kind: RuleKind,
    /// The codes this rule reports, in declaration order.
    pub codes: Vec<RuleCodeInfo>,
}

/// One unsaved-text overlay of the per-cycle config: the core owns the
/// open-document table and pushes the client's text so the host parses the
/// edited view, never stale disk bytes. `content: None` marks a previously
/// overlaid file whose text now lives on disk again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlayEntry {
    /// Path of the overlaid file (wire form, matching the analyzed set).
    pub file: String,
    /// The unsaved text, or `None` to fall back to disk.
    pub content: Option<String>,
}

/// The per-cycle execution config the core pushes as
/// `notification/plugin_config`: which of the plugin's census rules are
/// enabled (resolved from the governing analysis options), the per-code
/// severity overrides, and the open-document overlays. An empty config is
/// the official no-configuration posture: lint rules stay disabled, warning
/// rules stay enabled, declared severities stand, files read from disk.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CycleConfig {
    /// Enabled rule names (the plugin's own census names, lower-cased).
    #[serde(default)]
    pub enabled_rules: Vec<String>,
    /// Per-code severity overrides, keyed by lower-case code name.
    #[serde(default)]
    pub severity_overrides: BTreeMap<String, Severity>,
    /// Open-document overlays (dirty file → unsaved text).
    #[serde(default)]
    pub overlays: Vec<OverlayEntry>,
}

impl CycleConfig {
    /// Whether nothing is configured — the frame the config-dedup skips.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.enabled_rules.is_empty()
            && self.severity_overrides.is_empty()
            && self.overlays.is_empty()
    }
}

/// One analyze cycle's push material beyond the engine diagnostics
/// themselves: the analyzed file set the host runs rules over, and the
/// resolved config. The runtime turns this into the config frame (when it
/// changed) plus the `files` parameter of the engine-diagnostics frame.
#[derive(Clone, Debug, Default)]
pub struct CyclePush {
    /// The cycle's analyzed files (wire-form paths).
    pub analyzed: Vec<String>,
    /// The resolved execution config.
    pub config: CycleConfig,
}

/// Builds the core → plugin `notification/plugin_config` frame carrying
/// [config]. A notification: the plugin never replies to it.
#[must_use]
pub fn plugin_config_frame(config: &CycleConfig) -> Frame {
    let mut params = Map::new();
    params.insert("enabled_rules".into(), json!(config.enabled_rules.clone()));
    params.insert(
        "severity_overrides".into(),
        json!(config.severity_overrides.clone()),
    );
    params.insert("overlays".into(), json!(config.overlays.clone()));
    Frame::notification(METHOD_PLUGIN_CONFIG, params)
}

/// Builds the core → plugin `notification/engine_diagnostics` batch carrying
/// the engine's current diagnostics (one frame per analyze cycle) plus the
/// cycle's analyzed file set — the files a rule-executing plugin runs its
/// enabled rules over. A notification: the plugin never replies to it.
#[must_use]
pub fn engine_diagnostics_frame(diagnostics: &[PluginDiagnostic], analyzed: &[String]) -> Frame {
    let mut params = Map::new();
    params.insert("diagnostics".into(), json!(diagnostics));
    params.insert("files".into(), json!(analyzed));
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

/// Which engine fact a `request/query` asks for. The first five are the
/// daemon's index query kinds (the frozen schema's rows); `element_at` is
/// the offset-keyed face: what the queried position denotes. The lowercase
/// spellings are the wire names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QueryKind {
    /// Declaration rows plus every resolved reference to them.
    Symbol,
    /// The reference rows of the symbol face.
    References,
    /// The invocation-shaped subset of the reference rows.
    Callers,
    /// The queried type's subtypes / the queried member's overriding rows.
    Implementations,
    /// The declaring library's importers.
    Dependencies,
    /// Offset-keyed element lookup: `{file, offset}` → the element rows the
    /// position denotes (its own declaration, or the declaration its use
    /// resolves to).
    #[serde(rename = "element_at")]
    ElementAt,
}

impl QueryKind {
    /// Whether this kind is name-keyed (the index query family) — the
    /// payload validation's split.
    #[must_use]
    pub const fn is_name_keyed(self) -> bool {
        !matches!(self, Self::ElementAt)
    }
}

/// One `request/query` from the plugin. Name-keyed kinds require `name`;
/// [`QueryKind::ElementAt`] requires `file` and `offset`. Additional fields
/// are ignored (additive evolution, the same posture the rest of the wire
/// takes).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct QueryRequest {
    /// Which fact is requested.
    pub kind: QueryKind,
    /// The queried symbol name (name-keyed kinds).
    #[serde(default)]
    pub name: Option<String>,
    /// The queried file, in the analyzed set's wire spelling
    /// ([`QueryKind::ElementAt`]).
    #[serde(default)]
    pub file: Option<String>,
    /// UTF-16 offset into `file` ([`QueryKind::ElementAt`]).
    #[serde(default)]
    pub offset: Option<u64>,
}

impl QueryRequest {
    /// Checks the payload against the kind's requirements. A request the
    /// core cannot interpret is answered with an error reply (the session
    /// stays up), not a teardown — the plugin asked, the core declined.
    ///
    /// # Errors
    ///
    /// A human-readable reason when the payload does not satisfy the kind.
    pub fn validate(&self) -> Result<(), String> {
        if self.kind.is_name_keyed() {
            match self.name.as_deref() {
                Some(name) if !name.is_empty() => Ok(()),
                _ => Err(format!(
                    "query kind `{kind}` requires a non-empty string `name`",
                    kind = match self.kind {
                        QueryKind::Symbol => "symbol",
                        QueryKind::References => "references",
                        QueryKind::Callers => "callers",
                        QueryKind::Implementations => "implementations",
                        QueryKind::Dependencies => "dependencies",
                        QueryKind::ElementAt => unreachable!("name-keyed by definition"),
                    }
                )),
            }
        } else {
            let file_ok = self.file.as_deref().is_some_and(|f| !f.is_empty());
            let offset_ok = self.offset.is_some();
            match (file_ok, offset_ok) {
                (true, true) => Ok(()),
                (false, _) => {
                    Err("query kind `element_at` requires a non-empty string `file`".to_owned())
                }
                (_, false) => {
                    Err("query kind `element_at` requires an integer `offset`".to_owned())
                }
            }
        }
    }
}

/// Builds the core's reply frame to the plugin's query [id](Frame::id):
/// same method, same correlation. `params` is the answer object — the
/// index kinds answer with the frozen schema's query envelope, `element_at`
/// with its element rows.
#[must_use]
pub fn query_reply_frame(id: u64, params: Map<String, Value>) -> Frame {
    Frame::request(id, METHOD_REQUEST_QUERY, params)
}

/// Builds the core's error reply to the plugin's query [id](Frame::id):
/// `{"error": message}`, the wire's only error channel. The session stays
/// up — a declined query is not a contract break.
#[must_use]
pub fn query_error_frame(id: u64, message: &str) -> Frame {
    let mut params = Map::new();
    params.insert("error".into(), json!(message));
    Frame::request(id, METHOD_REQUEST_QUERY, params)
}

/// The core side of one plugin session. Pure state over frames: no I/O, no
/// threads — the caller owns the transport.
#[derive(Debug)]
pub struct Session {
    state: SessionState,
    local: CapabilitySet,
    negotiated: Option<CapabilitySet>,
    /// The plugin's registration census, parsed from the accepted
    /// `initialize` reply (empty until the handshake completed).
    rule_census: Vec<RuleCensus>,
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
            rule_census: Vec::new(),
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

    /// The plugin's registration census (the `rules` array of the accepted
    /// `initialize` reply; empty before the handshake completed). The core
    /// resolves per-cycle rule enablement against it.
    #[must_use]
    pub fn rule_census(&self) -> &[RuleCensus] {
        &self.rule_census
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
        // The wire's reverse direction: `request/query` is always the
        // plugin's request (the core never issues queries), regardless of
        // which id space the frame's id belongs to. Every other id-carrying
        // frame is a reply in the core's id space.
        if frame.method == METHOD_REQUEST_QUERY {
            return self.handle_query_request(frame);
        }
        match frame.id {
            Some(id) => self.handle_reply(frame, id),
            None => self.handle_notification(frame),
        }
    }

    /// Routes one inbound `request/query` frame. Requests are served in
    /// [`SessionState::Ready`] and tolerated (queued for an answer, like a
    /// diagnostic) crossing the shutdown window; a query without a
    /// negotiated `query` capability, in any earlier state, or without an
    /// id is a contract break.
    fn handle_query_request(&mut self, mut frame: Frame) -> Result<Incoming, SessionError> {
        let state = self.state.as_str();
        let Some(id) = frame.id else {
            return Err(SessionError::UnexpectedFrame {
                method: frame.method,
                state,
            });
        };
        match self.state {
            SessionState::Ready | SessionState::AwaitingShutdownReply => {}
            _ => {
                return Err(SessionError::UnexpectedFrame {
                    method: frame.method,
                    state,
                });
            }
        }
        if !self
            .negotiated
            .is_some_and(|caps| caps.contains(Capability::Query))
        {
            return Err(SessionError::CapabilityNotNegotiated(Capability::Query));
        }
        let request: QueryRequest =
            parse_payload(&frame.method, std::mem::take(&mut frame.params))?;
        request
            .validate()
            .map_err(|reason| SessionError::InvalidPayload {
                method: frame.method.clone(),
                reason,
            })?;
        Ok(Incoming::Query { id, request })
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
                self.rule_census = parse_census(&frame)?;
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

/// Parses the `initialize` reply's registration census: the optional
/// `rules` array (absent on hosts that register no rules — an additive
/// field), each row `{name, kind: "lint"|"warning", codes: [{name,
/// severity}]}`. A present-but-malformed census is an [`SessionError::InvalidPayload`]
/// (fatal): the core cannot resolve enablement against a census it cannot
/// read, and a wrong census silently disables or enables the wrong rules.
fn parse_census(frame: &Frame) -> Result<Vec<RuleCensus>, SessionError> {
    let Some(value) = frame.params.get("rules") else {
        return Ok(Vec::new());
    };
    let Value::Array(rows) = value else {
        return Err(SessionError::InvalidPayload {
            method: frame.method.clone(),
            reason: "`rules` must be an array".to_owned(),
        });
    };
    let mut census = Vec::with_capacity(rows.len());
    for row in rows {
        let Value::Object(row) = row else {
            return Err(SessionError::InvalidPayload {
                method: frame.method.clone(),
                reason: "each `rules` entry must be an object".to_owned(),
            });
        };
        let name = match row.get("name") {
            Some(Value::String(name)) if !name.is_empty() => name.clone(),
            _ => {
                return Err(SessionError::InvalidPayload {
                    method: frame.method.clone(),
                    reason: "census row requires a non-empty string `name`".to_owned(),
                });
            }
        };
        let kind = match row.get("kind") {
            Some(Value::String(kind)) if kind == "lint" => RuleKind::Lint,
            Some(Value::String(kind)) if kind == "warning" => RuleKind::Warning,
            _ => {
                return Err(SessionError::InvalidPayload {
                    method: frame.method.clone(),
                    reason: "census row `kind` must be \"lint\" or \"warning\"".to_owned(),
                });
            }
        };
        let mut codes = Vec::new();
        match row.get("codes") {
            Some(Value::Array(entries)) => {
                for entry in entries {
                    let Value::Object(entry) = entry else {
                        return Err(SessionError::InvalidPayload {
                            method: frame.method.clone(),
                            reason: "each census `codes` entry must be an object".to_owned(),
                        });
                    };
                    let name = match entry.get("name") {
                        Some(Value::String(name)) if !name.is_empty() => name.clone(),
                        _ => {
                            return Err(SessionError::InvalidPayload {
                                method: frame.method.clone(),
                                reason: "census code requires a non-empty string `name`".to_owned(),
                            });
                        }
                    };
                    let severity = match entry.get("severity") {
                        Some(Value::String(severity)) => match severity.as_str() {
                            "error" => Severity::Error,
                            "warning" => Severity::Warning,
                            "info" => Severity::Info,
                            other => {
                                return Err(SessionError::InvalidPayload {
                                    method: frame.method.clone(),
                                    reason: format!(
                                        "census code `severity` must be error/warning/info, \
                                         got `{other}`"
                                    ),
                                });
                            }
                        },
                        _ => {
                            return Err(SessionError::InvalidPayload {
                                method: frame.method.clone(),
                                reason: "census code requires a string `severity`".to_owned(),
                            });
                        }
                    };
                    codes.push(RuleCodeInfo { name, severity });
                }
            }
            _ => {
                return Err(SessionError::InvalidPayload {
                    method: frame.method.clone(),
                    reason: "census row requires a `codes` array".to_owned(),
                });
            }
        }
        census.push(RuleCensus { name, kind, codes });
    }
    Ok(census)
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
            session
                .handle(crossing)
                .expect("crossing diagnostic queues"),
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
    fn census_round_trips_through_the_initialize_reply() {
        let mut session = Session::new(CapabilitySet::all());
        let init = session.send_initialize().expect("send initialize");
        let mut params = Map::new();
        params.insert("protocol_version".into(), json!(1));
        params.insert(
            "rules".into(),
            json!([
                {
                    "name": "avoid_sync_io",
                    "kind": "lint",
                    "codes": [{"name": "avoid_sync_io", "severity": "warning"}],
                },
                {"name": "always_on", "kind": "warning", "codes": []},
            ]),
        );
        let reply = Frame::request(init.id.expect("id"), METHOD_INITIALIZE, params);
        session.handle(reply).expect("handshake");
        assert_eq!(
            session.rule_census(),
            [
                RuleCensus {
                    name: "avoid_sync_io".to_owned(),
                    kind: RuleKind::Lint,
                    codes: vec![RuleCodeInfo {
                        name: "avoid_sync_io".to_owned(),
                        severity: Severity::Warning,
                    }],
                },
                RuleCensus {
                    name: "always_on".to_owned(),
                    kind: RuleKind::Warning,
                    codes: Vec::new(),
                },
            ]
        );
    }

    #[test]
    fn census_is_empty_when_the_reply_omits_rules() {
        let mut session = Session::new(CapabilitySet::all());
        let init = session.send_initialize().expect("send initialize");
        let mut params = Map::new();
        params.insert("protocol_version".into(), json!(1));
        let reply = Frame::request(init.id.expect("id"), METHOD_INITIALIZE, params);
        session.handle(reply).expect("handshake");
        assert!(
            session.rule_census().is_empty(),
            "additive field: absent = none"
        );
    }

    #[test]
    fn malformed_census_is_a_typed_payload_error() {
        for rules in [
            json!("not an array"),
            json!([{"name": "r", "kind": "mystery", "codes": []}]),
            json!([{"name": "r", "kind": "lint"}]),
            json!([{"name": "", "kind": "lint", "codes": []}]),
            json!([{"name": "r", "kind": "lint", "codes": [{"name": "c"}]}]),
            json!([{"name": "r", "kind": "lint", "codes": [{"name": "c", "severity": "hint"}]}]),
        ] {
            let mut session = Session::new(CapabilitySet::all());
            let init = session.send_initialize().expect("send initialize");
            let mut params = Map::new();
            params.insert("protocol_version".into(), json!(1));
            params.insert("rules".into(), rules);
            let reply = Frame::request(init.id.expect("id"), METHOD_INITIALIZE, params);
            assert!(
                matches!(
                    session.handle(reply),
                    Err(SessionError::InvalidPayload { .. })
                ),
                "a census the core cannot resolve enablement against is fatal"
            );
        }
    }

    #[test]
    fn plugin_config_frame_round_trips_through_json() {
        let config = CycleConfig {
            enabled_rules: vec!["avoid_sync_io".to_owned()],
            severity_overrides: BTreeMap::from([
                ("code_a".to_owned(), Severity::Error),
                ("code_b".to_owned(), Severity::Info),
            ]),
            overlays: vec![OverlayEntry {
                file: "lib/main.dart".to_owned(),
                content: Some("void main() {}".to_owned()),
            }],
        };
        let frame = plugin_config_frame(&config);
        assert_eq!(frame.method, METHOD_PLUGIN_CONFIG);
        assert!(frame.is_notification());
        let line = crate::encode_frame(&frame).expect("encode");
        let decoded = crate::decode_line(&line).expect("decode");
        assert_eq!(decoded, frame);
        assert_eq!(
            decoded.params["severity_overrides"],
            json!({"code_a": "error", "code_b": "info"})
        );
        // Identical configs produce identical params (the runtime's dedup
        // key); the empty config is the no-push shape.
        assert_eq!(plugin_config_frame(&config).params, frame.params);
        assert!(CycleConfig::default().is_empty());
    }

    #[test]
    fn engine_diagnostics_frame_carries_the_analyzed_set() {
        let rows = [PluginDiagnostic {
            file: "lib/a.dart".to_owned(),
            severity: Severity::Warning,
            code: "c".to_owned(),
            message: "m".to_owned(),
            offset: 0,
            length: 1,
        }];
        let analyzed = ["lib/a.dart".to_owned(), "lib/b.dart".to_owned()];
        let frame = engine_diagnostics_frame(&rows, &analyzed);
        assert_eq!(frame.params["files"], json!(["lib/a.dart", "lib/b.dart"]));
        assert_eq!(
            frame.params["diagnostics"].as_array().expect("rows").len(),
            1
        );
        // No files: the additive array is empty, the batch still well-formed.
        let frame = engine_diagnostics_frame(&rows, &[]);
        assert_eq!(frame.params["files"], json!([]));
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

    /// Completes a handshake advertising exactly [caps].
    fn ready_session(caps: &[&str]) -> Session {
        let mut session = Session::new(CapabilitySet::all());
        let init = session.send_initialize().expect("send initialize");
        session
            .handle(FakePlugin::reply_initialize(init.id.expect("id"), caps, 1))
            .expect("handshake");
        session
    }

    /// A `request/query` frame as the Dart host would send it.
    fn query_frame(id: u64, params: Value) -> Frame {
        Frame::request(
            id,
            METHOD_REQUEST_QUERY,
            params.as_object().expect("object").clone(),
        )
    }

    #[test]
    fn reverse_query_routes_to_the_caller_with_the_plugin_id() {
        // The plugin's id space is its own: id 5 must route as a reverse
        // request even while the core has its own id 5 outstanding — the
        // method name is what separates request from reply.
        let mut session = ready_session(&["lint", "fix", "query"]);
        let fixes = session
            .send_fixes_request("lib/main.dart", 0, 0)
            .expect("send fixes");
        // The handshake consumed id 1; the fixes request is id 2.
        assert_eq!(fixes.id, Some(2));

        let query = query_frame(5, json!({"kind": "symbol", "name": "Foo"}));
        assert_eq!(
            session.handle(query).expect("route"),
            Incoming::Query {
                id: 5,
                request: QueryRequest {
                    kind: QueryKind::Symbol,
                    name: Some("Foo".to_owned()),
                    file: None,
                    offset: None,
                },
            },
            "the plugin's id 5 routes as a request, not a reply"
        );

        // And the core's own fixes reply still correlates through its id —
        // the two directions interleave freely.
        let mut fixes_params = Map::new();
        fixes_params.insert("fixes".into(), json!([]));
        let reply = Frame::request(fixes.id.expect("id"), METHOD_REQUEST_FIXES, fixes_params);
        assert!(matches!(
            session.handle(reply).expect("reply"),
            Incoming::Fixes { .. }
        ));
    }

    #[test]
    fn reverse_query_without_the_negotiated_capability_is_fatal() {
        let mut session = ready_session(&["lint"]);
        let query = query_frame(1, json!({"kind": "symbol", "name": "Foo"}));
        assert_eq!(
            session.handle(query).unwrap_err(),
            SessionError::CapabilityNotNegotiated(Capability::Query)
        );
    }

    #[test]
    fn reverse_query_crossing_the_shutdown_window_is_accepted() {
        let mut session = ready_session(&["query"]);
        let shutdown = session.send_shutdown().expect("send shutdown");
        let query = query_frame(
            2,
            json!({"kind": "element_at", "file": "f.dart", "offset": 3}),
        );
        assert_eq!(
            session.handle(query).expect("crossing query queues"),
            Incoming::Query {
                id: 2,
                request: QueryRequest {
                    kind: QueryKind::ElementAt,
                    name: None,
                    file: Some("f.dart".to_owned()),
                    offset: Some(3),
                },
            }
        );
        // The shutdown reply still ends the session.
        let reply = Frame::request(shutdown.id.expect("id"), METHOD_SHUTDOWN, Map::new());
        assert_eq!(
            session.handle(reply).expect("handle"),
            Incoming::ShutdownComplete
        );
        // After the reply the session is over; a late query is a violation.
        let late = query_frame(3, json!({"kind": "symbol", "name": "x"}));
        assert_eq!(
            session.handle(late).unwrap_err(),
            SessionError::UnexpectedFrame {
                method: METHOD_REQUEST_QUERY.to_owned(),
                state: SessionState::Terminated.as_str(),
            }
        );
    }

    #[test]
    fn reverse_query_payloads_are_validated_by_kind() {
        let mut session = ready_session(&["query"]);
        for params in [
            json!({"kind": "symbol"}),
            json!({"kind": "callers", "name": ""}),
            json!({"kind": "element_at", "offset": 3}),
            json!({"kind": "element_at", "file": "f.dart"}),
            json!({"kind": "mystery", "name": "x"}),
        ] {
            let frame = query_frame(1, params);
            assert!(
                matches!(
                    session.handle(frame),
                    Err(SessionError::InvalidPayload { .. } | SessionError::UnknownMethod { .. })
                ),
                "a query the core cannot interpret is a typed error"
            );
        }
        // A request without an id is malformed regardless of state.
        let mut session = ready_session(&["query"]);
        let idless = Frame::notification(
            METHOD_REQUEST_QUERY,
            json!({"kind": "symbol", "name": "x"})
                .as_object()
                .expect("object")
                .clone(),
        );
        assert!(matches!(
            session.handle(idless),
            Err(SessionError::UnexpectedFrame { .. })
        ));
    }

    #[test]
    fn query_reply_frames_round_trip_and_carry_the_error_channel() {
        let mut params = Map::new();
        params.insert("schema_version".into(), json!(1));
        params.insert("declarations".into(), json!([]));
        let reply = query_reply_frame(9, params);
        assert_eq!(reply.method, METHOD_REQUEST_QUERY);
        assert_eq!(reply.id, Some(9));
        let line = crate::encode_frame(&reply).expect("encode");
        assert_eq!(crate::decode_line(&line).expect("decode"), reply);

        let error = query_error_frame(9, "unknown file `x.dart`");
        assert_eq!(error.params["error"], json!("unknown file `x.dart`"));
    }
}
