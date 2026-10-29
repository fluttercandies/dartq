//! `dartq/test/*` — test runs from the daemon.
//!
//! A run is a `dartq test` child process with the JSON reporter: the same
//! selection, result cache, grouped execution, memory leases and exit
//! codes as the command line, and the `package:test` JSON protocol events
//! `dart test --reporter json` writes. The child keeps a test run's memory
//! out of the daemon, and an interrupted child takes its runners with it.
//!
//! ## Faces
//!
//! - `dartq/test/run` — `{ args: [string], cwd? }`: start
//!   `dartq test <args> --reporter=json` in `cwd` (relative to the
//!   workspace root; default the root). `args` is `dart test`'s argument
//!   grammar plus `--no-cache`/`--impacted[=REF]`. Answers `{ run_id }`.
//! - `dartq/test/status` — `{ run_id? }`: one run (`state`, `exit_code`,
//!   `passed`/`failed`/`skipped` as the runner counts them, `events`,
//!   `stderr`), or `{ runs }` for every known run.
//! - `dartq/test/events` — `{ run_id, cursor?, limit? }`: the run's protocol
//!   events from `cursor`, at most `limit` (default 200, at most 2000).
//! - `dartq/test/cancel` — `{ run_id }`: stop a running run.
//! - `dartq/test/probe` — `{}`: the `dart` a run would use and the
//!   workspace's `test` package version, read without spawning anything.
//!
//! The `--impacted` selection is the child's — resolved by the one shared
//! implementation (`dartq_commands::impact::impacted_tests`, the CLI test
//! face's own selection), so both faces answer from the same function for
//! the same input. Its conservative fallback (no repository, an unbounded
//! input, or a semantic graph that cannot assemble) is the full run,
//! announced on the child's stderr — surfaced here as the run's `stderr`.
//!
//! Every face answers at intake without blocking. The newest
//! [`MAX_FINISHED`] finished runs are kept.

use crate::jsonrpc::codes;
use rustc_hash::FxHashMap;
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

pub const TEST_RUN: &str = "dartq/test/run";
pub const TEST_STATUS: &str = "dartq/test/status";
pub const TEST_EVENTS: &str = "dartq/test/events";
pub const TEST_CANCEL: &str = "dartq/test/cancel";
pub const TEST_PROBE: &str = "dartq/test/probe";

pub const TEST_REQUESTS: [&str; 5] = [TEST_RUN, TEST_STATUS, TEST_EVENTS, TEST_CANCEL, TEST_PROBE];

/// Finished runs kept for `status`/`events`.
const MAX_FINISHED: usize = 16;

/// Standard error kept per run (its tail).
const MAX_STDERR: usize = 64 * 1024;

/// Whether `method` is one of the test faces this module serves.
#[must_use]
pub(crate) fn is_request(method: &str) -> bool {
    TEST_REQUESTS.contains(&method)
}

#[derive(Debug)]
pub(crate) enum TestError {
    InvalidParams(String),
    NoSuchRun(u64),
    Domain(String),
}

impl TestError {
    pub(crate) fn wire(self) -> (i64, String) {
        match self {
            Self::InvalidParams(m) => (codes::INVALID_PARAMS, m),
            Self::NoSuchRun(id) => (
                codes::TEST_RUN,
                format!("DARTQ/TEST/NO_SUCH_RUN: run {id} is not known"),
            ),
            Self::Domain(m) => (codes::TEST_RUN, format!("DARTQ/TEST: {m}")),
        }
    }
}

/// What a run's reader threads accumulate.
#[derive(Default)]
struct Progress {
    /// Protocol events, one JSON line each.
    events: Vec<String>,
    tally: Tally,
    stderr: String,
    /// `None` while running; the exit code (`-1` for a signal) after.
    exit: Option<i32>,
}

/// A test's place in the engine's sets.
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Passed,
    Skipped,
    Failed,
    Hidden,
}

/// Counts as the runner's engine keeps them: an error moves a passed or
/// skipped test to failed; hidden tests count only through their errors.
#[derive(Default)]
struct Tally {
    passed: u64,
    failed: u64,
    skipped: u64,
    state: FxHashMap<u64, State>,
}

