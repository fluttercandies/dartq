//! `dartq rename` — the agent-facing rename face: one anchor position,
//! one new identifier, the whole edit set.
//!
//! The plan (anchor resolution, the reference join, the edit rows), the
//! new-name gate, and the drift-checked patcher live in
//! [`dartq_commands::rename`] (the one implementation the daemon RPC face
//! shares); this binary adds the anchor-spec parsing, the workspace
//! boundary gate, the in-place apply, and the exit tiers:
//!
//! - the anchor is `PATH:LINE:COL` (1-based line, 1-based UTF-16 code-unit
//!   column — the diagnostics position convention every other face
//!   prints) or `PATH:OFFSET` (0-based byte offset);
//! - the default is a dry run: the machine document prints, nothing
//!   writes; `--write` applies the edit set in place — collect-then-write
//!   (every file's patched buffer computed before any file changes), only
//!   files inside the analyzed workspace directory, a drifted file
//!   refusing the whole run — and then prints the same document as the
//!   record of what was written;
//! - output is one single-line JSON document on stdout (the edit rows in
//!   path-then-offset order; a position miss is `target: null` with an
//!   empty edit set, still exit 0);
//! - exit tiers follow the query faces: 0 on success (a miss included),
//!   64 usage (malformed spec, unknown file, illegal new name, an edit
//!   outside the workspace), 4 internal.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use clap::Args;
use dartq_commands::rename::{self, Anchor, RenameError, RenameRun};
use dartq_commands::workspace::open_engine;
use dartq_diagnostics::FileLayoutProvider as _;

use crate::exit;

#[derive(Args)]
pub(crate) struct RenameArgs {
    /// The rename anchor: `PATH:LINE:COL` (1-based; the column counts
    /// UTF-16 code units) or `PATH:OFFSET` (0-based byte offset).
    pub(crate) target: String,
    /// The new identifier: a legal Dart identifier, not a reserved word.
    #[arg(long = "new-name", value_name = "IDENT")]
    pub(crate) new_name: String,
    /// Apply the edit set to disk — only files inside the analyzed
    /// workspace directory. The default prints the edit set without
    /// writing.
    #[arg(long)]
    pub(crate) write: bool,
    /// Package directory holding pubspec.yaml and
    /// `.dart_tool/package_config.json`; defaults to the current directory.
    #[arg(default_value = ".")]
    pub(crate) dir: PathBuf,
    /// Dart SDK root; defaults to `$DART_SDK`.
    #[arg(long)]
    pub(crate) sdk: Option<PathBuf>,
}

/// Splits the anchor spec into the file path plus the position: a
/// trailing `:LINE:COL` numeric pair is the line/column form, a single
/// trailing numeric segment the byte-offset form, and anything else is
/// malformed (the position is required).
fn parse_target(spec: &str) -> Result<(PathBuf, Anchor), String> {
    let malformed = || format!("expected PATH:LINE:COL or PATH:OFFSET, got `{spec}`");
    let Some((path, last)) = spec.rsplit_once(':') else {
        return Err(malformed());
    };
    if let Some((head, column_text)) = path.rsplit_once(':')
        && !head.is_empty()
        && let (Ok(line), Ok(column)) = (column_text.parse::<u32>(), last.parse::<u32>())
    {
        return Ok((PathBuf::from(head), Anchor::LineCol { line, column }));
    }
    if let Ok(offset) = last.parse::<u64>() {
        if path.is_empty() {
            return Err(malformed());
        }
        return Ok((PathBuf::from(path), Anchor::ByteOffset(offset)));
    }
    Err(malformed())
}

/// Crash tier like the other faces: a panic aborts with exit 4 and stdout
/// kept empty, because [`run`] writes only after the run succeeded.
pub(crate) fn run_entry(args: &RenameArgs) -> u8 {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(args, &mut io::BufWriter::new(io::stdout().lock()))
    }));
    if let Ok(code) = outcome {
        code
    } else {
        eprintln!("dartq: internal error: analysis panicked");
        exit::INTERNAL_ERROR
    }
}

