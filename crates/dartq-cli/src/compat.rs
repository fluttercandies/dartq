//! `dartq compat` — the PRD §15.5 compatibility face: the same workspace
//! analyzed under several SDK roots, aggregated into a per-axis ✓/✗ matrix.
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
//! remaining axes still answer.
//!
//! Output is the §15.7 JSON contract (`schema_version: 1`): the queried
//! file plus the matrix in `--sdk-root` order. The axis verdict rule and
//! the `first_failure` ordering live in [`dartq_ai::compatibility`].
//!
//! Exit codes follow the query faces: 0 on a successful query —
//! incompatible axes are the answer's data, not a failure — 64 usage, 4
//! internal. `--context` labels and dartiq.yaml context-set declaration are
//! follow-ups; this round takes explicit `--sdk-root` args (empty falls
//! back to `$DART_SDK`, the single default axis).

use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Args;

use dartq_ai::compatibility as query;
use dartq_ai::compatibility::{AxisAnalysis, AxisInput, Failure};
use dartq_diagnostics::{FileLayoutProvider, Severity};
use dartq_engine::{
    CancellationToken, Cancelled, Engine, EngineConfig, EngineEvent, memory_store, semdiag,
};
use dartq_package::PackageTable;
use dartq_pub::{PackageConfig, ResolvedPackageGraph, build_graph};
use dartq_sdk::SdkSnapshot;

use crate::{EngineLayout, exit, load_workspace_faces};

#[derive(Args)]
pub(crate) struct CompatArgs {
    /// The queried file — the matrix's subject; must be an analyzed `.dart`
    /// file of the workspace.
    pub(crate) path: PathBuf,
    /// Package directory holding pubspec.yaml and
    /// `.dart_tool/package_config.json`; defaults to the current directory.
    #[arg(default_value = ".")]
    pub(crate) dir: PathBuf,
    /// Dart SDK root, one matrix axis each; repeatable. Defaults to
    /// `$DART_SDK` (the single default axis).
    #[arg(long = "sdk-root")]
    pub(crate) sdk_roots: Vec<PathBuf>,
}

/// Crash tier like the other query faces: a panic aborts with exit 4 and
/// stdout kept empty, because [`run`] writes only after the query fully
/// succeeded.
pub(crate) fn run_entry(args: &CompatArgs) -> u8 {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(args, &mut BufWriter::new(io::stdout().lock()))
    }));
    let Ok(code) = outcome else {
        eprintln!("dartq: internal error: analysis panicked");
        return exit::INTERNAL_ERROR;
    };
    code
}

/// Why a compat run aborts without a matrix: workspace problems are the
/// usage tier, engine failures the internal tier (the query faces' tiers;
/// each carries its printed message body).
enum Abort {
    Usage(String),
    Internal(String),
}

fn abort_code(abort: Abort) -> u8 {
    match abort {
        Abort::Usage(message) => {
            eprintln!("error: {message}");
            exit::USAGE
        }
        Abort::Internal(message) => {
            eprintln!("dartq: internal error: {message}");
            exit::INTERNAL_ERROR
        }
    }
}

