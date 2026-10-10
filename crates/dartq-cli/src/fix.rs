//! `dartq fix` — the structured-patch face (PRD §15.6).
//!
//! The anchoring substrate, the drift-correction applier, and the §15.6
//! patch document live in [`dartq_commands::fix`] (the one implementation
//! the daemon's `dartq/fix` shares); this binary adds argument parsing,
//! the in-place apply, human formatting, and the exit tiers:
//!
//! - `--dry-run` only outputs the patches, never writes (§15.6); without it
//!   the edits apply in place (the machine document still prints, as the
//!   record of what was written);
//! - applies collect-then-write: every file's patched buffer is computed in
//!   memory before any file changes on disk (a read or patch failure leaves
//!   nothing mutated), and a write failing midway prints which files were
//!   already mutated before the internal-error exit;
//! - `--format=machine` emits one single-line §15.6 JSON object per patched
//!   file — empty context strings omitted (§15.7); the default format is a
//!   human listing;
//! - exit tiers follow the query faces: 0 on success (zero fixes included;
//!   stale entries are safe no-ops, not failures), 64 usage, 4 internal.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use clap::{Args, ValueEnum};
use dartq_commands::discover::DiscoverError;
use dartq_commands::fix::{self, EditOutcome, FileFixes};

use crate::exit;

