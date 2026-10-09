//! Analysis-options merge pins (PRD §12, ticket #313). The merge semantics
//! mirror the official include walk (`_applyParsedFiles`): depth-first,
//! included files apply before their includer, so nearer entries override
//! farther ones; exclude globs anchor to the declaring file's directory
//! (`_getExcludedGlobs`); `package:` includes resolve through the package
//! config. The discovery test closes the loop: excluded paths leave the
//! analysis set entirely.

mod common;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use dartq_engine::analysis_options::AnalysisOptions;
use dartq_pub::PackageConfig;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A throwaway directory removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "dartq-opt-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        TempDir(dir)
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }

    fn write(&self, relative: &str, content: &str) {
        let path = self.path(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path, content).expect("write");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A package config with no packages (relative includes need none).
fn empty_config() -> PackageConfig {
    PackageConfig::parse(r#"{"configVersion": 2, "packages": []}"#).expect("empty config")
}

#[test]
fn three_deep_include_chain_near_overrides_far() {
    let dir = TempDir::new("chain");
    dir.write(
        "options_far.yaml",
        "linter:\n  rules:\n    - alpha_rule\n    - beta_rule\nanalyzer:\n  exclude:\n    - \"far/**\"\n",
    );
    dir.write(
        "options_mid.yaml",
        "include: options_far.yaml\nlinter:\n  rules:\n    beta_rule: false\n    gamma_rule: true\n",
    );
    dir.write(
        "options_near.yaml",
        "include: options_mid.yaml\nlinter:\n  rules:\n    - delta_rule\n",
    );

    let options = AnalysisOptions::load(&dir.path("options_near.yaml"), &empty_config(), &dir.0);

    // far {alpha, beta} ∪ mid {beta: false, gamma} ∪ near {delta}:
    // the nearer `beta: false` disables the farther list entry.
    assert_eq!(
        options.lint_rules,
        vec!["alpha_rule", "delta_rule", "gamma_rule"]
    );
    // The far file's exclude survives the merge, anchored to the shared
    // declaring directory.
    assert!(options.is_excluded(&dir.path("far/x.dart")));
    assert!(!options.is_excluded(&dir.path("lib/x.dart")));
}

#[test]
fn include_stacking_and_diamond_dedupe() {
    let dir = TempDir::new("diamond");
    dir.write(
        "base.yaml",
        "linter:\n  rules:\n    - base_rule\nanalyzer:\n  exclude:\n    - \"shared/**\"\n",
    );
    dir.write(
        "mid1.yaml",
        "include: base.yaml\nlinter:\n  rules:\n    - mid_rule\n",
    );
    dir.write(
        "mid2.yaml",
        "include: base.yaml\nlinter:\n  rules:\n    - mid2_rule\n",
    );
    dir.write("near.yaml", "include:\n  - mid1.yaml\n  - mid2.yaml\n");

    let options = AnalysisOptions::load(&dir.path("near.yaml"), &empty_config(), &dir.0);

    // Both branches contribute; the diamond (base twice) unions idempotently
    // and does not duplicate the exclude pattern.
    assert_eq!(
        options.lint_rules,
        vec!["base_rule", "mid2_rule", "mid_rule"]
    );
    assert_eq!(
        options.exclude_globs().len(),
        1,
        "the diamond-included pattern contributes once"
    );
    assert!(options.is_excluded(&dir.path("shared/a.dart")));
}

#[test]
fn package_uri_include_resolves_and_expands_rule_sets() {
    let dir = TempDir::new("pkguri");
    // A miniature `package:lints`: recommended includes core, the same
    // shape as .third/lints/lib/recommended.yaml.
    dir.write(
        "vendor/lints/lib/core.yaml",
        "linter:\n  rules:\n    - core_rule_a\n    - core_rule_b\n",
    );
    dir.write(
        "vendor/lints/lib/recommended.yaml",
        "include: package:lints/core.yaml\nlinter:\n  rules:\n    - recommended_rule\n",
    );
    dir.write(
        ".dart_tool/package_config.json",
        r#"{"configVersion": 2, "packages": [{"name": "lints", "rootUri": "../vendor/lints", "packageUri": "lib/"}]}"#,
    );
    dir.write(
        "analysis_options.yaml",
        "include: package:lints/recommended.yaml\nlinter:\n  rules:\n    - local_rule\n",
    );

    let config = PackageConfig::parse(
        &std::fs::read_to_string(dir.path(".dart_tool/package_config.json")).expect("config"),
    )
    .expect("valid config");
    let options = AnalysisOptions::load(
        &dir.path("analysis_options.yaml"),
        &config,
        &dir.path(".dart_tool"),
    );

    // recommended → its explicit rules + core's, expanded through the
    // package: include chain, plus the local addition.
    assert_eq!(
        options.lint_rules,
        vec![
            "core_rule_a",
            "core_rule_b",
            "local_rule",
            "recommended_rule"
        ]
    );
}

#[test]
fn recursive_include_terminates_and_still_merges() {
    let dir = TempDir::new("cycle");
    dir.write(
        "a.yaml",
        "include: b.yaml\nlinter:\n  rules:\n    - a_rule\n",
    );
    dir.write(
        "b.yaml",
        "include: a.yaml\nlinter:\n  rules:\n    - b_rule\n",
    );

    let options = AnalysisOptions::load(&dir.path("a.yaml"), &empty_config(), &dir.0);

    assert_eq!(options.lint_rules, vec!["a_rule", "b_rule"]);
}

#[test]
fn exclude_globs_anchor_to_the_declaring_directory() {
    let dir = TempDir::new("excl");
    dir.write(
        "opts/analysis_options.yaml",
        "analyzer:\n  exclude:\n    - \"gen/**\"\n",
    );

    let options = AnalysisOptions::load(
        &dir.path("opts/analysis_options.yaml"),
        &empty_config(),
        &dir.0,
    );

    // Relative to the declaring file's directory (`opts/`), not the
    // workspace root.
    assert!(options.is_excluded(&dir.path("opts/gen/x.dart")));
    assert!(options.is_excluded(&dir.path("opts/gen/nested/y.dart")));
    assert!(!options.is_excluded(&dir.path("gen/x.dart")));
    assert!(!options.is_excluded(&dir.path("opts/genuine/x.dart")));
    // `foo/**` also matches the directory itself so the walk stops
    // descending (context_locator.dart:633-638).
    assert!(options.is_excluded(&dir.path("opts/gen")));
}

#[test]
fn invalid_exclude_sections_and_entries_are_ignored() {
    let dir = TempDir::new("exclshape");
    // A scalar exclude section (not a list) is ignored, as are non-string
    // list entries (analysis_options_parse_model.dart:634-681).
    dir.write(
        "analysis_options.yaml",
        "analyzer:\n  exclude: gen/**\nlinter:\n  rules:\n    - some_rule\n",
    );
    dir.write(
        "entries.yaml",
        "analyzer:\n  exclude:\n    - 42\n    - \"valid/**\"\n",
    );

    let scalar = AnalysisOptions::load(&dir.path("analysis_options.yaml"), &empty_config(), &dir.0);
    assert_eq!(scalar.lint_rules, vec!["some_rule"]);
    assert_eq!(scalar.exclude_globs().len(), 0);

    let entries = AnalysisOptions::load(&dir.path("entries.yaml"), &empty_config(), &dir.0);
    assert_eq!(entries.exclude_globs().len(), 1);
    assert!(entries.is_excluded(&dir.path("valid/x.dart")));
}

#[test]
fn unreadable_and_missing_includes_degrade_to_nothing() {
    let dir = TempDir::new("degrade");
    dir.write(
        "analysis_options.yaml",
        "include:\n  - missing.yaml\n  - package:nope/absent.yaml\nlinter:\n  rules:\n    - kept_rule\n",
    );

    let options =
        AnalysisOptions::load(&dir.path("analysis_options.yaml"), &empty_config(), &dir.0);

    assert_eq!(options.lint_rules, vec!["kept_rule"]);
    assert_eq!(options.exclude_globs().len(), 0);
}

/// The diagnostics pipeline applies exclusions: an excluded file never
/// enters the analysis set (PRD §12.2), so its errors never surface.
#[test]
fn discovery_excludes_files_from_the_analysis_set() {
    use common::Fixture;

    let fixture = Fixture::new(&[
        ("lib/keep.dart", "void keep() {}\n"),
        // Invalid syntax: if the exclusion failed, this would surface as a
        // diagnostic and flood the assertion.
        ("lib/gen/generated.dart", "void generated() {\n"),
    ]);
    std::fs::write(
        fixture.path("analysis_options.yaml"),
        "analyzer:\n  exclude:\n    - \"lib/gen/**\"\n",
    )
    .expect("options");

    let mut engine = fixture.engine();
    let stats = engine.discover_workspace().expect("discovery");
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &dartq_engine::CancellationToken::new())
        .expect("analyze");

    assert_eq!(stats.file_count, 1, "only lib/keep.dart enters the set");
    assert!(
        engine
            .file_table()
            .try_file_id(&fixture.path("lib/gen/generated.dart"))
            .is_none(),
        "excluded file must not be interned"
    );
    assert!(
        engine
            .file_table()
            .try_file_id(&fixture.path("lib/keep.dart"))
            .is_some()
    );
    assert!(
        diagnostics.is_empty(),
        "the excluded syntax error must not surface: {diagnostics:?}"
    );

    // The merged options face: rule names for the lint enable set, excludes
    // precompiled once.
    let options = engine.options_for_root(&fixture.root).expect("options");
    assert!(options.lint_rules.is_empty());
    assert_eq!(options.exclude_globs().len(), 1);
    assert!(options.is_excluded(&fixture.path("lib/gen/generated.dart")));
    assert!(!options.is_excluded(&fixture.path("lib/keep.dart")));
}

/// Per-root resolution (the `EngineState::new` wiring): a root without its
/// own options file inherits the nearest ancestor's; a root with one uses
/// its own. Inherited globs keep their declaring-directory anchors — the
/// root file's `shared/**` excludes `<root>/shared/**`, never each member's
/// own `shared/` (the official `LocatedGlob` anchoring).
#[test]
fn per_root_options_inherit_or_override() {
    let dir = TempDir::new("roots");
    dir.write(
        "analysis_options.yaml",
        "analyzer:\n  exclude:\n    - \"shared/**\"\n",
    );
    dir.write(
        "member_b/analysis_options.yaml",
        "analyzer:\n  exclude:\n    - \"local/**\"\n",
    );

    let roots = vec![dir.path("member_a"), dir.path("member_b")];
    let per_root = AnalysisOptions::for_roots(&roots, &empty_config(), &dir.0);

    let member_a = per_root.get(&dir.path("member_a")).expect("member_a");
    assert!(member_a.is_excluded(&dir.path("shared/x.dart")));
    assert!(!member_a.is_excluded(&dir.path("member_a/shared/x.dart")));

    let member_b = per_root.get(&dir.path("member_b")).expect("member_b");
    assert!(member_b.is_excluded(&dir.path("member_b/local/x.dart")));
    assert!(!member_b.is_excluded(&dir.path("shared/x.dart")));
}