impl Tally {
    fn event(&mut self, event: &Value) {
        let Some(id) = event.get("testID").and_then(Value::as_u64) else {
            return;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("error") => match self.state.insert(id, State::Failed) {
                Some(State::Failed) => {}
                Some(State::Passed) => {
                    self.passed -= 1;
                    self.failed += 1;
                }
                Some(State::Skipped) => {
                    self.skipped -= 1;
                    self.failed += 1;
                }
                Some(State::Hidden) | None => self.failed += 1,
            },
            Some("testDone") if self.state.get(&id) != Some(&State::Failed) => {
                let flag = |name: &str| event.get(name).and_then(Value::as_bool).unwrap_or(false);
                let state = if flag("hidden") {
                    State::Hidden
                } else if flag("skipped") {
                    self.skipped += 1;
                    State::Skipped
                } else if event.get("result").and_then(Value::as_str) == Some("success") {
                    self.passed += 1;
                    State::Passed
                } else {
                    self.failed += 1;
                    State::Failed
                };
                self.state.insert(id, state);
            }
            _ => {}
        }
    }
}

struct ManagedRun {
    /// The `dartq test` child until its reader reaps it.
    child: Arc<parking_lot::Mutex<Option<Child>>>,
    progress: Arc<parking_lot::Mutex<Progress>>,
    cancelled: bool,
}