fn run(args: &CompatArgs, out: &mut impl Write) -> u8 {
    // Canonicalized up front so the printed paths are symlink-resolved (the
    // oracle convention) and the target containment check compares like
    // with like.
    let Ok(dir) = args.dir.canonicalize() else {
        eprintln!(
            "error: Directory or file doesn't exist: {}",
            args.dir.display()
        );
        return exit::USAGE;
    };
    let Ok(target) = args.path.canonicalize() else {
        eprintln!(
            "error: Directory or file doesn't exist: {}",
            args.path.display()
        );
        return exit::USAGE;
    };
    if target.extension() != Some(std::ffi::OsStr::new("dart")) || !target.starts_with(&dir) {
        eprintln!("error: not an analyzed .dart file: {}", target.display());
        return exit::USAGE;
    }

    let Some(roots) = sdk_roots(args) else {
        return exit::USAGE;
    };

    // The pub faces and the package graph are axis-independent: built once,
    // reused by every axis's engine assembly.
    let config_dir = dir.join(".dart_tool");
    let (config, lock, pubspec) = match load_workspace_faces(&dir, &config_dir) {
        Ok(faces) => faces,
        Err(message) => return abort_code(Abort::Usage(message)),
    };
    let graph = match build_graph(
        &config,
        &lock,
        &pubspec,
        &std::collections::BTreeMap::new(),
        None,
        &config_dir,
    ) {
        Ok(graph) => graph,
        Err(error) => return abort_code(Abort::Usage(error.to_string())),
    };

    let mut matrix: Vec<AxisInput> = Vec::with_capacity(roots.len());
    for root in &roots {
        match analyze_axis(root, &config, &graph, &config_dir) {
            Ok(axis) => matrix.push(axis),
            Err(abort) => return abort_code(abort),
        }
    }

    let report = query::compatibility(&target.display().to_string(), &matrix);
    let written = out
        .write_all(
            serde_json::to_string(&report)
                .expect("CompatReport serializes")
                .as_bytes(),
        )
        .and_then(|()| out.write_all(b"\n"))
        .and_then(|()| out.flush());
    if let Err(error) = written {
        eprintln!("dartq: internal error: writing the compat report failed: {error}");
        return exit::INTERNAL_ERROR;
    }
    exit::CLEAN
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
) -> Result<AxisInput, Abort> {
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
        .map_err(|error| Abort::Usage(error.to_string()))?;
    let mut engine = Engine::new(EngineConfig {
        store: memory_store(),
        sdk: snapshot,
        packages,
        pub_config: config.clone(),
        config_dir: config_dir.to_path_buf(),
        // The semantic families are part of the analyze contract (#236):
        // the SDK-difference signal (`undefined_*` and friends) rides the
        // default diagnoser.
        semdiag: Some(semdiag::diagnoser()),
    })
    .map_err(|error| Abort::Usage(format!("engine context unusable: {error}")))?;

    if let Err(error) = engine.discover_workspace() {
        return Err(Abort::Internal(format!("discovery failed: {error}")));
    }
    // The read-after-write: the matrix reflects the analyzed state only.
    let mut on_event = |_: EngineEvent| {};
    let diags = engine
        .analyze(&[], &mut on_event, &CancellationToken::default())
        .map_err(|Cancelled| Abort::Internal("analysis cancelled".to_owned()))?;

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

/// The axis roots: the explicit `--sdk-root` args, else `$DART_SDK` (the
/// single default axis). `None` is the usage tier (the message is already
/// printed), mirroring the other engine faces' SDK resolution.
fn sdk_roots(args: &CompatArgs) -> Option<Vec<PathBuf>> {
    if !args.sdk_roots.is_empty() {
        return Some(args.sdk_roots.clone());
    }
    match std::env::var_os("DART_SDK") {
        Some(root) if !root.is_empty() => Some(vec![PathBuf::from(root)]),
        _ => {
            eprintln!("error: no Dart SDK root (pass --sdk-root PATH or set DART_SDK)");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// The fixture shape of the impact face's tests: a real package
    /// directory with stub SDKs (`version` + `lib/libraries.json` +
    /// `lib/_internal/allowed_experiments.json` are all
    /// [`dartq_sdk::SdkSnapshot::load`] needs) and one lib file whose
    /// `dart:async` import resolves only on the SDK whose library table
    /// carries it (the URI layer needs no source bodies; the stub body
    /// exists so a lazy read finds a file).
    fn fixture(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("dq_cli_compat_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let write = |relative: &str, content: &str| {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            fs::write(path, content).expect("file");
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
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "languageVersion": "3.5"}], "generator": "pub"}"#,
        );
        // The async-carrying SDK (the compatible axis).
        write("sdks/new/version", "3.13.1\n");
        write(
            "sdks/new/lib/libraries.json",
            r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}, "async": {"uri": "async/async.dart"}}}}"#,
        );
        write("sdks/new/lib/async/async.dart", "library dart.async;\n");
        // The async-less SDK (the incompatible axis): `dart:async` maps to
        // no library, `import 'dart:async';` codes `uri_does_not_exist`.
        write("sdks/old/version", "3.5.0\n");
        write(
            "sdks/old/lib/libraries.json",
            r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#,
        );
        // The broken SDK root (the isolated axis): exists, but no `version`.
        fs::create_dir_all(root.join("sdks/broken")).expect("dir");
        for sdk in ["new", "old"] {
            write(
                &format!("sdks/{sdk}/lib/_internal/allowed_experiments.json"),
                r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
            );
        }
        write("lib/a.dart", "import 'dart:async';\nclass Foo {}\n");
        root
    }

    /// End to end over stub SDK roots: the async-less axis answers
    /// incompatible with the `uri_does_not_exist` failure at the URI
    /// literal, the async-carrying axis clean, and the broken root is one
    /// isolated `sdk_unavailable` row — all under one successful exit.
    #[test]
    fn compat_end_to_end_differentiated_matrix() {
        let root = fixture("e2e");
        let args = CompatArgs {
            path: root.join("lib/a.dart"),
            dir: root.clone(),
            sdk_roots: vec![
                root.join("sdks/old"),
                root.join("sdks/new"),
                root.join("sdks/broken"),
            ],
        };
        let mut out: Vec<u8> = Vec::new();
        let code = run(&args, &mut out);
        assert_eq!(code, exit::CLEAN, "incompatible axes are data, not failure");

        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(out).expect("UTF-8")).expect("one object");
        assert_eq!(json["schema_version"], 1);
        assert_eq!(
            json["file"],
            root.canonicalize()
                .expect("root")
                .join("lib/a.dart")
                .display()
                .to_string(),
        );
        let matrix = json["matrix"].as_array().expect("matrix");
        assert_eq!(matrix.len(), 3, "one row per --sdk-root, in CLI order");

        assert_eq!(matrix[0]["sdk"], "3.5.0");
        assert_eq!(matrix[0]["compatible"], false);
        assert_eq!(matrix[0]["first_failure"]["code"], "uri_does_not_exist");
        assert_eq!(
            matrix[0]["first_failure"]["file"],
            root.canonicalize()
                .expect("root")
                .join("lib/a.dart")
                .display()
                .to_string(),
        );
        assert_eq!(matrix[0]["first_failure"]["line"], 1);
        assert_eq!(matrix[0]["first_failure"]["col"], 8, "the URI literal span");
        assert!(matrix[0].get("error").is_none());

        assert_eq!(matrix[1]["sdk"], "3.13.1");
        assert_eq!(matrix[1]["compatible"], true);
        assert!(matrix[1].get("first_failure").is_none());
        assert!(matrix[1].get("error").is_none());

        assert_eq!(
            matrix[2]["sdk"],
            root.join("sdks/broken").display().to_string()
        );
        assert_eq!(matrix[2]["compatible"], false);
        assert_eq!(matrix[2]["error"]["kind"], "sdk_unavailable");
        assert!(
            matrix[2]["error"]["message"]
                .as_str()
                .expect("message")
                .contains("sdks/broken"),
            "the load error names the offending root"
        );
        assert!(matrix[2].get("first_failure").is_none());

        fs::remove_dir_all(&root).ok();
    }

    /// Usage tier: a target outside the analyzed workspace.
    #[test]
    fn target_outside_workspace_is_usage() {
        let root = fixture("usage");
        let args = CompatArgs {
            path: root.join("lib/missing.dart"),
            dir: root.clone(),
            sdk_roots: vec![root.join("sdks/new")],
        };
        assert_eq!(run(&args, &mut Vec::new()), exit::USAGE);
        fs::remove_dir_all(&root).ok();
    }
}
