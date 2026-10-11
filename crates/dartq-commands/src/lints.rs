//! The lint face's mount (PRD §13): the analyze pipeline's bridge to
//! `dartq-lints` — enable-set resolution from the merged analysis options,
//! and the layer's rows mapped into engine diagnostics. This module lives
//! here, not in the engine: dartq-lints already depends on engine-side
//! types, so an engine→lints edge would be a cycle.
//!
//! # Selection
//!
//! [`enable_set`] resolves one enable set from one governing entry's
//! merged options ([`AnalysisOptions::lint_rules`], the include-chain
//! merge):
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

use rustc_hash::{FxHashMap, FxHashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use dartq_diagnostics::{CodeInfo, Diagnostic, DiagnosticType, SourceRange};
use dartq_engine::Engine;
use dartq_engine::analysis_options::AnalysisOptions;
use dartq_lints::{
    EnableSet, Features, Group, Lint, RULES, RuleId, Unit, WorkspaceFacts, check_name, check_url,
    parse_unit, rule_by_name, run,
};
use dartq_pub::pubspec_facts::scan as scan_pubspec;
use dartq_pub::{Pubspec, pubspec_facts::Scalar};
use dartq_source::{FileId, LineIndex};

/// The default posture: the `package:lints` slice (core + recommended,
/// recommended includes core), enabled by group so a rule outside the sets
/// — a diagnostic mirror — cannot silently join it.
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

/// The enable set one governing entry selects: `None` (no options file in
/// the governed file's ancestry) selects the default `package:lints`
/// posture, a present options file's effective rule list replaces it. See
/// the module docs for the selection semantics.
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

/// The containing-package facts behind `Unit.workspace`, cached per package
/// root for one run: the attribution resolves through the package table's
/// `containing` walk, the pubspec parse runs once per package and only its
/// names are kept. `None` — no containing package, an unreadable pubspec,
/// or a parse failure — is the official no-pubspec posture: the
/// package-aware rules stay silent for those files.
fn workspace_facts<'a>(
    engine: &Engine,
    path: &Path,
    cache: &'a mut FxHashMap<PathBuf, Option<WorkspaceFacts>>,
) -> Option<&'a WorkspaceFacts> {
    let identity = engine.packages().containing(path)?;
    if !cache.contains_key(&identity.root) {
        let face = std::fs::read_to_string(identity.root.join("pubspec.yaml"))
            .ok()
            .and_then(|text| Pubspec::parse(&text).ok())
            .map(|spec| {
                let mut dependencies: FxHashSet<String> =
                    spec.dependencies.keys().cloned().collect();
                dependencies.insert(spec.name.clone());
                WorkspaceFacts {
                    own_package: spec.name,
                    dependencies,
                    dev_dependencies: spec.dev_dependencies.keys().cloned().collect(),
                    root: identity.root.clone(),
                    lib_dir: identity.lib_dir.clone(),
                }
            });
        cache.insert(identity.root.clone(), face);
    }
    cache.get(&identity.root).and_then(Option::as_ref)
}

/// The analyze pipeline's lint stage (#355 activation): every resident
/// file's rows, one transient parse per file (the bridge's
/// zero-persistent-memory ruling — nothing enters engine residency), plus
/// the pubspec pass over the workspace's `pubspec.yaml` files (#434).
///
/// Per-subtree enable sets: each file runs under the options entry its own
/// ancestry governs ([`governing_options`]) — the same deepest-governing
/// resolution the engine's severity and exclude faces apply, so a nested
/// options file's `linter.rules` section refines its subtree here too.
/// Sets are resolved once per distinct file directory. An empty resolved
/// set skips the file without parsing (opt-in cost ruling). Files that are
/// no longer resident or are not UTF-8 are skipped — the engine's own pass
/// already reported nothing for them.
///
/// The package-aware rules read the containing package's facts through
/// `Unit.workspace` ([`workspace_facts`], one pubspec parse per package).
/// The pubspec pass runs after the file loop: the root package's and every
/// workspace member's `pubspec.yaml`, through the same enable-set
/// resolution, the scanned facts, and the YAML `# ignore:` variant
/// ([`YamlIgnores`]).
#[must_use]
pub fn analyze_lints(engine: &Engine, roots: &[PathBuf]) -> Vec<Diagnostic> {
    let mut files: Vec<FileId> = engine
        .libraries()
        .into_iter()
        .flat_map(|(_, files)| files)
        .collect();
    files.sort_unstable();
    files.dedup();

    // One enable set per distinct file directory (governance is by
    // ancestry, so one directory always maps to one entry).
    let mut enabled: FxHashMap<PathBuf, EnableSet> = FxHashMap::default();
    let mut workspace: FxHashMap<PathBuf, Option<WorkspaceFacts>> = FxHashMap::default();

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
        let dir = path.parent().unwrap_or(Path::new(""));
        let set = match enabled.get(dir) {
            Some(set) => *set,
            None => {
                let set = enable_set(
                    governing_options(engine, dir, roots)
                        .map(|options| options.lint_rules.as_slice()),
                );
                enabled.insert(dir.to_path_buf(), set);
                set
            }
        };
        if set == EnableSet::none() {
            continue;
        }
        let parsed = parse_unit(text, Features::default());
        let mut unit = parsed.unit(Some(path));
        unit.workspace = workspace_facts(engine, path, &mut workspace);
        out.extend(diagnose(file, &unit, &set));
    }
    out.extend(pubspec_rows(engine, roots));
    out
}

