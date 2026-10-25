//! The `dartq events` shell: the unified event read — cursor-paginated
//! JSONL over the session's event log (`session` handle) and the
//! cold-analysis progress snapshot (`progress` handle, derived from the
//! engine trace: files scanned, current package — the anti-hung signal
//! for a monster monorepo's first pass). Test-run and debug-session
//! streams keep their cursor-paged RPC faces on the session socket; this
//! verb is the CLI-side read of the same session's records.

use std::path::PathBuf;

use clap::Args;

use dartq_session::{EventLog, Layout, Progress};

use crate::exit;

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
    /// The cursor: events after this sequence (the last read line's
    /// `seq`); `progress` treats it as the last-seen file count.
    #[arg(long)]
    pub since: Option<u64>,
    /// At most this many events per read (session handle).
    #[arg(long, default_value_t = 100)]
    pub limit: usize,
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
            for (_, event) in log.read(None, args.since.unwrap_or_default(), args.limit) {
                println!("{event}");
            }
            exit::CLEAN
        }
        "progress" => {
            let progress = Progress::from_trace(&layout.dir.join("trace.jsonl"));
            if progress.files_scanned <= args.since.unwrap_or_default() {
                return exit::CLEAN;
            }
            println!(
                "{{\"type\":\"progress\",\"files_scanned\":{},\"from_cache\":{},\"current\":{},\"current_package\":{},\"cursor\":{}}}",
                progress.files_scanned,
                progress.from_cache,
                json_opt(&progress.current),
                json_opt(&progress.current_package),
                progress.files_scanned,
            );
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

fn json_opt(value: &Option<String>) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".into())
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
    /// `--since` is optional, and `--limit` defaults to 100.
    #[test]
    fn events_args_parse() {
        let args = parse(&[]).expect("parse");
        assert_eq!(args.handle, "session");
        assert_eq!(args.dir, PathBuf::from("."));
        assert_eq!(args.since, None);
        assert_eq!(args.limit, 100);
        let args = parse(&["progress", "/w", "--since", "5", "--limit", "3"]).expect("parse");
        assert_eq!(args.handle, "progress");
        assert_eq!(args.dir, PathBuf::from("/w"));
        assert_eq!(args.since, Some(5));
        assert_eq!(args.limit, 3);
        assert!(parse(&["--limit", "x"]).is_err());
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
            log.append("session", "serving", serde_json::json!({})).expect("append");
        }
        let read = |handle: &str, since: Option<u64>, limit: usize| {
            run_entry(&EventsArgs {
                handle: handle.to_owned(),
                dir: root.clone(),
                since,
                limit,
            })
        };
        assert_eq!(read("session", None, 100), exit::CLEAN);
        assert_eq!(read("session", Some(2), 100), exit::CLEAN, "cursor pagination");
        assert_eq!(read("session", None, 0), exit::CLEAN, "a zero limit reads nothing");
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
            run_entry(&EventsArgs { handle: "session".into(), dir: root.join("gone"), since: None, limit: 100 }),
            exit::USAGE,
            "an unresolvable dir is the usage tier"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
