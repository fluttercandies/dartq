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

/// The `errors` map merges per code with nearest-wins over the include
/// chain (`_applyErrorProcessors`' `processorsByCode` rebuild, parser:219-230
/// — local processors overwrite the include-merged ones because includes
/// apply first, parser:384-408), and the application point defers to the
/// override before the declared severity (`computeSeverity`,
/// `analyzer_cli error_severity.dart`:14-26).
#[test]
fn errors_overrides_merge_nearest_wins() {
    let dir = TempDir::new("errors_chain");
    // The far file re-levels two codes; the near file re-levels one of them
    // again — the near value must win for that code only.
    dir.write(
        "far.yaml",
        "analyzer:\n  errors:\n    undefined_identifier: warning\n    dead_code: error\n",
    );
    dir.write(
        "near.yaml",
        "include: far.yaml\nanalyzer:\n  errors:\n    undefined_identifier: info\n",
    );

    let options = AnalysisOptions::load(&dir.path("near.yaml"), &empty_config(), &dir.0);

    // `undefined_identifier` (registry ERROR, DiagnosticType::CompileTimeError):
    // the nearer `info` replaced the farther `warning`.
    assert_eq!(
        options.processed_severity("undefined_identifier", dartq_diagnostics::Severity::Error),
        Some(dartq_diagnostics::Severity::Info)
    );
    // `dead_code` (registry WARNING): only the far file moved it.
    assert_eq!(
        options.processed_severity("dead_code", dartq_diagnostics::Severity::Warning),
        Some(dartq_diagnostics::Severity::Error)
    );
    // No override: the declared severity stands (error_severity.dart:24).
    assert_eq!(
        options.processed_severity("prefer_single_quotes", dartq_diagnostics::Severity::Info),
        Some(dartq_diagnostics::Severity::Info)
    );
}

/// The `errors` action parsing (`_ParsedErrorProcessorsData.parse`,
/// parse_model:541-631, stringified lowercased through
/// `ErrorConfig._processMap`, error_processor.dart:36-54): `ignore`/`false`
/// filter the code (`ignoreSynonyms`, `analysis_options_file.dart`:46 — a bare
/// YAML `false` stringifies to "false" too), severity words re-level
/// case-insensitively, and any other scalar contributes nothing.
#[test]
fn errors_ignore_false_filter_and_scalar_shapes() {
    let dir = TempDir::new("errors_actions");
    dir.write(
        "analysis_options.yaml",
        concat!(
            "analyzer:\n",
            "  errors:\n",
            "    undefined_identifier: ignore\n",
            "    dead_code: false\n",
            "    prefer_single_quotes: ERROR\n",
            "    unused_local_variable: fatal\n",
            "    duplicate_definition: true\n",
        ),
    );

    let options =
        AnalysisOptions::load(&dir.path("analysis_options.yaml"), &empty_config(), &dir.0);

    // Both ignore synonyms filter (processed severity None — the diagnostic
    // is dropped from the output, error_severity.dart:20-22).
    assert_eq!(
        options.processed_severity("undefined_identifier", dartq_diagnostics::Severity::Error),
        None
    );
    assert_eq!(
        options.processed_severity("dead_code", dartq_diagnostics::Severity::Warning),
        None
    );
    // Severity words are lowercased before the severityMap lookup.
    assert_eq!(
        options.processed_severity("prefer_single_quotes", dartq_diagnostics::Severity::Info),
        Some(dartq_diagnostics::Severity::Error)
    );
    // `fatal` and `true` are neither ignore synonyms nor severities: the
    // entries contribute nothing and the declared severity stands.
    assert_eq!(
        options.processed_severity(
            "unused_local_variable",
            dartq_diagnostics::Severity::Warning
        ),
        Some(dartq_diagnostics::Severity::Warning)
    );
    assert_eq!(
        options.processed_severity("duplicate_definition", dartq_diagnostics::Severity::Error),
        Some(dartq_diagnostics::Severity::Error)
    );
}

/// Re-leveling covers all three severities (`severityMap`,
/// error_processor.dart:14-18) regardless of the declared severity, and the
/// codes lowercased at merge (`ErrorProcessor` constructor,
/// `error_processor.dart`:71) match case-insensitively in the file.
#[test]
fn errors_relevel_to_each_severity() {
    let dir = TempDir::new("errors_levels");
    dir.write(
        "analysis_options.yaml",
        concat!(
            "analyzer:\n",
            "  errors:\n",
            "    undefined_identifier: warning\n",
            "    DEAD_CODE: info\n",
            "    prefer_single_quotes: error\n",
        ),
    );

    let options =
        AnalysisOptions::load(&dir.path("analysis_options.yaml"), &empty_config(), &dir.0);

    let error = dartq_diagnostics::Severity::Error;
    let warning = dartq_diagnostics::Severity::Warning;
    let info = dartq_diagnostics::Severity::Info;
    assert_eq!(
        options.processed_severity("undefined_identifier", error),
        Some(warning)
    );
    assert_eq!(options.processed_severity("dead_code", warning), Some(info));
    assert_eq!(
        options.processed_severity("prefer_single_quotes", info),
        Some(error)
    );
}

