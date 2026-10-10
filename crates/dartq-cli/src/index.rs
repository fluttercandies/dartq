//! `dartq index` — the PRD §15.1 index-building face: build the symbol
//! index over the analyzed workspace and report what it built.
//!
//! The build (per-library capture + resolution riding the engine's
//! per-library view) and the totals live in [`dartq_commands::index`] — the
//! one implementation the daemon's RPC face will share. This binary adds
//! argument parsing, the crash tier, and the usage-tier mapping.
//!
//! Output is the §15.1 unified JSON contract (`schema_version: 1`), one
//! single-line object — the impact/compat convention for the newer command
//! family, not the query faces' machine grammar.

use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

use clap::Args;

use dartq_commands::index::IndexError;
use dartq_commands::index::run as index_run;

use crate::exit;

#[derive(Args)]
pub(crate) struct IndexArgs {
    /// Force a rebuild instead of reusing incremental state. The engine has
    /// no resident index yet — every run builds fresh — so both modes build
    /// full today; the flag pins the §15.1 grammar.
    #[arg(long)]
    pub(crate) full: bool,
    /// Package directory holding pubspec.yaml and
    /// `.dart_tool/package_config.json`; defaults to the current directory.
    #[arg(default_value = ".")]
    pub(crate) dir: PathBuf,
    /// Dart SDK root; defaults to `$DART_SDK`.
    #[arg(long)]
    pub(crate) sdk: Option<PathBuf>,
}

/// Crash tier like the other engine faces: a panic aborts with exit 4 and
/// stdout kept empty, because [`run`] writes only after the build fully
/// succeeded.
pub(crate) fn run_entry(args: &IndexArgs) -> u8 {
    run(args, &mut BufWriter::new(io::stdout().lock()))
}

fn run(args: &IndexArgs, out: &mut impl Write) -> u8 {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_checked(args, out)));
    let Ok(code) = outcome else {
        eprintln!("dartq: internal error: analysis panicked");
        return exit::INTERNAL_ERROR;
    };
    code
}

fn run_checked(args: &IndexArgs, out: &mut impl Write) -> u8 {
    let built = match index_run(&args.dir, args.sdk.as_deref(), args.full) {
        Ok(built) => built,
        // Workspace/engine-bind problems are the usage tier (their Display
        // bodies are the printed messages); prelude failures the internal
        // tier.
        Err(error @ (IndexError::Workspace(_) | IndexError::Engine(_))) => {
            eprintln!("error: {error}");
            return exit::USAGE;
        }
        Err(error) => {
            eprintln!("dartq: internal error: {error}");
            return exit::INTERNAL_ERROR;
        }
    };

    let written = out
        .write_all(
            serde_json::to_string(&built.to_json())
                .expect("IndexRun serializes")
                .as_bytes(),
        )
        .and_then(|()| out.write_all(b"\n"))
        .and_then(|()| out.flush());
    if let Err(error) = written {
        eprintln!("dartq: internal error: writing the index report failed: {error}");
        return exit::INTERNAL_ERROR;
    }
    exit::CLEAN
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// The fixture shape of the analyze/query faces' tests: a real package
    /// directory with a stub SDK and a lib pair whose cross-file uses the
    /// index must resolve (the same files the callers test pins).
    fn fixture(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("dq_cli_index_{tag}_{}", std::process::id()));
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
        write("sdk/version", "3.13.1\n");
        write(
            "sdk/lib/libraries.json",
            r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#,
        );
        write(
            "sdk/lib/_internal/allowed_experiments.json",
            r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
        );
        write(
            "lib/a.dart",
            "class Foo {\n  void meth() {}\n}\nvoid foo() {}\n",
        );
        write(
            "lib/b.dart",
            "import 'a.dart';\nvoid main() {\n  var f = Foo();\n  foo();\n}\n",
        );
        root
    }

    fn args(root: &std::path::Path) -> IndexArgs {
        IndexArgs {
            full: true,
            dir: root.to_path_buf(),
            sdk: Some(root.join("sdk")),
        }
    }

    /// End to end over the query test's fixture: the §15.1 JSON report with
    /// the pinned totals (2 files, 2 libraries, 4 declaration rows — Foo,
    /// meth, foo, main — and 2 resolved reference rows: the `Foo()` and
    /// `foo()` call sites), exit 0; a missing directory is the usage tier.
    #[test]
    fn index_runs_over_temp_workspace() {
        let root = fixture("smoke");
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(run(&args(&root), &mut out), exit::CLEAN);
        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(out).expect("UTF-8")).expect("one object");
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["files"], 2);
        assert_eq!(json["libraries"], 2);
        assert_eq!(json["symbols"], 4);
        assert_eq!(json["references"], 2);

        let missing = IndexArgs {
            dir: root.join("nope"),
            ..args(&root)
        };
        assert_eq!(run(&missing, &mut Vec::new()), exit::USAGE);

        fs::remove_dir_all(&root).ok();
    }
}
