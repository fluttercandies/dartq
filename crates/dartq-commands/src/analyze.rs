//! The analyze face's core: the full engine pass over an assembled
//! context — workspace discovery, one barrier+recompute, and the oracle
//! display order (DiagnosticWithPath.compareTo, dartdev analyze.dart:686–
//! 707): severity, file path, position, message. Position order is the
//! UTF-16 offset (line/column are monotone in it), so no line-column
//! computation is spent on sorting.

use rustc_hash::{FxHashMap, FxHashSet};
use std::cmp::Ordering;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dartq_diagnostics::{CodeInfo, Diagnostic, DiagnosticType, FileLayoutProvider as _};
use dartq_engine::analysis_options;
use dartq_engine::{CancellationToken, Cancelled, Engine, EngineEvent};
use dartq_source::{FileId, SourceRange};
use thiserror::Error;

use crate::layout::{EngineLayout, path_key, severity_rank};

/// Why the shared discover+analyze prelude of the engine-backed query faces
/// aborts (the CLI's internal tier; the daemon answers `INTERNAL_ERROR`).
#[derive(Debug, Error)]
pub enum AnalyzeError {
    /// Workspace discovery I/O failure.
    #[error("discovery failed: {0}")]
    Discovery(#[from] io::Error),
    /// The engine's cancel contract; not produced by the current callers,
    /// which pass an uncancellable token.
    #[error("analysis cancelled")]
    Cancelled,
}

/// Runs the prelude: [`Engine::discover_workspace`] then one
/// [`Engine::analyze`] pass — the read-after-write barrier every engine-
/// backed query face reflects. The faces read the analysis through the
/// collectors they installed before the pass, so the yield is only the
/// failure tier.
pub(crate) fn prelude(engine: &mut Engine) -> Result<(), AnalyzeError> {
    // No roots: the lint stage is skipped (query faces reflect the engine
    // pass only).
    run_overlaid(engine, &[], &[]).map(|_| ())
}

/// One [`Engine::analyze`] pass: the merged diagnostics, the wall-clock,
/// and the `LibraryAnalyzed` counters (fresh, cached) — [`run_overlaid`]'s
/// second half.
fn analyze_pass(
    engine: &mut Engine,
) -> Result<(Vec<Diagnostic>, Duration, usize, usize), AnalyzeError> {
    let mut analyzed = (0_usize, 0_usize);
    let mut on_event = |event: EngineEvent| {
        if let EngineEvent::LibraryAnalyzed { from_cache, .. } = event {
            if from_cache {
                analyzed.1 += 1;
            } else {
                analyzed.0 += 1;
            }
        }
    };
    let analyze_at = std::time::Instant::now();
    let diagnostics = engine
        .analyze(&[], &mut on_event, &CancellationToken::default())
        .map_err(|_: Cancelled| AnalyzeError::Cancelled)?;
    Ok((diagnostics, analyze_at.elapsed(), analyzed.0, analyzed.1))
}

/// One analyze run's result: the oracle-ordered diagnostic set, the layout
/// the writers resolve positions through, and the run facts behind
/// `--metrics-json`.
pub struct AnalyzeRun<'a> {
    /// Every live library's diagnostics, oracle display order.
    pub diagnostics: Vec<Diagnostic>,
    /// Wall-clock of the discovery and analysis passes (metrics facts).
    pub discovery: Duration,
    pub analysis: Duration,
    /// Discovered file count and total LOC ([`dartq_engine::DiscoveryStats`]).
    pub file_count: usize,
    pub loc: u64,
    /// Library counters ([`dartq_engine::EngineEvent::LibraryAnalyzed`]).
    pub libraries_analyzed: usize,
    pub libraries_from_cache: usize,
    /// Position/path resolution over the analyzed engine.
    pub layout: EngineLayout<'a>,
}

/// Runs the analyze face over an assembled engine. `roots` are the
/// workspace's package roots (the lint stage resolves per-subtree enable
/// sets beneath them).
///
/// # Errors
///
/// [`AnalyzeError`] on discovery failure or cancellation.
pub fn run<'a>(engine: &'a mut Engine, roots: &[PathBuf]) -> Result<AnalyzeRun<'a>, AnalyzeError> {
    run_overlaid(engine, &[], roots)
}