/// The pubspec pass (#434): the root package's and every workspace
/// member's `pubspec.yaml` — the same set the engine captured at analyze
/// time — under the same enable-set resolution the Dart files get. A
/// capture miss (no engine pass yet, or no pubspec file) leaves the file
/// unreported. Rows carry the official value-node span (UTF-16 over the
/// pubspec text) and the `IgnoreInfo.forYaml` suppression face.
fn pubspec_rows(engine: &Engine, roots: &[PathBuf]) -> Vec<Diagnostic> {
    let table = engine.packages();
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(root) = table.get(table.root().as_str()) {
        dirs.push(root.root.clone());
    }
    if let Some(members) = table.workspace_members() {
        dirs.extend(
            members
                .iter()
                .filter_map(|name| table.get(name.as_str()))
                .map(|identity| identity.root.clone()),
        );
    }

    let mut out = Vec::new();
    for dir in dirs {
        let path = dir.join("pubspec.yaml");
        let Some(file) = engine.file_table().try_file_id(&path) else {
            continue;
        };
        let Some(bytes) = engine.content(file) else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(bytes) else {
            continue;
        };
        let set = enable_set(
            governing_options(engine, &dir, roots).map(|options| options.lint_rules.as_slice()),
        );
        let urls_rule = rule_by_name("secure_pubspec_urls");
        let names_rule = rule_by_name("package_names");
        let urls_on = urls_rule.is_some_and(|id| set.is_enabled(id));
        let names_on = names_rule.is_some_and(|id| set.is_enabled(id));
        if !urls_on && !names_on {
            continue;
        }
        let facts = scan_pubspec(text);
        let ignores = YamlIgnores::parse(text);
        let lines = LineIndex::new(text);
        let mut lints = Vec::new();
        if urls_on {
            let rule = urls_rule.expect("checked above");
            let spec = &RULES[rule.0 as usize];
            for Scalar { range, value } in facts.urls.iter().chain(&facts.dep_urls) {
                let mut hits = Vec::new();
                check_url(range.clone(), value, &mut hits);
                lints.extend(hits.into_iter().filter_map(|hit| {
                    let (line, _) = lines.byte_to_line_col(hit.range.start as u32);
                    (!ignores.ignored(line, "secure_pubspec_urls")).then(|| Lint {
                        rule,
                        range: utf16_range(&lines, text, &hit.range),
                        message: spec.render_problem(&[hit.scheme]),
                        correction: spec.render_correction(&[]),
                    })
                }));
            }
        }
        if names_on {
            let rule = names_rule.expect("checked above");
            let spec = &RULES[rule.0 as usize];
            if let Some(name) = &facts.name {
                let mut hits = Vec::new();
                check_name(name.range.clone(), &name.value, &mut hits);
                lints.extend(hits.into_iter().filter_map(|hit| {
                    let (line, _) = lines.byte_to_line_col(hit.range.start as u32);
                    (!ignores.ignored(line, "package_names")).then(|| Lint {
                        rule,
                        range: utf16_range(&lines, text, &hit.range),
                        message: spec.render_problem(&[&hit.name]),
                        correction: spec.render_correction(&[]),
                    })
                }));
            }
        }
        out.extend(lints.into_iter().map(|lint| row(lint, file)));
    }
    out
}

