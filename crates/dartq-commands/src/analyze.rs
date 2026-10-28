//! The analyze face's core: the full engine pass over an assembled
//! context — workspace discovery, one barrier+recompute, and the oracle
//! display order (DiagnosticWithPath.compareTo, dartdev analyze.dart:686–
//! 707): severity, file path, position, message. Position order is the
//! UTF-16 offset (line/column are monotone in it), so no line-column
//! computation is spent on sorting.

use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use std::cmp::Ordering;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dartq_diagnostics::{
    AnalysisOutput, CodeInfo, Diagnostic, DiagnosticType, FileLayoutProvider as _, Severity,
};
use dartq_engine::analysis_options;
use dartq_engine::{CancellationToken, Cancelled, Engine, EngineEvent};
use dartq_source::{FileId, FileTable, SourceRange};
use thiserror::Error;

use crate::layout::{EngineLayout, path_key, severity_rank};
use crate::plugins::PluginSet;

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
/// the `LibraryAnalyzed` counters (fresh, cached), and the version-skew
/// degrade notices (#1311) — [`run_overlaid`]'s second half.
fn analyze_pass(
    engine: &mut Engine,
) -> Result<(Vec<Diagnostic>, Duration, usize, usize, Vec<String>), AnalyzeError> {
    let mut analyzed = (0_usize, 0_usize);
    let mut notices: Vec<String> = Vec::new();
    let mut on_event = |event: EngineEvent| match event {
        EngineEvent::LibraryAnalyzed { from_cache, .. } => {
            if from_cache {
                analyzed.1 += 1;
            } else {
                analyzed.0 += 1;
            }
        }
        EngineEvent::ExperimentDegrade {
            sdk_version,
            unknown,
            ..
        } => notices.push(degrade_notice(&sdk_version, &unknown)),
        EngineEvent::SourceRejected { path, .. } => {
            notices.push(rejection_notice(&path));
        }
        _ => {}
    };
    let analyze_at = std::time::Instant::now();
    let diagnostics = engine
        .analyze(&[], &mut on_event, &CancellationToken::default())
        .map_err(|_: Cancelled| AnalyzeError::Cancelled)?;
    Ok((
        diagnostics,
        analyze_at.elapsed(),
        analyzed.0,
        analyzed.1,
        notices,
    ))
}

/// The one-line human notice for an
/// [`dartq_engine::EngineEvent::ExperimentDegrade`] event (#1311):
/// `note: SDK <version> names unknown experiment(s) <names>; granting
/// known-only`. Warning-tier engine telemetry for the faces' human streams
/// (stderr) — never a diagnostic row, so it stays out of the machine rows,
/// the JSON envelopes, and the parity-caliber result sets. The event's
/// `granted` payload is always
/// [`dartq_engine::event::KNOWN_ONLY_GRANT`] (the only
/// fallback posture), rendered here as its human spelling. Bounded by the
/// engine: one event per unknown experiment name per engine.
pub(crate) fn degrade_notice(sdk_version: &str, unknown: &[String]) -> String {
    format!(
        "note: SDK {sdk_version} names unknown experiment(s) {}; granting known-only",
        unknown.join(", ")
    )
}

/// The one-line human notice for a span-capacity rejection (#1543): the
/// discovery face's `DiscoveryStats::oversized` rows and the
/// [`dartq_engine::EngineEvent::SourceRejected`] events — one line each,
/// `note: skipped <path>: ...`. Warning-tier telemetry for the faces'
/// human streams (stderr), the same channel as [`degrade_notice`] — never
/// a diagnostic row, so it stays out of the machine rows, the JSON
/// envelopes, and the parity-caliber result sets. The rejected file is
/// excluded from the analysis; the rest of the run is unaffected.
pub(crate) fn rejection_notice(path: &Path) -> String {
    format!(
        "note: skipped {}: source exceeds the {}-byte analysis capacity",
        path.display(),
        dartq_source::MAX_SOURCE_BYTES,
    )
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
    /// Wall-clock of the lint stage — the `analyze_lints` pass plus the
    /// options registry/conflict/removed rows that ride it — captured
    /// around the lint block, after the engine pass (never interleaved
    /// with `analysis`, so the two never double-count).
    pub lints: Duration,
    /// Discovered file count and total LOC ([`dartq_engine::DiscoveryStats`]).
    pub file_count: usize,
    pub loc: u64,
    /// Library counters ([`dartq_engine::EngineEvent::LibraryAnalyzed`]).
    pub libraries_analyzed: usize,
    pub libraries_from_cache: usize,
    /// One rendered warning-tier notice per telemetry event —
    /// [`degrade_notice`] per
    /// [`dartq_engine::EngineEvent::ExperimentDegrade`] (#1311),
    /// [`rejection_notice`] per span-capacity rejection (#1543: the
    /// discovery face's oversized rows and
    /// [`dartq_engine::EngineEvent::SourceRejected`] events). Warning-tier
    /// telemetry the faces print on their human streams (stderr). Never a
    /// diagnostic row: it joins no severity tier, no machine/JSON face,
    /// and no metrics count.
    pub degrade_notices: Vec<String>,
    /// The engine's memory posture at the run's end (the residency peak):
    /// the budget ladder's state and last boundary sample, plus the warm
    /// lane's counters — the `--memory-report` face's data, captured
    /// where the run's own telemetry prints.
    pub memory: (dartq_engine::MemoryStatus, dartq_engine::WarmStats),
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
/// the set-content flow, hoisted here as the one implementation
/// the CLI `--overlay` face and the daemon share (CLI face
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
    // Discovery's span-capacity rejections ride the notice face (one line
    // per rejected file, stderr at render time — never a diagnostic row).
    let mut notices: Vec<String> = stats
        .oversized
        .iter()
        .map(|path| rejection_notice(path))
        .collect();

    for (path, bytes) in overlays {
        engine.set_content(path, bytes);
    }

    let (mut diagnostics, analysis, libraries_analyzed, libraries_from_cache, mut event_notices) =
        analyze_pass(engine)?;
    notices.append(&mut event_notices);
    let lints_at = std::time::Instant::now();
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
        let loaded = loaded_options(engine, roots);
        let registry_rows = options_registry_rows(engine, &loaded);
        if !registry_rows.is_empty() {
            diagnostics.extend(engine.apply_severity_options(registry_rows));
        }
        // The cross-file plugin-source conflicts ride the same options-file
        // attribution (`_computePluginConflicts` reports on spec1's file),
        // and a malformed options file's failure is the stderr advisory —
        // the oracle's machine-face silence (pinned probe) means it joins
        // no diagnostic row.
        for options in &loaded {
            if let Some(notice) = options_failure_notice(options) {
                notices.push(notice);
            }
        }
        let conflict_rows =
            plugin_conflict_rows(&loaded, &|path| engine.file_table().try_file_id(path));
        if !conflict_rows.is_empty() {
            diagnostics.extend(engine.apply_severity_options(conflict_rows));
        }
        // The inner-options plugins rows ride the same face (the
        // parse-model's engagement, analysis time).
        let inner_rows = inner_plugins_rows(engine);
        if !inner_rows.is_empty() {
            diagnostics.extend(engine.apply_severity_options(inner_rows));
        }
        // The ignore-comment removed-state rows (#1005) ride the same face.
        let removed_rows = removed_lint_use_rows(engine, roots);
        if !removed_rows.is_empty() {
            diagnostics.extend(engine.apply_severity_options(removed_rows));
        }
    }
    let lints = lints_at.elapsed();
    report_memory(engine);
    // The run's memory posture: the same moment the decomposition probe
    // prints, the run's end (the residency peak).
    let memory = (engine.memory_status(), engine.warm_stats());
    // The layout snapshot covers every location the writers resolve — a
    // context message may anchor in a file that carries no row of its own
    // (#2164: its id joins the output set or the merged lookups panic).
    let layout = EngineLayout::over(
        engine,
        diagnostics.iter().flat_map(|d| {
            std::iter::once(d.file).chain(d.context.iter().flatten().map(|m| m.file))
        }),
    );
    // Stable like `sort_by` (rayon's merge sort), so ties keep their
    // arrival order and the output is byte-identical; the comparator walks
    // path bytes per pair, so 10⁵-row runs split across workers.
    diagnostics.par_sort_by(|a, b| oracle_cmp(a, b, &layout));
    Ok(AnalyzeRun {
        diagnostics,
        discovery,
        analysis,
        lints,
        file_count: stats.file_count,
        loc: stats.loc_total,
        libraries_analyzed,
        libraries_from_cache,
        degrade_notices: notices,
        memory,
        layout,
    })
}

/// One `dart analyze` rest argument, resolved to its on-disk kind — the
/// oracle's target classification (analyze.dart:185-193): an existing
/// directory yields a directory target, an existing file a file target.
/// Paths are canonicalized by the caller, so they live in the same spelling
/// universe the engine's discovery interns.
#[derive(Debug, Clone)]
pub enum Target {
    /// A directory target: the run emits every diagnostic whose file lies
    /// under it (the oracle's directory context root).
    Dir(PathBuf),
    /// A file target: the run emits only that file's rows (the oracle's
    /// file context root pushes diagnostics for the file alone); the file
    /// analyzes within its containing workspace.
    File(PathBuf),
}

/// Whether `path` is covered by one of the `targets` — under a directory
/// target or identical to a `.dart` file target. The emitted diagnostic set
/// is exactly the covered rows (the oracle pushes diagnostics only for
/// files inside the targets' context roots — and a file target's root only
/// ever analyzes `.dart` files, `file_paths.dart:75-78` `isDart`). Public:
/// the daemon's `dartq/analyze` filters its analyzed view through the same
/// coverage rule its CLI twin's target set applies.
#[must_use]
pub fn covered(path: &Path, targets: &[Target]) -> bool {
    targets.iter().any(|target| match target {
        Target::Dir(dir) => path.starts_with(dir),
        Target::File(file) => path == file && file.extension().is_some_and(|ext| ext == "dart"),
    })
}

/// [`run_overlaid`] over an explicit target set — the multi-argument
/// `dart analyze` face. The engine analyzes the whole assembled workspace
/// (the file/dir targets' containing context, upstream's model: a target's
/// context root supplies the resolution universe), and the emitted set
/// filters to the covered files.
///
/// A file target outside the discovered surfaces (the root-level pass plus
/// `lib`/`bin`/`test`/`tool`) is still analyzed: its bytes enqueue through
/// the same `set_content` contract the overlays use, so a
/// `dart analyze extra/x.dart` invocation produces the file's rows exactly
/// like the oracle's file context root. A file target that IS discovered
/// needs no injection.
///
/// `plugins` is the analyze face's plugin set — `Some` when the workspace's
/// effective options declares a `plugins:` section and the CLI's
/// `--no-plugins` did not disable it (upstream `dart analyze` loads plugins
/// by default). The engine+lint diagnostic set feeds the cycle, the drained
/// findings merge into the emitted rows, and crash rows join beside them —
/// `None` (the plugin-free workspace, or the flag's opt-out) skips the
/// whole step at one emptiness check, the zero-degradation posture.
///
/// # Errors
///
/// [`AnalyzeError`] on discovery failure, an unreadable file target, or
/// cancellation.
pub fn run_targets<'a>(
    engine: &'a mut Engine,
    overlays: &[(PathBuf, Vec<u8>)],
    roots: &[PathBuf],
    targets: &[Target],
    plugins: Option<&mut PluginSet>,
) -> Result<AnalyzeRun<'a>, AnalyzeError> {
    let started = std::time::Instant::now();
    let stats = engine
        .discover_workspace()
        .map_err(AnalyzeError::Discovery)?;
    let discovery = started.elapsed();
    let mut notices: Vec<String> = stats
        .oversized
        .iter()
        .map(|path| rejection_notice(path))
        .collect();

    // File targets discovery cannot reach (outside every analyzed surface)
    // enqueue through the overlay contract — same canonical path identity,
    // disk bytes. Discovered targets keep the bytes discovery already read.
    // Non-`.dart` file targets never inject: the oracle's driver only takes
    // `.dart` files (`isDart`, file_paths.dart:75-78) — a `pubspec.yaml`
    // target is a legal rest arg that yields no rows.
    for target in targets {
        if let Target::File(path) = target
            && path.extension().is_some_and(|ext| ext == "dart")
            && engine.file_table().try_file_id(path).is_none()
        {
            let bytes = std::fs::read(path).map_err(AnalyzeError::Discovery)?;
            engine.set_content(path, &bytes);
        }
    }
    for (path, bytes) in overlays {
        engine.set_content(path, bytes);
    }

    let (mut diagnostics, analysis, libraries_analyzed, libraries_from_cache, mut event_notices) =
        analyze_pass(engine)?;
    notices.append(&mut event_notices);
    // The loaded options set lives past the roots block: the plugin weave's
    // per-directory gate consumes the same governing directories.
    let loaded = loaded_options(engine, roots);
    let lints_at = std::time::Instant::now();
    if !roots.is_empty() {
        let lint_rows =
            crate::lints::analyze_lints(engine, roots, &mut crate::lints::LintMemo::default());
        diagnostics.extend(engine.apply_severity_options(lint_rows));
        let registry_rows = options_registry_rows(engine, &loaded);
        if !registry_rows.is_empty() {
            diagnostics.extend(engine.apply_severity_options(registry_rows));
        }
        for options in &loaded {
            if let Some(notice) = options_failure_notice(options) {
                notices.push(notice);
            }
        }
        let conflict_rows =
            plugin_conflict_rows(&loaded, &|path| engine.file_table().try_file_id(path));
        if !conflict_rows.is_empty() {
            diagnostics.extend(engine.apply_severity_options(conflict_rows));
        }
        let inner_rows = inner_plugins_rows(engine);
        if !inner_rows.is_empty() {
            diagnostics.extend(engine.apply_severity_options(inner_rows));
        }
        let removed_rows = removed_lint_use_rows(engine, roots);
        if !removed_rows.is_empty() {
            diagnostics.extend(engine.apply_severity_options(removed_rows));
        }
    }
    let lints = lints_at.elapsed();
    // The oracle emits rows for covered files only; the filter is a retain
    // on the interned (canonical) spelling, so it precedes the layout build.
    let files = engine.file_table();
    diagnostics.retain(|d| {
        files
            .path(d.file)
            .is_some_and(|path| covered(path, targets))
    });
    // The plugin weave runs once, after the engine+lint set is final: the
    // cycle feeds the analyzed context's files (the whole
    // `engine.libraries()` set — a plugin's rules run over the context root,
    // matching the daemon's `weave_plugins` `analyzed` face, not just the
    // covered emit subset), drains findings within the per-plugin budget,
    // and the merged rows (findings + the one-shot crash rows) join the emit
    // set before the oracle sort — the `dart analyze` plugin face the daemon
    // shapes identically for its own output.
    if let Some(plugin_set) = plugins {
        // The analyzed context's wire spellings (canonical paths — the same
        // universe `files.try_file_id` resolves through): every file the
        // engine analyzed, in library order — the files a rule-executing
        // plugin runs its enabled rules over. The same pass tracks the
        // crash anchor: the analyzed set's minimum `FileId` — the daemon's
        // anchor convention (`published.keys().min()`, session.rs), the
        // run's published state, so a crash row publishes even on a run
        // whose engine set came out empty.
        let mut anchor: Option<FileId> = None;
        let analyzed: Vec<String> = engine
            .libraries()
            .into_iter()
            .flat_map(|(_, library_files)| library_files)
            .filter_map(|file| {
                if anchor.is_none_or(|min| file < min) {
                    anchor = Some(file);
                }
                files.path(file)
            })
            .map(|path| path.display().to_string())
            .collect();
        // The overlays ride to the plugins as the edited view of the
        // analyzed set; files not handed here read from disk (the daemon's
        // contract). Source bytes are UTF-8 (the scanner's own gate), so
        // the lossy conversion never fires on legal input.
        let plugin_overlays: Vec<(String, String)> = overlays
            .iter()
            .map(|(path, bytes)| {
                (
                    path.display().to_string(),
                    String::from_utf8_lossy(bytes).into_owned(),
                )
            })
            .collect();
        // The per-directory `plugins:` semantics resolve against the loaded
        // options set — the same governing directories the conflict face
        // scanned. `loaded` is `Vec<&AnalysisOptions>` already; the cycle
        // borrows it directly.
        let context = crate::plugins::CycleContext {
            analyzed: &analyzed,
            overlays: &plugin_overlays,
            governing_options: &loaded,
            query_server: Some(&|query| crate::plugins::serve_query(engine, query)),
        };
        let cycle = plugin_set.cycle(&diagnostics, files, anchor, &context);
        let mut findings = crate::plugins::resolve_findings(
            &cycle.findings,
            files,
            |file| {
                let index = engine.line_index(file)?;
                let text = engine.content(file)?;
                Some((index, std::str::from_utf8(text).ok()?))
            },
            &loaded,
        );
        // The covered emit filter applies to plugin rows identically: the
        // context analyzes the whole root, but the oracle emits rows only
        // inside the targets — a finding on a non-covered file never
        // surfaces (a crash row's anchor may name any analyzed file, so it
        // rides unfiltered beside the findings).
        findings.retain(|d| {
            files
                .path(d.file)
                .is_some_and(|path| covered(path, targets))
        });
        // Crash rows ride beside the findings (engine `Diagnostic`s already);
        // the merge's dedup seeds from the engine rows so a plugin row never
        // duplicates a built-in finding.
        let plugin_rows = crate::plugins::merge(
            std::mem::take(&mut diagnostics),
            cycle.crashes.into_iter().chain(findings).collect(),
        );
        diagnostics = plugin_rows;
    }
    report_memory(engine);
    // The run's memory posture: the same moment the decomposition probe
    // prints, the run's end (the residency peak).
    let memory = (engine.memory_status(), engine.warm_stats());
    // The layout snapshot covers every location the writers resolve — a
    // context message may anchor in a file that carries no row of its own
    // (#2164: its id joins the output set or the merged lookups panic).
    let layout = EngineLayout::over(
        engine,
        diagnostics.iter().flat_map(|d| {
            std::iter::once(d.file).chain(d.context.iter().flatten().map(|m| m.file))
        }),
    );
    // Stable like `sort_by` (rayon's merge sort), so ties keep their
    // arrival order and the output is byte-identical; the comparator walks
    // path bytes per pair, so 10⁵-row runs split across workers.
    diagnostics.par_sort_by(|a, b| oracle_cmp(a, b, &layout));
    Ok(AnalyzeRun {
        diagnostics,
        discovery,
        analysis,
        lints,
        file_count: stats.file_count,
        loc: stats.loc_total,
        libraries_analyzed,
        libraries_from_cache,
        degrade_notices: notices,
        memory,
        layout,
    })
}