/// The `cannot-ignore` face (`_ParsedCannotIgnoreData.parse`,
/// parse_model:346-405; `_applyCannotIgnore`, parser:173-197): explicit
/// names lowercase into the unignorable set; a severity arm marks every
/// registry code whose effective severity — this file's `errors` overrides
/// first, else the registry severity, since `errors` applies before
/// `cannot-ignore` (parser:166-167) — equals it. Enforcement is the
/// suppression gate shape (library_analyzer.dart:552-563).
#[test]
fn cannot_ignore_explicit_names_and_severity_arms() {
    let dir = TempDir::new("cannot_ignore");
    dir.write(
        "analysis_options.yaml",
        concat!(
            "analyzer:\n",
            "  cannot-ignore:\n",
            "    - undefined_identifier\n",
            "    - DEAD_CODE\n",
            "    - info\n",
            "  errors:\n",
            "    duplicate_definition: info\n",
        ),
    );

    let options =
        AnalysisOptions::load(&dir.path("analysis_options.yaml"), &empty_config(), &dir.0);

    // Explicit names, matched lowercased (parser:193 `name.toLowerCase()`):
    // ignore comments cannot suppress either.
    assert!(!options.is_ignorable("undefined_identifier"));
    assert!(!options.is_ignorable("dead_code"));
    // Everything else stays suppressible — including the ERROR-severity
    // codes the `info` arm does not reach.
    assert!(options.is_ignorable("unused_local_variable"));
    // The `info` arm: registry INFO codes (`prefer_single_quotes` is a LINT →
    // Info) become unignorable, and `duplicate_definition` joins through its
    // this-file re-level to info (the errors-before-cannot-ignore order,
    // parser:166-167) despite its registry severity being ERROR.
    assert!(!options.is_ignorable("prefer_single_quotes"));
    assert!(!options.is_ignorable("duplicate_definition"));
    // A WARNING-severity registry code (`unused_local_variable` is
    // STATIC_WARNING) stays outside the `info` arm.
    assert!(options.is_ignorable("unused_local_variable"));
}

/// The language strictness scalars (`_ParsedLanguageData.parse`,
/// parse_model:904-998, applied set-if-present by `_applyLanguage`,
/// parser:250-260): booleans and `true`/`false` strings both parse
/// (`toBool`, util/yaml.dart:156-169), unknown keys and non-boolean values
/// contribute nothing, and absent sections leave all three `None`.
#[test]
fn language_scalars_parse_and_expose() {
    let dir = TempDir::new("language_scalars");
    dir.write(
        "full.yaml",
        concat!(
            "analyzer:\n",
            "  language:\n",
            "    strict-casts: true\n",
            "    strict-inference: false\n",
            "    strict-raw-types: true\n",
        ),
    );
    dir.write(
        "partial.yaml",
        concat!(
            "include: full.yaml\n",
            "analyzer:\n",
            "  language:\n",
            "    strict-casts: \"false\"\n",
            "    not-a-scalar-key: true\n",
            "    strict-raw-types: maybe\n",
        ),
    );

    let full = AnalysisOptions::load(&dir.path("full.yaml"), &empty_config(), &dir.0);
    assert_eq!(full.strict_casts, Some(true));
    assert_eq!(full.strict_inference, Some(false));
    assert_eq!(full.strict_raw_types, Some(true));

    // The nearer file overrides `strict-casts` (set-if-present, the
    // `_applyLanguage` scalar posture); the unknown key and the non-boolean
    // `maybe` contribute nothing, so `strict-raw-types` inherits the far
    // value.
    let partial = AnalysisOptions::load(&dir.path("partial.yaml"), &empty_config(), &dir.0);
    assert_eq!(partial.strict_casts, Some(false));
    assert_eq!(partial.strict_inference, Some(false));
    assert_eq!(partial.strict_raw_types, Some(true));

    let absent = TempDir::new("language_absent");
    absent.write("analysis_options.yaml", "linter:\n  rules: []\n");
    let none = AnalysisOptions::load(
        &absent.path("analysis_options.yaml"),
        &empty_config(),
        &absent.0,
    );
    assert_eq!(none.strict_casts, None);
    assert_eq!(none.strict_inference, None);
    assert_eq!(none.strict_raw_types, None);
}

/// The output face wiring: `Engine::analyze` applies the governing options'
/// `errors` overrides to every row — a filtered code never surfaces and a
/// re-leveled code surfaces at its new severity (`computeSeverity`,
/// `analyzer_cli error_severity.dart`:14-26, at the CLI's output processing).
#[test]
fn engine_analyze_applies_errors_overrides() {
    use common::Fixture;

    // Two reliable engine-pass rows: the semantic `undefined_identifier`
    // and the semantic `duplicate_definition` (both COMPILE_TIME_ERROR →
    // registry ERROR).
    let fixture = Fixture::new(&[(
        "lib/main.dart",
        "void main() {\n  var ok = unknown;\n}\nvoid dup() {}\nvoid dup() {}\n",
    )]);
    std::fs::write(
        fixture.path("analysis_options.yaml"),
        concat!(
            "analyzer:\n",
            "  errors:\n",
            "    undefined_identifier: warning\n",
            "    duplicate_definition: ignore\n",
        ),
    )
    .expect("options");

    let mut engine = fixture.engine();
    engine.discover_workspace().expect("discovery");
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &dartq_engine::CancellationToken::new())
        .expect("analyze");

    let rows: Vec<(String, dartq_diagnostics::Severity)> = diagnostics
        .iter()
        .map(|d| (d.code.name.to_owned(), d.severity))
        .collect();
    // The semantic ERROR row re-leveled to WARNING.
    assert!(
        rows.contains(&(
            "undefined_identifier".to_owned(),
            dartq_diagnostics::Severity::Warning
        )),
        "the override re-levels the row: {rows:?}"
    );
    // The `ignore`-filtered code dropped from the output entirely.
    assert!(
        rows.iter().all(|(code, _)| code != "duplicate_definition"),
        "the ignore filter drops the row: {rows:?}"
    );
}
