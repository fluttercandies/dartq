//! Runner invocation: spawn `dart test --reporter json` (or the grouped
//! runner, which speaks the same protocol), split its event stream into
//! per-suite blocks as suites settle, and enforce an idle watchdog over the
//! runner's whole process group.
//!
//! ## Wire format (upstream dart-lang/test JSON reporter + `flutter_tools` source)
//!
//! Every stdout line is a JSON object. Event types we care about:
//!
//! - `{"type":"suite","suite":{"id":N,"platform":"vm","path":"..."}}` —
//!   suite declared; `path` is the test file.
//! - `{"type":"testStart","test":{"id":N,"name":"...","suiteID":N,...}}`
//! - `{"type":"testDone","testID":N,"result":"success|failure|error",
//!   "skipped":bool,"durationMs":N,"error":null|{...},"stackTrace":"..."}`
//! - `{"type":"error","error":"..."}` — suite-level error (load failure).
//! - `{"type":"allSuites"}` / `{"type":"done"}` — run boundary.
//!
//! Every event is kept verbatim in its suite's block; dartq re-renders the
//! blocks with the reporter the user asked for.

use rustc_hash::FxHashMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Default per-run timeout. Suites that hang are killed.
const RUN_TIMEOUT: Duration = Duration::from_secs(30 * 60); // 30 min

/// Spawn configuration.
#[derive(Debug, Clone)]
pub struct ExecConfig {
    /// The `dart` binary.
    pub dart_bin: PathBuf,
    /// Working directory (workspace root).
    pub cwd: PathBuf,
    /// Extra args passed through to the test binary.
    pub extra_args: Vec<String>,
    /// Longest the runner may go without writing a line before it is
    /// declared hung and its process group killed; `None` = `RUN_TIMEOUT`.
    /// A run of any length that keeps reporting is never cut short (the
    /// runner enforces each test's own timeout).
    pub timeout: Option<Duration>,
    /// Receives each suite's events the moment the suite settles (the
    /// grouped runner's per-suite marker, or the end of the run) instead of
    /// collecting them in [`Execution::blocks`]: a caller can report and
    /// cache a suite while the rest of the run continues, holding no more
    /// than the suites still in flight. A raw output line that belongs to no
    /// suite in flight ([`Streamed::Line`]) arrives the moment it was
    /// written, so a caller prints it in place.
    pub sink: Option<std::sync::mpsc::Sender<Streamed>>,
    /// Signalled once the runner reports its first suite: everything it
    /// prepares before loading suites (packages, test assets, native
    /// assets) is in place by then.
    pub ready: Option<std::sync::mpsc::Sender<()>>,
}

/// One suite's events, handed over as soon as the suite settled.
#[derive(Debug)]
pub struct Settled {
    pub block: SuiteBlock,
    /// When the runner that produced the block was spawned: its events'
    /// `time` fields count from about then.
    pub spawned: Instant,
}

/// What a run's stream hands to its sink as the run progresses.
#[derive(Debug)]
pub enum Streamed {
    Settled(Settled),
    /// A raw output line that belongs to no suite in flight: print it now,
    /// before the suites still running settle.
    Line(String),
}

/// Spawn error.
#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    #[error("spawn: {0}")]
    Spawn(std::io::Error),
    #[error("wait: {0}")]
    Wait(std::io::Error),
    #[error("timeout after {0:?}")]
    Timeout(Duration),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// One finished runner invocation.
#[derive(Debug)]
pub struct Execution {
    /// The reporter emitted its `done` event: every suite it loaded ran to
    /// completion. A run killed or crashed mid-way is incomplete, and none
    /// of its rows may be trusted as a suite's full outcome.
    pub complete: bool,
    /// Every suite path the reporter announced, in announcement order.
    pub suites: Vec<PathBuf>,
    /// Peak resident size of the runner's whole process tree (its process
    /// group), sampled once a second; `0` where it cannot be read.
    pub peak_rss: u64,
    /// The reporter's own events, grouped per suite in emission order.
    pub blocks: Vec<SuiteBlock>,
    /// Events that belong to no suite (a runner-level `error`), and test
    /// output lines that are not reporter events, in emission order.
    pub loose: Vec<Loose>,
    /// Everything the runner wrote to standard error (warnings, the
    /// "no tests" message, configuration errors).
    pub stderr: String,
}

