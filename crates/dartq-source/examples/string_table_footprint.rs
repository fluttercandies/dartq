//! Memory footprint measurement for `StringTable`.
//!
//! Run with:
//! `cargo run -p dartq-source --release --example string_table_footprint`
//!
//! A counting global allocator tracks live heap bytes (requested `Layout`
//! sizes, excluding malloc slab slack) in a single-threaded process, so the
//! delta around each phase is attributable to the table alone. `format!`
//! temporaries inside the loops net out to zero live bytes; only table
//! internals and the leaked interned text remain.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            LIVE_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            if new_size >= layout.size() {
                LIVE_BYTES.fetch_add(new_size - layout.size(), Ordering::Relaxed);
            } else {
                LIVE_BYTES.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        new_ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            LIVE_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        ptr
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn live() -> usize {
    LIVE_BYTES.load(Ordering::Relaxed)
}

fn report(label: &str, distinct: usize, calls: usize, payload: usize, before: usize, after: usize) {
    let total = after - before;
    println!("{label}");
    println!("  distinct / intern calls : {distinct} / {calls}");
    println!(
        "  payload                 : {payload} B ({} B/string)",
        payload / distinct
    );
    println!(
        "  live table bytes        : {total} B ({} B/string)",
        total / distinct
    );
    println!(
        "  overhead over payload   : {} B/string",
        (total - payload) / distinct
    );
}

fn main() {
    const N: usize = 100_000;
    const POOL: usize = 20_000;

    // Scenario A: 100k distinct identifier-shaped strings (17 B each).
    let table = dartq_source::StringTable::new();
    let before = live();
    let mut payload = 0;
    for i in 0..N {
        let s = format!("identifier_{i:06}");
        payload += s.len();
        let _ = table.intern(&s);
    }
    let after = live();
    report(
        "scenario A: 100k distinct identifiers",
        N,
        N,
        payload,
        before,
        after,
    );

    // Hit path: re-interning must not move live bytes.
    let hit_before = live();
    for i in 0..N {
        let _ = table.intern(&format!("identifier_{i:06}"));
    }
    let hit_after = live();
    if hit_after == hit_before {
        println!("  hit sweep ({N} re-interns): 0 live-byte movement");
    } else {
        println!("  hit sweep ({N} re-interns): live bytes {hit_before} -> {hit_after}");
    }
    drop(table);

    // Scenario B: 100k intern calls over a 20k distinct pool (80% hits),
    // matching identifier reuse in a real corpus.
    let table = dartq_source::StringTable::new();
    let before = live();
    for call in 0..N {
        let _ = table.intern(&format!("name_{:06}", call % POOL));
    }
    let after = live();
    let payload = POOL * "name_000000".len();
    report(
        "scenario B: 100k calls over a 20k distinct pool (80% hit rate)",
        POOL,
        N,
        payload,
        before,
        after,
    );
}