/// The session's run registry.
#[derive(Default)]
pub(crate) struct TestManager {
    runs: FxHashMap<u64, ManagedRun>,
    next_id: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunParams {
    args: Vec<String>,
    #[serde(default)]
    cwd: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusParams {
    #[serde(default)]
    run_id: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EventsParams {
    run_id: u64,
    #[serde(default)]
    cursor: usize,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CancelParams {
    run_id: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProbeParams {}

fn decode<T: for<'de> Deserialize<'de>>(params: &Value) -> Result<T, TestError> {
    let params = if params.is_null() {
        json!({})
    } else {
        params.clone()
    };
    serde_json::from_value(params).map_err(|error| TestError::InvalidParams(error.to_string()))
}

/// Answer one `dartq/test/*` request.
pub(crate) fn handle(
    manager: &mut TestManager,
    method: &str,
    params: &Value,
    root: &Path,
) -> Result<Value, TestError> {
    match method {
        TEST_RUN => {
            let params: RunParams = decode(params)?;
            let cwd = params
                .cwd
                .as_ref()
                .map_or_else(|| root.to_path_buf(), |dir| root.join(dir));
            if !cwd.is_dir() {
                return Err(TestError::InvalidParams(format!(
                    "cwd {} is not a directory",
                    cwd.display()
                )));
            }
            let id = manager.start(&params.args, &cwd)?;
            Ok(json!({ "run_id": id }))
        }
        TEST_STATUS => {
            let params: StatusParams = decode(params)?;
            if let Some(id) = params.run_id {
                return manager.status(id);
            }
            let mut ids: Vec<u64> = manager.runs.keys().copied().collect();
            ids.sort_unstable();
            let runs = ids
                .into_iter()
                .map(|id| manager.status(id))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(json!({ "runs": runs }))
        }
        TEST_EVENTS => {
            let params: EventsParams = decode(params)?;
            let run = manager
                .runs
                .get(&params.run_id)
                .ok_or(TestError::NoSuchRun(params.run_id))?;
            let progress = run.progress.lock();
            let total = progress.events.len();
            let cursor = params.cursor.min(total);
            let end = (cursor + params.limit.unwrap_or(200).clamp(1, 2000)).min(total);
            let page: Vec<Value> = progress.events[cursor..end]
                .iter()
                .map(|line| serde_json::from_str(line).unwrap_or(Value::Null))
                .collect();
            Ok(json!({
                "run_id": params.run_id,
                "events": page,
                "next_cursor": end,
                "has_more": end < total || progress.exit.is_none(),
            }))
        }
        TEST_CANCEL => {
            let params: CancelParams = decode(params)?;
            let run = manager
                .runs
                .get_mut(&params.run_id)
                .ok_or(TestError::NoSuchRun(params.run_id))?;
            let running = run.progress.lock().exit.is_none();
            if running {
                run.cancelled = true;
                if let Some(child) = run.child.lock().as_ref() {
                    terminate(child);
                }
            }
            Ok(json!({ "run_id": params.run_id, "cancelled": running }))
        }
        TEST_PROBE => {
            let _: ProbeParams = decode(params)?;
            let test = std::fs::read_to_string(root.join("pubspec.lock"))
                .ok()
                .and_then(|text| dartq_pub::LockFile::parse(&text).ok())
                .and_then(|lock| {
                    lock.package("test")
                        .map(|package| package.version.to_string())
                });
            Ok(json!({
                "dart": dartq_test::host::find("dart").map(|path| path.display().to_string()),
                "test_package": test,
            }))
        }
        _ => Err(TestError::InvalidParams(format!(
            "unknown method: {method}"
        ))),
    }
}

impl TestManager {
    fn start(&mut self, args: &[String], cwd: &Path) -> Result<u64, TestError> {
        let bin = dartq_bin()?;
        // The run's own reporter goes after the caller's options (the last
        // one is in effect) and before any `--`.
        let at = args
            .iter()
            .position(|arg| arg == "--")
            .unwrap_or(args.len());
        let mut command = Command::new(&bin);
        command
            .arg("test")
            .args(&args[..at])
            .arg("--reporter=json")
            .args(&args[at..])
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // A group of its own: the daemon's terminal signals stay the
        // daemon's, and a cancel reaches the child and its compiler.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command.spawn().map_err(|error| {
            TestError::Domain(format!("cannot start {}: {error}", bin.display()))
        })?;
        let stdout = child.stdout.take().expect("stdout is piped");
        let mut stderr = child.stderr.take().expect("stderr is piped");
        let progress = Arc::new(parking_lot::Mutex::new(Progress::default()));
        let child = Arc::new(parking_lot::Mutex::new(Some(child)));

        let errors = {
            let progress = Arc::clone(&progress);
            std::thread::spawn(move || {
                let mut buffer = [0u8; 8192];
                while let Ok(read @ 1..) = stderr.read(&mut buffer) {
                    let mut progress = progress.lock();
                    progress
                        .stderr
                        .push_str(&String::from_utf8_lossy(&buffer[..read]));
                    if progress.stderr.len() > MAX_STDERR {
                        let mut cut = progress.stderr.len() - MAX_STDERR;
                        while !progress.stderr.is_char_boundary(cut) {
                            cut += 1;
                        }
                        progress.stderr.drain(..cut);
                    }
                }
            })
        };
        {
            let progress = Arc::clone(&progress);
            let child = Arc::clone(&child);
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    let Ok(event) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    let mut progress = progress.lock();
                    progress.tally.event(&event);
                    progress.events.push(line);
                }
                let _ = errors.join();
                let status = child.lock().take().map(|mut child| child.wait());
                let code = match status {
                    Some(Ok(status)) => status.code().unwrap_or(-1),
                    _ => -1,
                };
                progress.lock().exit = Some(code);
            });
        }
        self.next_id += 1;
        self.runs.insert(
            self.next_id,
            ManagedRun {
                child,
                progress,
                cancelled: false,
            },
        );
        self.evict();
        Ok(self.next_id)
    }

    /// Keep the newest [`MAX_FINISHED`] finished runs.
    fn evict(&mut self) {
        let mut finished: Vec<u64> = self
            .runs
            .iter()
            .filter(|(_, run)| run.progress.lock().exit.is_some())
            .map(|(&id, _)| id)
            .collect();
        if finished.len() > MAX_FINISHED {
            finished.sort_unstable();
            for id in &finished[..finished.len() - MAX_FINISHED] {
                self.runs.remove(id);
            }
        }
    }

