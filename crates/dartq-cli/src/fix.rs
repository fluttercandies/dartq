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
//! - `--format=machine` emits one single-line §15.6 JSON object per patched
//!   file — empty context strings omitted (§15.7); the default format is a
//!   human listing;
//! - exit tiers follow the query faces: 0 on success (zero fixes included;
//!   stale entries are safe no-ops, not failures), 64 usage, 4 internal.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use clap::{Args, ValueEnum};
use dartq_commands::fix::{self, EditOutcome, FileFixes};
use dartq_commands::discover::DiscoverError;

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
/// usage tier on a missing target), output, and — without `--dry-run` —
/// the in-place apply.
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

    let mut statuses: Vec<Vec<EditOutcome>> = Vec::new();
    if !args.dry_run {
        for fixes in &per_file {
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
            if let Err(error) = fs::write(&fixes.path, &patched) {
                eprintln!(
                    "dartq: internal error: writing {} failed: {error}",
                    fixes.path
                );
                return exit::INTERNAL_ERROR;
            }
            statuses.push(outcome);
        }
    }

    let written = match args.format {
        FixFormat::Machine => write_machine(out, &per_file),
        FixFormat::Default => write_human(out, &per_file, &statuses, args.dry_run),
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
    use clap::Parser as _;
    use crate::{Cli, Command};

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
}
