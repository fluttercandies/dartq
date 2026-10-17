//! Cold full-analysis benchmark skeleton (execution deferred to the
//! serial phase): fresh engine → discover → analyze, wall clocks per
//! iteration plus the discovery counters the metrics hook pairs with
//! RSS/100k LOC (the RSS axis itself is the dartq-cli `ru_maxrss` hook —
//! the engine contributes only numbers via `DiscoveryStats` and events).
//!
//! Warm-start slice (`DARTQ_BENCH_WARMSTART=1`, #1387): one fixture of
//! independent single-file libraries, two sessions — the cold session's
//! discover+analyze wall, then its facts snapshot saved to a throwaway
//! store, then a fresh engine that ingests the generation and runs the
//! identical wall. One JSON line plus the ratio: the warm wall's apply
//! serves the batch from staging instead of recomputing, which is the
//! whole warm-start product. `DARTQ_BENCH_WARMSTART_FILES` scales the
//! workspace.
//!
//! Cost axis: whole-run amortized. Every wall includes engine construction,
//! discovery, and the full analysis — cold cost in full. Any per-unit figure
//! derived from these walls (per-LOC, per-diagnostic, per-file) inherits
//! that cold cost and is amortized whole-run; it is never comparable to the
//! marginal per-edit/per-query axes of the warm benches.

mod common;

use std::time::Instant;

use common::{Fixture, LIB_A, LIB_B, LIB_C, millis, percentile};
use dartq_engine::{CancellationToken, PersistedFacts};
use dartq_snapshot::{SnapshotConfig, SnapshotStore};

fn main() {
    let iterations = std::env::var("DARTQ_BENCH_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    let fixture = Fixture::new(&[
        ("lib/a.dart", LIB_A),
        ("lib/b.dart", LIB_B),
        ("lib/c.dart", LIB_C),
    ]);

    let mut walls = Vec::with_capacity(iterations);
    let mut reported_stats = None;
    for _ in 0..iterations {
        let start = Instant::now();
        let mut engine = fixture.engine();
        let stats = engine.discover_workspace().expect("discovery");
        let mut events = 0usize;
        let diagnostics = engine
            .analyze(&[], &mut |_| events += 1, &CancellationToken::default())
            .expect("cold analyze");
        walls.push(start.elapsed());
        reported_stats = Some((stats, diagnostics.len(), events));
    }
    walls.sort();

    let (stats, diagnostic_count, events) = reported_stats.expect("ran");
    println!(
        "{{\"bench\":\"cold_full_analysis\",\"iterations\":{iterations},\"files\":{},\"loc\":{},\"diagnostics\":{},\"events\":{},\"p50_ms\":{:.1},\"p95_ms\":{:.1},\"max_ms\":{:.1}}}",
        stats.file_count,
        stats.loc_total,
        diagnostic_count,
        events,
        millis(percentile(&walls, 0.50)),
        millis(percentile(&walls, 0.95)),
        millis(percentile(&walls, 1.00)),
    );
    println!(
        "// acceptance axis: cold full-analysis vs `dart analyze` (oracle run deferred; pair with dartq-cli ru_maxrss for RSS/100k LOC) — whole-run amortized: every wall carries full cold cost, so per-LOC/per-diagnostic derivations are amortized, never marginal"
    );
    if std::env::var("DARTQ_BENCH_WARMSTART").is_ok() {
        let files = std::env::var("DARTQ_BENCH_WARMSTART_FILES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(400);
        run_warmstart(files);
    }
}

/// The warm-start axis (#1387): the cold session's first-analyze wall
/// against a fresh engine's, whose apply batch serves the same files from
/// the staged facts generation. Both walls carry full cold cost (engine
/// build + discovery + analyze), so the delta is exactly the staging
/// product; the staging retention means a session whose later edits revert
/// files keeps serving entries the single-shot drain would have dropped.
fn run_warmstart(files: usize) {
    let cancel = CancellationToken::default();
    let libraries: Vec<(String, String)> = (0..files)
        .map(|i| {
            (
                format!("lib/m{i}.dart"),
                format!("int f{i}() => {i};\nvoid top{i}() {{ print(f{i}); }}\n"),
            )
        })
        .collect();
    let refs: Vec<(&str, &str)> = libraries
        .iter()
        .map(|(path, body)| (path.as_str(), body.as_str()))
        .collect();
    let fixture = Fixture::new(&refs);

    // Arm 1 — the cold session, and the snapshot it leaves behind.
    let mut writer = fixture.engine();
    let store = SnapshotStore::open(SnapshotConfig {
        root: Some(fixture.root.join("snapshot-store")),
        capacity_bytes: 64 << 20,
    })
    .expect("throwaway store opens");
    let start = Instant::now();
    writer.discover_workspace().expect("discovery");
    writer
        .analyze(&[], &mut |_| {}, &cancel)
        .expect("cold analyze");
    let cold = start.elapsed();
    let saved = writer.save_facts_snapshot(&store).expect("facts flush");
    drop(writer);

    // Arm 2 — the warm start: ingest, then the identical wall.
    let mut engine = fixture.engine();
    let snapshot = store
        .load::<PersistedFacts>(&engine.snapshot_header())
        .expect("the just-saved generation loads");
    let staged = engine.ingest_facts_snapshot(snapshot);
    let start = Instant::now();
    engine.discover_workspace().expect("discovery");
    engine
        .analyze(&[], &mut |_| {}, &cancel)
        .expect("warm analyze");
    let warm = start.elapsed();

    println!(
        "{{\"bench\":\"cold_full_analysis_warmstart\",\"files\":{files},\"saved\":{saved},\"staged\":{staged},\"cold_ms\":{:.1},\"warm_ms\":{:.1},\"speedup\":{:.2}}}",
        millis(cold),
        millis(warm),
        cold.as_secs_f64() / warm.as_secs_f64().max(1e-9),
    );
    println!(
        "// warm-start axis (DARTQ_BENCH_WARMSTART=1): the warm arm's first apply serves the batch from the staged generation instead of recomputing — the wall delta is the staging product; #1387 retention keeps unclaimed entries standing for later revert hits; numbers land at the serial run, never at bench time"
    );
}
