//! The dartq daemon: the resident RPC shell over the multi-context runtime.
//! One client on stdin/stdout, one reader thread feeding a message channel,
//! and the main loop (this thread) doing all dispatch, engine calls, and
//! writing — the rust-analyzer main-loop posture with the thread count the
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
//! # Protocol face
//!
//! * No handshake gate: the session is ready from `run()`'s start — the
//!   workspace sweep and its first `dartq/diagnostics` pushes land ahead
//!   of the first client frame. A client close (end of stream) ends the
//!   session with [`EXIT_CLEAN`].
//! * Change intake: `dartq/sync` notifications name the paths the caller
//!   wrote (the explicit change-report face; `--watch` is the convenience
//!   equivalent polling the same files). Each sync stages a write task
//!   that fans out to the session's contexts.
//! * Scheduling: each turn batches every already-queued
//!   frame and drains once — writes apply before any read analyzes, N edits
//!   collapse into one apply + one analysis, and `dartq/diagnostics`
//!   pushes go out for every file whose diagnostic set changed (a diff
//!   against the last publish; a push replaces the client's set for that
//!   URI, so the empty array clears it). A `dartq/*` request queues a
//!   read stamped with the context's enqueue epoch; `dartq/cancel` bumps
//!   the epoch, the stale task drops at the drain, and its requests answer
//!   `RequestCancelled` (-32800) with no result.
//! * The `dartq/*` command family (the RPC faces 1:1 with the
//!   CLI commands): `dartq/analyze` (the merged diagnostic set, `file:` URIs),
//!   `dartq/check`, `dartq/query`, `dartq/impact`, `dartq/explain`,
//!   `dartq/fix`, and `dartq/compat` (CLI-mirror faces with canonical path
//!   labels), plus the per-agent overlay faces `dartq/overlay.set`,
//!   `dartq/overlay.commit` (the merge: the agent's staged
//!   buffers write back to the worktree and merge into the base view), and
//!   `dartq/overlay.clear`. All answers carry `schema_version` and
//!   honor an optional `verbosity` (the canonical tiers in dartq-schema's
//!   crate docs); the engine-view faces additionally accept `context` (the
//!   multi-root routing tag), `agentId` (the analyzed overlay
//!   view), and `since` (delta mode), with the per-face visibility contract
//!   documented in the `commands` module.
//! * The `dartq/debug/*` family (daemon-native, no CLI mirror): launch and
//!   attach debug sessions through `dartq-debug`, then drive them —
//!   breakpoints, exception pause mode, resume/pause/step, stopped
//!   snapshots with variables, and cursor-paged event/output reads.
//!   These faces target the session's debug registry, never the analysis
//!   engine, and answer at intake with synchronous, config-bounded
//!   session calls (the `debug` module's notes).
//! * Everything else: unknown requests answer `MethodNotFound`; unknown
//!   notifications are ignored.
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
//! # File watch
//!
//! The optional convenience layer: --watch polls the workspace on one
//! bounded-interval thread (no OS-event substrate in the dependency tree —
//! documented as such), debounces bursts, and feeds the debounced batches
//! through the reader channel as `dartq/sync`-equivalent overlay syncs at
//! `Tier::Normal`. Watch events are hints; the analysis's content check
//! stays the truth.
//!
//! # Transport
//!
//! Hand-written `Content-Length` framing ([`codec`]) — the shell layer has
//! no Dart semantics to inherit, so no RPC library is pulled in. The reader
//! thread decodes frames and sends [`jsonrpc::Message`] values; only the
//! main loop writes, so the output side needs no lock. A framing violation
//! (malformed or duplicate headers, a truncated frame body) ends the
//! session with exit code [`EXIT_PROTOCOL`] after one best-effort
//! `ParseError` answer, so supervision can tell a broken client from a
//! clean close (exit 0) — #947 F10.

pub mod codec;
pub(crate) mod commands;
mod debug;
pub mod jsonrpc;
pub(crate) mod plugins;
pub mod session;
mod watch;
pub mod wire;

