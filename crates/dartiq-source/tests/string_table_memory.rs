//! Measured per-entry footprint of [`StringTable`] (ast-layout-spec §5 note).
//!
//! One test per binary with a counting global allocator: net live bytes
//! (allocations minus frees) around interning 10k distinct 12-byte strings,
//! minus the payload itself, divided by the entry count. The reported figure
//! includes power-of-two growth slack of the id/origin vecs and the hash
//! bucket array at exactly 10k entries; the doc note in ast-layout-spec §5
//! quotes this measurement.
//!
//! Run: `cargo test -p dartiq-source --test string_table_memory -- --nocapture`

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

use dartiq_source::StringTable;

struct Counting;

static NET_LIVE: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        NET_LIVE.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        NET_LIVE.fetch_sub(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Counts requested (not OS-rounded) sizes, so the figure is allocator-
/// independent. The 128 B/entry ceiling is a loose regression guard placed
/// above the measured value (see the printed number), tight enough to trip
/// if the table ever starts copying text more than once per entry.
///
/// The `cast_precision_loss` allowance is sound by construction: every value
/// cast to `f64` here is below 2^52 (≈ 4 PiB of bytes), where f64 is exact.
#[test]
#[allow(clippy::cast_precision_loss)]
fn string_table_per_entry_overhead() {
    const N: u64 = 10_000;
    const PAYLOAD_LEN: u64 = 12; // "id0000000001" style, exactly 12 bytes

    let table = StringTable::new();
    let _ = table.intern("warm"); // one-time init outside the measurement window

    let before = NET_LIVE.load(Ordering::Relaxed);
    for i in 0..N {
        let s = format!("id{i:010}");
        let _ = table.intern(&s);
    }
    let delta = NET_LIVE.load(Ordering::Relaxed) - before;

    let per_entry = (delta - N * PAYLOAD_LEN) as f64 / N as f64;
    println!(
        "StringTable per-entry overhead over {N} entries of {PAYLOAD_LEN}B payload: {per_entry:.1} bytes/entry (net live bytes, incl. growth slack)"
    );
    assert!(
        per_entry < 128.0,
        "per-entry overhead ballooned: {per_entry:.1} bytes/entry"
    );
    // The leaked text stays live after the handle is gone: re-resolving is a
    // pure read, and the net byte count does not move on repeated interning.
    let id = table.intern("id0000000000");
    assert_eq!(table.resolve(id), "id0000000000");
    let after = NET_LIVE.load(Ordering::Relaxed);
    assert_eq!(after, delta + before, "intern hits allocate nothing");
}
