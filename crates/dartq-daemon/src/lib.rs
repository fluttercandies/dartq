//! The dartq daemon: an LSP shell over the synchronous engine (PRD §16,
//! runtime policy). One client on stdin/stdout, one reader thread feeding a
//! message channel, and the main loop (this thread) doing all dispatch,
//! engine calls, and writing — the rust-analyzer/lsp-server posture with the
//! thread count the single-client milestone actually needs.
//!
//! # Runtime posture
//!
//! Zero async, zero tokio: the engine is CPU-bound and synchronous, so the
//! shell is std threads + a channel. The channel is [`std::sync::mpsc`] —
//! the topology is strictly one reader → one consumer, so `mpsc` is exactly
//! sufficient and keeps the dependency tree unchanged (crossbeam-channel is
//! not in the workspace lock; swapping it in would be a one-line change if a
//! multi-consumer topology ever lands).
//!
//! # Protocol face (the milestone's minimal typed surface)
//!
//! * Lifecycle: `initialize` (capabilities: full text sync) → `initialized`
//!   → requests → `shutdown` (answers `null`) → `exit` (process exit 0;
//!   exit without shutdown exits 1, per LSP). Requests before `initialize`
//!   answer `ServerNotInitialized`.
//! * Overlay (PRD §16.3, M1 whole-file replacement): `textDocument/didOpen`
//!   and `textDocument/didChange` feed [`Session`], which drives
//!   [`Engine::set_content`] + a full [`Engine::analyze`] and pushes
//!   `textDocument/publishDiagnostics` for every file whose diagnostic set
//!   changed (diff against the last publish).
//! * `textDocument/didClose` drops the overlay by restoring the on-disk
//!   bytes (the engine has no per-file "clear overlay" face; replacing with
//!   the disk content is the equivalent whole-file operation).
//! * `dartq/analyze`: dartq's own pull face — a fresh full analysis whose
//!   merged diagnostic set comes back as the response payload.
//! * Everything else: unknown requests answer `MethodNotFound`; unknown
//!   notifications are ignored (both per LSP).
//!
//! # Transport
//!
//! Hand-written `Content-Length` framing ([`codec`]) — the shell layer has
//! no Dart semantics to inherit, so no LSP library is pulled in. The reader
//! thread decodes frames and sends [`jsonrpc::Message`] values; only the
//! main loop writes, so the output side needs no lock.

pub mod codec;
pub mod jsonrpc;
pub mod lsp;
pub mod session;
pub mod uri;

mod server;

pub use server::{EXIT_CLEAN, EXIT_WITHOUT_SHUTDOWN, run};
pub use session::{Publish, Session, SessionError};
