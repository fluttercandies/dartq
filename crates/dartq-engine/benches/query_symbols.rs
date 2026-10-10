//! §19.3 query-latency benchmark (execution deferred to the serial phase):
//! warm engine over the part-heavy library, M by-name `query_symbols`
//! calls; per-query walls → p50/p95 in microseconds. The answer assembles
//! from the maintained index state (per-file captures + per-library
//! reference surfaces rebuilt at the `LibraryAnalyzed` point) — before the
//! maintained index, every query recaptured the whole workspace (scan +
//! parse walk per member) and rebuilt the graph namespaces. Names rotate
//! across a declared top-level function, the defining file's function, a
//! part class, and a guaranteed miss — the miss bounds the empty path.

mod common;

use std::time::{Duration, Instant};

use common::{Fixture, part_library, percentile};

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
    let refs: Vec<(&str, &str)> = libraries
        .iter()
        .map(|(path, body)| (path.as_str(), body.as_str()))
        .collect();
    let fixture = Fixture::new(&refs);

    let mut engine = fixture.engine();
    engine.discover_workspace().expect("discovery");
    engine
        .analyze(&[], &mut |_| {}, &Default::default())
        .expect("warm analyze");

    let names = ["g010", "top", "C040", "no_such_symbol"];
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
        "{{\"bench\":\"query_symbols\",\"queries\":{queries},\"parts\":{parts},\"p50_us\":{:.1},\"p95_us\":{:.1},\"max_us\":{:.1}}}",
        micros(percentile(&walls, 0.50)),
        micros(percentile(&walls, 0.95)),
        micros(percentile(&walls, 1.00)),
    );
    println!(
        "// acceptance axis: PRD §19.3 batch-query amortization (maintained index) — per-query cost is answer-sized, not workspace-sized"
    );
}