/// Runs the analyze face over an engine with overlay content applied —
/// the PRD §16.3 set-content flow, hoisted here as the one implementation
/// the CLI `--overlay` face and the daemon share (PRD §15.1's CLI face
/// shape: the per-command core in `dartq-commands` runs the engine; the
/// faces only parse, render, and tier).
///
/// The order is the contract: discovery first (it re-enqueues disk bytes
/// for every discovered file, and pending content ops are last-wins per
/// path, so overlay content must enqueue after it), then each `(path,
/// bytes)` replaces the analyzed bytes via [`Engine::set_content`] — the
/// workspace on disk is never touched — then one analyze pass, the lint
/// stage over the resident files (per-subtree enable sets via the passed
/// package roots), and the same yield as [`run`]: the oracle display order
/// plus the run facts the metrics faces report. An empty `overlays` is the
/// plain run. The callers address discovered files by their canonical
/// package-root path (the identity discovery interns); a path discovery
/// never saw is interned as a new file.
///
/// # Errors
///
/// [`AnalyzeError`] on discovery failure or cancellation.
pub fn run_overlaid<'a>(
    engine: &'a mut Engine,
    overlays: &[(PathBuf, Vec<u8>)],
    roots: &[PathBuf],
) -> Result<AnalyzeRun<'a>, AnalyzeError> {
    let started = std::time::Instant::now();
    let stats = engine
        .discover_workspace()
        .map_err(AnalyzeError::Discovery)?;
    let discovery = started.elapsed();

    for (path, bytes) in overlays {
        engine.set_content(path, bytes);
    }

    let (mut diagnostics, analysis, libraries_analyzed, libraries_from_cache) =
        analyze_pass(engine)?;
    if !roots.is_empty() {
        // The lint rows ride the same severity face as the engine rows
        // (`analyzer.errors` re-levels and filters them identically — the
        // engine pass already applied it to its own output). A one-shot
        // run shares nothing across processes: a throwaway memo.
        let lint_rows =
            crate::lints::analyze_lints(engine, roots, &mut crate::lints::LintMemo::default());
        diagnostics.extend(engine.apply_severity_options(lint_rows));
        // The `undefined_lint` and `unrecognized_error_code` rows ride the
        // same face: the lint registry lives in dartq-lints and the engine
        // holds no diagnostic-name universe either, so validity is resolved
        // here, at the lint stage's application point.
        let registry_rows = options_registry_rows(engine, roots);
        if !registry_rows.is_empty() {
            diagnostics.extend(engine.apply_severity_options(registry_rows));
        }
        // The ignore-comment removed-state rows (#1005) ride the same face.
        let removed_rows = removed_lint_use_rows(engine, roots);
        if !removed_rows.is_empty() {
            diagnostics.extend(engine.apply_severity_options(removed_rows));
        }
    }
    let layout = EngineLayout::over(engine, diagnostics.iter().map(|d| d.file));
    diagnostics.sort_by(|a, b| oracle_cmp(a, b, &layout));
    Ok(AnalyzeRun {
        diagnostics,
        discovery,
        analysis,
        file_count: stats.file_count,
        loc: stats.loc_total,
        libraries_analyzed,
        libraries_from_cache,
        layout,
    })
}

/// The options-file rows whose validity needs a registry the engine does
/// not hold (#457, #936): the initial options file's own `linter.rules`
/// names against the dartq-lints registry plus the official registered-rule
/// universe (`undefined_lint`;
/// `_parseLocalRules`, analysis_options_parse_model.dart:1245-1251 — the
/// registry check fires before the entry's value is read, so a disabled
/// `name: false` reports too; `name:` with a null value returns before the
/// check and does not), the same names' removed/deprecated registry states
/// (`removed_lint` / `deprecated_lint`; parse_model.dart:1288-1326 —
/// initial-file declarations only, the included files' rows never carry
/// them), and the initial file's own `analyzer.errors` keys
/// against the diagnostic universe plus the same lint universes
/// (`unrecognized_error_code`; parse_model:558-585 — the check fires as the
/// key is read, before the value is inspected, and a non-string scalar key
/// reports unconditionally). The engine records the candidates and their
/// anchors at load; this face validates and emits rows attributed to the
/// options file itself. Distinct loaded files yield once (two roots sharing
/// one options file are one parse, officially too).
fn options_registry_rows(engine: &Engine, roots: &[PathBuf]) -> Vec<Diagnostic> {
    let mut seen = FxHashSet::default();
    let mut rows = Vec::new();
    for root in roots {
        let Some(options) = engine.options_for_root(root) else {
            continue;
        };
        if !seen.insert(options.file_path.clone()) {
            continue;
        }
        let Some(file) = engine.file_table().try_file_id(&options.file_path) else {
            continue;
        };
        // The removed/deprecated state gate reads this root's own
        // `sdkVersionConstraint` (`PubPackage.sdkVersionConstraint`,
        // pub.dart:516-530 — the pubspec `environment.sdk` constraint; each
        // analysis context parses the options with its package's value).
        let constraint_min = sdk_constraint_min(root);
        // The include chain's own declarations wrap into
        // `included_file_warning` (the official walker reports every
        // included file's findings on the initial file, before its own
        // parse rows — `_applyParsedFiles` applies includes first).
        for group in &options.included_groups {
            for rule in &group.rules {
                if !is_known_lint_name(&rule.name) {
                    let inner = analysis_options::OptionsIssue::undefined_lint(rule);
                    rows.push(group.wrap_row(&inner).to_diagnostic(file));
                }
            }
            for error in &group.errors {
                if !analysis_options::is_known_diagnostic_code(&error.code)
                    && !is_known_lint_name(&error.code)
                {
                    let inner = analysis_options::OptionsIssue::unrecognized_error_code(error);
                    rows.push(group.wrap_row(&inner).to_diagnostic(file));
                }
            }
        }
        for rule in &options.declared_rules {
            if !is_known_lint_name(&rule.name) {
                rows.push(analysis_options::OptionsIssue::undefined_lint(rule).to_diagnostic(file));
                continue;
            }
            // The removed/deprecated rows (#988): the official `parseRule`
            // reports them for every initial-file name the registry
            // resolves — enabled or disabled, the state check rides the
            // parse, not the entry's value (parse_model.dart:1288-1326) —
            // anchored at the name token like `undefined_lint`, and gated
            // on the package's SDK-constraint lower bound.
            if let Some(id) = dartq_lints::rule_by_name(&rule.name) {
                let spec = &dartq_lints::RULES[id.0 as usize];
                if spec.group == dartq_lints::Group::Removed
                    && let Some(since) = crate::rule_states::removed_since(spec.name)
                    && crate::rule_states::before_current(since, constraint_min)
                {
                    rows.push(
                        analysis_options::OptionsIssue {
                            code: analysis_options::REMOVED_LINT,
                            message: spec.render_problem(&[
                                &rule.name,
                                crate::rule_states::since_arg(since),
                            ]),
                            correction: spec.render_correction(&[&rule.name]),
                            offset: rule.offset,
                            length: rule.length,
                        }
                        .to_diagnostic(file),
                    );
                }
            } else if let Some(since) = crate::rule_states::deprecated_since(&rule.name)
                && crate::rule_states::before_current(Some(since), constraint_min)
            {
                // The deprecated rules are unimplemented in dartq-lints, so
                // no spec carries their message of record — the registry
                // texts render here (diagnostic.g.dart:3904-3913; no
                // `replacedBy` in the pinned set, so the
                // `deprecated_lint_with_replacement` variant never applies).
                rows.push(
                    analysis_options::OptionsIssue {
                        code: analysis_options::DEPRECATED_LINT,
                        message: format!(
                            "The lint rule '{}' is deprecated and shouldn't be enabled.",
                            rule.name
                        ),
                        correction: Some(format!("Try removing '{}'.", rule.name)),
                        offset: rule.offset,
                        length: rule.length,
                    }
                    .to_diagnostic(file),
                );
            }
        }
        for error in &options.declared_errors {
            if !analysis_options::is_known_diagnostic_code(&error.code)
                && !is_known_lint_name(&error.code)
            {
                rows.push(
                    analysis_options::OptionsIssue::unrecognized_error_code(error)
                        .to_diagnostic(file),
                );
            }
        }
    }
    rows
}

