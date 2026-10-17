//! Incremental P95 benchmark (execution deferred to the serial
//! phase): warm engine, N single-file edits each followed by apply+analyze;
//! per-step walls → p50/p95. Two shapes:
//!
//! * chain — the three-library import chain (a → b → c), comment churn on
//!   the head (the L1 baseline; L2 keeps the cascade signature-gated).
//! * parts — one defining file plus `DARTQ_BENCH_PARTS` cross-referencing
//!   parts (default 50), comment churn on the defining file, semdiag
//!   configured: the audit's Finding-3 shape, where the per-keystroke fold
//!   used to re-scan every member and now parses through the persisted
//!   scan faces. L1 posture note: a function-
//!   body edit cascades all downstream libraries by definition — P95 at M1
//!   is the L1 baseline, not an L2 number.
//!
//! Semdiag memo slice (`DARTQ_BENCH_SEMDIAG=1`, #1384): the parts shape
//! runs at 50 and 200 members, one JSON line each — after the member-level
//! fold memo a defining-file churn re-folds only the edited member while
//! the standing parts' folds are salvaged byte-identical, so the per-step
//! wall's fold term drops from O(members) folds to one. The two scales
//! read as the salvage curve; without the env the bench keeps its single
//! `DARTQ_BENCH_PARTS` line and measures nothing extra. Execution stays
//! deferred to the serial phase like every axis here.
//!
//! Merged-output share slice (`DARTQ_BENCH_SHARED=1`, #1387): a workspace
//! of independent one-row libraries (each file's `dart:math` combinator
//! miss reports one engine-native `undefined_shown_name`), warmed once,
//! then a zero-dirty analyze loop over the owned face and over the shared
//! face — two JSON lines. The owned wall carries a clone per standing row
//! per cycle; the shared wall is one refcount on the standing buffer, so
//! the row count is the scale term. `DARTQ_BENCH_SHARED_ROWS` scales it.
//!
//! Condensation dirty-front slice (`DARTQ_BENCH_CYCLE=1`, #1387): a
//! workspace of independent single-file libraries; each step appends a
//! fresh top-level declaration to one file, so every step moves the
//! structure face and rebuilds the condensation — wholesale that meant one
//! blake3 fold per library, salvaged it means one re-fold for the moved
//! cycle and a probe per standing library. One JSON line per scale
//! (500 and 2000 libraries); the slope against the scale is the salvage
//! curve's read. `DARTQ_BENCH_CYCLE_WHOLESALE=1` grows each scale into the
//! A/B pair: a second pass runs the same steps with the standing
//! condensation dropped per step ([`dartq_engine::Engine::clear_condensation`]),
//! so every cycle folds fresh — the pre-#1387 wholesale baseline — and the
//! per-scale salvaged-vs-wholesale delta is the salvage read.
//!
//! Cost axis: marginal. Each wall covers exactly one edit step
//! (`set_content` + analyze) on an already-warm engine — fixture build,
//! discovery, and the warm analyze sit outside the timed region, so no cold
//! cost leaks into these figures.

mod common;

use std::time::Instant;