/// One rename run: the anchor spec decodes and canonicalizes here (the
/// engine interns canonical spellings, so a raw relative spelling would
/// address a phantom file), the workspace assembly and the plan are the
/// command core's, and `--write` gates, patches, and writes before the
/// document prints.
fn run(args: &RenameArgs, out: &mut impl Write) -> u8 {
    let (path, anchor) = match parse_target(&args.target) {
        Ok(pair) => pair,
        Err(message) => {
            eprintln!("error: {message}");
            return exit::USAGE;
        }
    };
    let path = match path.canonicalize() {
        Ok(path) => path,
        Err(_) => {
            eprintln!("error: Directory or file doesn't exist: {}", args.target);
            return exit::USAGE;
        }
    };
    let (mut engine, dir) = match open_engine(&args.dir, args.sdk.as_deref()) {
        Ok(faces) => faces,
        Err(error) => {
            eprintln!("error: {error}");
            return exit::USAGE;
        }
    };
    let plan = match rename::rename(&mut engine, &path, anchor, &args.new_name) {
        Ok(plan) => plan,
        Err(
            error @ (RenameError::IllegalName(_)
            | RenameError::NotResident(_)
            | RenameError::OutOfRange { .. }),
        ) => {
            eprintln!("error: {error}");
            return exit::USAGE;
        }
        // The internal tier: drift (Stale) and analysis failures — the
        // run is well-formed but the machinery refused.
        Err(error) => {
            eprintln!("dartq: internal error: {error}");
            return exit::INTERNAL_ERROR;
        }
    };

    if args.write {
        if let Err(code) = apply(&plan, &args.new_name, &dir) {
            return code;
        }
    }

    let document = rename::to_json(&plan, &args.new_name);
    let written = writeln!(out, "{document}").and_then(|()| out.flush());
    if let Err(error) = written {
        eprintln!("dartq: internal error: writing the rename document failed: {error}");
        return exit::INTERNAL_ERROR;
    }
    exit::CLEAN
}

