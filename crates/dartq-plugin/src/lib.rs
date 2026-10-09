//! dartiq plugin protocol: the IPC contract between the Rust core and the
//! per-plugin Dart host process (PRD §14).
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
//! UTF-8 JSON object per line, at most [`MAX_FRAME_SIZE`] (16 MiB) bytes.
//! Every frame carries the same four fields:
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
//! | `initialize` | plugin → core | reply | `{protocol_version, capabilities: [..]}` |
//! | `shutdown` | core → plugin | request | `{}` |
//! | `shutdown` | plugin → core | reply | `{}` |
//! | `request/fixes` | core → plugin | request | `{file, offset, length}` (UTF-16 units) |
//! | `request/fixes` | plugin → core | reply | `{fixes: [{priority, message, edits: [{offset, length, replacement}]}]}` |
//! | `notification/diagnostic` | plugin → core | notification | `{file, severity, code, message, offset, length}` |
//!
//! # Handshake and capability negotiation
//!
//! The core opens every session with `initialize`. The plugin's reply version
//! must equal [`PROTOCOL_VERSION`], otherwise the session fails with a
//! [`SessionError::VersionMismatch`]. The effective capability set is the
//! **intersection** of both sides' `capabilities`; unknown capability names
//! on either side mean "not enabled", never an error. Diagnostics may flow
//! only after the handshake completed — a diagnostic arriving earlier is
//! rejected, never queued. `shutdown` is the final exchange; after its reply
//! the session is over.
//!
//! Any session error is fatal: the core tears the plugin process down and
//! reports a `plugin_crashed` diagnostic (PRD §14.3 crash isolation). There
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
//! let line = encode_frame(&frame)?;
//! let decoded = decode_line(format!("{line}\n").as_bytes())?;
//! assert_eq!(decoded, frame);
//! # Ok::<(), dartq_plugin::ProtocolError>(())
//! ```
//!
//! The Dart host implements the exact same schema (package `dartq_plugin_host`,
//! `lib/src/frame.dart`); both files must change together.
//!
//! # Relationship to `analysis_server_plugin`
//!
//! The lifecycle mirrors the official plugin system's observable contract
//! (`.third/dart-sdk/pkg/analysis_server_plugin/lib/src/plugin_server.dart`):
//! version check first, then registration/work, then shutdown, with fixes
//! requested per location. The method names are dartiq's own (the official
//! wire protocol is pre-1.0 and dartiq adapts at this layer, PRD §14.1).

pub mod capability;
pub mod frame;
pub mod manifest;
pub mod session;

pub use capability::{Capability, CapabilitySet};
pub use frame::{
    Frame, FrameDecoder, MAX_FRAME_SIZE, METHOD_DIAGNOSTIC_NOTIFICATION, METHOD_INITIALIZE,
    METHOD_REQUEST_FIXES, METHOD_SHUTDOWN, PROTOCOL_VERSION, ProtocolError, decode_line,
    encode_frame,
};
pub use manifest::{
    DEFAULT_MEMORY_BUDGET_MB, MANIFEST_FILE_NAME, Manifest, ManifestError, Timeouts,
};
pub use session::{
    FixesReply, Incoming, PluginDiagnostic, PrioritizedChange, Session, SessionError, SessionState,
    Severity, SourceEdit,
};
