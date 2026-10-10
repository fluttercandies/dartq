//! The analyze face's core: the full engine pass over an assembled
//! context — workspace discovery, one barrier+recompute, and the oracle
//! display order (DiagnosticWithPath.compareTo, dartdev analyze.dart:686–
//! 707): severity, file path, position, message. Position order is the
//! UTF-16 offset (line/column are monotone in it), so no line-column
//! computation is spent on sorting.

use std::cmp::Ordering;
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
    diagnostics.sort_by(|a, b| oracle_cmp(a, b, &layout));
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
