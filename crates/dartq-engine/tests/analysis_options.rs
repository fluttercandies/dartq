//! Analysis-options merge pins. The merge semantics
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
    // shape as the upstream `lints` package's lib/recommended.yaml.
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

/// The `exclude_invalid_glob` face (`_ParsedExcludesData.parse`,
/// parse_model:660-668): a string pattern starting with `/` reports the
/// verbatim row anchored at its own occurrence and contributes no matching
/// glob (an absolute-rooted glob never matches a parent-relative path),
/// other leading characters (`~`) stay silent and compile as ordinary
/// literal relative globs, and an included file's row wraps into
/// `included_file_warning` (the correction dropped).
#[test]
fn leading_slash_exclude_patterns_report_exclude_invalid_glob() {
    let dir = TempDir::new("bad_glob");
    dir.write(
        "analysis_options.yaml",
        concat!(
            "analyzer:\n",
            "  exclude:\n",
            "    - \"/abs/pattern.dart\"\n",
            "    - \"generated/**\"\n",
            "    - \"~/home.dart\"\n",
        ),
    );

    let options =
        AnalysisOptions::load(&dir.path("analysis_options.yaml"), &empty_config(), &dir.0);

    assert_eq!(options.issues.len(), 1, "{:?}", options.issues);
    let issue = &options.issues[0];
    assert_eq!(issue.code.name, "exclude_invalid_glob");
    assert_eq!(issue.code.machine, "EXCLUDE_INVALID_GLOB");
    assert_eq!(
        issue.code.dtype,
        dartq_diagnostics::DiagnosticType::StaticWarning
    );
    assert_eq!(
        issue.message,
        "The exclude glob pattern '/abs/pattern.dart' shouldn't start with a forward slash."
    );
    assert_eq!(
        issue.correction.as_deref(),
        Some("Remove the leading forward slash.")
    );
    // Anchored at the pattern token, not the section key (offset 28 in the
    // text above).
    assert_eq!(issue.offset, 28);
    assert_eq!(issue.length, 17);

    // The reported pattern contributes no glob: its leading `/` anchors it
    // at the filesystem root, so it can never match a path relative to the
    // declaring directory. The other two patterns compile — the locator's
    // `addGlob` runs unconditionally per pattern (context_locator.dart
    // :630-636) and `~` is a literal there, an ordinary relative glob.
    assert_eq!(options.exclude_globs().len(), 2);
    assert!(!options.is_excluded(&dir.path("abs/pattern.dart")));
    assert!(options.is_excluded(&dir.path("generated/a.dart")));
    assert!(options.is_excluded(&dir.path("~/home.dart")));

    // An included file's row wraps (`_IncludedDiagnosticListener`): the
    // message names the included file and the inner inclusive span
    // (`/deep/x.dart` in far.yaml: 28..39), the row anchors at the branch
    // directive, and the correction never survives.
    dir.write("near.yaml", "include: far.yaml\n");
    dir.write(
        "far.yaml",
        "analyzer:\n  exclude:\n    - \"/deep/x.dart\"\n",
    );
    let near = AnalysisOptions::load(&dir.path("near.yaml"), &empty_config(), &dir.0);
    assert_eq!(near.issues.len(), 1, "{:?}", near.issues);
    let wrapped = &near.issues[0];
    assert_eq!(wrapped.code.name, "included_file_warning");
    let far = dir.path("far.yaml").display().to_string();
    assert_eq!(
        wrapped.message,
        format!(
            "Warning in the included options file {far}(28..39): The exclude glob pattern \
             '/deep/x.dart' shouldn't start with a forward slash."
        )
    );
    assert_eq!(wrapped.correction, None);
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
/// enters the analysis set, so its errors never surface.
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
    // Everything else stays suppressible — including the WARNING-severity
    // codes the `info` arm does not reach.
    assert!(options.is_ignorable("unused_local_variable"));
    // The `info` arm: registry INFO-severity `diagnosticCodeValues` entries
    // (`deprecated_member_use` is HINT → Info upstream) become unignorable,
    // and `duplicate_definition` joins through its this-file re-level to
    // info (the errors-before-cannot-ignore order, parser:166-167) despite
    // its registry severity being ERROR. A lint rule name is NOT in
    // `diagnosticCodeValues` at all (`prefer_single_quotes` lives in the
    // lint registry upstream), so the arm never reaches it — only the
    // explicit-name path could.
    assert!(!options.is_ignorable("deprecated_member_use"));
    assert!(!options.is_ignorable("duplicate_definition"));
    assert!(options.is_ignorable("prefer_single_quotes"));
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

/// The `unsupported_value` face of the strict-* scalars
/// (`_ParsedLanguageData.parse`, parse_model:944-956): a scalar failing the
/// `toBool` conversion reports the verbatim row with the `trueOrFalse`
/// proposal anchored at the value's occurrence, case variants of the
/// boolean strings stay silent and still merge, and a present-but-null
/// value reports too while the section-level null stays tolerated (:918).
#[test]
fn strict_non_boolean_values_report_unsupported_value() {
    let dir = TempDir::new("strict_value");
    dir.write(
        "analysis_options.yaml",
        concat!(
            "analyzer:\n",
            "  language:\n",
            "    strict-casts: 42\n",
            "    strict-inference: TRUE\n",
            "    strict-raw-types: yes\n",
        ),
    );

    let options =
        AnalysisOptions::load(&dir.path("analysis_options.yaml"), &empty_config(), &dir.0);

    assert_eq!(options.issues.len(), 2, "{:?}", options.issues);
    let casts = &options.issues[0];
    assert_eq!(casts.code.name, "unsupported_value");
    assert_eq!(casts.code.machine, "UNSUPPORTED_VALUE");
    assert_eq!(
        casts.code.dtype,
        dartq_diagnostics::DiagnosticType::StaticWarning
    );
    assert_eq!(
        casts.message,
        "The value '42' isn't supported by 'strict-casts'."
    );
    assert_eq!(
        casts.correction.as_deref(),
        Some("Try using one of the supported values: 'true' and 'false'.")
    );
    // Anchored at the value token after its option key (offset 40 above).
    assert_eq!(casts.offset, 40);
    assert_eq!(casts.length, 2);

    let raw_types = &options.issues[1];
    assert_eq!(
        raw_types.message,
        "The value 'yes' isn't supported by 'strict-raw-types'."
    );
    assert_eq!(raw_types.offset, 92);
    assert_eq!(raw_types.length, 3);

    // The case-insensitive boolean string merges; the reported values
    // contribute nothing.
    assert_eq!(options.strict_casts, None);
    assert_eq!(options.strict_inference, Some(true));
    assert_eq!(options.strict_raw_types, None);

    // A present-but-null value is a scalar failing the conversion
    // (`valueNode.value.toString()` renders `null`).
    let nulls = TempDir::new("strict_null");
    nulls.write(
        "analysis_options.yaml",
        "analyzer:\n  language:\n    strict-casts: null\n",
    );
    let options = AnalysisOptions::load(
        &nulls.path("analysis_options.yaml"),
        &empty_config(),
        &nulls.0,
    );
    assert_eq!(options.issues.len(), 1, "{:?}", options.issues);
    assert_eq!(
        options.issues[0].message,
        "The value 'null' isn't supported by 'strict-casts'."
    );
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

// --- Options-file self-diagnostics (#457) ---

/// A missing include (relative or `package:`) reports `include_file_not_found`
/// at every edge (`_walkInclude`, include_walker.dart:121-134; registry row
/// `diagnostic.g.dart`:7757-7776 — `STATIC_WARNING`, message names the URI, the
/// declaring file, and the context root) while the merge still degrades to
/// "contributes nothing". The row anchors at the URI text's first occurrence.
#[test]
fn missing_include_reports_include_file_not_found() {
    let dir = TempDir::new("notfound");
    let text = "include:\n  - missing.yaml\n  - package:nope/absent.yaml\nlinter:\n  rules:\n    - kept_rule\n";
    dir.write("analysis_options.yaml", text);

    let options =
        AnalysisOptions::load(&dir.path("analysis_options.yaml"), &empty_config(), &dir.0);

    assert_eq!(options.lint_rules, vec!["kept_rule"]);
    let including = dir.path("analysis_options.yaml").display().to_string();
    let expected = [
        (
            "missing.yaml",
            13,
            format!(
                "The URI 'missing.yaml' included in '{including}' can't be found when \
                 analyzing '{}'.",
                dir.0.display()
            ),
        ),
        (
            "package:nope/absent.yaml",
            30,
            format!(
                "The URI 'package:nope/absent.yaml' included in '{including}' can't be \
                 found when analyzing '{}'.",
                dir.0.display()
            ),
        ),
    ];
    assert_eq!(options.issues.len(), expected.len(), "{:?}", options.issues);
    for (issue, (uri, offset, message)) in options.issues.iter().zip(&expected) {
        assert_eq!(issue.code.name, "include_file_not_found");
        assert_eq!(issue.code.machine, "INCLUDE_FILE_NOT_FOUND");
        assert_eq!(
            issue.code.dtype,
            dartq_diagnostics::DiagnosticType::StaticWarning
        );
        assert!(issue.code.has_published_docs, "registry :7757 publishes it");
        assert_eq!(issue.message, *message);
        assert_eq!(issue.correction, None);
        assert_eq!(issue.offset, *offset, "anchor at the URI text of {uri}");
        assert_eq!(
            issue.length,
            u32::try_from(uri.chars().map(char::len_utf16).sum::<usize>()).expect("u32"),
        );
    }
}

/// The edge decision order mirrors `_walkInclude` (`include_walker.dart`
/// :121-151) and `_walkParsedInclude` (:153-188): a parsed include resolving
/// back to the *initial* file reports `recursive_include_file` (:161-171,
/// registry diagnostic.g.dart:14675-14697) and stops the edge; a chain cycle
/// that does not pass through the initial file wraps into
/// `included_file_warning` naming the repeated file and the inclusive span
/// of its own outgoing include directive (`spanInChain`, walker :173-188);
/// missing wins for unresolvable edges because recursion is only reached for
/// parsed ones.
#[test]
fn recursive_include_reports_back_to_initial_and_chain_cycles_wrap() {
    let dir = TempDir::new("recursive");
    dir.write("a.yaml", "include: b.yaml\n");
    dir.write("b.yaml", "include: a.yaml\n");

    let options = AnalysisOptions::load(&dir.path("a.yaml"), &empty_config(), &dir.0);

    assert_eq!(options.issues.len(), 1, "{:?}", options.issues);
    let issue = &options.issues[0];
    assert_eq!(issue.code.name, "recursive_include_file");
    let b = dir.path("b.yaml").display().to_string();
    assert_eq!(
        issue.message,
        format!(
            "The URI 'a.yaml' included in '{b}' includes '{b}', creating a circular \
             reference."
        )
    );
    assert_eq!(
        issue.correction.as_deref(),
        Some("Try changing the chain of 'include's to break the circularity.")
    );

    // The indirect cycle b → c → b never passes through the initial file:
    // the official row is the `included_file_warning` wrap (walker :176-188)
    // — the message names b (the repeated file) and the inclusive span of
    // b's own outgoing directive (`c.yaml` in b.yaml: 9..14), and the row
    // anchors at the branch's first directive (`b.yaml` in a.yaml: 9, len 7).
    let deep = TempDir::new("chain_cycle");
    deep.write("a.yaml", "include: b.yaml\n");
    deep.write("b.yaml", "include: c.yaml\n");
    deep.write("c.yaml", "include: b.yaml\n");
    let options = AnalysisOptions::load(&deep.path("a.yaml"), &empty_config(), &deep.0);
    assert_eq!(options.issues.len(), 1, "{:?}", options.issues);
    let issue = &options.issues[0];
    assert_eq!(issue.code.name, "included_file_warning");
    assert_eq!(issue.code.machine, "INCLUDED_FILE_WARNING");
    assert_eq!(
        issue.code.dtype,
        dartq_diagnostics::DiagnosticType::StaticWarning
    );
    let b = deep.path("b.yaml").display().to_string();
    assert_eq!(
        issue.message,
        format!(
            "Warning in the included options file {b}(9..14): The file includes itself recursively."
        )
    );
    assert_eq!(issue.correction, None, "the wrap drops the correction");
    assert_eq!(issue.offset, 9, "the b.yaml URI token in a.yaml");
    assert_eq!(issue.length, 6);
}

/// Include-chain rows attribute to the *initial* file at every depth (the
/// official reports them on the initial diagnostic reporter, `include_walker`
/// .dart:125-133), while the message names the declaring file of the broken
/// edge (`includingFilePath`, walker :129).
#[test]
fn include_chain_rows_attribute_to_the_initial_file() {
    let dir = TempDir::new("chain_attr");
    dir.write("near.yaml", "include: mid.yaml\n");
    dir.write("mid.yaml", "include: far.yaml\n");
    dir.write("far.yaml", "include: missing.yaml\n");

    let options = AnalysisOptions::load(&dir.path("near.yaml"), &empty_config(), &dir.0);

    assert_eq!(options.issues.len(), 1, "{:?}", options.issues);
    let issue = &options.issues[0];
    assert_eq!(issue.code.name, "include_file_not_found");
    let far = dir.path("far.yaml").display().to_string();
    assert_eq!(
        issue.message,
        format!(
            "The URI 'missing.yaml' included in '{far}' can't be found when analyzing \
             '{}'.",
            dir.0.display()
        )
    );
    // Attribution: the initial file's own path (rows attribute there, and
    // `file_path` is what the output face interns).
    assert_eq!(options.file_path, dir.path("near.yaml"));
}

/// An included file's local shape problems wrap into `included_file_warning`
/// (`_IncludedDiagnosticListener`, include_walker.dart:236-247): the message
/// names the included file and the inner row's inclusive span, the wrapped
/// row anchors at the branch's first directive, and the correction never
/// survives. The included file's own load still reports the row raw (`_walk`
/// reports local diagnostics directly for the initial file, walker :74-76).
#[test]
fn included_file_local_problems_wrap_to_the_includer() {
    let dir = TempDir::new("wrapped");
    dir.write("near.yaml", "include: far.yaml\n");
    dir.write("far.yaml", "linter: 42\n");

    let near = AnalysisOptions::load(&dir.path("near.yaml"), &empty_config(), &dir.0);
    assert_eq!(near.issues.len(), 1, "{:?}", near.issues);
    let issue = &near.issues[0];
    assert_eq!(issue.code.name, "included_file_warning");
    // The inner invalid-section row anchors at `linter` (0, 6) in far.yaml —
    // the inclusive end folds to 5.
    let far = dir.path("far.yaml").display().to_string();
    assert_eq!(
        issue.message,
        format!(
            "Warning in the included options file {far}(0..5): Invalid format for the 'linter' section."
        )
    );
    assert_eq!(issue.offset, 9, "the branch's first include directive");
    assert_eq!(issue.length, 8, "the far.yaml URI token");

    let far = AnalysisOptions::load(&dir.path("far.yaml"), &empty_config(), &dir.0);
    assert_eq!(far.issues.len(), 1);
    assert_eq!(far.issues[0].code.name, "invalid_section_format");
    assert_eq!(
        far.issues[0].message,
        "Invalid format for the 'linter' section."
    );
}

/// The invalid-section shapes of the sections the engine consumes
/// (`_ParsedAnalyzerData.parse` parse_model:266-272, errors :551-557,
/// cannot-ignore :356-362, exclude :644-650, language :919-925) report
/// `invalid_section_format` with the section name, and the merged options
/// still degrade exactly as before (the rows add no merge behavior).
#[test]
fn invalid_section_shapes_report_rows() {
    let dir = TempDir::new("invalid_sections");
    dir.write("top.yaml", "analyzer: 42\nlinter: 42\n");
    dir.write(
        "sections.yaml",
        "analyzer:\n  errors: 42\n  exclude: 42\n  cannot-ignore: 42\n  language: 42\n",
    );

    let top = AnalysisOptions::load(&dir.path("top.yaml"), &empty_config(), &dir.0);
    let mut names: Vec<&str> = top
        .issues
        .iter()
        .map(|issue| issue.message.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "Invalid format for the 'analyzer' section.",
            "Invalid format for the 'linter' section."
        ]
    );

    let nested = AnalysisOptions::load(&dir.path("sections.yaml"), &empty_config(), &dir.0);
    let mut names: Vec<&str> = nested
        .issues
        .iter()
        .map(|issue| issue.message.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "Invalid format for the 'cannot-ignore' section.",
            "Invalid format for the 'errors' section.",
            "Invalid format for the 'exclude' section.",
            "Invalid format for the 'language' section.",
        ]
    );
    for issue in &nested.issues {
        assert_eq!(issue.code.name, "invalid_section_format");
        assert_eq!(issue.correction, None);
    }
}

