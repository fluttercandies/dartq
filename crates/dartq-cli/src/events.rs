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