/// Several workspaces' runs merged into one oracle-ordered output view —
/// the multi-target face's union (`dart analyze lib other_pkg`): every
/// run's diagnostics remap onto one [`FileTable`], the merged set carries
/// the oracle display order (severity, path bytes, offset, message —
/// [`oracle_cmp`]'s keys, resolved through each run's own layout), and the
/// run facts sum across engines. `runs` stays alive inside: the merged
/// layout delegates position lookups to the owning run's
/// [`EngineLayout`].
pub struct MergedRun<'a> {
    /// The union diagnostic set, oracle display order, `file` and every
    /// context message's `file` remapped onto `files`.
    pub diagnostics: Vec<Diagnostic>,
    /// Merged run facts: per-phase wall-clock summed across the sequential
    /// engine runs; counters summed.
    pub discovery: Duration,
    pub analysis: Duration,
    pub lints: Duration,
    pub file_count: usize,
    pub loc: u64,
    pub libraries_analyzed: usize,
    pub libraries_from_cache: usize,
    /// Every run's warning-tier notices, concatenated in run order.
    pub degrade_notices: Vec<String>,
    /// Each run's engine memory posture ([`AnalyzeRun::memory`]), in run
    /// order — the `--memory-report` face's per-engine lines.
    pub memory: Vec<(dartq_engine::MemoryStatus, dartq_engine::WarmStats)>,
    /// The remapped file ids' owning run and original id — `owners[i]` is
    /// the provenance of merged `FileId` `i`, the delegation table
    /// [`FileLayoutProvider::line_col`] resolves through.
    owners: Vec<(usize, FileId)>,
    /// The union path universe: the remapped ids' canonical spellings.
    files: FileTable,
    /// Kept for the per-run layouts the merged provider delegates to.
    runs: Vec<AnalyzeRun<'a>>,
}

/// Merges `runs` into one oracle-ordered output view — see [`MergedRun`].
/// An empty input yields an empty set over an empty table (the writers
/// render it as the oracle's clean face).
pub fn merge_runs<'a>(runs: Vec<AnalyzeRun<'a>>) -> MergedRun<'a> {
    // The oracle display order (`oracle_cmp`'s keys): severity, path bytes,
    // offset, message — paths resolve through each diagnostic's own run
    // layout until the ids are remapped below.
    let mut keyed: Vec<(usize, Diagnostic)> = runs
        .iter()
        .enumerate()
        .flat_map(|(run_index, run)| run.diagnostics.iter().map(move |d| (run_index, d.clone())))
        .collect();
    keyed.sort_by(|a, b| {
        severity_rank(a.1.severity)
            .cmp(&severity_rank(b.1.severity))
            .then_with(|| {
                path_key(runs[a.0].layout.file_path(a.1.file))
                    .cmp(path_key(runs[b.0].layout.file_path(b.1.file)))
            })
            .then_with(|| a.1.range.offset.cmp(&b.1.range.offset))
            .then_with(|| a.1.message.cmp(&b.1.message))
    });

    let mut files = FileTable::new();
    // `owners[merged_id]` = (run, original id): the intern can return an
    // existing id for a repeated path, so the table is filled by id, not
    // by push order (the `resize` keeps the sparse positions defaultable).
    let mut owners: Vec<Option<(usize, FileId)>> = Vec::new();
    // `(run, original id) → merged id`: each distinct source file interns
    // (and canonicalizes, one `realpath`) once, not once per row.
    let mut seen: FxHashMap<(usize, FileId), FileId> = FxHashMap::default();
    // The normalizations (one `realpath` each, the pass's whole cost at
    // 10⁵ files) are pure per path, so they run as one parallel pre-pass
    // over the distinct `(run, id)` locations; the serial walk below then
    // mints merged ids in the canonical row order exactly as before.
    let normalized: FxHashMap<(usize, FileId), PathBuf> = {
        let mut locations: Vec<(usize, FileId)> = keyed
            .iter()
            .flat_map(|(run_index, diagnostic)| {
                std::iter::once(diagnostic.file)
                    .chain(
                        diagnostic
                            .context
                            .iter()
                            .flatten()
                            .map(|message| message.file),
                    )
                    .map(move |file| (*run_index, file))
            })
            .collect();
        locations.sort_unstable();
        locations.dedup();
        // Sorted locations cluster by directory: one canonicalization per
        // distinct parent per slice (`normalize_paths`).
        locations
            .par_chunks(64)
            .flat_map_iter(|slice| {
                let paths: Vec<&Path> = slice
                    .iter()
                    .map(|&(run_index, id)| runs[run_index].layout.file_path(id))
                    .collect();
                let normalized = dartq_source::normalize_paths(&paths);
                slice.iter().copied().zip(normalized)
            })
            .collect()
    };
    // Every location the writers resolve — the diagnostic's own and each
    // context message's — interns onto the merged table: context messages
    // point at secondary locations, often in other files, whose ids would
    // otherwise stay raw engine ids (an out-of-table lookup panics on the
    // merged provider; a colliding id silently resolves a different
    // file's line and column).
    let mut remap = |run_index: usize, id: FileId| -> FileId {
        if let Some(&merged) = seen.get(&(run_index, id)) {
            return merged;
        }
        let original = id;
        let path = runs[run_index].layout.file_path(id);
        let merged = match normalized.get(&(run_index, id)) {
            Some(canonical) => files.intern_normalized(path, canonical.clone()),
            None => files.intern(path),
        };
        seen.insert((run_index, id), merged);
        let slot = merged.index() as usize;
        if owners.len() <= slot {
            owners.resize_with(slot + 1, || None);
        }
        // First writer wins: two runs diagnosing the same canonical file
        // read identical bytes (deterministic analysis), so either layout
        // resolves the position equivalently.
        owners[slot].get_or_insert((run_index, original));
        merged
    };
    let mut diagnostics = Vec::with_capacity(keyed.len());
    for (run_index, mut diagnostic) in keyed {
        diagnostic.file = remap(run_index, diagnostic.file);
        if let Some(context) = &mut diagnostic.context {
            for message in context.iter_mut() {
                message.file = remap(run_index, message.file);
            }
        }
        diagnostics.push(diagnostic);
    }
    let owners: Vec<(usize, FileId)> = owners
        .into_iter()
        .map(|owner| owner.expect("every interned id has an owner"))
        .collect();
    let memory = runs.iter().map(|run| run.memory).collect();

    let mut merged = MergedRun {
        diagnostics,
        discovery: Duration::ZERO,
        analysis: Duration::ZERO,
        lints: Duration::ZERO,
        file_count: 0,
        loc: 0,
        libraries_analyzed: 0,
        libraries_from_cache: 0,
        degrade_notices: Vec::new(),
        memory,
        owners,
        files,
        runs,
    };
    for run in &merged.runs {
        merged.discovery += run.discovery;
        merged.analysis += run.analysis;
        merged.lints += run.lints;
        merged.file_count += run.file_count;
        merged.loc += run.loc;
        merged.libraries_analyzed += run.libraries_analyzed;
        merged.libraries_from_cache += run.libraries_from_cache;
    }
    for run in &mut merged.runs {
        merged.degrade_notices.append(&mut run.degrade_notices);
    }
    merged
}

