//! dartq plugin protocol: the IPC contract between the Rust core and the
//! per-plugin Dart host process.
//!
//! Plugins never enter the Rust core. Each plugin runs as its own sidecar
//! process (the Dart host, package `dartq_plugin_host`), and the only thing
//! crossing the boundary is this wire protocol. This crate is the single
//! source of truth for that contract on the core side: frame types, the
//! codec, the handshake state machine, and the plugin manifest.
//!
//! # Wire format (protocol v1)
//!
//! Frames are newline-delimited JSON over the plugin process's stdio: one
//! UTF-8 JSON object per line, at most [`MAX_FRAME_SIZE`] (16 MiB) bytes
//! including the line terminator. Every frame carries the same four fields:
//!
//! ```json
//! {"schema_version": 1, "id": 7, "method": "initialize", "params": {}}
//! ```
//!
//! - `schema_version`: always `1`; a foreign version is a codec error.
//! - `id`: the request/reply correlation id. `null` on notifications
//!   (plugin-to-core), an integer on requests (core-to-plugin) and their
//!   replies (same id, same method as the request).
//! - `method`: one of the `METHOD_*` constants below.
//! - `params`: a JSON object; an error reply carries `{"error": "<message>"}`.
//!
//! # Method table
//!
//! | Method | Direction | Kind | `params` |
//! | --- | --- | --- | --- |
//! | `initialize` | core → plugin | request | `{protocol_version, capabilities: [..]}` |
//! | `initialize` | plugin → core | reply | `{protocol_version, capabilities: [..], rules: [{name, kind, codes: [{name, severity}]}]}` — `rules` is the registration census (T5); hosts that register no rules omit it |
//! | `shutdown` | core → plugin | request | `{}` |
//! | `shutdown` | plugin → core | reply | `{}` |
//! | `request/fixes` | core → plugin | request | `{file, offset, length}` (UTF-16 units) |
//! | `request/fixes` | plugin → core | reply | `{fixes: [{priority, message, edits: [{offset, length, replacement}]}]}` |
//! | `request/query` | plugin → core | request (the wire's reverse direction, T4) | `{kind, name?, file?, offset?}` — the five index kinds (`symbol`, `references`, `callers`, `implementations`, `dependencies`) are name-keyed; `element_at` is `{file, offset}`-keyed. The core answers with the same method and the plugin's id; a declined query is answered with `{"error": message}` and the session stays up |
//! | `request/query` | core → plugin | reply | the frozen query envelope `{schema_version, name, declarations, references[, implementations, dependencies]}` for the index kinds; `{schema_version, file, offset, elements}` for `element_at` — the rows are the daemon face's own (schema-identical); no pagination in v1 (the 16 MiB frame bound is the response ceiling) |
//! | `notification/diagnostic` | plugin → core | notification | `{file, severity, code, message, offset, length}` |
//! | `notification/engine_diagnostics` | core → plugin | notification | `{diagnostics: [{file, severity, code, message, offset, length}], files: [path..]}` — `files` is the cycle's analyzed set, the files a rule-executing plugin runs its enabled rules over |
//! | `notification/plugin_config` | core → plugin | notification | `{enabled_rules: [name..], severity_overrides: {code: severity}, overlays: [{file, content}]}` — the resolved per-cycle execution config; `overlays[].content` may be `null` (fall back to disk) |
//!
//! # Handshake and capability negotiation
//!
//! The core opens every session with `initialize`. The plugin's reply version
//! must equal [`PROTOCOL_VERSION`], otherwise the session fails with a
//! [`SessionError::VersionMismatch`]. The effective capability set is the
//! **intersection** of both sides' `capabilities`; unknown capability names
//! on either side mean "not enabled", never an error. Diagnostics may flow
//! only after the handshake completed — a diagnostic arriving earlier is
//! rejected, never queued. `shutdown` is the final exchange: a diagnostic
//! the host wrote before it could see the shutdown request crosses it
//! legitimately on the wire and is queued, never fatal; after the shutdown
//! reply the session is over.
//!
//! Any session error is fatal: the core tears the plugin process down and
//! reports a `plugin_crashed` diagnostic. There
//! is no resynchronization on the wire.
//!
//! # Round trip
//!
//! ```
//! use dartq_plugin::{decode_line, encode_frame, Frame, METHOD_REQUEST_FIXES};
//! use serde_json::json;
//!
//! let frame = Frame::request(7, METHOD_REQUEST_FIXES, json!({
//!     "file": "lib/main.dart", "offset": 10, "length": 2
//! }).as_object().unwrap().clone());
//!
//! // One frame = one line; the caller writes it with a trailing '\n'.
//! let mut line = encode_frame(&frame)?;
//! line.push(b'\n');
//! let decoded = decode_line(&line)?;
//! assert_eq!(decoded, frame);
//! # Ok::<(), dartq_plugin::ProtocolError>(())
//! ```
//!
//! The Dart host implements the exact same schema (package `dartq_plugin_host`,
//! `lib/src/frame.dart`); both files must change together.
//!
//! # Engine wiring and isolation metrics
//!
//! [`runtime::PluginRuntime`] is the engine-facing face this contract serves:
//!
//! - **Process model**: [`discovery`] locates manifests (the
//!   `<workspace>/plugins/*/dartq_plugin.json` convention plus explicit
//!   overrides), and one child process runs per plugin, wire-only contact —
//!   the plugin never holds a core reference. The engine read face is the
//!   existing [`dartq_diagnostics::Diagnostic`] output of
//!   `dartq_engine::Engine::analyze` plus `file_table()`; the runtime
//!   converts and pushes it (`notification/engine_diagnostics`, one batch
//!   per analyze cycle) and drains plugin-emitted diagnostics back for the
//!   caller's output merge. No engine crate changes are required.
//! - **Extension points**: the wire layer is dartq's own v1 (the
//!   official `analyzer_plugin` protocol is pre-1.0 and adapted here, risk
//!   R2); the official `registerAssist`/`registerFixForRule` API shape is
//!   mirrored by the host package's `Plugin`/`PluginRegistry`. The Query API
//!   (T4) rides the wire's reverse direction: a plugin whose `query`
//!   capability was negotiated pulls resolution-owned facts — the daemon's
//!   index query kinds plus `element_at` — through
//!   [`runtime::PluginRuntime::serve_queries`] against the session's
//!   current engine view. Parsed-level AST context stays host-local (the
//!   host parses with its own analyzer); full resolved-context parity is
//!   the recorded R2 evolution path.
//! - **Isolation**: per-plugin counters (peak child RSS, frame/byte
//!   totals per direction, 16 MiB guard drops, restart count) via
//!   [`runtime::PluginRuntime::stats`] → [`stats::PluginStatsSnapshot`];
//!   RSS over the manifest's `memory_budget_mb` kills the child (Linux
//!   `VmHWM`, macOS `proc_pid_rusage`); `request/fixes` runs under the
//!   manifest's `callback_ms` and `initialize`/`shutdown` under the request
//!   deadline; child death restarts within the manifest's retry budget and
//!   never takes the engine down.
//! - **Output face**: [`runtime::crash_diagnostic`] converts a
//!   terminal [`runtime::RuntimeError`] into a `dartq/plugin_crashed` /
//!   `dartq/plugin_timeout` [`dartq_diagnostics::Diagnostic`] for the
//!   daemon's output merge; plugin *findings* stay wire-form
//!   ([`session::PluginDiagnostic`]).
//!
//! # Relationship to `analysis_server_plugin`
//!
//! The lifecycle mirrors the official plugin system's observable contract
//! (upstream Dart SDK source,
//! `pkg/analysis_server_plugin/lib/src/plugin_server.dart`):
//! version check first, then registration/work, then shutdown, with fixes
//! requested per location. The method names are dartq's own (the official
//! wire protocol is pre-1.0 and dartq adapts at this layer).