#[derive(Args)]
pub(crate) struct FixArgs {
    /// Only output the patches; never write files (PRD §15.6).
    #[arg(long)]
    pub(crate) dry_run: bool,
    #[arg(long, value_enum, default_value_t = FixFormat::Default)]
    pub(crate) format: FixFormat,
    /// Targets; empty resolves to the current directory.
    #[arg(default_value = ".")]
    pub(crate) paths: Vec<PathBuf>,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum FixFormat {
    /// Human listing (rule, location, applied/stale status).
    Default,
    /// One single-line §15.6 JSON object per patched file.
    Machine,
}

/// Machine format: one single-line §15.6 JSON object per patched file.
fn write_machine(out: &mut impl Write, per_file: &[FileFixes]) -> io::Result<()> {
    for fixes in per_file {
        if fixes.fixes.is_empty() {
            continue;
        }
        writeln!(out, "{}", fix::patch_json(fixes))?;
    }
    Ok(())
}

/// Human format: one row per fix (`file:line:column • rule • message`),
/// `[applied]`/`[stale]` tagged when the run writes, then the summary.
fn write_human(
    out: &mut impl Write,
    per_file: &[FileFixes],
    statuses: &[Vec<EditOutcome>],
    dry_run: bool,
) -> io::Result<()> {
    let mut fix_count = 0usize;
    let mut file_count = 0usize;
    let mut applied = 0usize;
    let mut stale = 0usize;
    for (index, fixes) in per_file.iter().enumerate() {
        if fixes.fixes.is_empty() {
            continue;
        }
        file_count += 1;
        let status = statuses.get(index).map_or(&[][..], Vec::as_slice);
        for (i, fix_row) in fixes.fixes.iter().enumerate() {
            fix_count += 1;
            let anchor = &fix_row.edit.anchor;
            write!(
                out,
                "{}:{}:{} • {} • {}",
                fixes.path, anchor.line, anchor.column, fix_row.rule, fix_row.message
            )?;
            match status.get(i) {
                Some(EditOutcome::Applied) => {
                    applied += 1;
                    writeln!(out, " [applied]")?;
                }
                Some(EditOutcome::Stale) => {
                    stale += 1;
                    writeln!(out, " [stale]")?;
                }
                None => writeln!(out)?,
            }
        }
    }
    match (dry_run, fix_count) {
        (_, 0) => writeln!(out, "No fixes found."),
        (true, 1) => writeln!(out, "1 proposed fix in {file_count} file."),
        (true, n) => writeln!(out, "{n} proposed fixes in {file_count} files."),
        (false, _) if stale == 0 && fix_count == 1 => {
            writeln!(out, "1 fix applied in {file_count} file.")
        }
        (false, _) if stale == 0 => {
            writeln!(out, "{fix_count} fixes applied in {file_count} files.")
        }
        (false, _) if applied == 1 && stale == 1 => {
            writeln!(out, "1 fix applied, 1 stale in {file_count} file.")
        }
        (false, _) => {
            writeln!(
                out,
                "{applied} applied, {stale} stale in {file_count} files."
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Entry
// ---------------------------------------------------------------------------

/// Crash tier like the other faces: a panic aborts with exit 4 and stdout
/// kept empty, because [`run`] writes only after the run succeeded.
pub(crate) fn run_entry(args: &FixArgs) -> u8 {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(args, &mut io::BufWriter::new(io::stdout().lock()))
    }));
    let Ok(code) = outcome else {
        eprintln!("dartq: internal error: analysis panicked");
        return exit::INTERNAL_ERROR;
    };
    code
}

/// One fix run: the fix source stage (`dartq_commands::fix::collect` —
/// usage tier on a missing target), then the apply + report stages.
fn run(args: &FixArgs, out: &mut impl Write) -> u8 {
    let per_file: Vec<FileFixes> = match fix::collect(&args.paths) {
        Ok(per_file) => per_file,
        Err(DiscoverError::Missing(path)) => {
            eprintln!("error: Directory or file doesn't exist: {path}");
            return exit::USAGE;
        }
        Err(DiscoverError::Io(error)) => {
            eprintln!("dartq: internal error: {error}");
            return exit::INTERNAL_ERROR;
        }
    };
    apply_and_report(&per_file, args.dry_run, args.format, out)
}

/// The apply + report stages over collected fix sets, collect-then-write:
/// every file's patched buffer is computed in memory first — a read or
/// patch failure here leaves nothing on disk mutated — and only then do
/// the writes run; one failing midway prints which files were already
/// mutated before the internal-error exit. The report follows: machine
/// rows, or the human listing carrying the per-fix outcomes.
fn apply_and_report(
    per_file: &[FileFixes],
    dry_run: bool,
    format: FixFormat,
    out: &mut impl Write,
) -> u8 {
    let mut staged: Vec<(&str, String, Vec<EditOutcome>)> = Vec::new();
    if !dry_run {
        for fixes in per_file {
            let text = match fs::read_to_string(&fixes.path) {
                Ok(text) => text,
                Err(error) => {
                    eprintln!(
                        "dartq: internal error: reading {} failed: {error}",
                        fixes.path
                    );
                    return exit::INTERNAL_ERROR;
                }
            };
            let edits: Vec<fix::Edit> = fixes.fixes.iter().map(|f| f.edit.clone()).collect();
            let (patched, outcome) = fix::apply(&text, &edits);
            staged.push((&fixes.path, patched, outcome));
        }
    }

    let mut statuses: Vec<Vec<EditOutcome>> = Vec::new();
    for (index, (path, patched, outcome)) in staged.iter().enumerate() {
        if let Err(error) = fs::write(path, patched) {
            eprintln!("dartq: internal error: writing {path} failed: {error}");
            let mutated: Vec<&str> = staged[..index].iter().map(|(path, _, _)| *path).collect();
            if !mutated.is_empty() {
                eprintln!(
                    "dartq: partial write; files already mutated: {}",
                    mutated.join(", ")
                );
            }
            return exit::INTERNAL_ERROR;
        }
        statuses.push(outcome.clone());
    }

    let written = match format {
        FixFormat::Machine => write_machine(out, per_file),
        FixFormat::Default => write_human(out, per_file, &statuses, dry_run),
    }
    .and_then(|()| out.flush());
    if let Err(error) = written {
        eprintln!("dartq: internal error: writing fix output failed: {error}");
        return exit::INTERNAL_ERROR;
    }
    exit::CLEAN
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cli, Command};
    use clap::Parser as _;
    use dartq_source::LineIndex;

    /// The CLI face: defaults and flags; the zero-fix run prints the human
    /// summary (machine face stays silent) and exits clean; a missing
    /// target is the usage tier.
    #[test]
    fn fix_cli_defaults_flags_and_zero_fix_run() {
        fn parse(args: &[&str]) -> FixArgs {
            match Cli::try_parse_from(std::iter::once("dartq").chain(args.iter().copied()))
                .expect("parse")
                .command
            {
                Some(Command::Fix(args)) => args,
                Some(_) => panic!("expected the fix command for {args:?}"),
                None => panic!("expected a subcommand"),
            }
        }

        let args = parse(&["fix"]);
        assert!(!args.dry_run, "apply is the default");
        assert!(matches!(args.format, FixFormat::Default));
        assert_eq!(args.paths, vec![PathBuf::from(".")], "default target cwd");

        let args = parse(&["fix", "--dry-run", "--format=machine", "/tmp/w"]);
        assert!(args.dry_run);
        assert!(matches!(args.format, FixFormat::Machine));
        assert_eq!(args.paths, vec![PathBuf::from("/tmp/w")]);

        for rejected in [
            &["fix", "--format"][..],
            &["fix", "--format=yaml"][..],
            &["fix", "--bogus"][..],
        ] {
            assert!(
                Cli::try_parse_from(std::iter::once("dartq").chain(rejected.iter().copied()))
                    .is_err(),
                "must reject {rejected:?}"
            );
        }

        let root = std::env::temp_dir().join(format!("dq_cli_fix_{}", std::process::id()));
        fs::create_dir_all(&root).expect("mkdir");
        fs::write(root.join("a.dart"), "var ok = 1;\n").expect("write");
        let target = root.to_str().expect("utf-8 temp path");

        let mut out = Vec::new();
        let args = parse(&["fix", "--dry-run", target]);
        assert_eq!(run(&args, &mut out), exit::CLEAN);
        assert_eq!(String::from_utf8(out).expect("utf-8"), "No fixes found.\n");

        let mut out = Vec::new();
        let args = parse(&["fix", "--format=machine", target]);
        assert_eq!(run(&args, &mut out), exit::CLEAN);
        assert!(out.is_empty(), "machine face prints nothing without fixes");

        let mut out = Vec::new();
        let args = parse(&["fix", "/nonexistent/dq_fix_target"]);
        assert_eq!(run(&args, &mut out), exit::USAGE);
        assert!(out.is_empty(), "stdout stays empty on usage errors");

        fs::remove_dir_all(&root).ok();
    }

    /// The `unnecessary_new` fix set addressed to `path` — the exact edit
    /// the rule materializes over this source in dartq-commands (keyword
    /// plus trailing space at offset 19..23, verified in its
    /// `new_expression_end_to_end` test). The CLI's collect stage has no
    /// row source wired yet (dartq-commands `collect` reports zero patch
    /// sets), so the write-back tests stage the collected set directly.
    fn unnecessary_new_fix(path: &std::path::Path) -> FileFixes {
        let source = "void f() { var a = new B(); }\n";
        let lines = LineIndex::new(source);
        FileFixes {
            path: path.display().to_string(),
            fixes: vec![fix::RuleFix {
                rule: "unnecessary_new",
                message: "Unnecessary 'new' keyword.".to_owned(),
                edit: fix::anchored_edit(source, &lines, 19, 4, ""),
            }],
        }
    }

    /// The destructive write-back end to end (the zero-fix paths above
    /// never touch disk): `--dry-run` proposes without writing; the apply
    /// run rewrites the file's bytes on disk to the patched text, tags the
    /// row `[applied]` with the rule named, and exits clean.
    #[test]
    fn fix_apply_rewrites_the_file_on_disk() {
        let root = std::env::temp_dir().join(format!("dq_cli_fix_apply_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("mkdir");
        let path = root.join("a.dart");
        fs::write(&path, "void f() { var a = new B(); }\n").expect("write");
        let per_file = vec![unnecessary_new_fix(&path)];

        let mut out = Vec::new();
        assert_eq!(
            apply_and_report(&per_file, true, FixFormat::Default, &mut out),
            exit::CLEAN
        );
        let text = String::from_utf8(out).expect("utf-8");
        assert!(text.contains("1 proposed fix in 1 file."), "{text}");
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "void f() { var a = new B(); }\n",
            "dry run never writes"
        );

        let mut out = Vec::new();
        assert_eq!(
            apply_and_report(&per_file, false, FixFormat::Default, &mut out),
            exit::CLEAN
        );
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "void f() { var a = B(); }\n",
            "the fix rewrote the file in place"
        );
        let text = String::from_utf8(out).expect("utf-8");
        let applied = text
            .lines()
            .find(|l| l.contains("[applied]"))
            .expect("applied row");
        assert!(
            applied.contains("unnecessary_new") && applied.contains("Unnecessary 'new' keyword."),
            "the applied row names the rule: {applied}"
        );
        assert!(text.contains("1 fix applied in 1 file."), "{text}");

        fs::remove_dir_all(&root).ok();
    }

    /// The collect-then-write contract's failure half: when a write fails
    /// midway, the earlier files stay mutated (the partial-write record on
    /// stderr names them), the failing write never lands, stdout stays
    /// empty, and the run exits internal. The second file reads fine but
    /// is read-only, so the failure lands at the write stage — the stage
    /// where earlier files may already be mutated.
    #[cfg(unix)]
    #[test]
    fn partial_write_failure_keeps_the_mutated_files_recorded() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("dq_cli_fix_partial_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("mkdir");
        let first = root.join("a.dart");
        fs::write(&first, "void f() { var a = new B(); }\n").expect("write");
        let second = root.join("b.dart");
        fs::write(&second, "void f() { var a = new B(); }\n").expect("write");
        fs::set_permissions(&second, fs::Permissions::from_mode(0o444)).expect("chmod");

        let per_file = vec![unnecessary_new_fix(&first), unnecessary_new_fix(&second)];
        let mut out = Vec::new();
        let code = apply_and_report(&per_file, false, FixFormat::Default, &mut out);
        // Restore the mode before asserting so cleanup cannot fail on it.
        fs::set_permissions(&second, fs::Permissions::from_mode(0o644)).expect("restore mode");
        assert_eq!(code, exit::INTERNAL_ERROR);
        assert_eq!(
            fs::read_to_string(&first).expect("read"),
            "void f() { var a = B(); }\n",
            "the earlier file stays mutated; the record on stderr names it"
        );
        assert_eq!(
            fs::read_to_string(&second).expect("read"),
            "void f() { var a = new B(); }\n",
            "the failing write never landed"
        );
        assert!(out.is_empty(), "stdout stays empty on internal errors");

        fs::remove_dir_all(&root).ok();
    }
}