impl dartq_diagnostics::FileLayoutProvider for MergedRun<'_> {
    fn file_path(&self, file: FileId) -> &Path {
        self.files.path(file).expect("merged FileId")
    }

    /// The analyze face's analysis root: the engine's context root (the
    /// package table's workspace root — the configured workspace's root,
    /// or the analyzed directory on the no-config floors), shared by every
    /// merged run. Divergent roots (a multi-workspace merge) have no one
    /// root to project against: the no-root face, absolute spellings.
    fn analysis_root(&self) -> Option<&Path> {
        let mut roots = self
            .runs
            .iter()
            .map(|run| run.layout.engine().packages().workspace_root());
        let first = roots.next()?;
        roots.all(|root| root == first).then_some(first)
    }

    fn line_col(&self, file: FileId, utf16_offset: u32) -> (u32, u32) {
        let &(run, original) = self
            .owners
            .get(file.index() as usize)
            .expect("merged FileId");
        self.runs[run].layout.line_col(original, utf16_offset)
    }
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
/// reports unconditionally), and the same two shapes' `analyzer.cannot-ignore`
/// string entries — severity names (`error`/`info`/`warning`) skip the check
/// upstream, every other spelling validates against the same universes
/// (parse_model:346-405; the included files' rows wrap like every other
/// inner row). The same `linter.rules` entries carry the value
/// face and the enabled pass (`unsupported_value` / `incompatible_lint` /
/// `duplicate_rule`; parse_model.dart:1272-1284, 1379-1408 — the value row
/// is registry-gated like `undefined_lint`, and the pass walks the enabled
/// declarations in order, each checking the rules already taken before
/// joining them). The engine records the candidates and their
/// anchors at load; this face validates and emits rows attributed to the
/// options file itself. Distinct loaded files yield once (two roots sharing
/// one options file are one parse, officially too).
fn options_registry_rows(
    engine: &Engine,
    loaded: &[&analysis_options::AnalysisOptions],
) -> Vec<Diagnostic> {
    let mut rows = Vec::new();
    // Every context root's options file validates, not just the workspace
    // root's (context_builder.dart:199-224 — `_createOptionsMap` parses
    // each `optionsFileMap` entry as an initial file, so a directory with
    // its own package config carries its own full parse: the lints corpus's
    // `nested/` re-reports its include's conflicts wrapped and validates its
    // own names). The workspace roots plus every package root beneath them
    // — the package table holds the nested package-config members; a root
    // outside the analyzed surface never joins.
    for options in loaded {
        // The removed/deprecated state gate reads this file's own package's
        // `sdkVersionConstraint` (`PubPackage.sdkVersionConstraint`,
        // pub.dart:516-530 — the pubspec `environment.sdk` constraint; each
        // analysis context parses the options with its package's value).
        let Some(file) = engine.file_table().try_file_id(&options.file_path) else {
            continue;
        };
        // The package the options file belongs to is its declaring
        // directory's own — `sdk_constraint_min` reads `<dir>/pubspec.yaml`.
        let constraint_min =
            sdk_constraint_min(options.file_path.parent().unwrap_or_else(|| Path::new("")));
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
            for error in &group.cannot_ignore {
                if !analysis_options::is_known_diagnostic_code(&error.code)
                    && !is_known_lint_name(&error.code)
                {
                    let inner = analysis_options::OptionsIssue::unrecognized_error_code(error);
                    rows.push(group.wrap_row(&inner).to_diagnostic(file));
                }
            }
            // The included file's own parse-time pass — the same
            // `_parseLocalRules` arms the initial file renders raw, none of
            // them initial-gated, so every row rides the included file's
            // reporter and wraps (include_walker.dart:210-248): the illegal
            // value's row (parse_model.dart:1272-1284), then the enabled
            // pass's incompatible and duplicate checks against the rules
            // already taken (parse_model.dart:1379-1408). An unregistered
            // name reports above and never reaches here (parseRule returns
            // before its value is read).
            let mut active: FxHashMap<String, &analysis_options::DeclaredRule> =
                FxHashMap::default();
            for rule in &group.rules {
                if !is_known_lint_name(&rule.name) {
                    continue;
                }
                if let Some(illegal) = &rule.illegal_value {
                    let inner =
                        analysis_options::OptionsIssue::unsupported_lint_value(rule, illegal);
                    rows.push(group.wrap_row(&inner).to_diagnostic(file));
                }
                if rule.enabled {
                    let key = rule.name.to_ascii_lowercase();
                    let mut conflicting: Vec<&analysis_options::DeclaredRule> = Vec::new();
                    for incompatible in dartq_lints::incompatible_rules(&rule.name) {
                        if let Some(entry) = active.get(*incompatible) {
                            conflicting.push(*entry);
                        }
                    }
                    if !conflicting.is_empty() {
                        let inner = analysis_options::ContextualIssue::incompatible_lint(
                            rule,
                            &conflicting,
                        );
                        rows.push(group.wrap_row(inner.inner()).to_diagnostic(file));
                    }
                    if active.contains_key(&key) {
                        let inner = analysis_options::OptionsIssue::duplicate_rule(rule);
                        rows.push(group.wrap_row(&inner).to_diagnostic(file));
                    }
                    active.insert(key, rule);
                }
            }
            // The cross-file lint checks inside this included file
            // (`_LinterRuleDiagnostics`, linter_rule_options_validator.dart
            // :99-225) — the official reports them through the included
            // file's own reporter, which wraps every row into
            // `included_file_warning` at the branch anchor (message only —
            // the context face is the unwrapped row's).
            for issue in
                incompatible_cross_rows(&group.rules, &group.includes, &options.included_groups)
            {
                rows.push(group.wrap_row(issue.inner()).to_diagnostic(file));
            }
        }
        // The enabled pass's incremental state (parse_model.dart:1338-1343,
        // 1379-1408): the registry identities already taken by earlier
        // enabled declarations, keyed case-insensitively (the official
        // registry lowercases every name, registry.dart:42,53,133 — the
        // mixed-case duplicate test pins that) to the declaration whose
        // written spelling the messages render.
        let mut active: FxHashMap<String, &analysis_options::DeclaredRule> = FxHashMap::default();
        for rule in &options.declared_rules {
            if !is_known_lint_name(&rule.name) {
                rows.push(analysis_options::OptionsIssue::undefined_lint(rule).to_diagnostic(file));
                continue;
            }
            // The removed/deprecated rows (#988): the official `parseRule`
            // reports them from the resolved rule's `state` for every
            // initial-file name the registry resolves — enabled or
            // disabled, the state check rides the parse, not the entry's
            // value (parse_model.dart:1288-1326) — anchored at the name
            // token like `undefined_lint`, and gated on the package's
            // SDK-constraint lower bound. The deprecation arm switches
            // first and keys on the name alone: `rule.state` exists
            // whether or not dartq-lints implements the rule (the rules
            // keep running despite deprecation upstream), so the face
            // never consults the implementation lookup.
            if let Some(since) = crate::rule_states::deprecated_since(&rule.name)
                && crate::rule_states::before_current(Some(since), constraint_min)
            {
                // The pinned deprecation set carries no `replacedBy`
                // (rule_states.rs), so only the plain `deprecated_lint`
                // row applies (diagnostic.g.dart `deprecatedLint`) and
                // the registry texts render verbatim.
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
            } else if let Some(id) = dartq_lints::rule_by_name(&rule.name) {
                // The removed arm rides the spec: every `removed_lint`
                // name is a `Group::Removed` spec (the rule_states pin),
                // whose message of record renders the row.
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
            }
            // The value row (parseRule's fall-through arm,
            // parse_model.dart:1272-1284): an illegal scalar reports and
            // disables — and the name is already registry-gated above (the
            // official check fires first, so an unregistered name's value is
            // never inspected).
            if let Some(illegal) = &rule.illegal_value {
                rows.push(
                    analysis_options::OptionsIssue::unsupported_lint_value(rule, illegal)
                        .to_diagnostic(file),
                );
            }
            // The enabled pass, incrementally (parse_model.dart:1379-1408 —
            // per enabled declaration in order: the incompatible check
            // against the rules already taken, then the duplicate check,
            // then the rule joins the taken set; a disabled or illegal value
            // never joins, so it neither reports nor takes).
            if rule.enabled {
                let key = rule.name.to_ascii_lowercase();
                let mut conflicting: Vec<&analysis_options::DeclaredRule> = Vec::new();
                for incompatible in dartq_lints::incompatible_rules(&rule.name) {
                    if let Some(entry) = active.get(*incompatible) {
                        conflicting.push(*entry);
                    }
                }
                if !conflicting.is_empty() {
                    rows.push(
                        analysis_options::ContextualIssue::incompatible_lint(rule, &conflicting)
                            .to_diagnostic(file, |path| engine.file_table().try_file_id(path)),
                    );
                }
                if active.contains_key(&key) {
                    rows.push(
                        analysis_options::OptionsIssue::duplicate_rule(rule).to_diagnostic(file),
                    );
                }
                active.insert(key, rule);
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
        for error in &options.declared_cannot_ignore {
            if !analysis_options::is_known_diagnostic_code(&error.code)
                && !is_known_lint_name(&error.code)
            {
                rows.push(
                    analysis_options::OptionsIssue::unrecognized_error_code(error)
                        .to_diagnostic(file),
                );
            }
        }
        // The initial file's cross-file lint checks report their raw rows
        // (`_LinterRuleDiagnostics`, linter_rule_options_validator.dart
        // :99-225 — the walker runs them on the initial diagnostic reporter,
        // unwrapped) with their context face attached: each conflicting
        // rule's owning options file resolves through the same table the
        // rows attribute against, an unresolvable file dropping its row.
        for issue in incompatible_cross_rows(
            &options.declared_rules,
            &options.included_edges,
            &options.included_groups,
        ) {
            rows.push(issue.to_diagnostic(file, |path| engine.file_table().try_file_id(path)));
        }
    }
    rows
}

/// Every distinct loaded options file under the analyzed workspace — the
/// candidate set [`options_registry_rows`] walks (roots plus the package
/// roots beneath them), deduplicated by the options file's own path. The
/// cross-file plugin-conflict check and the parse-failure advisory both
/// consume it: upstream's `_computePluginConflicts` scans every options
/// file parsed in the context (context_manager.dart:614-685).
fn loaded_options<'a>(
    engine: &'a Engine,
    roots: &[PathBuf],
) -> Vec<&'a analysis_options::AnalysisOptions> {
    let mut candidates: Vec<&Path> = roots.iter().map(PathBuf::as_path).collect();
    for package in engine.packages().iter() {
        if roots.iter().any(|root| package.root.starts_with(root)) {
            candidates.push(package.root.as_path());
        }
    }
    let mut seen = FxHashSet::default();
    candidates
        .into_iter()
        .filter_map(|root| engine.options_for_root(root))
        .filter(|options| seen.insert(options.file_path.clone()))
        .collect()
}

/// The `incompatible_plugin_source` rows (`_computePluginConflicts`,
/// upstream Dart SDK source,
/// `pkg/analysis_server/lib/src/context_manager.dart`:606-685): for each
/// plugin name, every pair of options files that both declare it (their own
/// `plugins:` node a map — [`AnalysisOptions::has_plugins_map`]) and whose
/// declared sources differ reports one row on the *first* spec's file,
/// anchored at the plugin's key there (the merged configuration may arrive
/// through the include chain, so the anchor falls back to the section's
/// value node when no local key names it —
/// [`AnalysisOptions::plugin_name_anchor`]).
fn plugin_conflict_rows(
    loaded: &[&analysis_options::AnalysisOptions],
    file_id: &dyn Fn(&Path) -> Option<FileId>,
) -> Vec<Diagnostic> {
    // Specs grouped by plugin name, in load order — upstream iterates the
    // parse-results map's insertion order; ours is the candidates order.
    let mut specs: Vec<(&str, Vec<(usize, &analysis_options::PluginConfig)>)> = Vec::new();
    for (index, options) in loaded.iter().enumerate() {
        if !options.has_plugins_map {
            continue;
        }
        for config in &options.plugins {
            match specs.iter_mut().find(|(name, _)| *name == config.name) {
                Some((_, group)) => group.push((index, config)),
                None => specs.push((config.name.as_str(), vec![(index, config)])),
            }
        }
    }
    let mut rows = Vec::new();
    for (name, group) in specs {
        for first in 0..group.len() {
            for second in (first + 1)..group.len() {
                let (spec_file, spec1) = group[first];
                let (_, spec2) = group[second];
                if spec1.source != spec2.source {
                    let options = loaded[spec_file];
                    let Some(file) = file_id(&options.file_path) else {
                        continue;
                    };
                    rows.push(
                        analysis_options::OptionsIssue::incompatible_plugin_source(
                            name,
                            &loaded[group[second].0].file_path,
                            options.plugin_name_anchor(name),
                        )
                        .to_diagnostic(file),
                    );
                }
            }
        }
    }
    rows
}

/// The `plugins_in_inner_options` rows (`_ParsedPluginsData.parse`,
/// upstream Dart SDK source,
/// `pkg/analyzer/lib/src/analysis_options/analysis_options_parse_model.dart`
/// :1538-1545): every *initial* options file whose directory is not the
/// context root and whose top-level `plugins:` is a map reports at the
/// section's whole value node, the correction naming the context root — the
/// oracle's context root, the workspace bound (context_builder.dart passes
/// `contextRoot.root` into every `optionsFileMap` entry's parse, so a
/// nested file's directory — its own map key — is never the root).
/// [`AnalysisOptions::has_plugins_map`] encodes both legs, the initial one
/// and the map one (the flag is recorded for the initial file's own parse
/// only); a non-map section reports `invalid_section_format` instead and
/// never reaches this row. Distinct loaded files yield once (two keys
/// sharing one options file are one parse, officially too).
fn inner_plugins_rows(engine: &Engine) -> Vec<Diagnostic> {
    let context_root = engine.packages().workspace_root();
    let mut inner: Vec<&analysis_options::AnalysisOptions> = engine
        .options_entries()
        .filter(|(_, options)| {
            options.has_plugins_map && options.file_path.parent() != Some(context_root)
        })
        .map(|(_, options)| options)
        .collect();
    inner.sort_unstable_by(|a, b| a.file_path.cmp(&b.file_path));
    inner.dedup_by(|a, b| a.file_path == b.file_path);
    let table = engine.file_table();
    inner
        .into_iter()
        .filter_map(|options| {
            let file = table.try_file_id(&options.file_path)?;
            Some(
                analysis_options::OptionsIssue::plugins_in_inner_options(
                    &context_root.display().to_string(),
                    options.plugins_section_anchor(),
                )
                .to_diagnostic(file),
            )
        })
        .collect()
}