/// The unsupported-option family (`_reportUnsupportedSectionKeys`,
/// parse_model:13-37, and the errors-action check :599-612) with verbatim
/// correction forms: singular for one legal value (`linter` → `rules`),
/// the `quotedAndCommaSeparatedWithAnd` proposal otherwise (string.dart
/// :72-107 — Oxford comma at three or more), and the errors list verbatim
/// with its duplicate `false` and the error→info→warning `severityMap`
/// order (parse_model:55-59, error_processor.dart:14-19). Null sections:
/// `errors`/`exclude`/`cannot-ignore` report (valueAt returns the node,
/// parse_model:547-556), `language` and `linter` are null-tolerant
/// (:918, :1132).
#[test]
#[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
fn unsupported_options_report_verbatim_rows() {
    let dir = TempDir::new("unsupported");
    dir.write(
        "analysis_options.yaml",
        concat!(
            "analyzer:\n",
            "  bogus: true\n",
            "  language:\n",
            "    wrong-key: true\n",
            "    strict-casts: [1]\n",
            "  errors:\n",
            "    dead_code: fatal\n",
            "    dup: [1]\n",
            "linter:\n",
            "  notrules: true\n",
        ),
    );

    let options =
        AnalysisOptions::load(&dir.path("analysis_options.yaml"), &empty_config(), &dir.0);

    let expected: Vec<(&str, &str, &str)> = vec![
        (
            "unsupported_option",
            "The option 'bogus' isn't supported by 'analyzer'.",
            "Try using one of the supported options: 'cannot-ignore', \
             'enable-experiment', 'errors', 'exclude', 'language', \
             'optional-checks', 'plugins', and 'strong-mode'.",
        ),
        (
            "unsupported_option",
            "The option 'fatal' isn't supported by 'errors'.",
            "Try using one of the supported options: 'ignore', 'false', 'true', \
             'false', 'error', 'info', and 'warning'.",
        ),
        (
            "invalid_section_format",
            "Invalid format for the 'errors' section.",
            "",
        ),
        (
            "unsupported_option",
            "The option 'wrong-key' isn't supported by 'language'.",
            "Try using one of the supported options: 'strict-casts', \
             'strict-inference', and 'strict-raw-types'.",
        ),
        (
            "invalid_section_format",
            "Invalid format for the 'strict-casts' section.",
            "",
        ),
        (
            "unsupported_option",
            "The option 'notrules' isn't supported by 'linter'.",
            "Try using the only supported option: 'rules', or removing the option.",
        ),
    ];
    let actual: Vec<(&str, &str, Option<&str>)> = options
        .issues
        .iter()
        .map(|issue| {
            (
                issue.code.name,
                issue.message.as_str(),
                issue.correction.as_deref(),
            )
        })
        .collect();
    let expected: Vec<(&str, &str, Option<&str>)> = expected
        .into_iter()
        .map(|(code, message, correction)| {
            (
                code,
                message,
                if correction.is_empty() {
                    None
                } else {
                    Some(correction)
                },
            )
        })
        .collect();
    assert_eq!(actual, expected, "{:?}", options.issues);

    // Null sections: the not-null-tolerant trio reports, language and
    // linter do not (parse_model:918/:1132 vs :547-556/:640-649/:352-361).
    let nulls = TempDir::new("null_sections");
    nulls.write(
        "analysis_options.yaml",
        "analyzer:\n  errors:\n  exclude:\n  cannot-ignore:\n  language:\nlinter:\n",
    );
    let options = AnalysisOptions::load(
        &nulls.path("analysis_options.yaml"),
        &empty_config(),
        &nulls.0,
    );
    let mut names: Vec<&str> = options
        .issues
        .iter()
        .map(|issue| issue.message.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "Invalid format for the 'cannot-ignore' section.",
            "Invalid format for the 'errors' section.",
            "Invalid format for the 'exclude' section.",
        ]
    );
}

/// The `undefined_lint` candidate recording (`_parseLocalRules`,
/// parse_model:1224-1378): list entries and map keys declare; a `name:`
/// entry with a null value returns before the registry check (:1252-1253),
/// group maps are not descended (the local parse validates top-level
/// entries only), and non-string list entries are skipped (:1238-1239).
#[test]
fn declared_rule_names_record_for_the_registry_seam() {
    let dir = TempDir::new("declared");
    dir.write(
        "list.yaml",
        "linter:\n  rules:\n    - known_one\n    - unknown_one\n    - 42\n",
    );
    dir.write(
        "map.yaml",
        "linter:\n  rules:\n    unknown_two: false\n    null_rule:\n    group_key:\n      inner: true\n",
    );

    let list = AnalysisOptions::load(&dir.path("list.yaml"), &empty_config(), &dir.0);
    let names: Vec<&str> = list
        .declared_rules
        .iter()
        .map(|rule| rule.name.as_str())
        .collect();
    assert_eq!(names, ["known_one", "unknown_one"]);

    let map = AnalysisOptions::load(&dir.path("map.yaml"), &empty_config(), &dir.0);
    let names: Vec<&str> = map
        .declared_rules
        .iter()
        .map(|rule| rule.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["unknown_two", "group_key"],
        "null values return before the check (parse_model:1252-1253); a group value is \
         not null, so its key checks too — only its children are not descended"
    );
    // The anchor span points at the name's first occurrence.
    assert_eq!(map.declared_rules[0].offset, 21);
    assert_eq!(map.declared_rules[0].length, 11);
}

/// The engine output face: `Engine::analyze` appends the recorded rows,
/// attributed to the options file itself, at the registry severity, with
/// the file's positions resolvable through the engine's read faces — and
/// the rows ride the severity face, so an `analyzer.errors` override on the
/// code name filters it (`computeSeverity` parity, `error_severity.dart`
/// :14-26).
#[test]
fn engine_analyze_emits_and_relevels_options_rows() {
    use common::Fixture;
    use dartq_diagnostics::Severity;

    // Row emission: a broken include surfaces through analyze(), attributed
    // to the options file with real positions.
    let fixture = Fixture::new(&[("lib/keep.dart", "void keep() {}\n")]);
    std::fs::write(
        fixture.path("analysis_options.yaml"),
        "include: missing.yaml\nlinter:\n  rules:\n    - avoid_empty_else\n",
    )
    .expect("options");
    let mut engine = fixture.engine();
    engine.discover_workspace().expect("discovery");
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &dartq_engine::CancellationToken::new())
        .expect("analyze");
    let options_path = fixture.path("analysis_options.yaml");
    let rows: Vec<(&str, &Severity)> = diagnostics
        .iter()
        .map(|d| (d.code.name, &d.severity))
        .collect();
    assert_eq!(
        rows,
        vec![("include_file_not_found", &Severity::Warning)],
        "{rows:?}"
    );
    let row = diagnostics
        .iter()
        .find(|d| d.code.name == "include_file_not_found")
        .expect("row present");
    let file = engine
        .file_table()
        .try_file_id(&options_path)
        .expect("options file interned");
    assert_eq!(row.file, file);
    assert_eq!(row.range.offset, 9);
    assert_eq!(row.range.length, 12);
    // The layout faces resolve the options file even though it is never
    // resident as Dart.
    assert_eq!(
        engine.content(file),
        Some("include: missing.yaml\nlinter:\n  rules:\n    - avoid_empty_else\n".as_bytes())
    );
    assert_eq!(
        engine.line_index(file).map(|i| i.line_col_utf16(
            std::str::from_utf8(engine.content(file).expect("content")).expect("utf8"),
            9
        )),
        Some((1, 10))
    );

    // The severity face: the code's own override filters the row.
    let filtered = Fixture::new(&[("lib/keep.dart", "void keep() {}\n")]);
    std::fs::write(
        filtered.path("analysis_options.yaml"),
        "include: missing.yaml\nanalyzer:\n  errors:\n    include_file_not_found: ignore\n",
    )
    .expect("options");
    let mut engine = filtered.engine();
    engine.discover_workspace().expect("discovery");
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &dartq_engine::CancellationToken::new())
        .expect("analyze");
    assert!(
        diagnostics
            .iter()
            .all(|d| d.code.name != "include_file_not_found"),
        "the override filters the options row: {diagnostics:?}"
    );
}

// --- enable-experiment (#588) ---

