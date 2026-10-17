//! Snapshot-flush benchmark (#1386, execution deferred to the serial
//! phase): the daemon's debounced save at residency R — one
//! `Engine::save_facts_snapshot` per flush tick with exactly one changed
//! entry. The #1373 audit (P1-3) found every flush projecting, encoding,
//! and read-comparing every resident entry — O(residency) per 500ms tick
//! even when one file changed; the store's written-keys bookkeeping makes
//! the steady-state flush O(changed). The steady-state walls' flatness
//! across the scales is the evidence; `first_flush_ms` (cold store: every
//! entry a fresh projection + write) is the O(residency) baseline the
//! steady state must sit far below.
//!
//! Per scale, setup (fixture write + discovery + warm analyze) is
//! unmeasured; the cold first flush is measured separately from the loop.
//! Each measured step is one comment-churn edit (unmeasured `set_content`
//! + `analyze` — the daemon cadence) followed by one timed flush. The
//! libraries are self-contained and content-distinct (no imports, no
//! shared bodies), so one edit moves exactly one entry and the save-end
//! sweep removes exactly one.
//!
//! Memory posture matches `apply_residency`: ~4-line bodies, the scales
//! run strictly sequentially, and the peak allocation is the cold first
//! flush materializing one full entry set — the same peak every flush paid
//! before #1386.

mod common;

use std::time::Instant;

use common::{Fixture, millis, percentile};
use dartq_engine::CancellationToken;
use dartq_snapshot::{DEFAULT_CAPACITY_BYTES, SnapshotConfig, SnapshotStore};

/// Residency scales, each its own bench id so `cargo bench` output reads as
/// a growth curve.
const SCALES: [(usize, &str); 3] = [
    (1_000, "1k_files"),
    (10_000, "10k_files"),
    (100_000, "100k_files"),
];

fn main() {
    let steps = std::env::var("DARTQ_BENCH_STEPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    for (files, tag) in SCALES {
        run_scale(files, tag, steps);
    }
    println!(
        "// acceptance axis: marginal per-flush wall at residency R with exactly one changed entry (the daemon's debounced save; edit+analyze unmeasured) — the #1386 O(residency)→O(changed) collapse evidence, against the cold-store `first_flush_ms` baseline"
    );
}

/// Builds, warms, and measures one residency scale.
fn run_scale(files: usize, tag: &str, steps: usize) {
    let libraries = flush_workspace(files);
    let refs: Vec<(&str, &str)> = libraries
        .iter()
        .map(|(path, body)| (path.as_str(), body.as_str()))
        .collect();
    let fixture = Fixture::new(&refs);
    let store = SnapshotStore::open(SnapshotConfig {
        root: Some(fixture.root.join("snapshot-cache")),
        capacity_bytes: DEFAULT_CAPACITY_BYTES,
    })
    .expect("open store");

    let mut engine = fixture.engine();
    engine.discover_workspace().expect("discovery");
    engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("warm analyze");

    // Cold-store flush: every resident entry is a fresh projection +
    // write — the O(residency) shape the bookkeeping exists to remove.
    let first = Instant::now();
    engine.save_facts_snapshot(&store).expect("cold flush");
    let first_flush = first.elapsed();

    // Comment churn on f00000 — same shape, different bytes, alternating
    // so each flush writes one new key and sweeps the previous one (the
    // rejoining-key path the sweep-unrecord discipline serves).
    let edited = format!("// edited\n{}", libraries[0].1);
    let target = fixture.root.join("lib/f00000.dart");

    let mut flush_walls = Vec::with_capacity(steps);
    for step in 0..steps {
        let content = if step % 2 == 0 {
            edited.as_str()
        } else {
            libraries[0].1.as_str()
        };
        let file = engine.set_content(&target, content.as_bytes());
        engine
            .analyze(&[file], &mut |_| {}, &CancellationToken::default())
            .expect("incremental analyze");
        let start = Instant::now();
        engine.save_facts_snapshot(&store).expect("flush");
        flush_walls.push(start.elapsed());
    }
    flush_walls.sort();

    println!(
        "{{\"bench\":\"snapshot_flush/{tag}\",\"files\":{files},\"steps\":{steps},\"first_flush_ms\":{:.1},\"flush_p50_ms\":{:.1},\"flush_p95_ms\":{:.1},\"flush_max_ms\":{:.1}}}",
        millis(first_flush),
        millis(percentile(&flush_walls, 0.50)),
        millis(percentile(&flush_walls, 0.95)),
        millis(percentile(&flush_walls, 1.00)),
    );
}

/// Self-contained, content-distinct libraries: no imports (one edit moves
/// exactly one entry — no cascade) and no shared bodies (R files must map
/// to R distinct entry keys, or the residency axis collapses).
fn flush_workspace(files: usize) -> Vec<(String, String)> {
    (0..files)
        .map(|i| {
            (
                format!("lib/f{i:05}.dart"),
                format!("class C{i:05} {{\n  int m{i:05}() => {i};\n}}\n"),
            )
        })
        .collect()
}