/// The `note:` advisory for one options file whose YAML failed to parse —
/// the oracle emits no machine row for a malformed options file (the pinned
/// 2026-10-18 probe), so the failure rides the faces' stderr channel.
fn options_failure_notice(options: &analysis_options::AnalysisOptions) -> Option<String> {
    options
        .parse_failure
        .as_ref()
        .map(|issue| format!("note: {}: {}", options.file_path.display(), issue.message))
}
/// + `reportIncompatibleWithIncluded`, upstream Dart SDK source,
/// `pkg/analyzer/lib/src/analysis_options/linter_rule_options_validator.dart`
/// :99-225) — returned as the *inner* contextual rows: for the initial
/// options file the caller emits them raw with their context face
/// ([`analysis_options::ContextualIssue::to_diagnostic`]); an included
/// file's rows wrap into `included_file_warning` through
/// [`analysis_options::ContextualIssue::inner`] +
/// [`analysis_options::DeclarationGroup::wrap_row`] — the wrap carries the
/// message only, the context face is the unwrapped row's.
///
/// `local` is the reporting file's own `linter.rules` declarations
/// (`DeclaredRule`s), `edges` its resolved include edges in declaration
/// order, and `groups` the shared included-file table the edges index.
/// Registry membership filters through [`is_known_lint_name`] — upstream's
/// `parseRule` drops unregistered names before they ever reach the rule sets.
///
/// Ordering: `incompatible_lint_included` rows first (include order, the
/// walker's own sequence), then `incompatible_lint_files` (local declaration
/// order) — the two checks run in the walker's call order.
fn incompatible_cross_rows(
    local: &[analysis_options::DeclaredRule],
    edges: &[analysis_options::IncludeEdge],
    groups: &[analysis_options::DeclarationGroup],
) -> Vec<analysis_options::ContextualIssue> {
    /// A declaration's registry key: the lowercased name for a registered
    /// rule, `None` when unregistered (upstream's `_RuleData` only exists for
    /// registry-resolved names — `parseRule` returns null otherwise, so an
    /// unregistered declaration never enters the rule sets).
    fn key_of(name: &str) -> Option<String> {
        is_known_lint_name(name).then(|| name.to_lowercase())
    }

    let local_enabled: Vec<&analysis_options::DeclaredRule> = local
        .iter()
        .filter(|rule| rule.enabled && is_known_lint_name(&rule.name))
        .collect();
    let local_disabled_keys: FxHashSet<String> = local
        .iter()
        .filter(|rule| !rule.enabled)
        .filter_map(|rule| key_of(&rule.name))
        .collect();

    // ---- incompatible_lint_included (validator :105-186): conflicts
    // between rules introduced by *different* include entries. `seen` holds
    // the live rules contributed by earlier edges, keyed by registry name
    // to the contributing edge and declaration.
    let mut issues = Vec::new();
    let mut seen: FxHashMap<String, (usize, &analysis_options::RuleData)> = FxHashMap::default();
    for (edge_index, edge) in edges.iter().enumerate() {
        let child = &groups[edge.group];
        // Evict everything the current edge's subtree touches, then every
        // locally disabled rule — a nearer declaration shadows the earlier
        // contributions (:115-123).
        for rule in child
            .disabled_rules
            .iter()
            .chain(child.enabled_rules.iter())
        {
            seen.remove(&rule.name.to_lowercase());
        }
        for key in &local_disabled_keys {
            seen.remove(key);
        }
        // (contributing edge, declaration) pairs, deduplicated by the rule
        // token's identity — upstream `identical(node)`. Each `include`
        // entry parses its target fresh (every `_ParsedInclude` carries its
        // own nodes), so the same physical token reached through two edges
        // is two occurrences upstream; `(owning group, offset)` reproduces
        // that — one group per parse.
        let mut incompatible: Vec<(usize, &analysis_options::RuleData)> = Vec::new();
        for rule in &child.enabled_rules {
            let Some(key) = key_of(&rule.name) else {
                continue;
            };
            if local_disabled_keys.contains(&key) {
                continue;
            }
            // The earlier-seen rules this rule declares incompatible, in the
            // declared `incompatibleRules` order (:134-157).
            let mut priors: Vec<(usize, &analysis_options::RuleData)> = Vec::new();
            for incompatible in dartq_lints::incompatible_rules(&rule.name) {
                if let Some(&(prior_edge, prior_rule)) = seen.get(*incompatible) {
                    priors.push((prior_edge, prior_rule));
                }
            }
            if priors.is_empty() {
                continue;
            }
            if !incompatible.iter().any(|(prior_edge, entry)| {
                edges[*prior_edge].group == edge.group && entry.offset == rule.offset
            }) {
                incompatible.push((edge_index, rule));
            }
            for (prior_edge, prior_rule) in priors {
                let prior_group = edges[prior_edge].group;
                if !incompatible.iter().any(|(entry_edge, entry)| {
                    edges[*entry_edge].group == prior_group && entry.offset == prior_rule.offset
                }) {
                    incompatible.push((prior_edge, prior_rule));
                }
            }
        }
        if !incompatible.is_empty() {
            // The `{1}` join and the context rows are keyed by the
            // conflicting rules' files — upstream's `Map<file.path, node>`
            // fold keeps the first position and the last declaration per
            // file (:163-168).
            let mut conflicting: Vec<&analysis_options::RuleData> = Vec::new();
            for &(_, rule) in &incompatible {
                match conflicting
                    .iter_mut()
                    .find(|entry| entry.file.as_path() == rule.file.as_path())
                {
                    Some(slot) => *slot = rule,
                    None => conflicting.push(rule),
                }
            }
            let file_count = incompatible
                .iter()
                .map(|(edge_index, _)| *edge_index)
                .collect::<FxHashSet<_>>()
                .len();
            issues.push(
                analysis_options::ContextualIssue::incompatible_lint_included(
                    &edge.uri,
                    &conflicting,
                    file_count,
                    (edge.offset, edge.length),
                ),
            );
        }
        // The edge's enabled rules join `seen` — locally disabled names
        // never do (:176-184).
        for rule in &child.enabled_rules {
            let Some(key) = key_of(&rule.name) else {
                continue;
            };
            if local_disabled_keys.contains(&key) {
                continue;
            }
            seen.insert(key, (edge_index, rule));
        }
    }

    // ---- incompatible_lint_files (validator :188-225): local enabled rules
    // vs the merged effective enabled set of this file's includes. Local
    // declarations — enabled or disabled — remove their same-named entries
    // from the included set first (:199-205).
    let mut active: Vec<&analysis_options::RuleData> = Vec::new();
    for edge in edges {
        let child = &groups[edge.group];
        for rule in &child.disabled_rules {
            if let Some(position) = active
                .iter()
                .position(|entry| entry.name.eq_ignore_ascii_case(&rule.name))
            {
                active.remove(position);
            }
        }
        for rule in &child.enabled_rules {
            match active
                .iter_mut()
                .find(|entry| entry.name.eq_ignore_ascii_case(&rule.name))
            {
                Some(slot) => *slot = rule,
                None => active.push(rule),
            }
        }
    }
    for rule in local {
        if let Some(key) = key_of(&rule.name)
            && let Some(position) = active
                .iter()
                .position(|entry| entry.name.eq_ignore_ascii_case(&key))
        {
            active.remove(position);
        }
    }
    for rule in local_enabled {
        // `findIncompatibleRules` over the included set, in the declared
        // `incompatibleRules` order (:85-97, :207-224). The result rows are
        // the conflicting declarations themselves — the per-file fold below
        // mirrors upstream's `Map<file.path, node>` shape.
        let mut conflicting: Vec<&analysis_options::RuleData> = Vec::new();
        for incompatible in dartq_lints::incompatible_rules(&rule.name) {
            for entry in &active {
                if entry.name.eq_ignore_ascii_case(incompatible) {
                    conflicting.push(entry);
                }
            }
        }
        if conflicting.is_empty() {
            continue;
        }
        // The `{1}` join and the context rows are keyed by the conflicting
        // rules' files — upstream's `Map<file.path, node>` fold keeps the
        // first position and the last declaration per file (:214-219).
        let mut folded: Vec<&analysis_options::RuleData> = Vec::new();
        for entry in conflicting {
            match folded
                .iter_mut()
                .find(|slot| slot.file.as_path() == entry.file.as_path())
            {
                Some(slot) => *slot = entry,
                None => folded.push(entry),
            }
        }
        issues.push(analysis_options::ContextualIssue::incompatible_lint_files(
            &rule.name,
            &folded,
            (rule.offset, rule.length),
        ));
    }
    issues
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
/// `_reportUnnecessaryOrRemovedOrDeprecatedIgnores`, `ignore_validator.dart`
/// :199-240), gated on the file's governing options enabling the
/// `unnecessary_ignore` lint (`_validateUnnecessaryIgnores`,
/// `ignore_validator.dart:203` — the validator's caller wires the lint's
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
    // governing entry: a serial pre-pass resolves each distinct directory
    // once (the options walk stats ancestors) and keeps only the enabled
    // ones, so the fan-out below reads a frozen map.
    let mut governing: FxHashMap<&Path, Option<&analysis_options::AnalysisOptions>> =
        FxHashMap::default();
    let mut work: Vec<(FileId, &analysis_options::AnalysisOptions)> = Vec::new();
    for file in files {
        let Some(path) = engine.file_table().path(file) else {
            continue;
        };
        let dir = path.parent().unwrap_or(Path::new(""));
        let options = *governing.entry(dir).or_insert_with(|| {
            crate::lints::governing_options(engine, dir, roots).filter(|options| {
                options
                    .lint_rules
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case("unnecessary_ignore"))
            })
        });
        if let Some(options) = options {
            work.push((file, options));
        }
    }
    // One independent job per file, rows joined in ascending file order —
    // the serial walk's emission order. A file whose bytes never spell
    // `ignore` carries no ignore comment (both recognized forms,
    // `// ignore:` and `// ignore_for_file:`, contain it), so it skips the
    // scan outright: the walk re-scans only the files that can answer.
    let per_file: Vec<Vec<Diagnostic>> = work
        .par_iter()
        .map(|&(file, options)| removed_lint_use_rows_of(engine, file, options))
        .collect();
    per_file.into_iter().flatten().collect()
}

/// One file's `removed_lint_use` rows (see [`removed_lint_use_rows`]).
fn removed_lint_use_rows_of(
    engine: &Engine,
    file: FileId,
    options: &analysis_options::AnalysisOptions,
) -> Vec<Diagnostic> {
    let mut rows = Vec::new();
    {
        let unignorable = options.unignorable();
        let Some(bytes) = engine.content(file) else {
            return rows;
        };
        let Ok(text) = std::str::from_utf8(bytes) else {
            return rows;
        };
        if !text.contains("ignore") {
            return rows;
        }
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
                Some(format!("Remove the reference to '{name}'.")),
            ));
        }
    }
    rows
}

/// The memory-decomposition probe (#2210): with `DARTQ_RSS_DECOMPOSITION`
/// set (any value), the analyze faces print the engine's per-family
/// resident bytes ([`Engine::memory_report`]) to stderr once the run's
/// rows are final — the residency peak of a one-shot run — as
/// `dartq-rss <family> <bytes> <kind>` lines. Stderr only: the data
/// channel (stdout, every format) is untouched, so the parity face is
/// byte-identical armed or not. Disarmed, the probe is one environment
/// read per run.
fn report_memory(engine: &Engine) {
    use std::fmt::Write as _;
    if std::env::var_os("DARTQ_RSS_DECOMPOSITION").is_none() {
        return;
    }
    let mut out = String::new();
    for (family, bytes, kind) in engine.memory_report() {
        let _ = writeln!(out, "dartq-rss {family} {bytes} {kind}");
    }
    eprint!("{out}");
}

/// Whether `name` names a lint rule the official validity faces accept: any
/// registered rule of the pinned SDK's linter registry (`currentLintCodeNames`
/// + `_getRegisteredLint`, parse_model.dart:75-83, 1245-1251 — the full
/// `Registry.ruleRegistry`, not the implemented subset; probed 3.13.1:
/// `avoid_print` in `analyzer.errors` yields no row). #956. The membership
/// arm is the registry alone, so a dartq-implemented rule the pinned
/// registry dropped (`unnecessary_type_check`, the lint-tier mirror of the
/// analyzer hint) reports `undefined_lint` like any unregistered name —
/// the `analyzer.errors` face stays open to it through
/// [`analysis_options::is_known_diagnostic_code`] (a real HintCode).
fn is_known_lint_name(name: &str) -> bool {
    crate::lint_names::is_known(name)
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

/// The oracle display order over an analyzed engine — the one comparator
/// the CLI's analyze document and the daemon's `dartq/analyze` answer sort
/// their rows through (severity, path bytes, offset, message), public so
/// the daemon face keeps the same printed order as its CLI twin.
pub fn oracle_order<'a, I>(diagnostics: I, layout: &EngineLayout<'_>) -> Vec<&'a Diagnostic>
where
    I: IntoIterator<Item = &'a Diagnostic>,
{
    let mut rows: Vec<&Diagnostic> = diagnostics.into_iter().collect();
    rows.sort_by(|a, b| oracle_cmp(a, b, layout));
    rows
}

// ---------------------------------------------------------------------------
// The lon diagnostics document — the one builder every analyze face renders
// through (the CLI's `--format lon` default and the daemon's `dartq/analyze`
// answer; #2657: no second encoder exists)
// ---------------------------------------------------------------------------

/// The analyze/check faces' `lon` document: one flat `diags` row per
/// diagnostic — `{v root? diags:[{code sev file line col len msg}]}` —
/// the short-vocabulary spelling of the diagnostics set (the json face's
/// oracle envelope is a different, frozen dialect and stays untouched:
/// absolute paths, no root key, `documentation` per the oracle's url
/// rules).
///
/// The `file` column spells through the one file locator
/// (`dartq_commands::locate`): relative to the run's analysis root when
/// the file lies inside it, absolute otherwise. The envelope carries that
/// root exactly once (`root:<absolute path>`) so the relative spellings
/// are unambiguous — a mixed set (a workspace row beside a pub-cache row)
/// reads `root:/w/pkg` + `file:lib/a.dart` + `file:/Users/…/x.dart`. The
/// key rides only when rows exist and the run has one root (the empty
/// set is still `{v:1 diags:[]}`; a multi-root merge or a degenerate
/// root keeps absolute spellings and no key). The `doc` URL is off on
/// this face: `code` is the lookup key and `explain` returns the URL —
/// the oracle machine format carries none either.
///
/// # Row shape at small N
///
/// The lon crate's encoder already amortizes for row count: arrays below
/// two elements never form a record table (`lon` 0.1
/// `write_record_table`: `values.len() < 2` → the ordinary form), and the
/// two-row/two-field edge compares structural bytes before factoring.
/// One diagnostic therefore renders the object form
/// `diags:[{code:… sev:… …}]` — no table header. Measured on the m0
/// single-error row (located `file:main.dart`): the object form is 108
/// bytes, a forced single-row table (`[code sev file line col len
/// msg;…]`) 106 — a ±2-byte class delta that flips sign with value
/// lengths, not a lever. This function does no shape selection of its
/// own: the encoder's gate decides, deterministically for every N.
#[must_use]
pub fn diags_document(output: &AnalysisOutput<'_>) -> serde_json::Value {
    // One provider read for the whole document: the root decides every
    // row's spelling.
    let root = output.files.analysis_root();
    let rows = output
        .diagnostics
        .iter()
        .map(|diag| diag_row(diag, output, root))
        .collect::<Vec<_>>();
    match (root, rows.is_empty()) {
        (Some(root), false) => serde_json::json!({
            "v": 1,
            "root": root.display().to_string(),
            "diags": rows,
        }),
        _ => serde_json::json!({"v": 1, "diags": rows}),
    }
}

/// One `diags` row: the locator-spelled `file`, no `doc` (see
/// [`diags_document`]).
fn diag_row(
    diag: &Diagnostic,
    output: &AnalysisOutput<'_>,
    root: Option<&Path>,
) -> serde_json::Value {
    let (line, col) = output.files.line_col(diag.file, diag.range.offset);
    let path = output.files.file_path(diag.file);
    let file = match root {
        Some(root) => crate::locate(root, path),
        None => path.display().to_string(),
    };
    serde_json::json!({
        "code": diag.code.name,
        "sev": severity_word(diag.severity),
        "file": file,
        "line": line,
        "col": col,
        "len": diag.range.length,
        "msg": diag.message.to_string(),
    })
}

/// The `sev` value spelling: lowercase, the compact rows' own — the one
/// severity spelling the lon faces print (the json face's oracle envelope
/// keeps the analyzer's uppercase, in dartq-diagnostics).
#[must_use]
pub fn severity_word(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
    }
}

#[cfg(test)]
mod tests {
    use dartq_diagnostics::{
        AnalysisOutput, CodeInfo, DiagnosticType, FileId, Severity, write_json,
    };
    use dartq_engine::Engine;
    use dartq_pub::PackageConfig;
    use dartq_source::SourceRange;

    use super::*;
    use crate::workspace::{engine_config, test_fixture::workspace};

    /// A layout over the interned paths, optionally carrying an analysis
    /// root — the provider face the document locator tests project through.
    struct TableLayout {
        table: dartq_source::FileTable,
        root: Option<std::path::PathBuf>,
    }

    impl dartq_diagnostics::FileLayoutProvider for TableLayout {
        fn file_path(&self, file: FileId) -> &Path {
            self.table.path(file).expect("interned FileId")
        }

        fn analysis_root(&self) -> Option<&Path> {
            self.root.as_deref()
        }

        fn line_col(&self, _file: FileId, _offset: u32) -> (u32, u32) {
            (1, 27)
        }
    }

    fn published_diag(table: &mut dartq_source::FileTable, path: &Path) -> Diagnostic {
        let file = table.intern(path);
        Diagnostic::new(
            Severity::Error,
            dartq_diagnostics::UNDEFINED_IDENTIFIER,
            "Expected an identifier.",
            file,
            SourceRange {
                offset: 26,
                length: 1,
            },
            None,
        )
    }

    /// The locator policy on the document face: the envelope carries the
    /// root once, rows inside the root spell relative (no `package:`
    /// form, no leading `./`), rows outside keep the absolute spelling,
    /// and `doc` is off — even for a published code.
    #[test]
    fn diags_document_locates_files_and_carries_the_root() {
        let mut table = dartq_source::FileTable::new();
        let inside = table.intern(Path::new("/w/pkg/lib/a.dart"));
        let outside = table.intern(Path::new("/pub-cache/x/lib/b.dart"));
        let row = |file: FileId| {
            Diagnostic::new(
                Severity::Error,
                dartq_diagnostics::UNDEFINED_IDENTIFIER,
                "Expected an identifier.",
                file,
                SourceRange {
                    offset: 26,
                    length: 1,
                },
                None,
            )
        };
        let diagnostics = [row(inside), row(outside)];
        let files = TableLayout {
            table,
            root: Some(std::path::PathBuf::from("/w/pkg")),
        };
        let output = AnalysisOutput {
            diagnostics: &diagnostics,
            files: &files,
        };
        let document = diags_document(&output);
        assert_eq!(document["root"], "/w/pkg", "the root rides exactly once");
        let diags = document["diags"].as_array().expect("rows");
        assert_eq!(diags[0]["file"], "lib/a.dart", "inside the root: relative");
        assert_eq!(
            diags[1]["file"], "/pub-cache/x/lib/b.dart",
            "outside the root: absolute"
        );
        assert!(
            diags.iter().all(|row| row.get("doc").is_none()),
            "the doc URL is off on this face: {}",
            document
        );
    }