/// Every reporter event of one suite, as the runner wrote it: its `suite`
/// event, its groups, and the start, output, errors and end of each of its
/// tests (the hidden load pseudo-test included). Event ids are the
/// runner's own, and `time` fields are relative to the start of the run
/// that produced them.
#[derive(Debug, Clone, PartialEq)]
pub struct SuiteBlock {
    /// The suite path as the reporter spelled it.
    pub path: String,
    pub events: Vec<crate::event::Json>,
    /// The reporter wrote this suite's last event: the suite closed (the
    /// grouped runner's per-suite marker) or the run reached `done`.
    pub closed: bool,
    /// The grouped Flutter runner vouched that the suite's tester wrote
    /// nothing outside its own markers while the suite ran; `false` for
    /// every other runner, whose output is the runner's own to print.
    pub clean: bool,
}

/// Output of a run that no suite owns.
#[derive(Debug, Clone, PartialEq)]
pub enum Loose {
    /// A reporter event naming no suite.
    Event(crate::event::Json),
    /// A line on stdout that is not a reporter event.
    Line(String),
}

/// Run `dart test --reporter json` on `files`.
///
/// The child runs in its own process group; past the timeout the whole
/// group is killed — the runner and the compiler/test processes it
/// spawned — so a hung suite cannot outlive the deadline, and a silent
/// hang (no output at all) is caught as reliably as a chatty one.
///
/// A selection whose paths would overflow the platform's argument limit
/// runs as consecutive invocations of at most [`ARGV_BUDGET`] path bytes
/// each, merged into one [`Execution`]; every suite runs exactly once.
pub fn execute(files: &[PathBuf], cfg: &ExecConfig) -> Result<Execution, ExecError> {
    batched(files, |batch| run_command(build_command(batch, cfg)?, cfg))
}

