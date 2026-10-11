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
/// that does not pass through the initial file is `included_file_warning` —
/// a wrapping face dartq has not built — so it stays silent; missing wins
/// for unresolvable edges because recursion is only reached for parsed ones.
#[test]
fn recursive_include_reports_back_to_initial_and_chain_cycles_stay_silent() {
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
    // the official row would be `includedFileWarning` (walker :176-188),
    // silent here, and the walk still terminates.
    let deep = TempDir::new("chain_cycle");
    deep.write("a.yaml", "include: b.yaml\n");
    deep.write("b.yaml", "include: c.yaml\n");
    deep.write("c.yaml", "include: b.yaml\n");
    let options = AnalysisOptions::load(&deep.path("a.yaml"), &empty_config(), &deep.0);
    assert!(
        options.issues.is_empty(),
        "chain cycles are the wrapped face, not built: {:?}",
        options.issues
    );
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

/// An included file's local shape problems are officially wrapped into
/// `included_file_warning` (include_walker.dart:236-247) — a face dartq has
/// not built — so they stay silent through the includer's load, while the
/// included file's own load reports them (`_walk` reports local diagnostics
/// directly for the initial file, walker :74-76).
#[test]
fn included_file_local_problems_stay_silent_for_the_includer() {
    let dir = TempDir::new("wrapped");
    dir.write("near.yaml", "include: far.yaml\n");
    dir.write("far.yaml", "linter: 42\n");

    let near = AnalysisOptions::load(&dir.path("near.yaml"), &empty_config(), &dir.0);
    assert!(
        near.issues.is_empty(),
        "wrapped faces are not built: {:?}",
        near.issues
    );

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
    assert_eq!(
        actual,
        [
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