    /// The no-root and empty-set faces: a provider without one root keeps
    /// absolute spellings and no `root` key, and the clean run is still
    /// the bare `{v:1 diags:[]}` document.
    #[test]
    fn diags_document_without_a_root_or_rows() {
        let mut table = dartq_source::FileTable::new();
        let diagnostics = [published_diag(&mut table, Path::new("/w/pkg/lib/a.dart"))];
        let files = TableLayout { table, root: None };
        let output = AnalysisOutput {
            diagnostics: &diagnostics,
            files: &files,
        };
        let document = diags_document(&output);
        assert!(document.get("root").is_none(), "no root to carry");
        assert_eq!(
            document["diags"][0]["file"], "/w/pkg/lib/a.dart",
            "absolute spelling stands"
        );

        let empty: [Diagnostic; 0] = [];
        let output = AnalysisOutput {
            diagnostics: &empty,
            files: &files,
        };
        assert_eq!(
            diags_document(&output),
            serde_json::json!({"v": 1, "diags": []}),
            "the empty set carries no root"
        );
    }

    /// The small-N row shape gate (pinned so a codec change is a visible
    /// contract change): the row vocabulary is exactly the seven compact
    /// keys, in the builder's own order — the lon byte shapes themselves
    /// are pinned by the CLI face's tests over this same builder.
    #[test]
    fn diags_document_row_vocabulary() {
        let mut table = dartq_source::FileTable::new();
        let mut files = TableLayout {
            table,
            root: Some(std::path::PathBuf::from("/w/pkg")),
        };
        let one = [published_diag(
            &mut files.table,
            Path::new("/w/pkg/lib/a.dart"),
        )];
        let output = AnalysisOutput {
            diagnostics: &one,
            files: &files,
        };
        let document = diags_document(&output);
        assert_eq!(
            document,
            serde_json::json!({
                "v": 1,
                "root": "/w/pkg",
                "diags": [{
                    "code": "undefined_identifier",
                    "sev": "error",
                    "file": "lib/a.dart",
                    "line": 1,
                    "col": 27,
                    "len": 1,
                    "msg": "Expected an identifier.",
                }],
            }),
            "the whole document, as the value the lon face encodes"
        );
    }

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

