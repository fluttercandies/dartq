//! Allocation probe for the warm analyze merge path (#234, the #308/string-
//! table measurement posture): a counting global allocator tallies allocation
//! calls and requested bytes across one zero-dirty analyze — the Incremental
//! P95 shape — over a five-library workspace.
//!
//! The per-library runs are stored pre-sorted and move into the cache
//! without a clone, so the warm path must pay only the output clones (one
//! message allocation per diagnostic) plus O(libraries) fixed costs: the run
//! table, the merge heap, and the output vec. The retired path paid at least
//! three allocations per diagnostic on this same window (output clone, path
//! key, message key) plus an O(D log D) re-sort of the cached sets. The
//! ceiling below trips if per-diagnostic key materialization or a storage
//! clone ever returns.
//!
//! One test per binary with a counting global allocator (the measurement is
//! process-global; the window is exact because this binary has one test).
//!
//! Run: `cargo test -p dartq-engine --test merge_memory -- --nocapture`

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use common::Fixture;
use dartq_engine::CancellationToken;

struct Counting;

static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, AtomicOrdering::Relaxed);
        BYTES.fetch_add(layout.size() as u64, AtomicOrdering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        BYTES.fetch_sub(layout.size() as u64, AtomicOrdering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, AtomicOrdering::Relaxed);
        BYTES.fetch_add(layout.size() as u64, AtomicOrdering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, AtomicOrdering::Relaxed);
        BYTES.fetch_add(new_size as u64, AtomicOrdering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Counts calls (not net live bytes): the regression this guards is
/// per-diagnostic materialization on the warm path, so the window wraps
/// exactly one zero-dirty analyze. Fixed overhead is O(libraries); the
/// per-diagnostic ceiling is 2.0 allocations (message + optional
/// correction), decisively below the retired path's ≥ 3.
#[test]
#[allow(clippy::cast_precision_loss)]
fn warm_analyze_allocates_once_per_diagnostic() {
    const LIBRARIES: usize = 5;
    const CALLS_PER_LIBRARY: usize = 120;

    let mut calls = String::new();
    for i in 0..CALLS_PER_LIBRARY {
        let _ = writeln!(calls, "  nope{i}();");
    }
    let body = format!("void main() {{\n{calls}}}\n");
    let paths = [
        "lib/a.dart",
        "lib/m1.dart",
        "lib/m2.dart",
        "lib/y.dart",
        "lib/z.dart",
    ];
    let libraries: Vec<(&str, &str)> = paths.iter().map(|&path| (path, body.as_str())).collect();
    let fixture = Fixture::new(&libraries);
    let mut engine = fixture.engine();
    for path in paths {
        engine.set_content(&fixture.path(path), body.as_bytes());
    }

    // Cold run: builds the result cache outside the measurement window.
    let cold = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    let diagnostics = cold.len();
    assert!(
        diagnostics >= 300,
        "fixture must produce a diagnostic mass, got {diagnostics}"
    );

    // Warm run: zero dirty libraries — the P95 incremental shape.
    ALLOCS.store(0, AtomicOrdering::Relaxed);
    BYTES.store(0, AtomicOrdering::Relaxed);
    let warm = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("warm");
    let allocs = ALLOCS.load(AtomicOrdering::Relaxed);
    let bytes = BYTES.load(AtomicOrdering::Relaxed);

    assert_eq!(warm, cold, "warm merge must byte-match the cold pass");
    let per_diagnostic = allocs as f64 / diagnostics as f64;
    assert!(
        (allocs as f64) <= 40.0 + 2.0 * diagnostics as f64,
        "warm analyze ballooned: {allocs} allocations for {diagnostics} diagnostics ({per_diagnostic:.2}/diagnostic)"
    );
    println!(
        "warm analyze: {allocs} allocations ({bytes} bytes requested) for {diagnostics} \
         diagnostics across {LIBRARIES} libraries — {per_diagnostic:.2} allocs/diagnostic"
    );
}
