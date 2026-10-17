//! In-crate tests: scale math, sorted streaming order, determinism, and the
//! feature-matrix coverage gate. Executed in the serial verification phase
//! (`cargo test -p corpus-gen`), never during lane development.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::coverage::{CELL_CORE_VALUE_USE, CELL_COUNT, CELL_LIST, CELL_NAMES, Coverage};
use crate::emit::{RenderCtx, flutter::SURFACE_MEMBERS, flutter::SURFACE_TYPES, render};
use crate::generator::{Config, generate};
use crate::plan::{
    CORE_FIXED, FLUTTER_PARTS, FLUTTER_STUB_DIR, FLUTTER_STUB_FILES, Layout, Options, Plan, Scale,
};
use crate::rng::SplitMix64;

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
        options: Options::default(),
        emit_replay_seq: None,
    }
}

// Test ratio helper; f64 rounding at corpus sizes is irrelevant.
#[expect(clippy::cast_precision_loss, reason = "see above")]
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
    for (scale, target) in [
        (Scale::Slice, 160),
        (Scale::K1, 1_000),
        (Scale::K10, 10_000),
        (Scale::K100, 100_000),
    ] {
        let plan = Plan::build(scale);
        // The adversarial overlay is additive: the base corpus still budgets
        // exactly `target` files, and the overlay appends on top.
        let overlay = plan.patho
            + plan.unicode
            + plan.errstorm_packages as usize * crate::plan::ERRSTORM_PACKAGE_FILES;
        assert_eq!(
            plan.total_files,
            target + overlay,
            "file target for {}",
            scale.label()
        );

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
    for item in &plan {
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
    // Face-gated cells (1d/1e) stay dark in a default run — the gate only
    // spans the unconditional range.
    for cell in &CELL_LIST[..crate::coverage::UNCONDITIONAL_CELL_COUNT] {
        let count = stats.coverage.count(*cell);
        assert!(
            count >= 3,
            "cell {} under-covered: {count}",
            CELL_NAMES[*cell as usize]
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

// --- #1810 adversarial axes ---------------------------------------------------

/// Plan-level pins for the overlay budgets: the >=30 hits at 100k gate is a
/// plan-count fact, asserted without running a 100k generation.
#[test]
fn adversarial_budgets_scale_with_the_corpus() {
    let k1 = Plan::build(Scale::K1);
    assert!(k1.patho >= 3, "patho floor for the 1k gate");
    assert!(k1.unicode >= 3);
    assert!(k1.errstorm_packages >= 1);

    let k100 = Plan::build(Scale::K100);
    assert!(k100.patho >= 30, "patho >= 30 hits at 100k");
    assert!(k100.unicode >= 30);
    let storm_files = k100.errstorm_packages as usize * crate::plan::ERRSTORM_LIB_FILES;
    assert!(storm_files >= 30, "storm files >= 30 at 100k");
}

/// The overlay emits inside `zadv/` after `tests/`; the plan's lexicographic
/// invariant still covers the tail.
#[test]
fn adversarial_files_emit_after_tests_in_path_order() {
    let plan = Plan::build(Scale::K1);
    let mut saw_tests = false;
    let mut first_adv: Option<String> = None;
    let mut adv_count = 0usize;
    for item in &plan {
        if item.path.starts_with("zadv/") {
            adv_count += 1;
            first_adv.get_or_insert_with(|| item.path.clone());
        } else {
            saw_tests = true;
        }
    }
    assert!(saw_tests, "base corpus present");
    assert!(adv_count > 0, "overlay present");
    assert_eq!(
        first_adv.expect("overlay").as_str(),
        "zadv/errstorm_0000/lib/storm_00000.dart",
        "first overlay path in lexicographic order"
    );
}

/// Every storm file carries >= 5 stacked damage recipes, deterministically.
#[test]
fn errstorm_files_stack_five_or_more_damages() {
    let dir = temp_dir("errstorm");
    generate(&config(dir.clone(), Scale::K1, 3)).expect("generation");
    let storm = dir.join("zadv/errstorm_0000/lib/storm_00000.dart");
    let content = fs::read_to_string(&storm).expect("storm file");
    // Recipes are uniquified per instantiation with `{package}_{file}_{u}`;
    // the file must carry at least the 0..=4 instances (recipe count >= 5).
    for u in 0..5 {
        assert!(
            content.contains(&format!("0_0_{u}")),
            "storm file missing recipe index {u}: {content}"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

/// Patho singles render their deterministic budgets: the nested-collection
/// shape reaches its 1000-deep cap and stays balanced.
#[test]
fn patho_files_hit_their_depth_budgets() {
    let dir = temp_dir("patho");
    generate(&config(dir.clone(), Scale::K1, 5)).expect("generation");
    // index % 6 == 0 -> nested collection of PATHO_NEST_DEPTH=1000 brackets.
    let nested = dir.join("zadv/patho/patho_00000.dart");
    let content = fs::read_to_string(&nested).expect("patho file");
    let opens = content.matches('[').count();
    let closes = content.matches(']').count();
    assert_eq!(opens, 1000, "nesting depth");
    assert_eq!(opens, closes, "balanced brackets");
    let _ = fs::remove_dir_all(&dir);
}

/// Unicode family round-trips through the emit buffer byte-exact: a BOM
/// variant (index % 8 == 3) starts with U+FEFF and every family file is
/// present under `zadv/unicode/`.
#[test]
fn unicode_files_emit_byte_exact_edge_content() {
    let dir = temp_dir("unicode");
    generate(&config(dir.clone(), Scale::K1, 9)).expect("generation");
    // 1k plans 10 unicode files; index 3 carries the leading BOM.
    let bom = dir.join("zadv/unicode/uni_00003.dart");
    let bytes = fs::read(&bom).expect("unicode file");
    assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF], "UTF-8 BOM at file origin");
    // A RTL literal file (index % 8 == 0) must carry the Arabic text.
    let rtl = dir.join("zadv/unicode/uni_00000.dart");
    let text = fs::read_to_string(&rtl).expect("rtl file");
    assert!(text.contains("مرحبا"), "RTL literal present");
    let _ = fs::remove_dir_all(&dir);
}

// --- #1811 golden slice -------------------------------------------------------

/// Repo-root-relative path of the tracked golden slice. The slice is the
/// byte-stability pin that survives fresh clones: this test regenerates the
/// slice in-process and asserts byte-equality against the checked-in tree.
const SLICE_DIR: &str = "tests/corpus/mega-x/slice";
/// Replay-sequence length baked into the slice (also the X4 leg's edits).
const SLICE_REPLAY_STEPS: u64 = 30;

fn slice_config(dir: PathBuf) -> Config {
    Config {
        out: dir,
        scale: Scale::Slice,
        seed: 0,
        options: Options::default(),
        emit_replay_seq: Some(SLICE_REPLAY_STEPS),
    }
}

/// Collects every file under `root` as `<relative path> -> bytes`.
fn collect_tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).expect("read dir") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            let rel = if prefix.is_empty() {
                entry.file_name().to_string_lossy().into_owned()
            } else {
                format!("{prefix}/{}", entry.file_name().to_string_lossy())
            };
            if path.is_dir() {
                walk(&path, &rel, out);
            } else {
                out.insert(rel, fs::read(&path).expect("read file"));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, "", &mut out);
    out
}

/// Regenerating the slice at seed 0 must reproduce the checked-in tree byte
/// for byte — corpus files, manifest.json and edits.jsonl alike. This is the
/// determinism contract pinned where a fresh clone can check it.
#[test]
fn golden_slice_is_byte_stable() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("corpus-gen lives at <repo>/tools/corpus-gen")
        .to_path_buf();
    let golden = root.join(SLICE_DIR);
    assert!(
        golden.join("manifest.json").is_file(),
        "{SLICE_DIR} is missing — regenerate it: cargo run -p corpus-gen -- \
         --out {SLICE_DIR} --scale slice --seed 0 --emit-replay-seq {SLICE_REPLAY_STEPS}"
    );

    let dir = temp_dir("golden-slice");
    generate(&slice_config(dir.clone())).expect("slice generation");
    let produced = collect_tree(&dir);
    let checked_in = collect_tree(&golden);
    assert_eq!(
        produced.keys().collect::<Vec<_>>(),
        checked_in.keys().collect::<Vec<_>>(),
        "file set drifted from the golden slice"
    );
    for (path, bytes) in &produced {
        assert_eq!(
            bytes, &checked_in[path],
            "byte drift in slice file {path} — regenerate the golden only \
             alongside a deliberate generator change"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

/// The slice plan covers every adversarial shape and every replay capture
/// path — the properties the X2–X4 serial legs rely on.
#[test]
fn slice_covers_every_adversarial_shape() {
    let plan = Plan::build(Scale::Slice);
    assert_eq!(plan.patho, 6, "all six patho shapes");
    assert_eq!(plan.unicode, 8, "all eight unicode variants");
    assert_eq!(plan.errstorm_packages, 1);
    for path in crate::replay::CAPTURE_PATHS {
        assert!(
            plan.iter().any(|item| item.path == *path),
            "slice must emit replay capture path {path}"
        );
    }
}

/// The replay sequence is valid JSONL, lands only complete units, and the
/// unit shapes match the harness `replay_edit` schema.
#[test]
fn replay_seq_is_schema_valid_and_deterministic() {
    let dir_a = temp_dir("replay-a");
    let dir_b = temp_dir("replay-b");
    let mut cfg_a = config(dir_a.clone(), Scale::K1, 11);
    cfg_a.emit_replay_seq = Some(60);
    let mut cfg_b = config(dir_b.clone(), Scale::K1, 11);
    cfg_b.emit_replay_seq = Some(60);
    generate(&cfg_a).expect("generation A");
    generate(&cfg_b).expect("generation B");
    let edits_a = fs::read_to_string(dir_a.join("edits.jsonl")).expect("edits A");
    let edits_b = fs::read_to_string(dir_b.join("edits.jsonl")).expect("edits B");
    assert_eq!(edits_a, edits_b, "same seed -> identical replay sequence");

    let lines: Vec<&str> = edits_a.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 60, "exactly n steps");
    for line in &lines {
        // Minimal schema check without pulling serde: every line is a JSON
        // object carrying exactly one of the three step shapes.
        let has_splice = line.contains("\"file\":") && line.contains("\"new_text\":");
        let has_remove = line.contains("\"remove\":");
        let has_create = line.contains("\"create\":") && line.contains("\"text\":");
        assert!(
            has_splice ^ has_remove ^ has_create,
            "one step shape per line: {line}"
        );
    }
    let _ = fs::remove_dir_all(&dir_a);
    let _ = fs::remove_dir_all(&dir_b);
}

// --- #1812 version-ladder + workspace axes -------------------------------------

/// Options-aware config for the 1d/1e tests.
fn options_config(dir: PathBuf, scale: Scale, seed: u64, options: Options) -> Config {
    Config {
        out: dir,
        scale,
        seed,
        options,
        emit_replay_seq: None,
    }
}

/// The sorted-path invariant must hold under every option combination —
/// workspace extras (`.dart_tool/**`, `analysis_options.yaml`,
/// `third_party/**`) interleave at their true lexicographic positions.
#[test]
fn plan_stays_sorted_under_workspace_and_ladder() {
    for options in [
        Options {
            layout: Layout::Workspace,
            version_ladder: false,
            flutter_app: false,
        },
        Options {
            layout: Layout::Flat,
            version_ladder: true,
            flutter_app: false,
        },
        Options {
            layout: Layout::Workspace,
            version_ladder: true,
            flutter_app: false,
        },
    ] {
        let plan = Plan::build_with(Scale::K1, options);
        assert_eq!(plan.iter().count(), plan.total_files);
        let mut previous: Option<String> = None;
        for item in &plan {
            if let Some(previous) = &previous {
                assert!(
                    previous.as_str() < item.path.as_str(),
                    "order violation under {options:?}: {previous} !< {}",
                    item.path
                );
            }
            previous = Some(item.path);
        }
    }
}

/// 1e: the workspace plan carries every member's `workspace_ref.json`, the
/// root `package_config.json`, both vendored packages and the deep-member
/// split — and zero of those files under Flat.
#[test]
fn workspace_layout_plans_members_refs_and_vendored() {
    let ws = Plan::build_with(
        Scale::K1,
        Options {
            layout: Layout::Workspace,
            version_ladder: false,
            flutter_app: false,
        },
    );
    let flat = Plan::build(Scale::K1);
    assert_eq!(flat.workspace_extras, 0);
    let ws_paths: Vec<String> = ws.iter().map(|f| f.path).collect();
    // Every member carries the ref file; nested members exist.
    assert!(
        ws_paths
            .iter()
            .any(|p| p == "packages/core/.dart_tool/pub/workspace_ref.json")
    );
    assert!(
        ws_paths
            .iter()
            .any(|p| p == "packages/feature_0000/.dart_tool/pub/workspace_ref.json")
    );
    assert!(
        ws_paths
            .iter()
            .any(|p| p == "packages/nested/feature_0001/.dart_tool/pub/workspace_ref.json")
    );
    assert!(
        ws_paths
            .iter()
            .any(|p| p == ".dart_tool/package_config.json")
    );
    assert!(
        ws_paths
            .iter()
            .any(|p| p == "third_party/vendored_ext/pubspec.yaml")
    );
    assert!(
        ws_paths
            .iter()
            .any(|p| p == "third_party/vendored_compat/lib/vendored_compat.dart")
    );
    // Deep members: feature_0001 sits under packages/nested/, not packages/.
    assert!(
        !ws_paths
            .iter()
            .any(|p| p.starts_with("packages/feature_0001/"))
    );
    assert!(
        ws_paths
            .iter()
            .any(|p| p.starts_with("packages/nested/feature_0001/"))
    );
    // The flat plan never emits any of these paths.
    assert!(
        !flat
            .iter()
            .any(|f| f.path.contains(".dart_tool") || f.path.starts_with("third_party"))
    );
}

/// 1d: feature-package sdk constraints ladder across the matrix; workspace
/// members stay >= 3.5 (the pub `resolution: workspace` gate).
#[test]
fn version_ladder_cycles_sdk_constraints() {
    let flat = Plan::build_with(
        Scale::K1,
        Options {
            layout: Layout::Flat,
            version_ladder: true,
            flutter_app: false,
        },
    );
    assert_eq!(flat.feature_sdk_constraint(0), "^2.12.0");
    assert_eq!(flat.feature_sdk_constraint(7), "^3.15.0");
    assert_eq!(flat.feature_sdk_constraint(8), "^2.12.0");
    assert_eq!(flat.feature_language_version(0), "2.12");

    let ws = Plan::build_with(
        Scale::K1,
        Options {
            layout: Layout::Workspace,
            version_ladder: true,
            flutter_app: false,
        },
    );
    for i in 0..ws.packages {
        let version = ws.feature_language_version(i);
        let minor: u32 = version
            .split('.')
            .nth(1)
            .expect("minor")
            .parse()
            .expect("u32");
        assert!(
            minor >= 5,
            "member {i} at {version} breaks the >=3.5 workspace gate"
        );
    }
    // Ladder off: the single-version pin.
    let no_ladder = Plan::build(Scale::K1);
    assert_eq!(no_ladder.feature_sdk_constraint(0), "^3.13.0");
}

/// 1d: `// @dart =` banners bracket the package floor in both directions
/// and cross-version edges only land on different-rung packages.
#[test]
fn banners_bracket_and_edges_cross_versions() {
    let plan = Plan::build_with(
        Scale::K1,
        Options {
            layout: Layout::Flat,
            version_ladder: true,
            flutter_app: false,
        },
    );
    // feature_0000 sits at rung 0 (2.12): file 0 pins one rung BELOW the
    // floor (clamped to 2.12), file 2 pins one rung above (2.17).
    assert_eq!(plan.feature_file_banner(0, 0), Some((2, 12)));
    assert_eq!(plan.feature_file_banner(0, 2), Some((2, 17)));
    assert_eq!(plan.feature_file_banner(0, 1), None);
    // feature_0007 at the top rung (3.15) clamps above at 3.15.
    assert_eq!(plan.feature_file_banner(7, 2), Some((3, 15)));
    // Cross-version edges: file 1 in an 8-cycle; target = index + 3.
    assert_eq!(plan.feature_cross_version_import(0, 1), Some(3));
    assert_eq!(plan.feature_cross_version_import(0, 0), None);
    // Ladder off: nothing.
    let no_ladder = Plan::build(Scale::K1);
    assert_eq!(no_ladder.feature_file_banner(0, 0), None);
    assert_eq!(no_ladder.feature_cross_version_import(0, 1), None);
}

/// 1d materialized: the ladder run emits `// @dart =` banners inside the
/// leading comment block, per-package `analysis_options.yaml` files and at
/// least one cross-version import — all while the flat default tree stays
/// byte-identical (the golden-slice test pins that side).
#[test]
fn ladder_generation_emits_version_faces() {
    let dir = temp_dir("ladder");
    let stats = generate(&options_config(
        dir.clone(),
        Scale::K1,
        13,
        Options {
            layout: Layout::Flat,
            version_ladder: true,
            flutter_app: false,
        },
    ))
    .expect("ladder generation");
    assert!(
        stats
            .coverage
            .count(crate::coverage::Cell::DartVersionBanner)
            >= 3
    );
    assert!(
        stats
            .coverage
            .count(crate::coverage::Cell::EnableExperiment)
            >= 3
    );
    let options = fs::read_to_string(dir.join("analysis_options.yaml")).expect("root options");
    assert!(options.contains("enable-experiment:"), "options: {options}");
    // Banner sits inside the leading comment block.
    let mut bannered = false;
    for entry in fs::read_dir(dir.join("packages/feature_0000/lib/src/models")).expect("models") {
        let text = fs::read_to_string(entry.expect("entry").path()).expect("model");
        if text
            .lines()
            .next()
            .is_some_and(|first| first.starts_with("// @dart ="))
        {
            bannered = true;
        }
    }
    assert!(
        bannered,
        "no // @dart = banner under feature_0000/lib/src/models"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// 1e materialized: the workspace tree carries the shared resolution plus
/// member refs, and the root `package_config` names every member with the
/// languageVersion its constraint settles to.
#[test]
fn workspace_generation_emits_resolution_files() {
    let dir = temp_dir("workspace");
    generate(&options_config(
        dir.clone(),
        Scale::K1,
        3,
        Options {
            layout: Layout::Workspace,
            version_ladder: true,
            flutter_app: false,
        },
    ))
    .expect("workspace generation");
    let config =
        fs::read_to_string(dir.join(".dart_tool/package_config.json")).expect("package config");
    assert!(
        config.contains("\"name\": \"feature_0001\""),
        "config: {config}"
    );
    assert!(
        config.contains("\"name\": \"vendored_compat\""),
        "config: {config}"
    );
    let deep_ref = fs::read_to_string(
        dir.join("packages/nested/feature_0001/.dart_tool/pub/workspace_ref.json"),
    )
    .expect("deep member ref");
    assert_eq!(deep_ref.trim(), "{\n  \"workspaceRoot\": \"../../..\"\n}");
    let root_pubspec = fs::read_to_string(dir.join("pubspec.yaml")).expect("root pubspec");
    assert!(
        root_pubspec.contains("workspace:"),
        "root pubspec: {root_pubspec}"
    );
    assert!(
        root_pubspec.contains("- packages/nested/feature_0001"),
        "deep member listed: {root_pubspec}"
    );
    let member = fs::read_to_string(dir.join("packages/core/pubspec.yaml")).expect("member");
    assert!(member.contains("resolution: workspace"), "member: {member}");
    let _ = fs::remove_dir_all(&dir);
}

// --- #1812 axis 3c: adversarial RSS leg -----------------------------------------

/// X1-RSS serial gate in test form: peak tree RSS of one full `dartq
/// analyze` over the regenerated `tests/corpus/mega-x/100k` tree must stay
/// under the charter bar — 200000 KiB per 100k LOC, the same ceiling
/// `scripts/mega-x-run.sh` asserts in its X1 leg. This is the in-repo leg
/// for serial-phase runs that exercise the corpus without the full
/// mega-x-run report scaffolding.
///
/// Gated twice: `#[ignore]` (never runs in the development-phase suite) and
/// `DARTQ_MEGA_X=1` (explicit opt-in, same convention as the other serial
/// legs). Requires `tests/corpus/mega-x/100k` to be regenerated first
/// (`corpus-gen --scale 100k --seed 0`) and a release harness + dartq —
/// the leg exits early with a skip note when either is missing, so a bare
/// `--ignored` run degrades to a no-op rather than a false red.
#[test]
#[ignore = "serial RSS gate; run explicitly: DARTQ_MEGA_X=1 cargo test -p corpus-gen mega_x_rss -- --ignored"]
fn mega_x_adversarial_rss_ceiling() {
    if std::env::var("DARTQ_MEGA_X").ok().as_deref() != Some("1") {
        eprintln!("skipped: set DARTQ_MEGA_X=1 to run the adversarial RSS leg");
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("corpus-gen lives at <repo>/tools/corpus-gen")
        .to_path_buf();
    let corpus = root.join("tests/corpus/mega-x/100k");
    let harness = root.join("target/release/harness");
    for path in [&corpus, &harness] {
        if !path.exists() {
            eprintln!(
                "skipped: {} missing — regenerate the 100k tree and build the release harness",
                path.display()
            );
            return;
        }
    }
    let out = std::env::temp_dir().join(format!("mega-x-rss-{}", std::process::id()));
    let _ = fs::remove_dir_all(&out);
    let status = std::process::Command::new(&harness)
        .args(["bench", "--metric", "rss-100k", "--corpus-dir"])
        .arg(&corpus)
        .args(["--out"])
        .arg(&out)
        .env("DARTQ_MEGA_X", "1")
        .status()
        .expect("harness bench spawns");
    assert!(status.success(), "rss-100k bench failed: {status}");
    // The report nests as rss-100k/<corpus-name>/report.json and records the
    // normalized figure as `rss_per_100k_loc_kib` (harness bench.rs schema).
    let report =
        fs::read_to_string(out.join("rss-100k/100k/report.json")).expect("rss-100k report present");
    let key = "\"rss_per_100k_loc_kib\"";
    let value: f64 = report[key.len() + report.find(key).expect("key")..]
        .split([':', ',', '}'])
        .nth(1)
        .and_then(|s| s.trim().parse().ok())
        .expect("rss_per_100k_loc_kib numeric in report");
    assert!(
        value <= 200_000.0,
        "adversarial RSS {value} KiB/100k LOC exceeds the 200000 KiB charter bar"
    );
    let _ = fs::remove_dir_all(&out);
}

// --- #2063 flutter-app face -----------------------------------------------------

/// Renders a whole plan in memory — generator.rs's loop minus the disk — for
/// byte-level face tests.
fn render_plan(plan: &Plan, seed: u64) -> BTreeMap<String, String> {
    let mut rng = SplitMix64::new(seed);
    let mut coverage = Coverage::new();
    let mut out = BTreeMap::new();
    let mut rotation_ordinal = 0usize;
    for item in plan {
        let mut ctx = RenderCtx {
            plan,
            rng: &mut rng,
            coverage: &mut coverage,
            rotation_ordinal,
        };
        let content = render(&mut ctx, &item);
        rotation_ordinal = ctx.rotation_ordinal;
        out.insert(item.path, content);
    }
    out
}

/// Splits text into identifier-shaped words (`[A-Za-z_][A-Za-z0-9_]*`).
fn identifier_words(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, ch) in text.char_indices() {
        let part = ch.is_ascii_alphanumeric() || ch == '_';
        if part {
            start.get_or_insert(i);
        } else if let Some(s) = start.take() {
            out.push(&text[s..i]);
        }
    }
    if let Some(s) = start {
        out.push(&text[s..]);
    }
    out
}

/// Capitalized Dart-core types that may legitimately appear in app files.
const DART_CORE_TYPES: [&str; 9] = [
    "String", "List", "Map", "Set", "Iterable", "Future", "Stream", "Object", "Null",
];

/// Whether `token` is an emitter-generated widget class name (derived from
/// the package/file index arithmetic, not the stub surface).
fn generated_widget_base(token: &str) -> bool {
    let t = token.strip_prefix('_').unwrap_or(token);
    let t = t.strip_suffix("State").unwrap_or(t);
    let base = t.trim_end_matches(|c: char| c.is_ascii_digit() || c == '_');
    matches!(
        base,
        "HostWidget" | "TileWidget" | "CardScene" | "ShellScene"
    )
}

/// The stub must declare every surface name the app face may reference, and
/// its part declarations must cover exactly the planned part files.
#[test]
fn flutter_stub_declares_the_app_surface() {
    let plan = Plan::build_with(
        Scale::Slice,
        Options {
            flutter_app: true,
            ..Options::default()
        },
    );
    let files = render_plan(&plan, 0);
    let mut stub = String::new();
    for part in FLUTTER_PARTS {
        let path = format!("{FLUTTER_STUB_DIR}/lib/src/{part}.dart");
        stub.push_str(&files[&path]);
    }
    stub.push_str(&files[&format!("{FLUTTER_STUB_DIR}/lib/widgets.dart")]);
    for name in SURFACE_TYPES.iter().chain(SURFACE_MEMBERS.iter()) {
        assert!(
            identifier_words(&stub).contains(name),
            "flutter stub does not declare surface name {name}"
        );
    }
    for part in FLUTTER_PARTS {
        assert!(
            files[&format!("{FLUTTER_STUB_DIR}/lib/widgets.dart")]
                .contains(&format!("part 'src/{part}.dart';")),
            "widgets.dart missing part declaration for {part}"
        );
    }
}

/// Every widget-bearing app file must reference only names declared by the
/// stub (plus Dart-core types and the emitters' generated class names) —
/// the "no undefined names" half of the stub contract.
#[test]
fn flutter_app_widget_files_reference_only_declared_surface() {
    let plan = Plan::build_with(
        Scale::K1,
        Options {
            flutter_app: true,
            ..Options::default()
        },
    );
    let files = render_plan(&plan, 0);
    let mut widget_files = 0usize;
    for (path, body) in &files {
        if !body.contains("package:flutter/widgets.dart") {
            continue;
        }
        widget_files += 1;
        // The face's contract is syntactically valid Dart: brackets balance
        // (the templates are format strings, so a brace-escaping mistake
        // would otherwise only surface at the oracle run). The generated
        // strings and comments carry no brackets of their own.
        let mut depth = [0i32; 3];
        for ch in body.chars() {
            match ch {
                '{' => depth[0] += 1,
                '}' => depth[0] -= 1,
                '(' => depth[1] += 1,
                ')' => depth[1] -= 1,
                '[' => depth[2] += 1,
                ']' => depth[2] -= 1,
                _ => {}
            }
        }
        assert_eq!(
            depth,
            [0, 0, 0],
            "{path} has unbalanced brackets: {depth:?}"
        );
        // Scan code only: the generated-file notice comment carries
        // capitalized prose ("Generated") that is not a type reference.
        // The templates use line comments only.
        let code = body
            .lines()
            .map(|line| match line.find("//") {
                Some(idx) => &line[..idx],
                None => line,
            })
            .collect::<Vec<&str>>()
            .join("\n");
        for token in identifier_words(&code) {
            let type_shaped = token.starts_with(|c: char| c.is_ascii_uppercase())
                || (token.len() > 1
                    && token.starts_with('_')
                    && token.as_bytes()[1].is_ascii_uppercase());
            if !type_shaped || SURFACE_TYPES.contains(&token) || DART_CORE_TYPES.contains(&token) {
                continue;
            }
            assert!(
                generated_widget_base(token),
                "{path} references undeclared widget type {token}"
            );
        }
    }
    assert!(
        widget_files >= 20,
        "expected a real widget-bearing population, found {widget_files}"
    );
}

/// The face adds exactly the 5-file stub package and keeps the plan's
/// lexicographic order; the default face plans none of it.
#[test]
fn flutter_app_plan_adds_the_stub_package_sorted() {
    let base = Plan::build(Scale::Slice);
    let with = Plan::build_with(
        Scale::Slice,
        Options {
            flutter_app: true,
            ..Options::default()
        },
    );
    assert_eq!(with.total_files, base.total_files + FLUTTER_STUB_FILES);
    assert_eq!(with.iter().count(), with.total_files);

    let paths: Vec<String> = with.iter().map(|f| f.path).collect();
    assert_eq!(
        paths
            .iter()
            .filter(|p| p.starts_with(FLUTTER_STUB_DIR))
            .count(),
        FLUTTER_STUB_FILES
    );
    for part in FLUTTER_PARTS {
        assert!(
            paths.contains(&format!("{FLUTTER_STUB_DIR}/lib/src/{part}.dart")),
            "stub part {part} not planned"
        );
    }
    assert!(paths.contains(&format!("{FLUTTER_STUB_DIR}/lib/widgets.dart")));
    assert!(paths.contains(&format!("{FLUTTER_STUB_DIR}/pubspec.yaml")));

    let mut previous: Option<&String> = None;
    for path in &paths {
        if let Some(previous) = previous {
            assert!(
                previous.as_str() < path.as_str(),
                "order violation: {previous} !< {path}"
            );
        }
        previous = Some(path);
    }
    assert!(!base.iter().any(|f| f.path.starts_with(FLUTTER_STUB_DIR)));
}

/// The face generates deterministically (same seed -> identical bytes),
/// wires the stub dependency into the feature packages, renders real
/// StatefulWidget hosts and widget scenes — and the index-derived widget
/// bytes stay scale-invariant so the app-x golden check can pin them.
#[test]
fn flutter_app_generation_is_deterministic_and_wired() {
    let dir_a = temp_dir("flutter-a");
    let dir_b = temp_dir("flutter-b");
    let options = Options {
        flutter_app: true,
        ..Options::default()
    };
    let stats_a =
        generate(&options_config(dir_a.clone(), Scale::Slice, 0, options)).expect("run a");
    let stats_b = generate(&options_config(dir_b, Scale::Slice, 0, options)).expect("run b");
    assert_eq!(stats_a.manifest.fingerprint, stats_b.manifest.fingerprint);
    assert_eq!(stats_a.manifest.files, stats_b.manifest.files);

    let stub = fs::read_to_string(dir_a.join("packages/flutter/lib/widgets.dart")).expect("stub");
    assert!(
        stub.contains("abstract class StatefulWidget"),
        "stub: {stub}"
    );
    assert!(stub.contains("part 'src/material.dart';"), "stub: {stub}");
    let pubspec =
        fs::read_to_string(dir_a.join("packages/feature_0000/pubspec.yaml")).expect("pubspec");
    assert!(pubspec.contains("path: ../flutter"), "pubspec: {pubspec}");
    let host =
        fs::read_to_string(dir_a.join("packages/feature_0000/lib/src/widgets/widget_00002.dart"))
            .expect("host");
    assert!(host.contains("extends StatefulWidget"), "host: {host}");
    assert!(host.contains("createState"), "host: {host}");
    let scene =
        fs::read_to_string(dir_a.join("lib/src/controllers/controller_00002.dart")).expect("scene");
    assert!(
        scene.contains("import 'package:flutter/widgets.dart';"),
        "scene: {scene}"
    );

    let rendered_slice = render_plan(
        &Plan::build_with(
            Scale::Slice,
            Options {
                flutter_app: true,
                ..Options::default()
            },
        ),
        0,
    );
    let rendered_1k = render_plan(
        &Plan::build_with(
            Scale::K1,
            Options {
                flutter_app: true,
                ..Options::default()
            },
        ),
        0,
    );
    for path in [
        "packages/flutter/lib/widgets.dart",
        "packages/feature_0000/lib/src/widgets/widget_00002.dart",
        "packages/feature_0000/lib/src/widgets/widget_00002_part.dart",
        "packages/feature_0000/lib/src/facets/accel.dart",
    ] {
        assert_eq!(
            rendered_slice[path], rendered_1k[path],
            "scale-invariant bytes drifted for {path}"
        );
    }
    let _ = fs::remove_dir_all(&dir_a);
}
