//! §19.3 incremental P95 benchmark (execution deferred to the serial
//! phase): warm engine, N single-file edits each followed by apply+analyze;
//! per-step walls → p50/p95. Two shapes:
//!
//! * chain — the three-library import chain (a → b → c), comment churn on
//!   the head (the L1 baseline; L2 keeps the cascade signature-gated).
//! * parts — one defining file plus `DARTQ_BENCH_PARTS` cross-referencing
//!   parts (default 50), comment churn on the defining file, semdiag
//!   configured: the audit's Finding-3 shape, where the per-keystroke fold
//!   used to re-scan every member and now parses through the persisted
//!   scan faces. L1 posture note (incremental-l1-spec §7.3): a function-
//!   body edit cascades all downstream libraries by definition — P95 at M1
//!   is the L1 baseline, not an L2 number.

mod common;

use std::time::Instant;

use common::{Fixture, LIB_A, LIB_A_EDITED, LIB_B, LIB_C, millis, part_library, percentile};

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

    // Part-heavy shape: the audit's per-analyze re-scan multiplication
    // lives exactly here — every step recomputes the 51-member library and
    // the semdiag fold walks every member. Before the scanned-face cache
    // each member re-scanned per step; after, the fold parses through the
    // persisted token tables (the parse inside `diagnose` remains the
    // floor — dartq-semantic's seam).
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
    let edited = {
        let mut main = libraries[0].1.clone();
        main.insert_str(
            main.find("\nvoid top()").expect("body anchor"),
            "\n// edited",
        );
        main
    };

    let mut engine = fixture.diagnosing_engine();
    engine.discover_workspace().expect("discovery");
    engine
        .analyze(&[], &mut |_| {}, &Default::default())
        .expect("warm analyze");

    let mut walls = Vec::with_capacity(steps);
    let path = fixture.root.join("lib/main.dart");
    for step in 0..steps {
        let content = if step % 2 == 0 {
            edited.as_str()
        } else {
            libraries[0].1.as_str()
        };
        let start = Instant::now();
        engine.set_content(&path, content.as_bytes());
        engine
            .analyze(&[], &mut |_| {}, &Default::default())
            .expect("incremental analyze");
        walls.push(start.elapsed());
    }
    walls.sort();

    println!(
        "{{\"bench\":\"incremental_p95_parts\",\"parts\":{parts},\"steps\":{steps},\"p50_ms\":{:.1},\"p95_ms\":{:.1},\"max_ms\":{:.1}}}",
        millis(percentile(&walls, 0.50)),
        millis(percentile(&walls, 0.95)),
        millis(percentile(&walls, 1.00)),
    );
    println!(
        "// acceptance axis: PRD §19.3 incremental P95 (edit → diagnostics), 1000x+ target vs the oracle-per-edit baseline; L1 cascades bodies by definition"
    );
}
