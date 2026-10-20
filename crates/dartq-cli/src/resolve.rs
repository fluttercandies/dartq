//! `dartq resolve-at` / `dartq completion` — the position faces' one-shot
//! CLI spelling: what the analyzed workspace's maintained index holds under
//! one cursor, and the visible-name set at it, without a resident session.
//!
//! The substrate and the answers live in [`dartq_commands::resolve`] — the
//! one implementation the daemon's `dartq/resolveAt` / `dartq/completion`
//! RPC answers share, so the faces cannot drift. This binary adds argument
//! parsing, the crash tier, the usage-tier mapping, and the two output
//! spellings:
//!
//! * `--format json` — the daemon answer envelope verbatim, one line.
//! * `--format machine` (the default) — one row per line with a stable
//!   field order, the query face's own row grammar posture (a locator
//!   listing, deliberately not the diagnostics 8-field face):
//!
//! ```text
//! resolve-at:
//!   DECL|file|container|decl-kind|name|offset|length
//!                                                   (the cursor sits on a
//!                                                   declaration's name
//!                                                   token; container `-`
//!                                                   when top-level)
//!   REF|file|container|decl-kind|name|offset|length|target-file|target-offset|target-length|ref-kind|qualified|prefix
//!                                                   (the cursor sits on a
//!                                                   resolved use; `file` is
//!                                                   the queried file, the
//!                                                   target columns locate
//!                                                   the declaration the use
//!                                                   resolves to; prefix `-`
//!                                                   when bare)
//!   MISS                                           (an uncovered position)
//!
//! completion:
//!   PREFIX|<prefix>                                (the fragment the cursor
//!                                                   completes; empty when
//!                                                   the cursor does not
//!                                                   follow identifier
//!                                                   characters)
//!   CAND|name|origin|decl-kind|container|file|offset|length
//!                                                   (one row per candidate,
//!                                                   ascending by name; `-`
//!                                                   when a column is absent
//!                                                   — a `prefix` item is a
//!                                                   wire word and carries
//!                                                   only name + origin)
//! ```
//!
//! The file columns are the canonical paths (the machine grammar's
//! identity spelling); fields are not `|`-escaped, the same posture as the
//! query rows. The answers are tier-invariant (every field a locator or a
//! verdict), so there is no `--verbosity` axis.

use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use clap::Args;
use serde_json::Value;

use dartq_commands::resolve::resolve_at as resolve_at_core;
use dartq_commands::resolve::{ResolveError, completion as completion_core};
use dartq_commands::workspace::open_engine;

use crate::QueryFormat;
use crate::exit;

/// The two position faces' shared flags — one shape, two subcommands
/// (`resolve-at` and `completion` differ only in the answer they ask for).
#[derive(Args)]
pub(crate) struct PositionArgs {
    /// The `PATH:LINE:COL` cursor (1-based; the column counts UTF-16 code
    /// units). PATH resolves against DIR and must be an analyzed `.dart`
    /// file of the workspace; an uncovered position — a non-identifier, an
    /// unresolved use, a past-end offset — answers `MISS` / the empty
    /// candidate set, never an error.
    pub(crate) selector: String,
    /// Package directory holding pubspec.yaml and
    /// `.dart_tool/package_config.json`; defaults to the current directory.
    #[arg(default_value = ".")]
    pub(crate) dir: PathBuf,
    /// Output spelling: `machine` rows (the default) or the single-line
    /// JSON envelope — the same answer the daemon's `dartq/resolveAt` /
    /// `dartq/completion` serves.
    #[arg(long, value_enum, default_value_t = QueryFormat::Machine)]
    pub(crate) format: QueryFormat,
    /// Dart SDK root; defaults to `$DART_SDK`.
    #[arg(long)]
    pub(crate) sdk: Option<PathBuf>,
}

/// Crash tier like the other engine faces: a panic aborts with exit 4 and
/// stdout kept empty, because the runners write only after the query fully
/// succeeded.
pub(crate) fn resolve_at_entry(args: &PositionArgs) -> u8 {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        resolve_at_run(args, &mut BufWriter::new(io::stdout().lock()))
    }));
    let Ok(code) = outcome else {
        eprintln!("dartq: internal error: analysis panicked");
        return exit::INTERNAL_ERROR;
    };
    code
}

/// Crash tier like [`resolve_at_entry`].
pub(crate) fn completion_entry(args: &PositionArgs) -> u8 {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        completion_run(args, &mut BufWriter::new(io::stdout().lock()))
    }));
    let Ok(code) = outcome else {
        eprintln!("dartq: internal error: analysis panicked");
        return exit::INTERNAL_ERROR;
    };
    code
}

