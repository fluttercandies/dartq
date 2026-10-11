//! The lint face's mount (PRD §13): the analyze pipeline's bridge to
//! `dartq-lints` — enable-set resolution from the merged analysis options,
//! and the layer's rows mapped into engine diagnostics. This module lives
//! here, not in the engine: dartq-lints already depends on engine-side
//! types, so an engine→lints edge would be a cycle.
//!
//! # Selection
//!
//! [`enable_set`] resolves one run's enable set from a root's merged
//! options ([`AnalysisOptions::lint_rules`], the include-chain merge):
//!
//! * No options file in the root's ancestry (`None`) — the default
//!   posture is the whole `package:lints` slice dartq-lints implements
//!   (core plus recommended; every registered rule carries one of the two
//!   groups, so the default set is the full registry). Deliberate
//!   divergence from the oracle, which enables nothing without an
//!   `include:`: the slice is this product's release gate.
//! * Options present — their effective rule list is the run's set,
//!   verbatim (the official replace semantics: the merged `linter.rules`
//!   section *is* the set; `name: false` drops the name before it reaches
//!   us, so "disable one rule" means the include chain supplies the rest).
//!
//! Unknown rule names in the list resolve to nothing (the layer's
//! `EnableSet::resolve` reports them back; the oracle surfaces them as
//! options-file warnings on a separate face dartq has not built — they are
//! dropped here, not errors).
//!
//! # Row shape
//!
//! [`diagnose`] runs the layer over one unit and maps each row to a
//! [`Diagnostic`]: severity is the registry default
//! ([`DiagnosticType::Lint::severity`] — oracle-INFO, the
//! `dtype_matches_oracle_severity` invariant; the layer's `EnableSet`
//! models no severity overrides), the code identity derives from the rule
//! registry (name and UPPERCASE machine form, type `LINT`, never
//! published docs — mirroring the hand-written `PREFER_SINGLE_QUOTES`
//! row), and the UTF-16 row span becomes the engine's
//! [`SourceRange`]. `EnableSet` carries no severity overrides, so there is
//! nothing else to map.
//!
//! # Activation
//!
//! [`analyze_lints`] is the production activation: it re-parses each
//! resident file's source text transiently ([`dartq_lints::parse_unit`] —
//! the #355 bridge; engine residency retains neither tokens nor trees) and
//! runs the enabled rules, extending the engine's diagnostics. The CLI
//! face calls it from `analyze::run_overlaid`, the daemon from
//! `session::run_group`, both before their display sorts. The check face
//! stays lint-free by design (scan+parse only).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use dartq_diagnostics::{CodeInfo, Diagnostic, DiagnosticType, SourceRange};
use dartq_engine::Engine;
use dartq_lints::{EnableSet, Features, Group, Lint, RULES, RuleId, Unit, parse_unit, run};
use dartq_source::FileId;

/// The whole `package:lints` slice: every registered rule ships in core or
/// recommended (recommended includes core), so the default posture is the
/// full registry, enabled by group so a future rule outside the sets
/// cannot silently join it.
fn default_set() -> EnableSet {
    let mut set = EnableSet::none();
    for (index, spec) in RULES.iter().enumerate() {
        // The registry's growth is bounded by the pin test; `RuleId` is u16.
        let id = RuleId(u16::try_from(index).expect("registry fits u16"));
        if matches!(spec.group, Group::Core | Group::Recommended) {
            set.add(id);
        }
    }
    set
}

/// The enable set one lint run executes under: `None` (no options file in
/// the root's ancestry) selects the default `package:lints` posture, a
/// present options file's effective rule list replaces it. See the module
/// docs for the selection semantics.
#[must_use]
pub fn enable_set(rules: Option<&[String]>) -> EnableSet {
    static DEFAULT: LazyLock<EnableSet> = LazyLock::new(default_set);
    match rules {
        None => *DEFAULT,
        Some(names) => EnableSet::resolve(names.iter().map(String::as_str)).0,
    }
}

/// Per-rule code identity, registry-parallel (`RuleId` indexes it): the
/// rule name with its UPPERCASE machine form, type `LINT`, never published
/// docs. The machine forms leak once per rule (68 bounded, process-lifetime
/// strings) because `CodeInfo` pins `&'static str` and the names are
/// runtime-independent statics.
static CODES: LazyLock<Vec<CodeInfo>> = LazyLock::new(|| {
    RULES
        .iter()
        .map(|spec| {
            let machine: &'static str = Box::leak(spec.name.to_uppercase().into_boxed_str());
            CodeInfo::new(spec.name, machine, DiagnosticType::Lint, false)
        })
        .collect()
});

