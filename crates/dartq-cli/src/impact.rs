//! `dartq impact` — the PRD §15.4 impact face: what a change to one file
//! would affect, over the analyzed workspace.
//!
//! The substrate (import closure over the engine's CSR, the test graph, the
//! folded file snapshot) and the report assembly live in
//! [`dartq_commands::impact`] — the one implementation the daemon's
//! `dartq/impact` shares. This binary adds argument parsing, the crash
//! tier, the usage-tier mapping, and the JSON output.
//!
//! One-shot posture: a CLI run holds no baseline, so the seed enters the
//! query as a **hypothetical** API-level change — the report answers "what
//! depends on this file's API surface". The daemon passes real before/after
//! diffs through the same query.
//!
//! Output is the §15.7 JSON contract (`schema_version: 1`, stable path
//! labels; the v1 labels are the canonical file paths — package-form URIs
//! land with the daemon's URI-aware substrate).

use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

use clap::Args;

use dartq_commands::impact::{ImpactError, impact as query_impact};
use dartq_commands::workspace::open_engine;

use crate::exit;

#[derive(Args)]
pub(crate) struct ImpactArgs {
    /// The changed file to assess; must be an analyzed `.dart` file of the
    /// workspace.
    pub(crate) path: PathBuf,
    /// Package directory holding pubspec.yaml and
    /// `.dart_tool/package_config.json`; defaults to the current directory.
    #[arg(default_value = ".")]
    pub(crate) dir: PathBuf,
    /// Dart SDK root; defaults to `$DART_SDK`.
    pub(crate) sdk: Option<PathBuf>,
}

/// Crash tier like the other engine faces: a panic aborts with exit 4 and
/// stdout kept empty, because [`run`] writes only after the query fully
/// succeeded.
pub(crate) fn run_entry(args: &ImpactArgs) -> u8 {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(args, &mut BufWriter::new(io::stdout().lock()))
    }));
    let Ok(code) = outcome else {
        eprintln!("dartq: internal error: analysis panicked");
        return exit::INTERNAL_ERROR;
    };
    code
}

