//! `dartq resolve` / `dartq complete` — the position faces' one-shot (the
//! CLI spelling: what the analyzed workspace's maintained index holds under
//! one cursor, and the visible-name set at it, without a resident session).
//!
//! The substrate and the answers live in [`dartq_commands::resolve`] — the
//! one implementation the daemon's `dartq/resolveAt` / `dartq/completion`
//! RPC answers share, so the faces cannot drift. This binary adds argument
//! parsing, the crash tier, the usage-tier mapping, and the format axis:
//! `lon` (the default), `json`, and `text` all encode the daemon answer
//! envelope (`v`, `off`, `decl_kind`, the hit's locator fields) verbatim —
//! the same value on every face.
//!
//! The answers are tier-invariant (every field a locator or a verdict),
//! so there is no `--verbosity` axis.

use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use clap::Args;
use serde_json::Value;

use dartq_commands::resolve::resolve_at as resolve_at_core;
use dartq_commands::resolve::{ResolveError, completion as completion_core};
use dartq_commands::workspace::open_engine;

use crate::OutputFormat;
use crate::exit;
use crate::output::{write_json_value, write_lon, write_text_value};

/// The two position faces' shared flags — one shape, two subcommands
/// (`resolve-at` and `completion` differ only in the answer they ask for).
#[derive(Args)]
pub(crate) struct PositionArgs {
    /// The `PATH:LINE:COL` cursor (1-based; the column counts UTF-16 code
    /// units). PATH resolves against DIR and must be an analyzed `.dart`
    /// file of the workspace; an uncovered position — a non-identifier, an
    /// unresolved use, a past-end offset — answers the miss verdict / the
    /// empty candidate set, never an error.
    pub(crate) selector: String,
    /// Package directory holding pubspec.yaml and
    /// `.dart_tool/package_config.json`; defaults to the current directory.
    #[arg(default_value = ".")]
    pub(crate) dir: PathBuf,
    /// Output format: `lon` (the default), `json` (the daemon answer
    /// envelope verbatim, one line), or `text` (the indented envelope).
    #[arg(long, value_enum, default_value_t = OutputFormat::Lon)]
    pub(crate) format: OutputFormat,
    /// Dart SDK root; defaults to `$DART_SDK`.
    #[arg(long)]
    pub(crate) sdk: Option<PathBuf>,
}

/// Crash tier like the other engine faces: a panic aborts with exit 4 and
/// stdout kept empty, because the runners write only after the query fully
/// succeeded.
pub(crate) fn resolve_at_entry(args: &PositionArgs) -> u8 {
    let outcome =
        crate::crash_tier(|| resolve_at_run(args, &mut BufWriter::new(io::stdout().lock())));
    let Ok(code) = outcome else {
        eprintln!("dartq: internal error: analysis panicked");
        return exit::INTERNAL_ERROR;
    };
    code
}

/// Crash tier like [`resolve_at_entry`].
pub(crate) fn completion_entry(args: &PositionArgs) -> u8 {
    let outcome =
        crate::crash_tier(|| completion_run(args, &mut BufWriter::new(io::stdout().lock())));
    let Ok(code) = outcome else {
        eprintln!("dartq: internal error: analysis panicked");
        return exit::INTERNAL_ERROR;
    };
    code
}

pub(crate) fn resolve_at_run(args: &PositionArgs, out: &mut impl Write) -> u8 {
    // The selector parses first — its usage error is the face's own, and
    // the routed call needs the resolved position (#2629). The resident
    // consult answers the same envelope over the session's maintained
    // view; the cursor converts daemon-side against the analyzed text,
    // exactly where the cold pass below converts against its own.
    let (file, line, column) = match selector_position(&args.selector, &args.dir) {
        Ok(position) => position,
        Err(message) => {
            eprintln!("error: {message}");
            return exit::USAGE;
        }
    };
    if let Some(code) =
        crate::resident::route_position(args, "dartq/resolveAt", &file, line, column, out)
    {
        return code;
    }
    let (mut engine, _dir) = match open_engine(&args.dir, args.sdk.as_deref()) {
        Ok(faces) => faces,
        Err(error) => {
            eprintln!("error: {error}");
            return exit::USAGE;
        }
    };
    let answer = match resolve_at_core(&mut engine, &file, line, column) {
        Ok(answer) => answer,
        Err(error @ ResolveError::NotResident(_)) => {
            eprintln!("error: {error}");
            return exit::USAGE;
        }
        Err(error @ ResolveError::Analyze(_)) => {
            eprintln!("dartq: internal error: {error}");
            return exit::INTERNAL_ERROR;
        }
    };
    write_answer(out, &answer, args.format, "resolve-at")
}

