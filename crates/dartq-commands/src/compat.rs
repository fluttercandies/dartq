//! The compat face's core: the same workspace analyzed under
//! several SDK roots, aggregated into a per-axis ✓/✗ matrix.
//!
//! One matrix **axis** is one SDK context (an [`SdkSnapshot`]; Dart+Flutter
//! are fixed pairs, so one axis carries one label). The axes run
//! **sequentially, one engine alive at a time** (bounded-memory doctrine:
//! N concurrent semantic graphs would be the resource anti-pattern), over
//! the same package discovery — pub faces and package graph are built once,
//! the engine reassembled per axis from its own snapshot. The resolution is
//! the other faces' canonical assembly under the compat face's
//! config-required posture ([`crate::workspace::package_assembly_strict`]):
//! a pub-workspace member assembles the root's artifacts over the full
//! member set, and the assembly's one stale-config advisory rides the
//! notices stream (it is not an engine row, so it cannot enter the matrix);
//! a package with no resolvable config anywhere up the root chain refuses
//! (usage tier) where the analyze path would degrade. The overlay stays
//! empty; every axis sees the same on-disk base code.
//!
//! Per axis the run is the other engine faces' assembly + one analyze pass,
//! filtered to ERROR-severity diagnostics (warnings/lints never flip an
//! axis). The pass is the engine pass only — no lint stage: an
//! `analysis_options.yaml` severity escalation (a lint raised to `error`,
//! which can fail `dartq analyze`) never flips an axis, because escalation
//! is user configuration rather than SDK structure and is axis-independent
//! — it would flip every axis identically, adding no discriminating
//! signal. An SDK root that fails to load is **isolated**, not fatal: the
//! axis lands in the matrix as incompatible with the typed
//! `sdk_unavailable` error row (`dartq-ai::compatibility`), and the
//! remaining axes still answer. (Daemon note: the session's root context
//! stays resident alongside the per-axis engines — they drop at function
//! end, keeping at most the root plus one axis graph alive.)
//!
//! The axis verdict rule and the `first_failure` ordering live in
//! [`dartq_ai::compatibility`]. SDK-root *resolution* (the CLI's
//! `--sdk-root` args, the `$DART_SDK` fallback, and the `dartq.yaml`
//! context locators via [`crate::config::compat_axis_roots`]) stays at the
//! process edges: this core takes explicit roots.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dartq_ai::compatibility as query;
use dartq_ai::compatibility::{AxisAnalysis, AxisInput, Failure};
use dartq_diagnostics::{FileLayoutProvider as _, Severity};
use dartq_engine::{Engine, EngineConfig, memory_store, semdiag};
use dartq_package::PackageTable;
use dartq_pub::{PackageConfig, ResolvedPackageGraph};
use dartq_sdk::SdkSnapshot;
use thiserror::Error;