/// The comment faces' scan posture — the same configuration the dartq-lints
/// bridge scans every unit with (Dart 3.15: triple shift and augmentations
/// open), so the enumeration's comment lexemes match the lint stage's.
const SCAN_CONFIG: dartq_scanner::ScanConfig = dartq_scanner::ScanConfig {
    enable_triple_shift: true,
    enable_augmentations: true,
};

/// `'{0}' was removed in Dart '{1}'` (registry:15119-15128,
/// `STATIC_WARNING` → WARNING, published docs). The ignore-comment face's
/// code: a comment naming a removed rule reports at the name token.
const REMOVED_LINT_USE: CodeInfo = CodeInfo::new(
    "removed_lint_use",
    "REMOVED_LINT_USE",
    DiagnosticType::StaticWarning,
    true,
);

/// The `removed_lint_use` rows (#1005): an ignore comment naming a removed
/// rule reports at its name token (`IgnoreValidator`.
/// `_reportUnnecessaryOrRemovedOrDeprecatedIgnores`, ignore_validator.dart
/// :199-240), gated on the file's governing options enabling the
/// `unnecessary_ignore` lint (`_validateUnnecessaryIgnores`,
/// ignore_validator.dart:203 — the validator's caller wires the lint's
/// enablement through; the registry lookup matches case-insensitively).
/// The enumeration arrives with the official exact-duplicate removal
/// applied (`namesIgnoredForFile` / `namedIgnoredOnLine`, :82-132); the
/// `cannot-ignore` names leave here — the validator's unignorable branch
/// removes them from the list before this walk (:89, :113, :164-174). No
/// SDK-constraint gate: the validator renders `state.since` verbatim
/// (`null` for the unversioned state) where the options-file `removed_lint`
/// face applies `_beforeCurrentConstraint` — and the replaced-by variant
/// never applies: no pinned removed rule carries one. The validator's
/// reported-diagnostic removal (:137-144) cannot reach this face either:
/// no removed rule's name is a diagnostic code of the pinned SDK, so no
/// reported error ever carries one.
fn removed_lint_use_rows(engine: &Engine, roots: &[PathBuf]) -> Vec<Diagnostic> {
    let mut files: Vec<FileId> = engine
        .libraries()
        .into_iter()
        .flat_map(|(_, files)| files)
        .collect();
    files.sort_unstable();
    files.dedup();
    // Governance is by ancestry, so one directory always maps to one
    // governing entry; the memo keeps the options walk off the per-file
    // hot path.
    let mut governing: FxHashMap<PathBuf, Option<&analysis_options::AnalysisOptions>> =
        FxHashMap::default();
    let mut rows = Vec::new();
    for file in files {
        let Some(path) = engine.file_table().path(file) else {
            continue;
        };
        let dir = path.parent().unwrap_or(Path::new(""));
        let options = *governing
            .entry(dir.to_path_buf())
            .or_insert_with(|| crate::lints::governing_options(engine, dir, roots));
        let Some(options) = options else {
            continue;
        };
        if !options
            .lint_rules
            .iter()
            .any(|name| name.eq_ignore_ascii_case("unnecessary_ignore"))
        {
            continue;
        }
        let unignorable = options.unignorable();
        let Some(bytes) = engine.content(file) else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(bytes) else {
            continue;
        };
        let scanned = dartq_scanner::scan(text, SCAN_CONFIG, file);
        let comments = dartq_lints::Comments::extract(text, &scanned.tokens);
        for (name, offset, length) in dartq_lints::ignored_name_entries(text, &comments) {
            if unignorable.contains(&name) {
                continue;
            }
            let Some(id) = dartq_lints::rule_by_name(&name) else {
                continue;
            };
            let spec = &dartq_lints::RULES[id.0 as usize];
            if spec.group != dartq_lints::Group::Removed {
                continue;
            }
            let since = crate::rule_states::removed_since(spec.name).flatten();
            rows.push(Diagnostic::new(
                REMOVED_LINT_USE.dtype.severity(),
                REMOVED_LINT_USE,
                format!(
                    "'{}' was removed in Dart '{}'",
                    name,
                    crate::rule_states::since_arg(since)
                ),
                file,
                SourceRange { offset, length },
                Some(format!("Remove the reference to '{}'.", name)),
            ));
        }
    }
    rows
}