    fn status(&self, id: u64) -> Result<Value, TestError> {
        let run = self.runs.get(&id).ok_or(TestError::NoSuchRun(id))?;
        let progress = run.progress.lock();
        let state = match progress.exit {
            None => "running",
            Some(_) if run.cancelled => "cancelled",
            Some(_) => "done",
        };
        Ok(json!({
            "run_id": id,
            "state": state,
            "exit_code": progress.exit,
            "passed": progress.tally.passed,
            "failed": progress.tally.failed,
            "skipped": progress.tally.skipped,
            "events": progress.events.len(),
            "stderr": progress.stderr,
        }))
    }
}

impl Drop for TestManager {
    /// A session that ends stops its runs.
    fn drop(&mut self) {
        for run in self.runs.values() {
            if let Some(child) = run.child.lock().as_ref() {
                terminate(child);
            }
        }
    }
}

/// Ask a run's `dartq test` to stop: on `SIGTERM` it kills its runner
/// groups and exits.
fn terminate(child: &Child) {
    #[cfg(unix)]
    if let Ok(pid) = libc::pid_t::try_from(child.id()) {
        // SAFETY: plain syscall on the group this module created.
        unsafe { libc::kill(-pid, libc::SIGTERM) };
    }
    #[cfg(not(unix))]
    let _ = child;
}

/// The `dartq` binary runs start: `DARTQ_BIN`, else this process when it
/// is `dartq` itself.
fn dartq_bin() -> Result<PathBuf, TestError> {
    if let Some(bin) = std::env::var_os("DARTQ_BIN") {
        return Ok(PathBuf::from(bin));
    }
    let exe = std::env::current_exe()
        .map_err(|error| TestError::Domain(format!("cannot locate dartq: {error}")))?;
    if exe.file_stem().and_then(|stem| stem.to_str()) == Some("dartq") {
        Ok(exe)
    } else {
        Err(TestError::Domain(
            "test runs need the dartq binary (set DARTQ_BIN)".to_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tally_counts_as_the_engine() {
        let mut tally = Tally::default();
        for line in [
            r#"{"type":"testDone","testID":1,"result":"success","hidden":true}"#,
            r#"{"type":"testDone","testID":2,"result":"success","hidden":false}"#,
            r#"{"type":"testDone","testID":3,"result":"success","skipped":true}"#,
            r#"{"type":"error","testID":4}"#,
            r#"{"type":"testDone","testID":4,"result":"failure"}"#,
            // An error after completion moves a passed test to failed.
            r#"{"type":"testDone","testID":5,"result":"success"}"#,
            r#"{"type":"error","testID":5}"#,
            r#"{"type":"error","testID":5}"#,
            // A failing hidden test (`setUpAll`) counts through its error.
            r#"{"type":"error","testID":6}"#,
            r#"{"type":"testDone","testID":6,"result":"error","hidden":true}"#,
        ] {
            tally.event(&serde_json::from_str(line).expect("json"));
        }
        assert_eq!((tally.passed, tally.skipped, tally.failed), (1, 1, 3));
    }

    #[test]
    fn params_are_checked_before_anything_runs() {
        let mut manager = TestManager::default();
        let root = std::env::temp_dir();
        for (method, params) in [
            (TEST_RUN, json!({})),
            (TEST_RUN, json!({ "args": [], "extra": 1 })),
            (TEST_RUN, json!({ "args": [], "cwd": "no/such/dir" })),
            (TEST_EVENTS, json!({})),
            (TEST_CANCEL, json!({})),
            (TEST_PROBE, json!({ "force": true })),
        ] {
            let error = handle(&mut manager, method, &params, &root).expect_err("rejected");
            assert!(
                matches!(error, TestError::InvalidParams(_)),
                "{method}: {error:?}"
            );
        }
        assert!(matches!(
            handle(&mut manager, TEST_STATUS, &json!({ "run_id": 7 }), &root),
            Err(TestError::NoSuchRun(7))
        ));
        assert_eq!(
            handle(&mut manager, TEST_STATUS, &json!({}), &root).expect("status"),
            json!({ "runs": [] })
        );
    }
}