fn resolve_at_run(args: &PositionArgs, out: &mut impl Write) -> u8 {
    let (mut engine, dir) = match open_engine(&args.dir, args.sdk.as_deref()) {
        Ok(faces) => faces,
        Err(error) => {
            eprintln!("error: {error}");
            return exit::USAGE;
        }
    };
    let (file, line, column) = match selector_position(&args.selector, &dir) {
        Ok(position) => position,
        Err(message) => {
            eprintln!("error: {message}");
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

fn completion_run(args: &PositionArgs, out: &mut impl Write) -> u8 {
    let (mut engine, dir) = match open_engine(&args.dir, args.sdk.as_deref()) {
        Ok(faces) => faces,
        Err(error) => {
            eprintln!("error: {error}");
            return exit::USAGE;
        }
    };
    let (file, line, column) = match selector_position(&args.selector, &dir) {
        Ok(position) => position,
        Err(message) => {
            eprintln!("error: {message}");
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

fn write_answer(out: &mut impl Write, answer: &Value, format: QueryFormat, face: &str) -> u8 {
    let written = match format {
        QueryFormat::Machine => out
            .write_all(machine_rows(answer).as_bytes())
            .and_then(|()| out.flush()),
        QueryFormat::Json => out
            .write_all(answer.to_string().as_bytes())
            .and_then(|()| out.write_all(b"\n"))
            .and_then(|()| out.flush()),
    };
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

/// One machine column: the JSON field's verbatim spelling, `-` when the
/// key drops (the omit-on-absent columns — container, prefix, and the
/// prefix items' identity columns).
fn column(row: &Value, key: &str) -> String {
    match row.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::Bool(flag)) => flag.to_string(),
        _ => "-".to_owned(),
    }
}

/// A span's `(offset, length)` machine columns, `-` `-` when the span is
/// absent.
fn span_columns(row: &Value, key: &str) -> (String, String) {
    match row.get(key) {
        Some(span @ Value::Object(_)) => (column(span, "offset"), column(span, "length")),
        _ => ("-".to_owned(), "-".to_owned()),
    }
}

/// The machine grammar's rows over the answer envelope (the field names
/// are the same frozen wire contract the JSON face prints). The reference
/// hit's `file` field is the target declaration's file — the queried file
/// (the use site's own) is the envelope's top-level `file`.
fn machine_rows(answer: &Value) -> String {
    if answer.get("hit").is_some() {
        resolve_at_rows(answer)
    } else {
        completion_rows(answer)
    }
}

fn resolve_at_rows(answer: &Value) -> String {
    let hit = &answer["hit"];
    if hit.is_null() {
        return "MISS\n".to_owned();
    }
    let (offset, length) = span_columns(hit, "range");
    match hit["kind"].as_str() {
        Some("declaration") => format!(
            "DECL|{}|{}|{}|{}|{offset}|{length}\n",
            column(hit, "file"),
            column(hit, "container"),
            column(hit, "decl_kind"),
            column(hit, "name"),
        ),
        Some(_) => {
            let (target_offset, target_length) = span_columns(hit, "target_range");
            format!(
                "REF|{}|{}|{}|{}|{offset}|{length}|{}|{target_offset}|{target_length}|{}|{}|{}\n",
                column(answer, "file"),
                column(hit, "container"),
                column(hit, "decl_kind"),
                column(hit, "name"),
                column(hit, "file"),
                column(hit, "ref_kind"),
                column(hit, "qualified"),
                column(hit, "prefix"),
            )
        }
        None => "MISS\n".to_owned(),
    }
}

fn completion_rows(answer: &Value) -> String {
    let mut rows = format!("PREFIX|{}\n", column(answer, "prefix"));
    for item in answer["items"].as_array().into_iter().flatten() {
        let (offset, length) = span_columns(item, "range");
        rows.push_str(&format!(
            "CAND|{}|{}|{}|{}|{}|{offset}|{length}\n",
            column(item, "name"),
            column(item, "origin"),
            column(item, "decl_kind"),
            column(item, "container"),
            column(item, "file"),
        ));
    }
    rows
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
    /// spelling, `--format`/`--sdk` options.
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
            matches!(args.format, QueryFormat::Machine),
            "machine is the default"
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
            matches!(args.format, QueryFormat::Json),
            "--format json parses"
        );

        let args = parse_completion(&["completion", "lib/b.dart:3:3", "/pkg"]);
        assert_eq!(args.selector, "lib/b.dart:3:3");
        assert_eq!(args.dir, PathBuf::from("/pkg"));

        for rejected in [
            &["resolve-at"][..],
            &["resolve-at", "--bogus"][..],
            &["completion"][..],
        ] {
            assert!(
                Cli::try_parse_from(std::iter::once("dartq").chain(rejected.iter().copied()))
                    .is_err(),
                "must reject {rejected:?}"
            );
        }
    }

    /// End to end over the fixture: a declaration cursor answers the DECL
    /// row, a resolved-use cursor the REF row with the target's identity
    /// and name span, and the JSON spelling is the daemon envelope shape.
    #[test]
    fn resolve_at_end_to_end() {
        let root = fixture("resolve");
        let args = PositionArgs {
            selector: "lib/b.dart:3:3".to_owned(),
            dir: root.clone(),
            sdk: Some(root.join("sdk")),
            format: QueryFormat::Machine,
        };
        let mut out: Vec<u8> = Vec::new();
        let code = resolve_at_run(&args, &mut out);
        assert_eq!(code, exit::CLEAN);
        let rows = String::from_utf8(out).expect("UTF-8");
        // Line 3 is `  foo();` — the use of `foo` (offset 32, the use
        // site's span; the declaration's name token sits at offset 18 in
        // a.dart).
        assert_eq!(
            rows,
            format!(
                "REF|{}|-|function|foo|32|3|{}|18|3|invoked|false|-\n",
                root.canonicalize()
                    .expect("root")
                    .join("lib/b.dart")
                    .display(),
                root.canonicalize()
                    .expect("root")
                    .join("lib/a.dart")
                    .display(),
            ),
            "the REF row carries the use site, the target identity, and the relation"
        );

        // The declaration cursor answers the DECL row.
        let args = PositionArgs {
            selector: "lib/a.dart:1:7".to_owned(),
            dir: root.clone(),
            sdk: Some(root.join("sdk")),
            format: QueryFormat::Machine,
        };
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(resolve_at_run(&args, &mut out), exit::CLEAN);
        assert_eq!(
            String::from_utf8(out).expect("UTF-8"),
            format!(
                "DECL|{}|-|class|Foo|6|3\n",
                root.canonicalize()
                    .expect("root")
                    .join("lib/a.dart")
                    .display(),
            ),
        );

        // The JSON spelling is the daemon envelope (schema_version, file,
        // offset, hit).
        let args = PositionArgs {
            selector: "lib/a.dart:1:7".to_owned(),
            dir: root.clone(),
            sdk: Some(root.join("sdk")),
            format: QueryFormat::Json,
        };
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(resolve_at_run(&args, &mut out), exit::CLEAN);
        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(out).expect("UTF-8")).expect("one object");
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["hit"]["kind"], "declaration");
        assert_eq!(json["hit"]["name"], "Foo");
        assert_eq!(json["hit"]["range"]["offset"], 6);
        assert!(json["offset"].is_number());

        fs::remove_dir_all(&root).ok();
    }

    /// End to end: the completion face answers the visible names at the
    /// cursor — the PREFIX row plus the candidate rows, ascending by name.
    /// The cursor `3:5` is UTF-16 offset 34 in the fixture — right after
    /// the `fo` fragment of `  foo();`.
    #[test]
    fn completion_end_to_end() {
        let root = fixture("complete");
        let args = PositionArgs {
            selector: "lib/b.dart:3:5".to_owned(),
            dir: root.clone(),
            sdk: Some(root.join("sdk")),
            format: QueryFormat::Machine,
        };
        let mut out: Vec<u8> = Vec::new();
        let code = completion_run(&args, &mut out);
        assert_eq!(code, exit::CLEAN);
        let rows = String::from_utf8(out).expect("UTF-8");
        let mut lines = rows.lines();
        assert_eq!(
            lines.next(),
            Some("PREFIX|fo"),
            "the fragment filters the set"
        );
        let candidates: Vec<&str> = lines
            .map(|row| row.split('|').nth(1).expect("name column"))
            .collect();
        assert_eq!(
            candidates.first(),
            Some(&"foo"),
            "the import-proven name answers"
        );
        assert!(
            candidates.windows(2).all(|pair| pair[0] <= pair[1]),
            "candidates ascend by name: {candidates:?}"
        );

        // The JSON spelling is the daemon envelope (schema_version, file,
        // offset, prefix, items).
        let args = PositionArgs {
            selector: "lib/b.dart:3:5".to_owned(),
            dir: root.clone(),
            sdk: Some(root.join("sdk")),
            format: QueryFormat::Json,
        };
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(completion_run(&args, &mut out), exit::CLEAN);
        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(out).expect("UTF-8")).expect("one object");
        assert_eq!(json["prefix"], "fo");
        assert!(
            json["items"]
                .as_array()
                .expect("items")
                .iter()
                .any(|row| row["name"] == "foo")
        );

        fs::remove_dir_all(&root).ok();
    }

    /// The usage tier: a selector without the cursor position, a path
    /// outside the workspace, and an uncovered position's MISS verdict.
    #[test]
    fn resolve_at_usage_tier_and_miss() {
        let root = fixture("usage");
        let run_with = |selector: &str| {
            let args = PositionArgs {
                selector: selector.to_owned(),
                dir: root.clone(),
                sdk: Some(root.join("sdk")),
                format: QueryFormat::Machine,
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

        // A comment position is uncovered: exit 0 with the MISS verdict.
        let (code, out) = run_with("lib/b.dart:1:1");
        assert_eq!(code, exit::CLEAN);
        assert_eq!(String::from_utf8(out).expect("UTF-8"), "MISS\n");

        fs::remove_dir_all(&root).ok();
    }
}
