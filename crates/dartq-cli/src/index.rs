//! `dartq index` — the index-building face: build the symbol
//! index over the analyzed workspace and report what it built.
//!
//! The build (per-library capture + resolution riding the engine's
//! per-library view) and the totals live in [`dartq_commands::index`] — the
//! one implementation the daemon's RPC face will share. This binary adds
//! argument parsing, the crash tier, and the usage-tier mapping.
//!
//! Output is the unified JSON contract (`v: 1`), one single-line object
//! — the impact/compat convention for the newer command family, not the
//! query faces' machine grammar.

use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

use clap::Args;

use dartq_commands::index::IndexError;
use dartq_commands::index::run as index_run;

use crate::Verbosity;
use crate::exit;

#[derive(Args)]
pub(crate) struct IndexArgs {
    /// Export the maintained index as a JSONL document stream (the SCIP
    /// document model over JSON): a header line of row totals, then one
    /// document per analyzed file. `-v/--verbosity` shapes the documents
    /// (minimal = path + symbol counts, compact = + symbol table and
    /// declaration rows, full = + occurrence and supertype rows).
    #[arg(long)]
    pub(crate) full: bool,
    /// Package directory holding pubspec.yaml and
    /// `.dart_tool/package_config.json`; defaults to the current directory.
    #[arg(default_value = ".")]
    pub(crate) dir: PathBuf,
    /// Dart SDK root; defaults to `$DART_SDK`.
    #[arg(long)]
    pub(crate) sdk: Option<PathBuf>,
    /// Report detail tier (`-v`/`--verbosity minimal|compact|full`). Without
    /// `--full` the run summary is tier-invariant (four verdict totals —
    /// no locator, no prose, nothing to trim); under `--full` the tier
    /// shapes the export documents (the canonical `Domain::IndexExport`
    /// tier table).
    #[arg(short = 'v', long, value_enum, default_value_t = Verbosity::Compact)]
    pub(crate) verbosity: Verbosity,
    /// Output format for the summary face: `lon` (the default) or
    /// `json` (both the same value), or `text` (the indented report).
    /// The `--full` export stream is the commands crate's JSONL writer
    /// and stays JSON regardless.
    #[arg(long, value_enum, default_value_t = crate::OutputFormat::Lon)]
    pub(crate) format: crate::OutputFormat,
}

/// Crash tier like the other engine faces: a panic aborts with exit 4. The
/// summary face keeps stdout empty because it writes only after the build
/// fully succeeds; the `--full` export streams incrementally, so a mid-run
/// panic can leave a truncated prefix — the same trade every streaming face
/// makes, and a consumer detects it by the missing terminal newline/counts.
pub(crate) fn run_entry(args: &IndexArgs) -> u8 {
    run(args, &mut BufWriter::new(io::stdout().lock()))
}

fn run(args: &IndexArgs, out: &mut impl Write) -> u8 {
    let outcome = crate::crash_tier(|| run_checked(args, out));
    let Ok(code) = outcome else {
        eprintln!("dartq: internal error: analysis panicked");
        return exit::INTERNAL_ERROR;
    };
    code
}

fn run_checked(args: &IndexArgs, out: &mut impl Write) -> u8 {
    if args.full {
        return export_checked(args, out);
    }
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

    let report = built.to_json();
    let written = match args.format {
        crate::OutputFormat::Json => out
            .write_all(serde_json::to_string(&report).expect("IndexRun serializes").as_bytes())
            .and_then(|()| out.write_all(b"\n")),
        crate::OutputFormat::Lon => crate::output::write_lon(out, &report),
        crate::OutputFormat::Text => crate::output::write_text_value(out, &report),
    }
    .and_then(|()| out.flush());
    if let Err(error) = written {
        eprintln!("dartq: internal error: writing the index report failed: {error}");
        return exit::INTERNAL_ERROR;
    }
    exit::CLEAN
}

/// The `--full` half: the JSONL export stream (header + one document per
/// analyzed file), tier-shaped by `-v/--verbosity`. Errors map onto the
/// same tiers as the summary face.
fn export_checked(args: &IndexArgs, out: &mut impl Write) -> u8 {
    match dartq_commands::index::run_export(
        &args.dir,
        args.sdk.as_deref(),
        args.verbosity.into_commands(),
        out,
    ) {
        Ok(_) => exit::CLEAN,
        Err(error @ (IndexError::Workspace(_) | IndexError::Engine(_))) => {
            eprintln!("error: {error}");
            exit::USAGE
        }
        Err(error) => {
            eprintln!("dartq: internal error: {error}");
            exit::INTERNAL_ERROR
        }
    }
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
            full: false,
            dir: root.to_path_buf(),
            sdk: Some(root.join("sdk")),
            verbosity: Verbosity::Compact,
            format: crate::OutputFormat::Json,
        }
    }

    /// End to end over the query test's fixture: the JSON report with
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
        assert_eq!(json["v"], 1);
        assert_eq!(json["files"], 2);
        assert_eq!(json["libraries"], 2);
        assert_eq!(json["symbols"], 4);
        assert_eq!(json["refs"], 2);

        let missing = IndexArgs {
            dir: root.join("nope"),
            ..args(&root)
        };
        assert_eq!(run(&missing, &mut Vec::new()), exit::USAGE);

        fs::remove_dir_all(&root).ok();
    }

    /// The `--full` face end to end over the same fixture: the stream opens
    /// with the header counts (2 documents, 4 declaration rows, 2
    /// occurrences) and each document line is a schema-valid export object
    /// in canonical-path order.
    #[test]
    fn index_full_streams_jsonl_documents() {
        let root = fixture("full");
        let mut out: Vec<u8> = Vec::new();
        let export = IndexArgs {
            full: true,
            ..args(&root)
        };
        assert_eq!(run(&export, &mut out), exit::CLEAN);
        let text = String::from_utf8(out).expect("UTF-8 stream");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "header + 2 documents");
        let header: serde_json::Value = serde_json::from_str(lines[0]).expect("header");
        assert_eq!(header["documents"], 2);
        assert_eq!(header["declarations"], 4);
        // Compact tier: no occurrence/supertype rows.
        assert!(header.get("occurrences").is_none());
        for line in &lines[1..] {
            let doc: serde_json::Value = serde_json::from_str(line).expect("document");
            assert_eq!(doc["v"], 1);
            assert!(doc["path"].is_string());
            assert!(doc["declarations"].is_array());
        }

        // Full verbosity exposes the occurrence rows end to end.
        let mut out: Vec<u8> = Vec::new();
        let export = IndexArgs {
            verbosity: Verbosity::Full,
            ..export
        };
        assert_eq!(run(&export, &mut out), exit::CLEAN);
        let text = String::from_utf8(out).expect("UTF-8 stream");
        let header: serde_json::Value =
            serde_json::from_str(text.lines().next().expect("header")).expect("header");
        assert_eq!(header["occurrences"], 2, "Foo() and foo() in b.dart");
        assert_eq!(header["supertypes"], 0);
        let b_doc = text
            .lines()
            .nth(2)
            .and_then(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .expect("b.dart document");
        assert_eq!(b_doc["path"], "package:fixture/lib/b.dart");
        assert_eq!(b_doc["occurrences"].as_array().expect("rows").len(), 2);

        fs::remove_dir_all(&root).ok();
    }
}