/// The `enable-experiment` face (`_ParsedEnableExperimentsData.parse`,
/// parse_model:470-536 + `validateFlags`, experiments_impl.dart:183-222):
/// the flag list surfaces set-if-present across the include chain (a nearer
/// list replaces a farther one whole, parser:207-215), and the rows pin the
/// validation — unknown flag → `unsupported_option` (without values),
/// expired flag → `invalid_option` (request matching the hardcoded state is
/// "no longer required", contradicting it is "unconditionally enabled"),
/// an opposite re-request → the conflict message, non-string entries and
/// non-list sections → `invalid_section_format`.
#[test]
#[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
fn experiments_parse_merge_and_validate_rows() {
    let dir = TempDir::new("experiments");
    dir.write(
        "far.yaml",
        "analyzer:\n  enable-experiment: [data-assets, patterns]\n",
    );
    dir.write(
        "analysis_options.yaml",
        concat!(
            "include: far.yaml\n",
            "analyzer:\n",
            "  enable-experiment:\n",
            "    - variance\n",
            "    - no-wildcard-variables\n",
            "    - triple-shift\n",
            "    - not-a-real-flag\n",
            "    - 42\n",
        ),
    );

    let options =
        AnalysisOptions::load(&dir.path("analysis_options.yaml"), &empty_config(), &dir.0);

    // The nearer list replaces the farther one whole (no union).
    assert_eq!(
        options.experiments,
        Some(vec![
            "variance".to_owned(),
            "no-wildcard-variables".to_owned(),
            "triple-shift".to_owned(),
            "not-a-real-flag".to_owned(),
        ]),
        "string entries only, declaration order: {:?}",
        options.experiments
    );

    let actual: Vec<(&str, String)> = options
        .issues
        .iter()
        .map(|issue| (issue.code.name, issue.message.clone()))
        .collect();
    let far = dir.path("far.yaml").display().to_string();
    assert_eq!(
        actual,
        [
            // The included file's own expired flag wraps first (the walker
            // reports includes before the initial file's own rows): inner
            // anchor `patterns` at 45..52 inclusive in far.yaml.
            (
                "included_file_warning",
                format!(
                    "Warning in the included options file {far}(45..52): Invalid option \
                     specified for 'enable-experiment': Flag \"patterns\" is no longer required."
                )
            ),
            (
                "invalid_section_format",
                "Invalid format for the 'enable-experiment' section.".to_owned()
            ),
            (
                "invalid_option",
                "Invalid option specified for 'enable-experiment': Flag \
                 \"no-wildcard-variables\" was supplied, but the feature is already \
                 unconditionally enabled."
                    .to_owned()
            ),
            (
                "invalid_option",
                "Invalid option specified for 'enable-experiment': Flag \
                 \"triple-shift\" is no longer required."
                    .to_owned()
            ),
            (
                "unsupported_option",
                "The option 'not-a-real-flag' isn't supported by 'enable-experiment'.".to_owned()
            ),
        ],
        "row order follows the file: the non-string entry, then validateFlags \
         in flag order: {:?}",
        options.issues
    );
    // The without-values correction.
    assert_eq!(
        options
            .issues
            .iter()
            .find(|issue| issue.code.name == "unsupported_option")
            .and_then(|issue| issue.correction.as_deref()),
        Some("Try removing the option.")
    );

    // An opposite re-request reports the conflict; a same-value repeat and
    // a non-list section are pinned too.
    let conflict = TempDir::new("experiment_conflict");
    conflict.write(
        "analysis_options.yaml",
        "analyzer:\n  enable-experiment: [variance, no-variance, variance]\n",
    );
    let options = AnalysisOptions::load(
        &conflict.path("analysis_options.yaml"),
        &empty_config(),
        &conflict.0,
    );
    assert_eq!(
        options
            .issues
            .iter()
            .map(|issue| issue.message.as_str())
            .collect::<Vec<_>>(),
        ["Invalid option specified for 'enable-experiment': Flag \
         \"no-variance\" conflicts with previous flag \"variance\""],
        "the opposite re-request conflicts; the repeat does not: {:?}",
        options.issues
    );

    let scalar = TempDir::new("experiment_scalar");
    scalar.write(
        "analysis_options.yaml",
        "analyzer:\n  enable-experiment: variance\n",
    );
    let options = AnalysisOptions::load(
        &scalar.path("analysis_options.yaml"),
        &empty_config(),
        &scalar.0,
    );
    assert_eq!(
        options
            .issues
            .iter()
            .map(|issue| issue.code.name)
            .collect::<Vec<_>>(),
        ["invalid_section_format"],
        "a non-list section is invalid and nothing applies"
    );
    assert_eq!(options.experiments, None);
}

// --- Nested options directories (#588) ---

/// Nested options files govern their own subtrees end-to-end: the nearest
/// governing entry wins severity attribution (the deepest mapped ancestor,
/// context_builder.dart:220-224), its excludes prune its subtree during
/// discovery, and a file outside the subtree stays under the root's entry.
#[test]
fn nested_options_govern_their_subtree() {
    use common::Fixture;

    let fixture = Fixture::new(&[
        ("lib/main.dart", "void main() {\n  var x = unknown;\n}\n"),
        (
            "lib/sub/late.dart",
            "void delayed() {\n  var y = unknown;\n}\n",
        ),
        // Invalid syntax: if the nested exclusion failed, this would flood
        // the output with parse rows.
        ("lib/sub/hidden/gen.dart", "void broken() {\n"),
    ]);
    std::fs::write(
        fixture.path("analysis_options.yaml"),
        "analyzer:\n  errors:\n    undefined_identifier: warning\n",
    )
    .expect("root options");
    std::fs::write(
        fixture.path("lib/sub/analysis_options.yaml"),
        "analyzer:\n  errors:\n    undefined_identifier: error\n  exclude:\n    - \"hidden/**\"\n",
    )
    .expect("nested options");

    let mut engine = fixture.engine();
    let stats = engine.discover_workspace().expect("discovery");
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &dartq_engine::CancellationToken::new())
        .expect("analyze");

    assert_eq!(
        stats.file_count, 2,
        "the nested exclude prunes hidden/: {stats:?}"
    );
    assert!(
        engine
            .file_table()
            .try_file_id(&fixture.path("lib/sub/hidden/gen.dart"))
            .is_none(),
        "the nested glob drops the file from the analysis set"
    );

    let severity_of = |relative: &str| {
        let file = engine
            .file_table()
            .try_file_id(&fixture.path(relative))
            .expect("interned");
        diagnostics
            .iter()
            .find(|d| d.file == file && d.code.name == "undefined_identifier")
            .map(|d| d.severity)
            .expect("the row exists")
    };
    // main.dart sits under the root entry only: the root's `warning` holds.
    assert_eq!(
        severity_of("lib/main.dart"),
        dartq_diagnostics::Severity::Warning
    );
    // late.dart sits under both entries: the deepest (nested `error`) wins.
    assert_eq!(
        severity_of("lib/sub/late.dart"),
        dartq_diagnostics::Severity::Error
    );
}

/// Nested options files are independent include chains: without an explicit
/// `include:`, a nested file contributes nothing to the root entry's merge
/// and inherits nothing from it (each mapped file is parsed on its own,
/// context_builder.dart:196-206) — chaining happens only through `include:`.
#[test]
fn nested_options_are_independent_chains() {
    let dir = TempDir::new("nested_chains");
    dir.write(
        "analysis_options.yaml",
        "linter:\n  rules:\n    - alpha_rule\n",
    );
    dir.write(
        "lib/sub/analysis_options.yaml",
        "linter:\n  rules:\n    - beta_rule\n",
    );

    let per_root = AnalysisOptions::for_roots(&[dir.path("")], &empty_config(), &dir.0);

    let root = per_root.get(&dir.path("")).expect("root entry");
    assert_eq!(root.lint_rules, ["alpha_rule"]);
    let nested = per_root.get(&dir.path("lib/sub")).expect("nested entry");
    assert_eq!(nested.lint_rules, ["beta_rule"]);
}

/// The fallback base without any options file in the ancestry: the
/// synthesized default — the official `AnalysisOptionsImpl()` defaults
/// (no lints, strictness unset, no excludes, no overrides). The official
/// SDK default options file exists only for Blaze workspaces
/// (`WorkspaceWithDefaultAnalysisOptions`, workspace.dart:146; blaze.dart
/// :183) — a pub workspace never mounts one, so the defaults are the base.
#[test]
fn sdk_default_options_are_the_fallback_base() {
    let dir = TempDir::new("defaults");
    let per_root = AnalysisOptions::for_roots(&[dir.path("pkg")], &empty_config(), &dir.0);

    let options = per_root.get(&dir.path("pkg")).expect("default entry");
    assert!(options.lint_rules.is_empty());
    assert_eq!(options.strict_casts, None);
    assert_eq!(options.strict_inference, None);
    assert_eq!(options.strict_raw_types, None);
    assert_eq!(options.experiments, None);
    assert!(options.exclude_globs().is_empty());
    assert!(options.issues.is_empty());
    assert!(options.file_path.as_os_str().is_empty());
    // No overrides: every declared severity stands, every code ignorable.
    assert_eq!(
        options.processed_severity("dead_code", dartq_diagnostics::Severity::Warning),
        Some(dartq_diagnostics::Severity::Warning)
    );
    assert!(options.is_ignorable("dead_code"));
}

// --- analyzer language modes → the semantic gates (#588 item 4) ---

/// The `analyzer.language.strict-*` config → engine → semantic gate face
/// end to end: `strict-raw-types: true` turns the raw generic annotation
/// row on (`strict_raw_type` for a group-less local container with an
/// unbounded parameter), and without the flag the same source stays silent
/// (the official off posture — the config is the only activation).
#[test]
fn strict_modes_flow_from_options_to_the_semantic_gate() {
    use common::Fixture;

    let source = [("lib/main.dart", "class C<T> {}\nC c = C();\n")];

    let on = Fixture::new(&source);
    std::fs::write(
        on.path("analysis_options.yaml"),
        "analyzer:\n  language:\n    strict-raw-types: true\n",
    )
    .expect("options");
    let mut engine = on.engine();
    engine.discover_workspace().expect("discovery");
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &dartq_engine::CancellationToken::new())
        .expect("analyze");
    assert!(
        diagnostics.iter().any(|d| d.code.name == "strict_raw_type"),
        "strict-raw-types turns the raw annotation row on: {diagnostics:?}"
    );

    let off = Fixture::new(&source);
    let mut engine = off.engine();
    engine.discover_workspace().expect("discovery");
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &dartq_engine::CancellationToken::new())
        .expect("analyze");
    assert!(
        diagnostics.iter().all(|d| d.code.name != "strict_raw_type"),
        "the off posture stays silent: {diagnostics:?}"
    );
}

/// The options `enable-experiment` flags reach the per-file feature gates
/// (#936, probed on the 3.13.1 oracle): a grant-free package file at the
/// SDK version lifts the `variance` parse gate and runs the semantic
/// variance check (`WRONG_TYPE_PARAMETER_VARIANCE_POSITION`, the `out T`
/// field's implicit-setter face); without the options the gate row fires
/// as always, and the same request at a pre-release package version stays
/// gated — the flags re-restrict at the file's own version, the contract
/// the whitelist grants ride.
#[test]
fn options_experiments_lift_the_variance_gates() {
    use common::Fixture;

    const VARIANCE_SRC: &str = "class Box<out T> {\n  T x;\n}\n";
    const GATE: &str = "experiment_not_enabled_off_by_default";
    const SEMANTIC: &str = "wrong_type_parameter_variance_position";

    let codes = |options: Option<&str>, language_version: &str| {
        let fixture =
            Fixture::with_language_version(&[("lib/main.dart", VARIANCE_SRC)], language_version);
        if let Some(text) = options {
            std::fs::write(fixture.path("analysis_options.yaml"), text).expect("options");
        }
        let mut engine = fixture.engine();
        engine.discover_workspace().expect("discovery");
        let diagnostics = engine
            .analyze(&[], &mut |_| {}, &dartq_engine::CancellationToken::new())
            .expect("analyze");
        let codes: Vec<String> = diagnostics.iter().map(|d| d.code.name.to_owned()).collect();
        std::fs::remove_dir_all(&fixture.root).ok();
        codes
    };

    let off = codes(None, "3.13");
    assert!(
        off.iter().any(|name| name == GATE),
        "no options: the parse gate fires: {off:?}"
    );

    let on = codes(Some("analyzer:\n  enable-experiment: [variance]\n"), "3.13");
    assert!(
        on.iter().all(|name| name != GATE),
        "the options lift the parse gate: {on:?}"
    );
    assert!(
        on.iter().any(|name| name == SEMANTIC),
        "the semantic variance check runs: {on:?}"
    );

    let old = codes(Some("analyzer:\n  enable-experiment: [variance]\n"), "3.5");
    assert!(
        old.iter().any(|name| name == GATE),
        "the flags re-restrict at the file's version: {old:?}"
    );
}

/// The `unrecognized_error_code` membership universe (#936): the pinned
/// diagnostic-name table answers for the registry codes, the removed-code
/// compat set keeps the retired names accepted
/// (`removedCodeNames`, parse_model:66-76), and unknown names miss.
#[test]
fn the_diagnostic_name_universe_answers_membership() {
    use dartq_engine::analysis_options::is_known_diagnostic_code;

    assert!(is_known_diagnostic_code("unrecognized_error_code"));
    assert!(is_known_diagnostic_code("UNDEFINED_IDENTIFIER"));
    assert!(is_known_diagnostic_code("missing_return"));
    assert!(is_known_diagnostic_code("undefined_annotation"));
    assert!(!is_known_diagnostic_code("not_a_real_code"));
}

