//! The `dartq events` shell: the unified event read — newest-first
//! paged JSONL over the session's event log (`session` handle) and the
//! cold-analysis progress snapshot (`progress` handle, derived from the
//! engine trace: files scanned, current package — the anti-hung signal
//! for a monster monorepo's first pass). Test-run and debug-session
//! streams keep their cursor-paged RPC faces on the session socket; this
//! verb is the CLI-side read of the same session's records.

use std::path::PathBuf;

use clap::Args;

use dartq_session::{EventLog, Layout, Progress};

use crate::OutputFormat;
use crate::exit;
use crate::output::lon_line;

/// `dartq events` — read a session's events.
#[derive(Args)]
pub struct EventsArgs {
    /// The event handle: `session` (the event log — lifecycle, client
    /// attach/detach, test-run and debug-session records) or `progress`
    /// (the cold-analysis snapshot: files scanned, current package).
    #[arg(default_value = "session")]
    pub handle: String,
    /// The worktree; defaults to the current directory.
    #[arg(default_value = ".")]
    pub dir: PathBuf,
    /// The session handle's timestamp floor: only rows at or after
    /// this instant, in milliseconds — the same unit the rows' own
    /// `ts` field carries (#2657: one unit across the flag and the
    /// data); `progress` treats it as the last-seen file count.
    #[arg(long)]
    pub since: Option<u64>,
    /// At most this many events per read, keeping the newest rows
    /// (session handle). 100 is the standing default: one page a
    /// screen-sized agent read wants, and low enough that a monster
    /// session's log never floods a context.
    #[arg(long, default_value_t = 100)]
    pub limit: usize,
    /// Output format: `lon` (the default, one canonical line per row),
    /// `json` (one JSON line per row), or `text` (the indented rows).
    #[arg(long, value_enum, default_value_t = OutputFormat::Lon)]
    pub format: OutputFormat,
}

pub fn run_entry(args: &EventsArgs) -> u8 {
    let root = match std::fs::canonicalize(&args.dir) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("error: cannot resolve {}: {error}", args.dir.display());
            return exit::USAGE;
        }
    };
    let layout = Layout::new(&root);
    match args.handle.as_str() {
        "session" => {
            let log = EventLog::new(&layout);
            // #2622: newest first — an oldest-first page never reached
            // the newest rows, which is the read an agent tailing the
            // session wants. `--since` is a millisecond floor on the
            // rows' own `ts` unit (#2657).
            let since_ms = args.since.map_or(0, u128::from);
            for (_, event) in log.read_newest(None, since_ms, args.limit) {
                match args.format {
                    OutputFormat::Lon => println!("{}", lon_line(&event)),
                    OutputFormat::Json => println!("{event}"),
                    // One indented block per row: the read stays
                    // cursor-paginated line-wise on the data faces, and
                    // the text face stays humanly scannable per record.
                    OutputFormat::Text => println!(
                        "{}",
                        serde_json::to_string_pretty(&event).expect("the value pretty-prints")
                    ),
                }
            }
            exit::CLEAN
        }
        "progress" => {
            let progress = Progress::from_trace(&layout.dir.join("trace.jsonl"));
            if progress.files_scanned <= args.since.unwrap_or_default() {
                return exit::CLEAN;
            }
            let row = progress_row(&progress);
            match args.format {
                OutputFormat::Lon => println!("{}", lon_line(&row)),
                OutputFormat::Json => println!("{row}"),
                OutputFormat::Text => println!(
                    "progress: {} files scanned ({})",
                    progress.files_scanned,
                    progress
                        .current
                        .as_deref()
                        .or(progress.current_package.as_deref())
                        .unwrap_or("analyzing"),
                ),
            }
            exit::CLEAN
        }
        other => {
            eprintln!(
                "error: unknown event handle `{other}` (handles: session, progress — \
                 test-run and debug-session streams ride the session socket's RPC faces)"
            );
            exit::USAGE
        }
    }
}