/// Maps one lint row to an engine diagnostic — the oracle's lint shape:
/// INFO severity, the rule name as the code, the row's UTF-16 span, the
/// official problem/correction texts verbatim.
fn row(lint: Lint, file: FileId) -> Diagnostic {
    Diagnostic::new(
        DiagnosticType::Lint.severity(),
        CODES[lint.rule.0 as usize],
        lint.message,
        file,
        SourceRange {
            offset: lint.range.start,
            length: lint.range.end - lint.range.start,
        },
        lint.correction,
    )
}

/// Runs the enabled rules over one parsed unit and yields the rows as
/// engine diagnostics (traversal order; the oracle display sort happens at
/// the face). The unit's substrate contract is the layer's ([`Unit`] — the
/// unified post-parse token/AST pair).
#[must_use]
pub fn diagnose(file: FileId, unit: &Unit<'_>, enabled: &EnableSet) -> Vec<Diagnostic> {
    run(unit, enabled)
        .into_iter()
        .map(|lint| row(lint, file))
        .collect()
}

/// The analyze pipeline's lint stage (#355 activation): every resident
/// file's rows, one transient parse per file (the bridge's
/// zero-persistent-memory ruling — nothing enters engine residency).
///
/// Per-file enable set: the longest `roots` entry containing the file's
/// path wins (multi-package workspaces); the first root is the fallback,
/// matching the engine's discovery attribution for root-level files. An
/// empty resolved set skips the file without parsing (opt-in cost ruling).
/// Files that are no longer resident or are not UTF-8 are skipped — the
/// engine's own pass already reported nothing for them.
#[must_use]
pub fn analyze_lints(engine: &Engine, roots: &[PathBuf]) -> Vec<Diagnostic> {
    let mut files: Vec<FileId> = engine
        .libraries()
        .into_iter()
        .flat_map(|(_, files)| files)
        .collect();
    files.sort_unstable();
    files.dedup();

    // Per-root enable sets, resolved once (the engine's options table
    // carries a default entry for roots without a file, so the no-options
    // posture is decided by `find_for_dir`, not by `options_for_root`).
    let mut enabled: HashMap<PathBuf, EnableSet> = HashMap::new();

    let mut out = Vec::new();
    for file in files {
        let Some(path) = engine.file_table().path(file) else {
            continue;
        };
        let Some(bytes) = engine.content(file) else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(bytes) else {
            continue;
        };
        let root = containing_root(path, roots);
        let set = match enabled.get(root) {
            Some(set) => *set,
            None => {
                let set = match dartq_engine::analysis_options::AnalysisOptions::find_for_dir(root)
                {
                    // No options file in the root's ancestry: the whole
                    // `package:lints` slice (the mount's default posture).
                    None => enable_set(None),
                    Some(_) => enable_set(
                        engine
                            .options_for_root(root)
                            .map(|options| options.lint_rules.as_slice()),
                    ),
                };
                enabled.insert(root.to_path_buf(), set);
                set
            }
        };
        if set == EnableSet::none() {
            continue;
        }
        let parsed = parse_unit(text, Features::default());
        out.extend(diagnose(file, &parsed.unit(Some(path)), &set));
    }
    out
}