/// Membership compares the `analyzer.errors` key against the registry's
/// `name` field lowercased (`codeNames` is built from
/// `code.lowerCaseName.toUpperCase()`, parse_model:56-59) — NOT the
/// `uniqueName`. The two spellings diverge on 225 registry entries; these
/// pins lock both directions of the difference.
#[test]
fn the_diagnostic_name_universe_matches_on_name_not_unique_name() {
    use dartq_engine::analysis_options::is_known_diagnostic_code;

    // `argumentMustBeAConstant`'s registry `name` is
    // `argument_must_be_a_constant`; its `uniqueName` drops the second
    // underscore (`argument_must_be_aconstant`). The oracle accepts only the
    // `name` spelling.
    assert!(is_known_diagnostic_code("argument_must_be_a_constant"));
    assert!(!is_known_diagnostic_code("argument_must_be_aconstant"));

    // Name-sharing families: several registry codes carry one `name` under
    // distinct `uniqueName`s. `undefined_super_member` is the shared `name`
    // of `undefinedSuper{Getter,Method,Operator,Setter}` — the shared name
    // resolves, the uniqueName spellings do not.
    assert!(is_known_diagnostic_code("undefined_super_member"));
    assert!(!is_known_diagnostic_code("undefined_super_getter"));
    assert!(!is_known_diagnostic_code("undefined_super_method"));

    // The universe is the name column of the severity table, so a name that
    // exists only as a uniqueName-derived spelling of a differently-named
    // code misses too (`deadCodeLateWildcardVariableInitializer` carries
    // `name: dead_code`).
    assert!(is_known_diagnostic_code("dead_code"));
    assert!(!is_known_diagnostic_code(
        "dead_code_late_wildcard_variable_initializer"
    ));
}

/// The ignore-citation faces' second leg (#2131): a citation matches a
/// diagnostic by the registry `name` OR the `uniqueName`
/// (`IgnoredDiagnosticName._matches`, ignore_info.dart:39-52), so the
/// uniqueName spellings resolve through [`canonical_diagnostic_name`] while
/// staying out of the `analyzer.errors` universe. The match shape a file
/// rides is `is_known_diagnostic_code(citation) ||
/// canonical_diagnostic_name(citation).is_some()` — name-spelling
/// citations answer on the first leg, uniqueName-spelling citations on the
/// second, and the two legs never disagree about a dead name.
#[test]
fn the_unique_name_spelling_resolves_for_citation_matching() {
    use dartq_engine::analysis_options::{canonical_diagnostic_name, is_known_diagnostic_code};

    // The dispatch's case: the second `invalid_null_aware_operator`
    // constant.
    assert_eq!(
        canonical_diagnostic_name("invalid_null_aware_operator_after_short_circuit"),
        Some("invalid_null_aware_operator")
    );
    // Documented name/uniqueName divergences resolve too.
    assert_eq!(
        canonical_diagnostic_name("dead_code_late_wildcard_variable_initializer"),
        Some("dead_code")
    );
    assert_eq!(
        canonical_diagnostic_name("undefined_super_getter"),
        Some("undefined_super_member")
    );
    // The match is case-insensitive (the citation match lowercases,
    // `IgnoredDiagnosticName`'s constructor).
    assert_eq!(
        canonical_diagnostic_name("INVALID_NULL_AWARE_OPERATOR_AFTER_SHORT_CIRCUIT"),
        Some("invalid_null_aware_operator")
    );
    // Unknown and name-field spellings name nothing through this face.
    assert_eq!(canonical_diagnostic_name("not_a_real_code"), None);
    assert_eq!(canonical_diagnostic_name("invalid_null_aware_operator"), None);
    // The joined admission never fires on a dead name.
    let admitted = |citation: &str| {
        is_known_diagnostic_code(citation) || canonical_diagnostic_name(citation).is_some()
    };
    assert!(admitted("invalid_null_aware_operator_after_short_circuit"));
    assert!(admitted("invalid_null_aware_operator"));
    assert!(!admitted("not_a_real_code"));
}
/// The YAML failure faces (#938, machine-face posture re-adjudicated by the
/// #1589 five-form oracle probe and landed #2002): the initial file's broken
/// YAML is machine-face SILENT — the oracle emits no diagnostic row for a
/// malformed options file on any output face — so the failure never joins
/// `issues`; it is kept as data in `parse_failure` (same geometry, consumed
/// by the faces' stderr advisory, `options_failure_notice` in
/// dartq-commands::analyze) while the kept shape stays the probe's
/// parse-error geometry: the raw failure point as a zero-length span, the
/// message the located error text with its ` at line L column C` suffix
/// stripped. An included file's failure still reports
/// `included_file_parse_error` (`{message} in {file}({start}..{end})`,
/// walker :48-66) at the branch anchor — a probed-as-emitted code.
/// `serde_yaml` yields one failure point, so the wrapped span shows it at
/// both ends. This harness observes only the loaded struct, so the advisory
/// itself is pinned at the commands layer; here the zero-row contract and
/// the kept data shape are the pins.
#[test]
fn yaml_failures_report_parse_error_faces() {
    let dir = TempDir::new("parse_error");
    let broken = "linter:\n  rules:\n    - [\n";
    dir.write("analysis_options.yaml", broken);

    let options =
        AnalysisOptions::load(&dir.path("analysis_options.yaml"), &empty_config(), &dir.0);
    assert!(
        options.issues.is_empty(),
        "the oracle emits no machine row for a malformed options file: {:?}",
        options.issues
    );
    let issue = options
        .parse_failure
        .as_ref()
        .expect("the failure is kept as data for the stderr advisory");
    assert_eq!(issue.code.name, "parse_error");
    assert_eq!(issue.code.machine, "PARSE_ERROR");
    assert_eq!(
        issue.code.dtype,
        dartq_diagnostics::DiagnosticType::CompileTimeError
    );
    assert!(issue.code.has_published_docs);
    // The message is `serde_yaml`'s verbatim (its phrasing embeds the
    // location differently per failure kind; the row span carries the
    // position the oracle face reports). This failure points at the end of
    // the input: byte offset 25 = UTF-16 25.
    assert_eq!(
        issue.message,
        "did not find expected node content at line 4 column 1, while parsing a flow node"
    );
    assert_eq!(issue.offset, 25);
    assert_eq!(issue.length, 0, "the failure point, not a range");
    assert!(issue.correction.is_none());

    // The included variant wraps at the branch anchor.
    let near = TempDir::new("included_parse_error");
    near.write("near.yaml", "include: far.yaml\n");
    near.write("far.yaml", "linter:\n  rules:\n    - [\n");
    let options = AnalysisOptions::load(&near.path("near.yaml"), &empty_config(), &near.0);
    assert_eq!(options.issues.len(), 1, "{:?}", options.issues);
    let issue = &options.issues[0];
    assert_eq!(issue.code.name, "included_file_parse_error");
    assert_eq!(issue.code.machine, "INCLUDED_FILE_PARSE_ERROR");
    assert_eq!(
        issue.code.dtype,
        dartq_diagnostics::DiagnosticType::CompileTimeError
    );
    let far = near.path("far.yaml").display().to_string();
    // `{message} in {file}({at}..{at})` — same broken text, same failure
    // point (25).
    assert!(
        issue
            .message
            .starts_with("did not find expected node content")
            && issue.message.ends_with(&format!(" in {far}(25..25)")),
        "{:?}",
        issue.message
    );
    assert_eq!(issue.offset, 9, "the branch's first include directive");
    assert_eq!(issue.length, 8, "the far.yaml URI token");
}

/// The top-level `plugins:` section parses under the pinned 3.15 model
/// (`_ParsedPluginsData`, upstream Dart SDK source,
/// `analysis_options_parse_model.dart`:1529-1837 — VALIDATION ONLY: dartq
/// loads no plugins, so only the reported rows matter): a well-formed
/// section (`name: {version|git|path|hosted|diagnostics}` or a bare version
/// string) is row-free, a non-map section reports `invalid_section_format`,
/// an unknown plugin-map key reports `unsupported_option` under
/// `plugins/<name>`, `diagnostics` values validate against
/// `errorProcessorLegalValues`, a non-string `git` scalar or non-map `git`
/// reports under `plugins/<name>/git`, and a non-map
/// `dependency_overrides` reports under `plugins/dependency_overrides`.
#[test]
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn plugins_section_reports_the_315_faces() {
    let dir = TempDir::new("plugins315");

    // Well-formed: a bare version string, a `version` map, a `git` map with
    // legal keys, and a valid `diagnostics` map — zero rows.
    dir.write(
        "ok.yaml",
        concat!(
            "plugins:\n",
            "  plain: 1.0.0\n",
            "  mapped:\n",
            "    version: ^2.0.0\n",
            "    hosted: https://pub.example\n",
            "  from_git:\n",
            "    git:\n",
            "      url: https://example.com/repo.git\n",
            "      ref: main\n",
            "      path: sub/dir\n",
            "      tag_pattern: v{{version}}\n",
            "  diagged:\n",
            "    diagnostics:\n",
            "      some_code: error\n",
            "      other: ignore\n",
            "  dependency_overrides:\n",
            "    shared:\n",
            "      path: ../shared\n",
        ),
    );
    let options = AnalysisOptions::load(&dir.path("ok.yaml"), &empty_config(), &dir.0);
    assert!(options.issues.is_empty(), "{:?}", options.issues);

    // A non-map section reports `invalid_section_format` on `plugins` (a
    // bare `plugins:` null stays silent).
    dir.write("scalar.yaml", "plugins: scalar\n");
    let scalar = AnalysisOptions::load(&dir.path("scalar.yaml"), &empty_config(), &dir.0);
    assert_eq!(scalar.issues.len(), 1, "{:?}", scalar.issues);
    assert_eq!(scalar.issues[0].code.name, "invalid_section_format");
    assert_eq!(
        scalar.issues[0].message,
        "Invalid format for the 'plugins' section."
    );
    // The row anchors at the *value* node (`scalar`), not the section key —
    // the upstream `plugins.span` face (the pinned probe's anchor).
    assert_eq!(scalar.issues[0].offset, 9);
    assert_eq!(scalar.issues[0].length, 6);

    dir.write("null.yaml", "plugins:\n");
    let nulled = AnalysisOptions::load(&dir.path("null.yaml"), &empty_config(), &dir.0);
    assert!(nulled.issues.is_empty(), "{:?}", nulled.issues);

    // A plugin map's unknown key reports `unsupported_option` under
    // `plugins/<name>` with the pluginsOptions proposal; a non-string,
    // non-map plugin value reports `invalid_section_format` under
    // `plugins/<name>`.
    dir.write(
        "bad.yaml",
        concat!(
            "plugins:\n",
            "  some_plugin:\n",
            "    is_initialized: true\n",
            "    bogus: x\n",
            "  numbered: 42\n",
            "  listed: [1]\n",
        ),
    );
    let options = AnalysisOptions::load(&dir.path("bad.yaml"), &empty_config(), &dir.0);
    let actual: Vec<(&str, &str, Option<&str>)> = options
        .issues
        .iter()
        .map(|i| (i.code.name, i.message.as_str(), i.correction.as_deref()))
        .collect();
    assert_eq!(
        actual,
        [
            (
                "unsupported_option",
                "The option 'is_initialized' isn't supported by 'plugins/some_plugin'.",
                Some(
                    "Try using one of the supported options: 'diagnostics', 'git', 'path', \
                     'version', and 'hosted'."
                )
            ),
            (
                "unsupported_option",
                "The option 'bogus' isn't supported by 'plugins/some_plugin'.",
                Some(
                    "Try using one of the supported options: 'diagnostics', 'git', 'path', \
                     'version', and 'hosted'."
                )
            ),
            (
                "invalid_section_format",
                "Invalid format for the 'plugins/numbered' section.",
                None
            ),
            (
                "invalid_section_format",
                "Invalid format for the 'plugins/listed' section.",
                None
            ),
        ],
        "{:?}",
        options.issues
    );

    // `diagnostics` values validate against `errorProcessorLegalValues`
    // under `plugins/<name>/diagnostics`; a non-map diagnostics section is
    // `invalid_section_format`; a non-scalar diagnostics value reports and
    // stops the loop.
    dir.write(
        "diag.yaml",
        concat!(
            "plugins:\n",
            "  p:\n",
            "    diagnostics:\n",
            "      dead_code: fatal\n",
            "      other: warning\n",
        ),
    );
    let options = AnalysisOptions::load(&dir.path("diag.yaml"), &empty_config(), &dir.0);
    assert_eq!(options.issues.len(), 1, "{:?}", options.issues);
    assert_eq!(options.issues[0].code.name, "unsupported_option");
    assert_eq!(
        options.issues[0].message,
        "The option 'fatal' isn't supported by 'plugins/p/diagnostics'."
    );
    assert_eq!(
        options.issues[0].correction.as_deref(),
        Some(
            "Try using one of the supported options: 'ignore', 'false', 'true', 'false', \
             'error', 'info', and 'warning'."
        )
    );

    dir.write("diag_scalar.yaml", "plugins:\n  p:\n    diagnostics: 42\n");
    let options = AnalysisOptions::load(&dir.path("diag_scalar.yaml"), &empty_config(), &dir.0);
    assert_eq!(options.issues.len(), 1, "{:?}", options.issues);
    assert_eq!(
        options.issues[0].message,
        "Invalid format for the 'plugins/p/diagnostics' section."
    );

    // `git` faces: a non-string scalar and a list both report
    // `invalid_section_format` under `plugins/<name>/git`; an unknown git
    // key reports `unsupported_option` under the same name; a legal git
    // key with a non-string value reports under `plugins/<name>/git/<key>`.
    dir.write(
        "git.yaml",
        concat!(
            "plugins:\n",
            "  a:\n",
            "    git: 42\n",
            "  b:\n",
            "    git:\n",
            "      url: https://example.com/x.git\n",
            "      bogus: 1\n",
            "      ref: [1]\n",
        ),
    );
    let options = AnalysisOptions::load(&dir.path("git.yaml"), &empty_config(), &dir.0);
    let actual: Vec<(&str, &str)> = options
        .issues
        .iter()
        .map(|i| (i.code.name, i.message.as_str()))
        .collect();
    assert_eq!(
        actual,
        [
            (
                "invalid_section_format",
                "Invalid format for the 'plugins/a/git' section."
            ),
            (
                "unsupported_option",
                "The option 'bogus' isn't supported by 'plugins/b/git'."
            ),
            (
                "invalid_section_format",
                "Invalid format for the 'plugins/b/git/ref' section."
            ),
        ],
        "{:?}",
        options.issues
    );
    let git_correction = options
        .issues
        .iter()
        .find(|i| i.code.name == "unsupported_option")
        .and_then(|i| i.correction.as_deref());
    assert_eq!(
        git_correction,
        Some("Try using one of the supported options: 'url', 'ref', 'path', and 'tag_pattern'.")
    );

    // `dependency_overrides` must be a map; entries validate like plugin
    // entries under `plugins/dependency_overrides/<name>`.
    dir.write("deps_scalar.yaml", "plugins:\n  dependency_overrides: 42\n");
    let options = AnalysisOptions::load(&dir.path("deps_scalar.yaml"), &empty_config(), &dir.0);
    assert_eq!(options.issues.len(), 1, "{:?}", options.issues);
    assert_eq!(
        options.issues[0].message,
        "Invalid format for the 'plugins/dependency_overrides' section."
    );
    dir.write(
        "deps_bad.yaml",
        "plugins:\n  dependency_overrides:\n    shared: [1]\n",
    );
    let options = AnalysisOptions::load(&dir.path("deps_bad.yaml"), &empty_config(), &dir.0);
    assert_eq!(options.issues.len(), 1, "{:?}", options.issues);
    assert_eq!(
        options.issues[0].message,
        "Invalid format for the 'plugins/dependency_overrides/shared' section."
    );

    // The section still contributes nothing to the merged options.
    assert!(options.lint_rules.is_empty());
}

