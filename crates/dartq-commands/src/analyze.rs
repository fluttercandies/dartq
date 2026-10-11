//! The analyze face's core: the full engine pass over an assembled
//! context — workspace discovery, one barrier+recompute, and the oracle
//! display order (DiagnosticWithPath.compareTo, dartdev analyze.dart:686–
//! 707): severity, file path, position, message. Position order is the
//! UTF-16 offset (line/column are monotone in it), so no line-column
//! computation is spent on sorting.

use rustc_hash::FxHashSet;
use std::cmp::Ordering;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use dartq_diagnostics::{Diagnostic, FileLayoutProvider as _};
use dartq_engine::analysis_options;
use dartq_engine::{CancellationToken, Cancelled, Engine, EngineEvent};
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
        // engine pass already applied it to its own output).
        let lint_rows = crate::lints::analyze_lints(engine, roots);
        diagnostics.extend(engine.apply_severity_options(lint_rows));
        // The `undefined_lint` rows ride the same face: the lint registry
        // lives in dartq-lints (the engine holds no registry), so validity
        // is resolved here, at the lint stage's application point.
        let undefined_rows = undefined_lint_rows(engine, roots);
        if !undefined_rows.is_empty() {
            diagnostics.extend(engine.apply_severity_options(undefined_rows));
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

/// The `undefined_lint` rows (#457): every lint name an options file's own
/// `linter.rules` declares that the dartq-lints registry does not know
/// (`_parseLocalRules`, analysis_options_parse_model.dart:1245-1251 — the
/// registry check fires before the entry's value is read, so a disabled
/// `name: false` reports too; `name:` with a null value returns before the
/// check and does not). The engine records the candidates and their anchors
/// at load; this face validates them against `rule_by_name` and emits rows
/// attributed to the options file itself. Distinct loaded files yield once
/// (two roots sharing one options file are one parse, officially too).
fn undefined_lint_rows(engine: &Engine, roots: &[PathBuf]) -> Vec<Diagnostic> {
    let mut seen = FxHashSet::default();
    let mut rows = Vec::new();
    for root in roots {
        let Some(options) = engine.options_for_root(root) else {
            continue;
        };
        if options.declared_rules.is_empty() || !seen.insert(options.file_path.clone()) {
            continue;
        }
        let Some(file) = engine.file_table().try_file_id(&options.file_path) else {
            continue;
        };
        for rule in &options.declared_rules {
            if dartq_lints::rule_by_name(&rule.name).is_none() {
                rows.push(analysis_options::OptionsIssue::undefined_lint(rule).to_diagnostic(file));
            }
        }
    }
    rows
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
    fn diagnostic(severity: Severity, file: FileId, offset: u32, message: &str) -> Diagnostic {
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
        let messages: Vec<&str> = diagnostics.iter().map(|d| d.message.as_str()).collect();
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
                .map(|d| (d.range.offset, d.message.clone()))
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
    /// `linter.rules` names validate against the dartq-lints registry at the
    /// lint stage's application point (`_parseLocalRules`,
    /// analysis_options_parse_model.dart:1245-1251 — the check fires before
    /// the entry value is read, so a disabled `name: false` reports while a
    /// null `name:` does not), rows attribute to the options file itself at
    /// WARNING with the verbatim texts, and the layout resolves their
    /// positions through the engine's options face.
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
                    d.message.as_str(),
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
