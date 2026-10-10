//! In-crate tests: scale math, sorted streaming order, determinism, and the
//! feature-matrix coverage gate. Executed in the serial verification phase
//! (`cargo test -p corpus-gen`), never during lane development.

use std::fs;
use std::path::PathBuf;

use crate::coverage::{CELL_COUNT, CELL_CORE_VALUE_USE, CELL_LIST, CELL_NAMES};
use crate::generator::{generate, Config};
use crate::plan::{Plan, Scale, CORE_FIXED};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("corpus-gen-{tag}-{}", std::process::id()));
    // Clear stale output from a previous crashed run of the same test binary.
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn config(dir: PathBuf, scale: Scale, seed: u64) -> Config {
    Config {
        out: dir,
        scale,
        seed,
    }
}

fn ratio(value: usize, total: usize) -> f64 {
    value as f64 / total as f64
}

#[test]
fn cell_tables_stay_in_sync() {
    assert_eq!(CELL_LIST.len(), CELL_NAMES.len());
    assert_eq!(CELL_LIST.len(), CELL_COUNT);
    assert_eq!(CELL_NAMES[CELL_CORE_VALUE_USE], "core_value_use");
}

#[test]
fn plan_hits_scale_targets_and_distribution() {
    for (scale, target) in [(Scale::K1, 1_000), (Scale::K10, 10_000), (Scale::K100, 100_000)] {
        let plan = Plan::build(scale);
        assert_eq!(plan.total_files, target, "file target for {}", scale.label());

        let features = plan.packages as usize * plan.per_package + plan.extra_last_package;
        let core = CORE_FIXED + 1 + plan.core_dir_counts.iter().sum::<usize>(); // +1 core pubspec
        let app = plan.controllers + plan.screens + 2; // pubspec + main + wiring
        assert!(
            (ratio(features, target) - 0.60).abs() <= 0.02,
            "feature ratio {}",
            ratio(features, target)
        );
        assert!((ratio(core, target) - 0.15).abs() <= 0.02);
        assert!((ratio(app, target) - 0.10).abs() <= 0.02);
        assert!((ratio(plan.tests, target) - 0.15).abs() <= 0.02);

        // The streaming iterator must yield exactly the planned files.
        assert_eq!(plan.iter().count(), plan.total_files);
    }
}

#[test]
fn plan_yields_sorted_unique_paths() {
    // The fingerprint contract depends on generation order == lexicographic
    // path order; verify it directly on the full 1k plan.
    let plan = Plan::build(Scale::K1);
    let mut previous: Option<String> = None;
    let mut seen = 0;
    for item in plan.iter() {
        if let Some(previous) = &previous {
            assert!(
                previous.as_str() < item.path.as_str(),
                "order violation: {previous} !< {}",
                item.path
            );
        }
        previous = Some(item.path);
        seen += 1;
    }
    assert_eq!(seen, plan.total_files);
}

#[test]
fn same_seed_is_byte_identical() {
    let dir_a = temp_dir("same-a");
    let dir_b = temp_dir("same-b");
    let stats_a = generate(&config(dir_a.clone(), Scale::K1, 42)).expect("generation A");
    let stats_b = generate(&config(dir_b.clone(), Scale::K1, 42)).expect("generation B");
    assert_eq!(stats_a.manifest.fingerprint, stats_b.manifest.fingerprint);
    assert_eq!(stats_a.manifest.files, stats_b.manifest.files);
    assert_eq!(stats_a.manifest.lines, stats_b.manifest.lines);

    let manifest_a = fs::read_to_string(dir_a.join("manifest.json")).expect("manifest A");
    let manifest_b = fs::read_to_string(dir_b.join("manifest.json")).expect("manifest B");
    assert_eq!(manifest_a, manifest_b);

    let main_a = fs::read_to_string(dir_a.join("lib/main.dart")).expect("main A");
    let main_b = fs::read_to_string(dir_b.join("lib/main.dart")).expect("main B");
    assert_eq!(main_a, main_b);

    let _ = fs::remove_dir_all(&dir_a);
    let _ = fs::remove_dir_all(&dir_b);
}

#[test]
fn seed_changes_the_fingerprint() {
    let dir_a = temp_dir("seed-a");
    let dir_b = temp_dir("seed-b");
    let stats_a = generate(&config(dir_a.clone(), Scale::K1, 1)).expect("generation A");
    let stats_b = generate(&config(dir_b, Scale::K1, 2)).expect("generation B");
    assert_ne!(stats_a.manifest.fingerprint, stats_b.manifest.fingerprint);
    let _ = fs::remove_dir_all(&dir_a);
}

#[test]
fn feature_matrix_is_covered_at_1k() {
    let dir = temp_dir("coverage");
    let stats = generate(&config(dir.clone(), Scale::K1, 7)).expect("generation");
    for cell in CELL_LIST {
        let count = stats.coverage.count(cell);
        assert!(
            count >= 3,
            "cell {} under-covered: {count}",
            CELL_NAMES[cell as usize]
        );
    }
    let _ = fs::remove_dir_all(&dir);
}