/// The included files' registry candidates land in
/// [`dartq_engine::analysis_options::DeclarationGroup`]s — each with the
/// included file's anchors and the precomputed initial-file anchor span —
/// and the CLI face's wrap (`DeclarationGroup::wrap_row`) shapes the
/// official `included_file_warning` text over any inner row (inclusive
/// inner end, correction dropped).
#[test]
fn included_groups_carry_registry_candidates_with_wrap_anchors() {
    use dartq_engine::analysis_options::OptionsIssue;

    let dir = TempDir::new("groups");
    dir.write(
        "near.yaml",
        "include: far.yaml\nlinter:\n  rules:\n    - near_rule\n",
    );
    dir.write(
        "far.yaml",
        "linter:\n  rules:\n    - no_such_rule_here\nanalyzer:\n  errors:\n    not_a_code: ignore\n",
    );

    let options = AnalysisOptions::load(&dir.path("near.yaml"), &empty_config(), &dir.0);

    assert_eq!(
        options.included_groups.len(),
        1,
        "{:?}",
        options.included_groups
    );
    let group = &options.included_groups[0];
    assert_eq!(group.file, dir.path("far.yaml"));
    assert_eq!(group.anchor_span, (9, 8), "the branch's first directive");
    assert_eq!(group.rules.len(), 1);
    assert_eq!(group.rules[0].name, "no_such_rule_here");
    assert_eq!(group.errors.len(), 1);
    assert_eq!(group.errors[0].code, "not_a_code");

    // The wrap face over the inner undefined_lint row: inclusive inner end,
    // correction dropped, row span = the group anchor.
    let inner = OptionsIssue::undefined_lint(&group.rules[0]);
    let wrapped = group.wrap_row(&inner);
    assert_eq!(wrapped.code.name, "included_file_warning");
    let far = dir.path("far.yaml").display().to_string();
    let inner_end = inner.offset + inner.length - 1;
    assert_eq!(
        wrapped.message,
        format!(
            "Warning in the included options file {far}({}..{inner_end}): {}",
            inner.offset, inner.message
        )
    );
    assert_eq!(wrapped.correction, None);
    assert_eq!((wrapped.offset, wrapped.length), group.anchor_span);
}

/// The `cannot-ignore` names feed the engine's unignorable set (#938): the
/// plain names store lowercased (the official `IgnoreValidator` builds its
/// set lowercased and matches comment names case-sensitively), and the
/// severity-arm names contribute every registry code of that family.
#[test]
fn cannot_ignore_names_feed_the_unignorable_set() {
    let dir = TempDir::new("cannot_ignore");
    dir.write(
        "analysis_options.yaml",
        "analyzer:\n  errors:\n    unused_import: ignore\n  cannot-ignore:\n    - UNUSED_IMPORT\n    - dead_code\n",
    );

    let options =
        AnalysisOptions::load(&dir.path("analysis_options.yaml"), &empty_config(), &dir.0);

    assert!(
        options.unignorable().contains("unused_import"),
        "{:?}",
        options.unignorable()
    );
    assert!(options.unignorable().contains("dead_code"));
    assert!(
        !options.unignorable().contains("UNUSED_IMPORT"),
        "the set is lowercased"
    );
    assert!(!options.unignorable().contains("not_listed"));
}

/// The section-scoped anchor convention (#938): a declaration token that
/// also appears earlier in the file (an include URI) anchors at its
/// occurrence inside its own section, not at the first text hit.
#[test]
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn declaration_anchors_scope_to_their_section() {
    let dir = TempDir::new("scoped");
    dir.write(
        "analysis_options.yaml",
        "include: nope_rule.yaml\nlinter:\n  rules:\n    - nope_rule\n",
    );

    let options =
        AnalysisOptions::load(&dir.path("analysis_options.yaml"), &empty_config(), &dir.0);

    let uri_len = "nope_rule.yaml".chars().map(char::len_utf16).sum::<usize>() as u32;
    assert_eq!(options.declared_rules.len(), 1);
    assert_eq!(options.declared_rules[0].name, "nope_rule");
    // `include: nope_rule.yaml\n` = 24 units, `linter:\n` = 8, `  rules:\n` = 9,
    // `    - ` = 6 — the rules-section occurrence starts at 47.
    assert_eq!(options.declared_rules[0].offset, 47);
    assert_eq!(
        options.declared_rules[0].length,
        "nope_rule".chars().map(char::len_utf16).sum::<usize>() as u32
    );
    assert_eq!(uri_len, 14);
}

/// G-5: lint rule names lowercase at the section boundary
/// (`parseLinterSection`, lint/config.dart:86-90 — every entry key
/// lowercases; the registry lookups downstream match case-insensitively, so
/// a mixed-case spelling resolves the snake_case rule). The merge form
/// recurses exactly one level (config.dart:92-103 — a deeper map drops, it
/// does not descend); the declared recording has no descent at all (the
/// parse model visits top-level entries only), so group keys record while
/// their children never do. The recorded declaration names keep their
/// original spellings: the rows name the written form.
#[test]
fn rule_names_lowercase_at_the_section_boundary() {
    let dir = TempDir::new("rule_case");
    dir.write(
        "analysis_options.yaml",
        concat!(
            "linter:\n",
            "  rules:\n",
            "    Alpha_Rule: true\n",
            "    Avoid_Print: false\n",
            "    style:\n",
            "      Beta_Rule: true\n",
            "    deep:\n",
            "      middle:\n",
            "        Gamma_Rule: true\n",
        ),
    );

    let options =
        AnalysisOptions::load(&dir.path("analysis_options.yaml"), &empty_config(), &dir.0);

    // The mixed-case spellings merge lowercased; the group entry enables
    // one level deep; the group-of-group contributes nothing.
    assert_eq!(options.lint_rules, ["alpha_rule", "beta_rule"]);

    let names: Vec<&str> = options
        .declared_rules
        .iter()
        .map(|rule| rule.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["Alpha_Rule", "Avoid_Print", "style", "deep"],
        "top-level entries only: the group keys record (a non-null value \
         reaches the seam), their children are never descended — the \
         one-level descent lives on the merge side only",
    );
}