fn run(args: &ImpactArgs, out: &mut impl Write) -> u8 {
    let (mut engine, _dir) = match open_engine(&args.dir, args.sdk.as_deref()) {
        Ok(faces) => faces,
        Err(error) => {
            eprintln!("error: {error}");
            return exit::USAGE;
        }
    };
    let report = match query_impact(&mut engine, &args.path) {
        Ok(report) => report,
        // Target problems are the usage tier (their Display bodies are the
        // printed messages); prelude failures the internal tier.
        Err(error @ (ImpactError::Missing(_) | ImpactError::NotAnalyzed(_))) => {
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
            serde_json::to_string(&report)
                .expect("ImpactReport serializes")
                .as_bytes(),
        )
        .and_then(|()| out.write_all(b"\n"))
        .and_then(|()| out.flush());
    if let Err(error) = written {
        eprintln!("dartq: internal error: writing the impact report failed: {error}");
        return exit::INTERNAL_ERROR;
    }
    exit::CLEAN
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// The fixture shape of the analyze/query faces' tests: a real package
    /// directory with a stub SDK (`sdk/lib/libraries.json` is the only file
    /// [`dartq_sdk::SdkSnapshot::load`] needs), a lib pair, and the
    /// convention-placed test that reaches the lib through its package URI.
    fn fixture(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("dq_cli_impact_{tag}_{}", std::process::id()));
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
        write("lib/a.dart", "class Foo {}\nvoid foo() {}\n");
        write(
            "lib/b.dart",
            "import 'a.dart';\nvoid use() {\n  Foo? f = Foo();\n  foo();\n}\n",
        );
        write(
            "test/a_test.dart",
            "import 'package:fixture/a.dart';\nimport 'package:test/test.dart';\nvoid main() {\n  test('hits foo', () {\n    foo();\n  });\n}\n",
        );
        root
    }

    /// End to end: the hypothetical change of `lib/a.dart` names its face
    /// symbols, tags the direct importers (`b.dart`, the test) semantic,
    /// and lands the test file in AffectedTests.
    #[test]
    fn impact_end_to_end() {
        let root = fixture("e2e");
        let a = root.canonicalize().expect("root").join("lib/a.dart");
        let args = ImpactArgs {
            path: a.clone(),
            dir: root.clone(),
            sdk: Some(root.join("sdk")),
        };
        let mut out: Vec<u8> = Vec::new();
        let code = run(&args, &mut out);
        assert_eq!(code, exit::CLEAN);

        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(out).expect("UTF-8")).expect("one object");
        assert_eq!(json["schema_version"], 1);
        let symbols: Vec<&str> = json["changed"]
            .as_array()
            .expect("changed")
            .iter()
            .map(|row| row["symbol"].as_str().expect("symbol"))
            .collect();
        assert_eq!(
            symbols,
            vec![
                format!("{}|Foo", a.display()),
                format!("{}|foo", a.display()),
            ],
            "every face name, ASCII-sorted, hypothetical signature flags"
        );
        assert!(
            json["changed"]
                .as_array()
                .expect("changed")
                .iter()
                .all(|row| row["signature_changed"] == true),
        );

        let b = root.canonicalize().expect("root").join("lib/b.dart");
        let test_path = root.canonicalize().expect("root").join("test/a_test.dart");
        let affected: Vec<(String, &str)> = json["affected"]
            .as_array()
            .expect("affected")
            .iter()
            .map(|row| {
                (
                    row["path"].as_str().expect("path").to_string(),
                    row["impact"].as_str().expect("impact"),
                )
            })
            .collect();
        assert_eq!(
            affected,
            vec![
                (b.display().to_string(), "semantic"),
                (test_path.display().to_string(), "semantic"),
            ],
            "both import a directly; the import of the test resolves through the package URI"
        );
        let tests: Vec<String> = json["affected_tests"]
            .as_array()
            .expect("tests")
            .iter()
            .map(|row| row["path"].as_str().expect("path").to_string())
            .collect();
        assert_eq!(tests, vec![test_path.display().to_string()]);
        assert_eq!(
            json["affected_environments"],
            serde_json::json!([{ "label": "default", "compatible": true }]),
        );

        fs::remove_dir_all(&root).ok();
    }

    /// Usage tier: a path outside the analyzed set.
    #[test]
    fn unanalyzed_path_is_usage() {
        let root = fixture("usage");
        let args = ImpactArgs {
            path: root.join("lib/missing.dart"),
            dir: root.clone(),
            sdk: Some(root.join("sdk")),
        };
        assert_eq!(run(&args, &mut Vec::new()), exit::USAGE);
        fs::remove_dir_all(&root).ok();
    }

    /// The usage tier covers a target that does not exist at all: the
    /// message names the path as given.
    #[test]
    fn missing_path_is_usage() {
        let root = fixture("missing");
        let args = ImpactArgs {
            path: root.join("lib/nope.dart"),
            dir: root.clone(),
            sdk: Some(root.join("sdk")),
        };
        assert_eq!(run(&args, &mut Vec::new()), exit::USAGE);
        fs::remove_dir_all(&root).ok();
    }

    /// The engine assembly failure surfaces as the usage tier through the
    /// shared [`dartq_commands::workspace::WorkspaceError`] message body.
    #[test]
    fn unusable_sdk_is_usage() {
        let root = fixture("sdk");
        let args = ImpactArgs {
            path: root.join("lib/a.dart"),
            dir: root.clone(),
            sdk: Some(root.join("no-such-sdk")),
        };
        assert_eq!(run(&args, &mut Vec::new()), exit::USAGE);
        fs::remove_dir_all(&root).ok();
    }
}
