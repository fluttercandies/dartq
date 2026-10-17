//! Query-latency benchmark (execution deferred to the serial phase):
//! warm engine over the part-heavy library, M by-name `query_symbols`
//! calls; per-query walls → p50/p95 in microseconds. The answer assembles
//! from the maintained index state (per-file captures + per-library
//! reference surfaces rebuilt at the `LibraryAnalyzed` point) — before the
//! maintained index, every query recaptured the whole workspace (scan +
//! parse walk per member) and rebuilt the graph namespaces. Names rotate
//! across a declared top-level function, the defining file's function, a
//! part class, the hub (`hub_library`: a name declared in D files and
//! referenced from D libraries — its wall carries the ref join's D×L probe
//! term, so its p95/max deltas between two `DARTQ_BENCH_PARTS` scales read
//! the join's growth class), and a guaranteed miss — the miss bounds the
//! empty path.
//!
//! Cost axis: marginal per-query, warm engine — fixture build, discovery,
//! and the warm analyze sit outside the timed region. The five rotated
//! names pool four hit shapes (differing answer sizes) with one
//! empty-answer miss into a single distribution, so the reported p50/p95 is
//! a mixed-shape marginal query cost: queries are the denominator, answers
//! are not — this figure is not a per-answer cost and not an answer-size
//! curve; the hub's growth-across-scales reading is the one growth axis
//! here, not a per-answer curve either. Batch-query amortization is the
//! pairing-time ratio of this marginal figure against the
//! oracle's whole-run cold analyzer run per query, never a property of this
//! number alone.

mod common;

use std::time::{Duration, Instant};

use common::{Fixture, hub_library, part_library, percentile};

use dartq_engine::CancellationToken;

fn main() {
    let queries = std::env::var("DARTQ_BENCH_QUERIES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let parts = std::env::var("DARTQ_BENCH_PARTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);
    let libraries = part_library(parts);
    let hubs = hub_library(parts);
    let refs: Vec<(&str, &str)> = libraries
        .iter()
        .chain(hubs.iter())
        .map(|(path, body)| (path.as_str(), body.as_str()))
        .collect();
    let fixture = Fixture::new(&refs);

    let mut engine = fixture.engine();
    engine.discover_workspace().expect("discovery");
    engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("warm analyze");

    let names = ["g010", "top", "C040", "hub", "no_such_symbol"];
    let mut walls = Vec::with_capacity(queries);
    for i in 0..queries {
        let name = names[i % names.len()];
        let start = Instant::now();
        let answer = engine.query_symbols(name);
        walls.push(start.elapsed());
        std::hint::black_box(answer.decls.len() + answer.refs.len());
    }
    walls.sort();

    let micros = |d: Duration| d.as_secs_f64() * 1e6;
    println!(
        "{{\"bench\":\"query_symbols\",\"queries\":{queries},\"parts\":{parts},\"hubs\":{parts},\"p50_us\":{:.1},\"p95_us\":{:.1},\"max_us\":{:.1}}}",
        micros(percentile(&walls, 0.50)),
        micros(percentile(&walls, 0.95)),
        micros(percentile(&walls, 1.00)),
    );
    println!(
        "// acceptance axis: marginal per-query cost on a warm maintained index (cold build excluded; pooled across 4 hit shapes + 1 empty-answer miss; hub = D files declaring × D libraries referencing, its p95/max deltas across two DARTQ_BENCH_PARTS scales read the ref join's growth class) — answers are not the denominator; figures land at the serial validation run; batch-query amortization is the pairing-time ratio vs the oracle's whole-run cold analyzer run per query"
    );
}