/// G-6: the value facts the registry-gated rows need, recorded per
/// declaration (`parseRule`, parse_model.dart:1266-1283 — the engine holds
/// no lint registry, so the emission seam is the CLI face's): `enabled`
/// follows the parse model's value processing, `illegal_value` carries the
/// scalar text of a value that is neither a bool nor a legal severity
/// string (`ignore`/`info`/`warning`/`error`, case-sensitive), and a null
/// value returns before the registry check (:1252-1253) so it is not
/// recorded at all. The merge side legitimately differs on `ignore`:
/// `ConfiguredSeverity` has no `ignore` member (config.dart:116-131), so
/// the severity string enables there while the parse model disables.
#[test]
fn declared_rules_record_value_facts() {
    let dir = TempDir::new("rule_facts");
    dir.write(
        "analysis_options.yaml",
        concat!(
            "linter:\n",
            "  rules:\n",
            "    keep_on: true\n",
            "    shut_off: false\n",
            "    by_severity: warning\n",
            "    quieted: ignore\n",
            "    typed: 42\n",
            "    cased: Info\n",
            "    grouped:\n",
            "      inner: true\n",
            "    nulled:\n",
        ),
    );

    let options =
        AnalysisOptions::load(&dir.path("analysis_options.yaml"), &empty_config(), &dir.0);

    let facts: Vec<(&str, bool, Option<&str>)> = options
        .declared_rules
        .iter()
        .map(|rule| {
            (
                rule.name.as_str(),
                rule.enabled,
                rule.illegal_value.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        facts,
        [
            ("keep_on", true, None),
            ("shut_off", false, None),
            ("by_severity", true, None),
            ("quieted", false, None),
            ("typed", false, Some("42")),
            ("cased", false, Some("Info")),
            // A group map records its Dart `toString` text for the
            // `unsupported_value` row (#1793).
            ("grouped", false, Some("{inner: true}")),
        ],
        "{:?}",
        options.declared_rules
    );

    // The merge side (config.dart:83-104) only recognizes the exact
    // `disable` spelling: every severity string enables, and the same
    // `?? ConfiguredSeverity.enable` fallback that turns the parse model's
    // disabled `ignore` on turns the case-variant `Info` on too. The group
    // form parses its scalar children one level deep, so `inner: true`
    // enables; the `typed`/`nulled` scalars contribute nothing.
    assert_eq!(
        options.lint_rules,
        ["by_severity", "cased", "inner", "keep_on", "quieted"]
    );
}

/// G-8: the `analyzer.strong-mode` shape rows (`_ParsedStrongModeData.parse`,
/// parse_model:1841-1911) — the section is NOT null-tolerant (only an absent
/// key is silent), only `implicit-casts`/`implicit-dynamic` parse (the
/// legacy `declaration-casts` reports too), a container value is an
/// `invalid_section_format` sectioned by the option name, a non-boolean
/// scalar reports `unsupported_value`, and the unknown-key row proposes the
/// two legal keys. The section parses rows only: no strictness scalar
/// merges.
#[test]
fn strong_mode_shapes_report_rows() {
    let dir = TempDir::new("strong_mode");
    dir.write("sm_scalar.yaml", "analyzer:\n  strong-mode: 42\n");
    dir.write(
        "sm_map.yaml",
        concat!(
            "analyzer:\n",
            "  strong-mode:\n",
            "    implicit-casts: 42\n",
            "    implicit-dynamic: true\n",
            "    declaration-casts: false\n",
            "    bogus: 1\n",
        ),
    );
    dir.write("sm_null.yaml", "analyzer:\n  strong-mode:\n");

    let scalar = AnalysisOptions::load(&dir.path("sm_scalar.yaml"), &empty_config(), &dir.0);
    assert_eq!(scalar.issues.len(), 1, "{:?}", scalar.issues);
    assert_eq!(scalar.issues[0].code.name, "invalid_section_format");
    assert_eq!(
        scalar.issues[0].message,
        "Invalid format for the 'strong-mode' section."
    );

    let mapped = AnalysisOptions::load(&dir.path("sm_map.yaml"), &empty_config(), &dir.0);
    assert_eq!(mapped.issues.len(), 3, "{:?}", mapped.issues);
    let value = &mapped.issues[0];
    assert_eq!(value.code.name, "unsupported_value");
    assert_eq!(
        value.message,
        "The value '42' isn't supported by 'implicit-casts'."
    );
    assert_eq!(
        value.correction.as_deref(),
        Some("Try using one of the supported values: 'true' and 'false'.")
    );
    // `analyzer:\n` = 10, `  strong-mode:\n` = 15, 4 spaces = 4, key 14 + ': '
    // — the value token sits at 45.
    assert_eq!((value.offset, value.length), (45, 2));
    for issue in &mapped.issues[1..] {
        assert_eq!(issue.code.name, "unsupported_option");
        assert_eq!(
            issue.correction.as_deref(),
            Some(
                "Try using one of the supported options: 'implicit-casts' and \
                 'implicit-dynamic'."
            )
        );
    }
    assert_eq!(
        mapped.issues[1].message,
        "The option 'declaration-casts' isn't supported by 'strong-mode'."
    );
    assert_eq!(
        mapped.issues[2].message,
        "The option 'bogus' isn't supported by 'strong-mode'."
    );

    // A bare `strong-mode:` is a scalar node upstream (`strongMode == null`
    // tests only absence) — the row fires.
    let nulled = AnalysisOptions::load(&dir.path("sm_null.yaml"), &empty_config(), &dir.0);
    assert_eq!(nulled.issues.len(), 1, "{:?}", nulled.issues);
    assert_eq!(
        nulled.issues[0].message,
        "Invalid format for the 'strong-mode' section."
    );

    // Rows only: nothing merges into the strictness scalars.
    assert_eq!(mapped.strict_casts, None);
    assert_eq!(mapped.strict_inference, None);
    assert_eq!(mapped.strict_raw_types, None);
}

/// G-8: the `analyzer.optional-checks` shape rows
/// (`_ParsedOptionalChecksData.parse`, parse_model:1399-1499) — the map
/// form's parseBool arms mirror strong-mode, the unknown-key row is the odd
/// face whose section-name argument is the legal-key list itself
/// (`reportUnsupportedKey` :1423-1433, so the message carries doubled
/// quotes verbatim), the scalar form enables the legal names silently while
/// any other scalar — a bare null rendering `null` — reports, and a
/// sequence is an invalid section.
#[test]
fn optional_checks_shapes_report_rows() {
    let dir = TempDir::new("optional_checks");
    dir.write(
        "oc_map.yaml",
        concat!(
            "analyzer:\n",
            "  optional-checks:\n",
            "    chrome-os-manifest-checks: 42\n",
            "    propagate-linter-exceptions:\n",
            "      nested: true\n",
            "    bogus-key: true\n",
        ),
    );
    dir.write(
        "oc_scalar.yaml",
        "analyzer:\n  optional-checks: chrome-os-manifest-checks\n",
    );
    dir.write("oc_unknown.yaml", "analyzer:\n  optional-checks: bogus\n");
    dir.write("oc_bare.yaml", "analyzer:\n  optional-checks:\n");
    dir.write("oc_seq.yaml", "analyzer:\n  optional-checks: [a]\n");

    let odd_message = "The option 'bogus-key' isn't supported by \
''chrome-os-manifest-checks' or 'propagate-linter-exceptions'.";
    let odd_correction = "Try using one of the supported options: \
'chrome-os-manifest-checks' and 'propagate-linter-exceptions'.";

    let mapped = AnalysisOptions::load(&dir.path("oc_map.yaml"), &empty_config(), &dir.0);
    assert_eq!(mapped.issues.len(), 3, "{:?}", mapped.issues);
    let value = &mapped.issues[0];
    assert_eq!(value.code.name, "unsupported_value");
    assert_eq!(
        value.message,
        "The value '42' isn't supported by 'chrome-os-manifest-checks'."
    );
    // `analyzer:\n` = 10, `  optional-checks:\n` = 19, 4 spaces, key 25 + ': '
    // — the value token sits at 60.
    assert_eq!((value.offset, value.length), (60, 2));
    assert_eq!(
        mapped.issues[1].code.name, "invalid_section_format",
        "{:?}",
        mapped.issues[1]
    );
    assert_eq!(
        mapped.issues[1].message,
        "Invalid format for the 'propagate-linter-exceptions' section."
    );
    assert_eq!(mapped.issues[2].message, odd_message);
    assert_eq!(mapped.issues[2].correction.as_deref(), Some(odd_correction));

    // The legal scalar name enables silently.
    let scalar = AnalysisOptions::load(&dir.path("oc_scalar.yaml"), &empty_config(), &dir.0);
    assert!(scalar.issues.is_empty(), "{:?}", scalar.issues);

    // Any other scalar reports — including a bare null, whose rendered
    // value is `null`.
    let unknown = AnalysisOptions::load(&dir.path("oc_unknown.yaml"), &empty_config(), &dir.0);
    assert_eq!(unknown.issues.len(), 1, "{:?}", unknown.issues);
    assert_eq!(
        unknown.issues[0].message,
        "The option 'bogus' isn't supported by ''chrome-os-manifest-checks' \
 or 'propagate-linter-exceptions'."
    );
    let bare = AnalysisOptions::load(&dir.path("oc_bare.yaml"), &empty_config(), &dir.0);
    assert_eq!(bare.issues.len(), 1, "{:?}", bare.issues);
    assert_eq!(
        bare.issues[0].message,
        "The option 'null' isn't supported by ''chrome-os-manifest-checks' \
 or 'propagate-linter-exceptions'."
    );

    // A sequence is an invalid section (the switch's default arm).
    let sequence = AnalysisOptions::load(&dir.path("oc_seq.yaml"), &empty_config(), &dir.0);
    assert_eq!(sequence.issues.len(), 1, "{:?}", sequence.issues);
    assert_eq!(
        sequence.issues[0].message,
        "Invalid format for the 'optional-checks' section."
    );
}

/// G-8: the top-level `code-style` (`_ParsedCodeStyleData.parse`,
/// parse_model:407-468) and `formatter` (`_ParsedFormatterData.parse`,
/// :813-895) shape rows — both null-tolerant, unknown keys report the
/// without-values `unsupported_option` variant, `format` runs the
/// parseBool shape, `page_width` demands a positive integer and
/// `trailing_commas` the exact `automate`/`preserve` spellings (both via
/// `invalid_option` with the verbatim detail texts), and the legal shapes
/// stay silent.
#[test]
fn code_style_and_formatter_shapes_report_rows() {
    let dir = TempDir::new("style_formatter");
    dir.write("cs_values.yaml", "code-style:\n  format: 42\n  bogus: 1\n");
    dir.write("cs_ok.yaml", "code-style:\n  format: true\n");
    dir.write("cs_shape.yaml", "code-style:\n  format: [1]\n");
    dir.write("cs_section.yaml", "code-style: 42\n");
    dir.write("cs_null.yaml", "code-style:\n");
    dir.write(
        "fmt_ok.yaml",
        "formatter:\n  page_width: 80\n  trailing_commas: preserve\n",
    );
    dir.write(
        "fmt_float.yaml",
        "formatter:\n  page_width: 3.5\n  trailing_commas: Preserve\n  unknown: 1\n",
    );
    dir.write("fmt_negative.yaml", "formatter:\n  page_width: -5\n");
    dir.write("fmt_section.yaml", "formatter: 42\n");
    dir.write("fmt_null.yaml", "formatter:\n");

    // code-style: the value face, the unknown-key face, the legal shape,
    // the container-value face, the section face, and the null tolerance.
    let values = AnalysisOptions::load(&dir.path("cs_values.yaml"), &empty_config(), &dir.0);
    assert_eq!(values.issues.len(), 2, "{:?}", values.issues);
    assert_eq!(
        values.issues[0].message,
        "The value '42' isn't supported by 'format'."
    );
    assert_eq!(
        values.issues[1].message,
        "The option 'bogus' isn't supported by 'code-style'."
    );
    assert_eq!(
        values.issues[1].correction.as_deref(),
        Some("Try removing the option.")
    );
    let ok = AnalysisOptions::load(&dir.path("cs_ok.yaml"), &empty_config(), &dir.0);
    assert!(ok.issues.is_empty(), "{:?}", ok.issues);
    let shape = AnalysisOptions::load(&dir.path("cs_shape.yaml"), &empty_config(), &dir.0);
    assert_eq!(shape.issues.len(), 1, "{:?}", shape.issues);
    assert_eq!(
        shape.issues[0].message,
        "Invalid format for the 'format' section."
    );
    let section = AnalysisOptions::load(&dir.path("cs_section.yaml"), &empty_config(), &dir.0);
    assert_eq!(
        section.issues[0].message,
        "Invalid format for the 'code-style' section."
    );
    let nulled = AnalysisOptions::load(&dir.path("cs_null.yaml"), &empty_config(), &dir.0);
    assert!(nulled.issues.is_empty(), "{:?}", nulled.issues);

    // formatter: the legal shapes, then `page_width` non-integers
    // (float and negative), the case-sensitive `trailing_commas` spellings,
    // and the unknown key.
    let good = AnalysisOptions::load(&dir.path("fmt_ok.yaml"), &empty_config(), &dir.0);
    assert!(good.issues.is_empty(), "{:?}", good.issues);
    let float = AnalysisOptions::load(&dir.path("fmt_float.yaml"), &empty_config(), &dir.0);
    assert_eq!(float.issues.len(), 3, "{:?}", float.issues);
    assert_eq!(
        float.issues[0].message,
        "Invalid option specified for 'page_width': \"page_width\" must be a \
 positive integer."
    );
    assert_eq!(float.issues[0].correction, None);
    assert_eq!(
        float.issues[1].message,
        "Invalid option specified for 'trailing_commas': \"trailing_commas\" \
 must be \"automate\" or \"preserve\"."
    );
    assert_eq!(
        float.issues[2].message,
        "The option 'unknown' isn't supported by 'formatter'."
    );
    let negative = AnalysisOptions::load(&dir.path("fmt_negative.yaml"), &empty_config(), &dir.0);
    assert_eq!(negative.issues.len(), 1, "{:?}", negative.issues);
    assert_eq!(
        negative.issues[0].message,
        "Invalid option specified for 'page_width': \"page_width\" must be a \
 positive integer."
    );
    let section = AnalysisOptions::load(&dir.path("fmt_section.yaml"), &empty_config(), &dir.0);
    assert_eq!(
        section.issues[0].message,
        "Invalid format for the 'formatter' section."
    );
    let nulled = AnalysisOptions::load(&dir.path("fmt_null.yaml"), &empty_config(), &dir.0);
    assert!(nulled.issues.is_empty(), "{:?}", nulled.issues);
}

/// G-7: the nested-options boundary. `locate_nested` walks the four
/// discovery surfaces (`lib`/`bin`/`test`/`tool`) — the same surfaces
/// discovery analyzes — so an options file under any other directory is
/// neither located nor honored, and the surface directory still registers.
/// The combined posture ("options outside the four surfaces never govern")
/// is a differential face of the discovery axis, not this module's.
#[test]
fn nested_options_outside_the_surfaces_do_not_govern() {
    let dir = TempDir::new("nested_boundary");
    dir.write(
        "analysis_options.yaml",
        "linter:\n  rules:\n    - alpha_rule\n",
    );
    dir.write(
        "example/analysis_options.yaml",
        "linter:\n  rules:\n    - example_rule\nanalyzer:\n  language:\n    strict-casts: true\n",
    );
    dir.write(
        "tool/analysis_options.yaml",
        "linter:\n  rules:\n    - tool_rule\n",
    );

    let per_root = AnalysisOptions::for_roots(&[dir.path("")], &empty_config(), &dir.0);

    // The root entry merges only its own chain: the surface file registers
    // at its own directory as an independent governing entry (each mapped
    // file is parsed on its own, context_builder.dart:196-206) — the
    // containing root takes only the nested excludes into its walk
    // predicate (context_locator.dart:402-409), never the nested rules or
    // strictness. The non-surface subtree is not even located.
    let root = per_root.get(&dir.path("")).expect("root entry");
    assert_eq!(root.lint_rules, ["alpha_rule"]);
    assert_eq!(root.strict_casts, None);
    // The non-surface directory has no governing entry at all.
    assert!(!per_root.contains_key(&dir.path("example")));
    // The surface directory does.
    assert!(per_root.contains_key(&dir.path("tool")));
}

// --- #1793: the row-divergence fix wave ---

/// The `cannot-ignore` severity arms iterate the whole pinned
/// `diagnosticCodeValues` universe (`_applyCannotIgnore`, upstream Dart SDK
/// source, `analysis_options_parser.dart`:173-197 — every diagnostic whose
/// *effective* severity equals the arm becomes unignorable), so semantic
/// codes the builtin emit table does not list (`unused_import`,
/// `dead_null_aware_expression`, …) mark too, while `error`/`warning`/`info`
/// match on exact equality — a `warning` arm never reaches an ERROR code.
/// Explicit names still join lowercased.
#[test]
fn cannot_ignore_severity_arms_cover_the_full_universe() {
    let dir = TempDir::new("cannot_ignore_universe");
    dir.write(
        "analysis_options.yaml",
        concat!("analyzer:\n", "  cannot-ignore:\n", "    - warning\n",),
    );
    dir.write("error.yaml", "analyzer:\n  cannot-ignore:\n    - error\n");

    let options =
        AnalysisOptions::load(&dir.path("analysis_options.yaml"), &empty_config(), &dir.0);
    // Semantic static warnings — codes the builtin table never listed.
    assert!(options.unignorable().contains("unused_import"));
    assert!(options.unignorable().contains("dead_null_aware_expression"));
    assert!(options.unignorable().contains("unused_local_variable"));
    // Error-tier codes are untouched by the warning arm.
    assert!(!options.unignorable().contains("undefined_identifier"));
    // A lint rule name is not in `diagnosticCodeValues` at all.
    assert!(!options.unignorable().contains("prefer_single_quotes"));

    let error = AnalysisOptions::load(&dir.path("error.yaml"), &empty_config(), &dir.0);
    assert!(error.unignorable().contains("undefined_identifier"));
    assert!(error.unignorable().contains("multiple_combinators"));
    assert!(!error.unignorable().contains("unused_import"));

    // The `errors` re-level still overrides the registry severity before the
    // arm runs (`errors` applies first, parser:166-167): a code re-leveled
    // to warning joins the warning arm, and an ignore processor filters it
    // out entirely.
    dir.write(
        "leveled.yaml",
        concat!(
            "analyzer:\n",
            "  errors:\n",
            "    undefined_identifier: warning\n",
            "    dead_code: ignore\n",
            "  cannot-ignore:\n",
            "    - warning\n",
        ),
    );
    let options = AnalysisOptions::load(&dir.path("leveled.yaml"), &empty_config(), &dir.0);
    assert!(options.unignorable().contains("undefined_identifier"));
    assert!(!options.unignorable().contains("dead_code"));
}

/// The `analyzer.plugins` key reports `analysis_options_deprecated_plugins`
/// unconditionally at the key node (upstream `_ParsedAnalyzerData.parse`,
/// `analysis_options_parse_model.dart`:285-289 — the key's mere presence
/// fires, even a bare `plugins:` null value), and an included file's row
/// wraps into `included_file_warning`.
#[test]
fn analyzer_plugins_key_reports_deprecated_plugins() {
    let dir = TempDir::new("deprecated_plugins");
    dir.write(
        "analysis_options.yaml",
        "analyzer:\n  plugins:\n    some_plugin: 1.0.0\n",
    );
    let options =
        AnalysisOptions::load(&dir.path("analysis_options.yaml"), &empty_config(), &dir.0);
    assert_eq!(options.issues.len(), 1, "{:?}", options.issues);
    let issue = &options.issues[0];
    assert_eq!(issue.code.name, "analysis_options_deprecated_plugins");
    assert_eq!(issue.code.machine, "ANALYSIS_OPTIONS_DEPRECATED_PLUGINS");
    assert_eq!(
        issue.code.dtype,
        dartq_diagnostics::DiagnosticType::StaticWarning
    );
    assert!(!issue.code.has_published_docs);
    assert_eq!(
        issue.message,
        "Support for legacy plugins is deprecated, and will be removed in an \
         upcoming version of Dart."
    );
    assert_eq!(
        issue.correction.as_deref(),
        Some(
            "See https://dart.dev/tools/analyzer-plugins for documentation regarding \
             the new analyzer plugin system."
        )
    );
    // Anchored at the `plugins` key inside `analyzer:` (offset 12).
    assert_eq!(issue.offset, 12);
    assert_eq!(issue.length, 7);

    // A bare `plugins:` null value still reports (getKey fires on the key's
    // presence).
    dir.write("null.yaml", "analyzer:\n  plugins:\n");
    let options = AnalysisOptions::load(&dir.path("null.yaml"), &empty_config(), &dir.0);
    assert_eq!(options.issues.len(), 1, "{:?}", options.issues);
    assert_eq!(
        options.issues[0].code.name,
        "analysis_options_deprecated_plugins"
    );

    // An included file's row wraps into `included_file_warning` at the
    // branch anchor, message naming the included file and the inner span.
    dir.write("near.yaml", "include: far.yaml\n");
    dir.write("far.yaml", "analyzer:\n  plugins: {}\n");
    let near = AnalysisOptions::load(&dir.path("near.yaml"), &empty_config(), &dir.0);
    assert_eq!(near.issues.len(), 1, "{:?}", near.issues);
    let wrapped = &near.issues[0];
    assert_eq!(wrapped.code.name, "included_file_warning");
    let far = dir.path("far.yaml").display().to_string();
    assert_eq!(
        wrapped.message,
        format!(
            "Warning in the included options file {far}(12..18): Support for legacy plugins \
             is deprecated, and will be removed in an upcoming version of Dart."
        )
    );
}

/// `analyzer.errors` action validity rides the full
/// `errorProcessorLegalValues` list (upstream Dart SDK source,
/// `analysis_options_parse_model.dart`:61-65,605-618): `true` is a legal
/// action — a no-op processor — so `errors: {code: true}` reports nothing
/// and changes nothing, while `false` reports nothing but filters.
#[test]
fn errors_true_is_legal_and_no_op() {
    let dir = TempDir::new("errors_true");
    dir.write(
        "analysis_options.yaml",
        concat!(
            "analyzer:\n",
            "  errors:\n",
            "    unused_import: true\n",
            "    dead_code: false\n",
            "    bogus_action_code: maybe\n",
        ),
    );
    let options =
        AnalysisOptions::load(&dir.path("analysis_options.yaml"), &empty_config(), &dir.0);

    // `true` and `false` report no row; `maybe` reports the legal-values
    // `unsupported_option`.
    assert_eq!(options.issues.len(), 1, "{:?}", options.issues);
    assert_eq!(options.issues[0].code.name, "unsupported_option");
    assert_eq!(
        options.issues[0].message,
        "The option 'maybe' isn't supported by 'errors'."
    );
    // Effect: `true` is a no-op (declared severity stands), `false` filters.
    assert_eq!(
        options.processed_severity("unused_import", dartq_diagnostics::Severity::Warning),
        Some(dartq_diagnostics::Severity::Warning)
    );
    assert_eq!(
        options.processed_severity("dead_code", dartq_diagnostics::Severity::Warning),
        None
    );
}

/// A container rule value records its Dart `toString` text for the
/// `unsupported_value` row (`parseRule`'s fall-through arm,
/// `analysis_options_parse_model.dart`:1272-1284 — `ruleValue.toString()`
/// renders the YamlMap/YamlList as `{a: b}`/`[x]`): group maps and lists
/// now carry `illegal_value` text instead of dropping the row.
#[test]
fn container_rule_values_record_the_dart_tostring_text() {
    let dir = TempDir::new("rule_containers");
    dir.write(
        "analysis_options.yaml",
        concat!(
            "linter:\n",
            "  rules:\n",
            "    grouped:\n",
            "      inner: true\n",
            "    listed: [x, 42]\n",
            "    inline_map: {a: b, c: [1]}\n",
        ),
    );

    let options =
        AnalysisOptions::load(&dir.path("analysis_options.yaml"), &empty_config(), &dir.0);

    let facts: Vec<(&str, Option<&str>)> = options
        .declared_rules
        .iter()
        .map(|rule| (rule.name.as_str(), rule.illegal_value.as_deref()))
        .collect();
    assert_eq!(
        facts,
        [
            ("grouped", Some("{inner: true}")),
            ("listed", Some("[x, 42]")),
            ("inline_map", Some("{a: b, c: [1]}")),
        ],
        "{:?}",
        options.declared_rules
    );
    // Containers still record as disabled entries.
    assert!(options.declared_rules.iter().all(|rule| !rule.enabled));
}

/// The cross-file lint-check data the CLI consumes (the official
/// `_IncludedLinterRules` walk, linter_rule_options_validator.dart:105-186):
/// the initial file's `included_edges` carries one entry per resolved
/// `include:` directive in declaration order — its written URI, the URI
/// token's span in the initial file's text, and the target's
/// [`DeclarationGroup`] index — and each included file's group folds its
/// subtree's effective rule state (`enabled_rules`/`disabled_rules`, the
/// official `_EffectiveLinterRules` pair) with nearer entries shadowing
/// farther ones per name.
#[test]
fn include_edges_and_effective_rule_sets_track_the_chain() {
    let dir = TempDir::new("xlint");
    // The pair-fires shape: `a.yaml` enables `prefer_double_quotes`,
    // `b.yaml` enables `prefer_single_quotes` — the initial file's two
    // include edges carry both, in declaration order.
    let near = concat!(
        "include:\n",
        "  - a.yaml\n",
        "  - b.yaml\n",
        "linter:\n",
        "  rules:\n",
        "    - avoid_print\n",
    );
    dir.write("near.yaml", near);
    let a = "linter:\n  rules:\n    - prefer_double_quotes\n";
    dir.write("a.yaml", a);
    let b = "linter:\n  rules:\n    - prefer_single_quotes\n";
    dir.write("b.yaml", b);

    let options = AnalysisOptions::load(&dir.path("near.yaml"), &empty_config(), &dir.0);

    // One edge per include entry, in declaration order — the URI text and
    // the token's span in near.yaml's own text.
    assert_eq!(
        options.included_edges.len(),
        2,
        "{:?}",
        options.included_edges
    );
    let first = &options.included_edges[0];
    assert_eq!(first.uri, "a.yaml");
    assert_eq!(
        (first.offset, first.length),
        (
            u32::try_from(near.find("a.yaml").expect("anchor")).expect("u32"),
            6
        )
    );
    let second = &options.included_edges[1];
    assert_eq!(second.uri, "b.yaml");
    assert_eq!(
        (second.offset, second.length),
        (
            u32::try_from(near.find("b.yaml").expect("anchor")).expect("u32"),
            6
        )
    );

    // The groups the edges index carry each subtree's effective state —
    // the local enabled declaration lands in `enabled_rules` with its own
    // file and the name token's span in that file's text.
    let a_group = &options.included_groups[first.group];
    assert_eq!(a_group.file, dir.path("a.yaml"));
    assert_eq!(
        a_group.enabled_rules.len(),
        1,
        "{:?}",
        a_group.enabled_rules
    );
    let a_rule = &a_group.enabled_rules[0];
    assert_eq!(a_rule.name, "prefer_double_quotes");
    assert_eq!(a_rule.file, dir.path("a.yaml"));
    assert_eq!(
        (a_rule.offset, a_rule.length),
        (
            u32::try_from(a.find("prefer_double_quotes").expect("anchor")).expect("u32"),
            20
        )
    );
    assert!(a_group.disabled_rules.is_empty());
    assert!(a_group.includes.is_empty(), "a.yaml includes nothing");

    let b_group = &options.included_groups[second.group];
    assert_eq!(
        b_group.enabled_rules.len(),
        1,
        "{:?}",
        b_group.enabled_rules
    );
    assert_eq!(b_group.enabled_rules[0].name, "prefer_single_quotes");
    assert_eq!(b_group.enabled_rules[0].file, dir.path("b.yaml"));
}

/// A nearer include shadows a farther one's same-named declaration in the
/// folded effective state — the official `apply` removes the enabled entry
/// the disabled one names before inserting it (`_EffectiveLinterRules.apply`,
/// linter_rule_options_validator.dart:25-34). The included file's own local
/// declarations apply after its subtree, so the disable it declares hides
/// the subtree's enable.
#[test]
fn effective_rules_shadow_same_named_entries_across_the_subtree() {
    let dir = TempDir::new("xshadow");
    // near.yaml includes mid.yaml; mid.yaml includes far.yaml and disables
    // `avoid_print` locally — the group's effective state must show the
    // local disable winning over the subtree's enable.
    dir.write("near.yaml", "include: mid.yaml\n");
    dir.write(
        "mid.yaml",
        "include: far.yaml\nlinter:\n  rules:\n    avoid_print: false\n",
    );
    dir.write("far.yaml", "linter:\n  rules:\n    - avoid_print\n");

    let options = AnalysisOptions::load(&dir.path("near.yaml"), &empty_config(), &dir.0);

    assert_eq!(options.included_edges.len(), 1);
    let mid = &options.included_groups[options.included_edges[0].group];
    assert_eq!(mid.file, dir.path("mid.yaml"));
    // mid.yaml's own include edge lands in its group — the nested walk's
    // per-level entry.
    assert_eq!(mid.includes.len(), 1, "{:?}", mid.includes);
    assert_eq!(mid.includes[0].uri, "far.yaml");
    // The folded effective state: the local disable hides the subtree's
    // enable — `avoid_print` lands in `disabled_rules`, not `enabled_rules`.
    assert!(mid.enabled_rules.is_empty(), "{:?}", mid.enabled_rules);
    assert_eq!(mid.disabled_rules.len(), 1, "{:?}", mid.disabled_rules);
    assert_eq!(mid.disabled_rules[0].name, "avoid_print");
    assert_eq!(mid.disabled_rules[0].file, dir.path("mid.yaml"));

    // far.yaml's own group still reports its local enable — the effective
    // state is per-file, and far.yaml seen on its own has no disable.
    let far = &options.included_groups[mid.includes[0].group];
    assert_eq!(far.file, dir.path("far.yaml"));
    assert_eq!(far.enabled_rules.len(), 1, "{:?}", far.enabled_rules);
    assert_eq!(far.enabled_rules[0].name, "avoid_print");
}

/// An `ignore` value in an included file lands in the group's
/// `disabled_rules` — the validator's `_LocalLinterRules.disabled` side
/// (`parseRule` returns the `ignore`-valued declaration in the disabled
/// list, analysis_options_parse_model.dart:1266-1283), the fact that lets a
/// local `ignore` suppress the include-side conflict. The merged
/// `lint_rules` face keeps the name enabled (a non-`false` value is an
/// enable there) — the two faces diverge by design.
#[test]
fn ignore_value_included_file_lands_in_disabled_rules() {
    let dir = TempDir::new("xignore");
    dir.write("near.yaml", "include: far.yaml\n");
    dir.write(
        "far.yaml",
        "linter:\n  rules:\n    avoid_print: ignore\n    prefer_double_quotes: true\n",
    );

    let options = AnalysisOptions::load(&dir.path("near.yaml"), &empty_config(), &dir.0);

    let far = &options.included_groups[options.included_edges[0].group];
    let disabled: Vec<&str> = far
        .disabled_rules
        .iter()
        .map(|rule| rule.name.as_str())
        .collect();
    assert_eq!(disabled, ["avoid_print"], "{:?}", far.disabled_rules);
    let enabled: Vec<&str> = far
        .enabled_rules
        .iter()
        .map(|rule| rule.name.as_str())
        .collect();
    assert_eq!(enabled, ["prefer_double_quotes"], "{:?}", far.enabled_rules);
    // The merged face keeps `avoid_print` enabled — `ignore` is not `false`.
    assert!(
        options.lint_rules.iter().any(|name| name == "avoid_print"),
        "{:?}",
        options.lint_rules
    );
}

/// The file-count fact the `incompatible_lint_included` row carries (`{2}`
/// — the distinct include nodes contributing to one conflict): the initial
/// file's edges stay distinct per `include:` entry even when two entries
/// reach the same conflicting rule through different subtrees. The nested
/// walk's per-level entries land in each group's own `includes`.
#[test]
fn nested_include_of_include_keeps_per_level_edges() {
    let dir = TempDir::new("xnest");
    // top.yaml includes mid.yaml; mid.yaml includes leaf_a.yaml and
    // leaf_b.yaml — the pair conflict lives one level down, and both the
    // initial file's edge and mid.yaml's own two edges must exist.
    dir.write(
        "top.yaml",
        "include: mid.yaml\nlinter:\n  rules:\n    - avoid_print\n",
    );
    dir.write("mid.yaml", "include:\n  - leaf_a.yaml\n  - leaf_b.yaml\n");
    dir.write(
        "leaf_a.yaml",
        "linter:\n  rules:\n    - prefer_double_quotes\n",
    );
    dir.write(
        "leaf_b.yaml",
        "linter:\n  rules:\n    - prefer_single_quotes\n",
    );

    let options = AnalysisOptions::load(&dir.path("top.yaml"), &empty_config(), &dir.0);

    // The initial file sees only its direct edge; the nested pair lives in
    // mid.yaml's group.
    assert_eq!(options.included_edges.len(), 1);
    assert_eq!(options.included_edges[0].uri, "mid.yaml");
    let mid = &options.included_groups[options.included_edges[0].group];
    assert_eq!(mid.includes.len(), 2, "{:?}", mid.includes);
    assert_eq!(mid.includes[0].uri, "leaf_a.yaml");
    assert_eq!(mid.includes[1].uri, "leaf_b.yaml");
    // Each leaf group carries its subtree's effective state — folded from
    // its own (empty) includes plus its local declarations.
    let leaf_a = &options.included_groups[mid.includes[0].group];
    assert_eq!(leaf_a.file, dir.path("leaf_a.yaml"));
    assert_eq!(leaf_a.enabled_rules.len(), 1);
    assert_eq!(leaf_a.enabled_rules[0].name, "prefer_double_quotes");
    let leaf_b = &options.included_groups[mid.includes[1].group];
    assert_eq!(leaf_b.file, dir.path("leaf_b.yaml"));
    assert_eq!(leaf_b.enabled_rules.len(), 1);
    assert_eq!(leaf_b.enabled_rules[0].name, "prefer_single_quotes");
    // mid.yaml's effective state folds both leaves: the pair enabled.
    let names: Vec<&str> = mid
        .enabled_rules
        .iter()
        .map(|rule| rule.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["prefer_double_quotes", "prefer_single_quotes"],
        "{:?}",
        mid.enabled_rules
    );
}

/// The INCOMPATIBLE_LINT family's context producers (census W3). Upstream
/// (upstream Dart SDK source,
/// `pkg/analyzer/lib/src/diagnostic/diagnostic_factory.dart:288-382`) folds
/// every conflicting declaration into a `Map<file.path, node>` and reports
/// one context message per entry at the node's span — `"The rule '<name>'
/// is enabled here."`, with `" in the file '<path>'."` on the files variant
/// only; the local list's single-file fold collapses to the last entry.
#[test]
fn incompatible_lint_context_rows_carry_the_conflicting_rule_anchors() {
    use dartq_engine::analysis_options::{ContextualIssue, DeclaredRule, RuleData};
    use dartq_source::FileTable;

    let declared = |name: &str, offset: u32| DeclaredRule {
        name: name.to_owned(),
        offset,
        length: 8,
        enabled: true,
        illegal_value: None,
    };
    let rule_neg = declared("rule_neg", 30);
    let rule_pos = declared("rule_pos", 52);
    let rule_neu = declared("rule_neu", 74);

    let reporting_path = PathBuf::from("/t/analysis_options.yaml");
    let inc1 = PathBuf::from("/t/inc1.yaml");
    let inc2 = PathBuf::from("/t/inc2.yaml");
    let included = |name: &str, file: &PathBuf, offset: u32| RuleData {
        name: name.to_owned(),
        file: file.clone(),
        offset,
        length: 8,
    };
    let pos_in_inc1 = included("rule_pos", &inc1, 12);
    let neu_in_inc2 = included("rule_neu", &inc2, 20);

    let mut table = FileTable::new();
    let reporting = table.intern(&reporting_path);
    let id_inc1 = table.intern(&inc1);
    let id_inc2 = table.intern(&inc2);

    // Local list: both conflicts enabled in the reporting file, so the
    // single-file map fold collapses to the last entry; the row message
    // keeps the plain factory's rendering of all conflicting spellings.
    let issue = ContextualIssue::incompatible_lint(&rule_neg, &[&rule_pos, &rule_neu]);
    let diag = issue.to_diagnostic(reporting, |path| table.try_file_id(path));
    assert_eq!(
        diag.message.as_str(),
        "The rule 'rule_neg' is incompatible with ''rule_pos' and 'rule_neu''."
    );
    let context = diag.context.expect("local context attached");
    assert_eq!(
        context.len(),
        1,
        "the single-file fold keeps the last entry"
    );
    assert_eq!(context[0].file, reporting);
    assert_eq!(
        context[0].message.as_str(),
        "The rule 'rule_neu' is enabled here."
    );
    assert_eq!(context[0].range.offset, 74);
    assert_eq!(context[0].range.length, 8);

    // Files variant: one context row per entry, in fold order, each naming
    // its file and anchored at the rule's span in that file.
    let issue = ContextualIssue::incompatible_lint_files(
        "rule_neg",
        &[&pos_in_inc1, &neu_in_inc2],
        (40, 8),
    );
    let diag = issue.to_diagnostic(reporting, |path| table.try_file_id(path));
    assert_eq!(
        diag.message.as_str(),
        "The rule 'rule_neg' is incompatible with 'rule_pos' and 'rule_neu'."
    );
    let context = diag.context.expect("files context attached");
    assert_eq!(context.len(), 2);
    assert_eq!(context[0].file, id_inc1);
    assert_eq!(
        context[0].message.as_str(),
        format!(
            "The rule 'rule_pos' is enabled here in the file '{}'.",
            inc1.display()
        )
    );
    assert_eq!(context[0].range.offset, 12);
    assert_eq!(context[1].file, id_inc2);
    assert_eq!(
        context[1].message.as_str(),
        format!(
            "The rule 'rule_neu' is enabled here in the file '{}'.",
            inc2.display()
        )
    );
    assert_eq!(context[1].range.offset, 20);

    // Included variant: the same anchors with the plain context message
    // (the include URI lives in the row's message body, not the context).
    let issue = ContextualIssue::incompatible_lint_included(
        "inc2.yaml",
        &[&pos_in_inc1, &neu_in_inc2],
        2,
        (14, 12),
    );
    let diag = issue.to_diagnostic(reporting, |path| table.try_file_id(path));
    assert_eq!(
        diag.message.as_str(),
        "The rule 'inc2.yaml' is incompatible with 'rule_pos' and 'rule_neu', \
         which is included from 2 files."
    );
    let context = diag.context.expect("included context attached");
    assert_eq!(context.len(), 2);
    assert_eq!(context[0].file, id_inc1);
    assert_eq!(
        context[0].message.as_str(),
        "The rule 'rule_pos' is enabled here."
    );
    assert_eq!(
        context[1].message.as_str(),
        "The rule 'rule_neu' is enabled here."
    );
}

/// The context emission's degrade posture: a conflicting rule's file the
/// caller cannot resolve drops its row — under-report, never a crash — and
/// a fully-dropped context degrades to the plain no-context row.
#[test]
fn incompatible_lint_context_drops_unresolvable_files() {
    use dartq_engine::analysis_options::{ContextualIssue, RuleData};
    use dartq_source::FileTable;

    let inc1 = PathBuf::from("/t/missing.yaml");
    let pos = RuleData {
        name: "rule_pos".to_owned(),
        file: inc1.clone(),
        offset: 12,
        length: 8,
    };

    let mut table = FileTable::new();
    let reporting = table.intern(PathBuf::from("/t/analysis_options.yaml").as_path());

    let issue = ContextualIssue::incompatible_lint_included("inc1.yaml", &[&pos], 1, (14, 12));
    let diag = issue.to_diagnostic(reporting, |path| table.try_file_id(path));
    assert!(
        diag.context.is_none(),
        "unresolvable file drops the whole context"
    );

    // Partial resolution keeps the resolvable rows only.
    table.intern(&inc1);
    let diag = issue.to_diagnostic(reporting, |path| table.try_file_id(path));
    let context = diag.context.expect("resolved file attaches its row");
    assert_eq!(context.len(), 1);
}

/// The zero-conflict face: a producer handed no conflicting declarations
/// emits the plain row with no `contextMessages` key at all — the empty
/// fold degrades through `with_context`'s empty face, the same posture the
/// unresolvable-file drop lands in (`with_context` keeps `None` on empty).
#[test]
fn incompatible_lint_without_conflicts_carries_no_context() {
    use dartq_engine::analysis_options::{ContextualIssue, DeclaredRule, RuleData};
    use dartq_source::FileTable;

    let rule = DeclaredRule {
        name: "rule_neg".to_owned(),
        offset: 30,
        length: 8,
        enabled: true,
        illegal_value: None,
    };
    let included = RuleData {
        name: "rule_pos".to_owned(),
        file: PathBuf::from("/t/inc1.yaml"),
        offset: 12,
        length: 8,
    };

    let mut table = FileTable::new();
    let reporting = table.intern(PathBuf::from("/t/analysis_options.yaml").as_path());

    let local = ContextualIssue::incompatible_lint(&rule, &[])
        .to_diagnostic(reporting, |path| table.try_file_id(path));
    assert!(
        local.context.is_none(),
        "zero conflicts emit no contextMessages key"
    );

    let files = ContextualIssue::incompatible_lint_files("rule_neg", &[], (30, 8))
        .to_diagnostic(reporting, |path| table.try_file_id(path));
    assert!(
        files.context.is_none(),
        "zero conflicts emit no contextMessages key"
    );

    let thru_includes = ContextualIssue::incompatible_lint_included("inc1.yaml", &[], 1, (14, 12))
        .to_diagnostic(reporting, |path| table.try_file_id(path));
    assert!(
        thru_includes.context.is_none(),
        "zero conflicts emit no contextMessages key"
    );
}
