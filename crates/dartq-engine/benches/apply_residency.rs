//! Apply-at-residency benchmark (execution deferred to the serial
//! phase): per-`apply_pending` fixed cost at residency R. The #421 audit
//! (P1) found every apply — every keystroke batch — rebuilds the full
//! library-graph state from the whole resident set (`l1.rs`
//! `reform_libraries`: canonical-URI fill, defining+sort, `library_of_file`,
//! `importers`, all `LibraryInfo`s — several O(R)/O(R log R) passes per
//! apply). Three synthetic workspaces at R ∈ {1k, 10k, 100k} files; the
//! growth curve across the distinct bench ids is the evidence for whether
//! incremental library-graph maintenance (#431) is scheduled.
//!
//! Per scale, setup (fixture write + discovery + warm analyze — the cold
//! build to full residency) is unmeasured. Each measured step is one
//! signature-neutral edit (`set_content` comment churn, unmeasured) whose
//! walls split the two phases the daemon runs per keystroke: `apply_pending`
//! alone (the audited full-residency rebuild) and one follow-up `analyze`
//! of the edited file (the L2-gated floor). One defining library per file,
//! no parts — part fanout is `incremental_p95`'s axis; here residency scale
//! is the only variable.
//!
//! Reachability slice (`DARTQ_BENCH_REACHABILITY=1`, #1383): since the pass
//! turned lazy, the analyze wall above no longer contains the
//! declaration-reachability build, so the bench grew an env-gated probe
//! that times one extra step per iteration — the first
//! `engine.reachability` read after the incremental analyze. That is the
//! pass's marginal per-keystroke cost (the per-unit salvage: key walk plus
//! the moved unit's re-derivation plus the propagation over the standing
//! edge set); step 0 has no standing product and pays the wholesale cold
//! build, so it lands in the max column, not the percentiles. Execution
//! stays deferred to the serial phase like every axis here.
//!
//! L3 arming axis (`DARTQ_BENCH_L3=1`, #1385): the apply wall re-run at
//! three content-store postures — disarmed (no store; the `Option`-test
//! floor), armed-journal (the #1385 delta journal: the flush appends
//! O(changed) rows), armed-full-rewrite (compaction cadence 1, the
//! pre-journal wholesale manifest rewrite per flush). The store tail
//! inside `apply_pending` (changed-content puts, then the flush) is the
//! only thing that differs between the postures: the journal's win reads
//! as armed-journal sitting at the disarmed floor while
//! armed-full-rewrite carries the O(residency) manifest term. These
//! variant walls time `apply_pending` alone (no per-step analyze) to keep
//! the tail unmasked.
//!
//! Memory posture: file bodies are ~10 deterministic lines, and the scales
//! run strictly sequentially — each fixture + engine dropped before the
//! next builds — so peak RSS is one residency's worth, expected in the few
//! hundred MB at the 100k scale.

mod common;

use std::fmt::Write as _;
use std::time::Instant;

use common::{Fixture, millis, percentile};
use dartq_engine::CancellationToken;
use dartq_engine::store::StoreConfig;

/// Residency scales, each its own bench id so `cargo bench` output reads as
/// a growth curve. Steps default to 20 (not the incremental bench's 50):
/// each step at the 100k scale pays an O(R) rebuild, and 20 samples already
/// bound the p50/p95 estimate.
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
    let l3_probe = std::env::var("DARTQ_BENCH_L3").is_ok();
    for (files, tag) in SCALES {
        run_scale(files, tag, steps);
        if l3_probe {
            run_l3_axis(files, tag, steps);
        }
    }
    println!(
        "// acceptance axis: marginal per-apply fixed cost at residency R (warm engine; the cold residency build is unmeasured) — the #431 (incremental library-graph maintenance) go/no-go evidence"
    );
    println!(
        "// reachability axis (DARTQ_BENCH_REACHABILITY=1): the marginal per-keystroke salvage slice — the first engine.reachability read after each incremental analyze; step 0 is the wholesale cold build and lands in the max column, not the percentiles; numbers land at the serial run, never at bench time"
    );
    if l3_probe {
        println!(
            "// L3 arming axis (DARTQ_BENCH_L3=1, #1385): the apply wall at three content-store postures — l3_disarmed (no store, the floor), l3_armed_journal (the delta journal), l3_armed_full_rewrite (cadence 1, the pre-journal wholesale manifest rewrite); the journal's win is armed-journal at the disarmed floor while armed-full-rewrite carries the O(residency) manifest term; numbers land at the serial run, never at bench time"
        );
    }
}