/// A pubspec row's byte span in UTF-16 (the engine's diagnostic
/// coordinate).
fn utf16_range(
    lines: &LineIndex,
    text: &str,
    range: &core::ops::Range<usize>,
) -> core::ops::Range<u32> {
    lines.byte_to_utf16(text, range.start as u32)..lines.byte_to_utf16(text, range.end as u32)
}

/// The YAML-face suppression table (`IgnoreInfo.forYaml`,
/// pkg/analyzer/lib/src/ignore_comments/ignore_info.dart:97-104, 145-163):
/// `# ignore:` entries — alone on their line suppress the NEXT line,
/// trailing suppress their own — and `# ignore_for_file:` entries
/// file-wide. The names are the comma-separated, whitespace-trimmed list;
/// matching is case-insensitive (the Dart-token comment path is a
/// different face and is not reused).
struct YamlIgnores {
    on_line: FxHashMap<u32, Vec<String>>,
    for_file: Vec<String>,
}

impl YamlIgnores {
    fn parse(text: &str) -> Self {
        let mut info = Self {
            on_line: FxHashMap::default(),
            for_file: Vec::new(),
        };
        for (index, line) in text.lines().enumerate() {
            let line_no = u32::try_from(index).expect("line count fits u32") + 1;
            if let Some((before, names)) = yaml_ignore_entry(line, "ignore:") {
                // Alone on the line -> the next line (ignore_info.dart:157).
                let target = if before.trim().is_empty() {
                    line_no + 1
                } else {
                    line_no
                };
                info.on_line.entry(target).or_default().extend(names);
            }
            if let Some((_, names)) = yaml_ignore_entry(line, "ignore_for_file:") {
                info.for_file.extend(names);
            }
        }
        info
    }

    /// Whether a lint named `name` reported on 1-based `line` is
    /// suppressed (`_ignoredAt`, ignore_info.dart:186-212).
    fn ignored(&self, line: u32, name: &str) -> bool {
        self.for_file
            .iter()
            .any(|entry| entry.eq_ignore_ascii_case(name))
            || self
                .on_line
                .get(&line)
                .is_some_and(|entries| entries.iter().any(|entry| entry.eq_ignore_ascii_case(name)))
    }
}

/// The `#`-commented marker entry of one YAML line: the text before the
/// `#` run, and the name list after the marker. The official regexes'
/// greedy `.*` reads the LAST `#` run the rest still matches from; the
/// scan keeps the last candidate the same way.
fn yaml_ignore_entry<'a>(line: &'a str, marker: &str) -> Option<(&'a str, Vec<String>)> {
    let bytes = line.as_bytes();
    let mut found = None;
    for (i, &b) in bytes.iter().enumerate() {
        if b != b'#' {
            continue;
        }
        let mut j = i;
        while j < bytes.len() && bytes[j] == b'#' {
            j += 1;
        }
        let mut k = j;
        while k < bytes.len() && bytes[k] == b' ' {
            k += 1;
        }
        if line[k..].starts_with(marker) {
            found = Some((&line[..i], &line[k + marker.len()..]));
        }
    }
    found.map(|(before, rest)| {
        (
            before,
            rest.split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .collect(),
        )
    })
}

/// The options entry governing a file in `dir`: the nearest
/// `analysis_options.yaml` at or above `dir`
/// ([`AnalysisOptions::find_for_dir`]) names the governing directory, and
/// the engine's options table is keyed by exactly those directories — the
/// package roots plus every nested options directory the workspace
/// assembly registered ([`AnalysisOptions::for_roots`]). An options file
/// above the package root (the repository-wide shape) carries no key of
/// its own; its entry is the containing root's, which loaded it. `None` —
/// no options file in the ancestry — is the default `package:lints`
/// posture.
fn governing_options<'a>(
    engine: &'a Engine,
    dir: &Path,
    roots: &[PathBuf],
) -> Option<&'a AnalysisOptions> {
    let options_file = AnalysisOptions::find_for_dir(dir)?;
    engine
        .options_for_root(options_file.parent()?)
        .or_else(|| engine.options_for_root(containing_root(dir, roots)))
}

