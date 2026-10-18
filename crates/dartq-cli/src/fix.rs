//! `dartq fix` / `dartq assists` — the structured-patch faces.
//!
//! The anchoring substrate, the drift-correction applier, the patch
//! document, and the plugin dispatch live in [`dartq_commands::fix`] (the
//! one implementation the daemon's `dartq/fix` and `dartq/assists` share);
//! this binary adds argument parsing, the in-place apply, human
//! formatting, and the exit tiers:
//!
//! - `--dry-run` only outputs the patches, never writes; without it
//!   the edits apply in place (the machine document still prints, as the
//!   record of what was written);
//! - both faces dispatch the discovered plugin hosts (the `plugins/`
//!   convention and the pub-native `plugins:` section under the working
//!   directory): built-in lint rows, the
//!   semantic engine pass's rows, and the plugins' `request/fixes`
//!   proposals, every row provenance-tagged (`builtin-lint`,
//!   `builtin-semantic`, or the plugin's manifest name); a plugin's
//!   terminal failure surfaces on stderr and in the machine document's
//!   `errors`;
//! - applies collect-then-write: every file's patched buffer is computed in
//!   memory before any file changes on disk (a read or patch failure leaves
//!   nothing mutated), and a write failing midway prints which files were
//!   already mutated before the internal-error exit;
//! - `--format=machine` emits one single-line JSON document per patched
//!   file — empty context strings omitted; an apply run records each
//!   row's outcome (`applied`/`stale`) plus the document-level summary,
//!   so an all-stale run (a safe no-op, still exit 0) stays legible to
//!   the machine consumer; the default format is a human listing;
//! - exit tiers follow the query faces: 0 on success (zero fixes included;
//!   stale entries are safe no-ops, not failures), 64 usage, 4 internal.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::{Args, ValueEnum};
use dartq_commands::Verbosity;
use dartq_commands::discover::DiscoverError;
use dartq_commands::fix::{self, EditOutcome, FileFixSet, PluginHosts};
use dartq_pub::PackageConfig;
use dartq_source::{MAX_SOURCE_BYTES, OpenError};

use crate::exit;

#[derive(Args)]
pub(crate) struct FixArgs {
    /// Only output the patches; never write files.
    #[arg(long)]
    pub(crate) dry_run: bool,
    #[arg(long, value_enum, default_value_t = FixFormat::Default)]
    pub(crate) format: FixFormat,
    /// Targets; empty resolves to the current directory.
    #[arg(default_value = ".")]
    pub(crate) paths: Vec<PathBuf>,
}