/// The progress handle's row document — the one builder the CLI verb
/// prints and the MCP events tool answers (#2657: the two faces' bytes
/// come from the same shape).
pub(crate) fn progress_row(progress: &Progress) -> serde_json::Value {
    serde_json::json!({
        "type": "progress",
        "files_scanned": progress.files_scanned,
        "from_cache": progress.from_cache,
        "current": progress.current,
        "current_package": progress.current_package,
        "cursor": progress.files_scanned,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn temp(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dqs-events-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    /// `derive(Args)` brings no `try_parse_from`; parse through the
    /// augmented command the parent `Cli` would splice in.
    fn parse(args: &[&str]) -> Result<EventsArgs, clap::Error> {
        let command = <EventsArgs as clap::Args>::augment_args(clap::Command::new("events"));
        let matches =
            command.try_get_matches_from(std::iter::once("events").chain(args.iter().copied()))?;
        <EventsArgs as clap::FromArgMatches>::from_arg_matches(&matches)
    }

    /// The grammar: the handle defaults to `session`, the dir to the cwd,
    /// `--since` is an optional millisecond floor (session handle — the
    /// rows' own `ts` unit), `--limit` defaults to 100, and `lon` is the
    /// constant format default.
    #[test]
    fn events_args_parse() {
        let args = parse(&[]).expect("parse");
        assert_eq!(args.handle, "session");
        assert_eq!(args.dir, PathBuf::from("."));
        assert_eq!(args.since, None);
        assert_eq!(args.limit, 100);
        assert!(matches!(args.format, OutputFormat::Lon), "lon default");
        let args = parse(&["progress", "/w", "--since", "5", "--limit", "3"]).expect("parse");
        assert_eq!(args.handle, "progress");
        assert_eq!(args.dir, PathBuf::from("/w"));
        assert_eq!(args.since, Some(5));
        assert_eq!(args.limit, 3);
        assert!(parse(&["--limit", "x"]).is_err());
        assert!(parse(&["--format", "machine"]).is_err(), "machine retired");
    }

    /// Every face answers clean while the log drains; an unknown handle
    /// is the usage tier (test-run and debug streams ride the session
    /// socket, not this verb).
    #[test]
    fn events_run_entry_tiers_over_a_real_tree() {
        let root = temp("tiers");
        let layout = Layout::new(&root);
        let log = EventLog::new(&layout);
        for _ in 0..3 {
            log.append("session", "serving", serde_json::json!({}))
                .expect("append");
        }
        let read = |handle: &str, since: Option<u64>, limit: usize| {
            run_entry(&EventsArgs {
                handle: handle.to_owned(),
                dir: root.clone(),
                since,
                limit,
                format: OutputFormat::Json,
            })
        };
        assert_eq!(read("session", None, 100), exit::CLEAN);
        assert_eq!(
            read("session", Some(2), 100),
            exit::CLEAN,
            "a far-past floor keeps every row"
        );
        assert_eq!(
            read("session", Some(u64::MAX), 100),
            exit::CLEAN,
            "a future floor reads nothing, still clean"
        );
        assert_eq!(
            read("session", None, 0),
            exit::CLEAN,
            "a zero limit reads nothing"
        );
        // `--since` floors on the rows' own millisecond unit (#2657): a
        // 60 s-old floor keeps the fresh rows, a 60 s-future one drops
        // them — the same numbers a client chains from a row's `ts`.
        let now_ms = u64::try_from(dartq_session::SessionState::now_ms()).expect("unix ms");
        assert_eq!(read("session", Some(now_ms - 60_000), 100), exit::CLEAN);
        assert_eq!(read("session", Some(now_ms + 60_000), 100), exit::CLEAN);
        // The progress handle: no trace is zero progress; a trace past the
        // cursor prints, at-or-below prints nothing — both clean.
        assert_eq!(read("progress", None, 100), exit::CLEAN);
        std::fs::write(
            &layout.dir.join("trace.jsonl"),
            "{\"event\":\"LibraryAnalyzed\",\"uri\":\"package:fx/a.dart\",\"from_cache\":false}\n",
        )
        .expect("trace");
        assert_eq!(read("progress", Some(1), 100), exit::CLEAN);
        assert_eq!(read("progress", Some(0), 100), exit::CLEAN);
        assert_eq!(read("bogus", None, 100), exit::USAGE, "unknown handle");
        assert_eq!(
            run_entry(&EventsArgs {
                handle: "session".into(),
                dir: root.join("gone"),
                since: None,
                limit: 100,
                format: OutputFormat::Json,
            }),
            exit::USAGE,
            "an unresolvable dir is the usage tier"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
