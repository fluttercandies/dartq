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
//!   changed (diff against the last publish). A `dartq/*` request queues a
//!   read stamped with the context's enqueue epoch; `$/cancelRequest` bumps
//!   the epoch, the stale task drops at the drain, and its requests answer
//!   `RequestCancelled` (-32800) with no result.
//! * The `dartq/*` command family (PRD §15.1 — the RPC faces 1:1 with the
//!   CLI commands): `dartq/analyze` (the merged diagnostic set, LSP URIs),
//!   `dartq/check`, `dartq/query`, `dartq/impact`, `dartq/explain`,
//!   `dartq/fix`, and `dartq/compat` (CLI-mirror faces with canonical path
//!   labels), plus the per-agent overlay faces `dartq/overlay.set` and
//!   `dartq/overlay.clear` (PRD §16.3). All are announced under
//!   `capabilities.experimental`; all answers carry `schema_version` and
//!   honor an optional `verbosity` (the canonical tiers in dartq-schema's
//!   crate docs); the engine-view faces additionally accept `agentId` (the
//!   analyzed overlay view) and `since` (delta mode), with the per-face
//!   visibility contract documented in the `commands` module.
//! * Everything else: unknown requests answer `MethodNotFound`; unknown
//!   notifications are ignored (both per LSP).
//!
//! # Snapshot persistence (#306, #309)
//!
//! After overlay applies settle, the debounced sink saves one snapshot
//! generation: 500 ms after the last apply, checked at turn boundaries, plus
//! an unconditional flush at session end. While the sink is dirty the main
//! loop's receive is bounded by the window's remaining time, so an *idle*
//! daemon wakes at the deadline to save instead of blocking forever — still
//! no timer thread. Saves are accelerator semantics: failures log and
//! continue. The generation payload itself waits on engine faces still
//! missing (see the `session` module docs); until they land each flush logs
//! the gap and writes nothing.
//!
//! # File watch (`--watch`, PRD §16.7 mode 3)
//!
//! The optional convenience layer: --watch polls the workspace on one
//! bounded-interval thread (no OS-event substrate in the dependency tree —
//! documented as such), debounces bursts, and feeds the debounced batches
//! through the reader channel as didChange-equivalent overlay syncs at
//! `Tier::Normal`. Watch events are hints; the analysis's content check
//! stays the truth.
//!
//! # Transport
//!
//! Hand-written `Content-Length` framing ([`codec`]) — the shell layer has
//! no Dart semantics to inherit, so no LSP library is pulled in. The reader
//! thread decodes frames and sends [`jsonrpc::Message`] values; only the
//! main loop writes, so the output side needs no lock.

pub mod codec;
pub(crate) mod commands;
pub mod jsonrpc;
pub mod lsp;
pub(crate) mod plugins;
pub mod session;
pub mod uri;
mod watch;

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
pub use watch::WatchOptions;

/// Test-only observability for the daemon's hot-path allocation discipline
/// (#347): a counting wrapper over the system allocator plus counters on
/// the conversion and flush paths. Everything here is compiled out outside
/// `cfg(test)`.
#[cfg(test)]
pub(crate) mod probe {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};

    /// Diagnostic conversions this process performed (one per diagnostic
    /// per read task — a second call site would double it).
    static CONVERSIONS: AtomicUsize = AtomicUsize::new(0);
    /// Snapshot-sink saves attempted (the idle deadline wake shows up here).
    static FLUSHES: AtomicUsize = AtomicUsize::new(0);

    thread_local! {
        /// Only the measuring test's thread counts, so concurrent tests on
        /// other threads (parallel harnesses) never pollute the numbers.
        static ARMED: Cell<bool> = const { Cell::new(false) };
        static BULK_BYTES: Cell<usize> = const { Cell::new(0) };
    }

    /// Allocations at or above this size are bulk (whole-file texts);
    /// diagnostic- and JSON-sized traffic stays orders of magnitude below.
    const BULK_MIN: usize = 4096;

    pub(crate) struct ProbeAllocator;

    impl ProbeAllocator {
        fn note_bulk(layout: Layout) {
            ARMED.with(|armed| {
                if armed.get() {
                    BULK_BYTES.with(|total| total.set(total.get() + layout.size()));
                }
            });
        }
    }

    unsafe impl GlobalAlloc for ProbeAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if layout.size() >= BULK_MIN {
                Self::note_bulk(layout);
            }
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { System.dealloc(ptr, layout) }
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            if layout.size() >= BULK_MIN {
                Self::note_bulk(layout);
            }
            unsafe { System.alloc_zeroed(layout) }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            unsafe { System.realloc(ptr, layout, new_size) }
        }
    }

    #[global_allocator]
    static ALLOCATOR: ProbeAllocator = ProbeAllocator;

    pub(crate) fn count_conversion() {
        CONVERSIONS.fetch_add(1, Relaxed);
    }

    pub(crate) fn count_flush() {
        FLUSHES.fetch_add(1, Relaxed);
    }

    /// Starts counting bulk allocations on the calling thread, resetting the
    /// tally.
    pub(crate) fn arm_bulk() {
        BULK_BYTES.with(|total| total.set(0));
        ARMED.with(|armed| armed.set(true));
    }

    /// Stops counting and returns the bulk bytes seen since [`arm_bulk`].
    pub(crate) fn bulk_bytes() -> usize {
        ARMED.with(|armed| armed.set(false));
        BULK_BYTES.with(Cell::take)
    }

    /// The counters since the last reset, for the measuring test's window.
    pub(crate) fn counters() -> (usize, usize) {
        (CONVERSIONS.load(Relaxed), FLUSHES.load(Relaxed))
    }

    pub(crate) fn reset_counters() {
        CONVERSIONS.store(0, Relaxed);
        FLUSHES.store(0, Relaxed);
    }

    /// One-shot panic trigger for the containment test (a real engine panic
    /// cannot be conjured deterministically from RPC input).
    static READ_PANIC: AtomicBool = AtomicBool::new(false);

    /// Arms the next read group's execution to panic once.
    pub(crate) fn arm_read_panic() {
        READ_PANIC.store(true, Relaxed);
    }

    pub(crate) fn take_read_panic() -> bool {
        READ_PANIC.swap(false, Relaxed)
    }
}