use crate::Verbosity;
use crate::layout::EngineLayout;
use crate::workspace::package_assembly_strict;

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
/// queried `target` file's verdict matrix in axis order, plus the faces'
/// human-stream notices — the resolution's one stale-config advisory
/// (rendered `note:`, never a matrix input) ahead of the axes'
/// version-skew degrade notices (#1311), one rendered
/// [`crate::analyze::degrade_notice`] per
/// [`dartq_engine::EngineEvent::ExperimentDegrade`] event, axis order
/// concatenated (bounded per axis: one event per unknown name per axis
/// engine). The notices are the faces' human-stream telemetry; the report
/// is the wire contract, so they travel beside it.
///
/// # Errors
///
/// [`CompatError::Usage`] on a missing directory/target, a target outside
/// the workspace, a package with no resolvable package config anywhere up
/// the root chain, or unusable pub faces; [`CompatError::Internal`] on
/// discovery or analysis failure inside an axis.
pub fn compat(
    dir: &Path,
    target: &Path,
    sdk_roots: &[PathBuf],
) -> Result<(query::CompatReport, Vec<String>), CompatError> {
    // Canonicalized up front so the printed paths are symlink-resolved (the
    // oracle convention) and the target containment check compares like
    // with like.
    let canonical_dir = dir.canonicalize().map_err(|_| {
        CompatError::Usage(format!(
            "Directory or file doesn't exist: {}",
            dir.display()
        ))
    })?;
    let canonical_target = target.canonicalize().map_err(|_| {
        CompatError::Usage(format!(
            "Directory or file doesn't exist: {}",
            target.display()
        ))
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
    // reused by every axis's engine assembly. The resolution is the other
    // faces' canonical assembly under the strict posture: root resolution
    // (a member analyzes the root's artifacts over the full member set),
    // nested self-resolved roots, and the config-required refusals.
    let (config, graph, config_dir, advisory) = package_assembly_strict(&canonical_dir)
        .map_err(|error| CompatError::Usage(error.to_string()))?;
    // The assembly's one advisory rides the human stream ahead of the axis
    // notices; it is not an engine row, so it cannot enter the matrix.
    let mut notices: Vec<String> = Vec::new();
    if let Some(advisory) = advisory {
        notices.push(format!("note: {}", advisory.message()));
    }
    let mut matrix: Vec<AxisInput> = Vec::with_capacity(sdk_roots.len());
    for root in sdk_roots {
        let (axis, axis_notices) = analyze_axis(root, &config, &graph, &config_dir)?;
        matrix.push(axis);
        notices.extend(axis_notices);
    }

    Ok((
        query::compatibility(&canonical_target.display().to_string(), &matrix),
        notices,
    ))
}

/// Shapes the report at the requested verbosity tier — the canonical compat
/// table's one cut: `minimal` keeps the verdict + axis identity (`sdk`/
/// `compatible`) and drops the evidence rows (`first_failure`/`error` — the
/// detail is one compact run away); `compact` is the produced shape, and
/// `full` lists no compat-only extras, so it equals `compact`. The daemon's
/// `dartq/compat` answer applies the same cut to its serialized value.
#[must_use]
pub fn at_verbosity(mut report: query::CompatReport, verbosity: Verbosity) -> query::CompatReport {
    if verbosity == Verbosity::Minimal {
        for axis in &mut report.matrix {
            axis.first_failure = None;
            axis.error = None;
        }
    }
    report
}

/// The JSON face: one object — the (already tier-cut) report's serialized
/// shape. The payload is routed through `dartq_schema::validate(..,
/// Domain::Compat)` right here — the producer-side gate the index face
/// holds, shared verbatim by the CLI and the daemon (both call this
/// shaper). The tier cut rides [`at_verbosity`] before this shaper:
/// `minimal` drops the evidence keys, which are optional columns in the
/// published schema, so the cut shape stays schema-valid and validates
/// here like any other birth.
///
/// # Panics
///
/// Panics when the born payload violates the published schema — a
/// producer bug, surfaced never clamped (the CLI's crash tier turns it
/// into exit 4 with stdout kept empty; the daemon's read task contains
/// it at its panic boundary).
#[must_use]
pub fn to_json(report: &query::CompatReport) -> serde_json::Value {
    let value = serde_json::to_value(report).expect("CompatReport serializes");
    let text = value.to_string();
    dartq_schema::validate(&text, dartq_schema::Domain::Compat)
        .expect("the compat report is schema-valid at birth");
    value
}

/// One matrix axis: assemble the engine under `root`'s snapshot over the
/// shared package resolution, run the other faces' discover + analyze
/// passes, and collect the axis's ERROR-severity diagnostics plus the
/// axis's version-skew degrade notices (#1311). An SDK root that fails to
/// load is an `sdk_unavailable` axis, never an abort (axis failure
/// isolation). The engine drops at function end — one semantic graph alive
/// at a time (bounded-memory doctrine).
fn analyze_axis(
    root: &Path,
    config: &Arc<PackageConfig>,
    graph: &ResolvedPackageGraph,
    config_dir: &Path,
) -> Result<(AxisInput, Vec<String>), CompatError> {
    // A failed load has no version to label itself with: the axis keeps the
    // root it was given.
    let snapshot = match SdkSnapshot::load(root, None) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return Ok((
                AxisInput {
                    sdk: root.display().to_string(),
                    analysis: AxisAnalysis::SdkUnavailable(error.to_string()),
                },
                Vec::new(),
            ));
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
        target: dartq_engine::DEFAULT_TARGET.to_owned(),
        declared_variables: rustc_hash::FxHashMap::default(),
        experiments: Vec::new(),
    })
    .map_err(|error| CompatError::Usage(format!("engine context unusable: {error}")))?;

    if let Err(error) = engine.discover_workspace() {
        return Err(CompatError::Internal(format!("discovery failed: {error}")));
    }
    // The read-after-write: the matrix reflects the analyzed state only.
    // The axis's version-skew degrade notices ride the same event stream
    // (#1311) — one rendered line per `ExperimentDegrade` event.
    let mut notices: Vec<String> = Vec::new();
    let mut on_event = |event: dartq_engine::EngineEvent| {
        if let dartq_engine::EngineEvent::ExperimentDegrade {
            sdk_version,
            unknown,
            ..
        } = event
        {
            notices.push(crate::analyze::degrade_notice(&sdk_version, &unknown));
        }
    };
    let diags = engine
        .analyze(
            &[],
            &mut on_event,
            &dartq_engine::CancellationToken::default(),
        )
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
                // The same package-form label projection the other AI faces
                // emit, not the raw path.
                file: crate::pkg_label::pkg_label(engine.packages(), layout.file_path(diag.file)),
                line,
                col,
                message: diag.message.clone().into_owned(),
            }
        })
        .collect();
    Ok((
        AxisInput {
            sdk: sdk_label,
            analysis: AxisAnalysis::Analyzed(errors),
        },
        notices,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The verbosity cut ([`at_verbosity`]) on a hand-built report: minimal
    /// drops the evidence rows (`first_failure`/`error`) and keeps the
    /// verdict + axis identity; compact and full are the produced shape.
    #[test]
    fn at_verbosity_drops_evidence_rows_at_minimal() {
        let report = query::CompatReport {
            schema_version: query::COMPAT_SCHEMA_VERSION,
            file: "/w/lib/a.dart".to_owned(),
            matrix: vec![
                query::AxisCompat {
                    sdk: "3.5.0".to_owned(),
                    compatible: false,
                    first_failure: Some(query::Failure {
                        code: "uri_does_not_exist".to_owned(),
                        file: "/w/lib/a.dart".to_owned(),
                        line: 1,
                        col: 8,
                        message: "Target of URI doesn't exist".to_owned(),
                    }),
                    error: None,
                },
                query::AxisCompat {
                    sdk: "/w/sdks/broken".to_owned(),
                    compatible: false,
                    first_failure: None,
                    error: Some(query::AxisError {
                        kind: query::AxisError::SDK_UNAVAILABLE.to_owned(),
                        message: "no SDK version at /w/sdks/broken".to_owned(),
                    }),
                },
            ],
        };

        let minimal = at_verbosity(report.clone(), Verbosity::Minimal);
        assert_eq!(minimal.file, report.file, "the subject label stays");
        assert_eq!(minimal.schema_version, report.schema_version);
        for (axis, produced) in minimal.matrix.iter().zip(&report.matrix) {
            assert_eq!(axis.sdk, produced.sdk, "the axis identity stays");
            assert_eq!(axis.compatible, produced.compatible, "the verdict stays");
            assert!(axis.first_failure.is_none(), "evidence drops");
            assert!(axis.error.is_none(), "evidence drops");
        }

        assert_eq!(
            at_verbosity(report.clone(), Verbosity::Compact),
            report,
            "compact is the produced shape"
        );
        assert_eq!(
            at_verbosity(report.clone(), Verbosity::Full),
            at_verbosity(report, Verbosity::Compact),
            "full equals compact (no compat-only extras)"
        );
    }

    /// The canonical fixture matrix (the shaper tests' input): the three
    /// axis row shapes in one document — clean, analyzed-with-failure,
    /// isolated load failure.
    fn golden_report() -> query::CompatReport {
        query::CompatReport {
            schema_version: query::COMPAT_SCHEMA_VERSION,
            file: "/w/lib/a.dart".to_owned(),
            matrix: vec![
                query::AxisCompat {
                    sdk: "3.13.1".to_owned(),
                    compatible: true,
                    first_failure: None,
                    error: None,
                },
                query::AxisCompat {
                    sdk: "3.5.0".to_owned(),
                    compatible: false,
                    first_failure: Some(query::Failure {
                        code: "uri_does_not_exist".to_owned(),
                        file: "/w/lib/a.dart".to_owned(),
                        line: 1,
                        col: 8,
                        message: "Target of URI doesn't exist: 'dart:async'".to_owned(),
                    }),
                    error: None,
                },
                query::AxisCompat {
                    sdk: "/w/sdks/broken".to_owned(),
                    compatible: false,
                    first_failure: None,
                    error: Some(query::AxisError {
                        kind: query::AxisError::SDK_UNAVAILABLE.to_owned(),
                        message: "no SDK version at /w/sdks/broken".to_owned(),
                    }),
                },
            ],
        }
    }

    /// The AI-face golden (`goldens/ai/compat_matrix.json`): the promoted
    /// matrix envelope, pinned against the live shaper. The comparison
    /// rides the schema validate-at-birth gate inside [`to_json`]; a
    /// mismatch is a regression, not a golden to edit.
    #[test]
    fn the_ai_golden_pins_the_report_shaper() {
        let golden = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../goldens/ai/compat_matrix.json");
        let expected: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&golden).expect("the compat golden rides the repo"),
        )
        .expect("one JSON object");
        assert_eq!(to_json(&golden_report()), expected, "compat golden drift");
    }

    /// The tier pair boundary: the minimal cut's payload is tier-exact —
    /// the evidence keys dropped, the verdict rows schema-valid at the
    /// minimal tier — while the born shape carries the evidence columns,
    /// which the minimal tier forbids (compact and full accept them).
    #[test]
    fn the_tier_cut_stays_schema_valid() {
        let minimal = to_json(&at_verbosity(golden_report(), Verbosity::Minimal));
        let text = minimal.to_string();
        for tier in [
            dartq_schema::Verbosity::Minimal,
            dartq_schema::Verbosity::Compact,
            dartq_schema::Verbosity::Full,
        ] {
            dartq_schema::validate_at_tier(&text, dartq_schema::Domain::Compat, tier)
                .expect("the minimal cut is exact at every tier");
        }

        let born = to_json(&golden_report()).to_string();
        for tier in [
            dartq_schema::Verbosity::Compact,
            dartq_schema::Verbosity::Full,
        ] {
            dartq_schema::validate_at_tier(&born, dartq_schema::Domain::Compat, tier)
                .expect("the born shape carries its evidence at compact and full");
        }
        let violation = dartq_schema::validate_at_tier(
            &born,
            dartq_schema::Domain::Compat,
            dartq_schema::Verbosity::Minimal,
        )
        .expect_err("the evidence columns are not minimal-tier content");
        assert!(
            violation.path.contains("first_failure"),
            "the first evidence key is named: {violation}"
        );
    }

    /// The compat notice fixture (the config tests' twin shape): a real
    /// package directory with two stub SDKs — `skew`'s whitelist names a
    /// fabricated experiment the vendored feature registry does not know,
    /// `clean`'s is empty — and one trivial lib file as the queried target.
    fn fixture(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("dq_cmd_compat_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |relative: &str, content: &str| {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(path, content).expect("file");
        };
        write(
            "pubspec.yaml",
            "name: fixture\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            "pubspec.lock",
            "packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n",
        );
        write(
            ".dart_tool/package_config.json",
            // The pub-writer shape: pub always emits `packageUri: "lib/"`
            // (upstream pub source, lib/src/entrypoint.dart:511) — without
            // it the reader contract anchors the source root at the
            // package root itself (package_config_json.dart:152).
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "packageUri": "lib/", "languageVersion": "3.5"}], "generator": "pub"}"#,
        );
        write("sdks/clean/version", "3.13.1\n");
        write("sdks/skew/version", "3.9.0\n");
        for sdk in ["clean", "skew"] {
            write(
                &format!("sdks/{sdk}/lib/libraries.json"),
                r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#,
            );
        }
        write(
            "sdks/clean/lib/_internal/allowed_experiments.json",
            r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
        );
        write(
            "sdks/skew/lib/_internal/allowed_experiments.json",
            r#"{"version": 1, "experimentSets": {"sdkExperiments": ["future-flag"]}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {"fixture": {"experimentSet": "sdkExperiments"}}}"#,
        );
        write("lib/a.dart", "class Foo {}\n");
        root
    }

    /// The axis notice contract (#1311): the skewed axis's engine emits its
    /// `ExperimentDegrade` exactly once, rendered as the one-line human
    /// notice; the clean axis stays silent; the matrix answers regardless —
    /// an unknown grant degrades, it never fails an axis.
    #[test]
    fn compat_surfaces_the_skew_notice_once_per_skewed_axis() {
        let root = fixture("notices");
        let target = root.join("lib/a.dart");
        let (report, notices) = compat(
            &root,
            &target,
            &[root.join("sdks/clean"), root.join("sdks/skew")],
        )
        .expect("run");
        assert_eq!(report.matrix.len(), 2, "one row per axis, in order");
        let json = serde_json::to_value(&report).expect("report serializes");
        for axis in json["matrix"].as_array().expect("matrix") {
            assert_eq!(
                axis["compatible"], true,
                "the degrade never fails an axis: {axis}"
            );
        }
        assert_eq!(
            notices,
            vec![
                "note: SDK 3.9.0 names unknown experiment(s) future-flag; granting known-only"
                    .to_owned()
            ],
            "one notice from the skewed axis only"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The `Failure.file` label is the package-form projection (a `package:`
    /// URI for `lib/` files), not the raw canonical path: a `lib/` file of
    /// package `fixture` answers
    /// `package:fixture/a.dart`, matching the other AI faces' references.
    #[test]
    fn first_failure_file_is_the_package_label() {
        let root = fixture("pkglabel");
        // A missing file target lands `uri_does_not_exist` on every stub SDK
        // (the file-existence face is axis-independent; the `dart:` table is
        // the generated oracle-pinned one).
        std::fs::write(
            root.join("lib/a.dart"),
            "import 'gone.dart';\nclass Foo {}\n",
        )
        .expect("lib file");
        let target = root.join("lib/a.dart");
        let (report, _notices) = compat(&root, &target, &[root.join("sdks/clean")]).expect("run");
        assert_eq!(report.matrix.len(), 1);
        let failure = report.matrix[0]
            .first_failure
            .as_ref()
            .expect("the uri_does_not_exist row");
        assert_eq!(failure.code, "uri_does_not_exist");
        assert_eq!(
            failure.file, "package:fixture/a.dart",
            "the file reference projects to the package form"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The fixture turned into a two-package pub workspace: the root
    /// declares `packages/a`, the member opts in, and the root config
    /// covers both — the shape the pre-convergence assembly mis-refused
    /// (it demanded the member's own `.dart_tool/package_config.json`).
    fn member_fixture(tag: &str) -> PathBuf {
        let root = fixture(tag);
        std::fs::write(
            root.join("pubspec.yaml"),
            "name: fixture\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - packages/a\n",
        )
.expect("root pubspec");
        std::fs::create_dir_all(root.join("packages/a/lib")).expect("member dir");
        std::fs::write(
            root.join("packages/a/pubspec.yaml"),
            "name: a\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        )
        .expect("member pubspec");
        std::fs::write(
            root.join(".dart_tool/package_config.json"),
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "languageVersion": "3.5"}, {"name": "a", "rootUri": "../packages/a", "languageVersion": "3.5"}], "generator": "pub"}"#,
        )
.expect("config");
        std::fs::write(root.join("packages/a/lib/a.dart"), "class Bar {}\n").expect("member file");
        root
    }

    /// The convergence (#1374 P1-1): pointing compat at a workspace MEMBER
    /// analyzes the root's resolution and answers (the pre-convergence
    /// assembly refused with `no package config at <member>/.dart_tool/…`),
    /// and the graph carries the member set — the `is_workspace_member`
    /// facts the axes' tables now hold where the pre-fix graph answered
    /// `None`.
    #[test]
    fn member_invocation_assembles_the_root_workspace() {
        let root = member_fixture("member");
        let member = root.join("packages/a");
        let (report, notices) = compat(
            &member,
            &member.join("lib/a.dart"),
            &[root.join("sdks/clean")],
        )
        .expect("a member analyzes the root's resolution");
        assert_eq!(report.matrix.len(), 1, "one axis, answered");
        assert!(notices.is_empty(), "fresh resolution: {notices:?}");

        // The member facts the axes consume, through the same strict
        // assembly the face just ran.
        let (config, graph, config_dir, _) =
            package_assembly_strict(&member).expect("strict assembly");
        let packages = PackageTable::build(&config, &graph, &config_dir).expect("table");
        assert!(packages.is_workspace_member("a"), "the member set rides");
        assert!(
            !packages.is_workspace_member("fixture"),
            "the root is not a member"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The strict-refuse posture that survives the convergence: a package
    /// with no resolvable config anywhere up the root chain is the usage
    /// tier at the root's config path (the analyze path degrades where
    /// compat refuses), and an unusable config refuses too, naming the
    /// defect.
    #[test]
    fn strict_refuse_posture_holds() {
        let root = fixture("noconfig");
        std::fs::remove_file(root.join(".dart_tool/package_config.json")).expect("drop config");
        let root = root.canonicalize().expect("root");
        let error = compat(&root, &root.join("lib/a.dart"), &[root.join("sdks/clean")])
            .expect_err("no config anywhere refuses");
        assert!(
            matches!(error, CompatError::Usage(_)),
            "usage tier: {error:?}"
        );
        assert_eq!(
            error.to_string(),
            format!(
                "no package config at {} (run `dart pub get`)",
                root.join(".dart_tool/package_config.json").display()
            )
        );

        let corrupt = fixture("corrupt");
        std::fs::write(corrupt.join(".dart_tool/package_config.json"), "{not json")
            .expect("corrupt config");
        let error = compat(
            &corrupt,
            &corrupt.join("lib/a.dart"),
            &[corrupt.join("sdks/clean")],
        )
        .expect_err("an unusable config refuses");
        assert!(
            error.to_string().contains("is unusable"),
            "the refusal names the defect: {error}"
        );

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&corrupt).ok();
    }

    /// The resolution advisory rides the notices ahead of the axis notices
    /// — the strict assembly's Stale variant, rendered `note:` on the human
    /// stream — and never gates: the matrix answers regardless.
    #[test]
    fn stale_resolution_rides_the_notices() {
        let root = fixture("stale_notice");
        std::fs::write(
            root.join(".dart_tool/package_config.json"),
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "languageVersion": "3.5"}, {"name": "ghost", "rootUri": "../ghost", "languageVersion": "3.5"}], "generator": "pub"}"#,
        )
.expect("config");
        let (report, notices) = compat(&root, &root.join("lib/a.dart"), &[root.join("sdks/clean")])
            .expect("stale is advisory, not refusal");
        assert_eq!(report.matrix.len(), 1);
        assert!(report.matrix[0].compatible, "the advisory never gates");
        assert_eq!(notices.len(), 1, "one advisory line: {notices:?}");
        assert!(
            notices[0].starts_with("note: package_config.json is out of date"),
            "{notices:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