/// [`execute`] through a compiled [`crate::GroupRunner`] instead of
/// `dart test`: the same JSON reporter stream, the same arguments.
pub fn execute_grouped(
    group: &crate::GroupRunner,
    files: &[PathBuf],
    cfg: &ExecConfig,
) -> Result<Execution, ExecError> {
    batched(files, |batch| {
        let mut cmd = Command::new(&cfg.dart_bin);
        cmd.args(group.args())
            .args(["--reporter", "json"])
            .args(batch.iter().map(|p| p.as_os_str()))
            .args(&cfg.extra_args)
            .current_dir(&cfg.cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        run_command(cmd, cfg)
    })
}

/// [`execute`] through `flutter test --reporter json` (the `flutter`
/// entry point in `cfg.dart_bin`): the same JSON reporter stream. Lines
/// the tool itself writes to stdout around the reporter (its device
/// events under `--machine`, `Shell:` relays of a tester's raw output)
/// are loose lines.
pub fn execute_flutter(files: &[PathBuf], cfg: &ExecConfig) -> Result<Execution, ExecError> {
    batched(files, |batch| {
        let mut cmd = Command::new(&cfg.dart_bin);
        cmd.args(["test", "--reporter", "json"])
            .args(&cfg.extra_args)
            .arg("--")
            .args(batch.iter().map(|p| p.as_os_str()))
            .current_dir(&cfg.cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        run_command(cmd, cfg)
    })
}

/// Path bytes one invocation passes on its command line. macOS allows
/// 1 MiB of arguments and environment together, Linux 2 MiB with a
/// 128 KiB cap per argument; a quarter of the smaller leaves room for any
/// environment.
pub const ARGV_BUDGET: usize = 256 * 1024;

/// Run `files` through `invoke` in consecutive batches within
/// [`ARGV_BUDGET`], merging the results; one batch when they fit.
///
/// Paths are passed exactly as given: the runner reports each suite under
/// the spelling it received, so the caller spells them as the user did.
fn batched(
    files: &[PathBuf],
    mut invoke: impl FnMut(&[PathBuf]) -> Result<Execution, ExecError>,
) -> Result<Execution, ExecError> {
    let mut merged: Option<Execution> = None;
    let mut start = 0;
    while start < files.len() || merged.is_none() {
        let mut end = start;
        let mut bytes = 0;
        while end < files.len() && (end == start || bytes + files[end].as_os_str().len() + 1 <= ARGV_BUDGET) {
            bytes += files[end].as_os_str().len() + 1;
            end += 1;
        }
        let next = invoke(&files[start..end])?;
        merged = Some(match merged {
            None => next,
            Some(mut all) => {
                all.complete &= next.complete;
                all.suites.extend(next.suites);
                all.peak_rss = all.peak_rss.max(next.peak_rss);
                all.blocks.extend(next.blocks);
                all.loose.extend(next.loose);
                all.stderr.push_str(&next.stderr);
                all
            }
        });
        start = end;
    }
    Ok(merged.expect("at least one batch ran"))
}

/// Run `cmd` (a reporter-JSON runner) under the watchdog: see [`execute`].
pub(crate) fn run_command_for(cmd: Command, cfg: &ExecConfig) -> Result<Execution, ExecError> {
    run_command(cmd, cfg)
}

fn run_command(mut cmd: Command, cfg: &ExecConfig) -> Result<Execution, ExecError> {
    let timeout = cfg.timeout.unwrap_or(RUN_TIMEOUT);

    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut child = cmd.spawn().map_err(ExecError::Spawn)?;
    let _group = crate::host::Group::enter(child.id());
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take();
    let spawned = Instant::now();
    let sink = cfg.sink.clone();
    let ready = cfg.ready.clone();

    let activity = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let seen = std::sync::Arc::clone(&activity);
    let reader = std::thread::spawn(move || {
        let lines = Activity { inner: stdout, seen, spawned };
        parse_stream(BufReader::new(lines), sink.as_ref(), ready, spawned)
    });
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut stderr) = stderr {
            let _ = std::io::Read::read_to_string(&mut stderr, &mut text);
        }
        text
    });
    let mut peak_rss = 0u64;
    let mut sampled = Instant::now() - RSS_SAMPLE;
    loop {
        if child.try_wait().map_err(ExecError::Wait)?.is_some() {
            break;
        }
        if sampled.elapsed() >= RSS_SAMPLE {
            sampled = Instant::now();
            peak_rss = peak_rss.max(group_rss(child.id()));
        }
        let idle_since = spawned + Duration::from_millis(activity.load(std::sync::atomic::Ordering::Relaxed));
        if idle_since.elapsed() > timeout {
            kill_group(&mut child);
            // Reap the runner; the reader sees EOF once the group is gone.
            let _ = child.wait();
            let _ = reader.join();
            let _ = errors.join();
            return Err(ExecError::Timeout(timeout));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    // The runner exited; anything it left behind holding the pipe open
    // (a detached compiler) must not keep us reading forever.
    kill_group(&mut child);
    let parsed = reader.join().expect("the reporter parser does not panic")?;
    let stderr = errors.join().unwrap_or_default();

    Ok(Execution {
        complete: parsed.done,
        suites: parsed.suites,
        peak_rss,
        blocks: parsed.blocks,
        loose: parsed.loose,
        stderr,
    })
}

/// A reader that records when it last returned bytes (milliseconds since
/// `spawned`), feeding the idle watchdog.
struct Activity<R> {
    inner: R,
    seen: std::sync::Arc<std::sync::atomic::AtomicU64>,
    spawned: Instant,
}

impl<R: std::io::Read> std::io::Read for Activity<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        if read > 0 {
            let now = u64::try_from(self.spawned.elapsed().as_millis()).unwrap_or(u64::MAX);
            self.seen.store(now, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(read)
    }
}

/// How often the run's resident size is sampled.
const RSS_SAMPLE: Duration = Duration::from_secs(1);

/// Resident bytes of every process in the process group `pgid` (the runner
/// and everything it spawned: compiler, isolate hosts). Read through `ps`,
/// which every unix ships; one call per sample.
#[cfg(unix)]
fn group_rss(pgid: u32) -> u64 {
    let Ok(out) = Command::new("ps")
        .args(["-A", "-o", "pgid=,rss="])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return 0;
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let group: u32 = fields.next()?.parse().ok()?;
            let kib: u64 = fields.next()?.parse().ok()?;
            (group == pgid).then_some(kib * 1024)
        })
        .sum()
}

#[cfg(not(unix))]
fn group_rss(_pgid: u32) -> u64 {
    0
}