/// The longest `roots` entry that is a byte-prefix directory of `path`, or
/// the first root (single-root workspaces are the dominant shape).
fn containing_root<'a>(path: &Path, roots: &'a [PathBuf]) -> &'a Path {
    let mut best: Option<&Path> = None;
    for root in roots {
        if path.starts_with(root)
            && best.is_none_or(|longest| root.components().count() > longest.components().count())
        {
            best = Some(root);
        }
    }
    best.unwrap_or(
        roots
            .first()
            .map(PathBuf::as_path)
            .unwrap_or_else(|| Path::new("")),
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use dartq_diagnostics::{DiagnosticType, Severity};
    use dartq_engine::Engine;
    use dartq_engine::analysis_options::AnalysisOptions;
    use dartq_lints::rule_by_name;
    use dartq_pub::PackageConfig;
    use dartq_source::FileTable;

    use super::*;

    /// Every registered rule ships in the `package:lints` slice, so the
    /// no-options default is the full registry.
    #[test]
    fn absent_options_default_to_the_package_lints_slice() {
        assert_eq!(
            RULES
                .iter()
                .filter(|spec| matches!(spec.group, Group::Core | Group::Recommended))
                .count(),
            RULES.len(),
            "a rule outside both sets changes the default posture",
        );
        let set = enable_set(None);
        for index in 0..RULES.len() {
            assert!(set.is_enabled(RuleId(index as u16)));
        }
    }

    /// A present options file replaces the default: exactly the listed
    /// names resolve, nothing else.
    #[test]
    fn explicit_options_replace_the_default() {
        let set = enable_set(Some(&["avoid_empty_else".to_owned()]));
        assert!(set.is_enabled(rule_by_name("avoid_empty_else").expect("wired")));
        assert!(!set.is_enabled(rule_by_name("unnecessary_new").expect("wired")));
    }

    /// The options-file path end to end: the real `linter.rules` merge
    /// feeds [`enable_set`] — the list spelling enables exactly its names,
    /// the `name: false` spelling silences the rule.
    #[test]
    fn options_file_rules_drive_the_enable_set() {
        let root = crate::workspace::test_fixture::workspace("lints_options", &[]);
        let packages = PackageConfig::parse(
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "languageVersion": "3.5"}], "generator": "pub"}"#,
        )
        .expect("fixture config");
        let config_dir = root.join(".dart_tool");
        let load = |yaml: &str| {
            let path = root.join("analysis_options.yaml");
            std::fs::write(&path, yaml).expect("options file");
            AnalysisOptions::load(&path, &packages, &config_dir)
        };

        let enabled = enable_set(Some(
            &load("linter:\n  rules:\n    - avoid_empty_else\n").lint_rules,
        ));
        assert!(enabled.is_enabled(rule_by_name("avoid_empty_else").expect("wired")));
        assert!(!enabled.is_enabled(rule_by_name("unnecessary_new").expect("wired")));

        let disabled = enable_set(Some(
            &load("linter:\n  rules:\n    avoid_empty_else: false\n").lint_rules,
        ));
        assert!(
            !disabled.is_enabled(rule_by_name("avoid_empty_else").expect("wired")),
            "the disable spelling silences the rule"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// Rows map to the oracle's lint shape: INFO severity (the registry's
    /// single `DiagnosticType::Lint` derivation), the rule name and its
    /// UPPERCASE machine form as an unpublished `LINT` code, the UTF-16
    /// span, and the official texts verbatim — with and without a
    /// correction.
    #[test]
    fn lint_rows_map_to_oracle_diagnostics() {
        let mut files = FileTable::default();
        let file = files.intern(Path::new("t.dart"));
        let rule = rule_by_name("unnecessary_new").expect("wired");

        let corrected = row(
            Lint {
                rule,
                range: 19..22,
                message: "Unnecessary 'new' keyword.".to_owned(),
                correction: Some("Try removing the 'new' keyword.".to_owned()),
            },
            file,
        );
        assert_eq!(corrected.severity, Severity::Info);
        assert_eq!(corrected.severity, DiagnosticType::Lint.severity());
        assert_eq!(corrected.code.name, "unnecessary_new");
        assert_eq!(corrected.code.machine, "UNNECESSARY_NEW");
        assert_eq!(corrected.code.dtype, DiagnosticType::Lint);
        assert!(!corrected.code.has_published_docs);
        assert_eq!(corrected.message, "Unnecessary 'new' keyword.");
        assert_eq!(corrected.file, file);
        assert_eq!(corrected.range.offset, 19);
        assert_eq!(corrected.range.length, 3);
        assert_eq!(
            corrected.correction.as_deref(),
            Some("Try removing the 'new' keyword.")
        );

        let bare = row(
            Lint {
                rule,
                range: 0..0,
                message: "Unnecessary 'new' keyword.".to_owned(),
                correction: None,
            },
            file,
        );
        assert_eq!(bare.correction, None);
        assert_eq!(bare.range.length, 0);
    }

    /// The #355 activation end to end: a fixture workspace whose only
    /// library carries an `unnecessary_new` shape produces an INFO lint row
    /// through the full pipeline — engine pass, transient re-parse, row
    /// mapping, oracle order — with the semantic row of the same file.
    #[test]
    fn analyze_lints_rows_flow_through_the_resident_pipeline() {
        let root = crate::workspace::test_fixture::workspace(
            "lints_activation",
            &[("lib/bad.dart", "var x = new Object();\nvar ok = unknown;\n")],
        );
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        let rows: Vec<(String, Severity, u32)> = run
            .diagnostics
            .iter()
            .map(|d| (d.code.name.to_owned(), d.severity, d.range.offset))
            .collect();
        assert!(
            rows.contains(&("unnecessary_new".to_owned(), Severity::Info, 8)),
            "the lint row rides the pipeline: {rows:?}"
        );
        assert!(
            rows.contains(&("undefined_identifier".to_owned(), Severity::Error, 31)),
            "the semantic row still flows: {rows:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