/// The `--write` stages: the workspace boundary gate first (only the
/// analyzed workspace's own files are writable — an edit row outside it,
/// such as a resolution into the SDK, refuses the run before anything is
/// read), then collect-then-write (every patched buffer computed before
/// any file changes; a write failing midway names the files already
/// mutated).
fn apply(plan: &RenameRun<'_>, new_name: &str, workspace: &std::path::Path) -> Result<(), u8> {
    if let Some(path) = rename::outside_workspace(
        plan.edits
            .iter()
            .map(|edit| plan.layout.file_path(edit.file)),
        workspace,
    ) {
        eprintln!(
            "error: refusing to write outside the workspace {}: {}",
            workspace.display(),
            path.display()
        );
        return Err(exit::USAGE);
    }
    let mut staged: Vec<(PathBuf, String)> = Vec::new();
    // The rows sort path-major, so one file's edits are contiguous.
    for group in plan.edits.chunk_by(|a, b| a.file == b.file) {
        let path = plan.layout.file_path(group[0].file).to_owned();
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                eprintln!(
                    "dartq: internal error: reading {} failed: {error}",
                    path.display()
                );
                return Err(exit::INTERNAL_ERROR);
            }
        };
        let patched = match rename::patched_text(plan, group[0].file, &text, new_name) {
            Ok(patched) => patched,
            Err(error) => {
                eprintln!("dartq: internal error: {error}");
                return Err(exit::INTERNAL_ERROR);
            }
        };
        staged.push((path, patched));
    }
    for (index, (path, patched)) in staged.iter().enumerate() {
        if let Err(error) = fs::write(path, patched) {
            eprintln!(
                "dartq: internal error: writing {} failed: {error}",
                path.display()
            );
            let mutated: Vec<&str> = staged[..index]
                .iter()
                .map(|(path, _)| path.to_str().unwrap_or("-"))
                .collect();
            if !mutated.is_empty() {
                eprintln!(
                    "dartq: partial write; files already mutated: {}",
                    mutated.join(", ")
                );
            }
            return Err(exit::INTERNAL_ERROR);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cli, Command};
    use clap::Parser as _;

    /// The anchor-spec grammar: both position forms, a path holding a
    /// colon ahead of the offset tail, and the malformed shapes.
    #[test]
    fn parse_target_splits_the_spec_forms() {
        let (path, anchor) = parse_target("lib/a.dart:3:7").expect("line:col");
        assert_eq!(path, PathBuf::from("lib/a.dart"));
        assert_eq!(anchor, Anchor::LineCol { line: 3, column: 7 });

        let (path, anchor) = parse_target("lib/a.dart:58").expect("offset");
        assert_eq!(path, PathBuf::from("lib/a.dart"));
        assert_eq!(anchor, Anchor::ByteOffset(58));

        let (path, anchor) = parse_target("we:ird/a.dart:58").expect("colon path, offset form");
        assert_eq!(path, PathBuf::from("we:ird/a.dart"));
        assert_eq!(anchor, Anchor::ByteOffset(58));

        for malformed in ["lib/a.dart", "lib/a.dart:x", "lib/a.dart:1:x", ":58", ""] {
            assert!(
                parse_target(malformed).is_err(),
                "must reject `{malformed}`"
            );
        }
    }

    /// The throwaway pub workspace + synthetic dart:core-only SDK the CLI
    /// rename tests analyze (the command tests' fixture shape, inlined —
    /// the fixture module is crate-private to dartq-commands).
    fn workspace(tag: &str, libraries: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("dq_cli_rename_{tag}_{}", std::process::id()));
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
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "languageVersion": "3.5"}], "generator": "pub", "generatorVersion": "3.13.1"}"#,
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
        for (relative, content) in libraries {
            write(relative, content);
        }
        root
    }

    /// Parses `dartq rename` args over the fixture root — the root rides
    /// the trailing `dir` positional (the default `.` would analyze the
    /// test process's working directory, never the fixture).
    fn args_of(root: &std::path::Path, extra: &[&str]) -> RenameArgs {
        let argv: Vec<String> = std::iter::once("dartq")
            .chain(std::iter::once("rename"))
            .chain(extra.iter().copied())
            .map(String::from)
            .chain(std::iter::once(
                root.to_str().expect("utf-8 root").to_owned(),
            ))
            .collect();
        match Cli::try_parse_from(argv).expect("parse").command {
            Some(Command::Rename(args)) => args,
            _ => panic!("expected the rename command"),
        }
    }

    /// The end-to-end face: the dry run prints the cross-file edit set
    /// (path-then-offset rows, the new name echoed) and writes nothing;
    /// `--write` renames both files in place and prints the same
    /// document.
    #[test]
    fn rename_cli_dry_run_prints_and_write_rewrites() {
        let a = "void fa() {}\n";
        let b = "import 'a.dart';\n\nvoid caller() {\n  fa();\n}\n";
        let root = workspace("roundtrip", &[("lib/a.dart", a), ("lib/b.dart", b)]);
        // The edit rows carry the canonical paths (the file table's
        // spellings), so the assertions address the same identity.
        let root = root.canonicalize().expect("canonical root");
        let target = root.join("lib/a.dart");
        let target_spec = format!("{}:1:6", target.display());
        let sdk = root.join("sdk").display().to_string();

        let args = args_of(&root, &[&target_spec, "--new-name", "gb", "--sdk", &sdk]);
        assert!(!args.write, "dry run is the default");

        let mut out = Vec::new();
        assert_eq!(run(&args, &mut out), exit::CLEAN);
        let document = String::from_utf8(out).expect("utf-8");
        assert!(document.contains("\"new_name\":\"gb\""), "{document}");
        assert!(document.contains("\"name\":\"fa\""), "{document}");
        assert!(document.contains("\"edits\":[{"), "{document}");
        assert!(
            document.contains(target.display().to_string().as_str())
                && document.contains("lib/b.dart"),
            "both files carry edit rows: {document}"
        );
        assert_eq!(
            fs::read_to_string(&target).expect("read"),
            a,
            "the dry run wrote nothing"
        );

        let args = args_of(
            &root,
            &[&target_spec, "--new-name", "gb", "--sdk", &sdk, "--write"],
        );
        let mut out = Vec::new();
        assert_eq!(run(&args, &mut out), exit::CLEAN);
        assert_eq!(
            fs::read_to_string(&target).expect("read"),
            "void gb() {}\n",
            "the declaration renamed in place"
        );
        assert_eq!(
            fs::read_to_string(root.join("lib/b.dart")).expect("read"),
            "import 'a.dart';\n\nvoid caller() {\n  gb();\n}\n",
            "the cross-file use renamed in place"
        );
        assert!(
            String::from_utf8(out)
                .expect("utf-8")
                .contains("\"new_name\":\"gb\""),
            "the document prints as the record of the write"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// The usage tier: a malformed spec, a missing file, and an illegal
    /// new name report on stderr with stdout kept empty.
    #[test]
    fn rename_cli_usage_refusals() {
        let root = workspace("usage", &[("lib/a.dart", "void fa() {}\n")]);
        let sdk = root.join("sdk").display().to_string();
        let run_with = |extra: &[&str]| {
            let args = args_of(&root, extra);
            let mut out = Vec::new();
            let code = run(&args, &mut out);
            (code, out)
        };

        let (code, out) = run_with(&["bogus", "--new-name", "gb", "--sdk", &sdk]);
        assert_eq!(code, exit::USAGE);
        assert!(out.is_empty(), "stdout stays empty on usage errors");

        let missing = format!("{}:1:1", root.join("lib/missing.dart").display());
        let (code, out) = run_with(&[missing.as_str(), "--new-name", "gb", "--sdk", &sdk]);
        assert_eq!(code, exit::USAGE);
        assert!(out.is_empty());

        let target = format!("{}:1:6", root.join("lib/a.dart").display());
        let (code, out) = run_with(&[target.as_str(), "--new-name", "class", "--sdk", &sdk]);
        assert_eq!(code, exit::USAGE);
        assert!(out.is_empty());

        // A file that exists on disk but is not part of the analyzed
        // workspace is refused at the anchor gate — the file table never
        // interned it.
        let stranger = std::env::temp_dir()
            .join(format!("dq_cli_rename_stranger_{}", std::process::id()).as_str());
        fs::write(&stranger, "void fa() {}\n").expect("stranger file");
        let target = format!("{}:1:6", stranger.display());
        let (code, out) = run_with(&[target.as_str(), "--new-name", "gb", "--sdk", &sdk]);
        assert_eq!(code, exit::USAGE);
        assert!(out.is_empty());
        fs::remove_file(&stranger).ok();

        fs::remove_dir_all(&root).ok();
    }
}