pub(crate) fn completion_run(args: &PositionArgs, out: &mut impl Write) -> u8 {
    // The selector parses first (the resolve face's order — its usage
    // error is the face's own), then the resident consult (#2629).
    let (file, line, column) = match selector_position(&args.selector, &args.dir) {
        Ok(position) => position,
        Err(message) => {
            eprintln!("error: {message}");
            return exit::USAGE;
        }
    };
    if let Some(code) =
        crate::resident::route_position(args, "dartq/completion", &file, line, column, out)
    {
        return code;
    }
    let (mut engine, _dir) = match open_engine(&args.dir, args.sdk.as_deref()) {
        Ok(faces) => faces,
        Err(error) => {
            eprintln!("error: {error}");
            return exit::USAGE;
        }
    };
    let answer = match completion_core(&mut engine, &file, line, column) {
        Ok(answer) => answer,
        Err(error @ ResolveError::NotResident(_)) => {
            eprintln!("error: {error}");
            return exit::USAGE;
        }
        Err(error @ ResolveError::Analyze(_)) => {
            eprintln!("dartq: internal error: {error}");
            return exit::INTERNAL_ERROR;
        }
    };
    write_answer(out, &answer, args.format, "completion")
}

fn write_answer(out: &mut impl Write, answer: &Value, format: OutputFormat, face: &str) -> u8 {
    let written = match format {
        OutputFormat::Lon => write_lon(out, answer),
        OutputFormat::Json => write_json_value(out, answer),
        OutputFormat::Text => write_text_value(out, answer),
    }
    .and_then(|()| out.flush());
    if let Err(error) = written {
        eprintln!("dartq: internal error: writing the {face} answer failed: {error}");
        return exit::INTERNAL_ERROR;
    }
    exit::CLEAN
}

