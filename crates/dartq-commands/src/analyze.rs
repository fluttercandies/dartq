//! The analyze face's core: the full engine pass over an assembled
//! context — workspace discovery, one barrier+recompute, and the oracle
//! display order (DiagnosticWithPath.compareTo, dartdev analyze.dart:686–
//! 707): severity, file path, position, message. Position order is the
//! UTF-16 offset (line/column are monotone in it), so no line-column
//! computation is spent on sorting.

use std::io;
use std::time::Duration;

use dartq_diagnostics::{Diagnostic, FileLayoutProvider as _};
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

/// The discover+analyze prelude's full yield: the merged diagnostic set of
/// every live library (canonically sorted by the engine) plus the run facts
/// the metrics faces report.
pub(crate) struct Prelude {
    pub diagnostics: Vec<Diagnostic>,
    pub file_count: usize,
    pub loc: u64,
    pub discovery: Duration,
    pub analysis: Duration,
    pub libraries_analyzed: usize,
    pub libraries_from_cache: usize,
}

/// Runs the prelude: [`Engine::discover_workspace`] then one
/// [`Engine::analyze`] pass (the read-after-write every query face reflects).
pub(crate) fn prelude(engine: &mut Engine) -> Result<Prelude, AnalyzeError> {
    let started = std::time::Instant::now();
    let stats = engine
        .discover_workspace()
        .map_err(AnalyzeError::Discovery)?;
    let discovery = started.elapsed();

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
    let analysis = analyze_at.elapsed();

    Ok(Prelude {
        diagnostics,
        file_count: stats.file_count,
        loc: stats.loc_total,
        discovery,
        analysis,
        libraries_analyzed: analyzed.0,
        libraries_from_cache: analyzed.1,
    })
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

/// Runs the analyze face over an assembled engine.
///
/// # Errors
///
/// [`AnalyzeError`] on discovery failure or cancellation.
pub fn run(engine: &mut Engine) -> Result<AnalyzeRun<'_>, AnalyzeError> {
    let prelude = prelude(engine)?;
    let layout = EngineLayout::over(engine, prelude.diagnostics.iter().map(|d| d.file));
    let mut diagnostics = prelude.diagnostics;
    diagnostics.sort_by(|a, b| {
        severity_rank(a.severity)
            .cmp(&severity_rank(b.severity))
            .then_with(|| {
                path_key(layout.file_path(a.file)).cmp(path_key(layout.file_path(b.file)))
            })
            .then_with(|| a.range.offset.cmp(&b.range.offset))
            .then_with(|| a.message.cmp(&b.message))
    });
    Ok(AnalyzeRun {
        diagnostics,
        discovery: prelude.discovery,
        analysis: prelude.analysis,
        file_count: prelude.file_count,
        loc: prelude.loc,
        libraries_analyzed: prelude.libraries_analyzed,
        libraries_from_cache: prelude.libraries_from_cache,
        layout,
    })
}
