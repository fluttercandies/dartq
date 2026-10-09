//! The dartq daemon: an LSP shell over the multi-context runtime (PRD §16).
//! One client on stdin/stdout, one reader thread feeding a message channel,
//! and the main loop (this thread) doing all dispatch, engine calls, and
//! writing — the rust-analyzer/lsp-server posture with the thread count the
//! single-client milestone actually needs.
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
//!   and `textDocument/didChange` stage a write task on the session's root
//!   [`Runtime`] context; `textDocument/didClose` stages the on-disk bytes
//!   back (or a removal for an overlay-only buffer). None of this touches
//!   the workspace files on disk.
//! * Scheduling (PRD §16.5, §16.6): each turn batches every already-queued
//!   frame and drains once — writes apply before any read analyzes, N edits
//!   collapse into one apply + one analysis, and `textDocument/
//!   publishDiagnostics` pushes go out for every file whose diagnostic set
//!   changed (diff against the last publish). `dartq/analyze` queues a read
//!   stamped with the context's enqueue epoch; `$/cancelRequest` bumps the
//!   epoch, the stale task drops at the drain, and its requests answer
//!   `RequestCancelled` (-32800) with no result.
//! * `dartq/analyze`: dartq's own pull face — the merged diagnostic set of
//!   a fresh full analysis, one entry per diagnostic with its document URI.
//! * Everything else: unknown requests answer `MethodNotFound`; unknown
//!   notifications are ignored (both per LSP).
//!
//! # Snapshot persistence (#306, #309)
//!
//! After overlay applies settle, the debounced sink saves one snapshot
//! generation: 500 ms after the last apply, checked at turn boundaries (the
//! loop is turn-based — the window elapses across the blocking receive, no
//! timer thread), plus an unconditional flush at session end. Saves are
//! accelerator semantics: failures log and continue. The generation payload
//! itself waits on engine faces still missing (see the `session` module
//! docs); until they land each flush logs the gap and writes nothing.
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

// Lets the shared integration fixture (tests/common) compile inside the
// lib's own unit-test build too, where the crate is not an --extern.
#[cfg(test)]
extern crate self as dartq_daemon;
#[cfg(test)]
#[path = "../tests/common/mod.rs"]
mod test_fixtures;

mod server;

pub use server::{EXIT_CLEAN, EXIT_WITHOUT_SHUTDOWN, run};
pub use session::{Publish, Session, SessionError, Turn};