/// The `PATH:LINE:COL` cursor spelling: the path resolves against DIR (the
/// workspace assembly's canonical package directory — the same base the
/// impact face resolves its target against) and canonicalizes into the
/// engine's spelling universe (the file table's interned paths are
/// canonical); LINE and COLUMN are mandatory.
fn selector_position(selector: &str, dir: &Path) -> Result<(PathBuf, u32, u32), String> {
    let parsed = crate::parse_file_selector(selector)?;
    let (Some(line), Some(column)) = (parsed.line, parsed.column) else {
        return Err(format!(
            "'{selector}' names no cursor position; the selector is PATH:LINE:COL (1-based)"
        ));
    };
    let base = if parsed.path.is_absolute() {
        parsed.path.clone()
    } else {
        dir.join(&parsed.path)
    };
    let file = base
        .canonicalize()
        .map_err(|error| format!("{}: {error}", parsed.path.display()))?;
    Ok((file, line, column))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cli, Command};
    use clap::Parser as _;
    use std::fs;

    /// The throwaway pub workspace + synthetic dart:core-only SDK, the
    /// impact face's fixture shape: one lib pair where `b.dart` resolves
    /// uses of `a.dart`'s declaration.
    fn fixture(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("dq_cli_resolve_{tag}_{}", std::process::id()));
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
            // The pub-writer shape: pub always emits `packageUri: "lib/"`
            // (upstream pub source, lib/src/entrypoint.dart:511) — without
            // it the reader contract anchors the source root at the
            // package root itself (package_config_json.dart:152), and no
            // `package:` URI addresses the `lib/` members.
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "packageUri": "lib/", "languageVersion": "3.5"}], "generator": "pub"}"#,
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
            "import 'a.dart';\nvoid use() {\n  foo();\n}\n",
        );
        root
    }

    /// The CLI face's grammar, mirroring the sibling faces' parse tests:
    /// the selector is the one positional, `dir` the optional second
    /// spelling, `--format`/`--sdk` options, `lon` the constant default.
    #[test]
    fn position_args_parse() {
        let parse_resolve = |args: &[&str]| match Cli::try_parse_from(
            std::iter::once("dartq").chain(args.iter().copied()),
        )
        .expect("parse")
        .command
        {
            Some(Command::ResolveAt(args)) => args,
            Some(_) => panic!("expected the resolve-at command for {args:?}"),
            None => panic!("expected a subcommand"),
        };
        let parse_completion = |args: &[&str]| match Cli::try_parse_from(
            std::iter::once("dartq").chain(args.iter().copied()),
        )
        .expect("parse")
        .command
        {
            Some(Command::Completion(args)) => args,
            Some(_) => panic!("expected the completion command for {args:?}"),
            None => panic!("expected a subcommand"),
        };

        let args = parse_resolve(&["resolve-at", "lib/a.dart:1:7"]);
        assert_eq!(args.selector, "lib/a.dart:1:7");
        assert_eq!(args.dir, PathBuf::from("."), "default target cwd");
        assert!(args.sdk.is_none(), "no sdk by default");
        assert!(
            matches!(args.format, OutputFormat::Lon),
            "lon is the constant default"
        );

        let args = parse_resolve(&[
            "resolve-at",
            "--format",
            "json",
            "--sdk",
            "/sdk",
            "lib/a.dart:2:6",
            "/pkg",
        ]);
        assert_eq!(args.dir, PathBuf::from("/pkg"));
        assert_eq!(args.sdk, Some(PathBuf::from("/sdk")));
        assert!(
            matches!(args.format, OutputFormat::Json),
            "--format json parses"
        );

        let args = parse_completion(&["completion", "lib/b.dart:3:3", "/pkg"]);
        assert_eq!(args.selector, "lib/b.dart:3:3");
        assert_eq!(args.dir, PathBuf::from("/pkg"));

        for rejected in [
            &["resolve-at"][..],
            &["resolve-at", "--bogus"][..],
            &["resolve-at", "--format", "machine", "a.dart:1:1"][..],
            &["completion"][..],
        ] {
            assert!(
                Cli::try_parse_from(std::iter::once("dartq").chain(rejected.iter().copied()))
                    .is_err(),
                "must reject {rejected:?}"
            );
        }
    }

    /// Decodes one lon line back into a value (the tests' reader — the
    /// same crate that encodes).
    fn read_lon(out: &[u8]) -> serde_json::Value {
        let text = std::str::from_utf8(out).expect("UTF-8");
        lon::from_str(text.trim_end()).expect("the lon line decodes")
    }

    /// End to end over the fixture: a declaration cursor answers the hit's
    /// locator, a resolved-use cursor the reference hit with the target's
    /// identity, and the json spelling is the daemon envelope shape (the
    /// same keys as the lon face).
    #[test]
    fn resolve_at_end_to_end() {
        let root = fixture("resolve");
        let args = PositionArgs {
            selector: "lib/b.dart:3:3".to_owned(),
            dir: root.clone(),
            sdk: Some(root.join("sdk")),
            format: OutputFormat::Lon,
        };
        let mut out: Vec<u8> = Vec::new();
        let code = resolve_at_run(&args, &mut out);
        assert_eq!(code, exit::CLEAN);
        let answer = read_lon(&out);
        let b = root.canonicalize().expect("root").join("lib/b.dart");
        let a = root.canonicalize().expect("root").join("lib/a.dart");
        // Line 3 is `  foo();` — the use of `foo` (offset 32, the use
        // site's span; the declaration's name token sits at offset 18 in
        // a.dart).
        assert_eq!(answer["v"], 1, "the envelope's vocabulary version key");
        assert_eq!(answer["file"], b.display().to_string());
        assert_eq!(answer["off"], 32, "offset → off");
        assert_eq!(answer["hit"]["kind"], "reference");
        assert_eq!(answer["hit"]["name"], "foo");
        assert_eq!(answer["hit"]["decl_kind"], "function");
        assert_eq!(answer["hit"]["range"]["off"], 32);
        assert_eq!(answer["hit"]["range"]["len"], 3);
        assert_eq!(
            answer["hit"]["file"],
            a.display().to_string(),
            "the hit's target columns point at the declaration file"
        );

        // The declaration cursor answers the declaration hit.
        let args = PositionArgs {
            selector: "lib/a.dart:1:7".to_owned(),
            dir: root.clone(),
            sdk: Some(root.join("sdk")),
            format: OutputFormat::Lon,
        };
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(resolve_at_run(&args, &mut out), exit::CLEAN);
        let answer = read_lon(&out);
        assert_eq!(answer["hit"]["kind"], "declaration");
        assert_eq!(answer["hit"]["name"], "Foo");
        assert_eq!(answer["hit"]["decl_kind"], "class");
        assert_eq!(answer["hit"]["range"]["off"], 6);
        assert_eq!(answer["hit"]["range"]["len"], 3);

        // The json spelling is the daemon envelope verbatim (`v`, `off`,
        // the hit's locator fields — the same keys the lon face prints).
        let args = PositionArgs {
            selector: "lib/a.dart:1:7".to_owned(),
            dir: root.clone(),
            sdk: Some(root.join("sdk")),
            format: OutputFormat::Json,
        };
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(resolve_at_run(&args, &mut out), exit::CLEAN);
        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(out).expect("UTF-8")).expect("one object");
        assert_eq!(json["v"], 1);
        assert_eq!(json["hit"]["kind"], "declaration");
        assert_eq!(json["hit"]["name"], "Foo");
        assert_eq!(json["hit"]["range"]["off"], 6);
        assert!(json["off"].is_number());

        fs::remove_dir_all(&root).ok();
    }

    /// End to end: the completion face answers the visible names at the
    /// cursor, ascending by name. The cursor `3:5` is UTF-16 offset 34 in
    /// the fixture — right after the `fo` fragment of `  foo();`.
    #[test]
    fn completion_end_to_end() {
        let root = fixture("complete");
        let args = PositionArgs {
            selector: "lib/b.dart:3:5".to_owned(),
            dir: root.clone(),
            sdk: Some(root.join("sdk")),
            format: OutputFormat::Lon,
        };
        let mut out: Vec<u8> = Vec::new();
        let code = completion_run(&args, &mut out);
        assert_eq!(code, exit::CLEAN);
        let answer = read_lon(&out);
        assert_eq!(answer["prefix"], "fo", "the fragment filters the set");
        let candidates: Vec<&str> = answer["items"]
            .as_array()
            .expect("items")
            .iter()
            .map(|row| row["name"].as_str().expect("name"))
            .collect();
        assert!(
            candidates.first() == Some(&"foo"),
            "the import-proven name answers: {candidates:?}"
        );
        assert!(
            candidates.windows(2).all(|pair| pair[0] <= pair[1]),
            "candidates ascend by name: {candidates:?}"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// The usage tier: a selector without the cursor position, a path
    /// outside the workspace, and an uncovered position's miss verdict
    /// (`hit` null in the lon document, exit 0).
    #[test]
    fn resolve_at_usage_tier_and_miss() {
        let root = fixture("usage");
        let run_with = |selector: &str| {
            let args = PositionArgs {
                selector: selector.to_owned(),
                dir: root.clone(),
                sdk: Some(root.join("sdk")),
                format: OutputFormat::Lon,
            };
            let mut out: Vec<u8> = Vec::new();
            (resolve_at_run(&args, &mut out), out)
        };

        let (code, out) = run_with("lib/b.dart");
        assert_eq!(code, exit::USAGE, "no cursor position is a usage error");
        assert!(out.is_empty(), "usage errors keep stdout empty");

        let (code, out) = run_with("lib/missing.dart:1:1");
        assert_eq!(
            code,
            exit::USAGE,
            "a file outside the analyzed view is usage"
        );
        assert!(out.is_empty());

        // A comment position is uncovered: exit 0, the null hit verdict.
        let (code, out) = run_with("lib/b.dart:1:1");
        assert_eq!(code, exit::CLEAN);
        assert_eq!(read_lon(&out)["hit"], serde_json::Value::Null);

        fs::remove_dir_all(&root).ok();
    }
}