use common::{Fixture, LIB_A, LIB_A_EDITED, LIB_B, LIB_C, millis, part_library, percentile};
use dartq_engine::CancellationToken;

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
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("warm analyze");

    let mut walls = Vec::with_capacity(steps);
    for step in 0..steps {
        let content = if step % 2 == 0 { LIB_A_EDITED } else { LIB_A };
        let start = Instant::now();
        let path = fixture.root.join("lib/a.dart");
        engine.set_content(&path, content.as_bytes());
        engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
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
    // lives exactly here — every step recomputes the member-heavy library
    // and the semdiag fold walks every member. Before the scanned-face
    // cache each member re-scanned per step; after, the fold parses through
    // the persisted token tables; after the #1384 member memo, standing
    // members' folds are salvaged outright. `DARTQ_BENCH_SEMDIAG=1` grows
    // the single default scale into the 50/200 salvage curve.
    let memo_probe = std::env::var("DARTQ_BENCH_SEMDIAG").is_ok();
    let parts: Vec<usize> = if memo_probe {
        vec![50, 200]
    } else {
        vec![
            std::env::var("DARTQ_BENCH_PARTS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(50),
        ]
    };
    for parts in parts {
        run_parts(parts, steps);
    }
    println!(
        "// acceptance axis: incremental P95 (edit → diagnostics), 1000x+ target vs the oracle-per-edit baseline; L1 cascades bodies by definition — these walls are marginal warm per-edit cost, while the oracle baseline re-runs the whole analysis cold per edit (daemon-vs-oracle posture, not marginal-vs-marginal)"
    );
    if memo_probe {
        println!(
            "// semdiag memo axis (DARTQ_BENCH_SEMDIAG=1): the parts walls above fold through the #1384 member memo — a defining-file churn re-folds one member and salvages the standing parts' folds byte-identical; the 50→200 delta is the salvage curve's scale term; numbers land at the serial run, never at bench time"
        );
    }
    if std::env::var("DARTQ_BENCH_SHARED").is_ok() {
        let rows = std::env::var("DARTQ_BENCH_SHARED_ROWS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(500);
        run_shared(rows, steps);
    }
    if std::env::var("DARTQ_BENCH_CYCLE").is_ok() {
        for libs in [500, 2000] {
            run_condensation(libs, steps);
        }
    }
}

/// Builds, warms, and measures the `parts`-member library shape: comment
/// churn on the defining file, one set_content+analyze wall per step.
fn run_parts(parts: usize, steps: usize) {
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
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
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
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
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
}

/// Builds a fixture of `count` independent single-file libraries, each
/// carrying exactly one engine-native diagnostic row (the `dart:math`
/// combinator miss reports `undefined_shown_name` — no diagnoser needed).
fn one_row_fixture(count: usize) -> Fixture {
    let libraries: Vec<(String, String)> = (0..count)
        .map(|i| {
            (
                format!("lib/r{i}.dart"),
                format!("import 'dart:math' show zork{i};\nint f{i}() => {i};\n"),
            )
        })
        .collect();
    let refs: Vec<(&str, &str)> = libraries
        .iter()
        .map(|(path, body)| (path.as_str(), body.as_str()))
        .collect();
    Fixture::new(&refs)
}

/// The merged-output share axis (DARTQ_BENCH_SHARED=1, #1387): a warmed
/// workspace, then a zero-dirty analyze loop over the owned face (a clone
/// per standing row per cycle) and over the shared face (one refcount on
/// the standing buffer). Two JSON lines; the delta at a given row count is
/// the per-cycle clone term the share removes.
fn run_shared(rows: usize, cycles: usize) {
    let fixture = one_row_fixture(rows);
    let mut engine = fixture.engine();
    engine.discover_workspace().expect("discovery");
    let warm = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("warm analyze");
    assert_eq!(
        warm.len(),
        rows,
        "the shape carries exactly one row per library: {rows}"
    );

    let mut owned = Vec::with_capacity(cycles);
    for _ in 0..cycles {
        let start = Instant::now();
        let diagnostics = engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("owned analyze");
        owned.push(start.elapsed());
        drop(diagnostics);
    }
    owned.sort();
    println!(
        "{{\"bench\":\"incremental_p95_zerodirty_owned\",\"rows\":{rows},\"cycles\":{cycles},\"p50_us\":{:.1},\"p95_us\":{:.1}}}",
        common::micros(percentile(&owned, 0.50)),
        common::micros(percentile(&owned, 0.95)),
    );

    let mut shared = Vec::with_capacity(cycles);
    for _ in 0..cycles {
        let start = Instant::now();
        let diagnostics = engine
            .analyze_shared(&[], &mut |_| {}, &CancellationToken::default())
            .expect("shared analyze");
        shared.push(start.elapsed());
        drop(diagnostics);
    }
    shared.sort();
    println!(
        "{{\"bench\":\"incremental_p95_zerodirty_shared\",\"rows\":{rows},\"cycles\":{cycles},\"p50_us\":{:.1},\"p95_us\":{:.1}}}",
        common::micros(percentile(&shared, 0.50)),
        common::micros(percentile(&shared, 0.95)),
    );
    println!(
        "// merged-output share axis (DARTQ_BENCH_SHARED=1): owned = a clone per standing row per zero-dirty cycle; shared = one refcount on the standing buffer — the delta is the clone term #1387 removes; numbers land at the serial run, never at bench time"
    );
}

/// The condensation dirty-front axis (DARTQ_BENCH_CYCLE=1, #1387):
/// independent single-file libraries, one fresh top-level declaration
/// appended to the first file per step — every step moves the structure
/// face and rebuilds the condensation. Wholesale that folded one blake3
/// digest per library per step; salvaged it re-folds the moved cycle and
/// probes the standing front. The slope of the wall against the library
/// count (500 → 2000) is the salvage curve's read. With
/// `DARTQ_BENCH_CYCLE_WHOLESALE=1` a second pass drops the standing
/// condensation per step (`Engine::clear_condensation`), so every cycle
/// folds fresh — the wholesale baseline the salvaged walls A/B against.
fn run_condensation(libs: usize, steps: usize) {
    let libraries: Vec<(String, String)> = (0..libs)
        .map(|i| (format!("lib/c{i}.dart"), format!("int f{i}() => {i};\n")))
        .collect();
    let refs: Vec<(&str, &str)> = libraries
        .iter()
        .map(|(path, body)| (path.as_str(), body.as_str()))
        .collect();
    let fixture = Fixture::new(&refs);
    let mut engine = fixture.engine();
    engine.discover_workspace().expect("discovery");
    engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("warm analyze");

    let base = libraries[0].1.clone();
    let path = fixture.root.join("lib/c0.dart");
    let mut run_pass = |label: &str, wholesale: bool| {
        let mut walls = Vec::with_capacity(steps);
        for step in 0..steps {
            let mut edited = base.clone();
            edited.push_str(&format!("void extra{step}() {{}}\n"));
            if wholesale {
                // The standing front cannot be salvaged: the rebuild folds
                // every cycle — the pre-#1387 wholesale shape.
                engine.clear_condensation();
            }
            let start = Instant::now();
            engine.set_content(&path, edited.as_bytes());
            engine
                .analyze(&[], &mut |_| {}, &CancellationToken::default())
                .expect("incremental analyze");
            walls.push(start.elapsed());
        }
        walls.sort();
        println!(
            "{{\"bench\":\"incremental_p95_condensation{label}\",\"libs\":{libs},\"steps\":{steps},\"p50_ms\":{:.1},\"p95_ms\":{:.1},\"max_ms\":{:.1}}}",
            millis(percentile(&walls, 0.50)),
            millis(percentile(&walls, 0.95)),
            millis(percentile(&walls, 1.00)),
        );
    };
    run_pass("", false);
    if std::env::var("DARTQ_BENCH_CYCLE_WHOLESALE").is_ok() {
        run_pass("_wholesale", true);
    }
    println!(
        "// condensation dirty-front axis (DARTQ_BENCH_CYCLE=1): one structure keystroke rebuilds the condensation over `libs` standing cycles — salvaged, one cycle re-folds and the rest probe; with DARTQ_BENCH_CYCLE_WHOLESALE=1 the _wholesale line runs the same steps with the standing condensation dropped per step, every cycle folding fresh (the pre-#1387 shape) — the per-scale delta is the salvage read; numbers land at the serial run, never at bench time"
    );
}