/// Whether `name` names a lint rule the official validity faces accept: a
/// rule dartq-lints implements (`rule_by_name`), or any other registered
/// rule of the pinned SDK's linter registry (`currentLintCodeNames`,
/// parse_model.dart:75-83 — the full `Registry.ruleRegistry`, not the
/// implemented subset; probed 3.13.1: `avoid_print` in `analyzer.errors`
/// yields no row). #956.
fn is_known_lint_name(name: &str) -> bool {
    dartq_lints::rule_by_name(name).is_some() || crate::lint_names::is_known(name)
}

/// The lower bound of the package at `root`'s `environment.sdk` constraint —
/// the `sdkVersionConstraint` the official removed/deprecated state gate
/// switches over (`_beforeCurrentConstraint`, parse_model.dart:1197-1205:
/// only a `VersionRange` carries a min, so an exact-version or `any`
/// constraint gates every versioned since off). `None` — no pubspec, an
/// unreadable one, or no parseable `sdk` key — is the official null
/// constraint.
fn sdk_constraint_min(root: &std::path::Path) -> Option<(u64, u64, u64)> {
    let text = std::fs::read_to_string(root.join("pubspec.yaml")).ok()?;
    let spec = dartq_pub::Pubspec::parse(&text).ok()?;
    let dartq_pub::VersionConstraint::Range(range) = spec.environment.sdk? else {
        return None;
    };
    let version = range.min?;
    Some((version.major, version.minor, version.patch))
}

#[cfg(test)]
mod tests {
    use dartq_diagnostics::{CodeInfo, DiagnosticType, FileId, Severity};
    use dartq_engine::Engine;
    use dartq_source::SourceRange;

    use super::*;
    use crate::workspace::{engine_config, test_fixture::workspace};

    /// A hand-built diagnostic, identified by its message in the
    /// assertions; the engine never analyzes it.
    fn diagnostic(
        severity: Severity,
        file: FileId,
        offset: u32,
        message: &'static str,
    ) -> Diagnostic {
        Diagnostic::new(
            severity,
            CodeInfo::new(
                "undefined_function",
                "UNDEFINED_FUNCTION",
                DiagnosticType::CompileTimeError,
                false,
            ),
            message,
            file,
            SourceRange { offset, length: 1 },
            None,
        )
    }

    /// The oracle-sort pin: severity, path bytes, offset, message, with a
    /// crafted pair inverting under each adjacent key. The input arrives in
    /// exact reverse of the expected output, so a dropped key leaves a
    /// visible misorder: `omega` carries the smallest path and offset and
    /// only its severity demotes it, `delta`'s smaller offset only path
    /// bytes can outrank, `gamma`'s offset must overtake `beta`'s, and
    /// `beta` must fall behind `alpha` on the message tiebreak.
    #[test]
    fn oracle_sort_pins_the_four_key_order() {
        let root = workspace("analyze_sort", &[]);
        let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        // Paths interned directly (the daemon's set-content flow, applied
        // so the layout can read text and line index) — the byte order of
        // `lib/a.dart` vs `lib/b.dart` is the second sort key.
        let a = engine.set_content(&root.join("lib/a.dart"), b"// a\n");
        let b = engine.set_content(&root.join("lib/b.dart"), b"// b\n");
        engine.apply_pending(&mut |_| {});
        let layout = EngineLayout::over(&engine, [a, b]);

        let mut diagnostics = vec![
            diagnostic(Severity::Warning, a, 0, "omega"),
            diagnostic(Severity::Error, b, 5, "delta"),
            diagnostic(Severity::Error, a, 20, "gamma"),
            diagnostic(Severity::Error, a, 10, "beta"),
            diagnostic(Severity::Error, a, 10, "alpha"),
        ];
        diagnostics.sort_by(|left, right| oracle_cmp(left, right, &layout));
        let messages: Vec<&str> = diagnostics.iter().map(|d| d.message.as_ref()).collect();
        assert_eq!(messages, ["alpha", "beta", "gamma", "delta", "omega"]);

        std::fs::remove_dir_all(&root).ok();
    }