/// How often the watchdog checks the runner.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Kill the runner's whole process group (unix) or the runner (elsewhere).
fn kill_group(child: &mut Child) {
    #[cfg(unix)]
    {
        if let Ok(pid) = libc::pid_t::try_from(child.id()) {
            // SAFETY: plain syscall on a group id we created; a group that
            // is already gone makes it fail with ESRCH, which is fine.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}

/// `dart test --reporter json <files> <extra args>`.
fn build_command(files: &[PathBuf], cfg: &ExecConfig) -> Result<Command, ExecError> {
    let mut cmd = Command::new(&cfg.dart_bin);
    cmd.args(["test", "--reporter", "json"])
        .args(files.iter().map(|p| p.as_os_str()))
        .args(&cfg.extra_args)
        .current_dir(&cfg.cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Ok(cmd)
}

/// What the reporter stream yielded.
struct Parsed {
    done: bool,
    suites: Vec<PathBuf>,
    blocks: Vec<SuiteBlock>,
    loose: Vec<Loose>,
}

/// Parse stdout as NDJSON events until EOF. Non-JSON lines (a test's raw
/// stdout writes interleave with the reporter) are skipped.
fn parse_stream(
    reader: BufReader<impl std::io::Read>,
    sink: Option<&std::sync::mpsc::Sender<Streamed>>,
    mut ready: Option<std::sync::mpsc::Sender<()>>,
    spawned: Instant,
) -> Result<Parsed, ExecError> {
    let mut order: Vec<PathBuf> = Vec::new();
    let mut done = false;
    // A block handed to the sink leaves `None` behind.
    let mut blocks: Vec<Option<SuiteBlock>> = Vec::new();
    let mut loose: Vec<Loose> = Vec::new();
    // Suite id → its block; test id → its suite id.
    let mut block_of: FxHashMap<u64, usize> = FxHashMap::default();
    let mut suite_of_test: FxHashMap<u64, u64> = FxHashMap::default();
    let mut block_at: FxHashMap<String, usize> = FxHashMap::default();
    // The block that received the latest suite event and is still held: a
    // raw line interleaved with it is the suite's own (at the runner's
    // concurrency of one, exactly), so it joins the block at its position —
    // a suite's block renders as a unit, and the line prints where the
    // runner wrote it.
    let mut streaming: Option<usize> = None;

    for line in reader.lines() {
        let line = line?;
        let Some(raw) = crate::event::parse(&line) else {
            if !line.is_empty() {
                match streaming.and_then(|index| blocks[index].as_mut()) {
                    Some(block) => block.events.push(crate::event::shell(&line)),
                    None => match sink {
                        Some(sink) => {
                            let _ = sink.send(Streamed::Line(line));
                        }
                        None => loose.push(Loose::Line(line)),
                    },
                }
            }
            continue;
        };
        match raw.kind() {
            "start" | "allSuites" => {}
            "done" => {
                done = true;
                for block in blocks.iter_mut().flatten() {
                    block.closed = true;
                }
            }
            "dartq.suiteDone" => {
                if let Some(&index) = raw.str_at(&["path"]).and_then(|path| block_at.get(path)) {
                    if let Some(block) = blocks[index].as_mut() {
                        block.closed = true;
                        block.clean = raw.get("clean").and_then(crate::event::Json::as_bool).unwrap_or(false);
                    }
                    if let Some(sink) = sink {
                        if let Some(block) = blocks[index].take() {
                            let _ = sink.send(Streamed::Settled(Settled { block, spawned }));
                        }
                    }
                }
                continue;
            }
            "suite" => {
                if let Some(ready) = ready.take() {
                    let _ = ready.send(());
                }
                let path = raw.str_at(&["suite", "path"]).unwrap_or("").to_owned();
                let id = raw.u64_at(&["suite", "id"]).unwrap_or(u64::MAX);
                order.push(PathBuf::from(&path));
                block_of.insert(id, blocks.len());
                block_at.insert(path.clone(), blocks.len());
                streaming = Some(blocks.len());
                blocks.push(Some(SuiteBlock { path, events: vec![raw], closed: false, clean: false }));
            }
            _ => {
                if let Some(test) = raw.u64_at(&["test", "id"]) {
                    if let Some(suite) = raw.u64_at(&["test", "suiteID"]) {
                        suite_of_test.insert(test, suite);
                    }
                }
                let suite = crate::event::suite_of(&raw)
                    .or_else(|| raw.u64_at(&["testID"]).and_then(|test| suite_of_test.get(&test).copied()));
                let placed = suite.and_then(|suite| block_of.get(&suite)).copied();
                if let Some(index) = placed
                    && let Some(block) = blocks[index].as_mut()
                {
                    block.events.push(raw);
                    streaming = Some(index);
                } else {
                    loose.push(Loose::Event(raw));
                }
            }
        }
    }

    Ok(Parsed {
        done,
        suites: order,
        blocks: settle(blocks, sink, spawned),
        loose,
    })
}

/// The blocks still held at the end of the stream: handed to `sink` when
/// there is one (the caller then holds none), else returned.
fn settle(
    blocks: Vec<Option<SuiteBlock>>,
    sink: Option<&std::sync::mpsc::Sender<Streamed>>,
    spawned: Instant,
) -> Vec<SuiteBlock> {
    let held = blocks.into_iter().flatten();
    match sink {
        Some(sink) => {
            for block in held {
                let _ = sink.send(Streamed::Settled(Settled { block, spawned }));
            }
            Vec::new()
        }
        None => held.collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn execution(files: &[PathBuf]) -> Execution {
        Execution {
            complete: true,
            suites: files.to_vec(),
            peak_rss: 0,
            blocks: Vec::new(),
            loose: Vec::new(),
            stderr: String::new(),
        }
    }

    /// A raw line interleaved with a suite's events joins that suite's
    /// block at the runner's own position (so it renders in place); a line
    /// of no suite in flight reaches the sink the moment it was written.
    #[test]
    fn raw_lines_join_the_streaming_suite() {
        let stream = "\
runner banner\n\
{\"suite\":{\"id\":0,\"path\":\"test/a_test.dart\"},\"type\":\"suite\",\"time\":0}\n\
raw from a\n\
{\"test\":{\"id\":1,\"name\":\"a\",\"suiteID\":0},\"type\":\"testStart\",\"time\":1}\n\
{\"testID\":1,\"result\":\"success\",\"skipped\":false,\"hidden\":false,\"type\":\"testDone\",\"time\":2}\n\
between suites\n\
{\"suite\":{\"id\":2,\"path\":\"test/b_test.dart\"},\"type\":\"suite\",\"time\":3}\n\
{\"test\":{\"id\":3,\"name\":\"b\",\"suiteID\":2},\"type\":\"testStart\",\"time\":3}\n\
raw from b\n\
{\"testID\":3,\"result\":\"success\",\"skipped\":false,\"hidden\":false,\"type\":\"testDone\",\"time\":4}\n\
{\"success\":true,\"type\":\"done\",\"time\":5}\n";
        let (sender, receiver) = std::sync::mpsc::channel();
        parse_stream(
            BufReader::new(std::io::Cursor::new(stream.as_bytes().to_vec())),
            Some(&sender),
            None,
            Instant::now(),
        )
        .expect("parse");
        drop(sender);
        let streamed: Vec<Streamed> = receiver.into_iter().collect();
        // The runner banner (before any suite event) streams on its own.
        assert!(matches!(&streamed[0], Streamed::Line(line) if line == "runner banner"));
        let Streamed::Settled(Settled { block: a, .. }) = &streamed[1] else { panic!("settled") };
        assert_eq!(a.events.len(), 5);
        assert_eq!(a.events[1].kind(), crate::event::SHELL);
        assert_eq!(a.events[1].str_at(&["message"]), Some("raw from a"));
        // A line between suites still belongs to the suite that streamed
        // last: it renders with that suite, never after the run.
        assert_eq!(a.events[4].kind(), crate::event::SHELL);
        assert_eq!(a.events[4].str_at(&["message"]), Some("between suites"));
        let Streamed::Settled(Settled { block: b, .. }) = &streamed[2] else { panic!("settled") };
        // `raw from b` sits between b's testStart and its testDone.
        assert_eq!(b.events[2].kind(), crate::event::SHELL);
        assert_eq!(b.events[3].kind(), "testDone");

        // Without a sink the lines keep their order in `loose`.
        let parsed = parse_stream(
            BufReader::new(std::io::Cursor::new(stream.as_bytes().to_vec())),
            None,
            None,
            Instant::now(),
        )
        .expect("parse");
        assert!(parsed.blocks[0].events.iter().any(|event| event.kind() == crate::event::SHELL));
        assert!(matches!(&parsed.loose[0], Loose::Line(line) if line == "runner banner"));
    }

    /// A selection past the argument budget splits into consecutive
    /// invocations that each fit, cover every suite once, and merge.
    #[test]
    fn oversized_selections_run_in_batches() {
        let long = "x".repeat(300);
        let files: Vec<PathBuf> = (0..2000).map(|i| PathBuf::from(format!("/ws/{long}/{i}_test.dart"))).collect();
        let mut calls = Vec::new();
        let merged = batched(&files, |batch| {
            let bytes: usize = batch.iter().map(|p| p.as_os_str().len() + 1).sum();
            assert!(bytes <= ARGV_BUDGET, "{bytes}");
            calls.push(batch.len());
            Ok(execution(batch))
        })
        .expect("runs");
        assert!(calls.len() > 1, "{calls:?}");
        assert_eq!(calls.iter().sum::<usize>(), files.len());
        assert_eq!(merged.suites, files);
        assert!(merged.complete);

        // A small selection is one invocation; an empty one still runs once.
        let mut count = 0;
        batched(&files[..3], |batch| {
            count += 1;
            Ok(execution(batch))
        })
        .expect("runs");
        assert_eq!(count, 1);
        let mut empty = 0;
        batched(&[], |batch| {
            empty += 1;
            Ok(execution(batch))
        })
        .expect("runs");
        assert_eq!(empty, 1);
    }

    /// The grouped runner's per-suite marker settles a block the moment
    /// its suite closes — carrying its clean flag — while a plain run's
    /// blocks close at `done`; an event of no suite is loose.
    #[test]
    fn suite_done_markers_settle_blocks_early() {
        let stream = "\
{\"suite\":{\"id\":0,\"path\":\"test/a_test.dart\"},\"type\":\"suite\",\"time\":0}\n\
{\"test\":{\"id\":1,\"name\":\"loading test/a_test.dart\",\"suiteID\":0,\"groupIDs\":[]},\"type\":\"testStart\",\"time\":0}\n\
{\"testID\":1,\"result\":\"success\",\"skipped\":false,\"hidden\":true,\"type\":\"testDone\",\"time\":1}\n\
{\"type\":\"dartq.suiteDone\",\"path\":\"test/a_test.dart\",\"clean\":true}\n\
{\"suite\":{\"id\":2,\"path\":\"test/b_test.dart\"},\"type\":\"suite\",\"time\":2}\n\
{\"test\":{\"id\":3,\"name\":\"b\",\"suiteID\":2,\"groupIDs\":[]},\"type\":\"testStart\",\"time\":2}\n\
{\"testID\":3,\"result\":\"success\",\"skipped\":false,\"hidden\":false,\"type\":\"testDone\",\"time\":3}\n\
{\"testID\":9,\"result\":\"success\",\"skipped\":false,\"hidden\":false,\"type\":\"testDone\",\"time\":3}\n\
{\"success\":true,\"type\":\"done\",\"time\":4}\n";
        let (sender, receiver) = std::sync::mpsc::channel();
        let parsed = parse_stream(
            BufReader::new(std::io::Cursor::new(stream.as_bytes().to_vec())),
            Some(&sender),
            None,
            Instant::now(),
        )
        .expect("parse");
        drop(sender);
        assert!(parsed.blocks.is_empty(), "every block reached the sink");
        let streamed: Vec<Streamed> = receiver.into_iter().collect();
        let Streamed::Settled(Settled { block: a, .. }) = &streamed[0] else { panic!("settled") };
        assert!(a.closed && a.clean, "the marker closed the suite clean");
        assert_eq!(a.events.len(), 3, "the marker itself joins no block");
        let Streamed::Settled(Settled { block: b, .. }) = &streamed[1] else { panic!("settled") };
        assert!(b.closed, "`done` closes the suites still held");
        assert!(!b.clean);

        // Without a sink the same stream keeps its blocks; the orphan
        // event belongs to no suite.
        let parsed = parse_stream(
            BufReader::new(std::io::Cursor::new(stream.as_bytes().to_vec())),
            None,
            None,
            Instant::now(),
        )
        .expect("parse");
        assert!(parsed.blocks[0].closed && parsed.blocks[0].clean);
        assert!(parsed.blocks[1].closed && !parsed.blocks[1].clean);
        assert!(matches!(&parsed.loose[0], Loose::Event(event) if event.kind() == "testDone"));
    }
}
