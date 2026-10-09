//! §19.3 cold full-analysis benchmark skeleton (execution deferred to the
//! serial phase): fresh engine → discover → analyze, wall clocks per
//! iteration plus the discovery counters the metrics hook pairs with
//! RSS/100k LOC (the RSS axis itself is the dartiq-cli `ru_maxrss` hook,
//! engine-api-spec §3 — the engine contributes only numbers via
//! `DiscoveryStats` and events).

mod common;

use std::time::Instant;

use common::{Fixture, LIB_A, LIB_B, LIB_C, millis, percentile};

fn main() {
    let iterations = std::env::var("DARTIQ_BENCH_ITERS")
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
            .analyze(&[], &mut |_| events += 1, &Default::default())
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
        "// acceptance axis: PRD §19.3 cold full-analysis vs `dart analyze` (oracle run deferred; pair with dartiq-cli ru_maxrss for RSS/100k LOC)"
    );
}