pub mod capability;
pub mod discovery;
pub mod frame;
pub mod manifest;
pub mod runtime;
pub mod session;
pub mod stats;

pub use capability::{Capability, CapabilitySet};
pub use discovery::{
    DiscoveredPlugin, DiscoveryError, DiscoveryIssue, DiscoveryReport, PubNative, discover,
};
pub use frame::{
    Frame, FrameDecoder, MAX_FRAME_SIZE, METHOD_DIAGNOSTIC_NOTIFICATION, METHOD_ENGINE_DIAGNOSTICS,
    METHOD_INITIALIZE, METHOD_PLUGIN_CONFIG, METHOD_REQUEST_FIXES, METHOD_REQUEST_QUERY,
    METHOD_SHUTDOWN, PROTOCOL_VERSION, ProtocolError, decode_line, encode_frame,
};
pub use manifest::{
    DEFAULT_IDLE_EVICT_MS, DEFAULT_MEMORY_BUDGET_MB, EntrypointKind, MANIFEST_FILE_NAME, Manifest,
    ManifestError, Timeouts,
};
pub use runtime::{
    CycleDrain, PendingQuery, PluginRuntime, RuntimeError, SpawnSpec, StdioPolicy, crash_diagnostic,
};
pub use session::{
    CycleConfig, CyclePush, FixesReply, Incoming, OverlayEntry, PluginDiagnostic,
    PrioritizedChange, QueryKind, QueryRequest, RuleCensus, RuleCodeInfo, RuleKind, Session,
    SessionError, SessionState, Severity, SourceEdit, engine_diagnostics_frame,
    plugin_config_frame,
};
pub use stats::{PluginStats, PluginStatsSnapshot};
