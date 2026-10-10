//! `dartq compat` — the PRD §15.5 compatibility face: the same workspace
//! analyzed under several SDK roots, aggregated into a per-axis ✓/✗ matrix.
//!
//! The matrix mechanics (one axis = one SDK context, sequential axes over
//! one package discovery, axis failure isolation) and the report assembly
//! live in [`dartq_commands::compat`] — the one implementation the daemon's
//! `dartq/compat` shares. This binary adds argument parsing, the axis-root
//! resolution (explicit `--sdk-root` roots, then the `dartq.yaml` contexts
//! — all declared ones by default, the `--context` labels' subset when
//! given — through [`dartq_commands::config::compat_axis_roots`], falling
//! back to `$DART_SDK` when nothing declared any axis), the tier mapping,
//! and the JSON output.
//!
//! Output is the §15.7 JSON contract (`schema_version: 1`): the queried
//! file plus the matrix in axis order (CLI flags first, then contexts in
//! file order). Contexts add rows, never row fields. The axis verdict rule
//! and the `first_failure` ordering live in [`dartq_ai::compatibility`].
//!
//! Exit codes follow the query faces: 0 on a successful query —
//! incompatible axes are the answer's data, not a failure — 64 usage, 4
//! internal.

use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

use clap::Args;

use dartq_commands::compat::{CompatError, compat as query_compat};
use dartq_commands::config::{ConfigError, compat_axis_roots};

use crate::exit;

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
    /// A dartq.yaml context label, one matrix axis each; repeatable.
    /// Empty selects every declared context; unknown labels are usage
    /// errors. Needs a dartq.yaml at the workspace root.
    #[arg(long = "context")]
    pub(crate) contexts: Vec<String>,
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