#[derive(Args)]
pub(crate) struct AssistsArgs {
    /// The file to query plugin assists for.
    pub(crate) path: PathBuf,
    /// The UTF-16 offset the assists target (default: file start).
    #[arg(long, default_value_t = 0)]
    pub(crate) offset: u64,
    /// The UTF-16 length of the target span (default: zero).
    #[arg(long, default_value_t = 0)]
    pub(crate) length: u64,
    #[arg(long, value_enum, default_value_t = FixFormat::Default)]
    pub(crate) format: FixFormat,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum FixFormat {
    /// Human listing (rule, location, applied/stale status).
    Default,
    /// One single-line JSON document per patched file; an apply run adds
    /// each row's `outcome` (`applied`/`stale`) and the document `summary`
    /// (a dry run emits bare proposals).
    Machine,
}

/// Machine format: one single-line patch document per file with rows —
/// the proposal shape on a dry run, the apply record (one `outcome` per
/// row plus the `summary`) once the run writes, the statuses the apply
/// stage reported in row order.
fn write_machine(
    out: &mut impl Write,
    per_file: &[FileFixSet],
    statuses: &[Vec<EditOutcome>],
    dry_run: bool,
) -> io::Result<()> {
    for (index, set) in per_file.iter().enumerate() {
        if set.fixes.is_empty() && set.plugin.is_empty() {
            continue;
        }
        let document = match (dry_run, statuses.get(index)) {
            (false, Some(outcomes)) => fix::patch_json_applied(set, Verbosity::Compact, outcomes),
            _ => fix::patch_json(set, Verbosity::Compact),
        };
        writeln!(out, "{document}")?;
    }
    Ok(())
}

/// One file's edits in report order: the built-in rows first, then each
/// plugin change's edits flattened (the same order the patch document and
/// the human listing use, and the order [`fix::apply`]'s outcomes report).
/// The cross-file rows ([`fix::RuleFix::target`]) never ride the hit
/// file's patch set — their edit addresses another file and stages whole.
fn edits_of(set: &FileFixSet) -> Vec<fix::Edit> {
    set.fixes
        .iter()
        .filter(|f| f.target.is_none())
        .map(|f| f.edit.clone())
        .chain(set.plugin.iter().flat_map(|p| p.edits.iter().cloned()))
        .collect()
}

/// Human format: one row per edit (`file:line:column • source • message`),
/// `[applied]`/`[stale]` tagged when the run writes, then the summary.
/// Built-in rows name their rule; plugin rows name the plugin and the
/// change title.
fn write_human(
    out: &mut impl Write,
    per_file: &[FileFixSet],
    statuses: &[Vec<EditOutcome>],
    dry_run: bool,
) -> io::Result<()> {
    let mut fix_count = 0usize;
    let mut file_count = 0usize;
    let mut applied = 0usize;
    let mut stale = 0usize;
    for (index, set) in per_file.iter().enumerate() {
        if set.fixes.is_empty() && set.plugin.is_empty() {
            continue;
        }
        file_count += 1;
        let status = statuses.get(index).map_or(&[][..], Vec::as_slice);
        let mut consumed = 0usize;
        for fix_row in &set.fixes {
            fix_count += 1;
            // The cross-file row names the file it creates, at the head
            // the content lands at — never the hit file's coordinates.
            let (file, line, column) = match &fix_row.target {
                Some(target) => (target.as_str(), fix_row.edit.anchor.line, fix_row.edit.anchor.column),
                None => (
                    set.path.as_str(),
                    fix_row.edit.anchor.line,
                    fix_row.edit.anchor.column,
                ),
            };
            write!(out, "{file}:{line}:{column} • {} • {}", fix_row.rule, fix_row.message)?;
            consumed += write_status(out, status.get(consumed), &mut applied, &mut stale)?;
        }
        for plugin_row in &set.plugin {
            for edit in &plugin_row.edits {
                fix_count += 1;
                write!(
                    out,
                    "{}:{}:{} • {} • {}",
                    set.path,
                    edit.anchor.line,
                    edit.anchor.column,
                    plugin_row.plugin,
                    plugin_row.message
                )?;
                consumed += write_status(out, status.get(consumed), &mut applied, &mut stale)?;
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

/// Writes one row's status tag; returns 1 (the status slot consumed).
fn write_status(
    out: &mut impl Write,
    status: Option<&EditOutcome>,
    applied: &mut usize,
    stale: &mut usize,
) -> io::Result<usize> {
    match status {
        Some(EditOutcome::Applied) => {
            *applied += 1;
            writeln!(out, " [applied]")?;
        }
        Some(EditOutcome::Stale) => {
            *stale += 1;
            writeln!(out, " [stale]")?;
        }
        None => writeln!(out)?,
    }
    Ok(1)
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

/// Crash tier for the assists face (same contract as [`run_entry`]).
pub(crate) fn run_assists_entry(args: &AssistsArgs) -> u8 {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_assists(args, &mut io::BufWriter::new(io::stdout().lock()))
    }));
    let Ok(code) = outcome else {
        eprintln!("dartq: internal error: analysis panicked");
        return exit::INTERNAL_ERROR;
    };
    code
}

/// The plugin hosts for a one-shot CLI run: discovered from the working
/// directory's `plugins/` convention and the governing `analysis_options`'s
/// pub-native `plugins:` section, the latter resolved through the parsed
/// `.dart_tool/package_config.json` beside the root (the daemon passes its
/// session's resolution instead). The config's state decides the posture —
/// see [`resolve_pub_config`].
fn launch_hosts() -> PluginHosts {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    launch_hosts_in(&cwd)
}

/// [`launch_hosts`] over an injectable root.
fn launch_hosts_in(cwd: &Path) -> PluginHosts {
    let config_dir = cwd.join(".dart_tool");
    let Some(pub_config) = resolve_pub_config(&config_dir.join("package_config.json")) else {
        return PluginHosts::empty();
    };
    PluginHosts::launch(cwd, &pub_config, &config_dir)
}

/// The pub-native config at `config_path`, forked on its state:
/// missing → the empty substitution, so the launch proceeds and declared
/// plugins skip as declared-but-unresolved — pub never resolved here, the
/// skips' `pub get` advice is right; present but unusable (unparsable
/// JSON, non-UTF-8 bytes, unreadable) → `None` after naming the file and
/// the real cause on stderr — pub did resolve and the artifact broke
/// afterwards, so advising `pub get` would be wrong, and the run takes
/// the no-dispatch posture instead. A valid file parses through
/// untouched.
fn resolve_pub_config(config_path: &Path) -> Option<PackageConfig> {
    let empty = || {
        PackageConfig::parse(r#"{"configVersion": 2, "packages": []}"#)
            .expect("empty config parses")
    };
    match fs::read_to_string(config_path) {
        Ok(text) => match PackageConfig::parse(&text) {
            Ok(config) => Some(config),
            Err(error) => {
                eprintln!(
                    "dartq: `{}` is corrupt ({error}); pub-native plugins cannot be \
                     resolved this run — check what wrote the file or restore it",
                    config_path.display()
                );
                None
            }
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => Some(empty()),
        Err(error) => {
            eprintln!(
                "dartq: `{}` is unreadable ({error}); pub-native plugins cannot be \
                 resolved this run — check what wrote the file or restore it",
                config_path.display()
            );
            None
        }
    }
}

/// The hosts' terminal failures surface on stderr — never silently.
fn report_errors(hosts: &mut PluginHosts) {
    for error in hosts.take_errors() {
        eprintln!(
            "dartq: plugin `{}` unavailable: {}",
            error.plugin, error.message
        );
    }
}

/// One fix run: the fix source stage (`dartq_commands::fix::collect_set` —
/// usage tier on a missing target) over the built-in rows (lint plus the
/// semantic engine pass, `None` SDK falling back to the `DART_SDK`
/// environment) and the plugin hosts' proposals, then the apply + report
/// stages.
fn run(args: &FixArgs, out: &mut impl Write) -> u8 {
    let mut hosts = launch_hosts();
    let collected = fix::collect_set(&args.paths, &mut hosts, None);
    let per_file: Vec<FileFixSet> = match collected {
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
    report_errors(&mut hosts);
    hosts.shutdown();
    apply_and_report(&per_file, args.dry_run, args.format, out)
}

/// One assists run: the plugin hosts' proposals for one file, anchored
/// against its on-disk text — a report-only face (the daemon's mirror is
/// dry by the same contract; applying an assist is the client's act).
fn run_assists(args: &AssistsArgs, out: &mut impl Write) -> u8 {
    if !args.path.exists() {
        eprintln!(
            "error: Directory or file doesn't exist: {}",
            args.path.display()
        );
        return exit::USAGE;
    }
    let mut hosts = launch_hosts();
    let collected = fix::assists(&args.path, args.offset, args.length, &mut hosts, None);
    let answer = match collected {
        Ok(answer) => answer,
        Err(DiscoverError::Io(error)) => {
            eprintln!("dartq: internal error: {error}");
            return exit::INTERNAL_ERROR;
        }
        Err(DiscoverError::Missing(path)) => {
            eprintln!("error: Directory or file doesn't exist: {path}");
            return exit::USAGE;
        }
    };
    report_errors(&mut hosts);
    hosts.shutdown();

    let written = match args.format {
        FixFormat::Machine => {
            writeln!(out, "{}", fix::assists_json(&answer, Verbosity::Compact))
        }
        FixFormat::Default => write_assists_human(out, &answer),
    }
    .and_then(|()| out.flush());
    if let Err(error) = written {
        eprintln!("dartq: internal error: writing assists output failed: {error}");
        return exit::INTERNAL_ERROR;
    }
    exit::CLEAN
}

/// Human assists listing: one row per edit
/// (`file:line:column • plugin • message`), then the summary.
fn write_assists_human(out: &mut impl Write, answer: &fix::AssistAnswer) -> io::Result<()> {
    let mut count = 0usize;
    for assist in &answer.assists {
        for edit in &assist.edits {
            count += 1;
            writeln!(
                out,
                "{}:{}:{} • {} • {}",
                answer.file, edit.anchor.line, edit.anchor.column, assist.plugin, assist.message
            )?;
        }
    }
    match count {
        0 => writeln!(out, "No assists found."),
        1 => writeln!(out, "1 proposed assist."),
        n => writeln!(out, "{n} proposed assists."),
    }
}

/// The span-capacity rejection on the apply face's read-failure channel
/// (the #1543 bypass face): the canonical capacity message under the
/// face's `reading … failed` prefix.
fn report_capacity(path: &str, len: u64) {
    eprintln!(
        "dartq: internal error: reading {path} failed: {}",
        OpenError::TooLarge { len }
    );
}

/// The apply + report stages over collected fix sets, collect-then-write:
/// every file's patched buffer is computed in memory first — a read or
/// patch failure here leaves nothing on disk mutated — and only then do
/// the writes run; one failing midway prints which files were already
/// mutated before the internal-error exit. The report follows: machine
/// documents (the apply record once the run writes), or the human listing
/// carrying the per-edit outcomes.
fn apply_and_report(
    per_file: &[FileFixSet],
    dry_run: bool,
    format: FixFormat,
    out: &mut impl Write,
) -> u8 {
    let mut staged: Vec<(&str, String, Vec<EditOutcome>)> = Vec::new();
    // The cross-file rows: `(target, born content, set index, row index)`
    // — the write is the file coming into existence, and the set/row
    // indices let a late-appearing target flip its status to stale.
    let mut created: Vec<(&str, &str, usize, usize)> = Vec::new();
    if !dry_run {
        for (set_index, set) in per_file.iter().enumerate() {
            // Span-capacity gates (#1543 bypass face): this is the last
            // read before the edits' `LineIndex`, so the stat refuses
            // before the bytes are read and the byte length is the
            // authoritative bound — a file swapped to oversized between
            // the collect read and this one is refused, never indexed.
            if let Ok(metadata) = fs::metadata(&set.path)
                && metadata.len() > MAX_SOURCE_BYTES as u64
            {
                report_capacity(&set.path, metadata.len());
                return exit::INTERNAL_ERROR;
            }
            let text = match fs::read_to_string(&set.path) {
                Ok(text) => text,
                Err(error) => {
                    eprintln!(
                        "dartq: internal error: reading {} failed: {error}",
                        set.path
                    );
                    return exit::INTERNAL_ERROR;
                }
            };
            if text.len() > MAX_SOURCE_BYTES {
                report_capacity(&set.path, text.len() as u64);
                return exit::INTERNAL_ERROR;
            }
            let edits = edits_of(set);
            let (patched, outcome) = fix::apply(&text, &edits);
            // Full row order for the report faces: the same-file rows take
            // apply()'s outcomes in order, each cross-file row holds its
            // slot until its own write answers, the plugin rows' outcomes
            // follow. The `unwrap_or` arm is unreachable by construction —
            // `edits_of` mirrors exactly this partition — and reports
            // stale rather than lying applied if it ever moved.
            let mut statuses = Vec::with_capacity(set.fixes.len());
            let mut same_file = outcome.into_iter();
            for (row_index, fix_row) in set.fixes.iter().enumerate() {
                match &fix_row.target {
                    Some(target) => {
                        created.push((target, fix_row.edit.replacement.as_str(), set_index, row_index));
                        statuses.push(EditOutcome::Applied);
                    }
                    None => statuses.push(same_file.next().unwrap_or(EditOutcome::Stale)),
                }
            }
            statuses.extend(same_file);
            staged.push((&set.path, patched, statuses));
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
    // The created files, after the hit files' writes: a target that
    // appeared between the offer and the write keeps the run's snapshot
    // honesty (stale, skipped — first writer wins), the write itself
    // brings the parent dirs the URI names into being.
    for (target, content, set_index, row_index) in &created {
        let target_path = Path::new(target);
        let outcome = if target_path.exists() {
            EditOutcome::Stale
        } else {
            let parent = target_path.parent().filter(|p| !p.as_os_str().is_empty());
            if let Some(parent) = parent
                && let Err(error) = fs::create_dir_all(parent)
            {
                eprintln!("dartq: internal error: creating {parent:?} failed: {error}");
                return exit::INTERNAL_ERROR;
            }
            match fs::write(target_path, content) {
                Ok(()) => EditOutcome::Applied,
                Err(error) => {
                    eprintln!("dartq: internal error: writing {target} failed: {error}");
                    return exit::INTERNAL_ERROR;
                }
            }
        };
        statuses[*set_index][*row_index] = outcome;
    }

    let written = match format {
        FixFormat::Machine => write_machine(out, per_file, &statuses, dry_run),
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

    /// The pub-config fork behind the host launch: missing → the empty
    /// substitution stands in (the launch proceeds; declared-but-unresolved
    /// skips keep their correct `pub get` advice); present but corrupt
    /// (unparsable, non-UTF-8) → no config, so the run cannot advise
    /// `pub get` for a broken artifact; a valid file parses through
    /// untouched.
    #[test]
    fn pub_config_missing_stands_in_corrupt_takes_no_dispatch() {
        let root = std::env::temp_dir().join(format!("dq_cli_fix_cfg_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let config_dir = root.join(".dart_tool");
        fs::create_dir_all(&config_dir).expect("mkdir");
        let config_path = config_dir.join("package_config.json");

        let config =
            resolve_pub_config(&config_path).expect("missing substitutes the empty config");
        assert!(
            config.entry("anything").is_none(),
            "the substitution is empty"
        );
        assert!(
            launch_hosts_in(&root).is_empty(),
            "the missing face launches normally over an empty workspace"
        );

        fs::write(&config_path, "{ not json").expect("write");
        assert!(
            resolve_pub_config(&config_path).is_none(),
            "a corrupt artifact takes the no-dispatch posture"
        );
        fs::write(&config_path, [0xff, 0x00]).expect("write bytes");
        assert!(
            resolve_pub_config(&config_path).is_none(),
            "non-UTF-8 bytes are the corrupt face too"
        );
        assert!(
            launch_hosts_in(&root).is_empty(),
            "the corrupt face skips the launch entirely"
        );

        fs::write(
            &config_path,
            r#"{"configVersion": 2, "packages": [{"name": "a", "rootUri": "../", "packageUri": "lib/"}]}"#,
        )
        .expect("write");
        let config = resolve_pub_config(&config_path).expect("a valid config parses");
        assert!(
            config.entry("a").is_some(),
            "the real config resolves its packages"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// The `unnecessary_new` fix set addressed to `path` — the exact edit
    /// the rule materializes over this source in dartq-commands (keyword
    /// plus trailing space at offset 19..23, verified in its
    /// `new_expression_end_to_end` test). The write-back tests stage the
    /// collected set directly.
    fn unnecessary_new_fix(path: &std::path::Path) -> FileFixSet {
        let source = "void f() { var a = new B(); }\n";
        let lines = LineIndex::new(source);
        FileFixSet {
            path: path.display().to_string(),
            fixes: vec![fix::RuleFix {
                rule: "unnecessary_new",
                message: "Unnecessary 'new' keyword.".to_owned(),
                source: fix::BUILTIN_SOURCE,
                edit: fix::anchored_edit(source, &lines, 19, 4, ""),
                target: None,
            }],
            plugin: Vec::new(),
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

    /// The machine face's two documents: a dry run keeps the proposal
    /// shape (no `outcome`, no `summary` — the rows are proposals); an
    /// apply run records every row's outcome plus the summary, so an
    /// all-stale run — a safe no-op that writes nothing and exits 0 — is
    /// still legible to the machine consumer.
    #[test]
    fn machine_face_proposes_on_dry_run_and_records_on_apply() {
        let root = std::env::temp_dir().join(format!("dq_cli_fix_machine_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("mkdir");
        let path = root.join("a.dart");
        fs::write(&path, "void f() { var a = new B(); }\n").expect("write");
        let per_file = vec![unnecessary_new_fix(&path)];

        let mut out = Vec::new();
        assert_eq!(
            apply_and_report(&per_file, true, FixFormat::Machine, &mut out),
            exit::CLEAN
        );
        let line = String::from_utf8(out).expect("utf-8");
        assert!(
            line.contains("\"edits\":[{"),
            "one proposal document: {line}"
        );
        assert!(
            !line.contains("outcome") && !line.contains("summary"),
            "proposals carry neither field: {line}"
        );
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "void f() { var a = new B(); }\n",
            "the dry run wrote nothing"
        );

        let mut out = Vec::new();
        assert_eq!(
            apply_and_report(&per_file, false, FixFormat::Machine, &mut out),
            exit::CLEAN
        );
        let line = String::from_utf8(out).expect("utf-8");
        assert!(line.contains("\"outcome\":\"applied\""), "{line}");
        assert!(
            line.contains("\"summary\":{\"applied\":1,\"stale\":0}"),
            "{line}"
        );
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "void f() { var a = B(); }\n",
            "the apply run rewrote the file"
        );

        // The stale record: the edit no longer matches the file, nothing
        // is written, and the document says so (applied 0, stale 1).
        fs::write(&path, "void f() { var a = B(); }\n").expect("write");
        let mut out = Vec::new();
        assert_eq!(
            apply_and_report(&per_file, false, FixFormat::Machine, &mut out),
            exit::CLEAN
        );
        let line = String::from_utf8(out).expect("utf-8");
        assert!(line.contains("\"outcome\":\"stale\""), "{line}");
        assert!(
            line.contains("\"summary\":{\"applied\":0,\"stale\":1}"),
            "{line}"
        );
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "void f() { var a = B(); }\n",
            "the stale run wrote nothing"
        );

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

    /// The apply stage's span-capacity funnel (#1543 bypass face): a
    /// target that turned oversized between the collect read and the
    /// apply read is refused at the internal tier — stdout stays empty,
    /// nothing is written. The sparse fixture claims the size with
    /// `set_len` (no blocks materialized); the stat rejects before any
    /// read.
    #[test]
    fn apply_rejects_span_capacity_target() {
        let root = std::env::temp_dir().join(format!("dq_cli_fix_capacity_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("mkdir");
        let huge = root.join("huge.dart");
        fs::File::create(&huge)
            .and_then(|file| file.set_len(dartq_source::MAX_SOURCE_BYTES as u64 + 1))
            .expect("sparse fixture");

        let per_file = vec![unnecessary_new_fix(&huge)];
        let mut out = Vec::new();
        assert_eq!(
            apply_and_report(&per_file, false, FixFormat::Default, &mut out),
            exit::INTERNAL_ERROR
        );
        assert!(out.is_empty(), "stdout stays empty on internal errors");
        assert_eq!(
            fs::metadata(&huge).expect("stat").len(),
            dartq_source::MAX_SOURCE_BYTES as u64 + 1,
            "nothing was written"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// The assists face: argument defaults, the usage tier on a missing
    /// target, and the plugin-less run (no plugins under the working
    /// directory): the built-in producers still propose — the default
    /// human run lists the async-body assist one row per edit, the
    /// off-selection machine run carries the empty assist list.
    #[test]
    fn assists_cli_defaults_and_zero_plugin_run() {
        fn parse(args: &[&str]) -> AssistsArgs {
            match Cli::try_parse_from(std::iter::once("dartq").chain(args.iter().copied()))
                .expect("parse")
                .command
            {
                Some(Command::Assists(args)) => args,
                Some(_) => panic!("expected the assists command for {args:?}"),
                None => panic!("expected a subcommand"),
            }
        }

        let root = std::env::temp_dir().join(format!("dq_cli_assists_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("mkdir");
        let path = root.join("a.dart");
        fs::write(&path, "void f() { var a = new B(); }\n").expect("write");

        let args = parse(&["assists", path.to_str().expect("utf-8")]);
        assert_eq!(args.offset, 0, "assists default to the file start");
        assert_eq!(args.length, 0);
        assert!(matches!(args.format, FixFormat::Default));

        let args = parse(&[
            "assists",
            path.to_str().expect("utf-8"),
            "--offset",
            "19",
            "--length",
            "4",
            "--format=machine",
        ]);
        assert_eq!(args.offset, 19);
        assert_eq!(args.length, 4);
        assert!(matches!(args.format, FixFormat::Machine));

        let mut out = Vec::new();
        assert_eq!(run_assists(&args, &mut out), exit::CLEAN);
        let text = String::from_utf8(out).expect("utf-8");
        assert!(
            text.contains("\"assists\":[]") && text.contains("\"schema_version\":1"),
            "the machine document carries the empty assist list: {text}"
        );

        let mut out = Vec::new();
        let args = parse(&["assists", path.to_str().expect("utf-8")]);
        assert_eq!(run_assists(&args, &mut out), exit::CLEAN);
        // Offset 0 sits at the body's first token end, so the builtin
        // async-body assist proposes (the official guard declines only
        // past `body.block.beginToken.end`). One change, two edits — the
        // `async ` insertion at the body and the `Future<void>` wrap at
        // the `void` return type — one human row per edit.
        assert_eq!(
            String::from_utf8(out).expect("utf-8"),
            format!(
                "{path}:1:10 • builtin-assist • Convert to async function body\n\
                 {path}:1:1 • builtin-assist • Convert to async function body\n\
                 2 proposed assists.\n",
                path = path.display()
            )
        );

        let mut out = Vec::new();
        let args = parse(&["assists", "/nonexistent/dq_assists_target"]);
        assert_eq!(run_assists(&args, &mut out), exit::USAGE);
        assert!(out.is_empty(), "stdout stays empty on usage errors");

        fs::remove_dir_all(&root).ok();
    }

    /// The cross-file row end to end: the apply run materializes the born
    /// file (the nested dir the URI names included), the hit file's bytes
    /// stay untouched, the human listing names the created file at its
    /// head, and a target that already exists flips the row stale — the
    /// first writer wins. `--dry-run` never writes.
    #[test]
    fn fix_apply_materializes_the_created_file() {
        let root = std::env::temp_dir().join(format!("dq_cli_fix_create_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("mkdir");
        let hit = root.join("test.dart");
        fs::write(&hit, "import 'src/missing.dart';\n").expect("write");
        let target = root.join("src/missing.dart");
        let born = "// TODO Implement this library.";
        let lines = LineIndex::new("");
        let per_file = vec![FileFixSet {
            path: hit.display().to_string(),
            fixes: vec![fix::RuleFix {
                rule: "uri_does_not_exist",
                message: "Target of URI doesn't exist: 'src/missing.dart'.".to_owned(),
                source: fix::BUILTIN_SEMANTIC_SOURCE,
                edit: fix::anchored_edit("", &lines, 0, 0, born),
                target: Some(target.display().to_string()),
            }],
            plugin: Vec::new(),
        }];

        // Dry-run: the proposal names the created file, nothing on disk.
        let mut out = Vec::new();
        assert_eq!(
            apply_and_report(&per_file, true, FixFormat::Default, &mut out),
            exit::CLEAN
        );
        assert!(
            !target.exists(),
            "a dry run never materializes the born file"
        );
        let listing = String::from_utf8(out).expect("utf-8");
        assert!(
            listing.starts_with(&format!(
                "{}:1:1 • uri_does_not_exist • Target of URI",
                target.display()
            )),
            "the listing names the created file at its head: {listing:?}"
        );

        // The apply run: the file comes into existence with the stub, the
        // row reports applied.
        let mut out = Vec::new();
        assert_eq!(
            apply_and_report(&per_file, false, FixFormat::Default, &mut out),
            exit::CLEAN
        );
        assert_eq!(fs::read_to_string(&target).expect("born text"), born);
        assert!(
            String::from_utf8(out).expect("utf-8").contains("[applied]"),
            "the written row tags applied"
        );

        // The target now exists: the executor's write-time re-check flips
        // the row stale and leaves the file's bytes alone.
        let mut out = Vec::new();
        assert_eq!(
            apply_and_report(&per_file, false, FixFormat::Default, &mut out),
            exit::CLEAN
        );
        assert_eq!(
            fs::read_to_string(&target).expect("born text"),
            born,
            "first writer wins"
        );
        assert!(
            String::from_utf8(out).expect("utf-8").contains("[stale]"),
            "the already-satisfied row tags stale"
        );

        fs::remove_dir_all(&root).ok();
    }
}