        let mut diagnostics = [
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

    /// #2589: recovery-shaped sources must run through the analyze face
    /// and render through the JSON writer without the position conversion
    /// panicking. Parser recovery reports the eof-anchored `expected_token`
    /// row as length-1 at the eof offset (the official `translateErrorToken`
    /// law), so the writer converts that row's **end** position at eof+1 —
    /// through this exact production composition the run formerly died as
    /// "UTF-16 offset 15 out of range" (a 14-unit source) and the CLI
    /// catch-all surfaced it as an internal error. The bounded behavior is
    /// the official `getLocation` extrapolation (`line_col_utf16`'s total
    /// contract in dartq-source): the row renders start (2,1), end (2,2).
    #[test]
    fn recovery_shaped_sources_render_through_the_json_writer() {
        for source in ["void main( {}\n", "void main() {\n"] {
            let root = workspace("analyze_recovery", &[("lib/main.dart", source)])
                .canonicalize()
                .expect("canonical root");
            let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
            let mut engine = Engine::new(config).expect("engine");
            let analyzed = run(&mut engine, std::slice::from_ref(&root)).expect("run");
            // The eof-anchored row is present — offset 14 (the 14-unit
            // source's eof), length 1, so its end converts at 15. Pinning
            // the trigger keeps this test from passing vacuously if the
            // parser's recovery shape ever shifts.
            assert!(
                analyzed
                    .diagnostics
                    .iter()
                    .any(|d| d.range.offset == 14 && d.range.end() == 15),
                "the eof-anchored recovery row is present: {:?}",
                analyzed.diagnostics
            );
            let mut json = Vec::new();
            write_json(
                &mut json,
                &AnalysisOutput {
                    diagnostics: &analyzed.diagnostics,
                    files: &analyzed.layout,
                },
            )
            .expect("write");
            let json = String::from_utf8(json).expect("utf-8");
            assert!(
                json.contains(r#""end":{"offset":15,"line":2,"column":2}"#),
                "the eof+1 end position extrapolates onto the final line: {json}"
            );
            std::fs::remove_dir_all(&root).ok();
        }
    }

    /// The overlay run's contract: an empty overlay list is the
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
    /// row (`computeSeverity`, `analyzer_cli` error_severity.dart:14-26).
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

    /// The `cannot-ignore` arm of the same face (#2387): a list entry that
    /// is neither a severity name nor a known diagnostic/lint code reports
    /// `unrecognized_error_code` at its entry anchor (parse_model:346-405 —
    /// the check rides the uppercased form; the severity arm skips it).
    /// Oracle-probed 3.15 main, 2026-10-20: initial-file rows raw, WARNING,
    /// value-token span, declaration order. The severity arm (`error`), a
    /// registered lint name (`prefer_final_locals`), and a known diagnostic
    /// (`undefined_identifier`) stay silent.
    #[test]
    fn run_overlaid_reports_unrecognized_error_code_rows_for_cannot_ignore() {
        let root = workspace(
            "analyze_cannot_ignore_unknown",
            &[("lib/ok.dart", "var ok = 1;\n")],
        );
        let root = root.canonicalize().expect("canonical root");
        let options_text = concat!(
            "analyzer:\n",
            "  cannot-ignore:\n",
            "    - not_a_real_code_name\n",
            "    - error\n",
            "    - prefer_final_locals\n",
            "    - undefined_identifier\n",
            "    - also_fake_code\n",
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
                    "'not_a_real_code_name' isn't a recognized diagnostic code.",
                    Severity::Warning,
                    u32::try_from(options_text.find("not_a_real_code_name").expect("anchor"))
                        .expect("u32"),
                    20,
                ),
                (
                    expected_path,
                    "'also_fake_code' isn't a recognized diagnostic code.",
                    Severity::Warning,
                    u32::try_from(options_text.find("also_fake_code").expect("anchor"))
                        .expect("u32"),
                    14,
                ),
            ],
            "only the unknown names report, in declaration order: {rows:?}"
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
        let root = workspace("analyze_removed_lint", &[("lib/ok.dart", "var ok = 1;\n")]);
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
    /// (registered in the SDK — no `undefined_lint`) reports with the
    /// registry texts (diagnostic.g.dart `deprecatedLint`) once the
    /// package's constraint reaches the deprecation's since (3.11.0,
    /// parse_model.dart:1288-1296) — and the row keys on the name alone:
    /// the rule is implemented in dartq-lints (the flipped rule_states
    /// pin), which must not drop the row (#1322 F1).
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
                u32::try_from(
                    options_text
                        .find("prefer_final_parameters")
                        .expect("anchor")
                )
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

    /// The 3.13.0 deprecation wave (#2042 W-F, the census `deprecated_lint`
    /// delta): `avoid_private_typedef_functions` and `one_member_abstracts`
    /// carry `state: .deprecated(since: Version(3, 13, 0))` in the pinned
    /// registry (upstream Dart SDK source, `pkg/linter/lib/src/rules/
    /// avoid_private_typedef_functions.dart:23` and `one_member_abstracts.dart:24`,
    /// neither with a `replacedBy`), so both names render the plain
    /// `deprecated_lint` row once the constraint reaches the since — the
    /// corpus root options file's two missing oracle rows.
    #[test]
    fn run_overlaid_reports_the_three_thirteen_deprecations() {
        let root = workspace(
            "analyze_deprecated_lint_3_13",
            &[("lib/ok.dart", "var ok = 1;\n")],
        );
        std::fs::write(
            root.join("pubspec.yaml"),
            "name: fixture\nversion: 0.1.0\nenvironment:\n  sdk: ^3.13.0\n",
        )
        .expect("pubspec");
        let root = root.canonicalize().expect("canonical root");
        let options_text = concat!(
            "linter:\n",
            "  rules:\n",
            "    avoid_private_typedef_functions: true\n",
            "    one_member_abstracts: true\n",
        );
        std::fs::write(root.join("analysis_options.yaml"), options_text).expect("options");
        let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");

        let rows: Vec<(&str, u32, u32)> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "deprecated_lint")
            .map(|d| (d.message.as_ref(), d.range.offset, d.range.length))
            .collect();
        assert_eq!(
            rows,
            vec![
                (
                    "The lint rule 'avoid_private_typedef_functions' is deprecated and \
                     shouldn't be enabled.",
                    u32::try_from(
                        options_text
                            .find("avoid_private_typedef_functions")
                            .expect("anchor")
                    )
                    .expect("u32"),
                    u32::try_from("avoid_private_typedef_functions".len()).expect("u32"),
                ),
                (
                    "The lint rule 'one_member_abstracts' is deprecated and shouldn't be \
                     enabled.",
                    u32::try_from(options_text.find("one_member_abstracts").expect("anchor"))
                        .expect("u32"),
                    u32::try_from("one_member_abstracts".len()).expect("u32"),
                ),
            ],
            "both 3.13.0 deprecations render at their name anchors: {rows:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The dominant posture: normal rule names — implemented or
    /// registered-but-unimplemented — carry no removed/deprecated rows
    /// (#988's zero branch).
    #[test]
    fn normal_rule_names_carry_no_removed_or_deprecated_rows() {
        let root = workspace("analyze_normal_lints", &[("lib/ok.dart", "var ok = 1;\n")]);
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
            run.diagnostics
                .iter()
                .map(|d| d.code.name)
                .collect::<Vec<_>>()
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The `duplicate_rule` face end to end (`_processEnabledRule`,
    /// parse_model.dart:1397-1404): enabled declarations repeat-check
    /// against the rules already taken, in declaration order — the first
    /// occurrence wins, every later enabled one reports at its name anchor
    /// with the registry texts, and registry identity is case-insensitive
    /// (the official mixed-case duplicate test pins `staBle_lint`). A
    /// disabled entry never joins the taken set (no report, no take), a
    /// registered-but-unimplemented name takes and reports like any
    /// registry entry, and an unregistered name stays on the
    /// `undefined_lint` face only.
    #[test]
    fn run_overlaid_reports_duplicate_rule_rows() {
        let root = workspace(
            "analyze_duplicate_rule",
            &[("lib/ok.dart", "var ok = 1;\n")],
        );
        let root = root.canonicalize().expect("canonical root");
        let rows_for = |options: &str| -> Vec<(String, Option<String>, u32)> {
            std::fs::write(root.join("analysis_options.yaml"), options).expect("options");
            let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
            let mut engine = Engine::new(config).expect("engine");
            let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");
            run.diagnostics
                .iter()
                .filter(|d| d.code.name == "duplicate_rule")
                .map(|d| {
                    (
                        d.message.clone().into_owned(),
                        d.correction.clone().map(|text| text.to_string()),
                        d.range.offset,
                    )
                })
                .collect()
        };
        let offset_of = |options: &str, name: &str, nth: usize| -> u32 {
            u32::try_from(options.match_indices(name).nth(nth).expect("occurrence").0).expect("u32")
        };
        let message = |name: &str| {
            format!("The rule '{name}' is already enabled and doesn't need to be enabled again.")
        };

        let options = "linter:\n  rules:\n    - avoid_empty_else\n    - avoid_empty_else\n";
        assert_eq!(
            rows_for(options),
            vec![(
                message("avoid_empty_else"),
                Some("Try removing all but one occurrence of the rule.".to_owned()),
                offset_of(options, "avoid_empty_else", 1),
            )],
            "the second enabled occurrence reports, the first takes"
        );

        // Mixed-case spellings resolve to one registry identity; the row
        // names the written form.
        let options = "linter:\n  rules:\n    - avoid_empty_else\n    - AVOID_EMPTY_ELSE\n";
        assert_eq!(
            rows_for(options),
            vec![(
                message("AVOID_EMPTY_ELSE"),
                Some("Try removing all but one occurrence of the rule.".to_owned()),
                offset_of(options, "AVOID_EMPTY_ELSE", 0),
            )],
            "the written spelling renders"
        );

        // Disabled entries never join: no report, no take — the later
        // enabled occurrence is the first taker and stays silent.
        assert!(
            rows_for(
                "linter:\n  rules:\n    avoid_empty_else: false\n    avoid_empty_else: true\n",
            )
            .is_empty()
        );

        // A registered-but-unimplemented name takes and reports like any
        // registry entry; an unregistered name never reaches this face.
        let options = "linter:\n  rules:\n    - avoid_print\n    - avoid_print\n";
        assert_eq!(
            rows_for(options),
            vec![(
                message("avoid_print"),
                Some("Try removing all but one occurrence of the rule.".to_owned()),
                offset_of(options, "avoid_print", 1),
            )],
            "the registry resolves the unimplemented name"
        );
        assert!(rows_for("linter:\n  rules:\n    - not_a_lint\n    - not_a_lint\n").is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    /// The analyze face's plugin weave over the real `run_targets` path,
    /// seam-tested with a dead plugin set (the crash-row posture needs no
    /// live child): the one-shot `dartq/plugin_crashed` row joins the
    /// covered emit set against the analyzed set's anchor — even though the
    /// engine rows here are non-empty, the anchor is the run's analyzed
    /// state, not the first row — and the `None` set (the `--no-plugins`
    /// posture) contributes nothing.
    #[test]
    fn run_targets_weaves_plugin_crash_rows_and_the_opt_out_stays_silent() {
        let root = workspace(
            "analyze_plugin_weave",
            &[("lib/main.dart", "void f() { x }\n")],
        );
        let root = root.canonicalize().expect("canonical root");
        let targets = vec![Target::Dir(root.clone())];

        let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let mut set = PluginSet::with_dead_plugin("demo", "spawn refused");
        let run =
            run_targets(&mut engine, &[], &[root.clone()], &targets, Some(&mut set)).expect("run");
        set.shutdown();
        let crashes: Vec<_> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "dartq/plugin_crashed")
            .collect();
        assert_eq!(crashes.len(), 1, "the crash row joins the emit set");
        assert_eq!(crashes[0].severity, Severity::Error);
        assert!(
            crashes[0].message.contains("demo") && crashes[0].message.contains("spawn refused"),
            "the crash message names the plugin and the cause: {}",
            crashes[0].message
        );

        // The opt-out: the same run without a set emits no plugin rows —
        // the flag's skip seam, one `None` short of the whole weave.
        let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = run_targets(&mut engine, &[], &[root.clone()], &targets, None).expect("run");
        assert!(
            run.diagnostics
                .iter()
                .all(|d| !d.code.name.starts_with("dartq/")),
            "no plugin rows without a set: {:?}",
            run.diagnostics
                .iter()
                .map(|d| d.code.name)
                .collect::<Vec<_>>()
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The launch seam over the real discovery face, driven from a
    /// `path:`-declared `plugins:` section: the manifest resolves through
    /// the augmented package config and registers a lazy runtime — the set
    /// is non-empty with zero sidecar processes started (the spawn lands on
    /// the first cycle that has something to feed), and the plugin-free
    /// workspace next door discovers nothing. Production path end to end;
    /// only the child process stays out of the unit tier.
    #[test]
    fn launch_discovers_path_declared_plugins_and_the_plugin_free_workspace_stays_empty() {
        let declared = workspace(
            "plugin_launch_declared",
            &[("lib/main.dart", "var x = 1;\n")],
        );
        std::fs::write(
            declared.join("analysis_options.yaml"),
            "plugins:\n  demo:\n    path: plugins/demo\n",
        )
        .expect("options");
        let dir = declared.join("plugins/demo");
        std::fs::create_dir_all(dir.join("lib")).expect("plugin tree");
        std::fs::write(
            dir.join("dartq_plugin.json"),
            r#"{"name": "demo", "version": "1.0.0", "entrypoint": "lib/main.dart"}"#,
        )
        .expect("manifest");
        std::fs::write(dir.join("lib/main.dart"), "void main() {}\n").expect("entrypoint");
        let config =
            PackageConfig::parse(r#"{"configVersion": 2, "packages": []}"#).expect("config");
        let set = PluginSet::launch(&declared, &config, &declared);
        assert!(!set.is_empty(), "the declared plugin registers lazily");
        set.shutdown();

        let plain = workspace("plugin_launch_plain", &[("lib/main.dart", "var x = 1;\n")]);
        let set = PluginSet::launch(&plain, &config, &plain);
        assert!(set.is_empty(), "no declaration, no convention dir, no set");
    }

    /// The reachability face's production join (#1729 regression pin): the
    /// lint stage mounts the engine's capture-derived flags over a fresh
    /// parse of the same file, and the `unreachable_from_main` judge must
    /// anchor every candidate at its name TOKEN's UTF-16 offset — the
    /// capture row's `range` convention. A judge anchor that consumed an
    /// arena node id as a token index panicked `utf16_token_span` ("token
    /// outside the table") on this fixture through this exact driver (the
    /// `dartq analyze` face): the enum-family shapes mint more arena rows
    /// than the token table, so name node ids land past it. The two
    /// unreachable public declarations — the never-read field of a
    /// constructed class and the unreferenced top-level function — report
    /// at their name offsets; the entry point and the constructed class
    /// stay silent.
    #[test]
    fn run_overlaid_reachability_face_reports_unreachable_members() {
        let content = "void main() {\n  C();\n}\n\nclass C {\n  int? f;\n}\n\nvoid g() {}\n";
        let root = workspace(
            "analyze_unreachable_from_main",
            &[
                ("lib/unreachable_from_main.dart", content),
                (
                    "analysis_options.yaml",
                    "linter:\n  rules:\n    - unreachable_from_main\n",
                ),
            ],
        );
        let root = root.canonicalize().expect("canonical root");
        let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");

        let offset_of = |needle: &str| {
            u32::try_from(content.match_indices(needle).next().expect("occurrence").0).expect("u32")
        };
        let rows: Vec<(&str, u32)> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "unreachable_from_main")
            .map(|d| (d.message.as_ref(), d.range.offset))
            .collect();
        assert_eq!(
            rows,
            vec![
                (
                    "Unreachable member 'f' in an executable library.",
                    offset_of("f"),
                ),
                (
                    "Unreachable member 'g' in an executable library.",
                    offset_of("g"),
                ),
            ],
            "the unreachable field and function report at their name offsets"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The `unsupported_value` face for lint entries (`parseRule`'s
    /// fall-through arm, parse_model.dart:1272-1284): a scalar that is
    /// neither a bool nor a legal severity string reports the registry
    /// texts with the `_validLintValues` or-join proposal and disables the
    /// entry; legal severities and booleans stay silent, and the registry
    /// gate holds — an unregistered name reports `undefined_lint` and its
    /// value is never inspected. Rows anchor at the declared name (the
    /// facts face's span; the official row anchors at the value node).
    #[test]
    fn run_overlaid_reports_unsupported_lint_value_rows() {
        let root = workspace(
            "analyze_unsupported_value",
            &[("lib/ok.dart", "var ok = 1;\n")],
        );
        let root = root.canonicalize().expect("canonical root");
        let options_text = concat!(
            "linter:\n",
            "  rules:\n",
            "    avoid_empty_else: 42\n",
            "    avoid_catches_without_on_clauses: maybe\n",
            "    avoid_print: 42\n",
            "    not_a_lint: 42\n",
            "    collection_methods_unrelated_type: warning\n",
            "    valid_regexps: ignore\n",
            "    hash_and_equals: true\n",
        );
        std::fs::write(root.join("analysis_options.yaml"), options_text).expect("options");
        let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");

        let rows: Vec<(&str, u32)> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "unsupported_value")
            .map(|d| (d.message.as_ref(), d.range.offset))
            .collect();
        assert_eq!(
            rows,
            vec![
                (
                    "The value '42' isn't supported by 'avoid_empty_else'.",
                    u32::try_from(options_text.find("avoid_empty_else").expect("anchor"))
                        .expect("u32"),
                ),
                (
                    "The value 'maybe' isn't supported by 'avoid_catches_without_on_clauses'.",
                    u32::try_from(
                        options_text
                            .find("avoid_catches_without_on_clauses")
                            .expect("anchor")
                    )
                    .expect("u32"),
                ),
                (
                    "The value '42' isn't supported by 'avoid_print'.",
                    u32::try_from(options_text.find("avoid_print").expect("anchor")).expect("u32"),
                ),
            ],
            "illegal scalars report, legal ones and unregistered names do not: {rows:?}"
        );
        let row = run
            .diagnostics
            .iter()
            .find(|d| d.code.name == "unsupported_value")
            .expect("the rows above");
        assert_eq!(row.code.machine, "UNSUPPORTED_VALUE");
        assert_eq!(row.code.dtype, DiagnosticType::StaticWarning);
        assert_eq!(
            row.correction.as_deref(),
            Some(
                "Try using one of the supported values: 'true', 'false', 'ignore', 'info', 'warning', or 'error'.",
            ),
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The `incompatible_lint` face end to end (`_processEnabledRule`,
    /// parse_model.dart:1385-1395): each enabled declaration checks the
    /// rules already taken against its registry `incompatibleRules` list
    /// and reports the quoted join nested inside the template's own quotes
    /// — the official rendering (the parser test pins `''rule_pos''`). A
    /// disabled conflicting entry neither reports nor conflicts, the join
    /// follows the declared list's order across multiple conflicts, and
    /// either direction of a pair reports on the later declaration.
    #[test]
    fn run_overlaid_reports_incompatible_lint_rows() {
        let root = workspace(
            "analyze_incompatible_lint",
            &[("lib/ok.dart", "var ok = 1;\n")],
        );
        let root = root.canonicalize().expect("canonical root");
        let rows_for = |options: &str| -> Vec<(String, Option<String>, u32)> {
            std::fs::write(root.join("analysis_options.yaml"), options).expect("options");
            let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
            let mut engine = Engine::new(config).expect("engine");
            let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");
            run.diagnostics
                .iter()
                .filter(|d| d.code.name == "incompatible_lint")
                .map(|d| {
                    (
                        d.message.clone().into_owned(),
                        d.correction.clone().map(|text| text.to_string()),
                        d.range.offset,
                    )
                })
                .collect()
        };
        let correction = Some("Try removing all but one of the incompatible rules.".to_owned());

        let options = "linter:\n  rules:\n    - prefer_double_quotes\n    - prefer_single_quotes\n";
        assert_eq!(
            rows_for(options),
            vec![(
                "The rule 'prefer_single_quotes' is incompatible with ''prefer_double_quotes''."
                    .to_owned(),
                correction.clone(),
                u32::try_from(options.find("prefer_single_quotes").expect("anchor")).expect("u32"),
            )],
            "the later declaration reports the nested-quote join"
        );

        // Either direction of a pair reports on the later declaration.
        let options = "linter:\n  rules:\n    - prefer_single_quotes\n    - prefer_double_quotes\n";
        assert_eq!(
            rows_for(options),
            vec![(
                "The rule 'prefer_double_quotes' is incompatible with ''prefer_single_quotes''."
                    .to_owned(),
                correction.clone(),
                u32::try_from(options.find("prefer_double_quotes").expect("anchor")).expect("u32"),
            )],
            "the reverse pair reports the same way"
        );

        // Multiple conflicts join in the declared rule's list order (the
        // Oxford `and` form), one row on the last declaration only.
        let options = concat!(
            "linter:\n",
            "  rules:\n",
            "    - prefer_final_locals\n",
            "    - prefer_final_parameters\n",
            "    - unnecessary_final\n",
        );
        assert_eq!(
            rows_for(options),
            vec![(
                "The rule 'unnecessary_final' is incompatible with ''prefer_final_locals' and 'prefer_final_parameters''.".to_owned(),
                correction.clone(),
                u32::try_from(options.find("unnecessary_final").expect("anchor"))
                    .expect("u32"),
            )],
            "the earlier declarations' conflicts are not yet taken"
        );

        // A disabled conflicting entry neither reports nor conflicts.
        assert!(
            rows_for(
                "linter:\n  rules:\n    - prefer_double_quotes\n    prefer_single_quotes: false\n",
            )
            .is_empty()
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The `incompatible_lint_included` face end to end
    /// (`reportIncompatibleIncluded`, linter_rule_options_validator.dart
    /// :105-186): incompatible rules enabled through *different* include
    /// entries of one file report once — at the later entry's include URI
    /// token, `{0}` the URI text, `{1}` the quoted join of the conflicting
    /// rules' written spellings (the current edge's rule first, then the
    /// earlier contributors'), `{2}` the count of distinct include
    /// directives they came in through. Compatible sets never report, a
    /// local disable suppresses the conflict from every include, and the
    /// same-family correction lands.
    #[test]
    fn run_overlaid_reports_incompatible_lint_included_rows() {
        let root = workspace(
            "analyze_incompatible_included",
            &[("lib/ok.dart", "var ok = 1;\n")],
        );
        let root = root.canonicalize().expect("canonical root");
        // The two included files each enable one half of the incompatible
        // pair (the registry pins `prefer_double_quotes` ↔
        // `prefer_single_quotes`, registry.rs:558-566).
        std::fs::write(
            root.join("a.yaml"),
            "linter:\n  rules:\n    - prefer_double_quotes\n",
        )
        .expect("a.yaml");
        std::fs::write(
            root.join("b.yaml"),
            "linter:\n  rules:\n    - prefer_single_quotes\n",
        )
        .expect("b.yaml");
        let rows_for = |options: &str| -> Vec<(String, Option<String>, u32)> {
            std::fs::write(root.join("analysis_options.yaml"), options).expect("options");
            let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
            let mut engine = Engine::new(config).expect("engine");
            let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");
            run.diagnostics
                .iter()
                .filter(|d| d.code.name == "incompatible_lint")
                .map(|d| {
                    (
                        d.message.clone().into_owned(),
                        d.correction.clone().map(|text| text.to_string()),
                        d.range.offset,
                    )
                })
                .collect()
        };
        let correction = Some(
            "Try locally disabling all but one of the conflicting rules or \
             removing one of the incompatible files."
                .to_owned(),
        );

        // The pair fires on the later include entry: `{0}` is its URI text,
        // `{1}` the current edge's rule first then the earlier edge's, `{2}`
        // the two distinct include directives, and the row anchors at the
        // `b.yaml` token inside the include list.
        let options = concat!(
            "include:\n",
            "  - a.yaml\n",
            "  - b.yaml\n",
            "linter:\n",
            "  rules:\n",
            "    - avoid_print\n",
        );
        assert_eq!(
            rows_for(options),
            vec![(
                "The rule 'b.yaml' is incompatible with 'prefer_single_quotes' and \
                 'prefer_double_quotes', which is included from 2 files."
                    .to_owned(),
                correction.clone(),
                u32::try_from(options.find("b.yaml").expect("anchor")).expect("u32"),
            )],
            "the later include entry reports at its own URI token"
        );

        // The same pair one include deep reports `1 file` — both
        // conflicting rules came in through the single directive.
        let options = "include: pair.yaml\n";
        std::fs::write(
            root.join("pair.yaml"),
            "linter:\n  rules:\n    - prefer_double_quotes\n    - prefer_single_quotes\n",
        )
        .expect("pair.yaml");
        assert_eq!(
            rows_for(options),
            vec![],
            "one include carries the whole subtree — no cross-entry conflict"
        );

        // Compatible sets never report.
        std::fs::write(
            root.join("b.yaml"),
            "linter:\n  rules:\n    - avoid_print\n",
        )
        .expect("b.yaml");
        assert!(
            rows_for(concat!(
                "include:\n",
                "  - a.yaml\n",
                "  - b.yaml\n",
                "linter:\n",
                "  rules:\n",
                "    - avoid_print\n",
            ))
            .is_empty()
        );

        // A local disable suppresses the conflict every include contributes
        // to (`disabledRuleSet`, validator :121-123) — the same shape the
        // `multipleIncludes_disabledInMain` parser test pins.
        std::fs::write(
            root.join("b.yaml"),
            "linter:\n  rules:\n    - prefer_single_quotes\n",
        )
        .expect("b.yaml");
        assert!(
            rows_for(concat!(
                "include:\n",
                "  - a.yaml\n",
                "  - b.yaml\n",
                "linter:\n",
                "  rules:\n",
                "    prefer_single_quotes: false\n",
            ))
            .is_empty()
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The `incompatible_lint_files` face end to end
    /// (`reportIncompatibleWithIncluded`, linter_rule_options_validator.dart
    /// :188-225): a *locally* enabled rule conflicting with the include
    /// chain's effective enabled set reports at the local declaration's name
    /// token — `{0}` the local spelling, `{1}` the quoted join of the
    /// conflicting included rules' written spellings in the declared
    /// `incompatibleRules` order — while a local disable removes the
    /// same-named entry from the included set before the comparison, so a
    /// disabled local never reports and never shadows.
    #[test]
    fn run_overlaid_reports_incompatible_lint_files_rows() {
        let root = workspace(
            "analyze_incompatible_files",
            &[("lib/ok.dart", "var ok = 1;\n")],
        );
        let root = root.canonicalize().expect("canonical root");
        std::fs::write(
            root.join("inc.yaml"),
            "linter:\n  rules:\n    - prefer_double_quotes\n",
        )
        .expect("inc.yaml");
        let rows_for = |options: &str| -> Vec<(String, Option<String>, u32)> {
            std::fs::write(root.join("analysis_options.yaml"), options).expect("options");
            let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
            let mut engine = Engine::new(config).expect("engine");
            let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");
            run.diagnostics
                .iter()
                .filter(|d| d.code.name == "incompatible_lint")
                .map(|d| {
                    (
                        d.message.clone().into_owned(),
                        d.correction.clone().map(|text| text.to_string()),
                        d.range.offset,
                    )
                })
                .collect()
        };
        let correction = Some(
            "Try locally disabling all but one of the conflicting rules or \
             removing one of the incompatible files."
                .to_owned(),
        );

        // The local enable conflicts with the included effective set — the
        // row anchors at the local `prefer_single_quotes` token.
        let options = concat!(
            "include: inc.yaml\n",
            "linter:\n",
            "  rules:\n",
            "    - prefer_single_quotes\n",
        );
        assert_eq!(
            rows_for(options),
            vec![(
                "The rule 'prefer_single_quotes' is incompatible with \
                 'prefer_double_quotes'."
                    .to_owned(),
                correction.clone(),
                u32::try_from(options.find("prefer_single_quotes").expect("anchor")).expect("u32"),
            )],
            "the local declaration reports against the included enable"
        );

        // Disabling the conflicting name locally removes it from the
        // included set — no row (the `includedFile_disabledInMain` parser
        // test pins this suppression arm).
        assert!(
            rows_for(concat!(
                "include: inc.yaml\n",
                "linter:\n",
                "  rules:\n",
                "    prefer_double_quotes: false\n",
                "    prefer_single_quotes: true\n",
            ))
            .is_empty(),
            "a local disable shadows the same-named included enable"
        );

        // An `ignore` value is a disable for the validator — the included
        // enable is shadowed the same way (the `disabledInIncludedFile`
        // family pins `ignore` as a disabled-side declaration).
        assert!(
            rows_for(concat!(
                "include: inc.yaml\n",
                "linter:\n",
                "  rules:\n",
                "    prefer_double_quotes: ignore\n",
                "    prefer_single_quotes: true\n",
            ))
            .is_empty(),
            "an ignore-valued local disables for the conflict check"
        );

        // A compatible local enable reports nothing.
        assert!(
            rows_for(concat!(
                "include: inc.yaml\n",
                "linter:\n",
                "  rules:\n",
                "    - avoid_print\n",
            ))
            .is_empty()
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The wrapped face: a conflict *inside* an included file reports
    /// through that file's reporter, which wraps the row into
    /// `included_file_warning` at the branch's first include directive in
    /// the initial file (`_IncludedDiagnosticListener`, include_walker.dart
    /// :210-248) — the message names the included file and the inner span's
    /// inclusive end; the inner correction never survives the wrap. The
    /// nested pair's own conflict — sibling includes of `mid.yaml` — lands
    /// the same way, one level down.
    #[test]
    fn run_overlaid_wraps_incompatible_lint_rows_from_included_files() {
        let root = workspace(
            "analyze_incompatible_wrapped",
            &[("lib/ok.dart", "var ok = 1;\n")],
        );
        let root = root.canonicalize().expect("canonical root");
        // `top.yaml` enables one half locally and includes the other half —
        // its `incompatible_lint_files` row wraps into the initial file.
        std::fs::write(
            root.join("inc.yaml"),
            "linter:\n  rules:\n    - prefer_double_quotes\n",
        )
        .expect("inc.yaml");
        let top = concat!(
            "include: inc.yaml\n",
            "linter:\n",
            "  rules:\n",
            "    - prefer_single_quotes\n",
        );
        std::fs::write(root.join("top.yaml"), top).expect("top.yaml");
        let options = "include: top.yaml\n";
        std::fs::write(root.join("analysis_options.yaml"), options).expect("options");
        let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");

        let wrapped: Vec<(String, u32)> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "included_file_warning")
            .map(|d| (d.message.clone().into_owned(), d.range.offset))
            .collect();
        let top_path = root.join("top.yaml").display().to_string();
        let inner_start =
            u32::try_from(top.find("prefer_single_quotes").expect("anchor")).expect("u32");
        let inner_end = inner_start + "prefer_single_quotes".len() as u32 - 1;
        assert_eq!(
            wrapped,
            vec![(
                format!(
                    "Warning in the included options file {top_path}({inner_start}..{inner_end}): \
                     The rule 'prefer_single_quotes' is incompatible with 'prefer_double_quotes'."
                ),
                u32::try_from(options.find("top.yaml").expect("anchor")).expect("u32"),
            )],
            "the included file's row wraps at the branch's first directive"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The wrapped face of the same `cannot-ignore` arm (#2387): an unknown
    /// name inside an included file reports through that file's reporter,
    /// wrapping into `included_file_warning` at the branch's first include
    /// directive in the initial file — the message names the included file
    /// and the inner span's inclusive end (the oracle probe pins the
    /// `opts.yaml(33..52)` form verbatim, 3.15 main, 2026-10-20). Severity,
    /// lint, and known-code entries in the included list stay silent.
    #[test]
    fn run_overlaid_wraps_unrecognized_error_code_rows_from_included_files() {
        let root = workspace(
            "analyze_cannot_ignore_wrapped",
            &[("lib/ok.dart", "var ok = 1;\n")],
        );
        let root = root.canonicalize().expect("canonical root");
        let inc = concat!(
            "analyzer:\n",
            "  cannot-ignore:\n",
            "    - another_unknown_code\n",
            "    - error\n",
            "    - prefer_final_locals\n",
        );
        std::fs::write(root.join("inc.yaml"), inc).expect("inc.yaml");
        let options = "include: inc.yaml\n";
        std::fs::write(root.join("analysis_options.yaml"), options).expect("options");
        let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");

        let wrapped: Vec<(String, u32)> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "included_file_warning")
            .map(|d| (d.message.clone().into_owned(), d.range.offset))
            .collect();
        let inc_path = root.join("inc.yaml").display().to_string();
        let inner_start =
            u32::try_from(inc.find("another_unknown_code").expect("anchor")).expect("u32");
        let inner_end = inner_start + "another_unknown_code".len() as u32 - 1;
        assert_eq!(
            wrapped,
            vec![(
                format!(
                    "Warning in the included options file {inc_path}({inner_start}..{inner_end}): \
                     'another_unknown_code' isn't a recognized diagnostic code."
                ),
                u32::try_from(options.find("inc.yaml").expect("anchor")).expect("u32"),
            )],
            "the included file's unknown name wraps at the branch's first directive"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The nested-context fixture (#2042 W-F): a second package with its
    /// own package-config entry under the workspace root (the lints
    /// corpus's `nested/` shape) — its own oracle context root, whose
    /// options file validates as an initial file (`_createOptionsMap`,
    /// context_builder.dart:199-224, one full parse per `optionsFileMap`
    /// entry).
    fn nested_workspace(tag: &str) -> PathBuf {
        let root = workspace(tag, &[("lib/ok.dart", "var ok = 1;\n")]);
        std::fs::write(
            root.join(".dart_tool/package_config.json"),
            concat!(
                r#"{"configVersion": 2, "packages": [{"name": "fixture", "#,
                r#""rootUri": "../", "languageVersion": "3.5"}, {"name": "fixture_nested", "#,
                r#""rootUri": "../nested", "languageVersion": "3.5"}], "#,
                r#""generator": "pub", "generatorVersion": "3.13.1"}"#,
            ),
        )
        .expect("package config");
        std::fs::create_dir_all(root.join("nested")).expect("nested dir");
        std::fs::write(
            root.join("nested/pubspec.yaml"),
            "name: fixture_nested\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n",
        )
        .expect("nested pubspec");
        root
    }

    /// The include-site re-report at a second context root (#2042 W-F, the
    /// census `included_file_warning` delta): the nested options file
    /// includes the root's, and the root's cross-lint conflicts re-report
    /// wrapped at the nested file's include directive — the official walker
    /// runs every included file's checks through that file's reporter,
    /// which wraps each row into `included_file_warning` at the initial
    /// include span (`_IncludedDiagnosticListener`,
    /// analysis_options_include_walker.dart:210-248). The wrapped rows
    /// attribute to the nested file; the root file's own row stays
    /// unwrapped on it.
    #[test]
    fn run_overlaid_rereports_included_conflicts_at_the_nested_context_root() {
        let root = nested_workspace("analyze_nested_context_root");
        let root = root.canonicalize().expect("canonical root");
        let root_options = concat!(
            "linter:\n",
            "  rules:\n",
            "    prefer_double_quotes: true\n",
            "    prefer_single_quotes: true\n",
        );
        std::fs::write(root.join("analysis_options.yaml"), root_options).expect("root options");
        let nested_options = "include: ../analysis_options.yaml\n";
        std::fs::write(root.join("nested/analysis_options.yaml"), nested_options)
            .expect("nested options");
        let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");

        let wrapped: Vec<Diagnostic> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "included_file_warning")
            .cloned()
            .collect();
        drop(run);
        let nested_file = engine
            .file_table()
            .try_file_id(&root.join("nested/analysis_options.yaml"))
            .expect("the nested options file is interned");
        let wrapped: Vec<(String, u32, u32)> = wrapped
            .into_iter()
            .map(|d| {
                assert_eq!(
                    d.file, nested_file,
                    "the wrap attributes to the nested file"
                );
                (
                    d.message.clone().into_owned(),
                    d.range.offset,
                    d.range.length,
                )
            })
            .collect();
        let root_path = root.join("analysis_options.yaml").display().to_string();
        let inner_start =
            u32::try_from(root_options.find("prefer_single_quotes").expect("anchor")).expect("u32");
        let inner_end = inner_start + "prefer_single_quotes".len() as u32 - 1;
        assert_eq!(
            wrapped,
            vec![(
                format!(
                    "Warning in the included options file {root_path}({inner_start}..{inner_end}): \
                     The rule 'prefer_single_quotes' is incompatible with \
                     ''prefer_double_quotes''."
                ),
                u32::try_from(
                    nested_options
                        .find("../analysis_options.yaml")
                        .expect("anchor")
                )
                .expect("u32"),
                u32::try_from("../analysis_options.yaml".len()).expect("u32"),
            )],
            "the root's conflict re-reports wrapped at the nested include directive"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The `undefined_lint` verdict at a second context root (#2042 W-F): a
    /// rule the pinned registry dropped (`unnecessary_type_check` — it left
    /// `pkg/linter/lib/src/rules/`; the lint-tier mirror of the analyzer
    /// hint lives on in the semantic tier) is unregistered for the options
    /// faces even though dartq-lints implements it — the official check is
    /// `_getRegisteredLint` (analysis_options_parse_model.dart:1245-1251) —
    /// so the nested file's enabled declaration reports at its name token
    /// with the registry texts, and the included root file's registered
    /// names stay silent.
    #[test]
    fn run_overlaid_reports_undefined_lint_at_the_nested_context_root() {
        let root = nested_workspace("analyze_nested_undefined_lint");
        let root = root.canonicalize().expect("canonical root");
        std::fs::write(
            root.join("analysis_options.yaml"),
            "linter:\n  rules:\n    prefer_single_quotes: true\n",
        )
        .expect("root options");
        let nested_options = concat!(
            "include: ../analysis_options.yaml\n",
            "linter:\n",
            "  rules:\n",
            "    unnecessary_type_check: true\n",
        );
        std::fs::write(root.join("nested/analysis_options.yaml"), nested_options)
            .expect("nested options");
        let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");

        let rows: Vec<(String, &str, u32, u32)> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "undefined_lint")
            .map(|d| {
                (
                    run.layout.file_path(d.file).to_string_lossy().into_owned(),
                    d.message.as_ref(),
                    d.range.offset,
                    d.range.length,
                )
            })
            .collect();
        let nested_path = root
            .join("nested/analysis_options.yaml")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            rows,
            vec![(
                nested_path,
                "'unnecessary_type_check' isn't a recognized lint rule.",
                u32::try_from(
                    nested_options
                        .find("unnecessary_type_check")
                        .expect("anchor")
                )
                .expect("u32"),
                u32::try_from("unnecessary_type_check".len()).expect("u32"),
            )],
            "the registry-absent name reports at the nested file alone: {rows:?}"
        );
        let row = run
            .diagnostics
            .iter()
            .find(|d| d.code.name == "undefined_lint")
            .expect("the row above");
        assert_eq!(row.code.machine, "UNDEFINED_LINT");
        assert_eq!(row.code.dtype, DiagnosticType::StaticWarning);
        std::fs::remove_dir_all(&root).ok();
    }

    /// The Unicode text face through the real pipeline: a bidi code point
    /// in a comment reports on the Dart file it lives in (`lib/test.dart`,
    /// the row's own FileId), a clean sibling file stays silent, and the
    /// row renders the oracle's message, severity, and span — the landed
    /// `options_verifier_pins` row now firing through the engine's
    /// per-file walk.
    #[test]
    fn run_overlaid_reports_text_direction_rows_on_their_own_files() {
        let bidi = '\u{202E}';
        let flipped = format!("// The text direction is silently flipped here {bidi}\nvoid f() {{\n}}\n");
        let root = workspace(
            "analyze_text_direction_rows",
            &[
                ("lib/test.dart", flipped.as_str()),
                ("lib/clean.dart", "void g() {}\n"),
            ],
        );
        let root = root.canonicalize().expect("canonical root");
        let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");

        let rows: Vec<&Diagnostic> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "text_direction_code_point_in_comment")
            .collect();
        let test_file = engine
            .file_table()
            .try_file_id(&root.join("lib/test.dart"))
            .expect("the analyzed file is interned");
        assert_eq!(rows.len(), 1, "only the flipped file's row: {rows:?}");
        let row = rows[0];
        assert_eq!(row.file, test_file, "the row rides the Dart file's own id");
        assert_eq!(row.severity, Severity::Warning);
        assert_eq!(
            row.message.as_str(),
            "The Unicode code point 'U+202E' changes the appearance of text from how \
             it's interpreted by the compiler."
        );
        assert_eq!(
            row.range.offset,
            u32::try_from(flipped.find(bidi).expect("anchor")).expect("u32")
        );
        assert_eq!(row.range.length, 1);
        std::fs::remove_dir_all(&root).ok();
    }

    /// The inner-plugins face through the real pipeline: an inner options
    /// file (a nested directory's own initial file) declaring a top-level
    /// `plugins:` map reports on that file at the section's whole value
    /// node, the correction naming the context root; the root's own map
    /// stays silent at its context root; and a non-map section never
    /// reaches the row.
    #[test]
    fn run_overlaid_reports_plugins_in_inner_options_on_inner_files_only() {
        let root = workspace("analyze_inner_plugins", &[("lib/test.dart", "void f() {}\n")]);
        let root = root.canonicalize().expect("canonical root");
        let inner_options = "plugins:\n  custom_lint:\n    path: .\n";
        std::fs::write(root.join("lib/analysis_options.yaml"), inner_options)
            .expect("inner options");
        std::fs::write(
            root.join("analysis_options.yaml"),
            "plugins:\n  root_plugin:\n    path: .\n",
        )
        .expect("root options");
        let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");

        let rows: Vec<&Diagnostic> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "plugins_in_inner_options")
            .collect();
        let inner_file = engine
            .file_table()
            .try_file_id(&root.join("lib/analysis_options.yaml"))
            .expect("the inner options file is interned");
        assert_eq!(rows.len(), 1, "the root's map stays silent: {rows:?}");
        let row = rows[0];
        assert_eq!(row.file, inner_file);
        assert_eq!(
            row.message.as_str(),
            "Plugins can only be specified in the root of a pub workspace or the root of a \
             package that isn't in a workspace."
        );
        let correction = row.correction.clone().expect("the pinned correction");
        assert_eq!(
            correction.as_ref(),
            format!(
                "Try specifying plugins in an analysis options file at '{}'.",
                root.display()
            )
        );
        assert_eq!(
            row.range.offset,
            u32::try_from(inner_options.find("custom_lint").expect("anchor")).expect("u32")
        );
        assert_eq!(row.range.length, 24);
        std::fs::remove_dir_all(&root).ok();

        // The non-map arm: a scalar `plugins:` section has no map leg, so
        // the inner-options row never fires (the section's own row face is
        // the parse model's, not this one's).
        let root = workspace(
            "analyze_inner_plugins_non_map",
            &[("lib/test.dart", "void f() {}\n")],
        );
        let root = root.canonicalize().expect("canonical root");
        std::fs::write(root.join("lib/analysis_options.yaml"), "plugins: custom_lint\n")
            .expect("inner options");
        let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");
        assert!(
            run.diagnostics
                .iter()
                .all(|d| d.code.name != "plugins_in_inner_options"),
            "a non-map section never reaches the inner-options row"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The context face end to end: every raw INCOMPATIBLE_LINT row the
    /// initial file reports carries the conflicting rule's anchor as its
    /// `contextMessages`. The enabled pass's row anchors at the conflicting
    /// declaration's name token in the reporting file itself
    /// (diagnostic_factory.dart:302-313's same-file map fold — the last
    /// entry wins); the files variant anchors at the rule's span in its
    /// included options file, resolved through the file table (a governing
    /// nested options file is interned by the engine pass); a conflicting
    /// rule owned by an include target no directory governs has no `FileId`
    /// to resolve to — its context row drops and the diagnostic degrades to
    /// the plain no-context face (under-report, never a crash).
    #[test]
    fn run_overlaid_reports_incompatible_lint_context_rows() {
        let root = workspace(
            "analyze_incompatible_context",
            &[("lib/ok.dart", "var ok = 1;\n")],
        );
        let root = root.canonicalize().expect("canonical root");

        // The enabled pass's row: one context row at the conflicting
        // declaration's name token, filed under the reporting options file.
        let options = "linter:\n  rules:\n    - prefer_double_quotes\n    - prefer_single_quotes\n";
        std::fs::write(root.join("analysis_options.yaml"), options).expect("options");
        let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");
        let row = run
            .diagnostics
            .iter()
            .find(|d| d.code.name == "incompatible_lint")
            .cloned()
            .expect("the pinned row");
        drop(run);
        let options_file = engine
            .file_table()
            .try_file_id(&root.join("analysis_options.yaml"))
            .expect("the options file is interned");
        assert_eq!(
            row.message.as_str(),
            "The rule 'prefer_single_quotes' is incompatible with ''prefer_double_quotes''."
        );
        let context = row.context.as_deref().expect("context messages");
        assert_eq!(context.len(), 1);
        assert_eq!(
            context[0].message.as_str(),
            "The rule 'prefer_double_quotes' is enabled here."
        );
        assert_eq!(context[0].file, options_file);
        assert_eq!(
            context[0].range,
            SourceRange {
                offset: u32::try_from(options.find("prefer_double_quotes").expect("anchor"))
                    .expect("u32"),
                length: "prefer_double_quotes".len() as u32,
            },
        );

        // The files face: the conflicting rule's owning included file is a
        // governing nested options file (under a discovery surface), so the
        // context row resolves to its `FileId` and names the file.
        std::fs::create_dir(root.join("tool")).expect("tool dir");
        std::fs::write(
            root.join("tool/analysis_options.yaml"),
            "linter:\n  rules:\n    - prefer_double_quotes\n",
        )
        .expect("nested options");
        let options = concat!(
            "include: tool/analysis_options.yaml\n",
            "linter:\n",
            "  rules:\n",
            "    - prefer_single_quotes\n",
        );
        std::fs::write(root.join("analysis_options.yaml"), options).expect("options");
        let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");
        let row = run
            .diagnostics
            .iter()
            .find(|d| d.code.name == "incompatible_lint")
            .cloned()
            .expect("the pinned row");
        drop(run);
        let nested_file = engine
            .file_table()
            .try_file_id(&root.join("tool/analysis_options.yaml"))
            .expect("the nested options file is interned");
        assert_eq!(
            row.message.as_str(),
            "The rule 'prefer_single_quotes' is incompatible with 'prefer_double_quotes'."
        );
        let context = row.context.as_deref().expect("context messages");
        let nested_text =
            std::fs::read_to_string(root.join("tool/analysis_options.yaml")).expect("text");
        assert_eq!(context.len(), 1);
        assert_eq!(
            context[0].message.as_str(),
            format!(
                "The rule 'prefer_double_quotes' is enabled here in the file '{}'.",
                root.join("tool/analysis_options.yaml").display()
            )
        );
        assert_eq!(context[0].file, nested_file);
        assert_eq!(
            context[0].range,
            SourceRange {
                offset: u32::try_from(nested_text.find("prefer_double_quotes").expect("anchor"))
                    .expect("u32"),
                length: "prefer_double_quotes".len() as u32,
            },
        );

        // The dropped-file case: the conflicting rules live in include
        // targets no directory governs — no `FileId` resolves, the context
        // rows drop, and the row carries no context at all.
        std::fs::write(
            root.join("a.yaml"),
            "linter:\n  rules:\n    - prefer_double_quotes\n",
        )
        .expect("a.yaml");
        std::fs::write(
            root.join("b.yaml"),
            "linter:\n  rules:\n    - prefer_single_quotes\n",
        )
        .expect("b.yaml");
        let options = concat!(
            "include:\n",
            "  - a.yaml\n",
            "  - b.yaml\n",
            "linter:\n",
            "  rules:\n",
            "    - avoid_print\n",
        );
        std::fs::write(root.join("analysis_options.yaml"), options).expect("options");
        let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");
        let row = run
            .diagnostics
            .iter()
            .find(|d| d.code.name == "incompatible_lint")
            .expect("the pinned row");
        assert_eq!(
            row.message.as_str(),
            "The rule 'b.yaml' is incompatible with 'prefer_single_quotes' and \
             'prefer_double_quotes', which is included from 2 files."
        );
        assert!(row.context.is_none(), "unresolvable files drop their rows");
        std::fs::remove_dir_all(&root).ok();
    }

    /// The `removed_lint_use` face end to end (#1005): with the governing
    /// options enabling `unnecessary_ignore`, an ignore comment naming a
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
                        d.correction.as_deref().expect("correction").to_owned(),
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
    /// `ignore_validator.dart:89`, :164-174 — the `unignorable_ignore` rows
    /// themselves are the lints stage's face), an exact duplicate reports
    /// once, first occurrence winning (:91), and a non-removed rule's name
    /// resolves through the registry without a row (:214-240 — only
    /// `state.isRemoved` reports).
    #[test]
    fn removed_lint_use_filters_unignorable_and_duplicate_names() {
        let source = "var a = 1; // ignore: super_goes_last, super_goes_last, avoid_empty_else\n";
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
        assert!(
            offsets_for(concat!(
                "analyzer:\n",
                "  cannot-ignore:\n",
                "    - super_goes_last\n",
                "linter:\n",
                "  rules:\n",
                "    - unnecessary_ignore\n",
            ))
            .is_empty()
        );

        // Without it the duplicate collapses to its first occurrence and the
        // non-removed neighbor stays silent.
        assert_eq!(
            offsets_for("linter:\n  rules:\n    - unnecessary_ignore\n"),
            vec![22],
            "first occurrence wins, only removed names report"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The degrade notice face (#1311): the fixture's clean whitelist
    /// carries zero notices, and a whitelist naming an experiment the
    /// vendored feature registry does not know yields exactly one rendered
    /// notice on the run — the engine emits `ExperimentDegrade` once per
    /// unknown name (#1304), the core renders one line per event. The skew
    /// fixture mirrors dartq-engine's in-module `Whitelist::Skewed` shape
    /// (cfg(test)-private there), with the known `enhanced-parts` grant
    /// alongside to pin that known names never join the notice.
    #[test]
    fn run_reports_the_degrade_notice_once_on_skew_and_never_clean() {
        let root = workspace(
            "analyze_degrade_notice",
            &[("lib/ok.dart", "var ok = 1;\n")],
        );
        let root = root.canonicalize().expect("canonical root");
        let notices_for = |whitelist: &str| -> Vec<String> {
            std::fs::write(
                root.join("sdk/lib/_internal/allowed_experiments.json"),
                whitelist,
            )
            .expect("whitelist");
            let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
            let mut engine = Engine::new(config).expect("engine");
            let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");
            run.degrade_notices
        };
        assert!(
            notices_for(
                r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
            )
.is_empty(),
            "clean whitelist: no notices"
        );
        assert_eq!(
            notices_for(
                r#"{"version": 1, "experimentSets": {"sdkExperiments": ["enhanced-parts", "future-flag"]}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {"fixture": {"experimentSet": "sdkExperiments"}}}"#,
            ),
            vec![
                "note: SDK 3.13.1 names unknown experiment(s) future-flag; granting known-only"
                    .to_owned()
            ],
            "one notice per unknown name; the known grant stays silent"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The file-target face end to end: `run_targets` emits only the named
    /// file's rows — the sibling file's identical diagnostics stay on the
    /// engine's analysis but off the emitted set (the oracle's file context
    /// root pushes the file alone, analyze.dart:188-189). A directory
    /// target emits everything under it; a file outside the discovered
    /// surfaces (here `tool/`-less `extra/x.dart`) still analyzes — the
    /// injection enqueues it through the overlay contract.
    #[test]
    fn run_targets_filters_the_emitted_set_to_covered_files() {
        let root = workspace(
            "analyze_targets",
            &[
                ("lib/broken_a.dart", "void f() { x }\n"),
                ("lib/broken_b.dart", "void g() { y }\n"),
                ("extra/broken_c.dart", "void h() { z }\n"),
            ],
        );
        let root = root.canonicalize().expect("canonical root");
        let broken_a = root.join("lib/broken_a.dart");
        let lib_dir = root.join("lib");

        let diagnostics_for = |targets: Vec<Target>| -> Vec<String> {
            let (config, _) = engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
            let mut engine = Engine::new(config).expect("engine");
            let run = run_targets(
                &mut engine,
                &[],
                std::slice::from_ref(&root),
                &targets,
                None,
            )
            .expect("run");
            run.diagnostics
                .iter()
                .map(|d| {
                    format!(
                        "{}|{}",
                        run.layout.file_path(d.file).to_string_lossy(),
                        d.code.name
                    )
                })
                .collect()
        };

        // A file target emits its rows alone — the sibling's identical
        // rows and the undiscovered `extra/` file stay off the set.
        let file_rows = diagnostics_for(vec![Target::File(broken_a.clone())]);
        assert!(
            !file_rows.is_empty()
                && file_rows
                    .iter()
                    .all(|row| row.starts_with(&format!("{}", broken_a.display()))),
            "the file target emits only its own rows: {file_rows:?}"
        );

        // A directory target covers its subtree — `lib/` emits both broken
        // files and not `extra/`.
        let dir_rows = diagnostics_for(vec![Target::Dir(lib_dir.clone())]);
        assert_eq!(
            dir_rows
                .iter()
                .map(|row| row.split('|').next().expect("path").to_owned())
                .collect::<std::collections::BTreeSet<_>>(),
            [broken_a.clone(), root.join("lib/broken_b.dart")]
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            "the dir target covers its subtree only: {dir_rows:?}"
        );

        // An undiscovered-surface file target still analyzes — the oracle's
        // file context root behavior (context_locator): `extra/` is no
        // declared surface, yet its rows emit.
        let extra_rows = diagnostics_for(vec![Target::File(root.join("extra/broken_c.dart"))]);
        assert!(
            !extra_rows.is_empty() && extra_rows.iter().all(|row| row.contains("broken_c.dart")),
            "the never-discovered file target still analyzes: {extra_rows:?}"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The multi-workspace union: `merge_runs` merges two engines' runs
    /// into one oracle-ordered set (severity, path bytes, offset, message —
    /// `oracle_cmp`'s keys) remapped onto one file table, sums the run
    /// facts, and the merged layout resolves every emitted row's position.
    #[test]
    fn merge_runs_unions_workspaces_in_oracle_order() {
        let root_a = workspace("merge_a", &[("lib/bad.dart", "void f() { x }\n")]);
        let root_b = workspace("merge_b", &[("lib/warn.dart", "var w = unused;\n")]);
        let root_a = root_a.canonicalize().expect("canonical a");
        let root_b = root_b.canonicalize().expect("canonical b");

        // The engines outlive the runs (the merged layout delegates to the
        // runs' own layouts, each borrowing its engine).
        let mut engines: Vec<Engine> = [&root_a, &root_b]
            .into_iter()
            .map(|root| {
                let (config, _) = engine_config(root, Some(&root.join("sdk"))).expect("assembly");
                Engine::new(config).expect("engine")
            })
            .collect();
        let runs: Vec<AnalyzeRun> = engines
            .iter_mut()
            .zip([&root_a, &root_b])
            .map(|(engine, root)| {
                run_targets(
                    engine,
                    &[],
                    std::slice::from_ref(root),
                    &[Target::Dir(root.to_path_buf())],
                    None,
                )
                .expect("run")
            })
            .collect();
        let merged = merge_runs(runs);
        // The union carries both workspaces' rows; every emitted path is
        // one of the two canonical roots' files, in the oracle display
        // order.
        assert!(!merged.diagnostics.is_empty(), "the union is non-empty");
        let mut sorted = true;
        for pair in merged.diagnostics.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            let ordered = severity_rank(a.severity)
                .cmp(&severity_rank(b.severity))
                .then_with(|| {
                    path_key(merged.file_path(a.file)).cmp(path_key(merged.file_path(b.file)))
                })
                .then_with(|| a.range.offset.cmp(&b.range.offset))
                .then_with(|| a.message.cmp(&b.message));
            sorted &= ordered != Ordering::Greater;
        }
        assert!(sorted, "the merged set stays oracle-ordered");
        // The merged layout resolves positions for every row.
        for d in &merged.diagnostics {
            let (line, col) = merged.line_col(d.file, d.range.offset);
            assert!(line >= 1 && col >= 1, "merged layout resolves");
        }
        // Run facts sum across engines.
        assert_eq!(merged.file_count, 2, "one file per workspace");
        assert_eq!(merged.libraries_analyzed, 2, "both engines recomputed");

        std::fs::remove_dir_all(&root_a).ok();
        std::fs::remove_dir_all(&root_b).ok();
    }
}