// The `file:` URI mapping lives with the shared per-command cores (the
// analyze faces' document-URI rows use it too); re-exported so `crate::uri`
// keeps resolving for the overlay-key call sites.
pub use dartq_commands::uri;

// Lets the shared integration fixture (tests/common) compile inside the
// lib's own unit-test build too, where the crate is not an --extern.
#[cfg(test)]
extern crate self as dartq_daemon;
#[cfg(test)]
#[path = "../tests/common/mod.rs"]
mod test_fixtures;

mod server;

pub use commands::METHODS;
pub use debug::DEBUG_REQUESTS;
pub use server::{EXIT_CLEAN, EXIT_PROTOCOL, INTAKE_REQUESTS, SERVED_NOTIFICATIONS, run};
pub use session::{Publish, Session, SessionError, SessionRuntime, Turn};
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
        /// Same seam for the write-task arm (the apply path's containment
        /// pin): one-shot, thread-local.
        static WRITE_PANIC: Cell<bool> = const { Cell::new(false) };
        /// One-shot panic trigger for the plugin-weave containment test (a
        /// real plugin-path panic cannot be conjured deterministically from
        /// RPC input). Thread-local, same discipline as READ_PANIC.
        static WEAVE_PANIC: Cell<bool> = const { Cell::new(false) };
        /// The overlay set the last plugin weave built (#1819's push pin:
        /// the staged documents' path/text pairs handed to the cycle
        /// context). Thread-local like the rest: tests drive their session
        /// on the harness thread.
        static WEAVE_OVERLAYS: std::cell::RefCell<Vec<(String, String)>> =
            const { std::cell::RefCell::new(Vec::new()) };
        /// How many read groups the armed READ_PANIC lets pass before it
        /// fires: the #690 containment tests need a later group surviving an
        /// earlier group's panic, so the trigger must target a group.
        static READ_PANIC_SKIP: Cell<usize> = const { Cell::new(0) };
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
        READ_PANIC_SKIP.with(|skip| skip.set(0));
    }

    /// [`arm_read_panic`], firing on the (n+1)th read group of the turn
    /// instead of the first — group targeting for the containment tests
    /// (groups sort base first, then agents in first-appearance order).
    pub(crate) fn arm_read_panic_on_group(skip: usize) {
        READ_PANIC.with(|armed| armed.set(true));
        READ_PANIC_SKIP.with(|left| left.set(skip));
    }

    pub(crate) fn take_read_panic() -> bool {
        let skip = READ_PANIC_SKIP.with(Cell::get);
        if skip > 0 {
            READ_PANIC_SKIP.with(|left| left.set(skip - 1));
            return false;
        }
        READ_PANIC.with(|armed| armed.replace(false))
    }

    /// Arms the next plugin weave's execution to panic once (the weave-level
    /// containment pin, inside the group boundary).
    pub(crate) fn arm_weave_panic() {
        WEAVE_PANIC.with(|armed| armed.set(true));
    }

    pub(crate) fn take_weave_panic() -> bool {
        WEAVE_PANIC.with(|armed| armed.replace(false))
    }

    /// Records the overlay set a plugin weave built (the `CycleContext`'s
    /// `overlays` argument).
    pub(crate) fn record_weave_overlays(overlays: &[(String, String)]) {
        WEAVE_OVERLAYS.with(|seen| *seen.borrow_mut() = overlays.to_vec());
    }

    /// Drains the recorded weave overlays (each test reads its own turn's).
    pub(crate) fn take_weave_overlays() -> Vec<(String, String)> {
        WEAVE_OVERLAYS.with(|seen| std::mem::take(&mut *seen.borrow_mut()))
    }

    /// Arms the next write task's apply to panic once (the write path's
    /// containment pin).
    pub(crate) fn arm_write_panic() {
        WRITE_PANIC.with(|armed| armed.set(true));
    }

    pub(crate) fn take_write_panic() -> bool {
        WRITE_PANIC.with(|armed| armed.replace(false))
    }
}
