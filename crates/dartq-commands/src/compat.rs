//! The compat face's core (PRD §15.5): the same workspace analyzed under
//! several SDK roots, aggregated into a per-axis ✓/✗ matrix.
//!
//! One matrix **axis** is one SDK context (an [`SdkSnapshot`]; Dart+Flutter
//! are fixed pairs, so one axis carries one label). The axes run
//! **sequentially, one engine alive at a time** (bounded-memory doctrine:
//! N concurrent semantic graphs would be the resource anti-pattern), over
//! the same package discovery — pub faces and package graph are built once,
//! the engine reassembled per axis from its own snapshot. The overlay stays
//! empty; every axis sees the same on-disk base code.
//!
//! Per axis the run is the other engine faces' assembly + one analyze pass,
//! filtered to ERROR-severity diagnostics (warnings/lints never flip an
//! axis). An SDK root that fails to load is **isolated**, not fatal: the
//! axis lands in the matrix as incompatible with the typed
//! `sdk_unavailable` error row (`dartq-ai::compatibility`), and the
//! remaining axes still answer. (Daemon note: the session's root context
//! stays resident alongside the per-axis engines — they drop at function
//! end, keeping at most the root plus one axis graph alive.)
//!
//! The axis verdict rule and the `first_failure` ordering live in
//! [`dartq_ai::compatibility`]. SDK-root *resolution* (the CLI's
//! `--sdk-root` args with the `$DART_SDK` fallback) stays at the process
//! edges: this core takes explicit roots.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dartq_ai::compatibility as query;
use dartq_ai::compatibility::{AxisAnalysis, AxisInput, Failure};
use dartq_diagnostics::{FileLayoutProvider as _, Severity};
use dartq_engine::{Engine, EngineConfig, memory_store, semdiag};
use dartq_package::PackageTable;
use dartq_pub::{PackageConfig, ResolvedPackageGraph, build_graph};
use dartq_sdk::SdkSnapshot;
use thiserror::Error;

use crate::layout::EngineLayout;
use crate::workspace::load_workspace_faces;

/// Why a compat run aborts without a matrix: workspace problems are the
/// CLI's usage tier, engine failures the internal tier (each carries its
/// printed message body).
#[derive(Debug, Error)]
pub enum CompatError {
    /// The CLI's usage tier (`error: {0}`).
    #[error("{0}")]
    Usage(String),
    /// The CLI's internal tier (`dartq: internal error: {0}`).
    #[error("{0}")]
    Internal(String),
}

/// Runs the compat face: `dir`'s workspace analyzed once per SDK root, the
/// queried `target` file's verdict matrix in axis order.
///
/// # Errors
///
/// [`CompatError::Usage`] on a missing directory/target, a target outside
/// the workspace, or unusable pub faces; [`CompatError::Internal`] on
/// discovery or analysis failure inside an axis.
pub fn compat(
    dir: &Path,
    target: &Path,
    sdk_roots: &[PathBuf],
) -> Result<query::CompatReport, CompatError> {
    // Canonicalized up front so the printed paths are symlink-resolved (the
    // oracle convention) and the target containment check compares like
    // with like.
    let canonical_dir = dir.canonicalize().map_err(|_| {
        CompatError::Usage(format!("Directory or file doesn't exist: {}", dir.display()))
    })?;
    let canonical_target = target.canonicalize().map_err(|_| {
        CompatError::Usage(format!("Directory or file doesn't exist: {}", target.display()))
    })?;
    if canonical_target.extension() != Some(std::ffi::OsStr::new("dart"))
        || !canonical_target.starts_with(&canonical_dir)
    {
        return Err(CompatError::Usage(format!(
            "not an analyzed .dart file: {}",
            canonical_target.display()
        )));
    }

    // The pub faces and the package graph are axis-independent: built once,
    // reused by every axis's engine assembly.
    let config_dir = canonical_dir.join(".dart_tool");
    let (config, lock, pubspec) =
        load_workspace_faces(&canonical_dir, &config_dir).map_err(CompatError::Usage)?;
    let graph = build_graph(
        &config,
        &lock,
        &pubspec,
        &BTreeMap::new(),
        None,
        &config_dir,
    )
    .map_err(|error| CompatError::Usage(error.to_string()))?;

    let mut matrix: Vec<AxisInput> = Vec::with_capacity(sdk_roots.len());
    for root in sdk_roots {
        matrix.push(analyze_axis(root, &config, &graph, &config_dir)?);
    }

    Ok(query::compatibility(
        &canonical_target.display().to_string(),
        &matrix,
    ))
}

/// One matrix axis: assemble the engine under `root`'s snapshot over the
/// shared package resolution, run the other faces' discover + analyze
/// passes, and collect the axis's ERROR-severity diagnostics. An SDK root
/// that fails to load is an `sdk_unavailable` axis, never an abort (axis
/// failure isolation). The engine drops at function end — one semantic
/// graph alive at a time (bounded-memory doctrine).
fn analyze_axis(
    root: &Path,
    config: &Arc<PackageConfig>,
    graph: &ResolvedPackageGraph,
    config_dir: &Path,
) -> Result<AxisInput, CompatError> {
    // A failed load has no version to label itself with: the axis keeps the
    // root it was given.
    let snapshot = match SdkSnapshot::load(root, None) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return Ok(AxisInput {
                sdk: root.display().to_string(),
                analysis: AxisAnalysis::SdkUnavailable(error.to_string()),
            });
        }
    };
    let sdk_label = snapshot.dart.version.clone();
    // The identity table is engine-owned (not `Clone`); identical inputs
    // rebuild it per axis.
    let packages = PackageTable::build(config, graph, config_dir)
        .map_err(|error| CompatError::Usage(error.to_string()))?;
    let mut engine = Engine::new(EngineConfig {
        store: memory_store(),
        sdk: snapshot,
        packages,
        pub_config: Arc::clone(config),
        config_dir: config_dir.to_path_buf(),
        // The semantic families are part of the analyze contract (#236):
        // the SDK-difference signal (`undefined_*` and friends) rides the
        // default diagnoser.
        semdiag: Some(semdiag::diagnoser()),
    })
    .map_err(|error| CompatError::Usage(format!("engine context unusable: {error}")))?;

    if let Err(error) = engine.discover_workspace() {
        return Err(CompatError::Internal(format!("discovery failed: {error}")));
    }
    // The read-after-write: the matrix reflects the analyzed state only.
    let mut on_event = |_: dartq_engine::EngineEvent| {};
    let diags = engine
        .analyze(&[], &mut on_event, &dartq_engine::CancellationToken::default())
        .map_err(|_: dartq_engine::Cancelled| CompatError::Internal("analysis cancelled".into()))?;

    // The verdict input: ERROR-severity diagnostics only, laid out through
    // the engine's read faces.
    let error_diags: Vec<_> = diags
        .iter()
        .filter(|diag| diag.severity == Severity::Error)
        .collect();
    let layout = EngineLayout::over(&engine, error_diags.iter().map(|diag| diag.file));
    let errors = error_diags
        .iter()
        .map(|diag| {
            let (line, col) = layout.line_col(diag.file, diag.range.offset);
            Failure {
                code: diag.code.name.to_owned(),
                file: layout.file_path(diag.file).display().to_string(),
                line,
                col,
                message: diag.message.clone(),
            }
        })
        .collect();
    Ok(AxisInput {
        sdk: sdk_label,
        analysis: AxisAnalysis::Analyzed(errors),
    })
}