/// The L3 arming axis: one engine per posture over the same residency
/// workspace and churn cadence as `run_scale`, each printing its own
/// apply-walls line.
fn run_l3_axis(files: usize, tag: &str, steps: usize) {
    run_l3_variant(files, tag, "l3_disarmed", steps, false, None);
    run_l3_variant(files, tag, "l3_armed_journal", steps, true, None);
    run_l3_variant(files, tag, "l3_armed_full_rewrite", steps, true, Some(1));
}

/// Builds, warms, and measures one arming posture: `apply_pending` walls
/// only — the store tail (changed-content puts, then the flush) rides
/// inside them, with no per-step analyze masking it.
fn run_l3_variant(
    files: usize,
    tag: &str,
    posture: &str,
    steps: usize,
    arm: bool,
    cadence: Option<u32>,
) {
    let store_root = std::env::temp_dir().join(format!(
        "dartq-bench-l3-{tag}-{posture}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&store_root);
    let libraries = residency_workspace(files);
    let refs: Vec<(&str, &str)> = libraries
        .iter()
        .map(|(path, body)| (path.as_str(), body.as_str()))
        .collect();
    let fixture = Fixture::new(&refs);

    let mut engine = fixture.engine();
    if arm {
        engine
            .open_content_store(&StoreConfig {
                root: store_root.clone(),
                size_limit: u64::MAX,
            })
            .expect("arm content store");
        if let (Some(every), Some(store)) = (cadence, engine.content_store_mut()) {
            store.set_flushes_per_compaction(every);
        }
    }
    engine.discover_workspace().expect("discovery");
    engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("warm analyze");

    // Comment churn on f00000 — the same shape, different bytes cadence
    // `run_scale` runs.
    let edited = {
        let mut head = libraries[0].1.clone();
        head.insert_str(
            head.find("\nint g00000").expect("body anchor"),
            "\n// edited",
        );
        head
    };
    let target = fixture.root.join("lib/f00000.dart");

    let mut apply_walls = Vec::with_capacity(steps);
    for step in 0..steps {
        let content = if step % 2 == 0 {
            edited.as_str()
        } else {
            libraries[0].1.as_str()
        };
        let _ = engine.set_content(&target, content.as_bytes());
        let start = Instant::now();
        engine.apply_pending(&mut |_| {});
        apply_walls.push(start.elapsed());
    }
    apply_walls.sort();

    println!(
        "{{\"bench\":\"apply/{tag}/{posture}\",\"files\":{files},\"steps\":{steps},\"apply_p50_ms\":{:.1},\"apply_p95_ms\":{:.1},\"apply_max_ms\":{:.1}}}",
        millis(percentile(&apply_walls, 0.50)),
        millis(percentile(&apply_walls, 0.95)),
        millis(percentile(&apply_walls, 1.00)),
    );
    drop(engine);
    let _ = std::fs::remove_dir_all(&store_root);
}

/// Builds, warms, and measures one residency scale.
fn run_scale(files: usize, tag: &str, steps: usize) {
    let reach_probe = std::env::var("DARTQ_BENCH_REACHABILITY").is_ok();
    let libraries = residency_workspace(files);
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

    // Comment churn on f00000 — same shape, different bytes: the ContentId
    // gate passes and reform_libraries runs in full, while the signature
    // gate keeps the importer cascade off (f00000 is imported by nobody by
    // construction, so one library invalidates per step).
    let edited = {
        let mut head = libraries[0].1.clone();
        head.insert_str(
            head.find("\nint g00000").expect("body anchor"),
            "\n// edited",
        );
        head
    };
    let target = fixture.root.join("lib/f00000.dart");

    let mut apply_walls = Vec::with_capacity(steps);
    let mut analyze_walls = Vec::with_capacity(steps);
    let mut reach_walls = Vec::with_capacity(steps);
    let mut affected = None;
    for step in 0..steps {
        let content = if step % 2 == 0 {
            edited.as_str()
        } else {
            libraries[0].1.as_str()
        };
        let file = engine.set_content(&target, content.as_bytes());
        let start = Instant::now();
        let report = engine.apply_pending(&mut |_| {});
        apply_walls.push(start.elapsed());
        let start = Instant::now();
        engine
            .analyze(&[file], &mut |_| {}, &CancellationToken::default())
            .expect("incremental analyze");
        analyze_walls.push(start.elapsed());
        if reach_probe {
            // The reachability slice: the first read after the analyze pays
            // the (salvaged) rebuild; every later read in the loop is a
            // standing-map clone and measures nothing — one probe per step.
            let start = Instant::now();
            let _map = engine.reachability();
            reach_walls.push(start.elapsed());
        }
        affected = Some(report);
    }
    apply_walls.sort();
    analyze_walls.sort();
    let report = affected.expect("ran");

    println!(
        "{{\"bench\":\"apply/{tag}\",\"files\":{files},\"steps\":{steps},\"apply_p50_ms\":{:.1},\"apply_p95_ms\":{:.1},\"apply_max_ms\":{:.1},\"analyze_p50_ms\":{:.1},\"analyze_p95_ms\":{:.1},\"analyze_max_ms\":{:.1},\"changed\":{},\"invalidated\":{}}}",
        millis(percentile(&apply_walls, 0.50)),
        millis(percentile(&apply_walls, 0.95)),
        millis(percentile(&apply_walls, 1.00)),
        millis(percentile(&analyze_walls, 0.50)),
        millis(percentile(&analyze_walls, 0.95)),
        millis(percentile(&analyze_walls, 1.00)),
        report.changed.len(),
        report.invalidated.len(),
    );
    if reach_probe {
        reach_walls.sort();
        println!(
            "{{\"bench\":\"apply/{tag}/reachability_slice\",\"files\":{files},\"steps\":{steps},\"reach_p50_ms\":{:.1},\"reach_p95_ms\":{:.1},\"reach_max_ms\":{:.1}}}",
            millis(percentile(&reach_walls, 0.50)),
            millis(percentile(&reach_walls, 0.95)),
            millis(percentile(&reach_walls, 1.00)),
        );
    }
}

/// One defining library per file (no parts): file `i` imports `2 + i % 4`
/// (2-5) following siblings deterministically; trailing files import fewer
/// (nothing wraps past the end) and no file imports `f00000`, so the edit
/// target's importer set is empty by construction. Bodies are self-contained
/// (~10 lines) — no cross-file semantic dependencies, keeping the measured
/// single-file analyze floor flat across scales.
fn residency_workspace(files: usize) -> Vec<(String, String)> {
    let mut libraries = Vec::with_capacity(files);
    for i in 0..files {
        let mut body = String::new();
        let deps = 2 + i % 4;
        for t in i + 1..=((i + deps).min(files - 1)) {
            let _ = writeln!(body, "import 'f{t:05}.dart';");
        }
        let _ = write!(
            body,
            "\nclass C{i:05} {{\n  final int field;\n  C{i:05}(this.field);\n  int get doubled => field * 2;\n}}\n\nint g{i:05}(int x) => x + {i};\n"
        );
        libraries.push((format!("lib/f{i:05}.dart"), body));
    }
    libraries
}
