//! §19.3 incremental P95 benchmark skeleton (execution deferred): warm
//! engine, N single-file edits each followed by apply+analyze; per-step
//! walls → p50/p95. L1 posture note (incremental-l1-spec §7.3): a function-
//! body edit cascades all downstream libraries by definition — P95 at M1 is
//! the L1 baseline, not an L2 number.

mod common;

use std::time::Instant;

use common::{Fixture, LIB_A, LIB_A_EDITED, LIB_B, LIB_C, millis, percentile};

fn main() {
    let steps = std::env::var("DARTQ_BENCH_STEPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);
    let fixture = Fixture::new(&[
        ("lib/a.dart", LIB_A),
        ("lib/b.dart", LIB_B),
        ("lib/c.dart", LIB_C),
    ]);

    let mut engine = fixture.engine();
    engine.discover_workspace().expect("discovery");
    engine
        .analyze(&[], &mut |_| {}, &Default::default())
        .expect("warm analyze");

    let mut walls = Vec::with_capacity(steps);
    for step in 0..steps {
        let content = if step % 2 == 0 { LIB_A_EDITED } else { LIB_A };
        let start = Instant::now();
        let path = fixture.root.join("lib/a.dart");
        engine.set_content(&path, content.as_bytes());
        engine
            .analyze(&[], &mut |_| {}, &Default::default())
            .expect("incremental analyze");
        walls.push(start.elapsed());
    }
    walls.sort();

    println!(
        "{{\"bench\":\"incremental_p95\",\"steps\":{steps},\"p50_ms\":{:.1},\"p95_ms\":{:.1},\"max_ms\":{:.1}}}",
        millis(percentile(&walls, 0.50)),
        millis(percentile(&walls, 0.95)),
        millis(percentile(&walls, 1.00)),
    );
    println!(
        "// acceptance axis: PRD §19.3 incremental P95 (edit → diagnostics), 1000x+ target vs the oracle-per-edit baseline; L1 cascades bodies by definition"
    );
}