    /// The overlay run's contract (PRD §16.3): an empty overlay list is the
    /// plain run, an overlay replaces the discovered bytes for the analyzed
    /// pass only (the workspace on disk stays broken), and the yield keeps
    /// the run facts plus the oracle display order — the semantic row
    /// (offset 11) precedes the syntax row (offset 13) inside one library.
    #[test]
    fn run_overlaid_replaces_discovered_bytes_for_one_run() {
        let root = workspace(
            "analyze_overlay",
            &[("lib/broken.dart", "void f() { x }\n")],
        );
        // Discovery interns canonical package-root paths; the overlay must
        // address that identity (the CLI's spec decoding canonicalizes for
        // the same reason).
        let root = root.canonicalize().expect("canonical root");
        let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let target = root.join("lib/broken.dart");
        // Each run borrows the engine through its layout, so every run's
        // yield is reduced to owned data inside its own scope.
        let hits = |diag: &[Diagnostic]| -> Vec<(u32, String)> {
            diag.iter()
                .map(|d| (d.range.offset, d.message.clone().into_owned()))
                .collect()
        };

        // Plain run over the discovered bytes: the on-disk error surfaces,
        // and an empty overlay list runs the same pipeline.
        let (plain_hits, plain_facts) = {
            let plain = run(&mut engine, std::slice::from_ref(&root)).expect("plain run");
            let hits = hits(&plain.diagnostics);
            assert!(
                hits.iter().any(|(_, m)| m.contains("'x'")),
                "the discovered bytes are analyzed: {hits:?}"
            );
            (hits, (plain.file_count, plain.loc))
        };
        let (bare_hits, bare_facts) = {
            let bare =
                run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("bare run");
            (hits(&bare.diagnostics), (bare.file_count, bare.loc))
        };
        assert_eq!(bare_hits, plain_hits, "no overlays: the plain run");
        assert_eq!(bare_facts, plain_facts);

        // A clean overlay silences it for one run; the workspace on disk is
        // never touched, and the run facts keep flowing.
        let overlays = vec![(target.clone(), b"var ok = 1;\n".to_vec())];
        let fixed_facts = {
            let fixed = run_overlaid(&mut engine, &overlays, std::slice::from_ref(&root))
                .expect("overlaid run");
            assert!(
                fixed.diagnostics.is_empty(),
                "clean overlay: no diagnostics"
            );
            (fixed.file_count, fixed.loc)
        };
        assert_eq!(
            std::fs::read_to_string(&target).expect("target intact"),
            "void f() { x }\n",
            "the overlay never rewrites the workspace"
        );
        assert_eq!(fixed_facts, plain_facts);

        // A broken overlay reports against the analyzed path in oracle
        // order. Both rows sit at offset 11 (`y`), so the message
        // tie-break orders them; the spans themselves are the parser
        // lane's oracle-conformant output, pinned here only to exercise
        // the comparator (DiagnosticWithPath.compareTo,
        // dartdev analyze.dart:686-707).
        let overlays = vec![(target, b"void g() { y }\n".to_vec())];
        let broken_hits = {
            let broken = run_overlaid(&mut engine, &overlays, std::slice::from_ref(&root))
                .expect("overlaid run");
            hits(&broken.diagnostics)
        };
        assert_eq!(
            broken_hits,
            vec![
                (11, "Expected to find ';'.".to_owned()),
                (11, "Undefined name 'y'.".to_owned()),
            ],
            "oracle order inside one library: severity, path, offset, message"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The commands output wiring: the lint rows ride the same severity
    /// face as the engine rows — `run_overlaid` applies the governing
    /// options' `errors` overrides to them — so an option re-level surfaces
    /// in the oracle-ordered set and an `ignore` filter drops the engine
    /// row (`computeSeverity`, analyzer_cli error_severity.dart:14-26).
    #[test]
    fn run_overlaid_applies_errors_overrides_to_output_rows() {
        let root = workspace(
            "analyze_severity",
            &[("lib/bad.dart", "var x = new Object();\nvar ok = unknown;\n")],
        );
        let root = root.canonicalize().expect("canonical root");
        std::fs::write(
            root.join("analysis_options.yaml"),
            concat!(
                "analyzer:\n",
                "  errors:\n",
                "    unnecessary_new: error\n",
                "    undefined_identifier: ignore\n",
                "linter:\n",
                "  rules:\n",
                "    - unnecessary_new\n",
            ),
        )
        .expect("options");
        let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");
        let rows: Vec<(String, Severity)> = run
            .diagnostics
            .iter()
            .map(|d| (d.code.name.to_owned(), d.severity))
            .collect();
        // The lint row (INFO by rule shape) re-leveled to ERROR by the
        // options override.
        assert!(
            rows.contains(&("unnecessary_new".to_owned(), Severity::Error)),
            "the lint row rides the severity face: {rows:?}"
        );
        // The engine-pass row filtered out by the options override.
        assert!(
            rows.iter().all(|(code, _)| code != "undefined_identifier"),
            "the ignore filter drops the engine row: {rows:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The `undefined_lint` face end to end (#457): the options file's own
    /// `linter.rules` names validate against the lint universes at the
    /// lint stage's application point (`_parseLocalRules`,
    /// analysis_options_parse_model.dart:1245-1251 — the check fires before
    /// the entry value is read, so a disabled `name: false` reports while a
    /// null `name:` does not), rows attribute to the options file itself at
    /// WARNING with the verbatim texts, and the layout resolves their
    /// positions through the engine's options face. A
    /// registered-but-unimplemented rule (`avoid_print`, #956) joins the
    /// known side: no row.
    #[test]
    fn run_overlaid_reports_undefined_lint_rows() {
        let root = workspace(
            "analyze_undefined_lint",
            &[("lib/ok.dart", "var ok = 1;\n")],
        );
        let root = root.canonicalize().expect("canonical root");
        let options_text = concat!(
            "linter:\n",
            "  rules:\n",
            "    avoid_empty_else: true\n",
            "    avoid_print: true\n",
            "    not_a_lint: false\n",
            "    unknown_one: false\n",
            "    null_rule:\n",
        );
        std::fs::write(root.join("analysis_options.yaml"), options_text).expect("options");
        let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");

        let options_path = root.join("analysis_options.yaml");
        let expected_path = options_path.to_string_lossy().into_owned();
        let rows: Vec<(String, &str, Severity, u32)> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "undefined_lint")
            .map(|d| {
                (
                    run.layout.file_path(d.file).to_string_lossy().into_owned(),
                    d.message.as_ref(),
                    d.severity,
                    d.range.offset,
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                (
                    expected_path.clone(),
                    "'not_a_lint' isn't a recognized lint rule.",
                    Severity::Warning,
                    u32::try_from(options_text.find("not_a_lint").expect("anchor")).expect("u32"),
                ),
                (
                    expected_path,
                    "'unknown_one' isn't a recognized lint rule.",
                    Severity::Warning,
                    u32::try_from(options_text.find("unknown_one").expect("anchor")).expect("u32"),
                ),
            ],
            "a disabled name reports, a null name does not, a known name never: {rows:?}"
        );
        for row in run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "undefined_lint")
        {
            assert_eq!(row.code.machine, "UNDEFINED_LINT");
            assert_eq!(row.code.dtype, DiagnosticType::StaticWarning);
            assert_eq!(
                row.correction.as_deref(),
                Some("Try using the name of a recognized lint rule.")
            );
        }
        std::fs::remove_dir_all(&root).ok();
    }

    /// The `unrecognized_error_code` face end to end (#936): the options
    /// file's own `analyzer.errors` keys validate against the diagnostic
    /// universe plus the lint universes at the lint stage's application
    /// point (parse_model:558-585 — the check fires as the key is read, so
    /// the value's action never matters). An unknown code reports at its
    /// key anchor, WARNING, no correction; a registered-but-unimplemented
    /// lint name (`avoid_print`, the #956 boundary — the oracle accepts it
    /// silently, probed 3.13.1), a removed-code compat name, a known
    /// diagnostic, and a non-string scalar key (`'123'`, reported
    /// unconditionally) pin the membership arms.
    #[test]
    fn run_overlaid_reports_unrecognized_error_code_rows() {
        let root = workspace(
            "analyze_unrecognized_error_code",
            &[("lib/ok.dart", "var ok = 1;\n")],
        );
        let root = root.canonicalize().expect("canonical root");
        let options_text = concat!(
            "analyzer:\n",
            "  errors:\n",
            "    not_a_real_code: ignore\n",
            "    avoid_print: ignore\n",
            "    missing_return: ignore\n",
            "    undefined_identifier: ignore\n",
            "    123: ignore\n",
        );
        std::fs::write(root.join("analysis_options.yaml"), options_text).expect("options");
        let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");

        let expected_path = root
            .join("analysis_options.yaml")
            .to_string_lossy()
            .into_owned();
        let rows: Vec<(String, &str, Severity, u32, u32)> = run
            .diagnostics
            .iter()
            .map(|d| {
                (
                    run.layout.file_path(d.file).to_string_lossy().into_owned(),
                    d.message.as_ref(),
                    d.severity,
                    d.range.offset,
                    d.range.length,
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                (
                    expected_path.clone(),
                    "'not_a_real_code' isn't a recognized diagnostic code.",
                    Severity::Warning,
                    u32::try_from(options_text.find("not_a_real_code").expect("anchor"))
                        .expect("u32"),
                    15,
                ),
                (
                    expected_path,
                    "'123' isn't a recognized diagnostic code.",
                    Severity::Warning,
                    u32::try_from(options_text.find("123").expect("anchor")).expect("u32"),
                    3,
                ),
            ],
            "only the unknown code name and the non-string scalar report: {rows:?}"
        );
        for row in run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "unrecognized_error_code")
        {
            assert_eq!(row.code.machine, "UNRECOGNIZED_ERROR_CODE");
            assert_eq!(row.code.dtype, DiagnosticType::StaticWarning);
            assert_eq!(row.correction, None);
        }
        std::fs::remove_dir_all(&root).ok();
    }

    /// The `removed_lint` face end to end (#988): a removed rule named
    /// directly in the initial options file reports at its name anchor with
    /// the registry's removed-spec texts verbatim (`parseRule`,
    /// parse_model.dart:1308-1326), gated on the package's SDK-constraint
    /// lower bound (`_beforeCurrentConstraint`, parse_model.dart:1197-1205 —
    /// the fixture's `^3.5.0` admits the 3.0.0 removals and holds back the
    /// 3.7.0 ones), and a normal name stays silent.
    #[test]
    fn run_overlaid_reports_removed_lint_rows() {
        let root = workspace(
            "analyze_removed_lint",
            &[("lib/ok.dart", "var ok = 1;\n")],
        );
        let root = root.canonicalize().expect("canonical root");
        let options_text = concat!(
            "linter:\n",
            "  rules:\n",
            "    - super_goes_last\n",
            "    - unsafe_html\n",
            "    - avoid_empty_else\n",
        );
        std::fs::write(root.join("analysis_options.yaml"), options_text).expect("options");
        let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");

        let rows: Vec<(&str, &str, u32, u32)> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "removed_lint")
            .map(|d| {
                (
                    d.message.as_ref(),
                    d.correction.as_deref().expect("correction"),
                    d.range.offset,
                    d.range.length,
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![(
                "'super_goes_last' was removed in Dart '3.0.0'",
                "Try removing the reference to 'super_goes_last'.",
                u32::try_from(options_text.find("super_goes_last").expect("anchor")).expect("u32"),
                u32::try_from("super_goes_last".len()).expect("u32"),
            )],
            "only the removal the constraint admits reports: {rows:?}"
        );
        for row in run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "removed_lint")
        {
            assert_eq!(row.code.machine, "REMOVED_LINT");
            assert_eq!(row.code.dtype, DiagnosticType::StaticWarning);
        }
        std::fs::remove_dir_all(&root).ok();
    }

    /// The `deprecated_lint` face end to end (#988): a deprecated rule
    /// (registered in the SDK, unimplemented in dartq-lints — no
    /// `undefined_lint`) reports with the registry texts
    /// (diagnostic.g.dart:3904-3913) once the package's constraint reaches
    /// the deprecation's since (3.11.0; the fixture's `^3.5.0` gates it off,
    /// parse_model.dart:1288-1296).
    #[test]
    fn run_overlaid_reports_deprecated_lint_rows() {
        let root = workspace(
            "analyze_deprecated_lint",
            &[("lib/ok.dart", "var ok = 1;\n")],
        );
        std::fs::write(
            root.join("pubspec.yaml"),
            "name: fixture\nversion: 0.1.0\nenvironment:\n  sdk: ^3.11.0\n",
        )
        .expect("pubspec");
        let root = root.canonicalize().expect("canonical root");
        let options_text = "linter:\n  rules:\n    - prefer_final_parameters\n";
        std::fs::write(root.join("analysis_options.yaml"), options_text).expect("options");
        let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");

        let rows: Vec<(&str, Option<&str>, u32, u32)> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "deprecated_lint")
            .map(|d| {
                (
                    d.message.as_ref(),
                    d.correction.as_deref(),
                    d.range.offset,
                    d.range.length,
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![(
                "The lint rule 'prefer_final_parameters' is deprecated and shouldn't be enabled.",
                Some("Try removing 'prefer_final_parameters'."),
                u32::try_from(options_text.find("prefer_final_parameters").expect("anchor"))
                    .expect("u32"),
                u32::try_from("prefer_final_parameters".len()).expect("u32"),
            )],
            "{rows:?}"
        );
        let row = run
            .diagnostics
            .iter()
            .find(|d| d.code.name == "deprecated_lint")
            .expect("the row above");
        assert_eq!(row.code.machine, "DEPRECATED_LINT");
        assert_eq!(row.code.dtype, DiagnosticType::StaticWarning);
        std::fs::remove_dir_all(&root).ok();
    }

    /// The dominant posture: normal rule names — implemented or
    /// registered-but-unimplemented — carry no removed/deprecated rows
    /// (#988's zero branch).
    #[test]
    fn normal_rule_names_carry_no_removed_or_deprecated_rows() {
        let root = workspace(
            "analyze_normal_lints",
            &[("lib/ok.dart", "var ok = 1;\n")],
        );
        let root = root.canonicalize().expect("canonical root");
        std::fs::write(
            root.join("analysis_options.yaml"),
            "linter:\n  rules:\n    - avoid_empty_else\n    - avoid_print\n",
        )
        .expect("options");
        let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");
        assert!(
            run.diagnostics
                .iter()
                .all(|d| d.code.name != "removed_lint" && d.code.name != "deprecated_lint"),
            "no state rows for normal names: {:?}",
            run.diagnostics.iter().map(|d| d.code.name).collect::<Vec<_>>()
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The `removed_lint_use` face end to end (#1005): with the governing
    /// options enabling `unnecessary_ignore`, an ignore comment naming a
    /// removed rule reports at its name token with the registry texts
    /// (`_reportUnnecessaryOrRemovedOrDeprecatedIgnores`,
    /// ignore_validator.dart:199-240) — the as-written spelling in the
    /// message, the since verbatim, and no SDK-constraint gate
    /// (`unsafe_html`'s 3.7.0 removal reports under the fixture's ^3.5.0
    /// package), the same name on two different lines reporting twice (the
    /// duplicate walk is scope-aware, :105-126). The gate arms: off without
    /// the lint, case-insensitive with it, off under the `name: false`
    /// spelling that drops the name from the merged rule list.
    #[test]
    fn run_overlaid_reports_removed_lint_use_rows() {
        let source = concat!(
            "var a = 1; // ignore: super_goes_last, unsafe_html\n",
            "var b = 2; // ignore: super_goes_last\n",
        );
        let root = workspace("analyze_removed_lint_use", &[("lib/a.dart", source)]);
        let root = root.canonicalize().expect("canonical root");
        let rows_for = |options: &str| -> Vec<(u32, u32, String, String)> {
            std::fs::write(root.join("analysis_options.yaml"), options).expect("options");
            let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
            let mut engine = Engine::new(config).expect("engine");
            let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");
            run.diagnostics
                .iter()
                .filter(|d| d.code.name == "removed_lint_use")
                .map(|d| {
                    (
                        d.range.offset,
                        d.range.length,
                        d.message.clone().into_owned(),
                        d.correction.clone().expect("correction"),
                    )
                })
                .collect()
        };

        assert_eq!(
            rows_for("linter:\n  rules:\n    - unnecessary_ignore\n"),
            vec![
                (
                    22,
                    15,
                    "'super_goes_last' was removed in Dart '3.0.0'".to_owned(),
                    "Remove the reference to 'super_goes_last'.".to_owned(),
                ),
                (
                    39,
                    11,
                    "'unsafe_html' was removed in Dart '3.7.0'".to_owned(),
                    "Remove the reference to 'unsafe_html'.".to_owned(),
                ),
                (
                    73,
                    15,
                    "'super_goes_last' was removed in Dart '3.0.0'".to_owned(),
                    "Remove the reference to 'super_goes_last'.".to_owned(),
                ),
            ],
            "both lines' entries report, the gate's lint enabled: {:?}",
            rows_for("linter:\n  rules:\n    - unnecessary_ignore\n"),
        );
        assert!(
            rows_for("linter:\n  rules:\n    - avoid_empty_else\n").is_empty(),
            "gate off: no rows"
        );
        assert_eq!(
            rows_for("linter:\n  rules:\n    - UNNECESSARY_IGNORE\n").len(),
            3,
            "the gate matches case-insensitively"
        );
        assert!(
            rows_for("linter:\n  rules:\n    unnecessary_ignore: false\n").is_empty(),
            "the disable spelling drops the name from the merged list"
        );

        // The row shape: WARNING, the registry code identity, attributed to
        // the dart file the comment lives in.
        std::fs::write(
            root.join("analysis_options.yaml"),
            "linter:\n  rules:\n    - unnecessary_ignore\n",
        )
        .expect("options");
        let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");
        let row = run
            .diagnostics
            .iter()
            .find(|d| d.code.name == "removed_lint_use")
            .expect("the rows above");
        assert_eq!(row.code.machine, "REMOVED_LINT_USE");
        assert_eq!(row.code.dtype, DiagnosticType::StaticWarning);
        assert_eq!(row.severity, Severity::Warning);
        assert!(row.code.has_published_docs);
        assert!(
            run.layout.file_path(row.file).ends_with("a.dart"),
            "rows attribute to the dart file: {}",
            run.layout.file_path(row.file).to_string_lossy()
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The face's filters (#1005): a `cannot-ignore` name never reaches the
    /// removed-state walk (the validator removes unignorable names first,
    /// ignore_validator.dart:89, :164-174 — the `unignorable_ignore` rows
    /// themselves are the lints stage's face), an exact duplicate reports
    /// once, first occurrence winning (:91), and a non-removed rule's name
    /// resolves through the registry without a row (:214-240 — only
    /// `state.isRemoved` reports).
    #[test]
    fn removed_lint_use_filters_unignorable_and_duplicate_names() {
        let source =
            "var a = 1; // ignore: super_goes_last, super_goes_last, avoid_empty_else\n";
        let root = workspace(
            "analyze_removed_lint_use_filters",
            &[("lib/a.dart", source)],
        );
        let root = root.canonicalize().expect("canonical root");
        let offsets_for = |options: &str| -> Vec<u32> {
            std::fs::write(root.join("analysis_options.yaml"), options).expect("options");
            let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
            let mut engine = Engine::new(config).expect("engine");
            let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");
            run.diagnostics
                .iter()
                .filter(|d| d.code.name == "removed_lint_use")
                .map(|d| d.range.offset)
                .collect()
        };

        // The cannot-ignore claim silences the removed-state face entirely.
        assert!(offsets_for(concat!(
            "analyzer:\n",
            "  cannot-ignore:\n",
            "    - super_goes_last\n",
            "linter:\n",
            "  rules:\n",
            "    - unnecessary_ignore\n",
        ))
        .is_empty());

        // Without it the duplicate collapses to its first occurrence and the
        // non-removed neighbor stays silent.
        assert_eq!(
            offsets_for("linter:\n  rules:\n    - unnecessary_ignore\n"),
            vec![22],
            "first occurrence wins, only removed names report"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}

/// One comparison of the oracle display order (DiagnosticWithPath.compareTo,
/// dartdev analyze.dart:686-707): severity, file path, position, message.
/// Position order is the UTF-16 offset (line/column are monotone in it), so
/// no line-column computation is spent on sorting.
fn oracle_cmp(a: &Diagnostic, b: &Diagnostic, layout: &EngineLayout<'_>) -> Ordering {
    severity_rank(a.severity)
        .cmp(&severity_rank(b.severity))
        .then_with(|| path_key(layout.file_path(a.file)).cmp(path_key(layout.file_path(b.file))))
        .then_with(|| a.range.offset.cmp(&b.range.offset))
        .then_with(|| a.message.cmp(&b.message))
}