fn run(args: &CompatArgs, out: &mut impl Write) -> u8 {
    let axes = match compat_axis_roots(&args.dir, &args.sdk_roots, &args.contexts) {
        Ok(axes) => axes,
        Err(ConfigError::Invalid(message)) => {
            eprintln!("error: {message}");
            return exit::USAGE;
        }
        Err(ConfigError::Io(error)) => {
            eprintln!("dartq: internal error: {error}");
            return exit::INTERNAL_ERROR;
        }
    };
    let roots = if axes.is_empty() {
        match dart_sdk_fallback() {
            Some(roots) => roots,
            None => return exit::USAGE,
        }
    } else {
        axes
    };
    let report = match query_compat(&args.dir, &args.path, &roots) {
        Ok(report) => report,
        Err(CompatError::Usage(message)) => {
            eprintln!("error: {message}");
            return exit::USAGE;
        }
        Err(CompatError::Internal(message)) => {
            eprintln!("dartq: internal error: {message}");
            return exit::INTERNAL_ERROR;
        }
    };

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

/// The single default axis when neither the flags nor dartq.yaml declared
/// any: `$DART_SDK`, else the usage-tier error (the pre-config behavior,
/// byte-identical). `None` is the usage tier (the message is already
/// printed), mirroring the other engine faces' SDK resolution.
fn dart_sdk_fallback() -> Option<Vec<PathBuf>> {
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
            contexts: vec![],
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
            contexts: vec![],
            sdk_roots: vec![root.join("sdks/new")],
        };
        assert_eq!(run(&args, &mut Vec::new()), exit::USAGE);
        fs::remove_dir_all(&root).ok();
    }

    /// The matrix preserves the caller's axis order even when a path is
    /// relative (resolved against the process cwd by the OS, matching the
    /// other faces' argument handling).
    #[test]
    fn relative_sdk_roots_keep_axis_order() {
        let root = fixture("relative");
        let args = CompatArgs {
            path: root.join("lib/a.dart"),
            dir: root.clone(),
            contexts: vec![],
            sdk_roots: vec![root.join("sdks/new"), root.join("sdks/old")],
        };
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(run(&args, &mut out), exit::CLEAN);
        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(out).expect("UTF-8")).expect("one object");
        let matrix = json["matrix"].as_array().expect("matrix");
        assert_eq!(matrix[0]["sdk"], "3.13.1", "first axis answers first");
        assert_eq!(matrix[1]["sdk"], "3.5.0", "second axis answers second");
        fs::remove_dir_all(&root).ok();
    }

    /// Writes the two-context config at the workspace root; `new` is
    /// declared first on purpose — file order, not the sorted order.
    fn write_context_config(root: &std::path::Path) {
        fs::write(
            root.join("dartq.yaml"),
            "version: 1\ncontexts:\n  new:\n    sdk: path:sdks/new\n  old:\n    sdk: path:sdks/old\n",
        )
        .expect("config");
    }

    /// The dartq.yaml contract end to end: a config declaring two contexts
    /// is the whole axis set — no `--sdk-root`, no `--context` — and the
    /// matrix answers in file order with the frozen row shape.
    #[test]
    fn config_contexts_drive_the_matrix_without_sdk_root() {
        let root = fixture("cfgmatrix");
        write_context_config(&root);
        let args = CompatArgs {
            path: root.join("lib/a.dart"),
            dir: root.clone(),
            contexts: vec![],
            sdk_roots: vec![],
        };
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(run(&args, &mut out), exit::CLEAN);
        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(out).expect("UTF-8")).expect("one object");
        let matrix = json["matrix"].as_array().expect("matrix");
        assert_eq!(matrix.len(), 2, "one row per declared context");
        assert_eq!(
            matrix[0]["sdk"], "3.13.1",
            "file order: `new` declared first"
        );
        assert_eq!(matrix[0]["compatible"], true);
        assert_eq!(matrix[1]["sdk"], "3.5.0");
        assert_eq!(matrix[1]["compatible"], false);
        assert_eq!(matrix[1]["first_failure"]["code"], "uri_does_not_exist");
        fs::remove_dir_all(&root).ok();
    }

    /// `--context` selects a subset (explicit `--sdk-root` roots keep their
    /// CLI-order lead) and duplicate roots collapse into one row.
    #[test]
    fn context_flag_selects_subset_and_dedups() {
        let root = fixture("cfgselect");
        write_context_config(&root);

        let mut out: Vec<u8> = Vec::new();
        let args = CompatArgs {
            path: root.join("lib/a.dart"),
            dir: root.clone(),
            contexts: vec!["old".to_owned()],
            sdk_roots: vec![root.join("sdks/new")],
        };
        assert_eq!(run(&args, &mut out), exit::CLEAN);
        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(out).expect("UTF-8")).expect("one object");
        let matrix = json["matrix"].as_array().expect("matrix");
        assert_eq!(matrix.len(), 2);
        assert_eq!(matrix[0]["sdk"], "3.13.1", "the explicit flag axis leads");
        assert_eq!(matrix[1]["sdk"], "3.5.0", "the selected context follows");

        let mut out: Vec<u8> = Vec::new();
        let args = CompatArgs {
            contexts: vec!["old".to_owned(), "old".to_owned()],
            sdk_roots: vec![],
            ..args
        };
        assert_eq!(run(&args, &mut out), exit::CLEAN);
        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(out).expect("UTF-8")).expect("one object");
        let matrix = json["matrix"].as_array().expect("matrix");
        assert_eq!(matrix.len(), 1, "repeated labels resolve to one axis");
        assert_eq!(matrix[0]["sdk"], "3.5.0");
        fs::remove_dir_all(&root).ok();
    }

    /// Usage tier: an unknown label, and `--context` where no dartq.yaml
    /// declares contexts.
    #[test]
    fn context_errors_are_usage() {
        let root = fixture("cfgerrors");
        write_context_config(&root);
        let unknown = CompatArgs {
            path: root.join("lib/a.dart"),
            dir: root.clone(),
            contexts: vec!["nope".to_owned()],
            sdk_roots: vec![],
        };
        assert_eq!(run(&unknown, &mut Vec::new()), exit::USAGE);

        let bare = fixture("cfgbare");
        let no_config = CompatArgs {
            dir: bare.clone(),
            contexts: vec!["old".to_owned()],
            ..unknown
        };
        assert_eq!(run(&no_config, &mut Vec::new()), exit::USAGE);
        fs::remove_dir_all(&root).ok();
        fs::remove_dir_all(&bare).ok();
    }
}