/// The longest `roots` entry that is a byte-prefix directory of `dir`, or
/// the first root (single-root workspaces are the dominant shape).
fn containing_root<'a>(dir: &Path, roots: &'a [PathBuf]) -> &'a Path {
    let mut best: Option<&Path> = None;
    for root in roots {
        if dir.starts_with(root)
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

    use dartq_diagnostics::{DiagnosticType, FileLayoutProvider as _, Severity};
    use dartq_engine::Engine;
    use dartq_engine::analysis_options::AnalysisOptions;
    use dartq_lints::rule_by_name;
    use dartq_pub::PackageConfig;
    use dartq_source::FileTable;

    use super::*;

    /// The no-options default is the `package:lints` slice — 34 core + 55
    /// recommended-only; diagnostic mirrors join only by name.
    #[test]
    fn absent_options_default_to_the_package_lints_slice() {
        assert_eq!(
            RULES
                .iter()
                .filter(|spec| matches!(spec.group, Group::Core | Group::Recommended))
                .count(),
            89,
            "the package:lints slice size changed",
        );
        let set = enable_set(None);
        for (index, spec) in RULES.iter().enumerate() {
            assert_eq!(
                set.is_enabled(RuleId(index as u16)),
                matches!(spec.group, Group::Core | Group::Recommended),
                "the default posture is exactly the package:lints slice",
            );
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

    /// Nested options refine the enable set per subtree (#595): the root's
    /// `linter.rules` run outside the nested directory, the nested file's
    /// replace them inside it — the deepest-governing resolution the
    /// engine's severity and exclude faces already apply, now at the lint
    /// face too.
    #[test]
    fn nested_options_refine_the_subtree_enable_sets() {
        let root = crate::workspace::test_fixture::workspace(
            "lints_nested",
            &[
                (
                    "analysis_options.yaml",
                    "linter:\n  rules:\n    - unnecessary_new\n",
                ),
                (
                    "lib/sub/analysis_options.yaml",
                    "linter:\n  rules:\n    - avoid_empty_else\n",
                ),
                ("lib/root_side.dart", "var x = new Object();\n"),
                (
                    "lib/sub/nested_side.dart",
                    "var y = new Object();\nvoid f(bool c) {\n  if (c) ; else ;\n}\n",
                ),
            ],
        );
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        let rows: Vec<(String, String)> = run
            .diagnostics
            .iter()
            .map(|d| {
                (
                    run.layout.file_path(d.file).to_string_lossy().into_owned(),
                    d.code.name.to_owned(),
                )
            })
            .collect();
        let fires = |file: &str, code: &str| {
            rows.iter()
                .any(|(path, name)| path.ends_with(file) && name == code)
        };
        // Outside the subtree the root's set runs: `unnecessary_new` reports.
        assert!(fires("lib/root_side.dart", "unnecessary_new"), "{rows:?}");
        // Inside it the nested set replaces the root's: its own rule
        // reports and the root's does not, despite the same `new` shape.
        assert!(
            fires("lib/sub/nested_side.dart", "avoid_empty_else"),
            "{rows:?}"
        );
        assert!(
            !fires("lib/sub/nested_side.dart", "unnecessary_new"),
            "{rows:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The pubspec pass (#434) end to end: the root `pubspec.yaml`'s
    /// insecure URLs report at their value spans (the official value-node
    /// anchor), the secure ones stay silent.
    #[test]
    fn pubspec_url_rows_report_at_the_value_span() {
        let root = crate::workspace::test_fixture::workspace("lints_pubspec_urls", &[]);
        std::fs::write(
            root.join("pubspec.yaml"),
            "name: fixture\nhomepage: http://github.com/fixture\nrepository: https://github.com/fixture\nenvironment:\n  sdk: ^3.5.0\ndependency_overrides:\n  dep:\n    git:\n      url: git://github.com/x/y.git\n",
        )
        .expect("pubspec");
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        let mut spans: Vec<(u32, u32)> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "secure_pubspec_urls")
            .map(|d| (d.range.offset, d.range.offset + d.range.length))
            .collect();
        spans.sort_unstable();
        assert_eq!(spans, vec![(24, 49), (165, 189)], "{spans:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    /// `package_names` over the same pass: a camel-case `name:` reports at
    /// its value span with the official argument fill. `fooBar` is a legal
    /// pub name, so the assembly needs the config's root entry renamed to
    /// match (a real `pub get` writes it).
    #[test]
    fn package_names_row_reports_the_camel_case_name() {
        let root = crate::workspace::test_fixture::workspace("lints_pubspec_names", &[]);
        std::fs::write(
            root.join("pubspec.yaml"),
            "name: fooBar\nenvironment:\n  sdk: ^3.5.0\n",
        )
        .expect("pubspec");
        std::fs::write(
            root.join(".dart_tool/package_config.json"),
            r#"{"configVersion": 2, "packages": [{"name": "fooBar", "rootUri": "../", "languageVersion": "3.5"}], "generator": "pub"}"#,
        )
        .expect("config");
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        let rows: Vec<(u32, u32, String)> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "package_names")
            .map(|d| {
                (
                    d.range.offset,
                    d.range.offset + d.range.length,
                    d.message.clone(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![(
                6,
                12,
                "The package name 'fooBar' isn't a lower_case_with_underscores identifier."
                    .to_owned()
            )],
            "{rows:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The YAML-face suppression (`IgnoreInfo.forYaml`): a `# ignore:`
    /// line above the `homepage:` entry drops its row.
    #[test]
    fn pubspec_ignore_line_suppresses_the_url_row() {
        let root = crate::workspace::test_fixture::workspace("lints_pubspec_ignore", &[]);
        std::fs::write(
            root.join("pubspec.yaml"),
            "name: fixture\n# ignore: secure_pubspec_urls\nhomepage: http://a.dev\nrepository: https://b.dev\nenvironment:\n  sdk: ^3.5.0\n",
        )
        .expect("pubspec");
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        assert!(
            run.diagnostics
                .iter()
                .all(|d| d.code.name != "secure_pubspec_urls"),
            "the suppressed homepage row is gone: {:?}",
            run.diagnostics
                .iter()
                .map(|d| d.code.name)
                .collect::<Vec<_>>()
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The package-aware directive rules ride the pipeline with the
    /// mounted facts: a foreign import and a foreign `src/` import report
    /// in `lib/`, the own package and the dev dependency stay silent
    /// where the official gates hold.
    #[test]
    fn package_aware_directive_rules_flow_through_the_pipeline() {
        let root = crate::workspace::test_fixture::workspace(
            "lints_pub_facts",
            &[
                (
                    "lib/a.dart",
                    "import 'package:foreign/foreign.dart';\nimport 'package:foreign/src/impl.dart';\nimport 'package:fixture/src/own.dart';\n",
                ),
                (
                    "test/t.dart",
                    "import 'package:fixture_dev/dev.dart';\nimport 'package:foreign/src/impl.dart';\n",
                ),
            ],
        );
        std::fs::write(
            root.join("pubspec.yaml"),
            "name: fixture\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\ndev_dependencies:\n  fixture_dev: ^1.0.0\n",
        )
        .expect("pubspec");
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        let rows: Vec<(String, String, u32)> = run
            .diagnostics
            .iter()
            .filter(|d| {
                matches!(
                    d.code.name.as_ref(),
                    "depend_on_referenced_packages" | "implementation_imports"
                )
            })
            .map(|d| {
                (
                    run.layout
                        .file_path(d.file)
                        .file_name()
                        .expect("name")
                        .to_string_lossy()
                        .into_owned(),
                    d.code.name.to_string(),
                    d.range.offset,
                )
            })
            .collect();
        // a.dart: the plain foreign import and the foreign `src/` import
        // both lack a dependency entry (the latter also reports as an
        // implementation import); the own package stays silent. t.dart:
        // the dev dependency is available outside the public dirs (no
        // row), `foreign` still is not, and the `src/` import is outside
        // the rule's `package:`-URI gate.
        //
        // The two a.dart rows at offset 46 tie through severity, path and
        // offset, so the oracle display order's final key decides
        // (`DiagnosticWithPath.compareTo`, dartdev analyze.dart:703-707 —
        // message text): "Import of a library…" < "The imported
        // package…".
        assert_eq!(
            rows,
            vec![
                (
                    "a.dart".to_owned(),
                    "depend_on_referenced_packages".to_owned(),
                    7
                ),
                ("a.dart".to_owned(), "implementation_imports".to_owned(), 46),
                (
                    "a.dart".to_owned(),
                    "depend_on_referenced_packages".to_owned(),
                    46
                ),
                (
                    "t.dart".to_owned(),
                    "depend_on_referenced_packages".to_owned(),
                    46
                ),
            ],
            "{rows:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
