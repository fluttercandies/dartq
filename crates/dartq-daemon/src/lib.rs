//! The dartq daemon: an LSP shell over the multi-context runtime (PRD §16).
//! One client on stdin/stdout, one reader thread feeding a message channel,
//! and the main loop (this thread) doing all dispatch, engine calls, and
//! writing — the rust-analyzer/lsp-server posture with the thread count the
//! single-client milestone actually needs.
//!
//! # Runtime posture
//!
//! Zero async, zero tokio: the engine is CPU-bound and synchronous, so the
//! shell is std threads + a channel. The intake channel is
//! `crossbeam-channel` — one reader (plus the optional watch thread) → one
//! consumer, with the multi-producer channel the topology actually is, and
//! the select surface the answer pool's results ride (#487). Read answering
//! fans out across worker threads over the shared engine view (each read
//! task's analyzed view is `&`-shared; writes and analyses stay
//! single-owner); the main loop remains the only writer.
//!
//! # Protocol face (the milestone's minimal typed surface)
//!
//! * Lifecycle: `initialize` (capabilities: full text sync) → `initialized`
//!   → requests → `shutdown` (answers `null`) → `exit` (process exit 0;
//!   exit without shutdown exits 1, per LSP). Requests before `initialize`
//!   answer `ServerNotInitialized`.
//! * Overlay (PRD §16.3, M1 whole-file replacement): `textDocument/didOpen`
//!   and `textDocument/didChange` stage a write task that fans out to the
//!   session's contexts; `textDocument/didClose` stages the on-disk bytes
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
//!   labels), plus the per-agent overlay faces `dartq/overlay.set`,
//!   `dartq/overlay.commit` (the PRD §16.3 merge: the agent's staged
//!   buffers write back to the worktree and merge into the base view), and
//!   `dartq/overlay.clear`. All are announced under
//!   `capabilities.experimental`; all answers carry `schema_version` and
//!   honor an optional `verbosity` (the canonical tiers in dartq-schema's
//!   crate docs); the engine-view faces additionally accept `context` (the
//!   multi-root routing tag, PRD §16.2), `agentId` (the analyzed overlay
//!   view), and `since` (delta mode), with the per-face visibility contract
//!   documented in the `commands` module.
//! * Everything else: unknown requests answer `MethodNotFound`; unknown
//!   notifications are ignored (both per LSP).
//!
//! # Snapshot persistence (#306, #309)
//!
//! After overlay applies settle, the debounced sink saves every context's
//! facts generation through the engine's flush face: 500 ms after the last
//! apply, checked at turn boundaries, plus an unconditional flush at
//! session end. The header's context salt separates forked contexts'
//! generation directories (#482), so N contexts coexist collision-free in
//! one store; session construction warm-starts each context from its
//! matching generation. While the sink is dirty the main loop's receive is
//! bounded by the window's remaining time, so an *idle* daemon wakes at the
//! deadline to save instead of blocking forever — still no timer thread.
//! Saves are accelerator semantics: failures log and continue.
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
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

    thread_local! {
        /// Diagnostic conversions this thread performed (one per diagnostic
        /// per read task — a second call site would double it). Thread-local
        /// on purpose: every test drives its session synchronously on its
        /// own harness thread, so a process-global tally would be inflated
        /// by concurrent tests converting between a reset and its assert.
        static CONVERSIONS: Cell<usize> = const { Cell::new(0) };
        /// One-shot panic trigger for the containment test (a real engine
        /// panic cannot be conjured deterministically from RPC input).
        /// Thread-local so a parallel test's read group cannot steal an
        /// armed trigger between `arm` and the owner's own turn.
        static READ_PANIC: Cell<bool> = const { Cell::new(false) };
        /// Only the measuring test's thread counts, so concurrent tests on
        /// other threads (parallel harnesses) never pollute the numbers.
        static ARMED: Cell<bool> = const { Cell::new(false) };
        static BULK_BYTES: Cell<usize> = const { Cell::new(0) };
    }

    /// Snapshot-sink saves attempted (the idle deadline wake shows up here).
    /// Deliberately process-global: the idle-flush test reads it from its
    /// harness thread while its server thread performs the saves, so a
    /// thread-local tally would be invisible. That test is a serial-phase
    /// gate (real-time window), where no concurrent test can pollute it.
    static FLUSHES: AtomicUsize = AtomicUsize::new(0);

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
        CONVERSIONS.with(|count| count.set(count.get() + 1));
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

    /// The counters since the last reset, for the measuring test's window:
    /// conversions counted on the calling thread, flushes process-wide.
    pub(crate) fn counters() -> (usize, usize) {
        (CONVERSIONS.with(Cell::get), FLUSHES.load(Relaxed))
    }

    pub(crate) fn reset_counters() {
        CONVERSIONS.with(|count| count.set(0));
        FLUSHES.store(0, Relaxed);
    }

    /// Arms the next read group's execution to panic once.
    pub(crate) fn arm_read_panic() {
        READ_PANIC.with(|armed| armed.set(true));
    }

    pub(crate) fn take_read_panic() -> bool {
        READ_PANIC.with(|armed| armed.replace(false))
    }
}
