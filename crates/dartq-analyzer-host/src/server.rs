//! One official analysis server child, driven exactly as `dart analyze`
//! drives it: the same launch flags, `server.setSubscriptions[STATUS]`,
//! `analysis.setAnalysisRoots` over the realpath'd targets, and analysis
//! complete at the `server.status` that turns `isAnalyzing` false. Per
//! file, the latest `analysis.errors` list replaces the previous one, which
//! is the error map dartdev reports from.
//!
//! Between requests the child stays up. A stat diff ([`crate::snapshot`])
//! tells the host what changed on disk; [`Server::changed`] hands Dart
//! edits to the server as content overlays — the file's bytes as of the
//! push as an `AddContentOverlay`, with a `RemoveContentOverlay` fallback
//! for a file that cannot be read back (either one is a `changeFile` in
//! the server), and
//! Additions, deletions and configuration edits get a fresh server (the
//! daemon restarts it): `analysis.reanalyze` rebuilds the contexts
//! asynchronously after its response, and the server's own file watcher
//! interleaves flushes and analysis cycles of its own, so no event marks
//! the rebuilt analysis reliably. A fresh server is `dart analyze` by
//! construction.
//!
//! A server also exists in a standby state between borrows:
//! [`Server::spawn_and_connect`] leaves it spawned and connected with no
//! roots (optionally armed by one tiny roots pass that warms the
//! server-lifetime byte store), and [`Server::adopt`] turns it into the
//! next borrower's serving server with the same first roots assignment a
//! fresh spawn runs.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use rustc_hash::FxHashMap;
use serde::Deserialize;
use serde_json::{Value, json};

#[cfg(unix)]
use crate::face::{Face, FaceRenderer};
use crate::report::{Report, Row, severity_rank};
use crate::support::{Sdk, Version};

/// How long the server may take to announce itself.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
/// The most one request may take end-to-end: a request whose server never
/// answers (alive but wedged, or analyzing forever) is a fault after this —
/// `analyze_result` restarts the server once, then the caller falls back.
/// Generous: the largest monorepo's first analysis can take many minutes.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60 * 60);
/// After a request's response, how long a server that never reports
/// `isAnalyzing: true` is given before the request counts as no work. The
/// server queues the status event before it answers (the handlers run the
/// file change or the context rebuild first), so the window only absorbs
/// event-loop latency.
const NO_WORK_GRACE: Duration = Duration::from_secs(2);
/// The RSS sample cadence while an analysis runs.
const RSS_POLL: Duration = Duration::from_secs(1);
/// How long `server.shutdown` may take before the tree is killed.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Why a server cannot answer. Every variant but [`Fault::Budget`],
/// [`Fault::Memory`] and [`Fault::Yield`] sends the caller to a per-run
/// analysis.
#[derive(Debug, Clone, PartialEq)]
pub enum Fault {
    /// The server did not start, crashed, answered a request with an
    /// error, or reported a `server.error`/`server.pluginError`.
    Server(String),
    /// The server's process tree crossed the RSS budget and was killed.
    Budget { budget: u64, measured: u64 },
    /// The server never got its memory lease: the machine-wide ledger's
    /// wait timed out (the message names the holders).
    Memory(String),
    /// The growth brake fired: the host sat under the critical floor and
    /// this job was the youngest active heavy holder. `peak` is the
    /// observed process-tree peak — the daemon books it as the job's
    /// lease before answering `Answer::Yield`, so the re-queued client is
    /// admitted on the honest figure, never a per-run fallback (a per-run
    /// analysis would need the same memory).
    Yield { peak: u64 },
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Server(message) | Self::Memory(message) => f.write_str(message),
            Self::Budget { budget, measured } => write!(
                f,
                "analyzer exceeded the memory budget ({} GB configured; measured {:.1} GB) — \
                 killed; raise analyzer.official.memory_budget_gb in dartq.yaml or retry",
                budget / (1 << 30),
                *measured as f64 / f64::from(1u32 << 30)
            ),
            Self::Yield { .. } => f.write_str("the analyzer yielded under memory pressure"),
        }
    }
}

/// The fixed launch facts of one server.
#[derive(Clone, Debug)]
pub struct Launch {
    pub sdk: Sdk,
    pub packages: Option<PathBuf>,
}

impl Launch {
    /// dartdev's argument list (`AnalysisServer._startProcess`).
    ///
    /// No `--cache`: dartdev leaves the flag unset, so the server picks
    /// `PhysicalResourceProvider`'s own state location — `~/.dartServer` on
    /// POSIX, the same store every `dart`/`flutter` invocation on the host
    /// already shares. Seeding it once per SDK buys every later analysis —
    /// stock's and this daemon's alike — the linked-bundle hits that a
    /// fresh per-worktree `--cache` dir can never serve. On a monorepo the
    /// difference between a warm `~/.dartServer` and an empty per-worktree
    /// dir is the full re-link of every package cycle plus the
    /// `EvictingFileByteStore` write tax (~10× wall on a 50k-file corpus).
    /// The shared store's URI-keyed context-message caveat is upstream's
    /// own (`dart analyze` carries it too); per-worktree isolation was a
    /// nicety, not a correctness requirement, and it priced the cold path
    /// out of parity with the oracle.
    fn command(&self) -> Command {
        let (runtime, snapshot) = self.sdk.server_command();
        let mut command = Command::new(runtime);
        command
            .arg(snapshot)
            .args([
                "--suppress-analytics",
                "--client-id=dart-analyze",
                "--disable-server-feature-completion",
                "--disable-server-feature-search",
                "--disable-status-notification-debouncing",
                "--disable-silent-analysis-exceptions",
                "--sdk",
            ])
            .arg(&self.sdk.root);
        if let Some(packages) = &self.packages {
            command.arg(format!("--packages={}", packages.display()));
        }
        command
    }
}

/// One file's pushed error list, with the rendered row fragments the
/// answer line reuses while the list is unchanged (`None` = stale, the
/// next render rebuilds them — the error list's own edit epoch).
struct FileErrors {
    rows: Vec<Row>,
    segs: Option<FileSeg>,
}

/// The per-file half of the answer splice ([`State::render`]): the row
/// fragments this file contributes to the report — the kept rows' sort
/// facts and serialized bytes, plus the file's `priority_errors` count.
/// Sorted order never crosses files mid-row, so a changed file's splice
/// is a re-render of its fragments and a re-sort of the row set; every
/// other file's bytes ride along untouched.
struct FileSeg {
    /// The file's name in UTF-16 — the sort key's file leg, shared by
    /// every row of the file. Boxed: a `Vec`'s capacity slack is dead
    /// bytes per file for the daemon's life.
    file16: Box<[u16]>,
    /// The rows the report keeps (TODO-info dropped, priority errors
    /// counted out), in pushed order.
    kept: Box<[RowSeg]>,
    /// This file's contribution to `priority_errors`.
    priority: usize,
}

/// One kept row's sort facts — the report line serializes the row it
/// indexes, so the daemon holds one representation per diagnostic (the
/// `Row`), not a rendered copy beside it.
struct RowSeg {
    /// The row's index in the file's pushed list.
    row: u32,
    rank: u8,
    offset: u32,
    /// The message in UTF-16 — the sort key's last leg.
    msg16: Box<[u16]>,
}

/// The rendered answer of one (aims, face) shape.
struct CachedLine {
    /// The error-map epoch the bytes were rendered at.
    epoch: u64,
    /// The full wire bytes, ready to ship (the `Answer::Report` line, or
    /// the `Answer::Face` header line's newline followed by the payload).
    bytes: std::sync::Arc<[u8]>,
}

/// The answer cache: one rendered answer per (aims, face) shape,
/// validated against the error-map epoch, LRU-bounded on entries and on
/// total bytes so a client that cycles shapes or faces cannot grow it
/// without bound. The face bytes subsumed the report line for the routed
/// CLI — one shared cache, keyed by the shape pair, keeps the lowest
/// memory.
#[derive(Default)]
struct AnswerCache {
    /// `((aims-key, face-key), line)` pairs, least recently used first.
    entries: std::collections::VecDeque<((Vec<u8>, Vec<u8>), CachedLine)>,
    /// Live `bytes` sum across the entries.
    bytes_total: usize,
}

impl AnswerCache {
    /// The most shapes cached at once.
    const CAP: usize = 8;
    /// The most payload bytes cached across all entries (64 MiB).
    const BYTES_CAP: usize = 64 << 20;

    fn get(&mut self, key: &(Vec<u8>, Vec<u8>)) -> Option<&CachedLine> {
        let index = self.entries.iter().position(|(shape, _)| shape == key)?;
        let entry = self.entries.remove(index).expect("positioned");
        self.entries.push_back(entry);
        Some(&self.entries.back().expect("just pushed").1)
    }

    fn insert(&mut self, key: (Vec<u8>, Vec<u8>), line: CachedLine) {
        if let Some(index) = self.entries.iter().position(|(shape, _)| *shape == key) {
            self.bytes_total -= self.entries[index].1.bytes.len();
            self.entries.remove(index);
        }
        // An answer that alone exceeds the byte cap is not cacheable: it
        // is shipped once, never stored.
        if line.bytes.len() > Self::BYTES_CAP {
            return;
        }
        while self.entries.len() >= Self::CAP
            || self.bytes_total + line.bytes.len() > Self::BYTES_CAP
        {
            if let Some((_, evicted)) = self.entries.pop_front() {
                self.bytes_total -= evicted.bytes.len();
            } else {
                break;
            }
        }
        self.bytes_total += line.bytes.len();
        self.entries.push_back((key, line));
    }
}

#[derive(Default)]
struct State {
    files: FxHashMap<Arc<str>, FileErrors>,
    analyzing: bool,
    busy: u64,
    idle: u64,
    responses: FxHashMap<u64, Result<(), String>>,
    connected: Option<String>,
    fault: Option<String>,
    /// The error map's version: bumped on every pushed `analysis.errors`
    /// and `analysis.flushResults`, even when the pushed content is
    /// identical — the answer cache's validator.
    epoch: u64,
    /// The rendered `Answer::Report` lines, one per aims shape.
    answers: AnswerCache,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

/// The status counters before a request, the baseline [`Server::settle`]
/// waits past.
#[derive(Clone, Copy)]
struct Mark {
    busy: u64,
    idle: u64,
}

/// A live analysis server child.
pub struct Server {
    child: Child,
    stdin: ChildStdin,
    shared: Arc<Shared>,
    next_id: u64,
    budget: Option<u64>,
    peak: u64,
    /// The facts the child was spawned under — an adoption's validation
    /// input (a standby serves the launch it was spawned for, no other).
    launch: Launch,
}

impl Server {
    /// Starts the server, sets its roots and waits for the first analysis.
    /// `budget` (bytes, `None` unbounded) caps the process tree's RSS
    /// while analyses run. `brake` is the growth-brake sample — `true`
    /// kills the analysis with [`Fault::Yield`].
    ///
    /// # Errors
    ///
    /// [`Fault`] when the server does not start, the handshake fails, or
    /// the first analysis does not finish.
    pub fn start(
        launch: &Launch,
        roots: &[PathBuf],
        budget: Option<u64>,
        mut brake: impl FnMut() -> bool,
    ) -> Result<Self, Fault> {
        let mut server = Self::spawn_and_connect(launch, budget)?;
        let settled = server.set_roots_initial(roots, &mut brake);
        if let Err(fault) = settled {
            server.kill();
            return Err(fault);
        }
        // The full analysis just ended: the heap is near its peak even when
        // the analysis was shorter than one sample period.
        server.peak = server.peak.max(tree_rss(server.pid()));
        Ok(server)
    }

    /// Spawns the server child and waits for its `server.connected`: the
    /// standby state — a live server with no subscriptions and no roots,
    /// one `analysis.setAnalysisRoots` away from serving any borrower
    /// ([`Server::adopt`]). The protocol check is the gate the handshake
    /// always was: a protocol the daemon does not know never serves.
    ///
    /// # Errors
    ///
    /// [`Fault`] when the server does not spawn or never connects.
    pub fn spawn_and_connect(launch: &Launch, budget: Option<u64>) -> Result<Self, Fault> {
        let mut command = launch.command();
        // The server stays in its parent's process group: the daemon runs in
        // a group of its own and takes the whole group down with it.
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| Fault::Server(format!("cannot start the analysis server: {error}")))?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let shared = Arc::new(Shared::default());
        let reader = Arc::clone(&shared);
        std::thread::spawn(move || read_messages(stdout, &reader));
        // The server's stderr is drained so a chatty server never blocks.
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut stderr, &mut std::io::sink());
        });
        let server = Self {
            child,
            stdin,
            shared,
            next_id: 0,
            budget,
            peak: 0,
            launch: launch.clone(),
        };
        server.await_connected(launch.sdk.version)?;
        Ok(server)
    }

    /// Adopts a standby as this borrower's serving server: the launch
    /// facts and protocol the standby was spawned under are re-validated,
    /// and the first roots assignment runs exactly as a fresh spawn's
    /// ([`Server::start`]) — the standby is rootless by construction
    /// (spawn + connected, never subscribed, never served), so its
    /// answers are the fresh spawn's by construction: the borrower's
    /// roots build is the server's first and only context. The standby
    /// carries its own budget from [`Server::spawn_and_connect`] — the
    /// one shared daemon config.
    ///
    /// # Errors
    ///
    /// [`Fault`] as in [`Server::start`] (a failed adoption leaves the
    /// child killed; the caller spawns fresh).
    pub fn adopt(
        mut standby: Self,
        launch: &Launch,
        roots: &[PathBuf],
        mut brake: impl FnMut() -> bool,
    ) -> Result<Self, Fault> {
        if standby.launch.sdk != launch.sdk || standby.launch.packages != launch.packages {
            standby.kill();
            return Err(Fault::Server(
                "the standby server was spawned for another SDK".into(),
            ));
        }
        let sdk = launch.sdk.version;
        standby.await_connected(sdk)?;
        let settled = standby.set_roots_initial(roots, &mut brake);
        if let Err(fault) = settled {
            standby.kill();
            return Err(fault);
        }
        standby.peak = standby.peak.max(tree_rss(standby.pid()));
        Ok(standby)
    }

    /// Waits for `server.connected` and gates its protocol version.
    fn await_connected(&self, sdk: Version) -> Result<(), Fault> {
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        let mut state = self.shared.state.lock();
        while state.connected.is_none() && state.fault.is_none() {
            if self
                .shared
                .changed
                .wait_until(&mut state, deadline)
                .timed_out()
            {
                return Err(Fault::Server("the analysis server never connected".into()));
            }
        }
        if let Some(fault) = &state.fault {
            return Err(Fault::Server(fault.clone()));
        }
        // `server.connected` carries the protocol version, not the SDK
        // version: a protocol the gate does not know is a per-run run.
        let protocol = state.connected.as_deref().unwrap_or_default();
        if Version::parse(protocol).is_none_or(|v| v.0 != 1) {
            return Err(Fault::Server(format!(
                "Dart {sdk}: unexpected analysis server protocol {protocol:?}"
            )));
        }
        Ok(())
    }

    /// The first roots assignment: subscriptions on, `analysis.
    /// setAnalysisRoots` over `roots`, the settle past it. A fresh spawn's
    /// handshake tail, and an adoption's whole arm.
    pub(crate) fn set_roots_initial(
        &mut self,
        roots: &[PathBuf],
        brake: &mut dyn FnMut() -> bool,
    ) -> Result<(), Fault> {
        self.send(
            "server.setSubscriptions",
            json!({"subscriptions": ["STATUS"]}),
        )?;
        let included: Vec<String> = roots.iter().map(|root| root_spelling(root)).collect();
        let mark = self.mark();
        self.call(
            "analysis.setAnalysisRoots",
            json!({"included": included, "excluded": []}),
        )?;
        self.settle(mark, brake)
    }

    /// Replaces the server's analysis roots — the pooled daemon's
    /// root-set change (a join adds, an eviction drops) — and waits out
    /// the re-analysis of the new set.
    ///
    /// # Errors
    ///
    /// [`Fault`] as in [`Server::start`].
    pub fn set_roots(
        &mut self,
        roots: &[PathBuf],
        mut brake: impl FnMut() -> bool,
    ) -> Result<(), Fault> {
        let included: Vec<String> = roots.iter().map(|root| root_spelling(root)).collect();
        let mark = self.mark();
        self.call(
            "analysis.setAnalysisRoots",
            json!({"included": included, "excluded": []}),
        )?;
        self.settle(mark, &mut brake)?;
        // The served set changed (a join or an eviction): the pushed
        // notifications during the re-analysis already bumped the epoch,
        // and this drop is the belt to that suspender — a line rendered
        // for the old root set is never this pool's answer again.
        let answers = &mut self.shared.state.lock().answers;
        answers.entries.clear();
        answers.bytes_total = 0;
        Ok(())
    }

    /// Hands changed Dart files to the server and waits for the analysis.
    ///
    /// # Errors
    ///
    /// [`Fault`] as in [`Server::start`].
    pub fn changed(
        &mut self,
        files: &[PathBuf],
        mut brake: impl FnMut() -> bool,
    ) -> Result<(), Fault> {
        // Each changed file's bytes as of this push (the caller read the
        // freshness stat just above): an add overlay pins exactly what
        // was on disk, so the server never analyzes a half-written save;
        // a file that cannot be read back (deleted mid-request, or not
        // UTF-8) falls back to the remove overlay and the server re-reads
        // it from disk. Each entry is a `changeFile` either way.
        let changes: serde_json::Map<String, Value> = files
            .iter()
            .map(|file| {
                let overlay = match std::fs::read_to_string(file) {
                    Ok(content) => json!({"type": "add", "content": content}),
                    Err(_) => json!({"type": "remove"}),
                };
                (file.display().to_string(), overlay)
            })
            .collect();
        let mark = self.mark();
        self.call("analysis.updateContent", json!({"files": changes}))?;
        self.settle(mark, &mut brake)
    }

    /// Waits out an analysis the server started on its own (its file
    /// watcher), so a report never reads a map mid-analysis.
    ///
    /// # Errors
    ///
    /// [`Fault`] as in [`Server::start`].
    pub fn wait_idle(&mut self, mut brake: impl FnMut() -> bool) -> Result<(), Fault> {
        let mark = self.mark();
        if !self.shared.state.lock().analyzing {
            return Ok(());
        }
        self.settle(
            Mark {
                busy: mark.busy,
                idle: mark.idle,
            },
            &mut brake,
        )
    }

    /// dartdev's report over the current error map.
    #[must_use]
    pub fn report(&self) -> Report {
        let state = self.shared.state.lock();
        Report::from_files(
            state
                .files
                .iter()
                .map(|(file, errors)| (&**file, errors.rows.as_slice())),
        )
    }

    /// dartdev's report over the error lists of the files `aims` cover —
    /// under a directory aim or identical to a `.dart` file aim, the same
    /// coverage `dart analyze <paths>` emits (a file aim's context root
    /// only ever analyzes `.dart` files). `None` when a `.dart` file aim
    /// names no analyzed file: the daemon's one context cannot serve a
    /// file the options exclude or the roots do not reach, so the caller
    /// falls back to a per-run `dart analyze`, which builds that file's
    /// own context root exactly as the oracle does.
    #[must_use]
    pub fn report_scoped(&self, aims: &[crate::Aim]) -> Option<Report> {
        let state = self.shared.state.lock();
        scoped_report(&state.files, aims)
    }

    /// The serialized `Answer::Report` line for `aims` (empty = the whole
    /// report), newline included — byte-identical to
    /// `serde_json::to_vec(&Answer::Report(report))` over the same error
    /// map by construction: the same rows, the same dartdev order, the
    /// same `priority_errors` sum. `None` in `report_scoped`'s case (a
    /// `.dart` file aim names no analyzed file).
    ///
    /// Two caches skip the rebuild: the whole line, one per aims shape,
    /// valid while the error-map epoch stands; and each file's rendered
    /// row fragments, so an edit re-renders only the files the push
    /// touched and re-sorts their keys — the rest of the line is reused.
    #[must_use]
    pub fn render(&self, aims: &[crate::Aim]) -> Option<std::sync::Arc<[u8]>> {
        self.shared.state.lock().render(aims)
    }

    /// The `Answer::Face` wire bytes for `aims` under `face` — the meta
    /// header line's newline followed by the rendered payload, rendered
    /// through `renderer` (the CLI's one writer) over the report's kept
    /// rows in dartdev order. `None` in `render`'s not-served case.
    #[cfg(unix)]
    #[must_use]
    pub fn render_face(
        &self,
        aims: &[crate::Aim],
        face: &Face,
        renderer: FaceRenderer,
    ) -> Option<std::sync::Arc<[u8]>> {
        self.shared.state.lock().render_face(aims, face, renderer)
    }

    #[must_use]
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// The highest process-tree RSS sampled during this server's analyses,
    /// in bytes (`0` on a host without `ps`).
    #[must_use]
    pub fn peak(&self) -> u64 {
        self.peak
    }

    /// `server.shutdown`, then the tree's kill and reap.
    pub fn shutdown(mut self) {
        if let Ok(id) = self.send("server.shutdown", Value::Null) {
            let deadline = Instant::now() + SHUTDOWN_GRACE;
            let mut state = self.shared.state.lock();
            while !state.responses.contains_key(&id) && state.fault.is_none() {
                if self
                    .shared
                    .changed
                    .wait_until(&mut state, deadline)
                    .timed_out()
                {
                    break;
                }
            }
        }
        self.kill();
    }

    fn kill(&mut self) {
        #[cfg(unix)]
        dartq_toolchain::proc::kill_tree(self.child.id());
        #[cfg(not(unix))]
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn mark(&self) -> Mark {
        let state = self.shared.state.lock();
        Mark {
            busy: state.busy,
            idle: state.idle,
        }
    }

    fn send(&mut self, method: &str, params: Value) -> Result<u64, Fault> {
        self.next_id += 1;
        let id = self.next_id;
        let mut line =
            serde_json::to_vec(&json!({"id": id.to_string(), "method": method, "params": params}))
                .expect("a json value serializes");
        line.push(b'\n');
        self.stdin
            .write_all(&line)
            .and_then(|()| self.stdin.flush())
            .map_err(|error| {
                Fault::Server(format!("the analysis server stopped reading: {error}"))
            })?;
        Ok(id)
    }

    /// Sends a request and waits for its response.
    fn call(&mut self, method: &str, params: Value) -> Result<(), Fault> {
        let id = self.send(method, params)?;
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        let mut state = self.shared.state.lock();
        loop {
            if let Some(response) = state.responses.remove(&id) {
                return response
                    .map_err(|error| Fault::Server(format!("{method} failed: {error}")));
            }
            if let Some(fault) = &state.fault {
                return Err(Fault::Server(fault.clone()));
            }
            if self
                .shared
                .changed
                .wait_until(&mut state, deadline)
                .timed_out()
            {
                return Err(Fault::Server(format!(
                    "{method}: the analysis server never answered"
                )));
            }
        }
    }

    /// Waits until the analysis started after `mark` is complete, sampling
    /// the tree's RSS once per [`RSS_POLL`] for the peak, the budget and
    /// the growth brake. The cadence is by the clock, not per wake-up: a
    /// large analysis sends a notification per file. Past
    /// [`REQUEST_TIMEOUT`] the wait is a fault — a server that never
    /// reports idle is wedged, and `analyze_result` restarts it once
    /// before the caller falls back.
    fn settle(&mut self, mark: Mark, brake: &mut dyn FnMut() -> bool) -> Result<(), Fault> {
        let answered = Instant::now();
        let mut sampled = answered;
        let mut state = self.shared.state.lock();
        loop {
            if let Some(fault) = &state.fault {
                return Err(Fault::Server(fault.clone()));
            }
            if !state.analyzing
                && (state.idle > mark.idle
                    || (state.busy == mark.busy && answered.elapsed() >= NO_WORK_GRACE))
            {
                return Ok(());
            }
            if answered.elapsed() >= REQUEST_TIMEOUT {
                return Err(Fault::Server(
                    "the analysis server never finished analyzing".into(),
                ));
            }
            self.shared
                .changed
                .wait_until(&mut state, sampled + RSS_POLL);
            if sampled.elapsed() < RSS_POLL {
                continue;
            }
            sampled = Instant::now();
            let measured = tree_rss(self.child.id());
            self.peak = self.peak.max(measured);
            if let Some(budget) = self.budget
                && measured > budget
            {
                drop(state);
                self.kill();
                return Err(Fault::Budget { budget, measured });
            }
            // The growth brake: the host is under the critical floor and
            // this job is the youngest active heavy holder — the machine
            // comes first. The observed peak rides out so the daemon can
            // book it as the job's lease before the client re-queues.
            if brake() {
                let peak = self.peak;
                drop(state);
                self.kill();
                return Err(Fault::Yield { peak });
            }
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            self.kill();
        }
    }
}

#[cfg(unix)]
fn tree_rss(pid: u32) -> u64 {
    dartq_toolchain::proc::tree_rss_bytes(pid)
}

#[cfg(not(unix))]
fn tree_rss(_pid: u32) -> u64 {
    0
}

/// dartdev's root spelling: the realpath with trailing separators trimmed.
fn root_spelling(root: &Path) -> String {
    let spelled = root.display().to_string();
    let trimmed = spelled.trim_end_matches(std::path::MAIN_SEPARATOR);
    if trimmed.is_empty() {
        spelled
    } else {
        trimmed.to_owned()
    }
}

/// The scoped report over one error map: the covered files' rows, or
/// `None` when a `.dart` file aim names no analyzed file (the per-run
/// fallback signal).
fn scoped_report(files: &FxHashMap<Arc<str>, FileErrors>, aims: &[crate::Aim]) -> Option<Report> {
    for aim in aims {
        if let crate::Aim::File(file) = aim
            && file.extension().is_some_and(|ext| ext == "dart")
            && !files.contains_key(file.display().to_string().as_str())
        {
            return None;
        }
    }
    Some(Report::from_files(files.iter().filter_map(|(file, errors)| {
        crate::covered(Path::new(&**file), aims).then_some((&**file, errors.rows.as_slice()))
    })))
}

impl State {
    /// The report line's face key — the compat arm shares the one cache.
    const REPORT_FACE: &'static [u8] = b"report";

    /// The render behind [`Server::render`]: cache check, per-file
    /// fragment rebuild, sort, splice — all under the one state lock.
    fn render(&mut self, aims: &[crate::Aim]) -> Option<std::sync::Arc<[u8]>> {
        self.served(aims)?;
        let key = (aims_key(aims), Self::REPORT_FACE.to_vec());
        if let Some(hit) = self.answers.get(&key)
            && hit.epoch == self.epoch
        {
            return Some(std::sync::Arc::clone(&hit.bytes));
        }
        // The spliced row's sort view: keys by reference, rows by
        // reference — the sort and the join move no row data, and the
        // write serializes each `Row` in place (the one representation
        // the daemon keeps).
        struct Srow<'a> {
            rank: u8,
            file16: &'a [u16],
            offset: u32,
            msg16: &'a [u16],
            row: &'a Row,
        }
        let mut srows: Vec<Srow<'_>> = Vec::new();
        let mut priority = 0usize;
        for (file, errors) in self.files.iter_mut() {
            if !aims.is_empty() && !crate::covered(Path::new(&**file), aims) {
                continue;
            }
            let rows = &errors.rows;
            let seg = errors
                .segs
                .get_or_insert_with(|| file_seg(file, rows));
            priority += seg.priority;
            srows.extend(seg.kept.iter().map(|frag| Srow {
                rank: frag.rank,
                file16: &seg.file16,
                offset: frag.offset,
                msg16: &frag.msg16,
                row: &rows[frag.row as usize],
            }));
        }
        // dartdev's `AnalysisError.compareTo` over the precomputed legs.
        srows.sort_by(|a, b| {
            a.rank
                .cmp(&b.rank)
                .then_with(|| a.file16.cmp(b.file16))
                .then_with(|| a.offset.cmp(&b.offset))
                .then_with(|| a.msg16.cmp(b.msg16))
        });
        let mut out = Vec::with_capacity(64 * 1024);
        out.extend_from_slice(b"{\"Report\":{\"rows\":[");
        for (index, srow) in srows.iter().enumerate() {
            if index > 0 {
                out.push(b',');
            }
            serde_json::to_writer(&mut out, srow.row).expect("a row serializes");
        }
        out.extend_from_slice(b"],\"priority_errors\":");
        out.extend_from_slice(priority.to_string().as_bytes());
        out.extend_from_slice(b"}}");
        out.push(b'\n');
        let bytes: std::sync::Arc<[u8]> = out.into();
        self.answers.insert(
            key,
            CachedLine {
                epoch: self.epoch,
                bytes: std::sync::Arc::clone(&bytes),
            },
        );
        Some(bytes)
    }

    /// The render behind [`Server::render_face`]: the report's kept rows
    /// in dartdev order through the CLI's writer, wrapped in the
    /// `Answer::Face` header line plus payload.
    #[cfg(unix)]
    fn render_face(
        &mut self,
        aims: &[crate::Aim],
        face: &Face,
        renderer: FaceRenderer,
    ) -> Option<std::sync::Arc<[u8]>> {
        self.served(aims)?;
        let face_key = serde_json::to_vec(face).expect("a face serializes");
        let key = (aims_key(aims), face_key);
        if let Some(hit) = self.answers.get(&key)
            && hit.epoch == self.epoch
        {
            return Some(std::sync::Arc::clone(&hit.bytes));
        }
        // The face's row view: `&Row` references beside the sort legs, so
        // the sort and the writer read the error map, never a clone.
        struct Srow<'a> {
            rank: u8,
            file16: &'a [u16],
            offset: u32,
            msg16: &'a [u16],
            row: &'a Row,
        }
        let mut srows: Vec<Srow<'_>> = Vec::new();
        for (file, errors) in self.files.iter_mut() {
            if !aims.is_empty() && !crate::covered(Path::new(&**file), aims) {
                continue;
            }
            let rows = &errors.rows;
            let seg = errors
                .segs
                .get_or_insert_with(|| file_seg(file, rows));
            // Each fragment indexes its own `Row` — the one
            // representation the daemon keeps.
            srows.extend(seg.kept.iter().map(|frag| Srow {
                rank: frag.rank,
                file16: &seg.file16,
                offset: frag.offset,
                msg16: &frag.msg16,
                row: &rows[frag.row as usize],
            }));
        }
        srows.sort_by(|a, b| {
            a.rank
                .cmp(&b.rank)
                .then_with(|| a.file16.cmp(b.file16))
                .then_with(|| a.offset.cmp(&b.offset))
                .then_with(|| a.msg16.cmp(b.msg16))
        });
        let rows: Vec<&Row> = srows.iter().map(|srow| srow.row).collect();
        let render = renderer(&rows, face);
        // The wire is the header line (the `Answer::Face` meta member plus
        // the payload length) then the raw payload — one contiguous run
        // the client reads as header-line + `bytes`. An over-cap payload
        // is a `NotServed`, the client's per-run fallback.
        let header = crate::daemon::face_header(&render.meta, render.bytes.len())?;
        let mut out = Vec::with_capacity(header.len() + render.bytes.len());
        out.extend_from_slice(&header);
        out.extend_from_slice(&render.bytes);
        let bytes: std::sync::Arc<[u8]> = out.into();
        self.answers.insert(
            key,
            CachedLine {
                epoch: self.epoch,
                bytes: std::sync::Arc::clone(&bytes),
            },
        );
        Some(bytes)
    }

    /// `render`'s not-served gate: `None` when a `.dart` file aim names no
    /// analyzed file — the caller falls back to a per-run `dart analyze`,
    /// which builds that file's own context root exactly as the oracle
    /// does.
    fn served(&self, aims: &[crate::Aim]) -> Option<()> {
        for aim in aims {
            if let crate::Aim::File(file) = aim
                && file.extension().is_some_and(|ext| ext == "dart")
                && !self.files.contains_key(file.display().to_string().as_str())
            {
                return None;
            }
        }
        Some(())
    }
}

/// The answer cache's key for one aims list: the canonical aims spelling
/// (variant-tagged, path-sorted, deduped, serialized) — asks that name
/// the same coverage in a different order reuse one line.
fn aims_key(aims: &[crate::Aim]) -> Vec<u8> {
    fn leg(aim: &crate::Aim) -> (u8, &Path) {
        match aim {
            crate::Aim::Dir(path) => (0, path.as_path()),
            crate::Aim::File(path) => (1, path.as_path()),
        }
    }
    let mut canonical: Vec<crate::Aim> = aims.to_vec();
    canonical.sort_by(|a, b| leg(a).cmp(&leg(b)));
    canonical.dedup();
    serde_json::to_vec(&canonical).expect("aims serialize")
}

/// One file's rendered share of the answer: its kept rows' sort facts
/// and serialized bytes and its `priority_errors` count — `Report::from_files`
/// restricted to this file, so the splice is dartdev's report by parts.
fn file_seg(file: &str, rows: &[Row]) -> FileSeg {
    let mut kept = Vec::new();
    let mut priority = 0;
    for (index, row) in rows.iter().enumerate() {
        if crate::report::kept(file, row) {
            kept.push(RowSeg {
                row: index as u32,
                rank: severity_rank(&row.severity),
                offset: row.location.offset,
                msg16: row.message.encode_utf16().collect(),
            });
        } else if row.severity == "ERROR" {
            // `!kept` is a dropped TODO-info or a priority-file ERROR —
            // the ERROR spelling is only the latter.
            priority += 1;
        }
    }
    FileSeg {
        file16: file.encode_utf16().collect(),
        kept: kept.into(),
        priority,
    }
}

#[derive(Deserialize)]
struct Message {
    id: Option<String>,
    event: Option<String>,
    /// `RawValue` keeps the subtree as wire text — the protocol's big
    /// notifications (`analysis.errors`) never materialize a `Value`
    /// tree; the small arms re-parse theirs in place.
    #[serde(default)]
    params: Option<Box<serde_json::value::RawValue>>,
    error: Option<Value>,
}

impl Message {
    /// The `params` subtree as a `Value` — the small-event arms'
    /// accessor (the `analysis.errors` arm never calls this: its payload
    /// is worth megabytes and reads the line's own parse instead).
    fn params(&self) -> Value {
        self.params
            .as_deref()
            .and_then(|raw| serde_json::from_str(raw.get()).ok())
            .unwrap_or(Value::Null)
    }
}

/// The one notification whose payload is worth megabytes: its rows
/// deserialize straight off the wire text — never through a `Value`
/// tree, which is the reader's one big transient allocation per file.
#[derive(Deserialize)]
struct ErrorsNote {
    params: ErrorsParams,
}

#[derive(Deserialize)]
struct ErrorsParams {
    file: String,
    errors: Vec<Row>,
}

/// The stdout reader: every protocol line updates the shared state.
fn read_messages(stdout: impl Read, shared: &Shared) {
    let reader = BufReader::new(stdout);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Message>(&line) else {
            continue;
        };
        let mut state = shared.state.lock();
        apply(&mut state, message, &line);
        drop(state);
        shared.changed.notify_all();
    }
    let mut state = shared.state.lock();
    state
        .fault
        .get_or_insert_with(|| "the analysis server exited".into());
    drop(state);
    shared.changed.notify_all();
}

fn apply(state: &mut State, message: Message, line: &str) {
    match message.event.as_deref() {
        Some("server.connected") => {
            state.connected = Some(
                message.params()["version"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            );
        }
        Some("server.status") => {
            if let Some(analyzing) = message.params()["analysis"]["isAnalyzing"].as_bool() {
                state.analyzing = analyzing;
                if analyzing {
                    state.busy += 1;
                } else {
                    state.idle += 1;
                }
            }
        }
        Some("analysis.errors") => {
            // Straight off the wire text: `Message`'s `params` stays
            // `RawValue` — the `errors` array never becomes a `Value`
            // tree, so a file's notification costs only its `Row`s.
            match serde_json::from_str::<ErrorsNote>(line) {
                Ok(note) => {
                    let ErrorsParams { file, errors: mut rows } = note.params;
                    // The pushed list lives for the analysis's life:
                    // serde's growth slack is dead bytes per file, and
                    // every row's `location.file` names the one path the
                    // map key already holds — shared, not duplicated.
                    let file: Arc<str> = file.into();
                    rows.shrink_to_fit();
                    for row in &mut rows {
                        row.location.file = Arc::clone(&file);
                        row.shrink();
                    }
                    state.epoch += 1;
                    state.files.insert(file, FileErrors { rows, segs: None });
                }
                Err(error) => {
                    let params = message.params();
                    let file = params["file"].as_str().unwrap_or_default();
                    state.fault = Some(format!("unreadable analysis.errors for {file}: {error}"));
                }
            }
        }
        Some("analysis.flushResults") => {
            state.epoch += 1;
            let params = message.params();
            for file in params["files"].as_array().into_iter().flatten() {
                if let Some(file) = file.as_str() {
                    state.files.remove(file);
                }
            }
        }
        Some(event @ ("server.error" | "server.pluginError")) => {
            let params = message.params();
            let text = params["message"].as_str().unwrap_or_default();
            state.fault = Some(format!("{event}: {text}"));
        }
        Some(_) => {}
        None => {
            if let Some(id) = message.id.and_then(|id| id.parse().ok()) {
                let response = match message.error {
                    Some(error) => Err(error["message"]
                        .as_str()
                        .unwrap_or("request failed")
                        .to_owned()),
                    None => Ok(()),
                };
                state.responses.insert(id, response);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(state: &mut State, line: &str) {
        apply(state, serde_json::from_str(line).expect("message"), line);
    }

    /// The reader's state machine: per-file replacement, flushes, status
    /// counters, responses, and faults.
    #[test]
    fn protocol_messages_update_the_error_map_and_status() {
        let mut state = State::default();
        feed(
            &mut state,
            r#"{"event":"server.connected","params":{"version":"1.40.1","pid":7}}"#,
        );
        assert_eq!(state.connected.as_deref(), Some("1.40.1"));
        feed(
            &mut state,
            r#"{"event":"server.status","params":{"analysis":{"isAnalyzing":true}}}"#,
        );
        let row = r#"{"severity":"INFO","type":"LINT","location":{"file":"/p/a.dart","offset":0,"length":1,"startLine":1,"startColumn":1,"endLine":1,"endColumn":2},"message":"m","code":"c","hasFix":false}"#;
        feed(
            &mut state,
            &format!(
                r#"{{"event":"analysis.errors","params":{{"file":"/p/a.dart","errors":[{row},{row}]}}}}"#
            ),
        );
        assert_eq!(state.files["/p/a.dart"].rows.len(), 2);
        feed(
            &mut state,
            &format!(
                r#"{{"event":"analysis.errors","params":{{"file":"/p/a.dart","errors":[{row}]}}}}"#
            ),
        );
        assert_eq!(
            state.files["/p/a.dart"].rows.len(),
            1,
            "the latest list replaces"
        );
        feed(
            &mut state,
            r#"{"event":"server.status","params":{"analysis":{"isAnalyzing":false}}}"#,
        );
        assert_eq!((state.busy, state.idle, state.analyzing), (1, 1, false));
        feed(
            &mut state,
            r#"{"event":"analysis.flushResults","params":{"files":["/p/a.dart"]}}"#,
        );
        assert!(state.files.is_empty());
        feed(&mut state, r#"{"id":"3","result":{}}"#);
        feed(
            &mut state,
            r#"{"id":"4","error":{"code":"X","message":"nope"}}"#,
        );
        assert_eq!(state.responses[&3], Ok(()));
        assert_eq!(state.responses[&4], Err("nope".into()));
        assert!(state.fault.is_none());
        feed(
            &mut state,
            r#"{"event":"server.error","params":{"isFatal":false,"message":"boom","stackTrace":""}}"#,
        );
        assert_eq!(state.fault.as_deref(), Some("server.error: boom"));
    }

    #[test]
    fn roots_are_spelled_without_trailing_separators() {
        assert_eq!(root_spelling(Path::new("/p/app/")), "/p/app");
        assert_eq!(root_spelling(Path::new("/")), "/");
    }

    /// The scoped report: directory aims cover their files, `.dart` file
    /// aims cover themselves, non-Dart file aims cover nothing, and a
    /// file aim the analyzer never pushed (an excluded or out-of-roots
    /// file) answers `None` — the silent per-run fallback.
    #[test]
    fn the_scoped_report_filters_to_the_aims_coverage() {
        let mut files: FxHashMap<Arc<str>, FileErrors> = FxHashMap::default();
        let row = |file: &str| Row {
            severity: "INFO".into(),
            kind: "LINT".into(),
            location: crate::report::Location {
                file: file.into(),
                offset: 0,
                length: 1,
                start_line: Some(1),
                start_column: Some(1),
                end_line: Some(1),
                end_column: Some(2),
            },
            message: "m".into(),
            correction: None,
            code: "c".into(),
            url: None,
            context_messages: Vec::new(),
        };
        for file in ["/p/lib/a.dart", "/p/lib/deep/b.dart", "/p/test/t.dart"] {
            files.insert(
                file.into(),
                FileErrors {
                    rows: vec![row(file)],
                    segs: None,
                },
            );
        }
        files.insert(
            "/p/lib/skip.dart".into(),
            FileErrors {
                rows: Vec::new(),
                segs: None,
            },
        );

        let lib = crate::Aim::Dir(PathBuf::from("/p/lib"));
        let scoped = scoped_report(&files, &[lib]).expect("served");
        let mut names: Vec<&str> = scoped
            .rows
            .iter()
            .map(|row| &*row.location.file)
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["/p/lib/a.dart", "/p/lib/deep/b.dart"]);

        let file_aim = crate::Aim::File(PathBuf::from("/p/test/t.dart"));
        assert_eq!(scoped_report(&files, &[file_aim]).expect("served").rows.len(), 1);

        let yaml_aim = crate::Aim::File(PathBuf::from("/p/pubspec.yaml"));
        assert!(scoped_report(&files, &[yaml_aim]).expect("served").rows.is_empty());

        let missing = crate::Aim::File(PathBuf::from("/p/lib/gone.dart"));
        assert!(scoped_report(&files, &[missing]).is_none(), "unanalyzed file aim falls back");

        // An excluded file present in no error list is served — its dir
        // aim simply reports the covered files.
        let excluded = crate::Aim::File(PathBuf::from("/p/lib/skip.dart"));
        assert!(scoped_report(&files, &[excluded]).expect("served").rows.is_empty());
    }

    /// The rendered answer line is byte-identical to
    /// `serde_json::to_vec(&Answer::Report(report))` over the same error
    /// map — the splice is dartdev's report by construction, not by
    /// approximation. Feeds the map through the same protocol events the
    /// reader applies, then compares every aims shape it serves.
    #[cfg(unix)]
    mod render {
        use super::*;

        /// One protocol `analysis.errors` line for `file` with `rows`
        /// spelled as serde serializes them — the reader's own input.
        fn errors(file: &str, rows: &[Row]) -> String {
            format!(
                r#"{{"event":"analysis.errors","params":{{"file":"{file}","errors":{}}}}}"#,
                serde_json::to_string(rows).expect("rows serialize")
            )
        }

        fn row(severity: &str, kind: &str, file: &str, offset: u32, message: &str) -> Row {
            Row {
                severity: severity.into(),
                kind: kind.into(),
                location: crate::report::Location {
                    file: file.into(),
                    offset,
                    length: 1,
                    start_line: Some(1),
                    start_column: Some(offset + 1),
                    end_line: Some(1),
                    end_column: Some(offset + 2),
                },
                message: message.into(),
                correction: Some("fix it".into()),
                code: "c".into(),
                url: Some("https://dart.dev/x".into()),
                context_messages: vec![crate::report::ContextMessage {
                    message: "ctx".into(),
                    location: crate::report::Location {
                        file: file.into(),
                        offset: offset + 100,
                        length: 2,
                        start_line: Some(9),
                        start_column: Some(1),
                        end_line: Some(9),
                        end_column: Some(3),
                    },
                }],
            }
        }

        /// A map with every row shape the splice distinguishes: severities
        /// across the rank ladder, a dropped TODO-info, a priority file,
        /// non-ASCII file and message legs (UTF-16 order), and an empty
        /// error list.
        fn sample_map() -> State {
            let mut state = State::default();
            let files: Vec<(&str, Vec<Row>)> = vec![
                (
                    "/p/lib/a.dart",
                    vec![
                        row("ERROR", "COMPILE_TIME_ERROR", "/p/lib/a.dart", 30, "boom"),
                        row("WARNING", "STATIC_WARNING", "/p/lib/a.dart", 10, "careful"),
                        row("INFO", "TODO", "/p/lib/a.dart", 0, "todo dropped"),
                        row("INFO", "LINT", "/p/lib/a.dart", 40, "info"),
                        row("INFO", "LINT", "/p/lib/a.dart", 10, "á unicode message"),
                    ],
                ),
                (
                    "/p/lib/deep/b.dart",
                    vec![row("ERROR", "COMPILE_TIME_ERROR", "/p/lib/deep/b.dart", 5, "bad")],
                ),
                (
                    "/p/lib/é.dart",
                    vec![row("WARNING", "LINT", "/p/lib/é.dart", 1, "nonascii file")],
                ),
                (
                    "/p/pubspec.yaml",
                    vec![
                        row("ERROR", "COMPILE_TIME_ERROR", "/p/pubspec.yaml", 0, "priority"),
                        row("WARNING", "STATIC_WARNING", "/p/pubspec.yaml", 0, "kept"),
                    ],
                ),
                ("/p/lib/empty.dart", Vec::new()),
            ];
            for (file, rows) in files {
                let line = errors(file, &rows);
                apply(&mut state, serde_json::from_str(&line).expect("message"), &line);
            }
            state
        }

        /// The from-scratch line over `state`'s map for `aims`.
        fn scratch(state: &State, aims: &[crate::Aim]) -> Option<Vec<u8>> {
            let report = if aims.is_empty() {
                Report::from_files(
                    state
                        .files
                        .iter()
                        .map(|(file, errors)| (&**file, errors.rows.as_slice())),
                )
            } else {
                scoped_report(&state.files, aims)?
            };
            let mut line = serde_json::to_vec(&crate::daemon::Answer::Report(report))
                .expect("an answer serializes");
            line.push(b'\n');
            Some(line)
        }

        fn rendered(state: &mut State, aims: &[crate::Aim]) -> Option<Vec<u8>> {
            state.render(aims).map(|line| line.to_vec())
        }

        #[test]
        fn the_rendered_line_is_byte_identical_to_the_report_serialization() {
            let mut state = sample_map();
            let aims_sets: Vec<Vec<crate::Aim>> = vec![
                Vec::new(),
                vec![crate::Aim::Dir(PathBuf::from("/p"))],
                vec![crate::Aim::Dir(PathBuf::from("/p/lib"))],
                vec![crate::Aim::File(PathBuf::from("/p/lib/a.dart"))],
                vec![
                    crate::Aim::File(PathBuf::from("/p/lib/a.dart")),
                    crate::Aim::Dir(PathBuf::from("/p/lib/deep")),
                ],
            ];
            for aims in &aims_sets {
                assert_eq!(
                    rendered(&mut state, aims).as_deref(),
                    scratch(&state, aims).as_deref(),
                    "aims {aims:?}",
                );
            }
            // The empty report still serializes in full.
            let mut empty = State::default();
            assert_eq!(
                rendered(&mut empty, &[]),
                Some(b"{\"Report\":{\"rows\":[],\"priority_errors\":0}}\n".to_vec())
            );
        }

        /// Every invalidation path: a pushed error list (even an identical
        /// one), a flush, and a restarted map drop the cached line.
        #[test]
        fn every_error_map_change_invalidates_the_answer_line() {
            let mut state = sample_map();
            let aims: Vec<crate::Aim> = Vec::new();
            let first = rendered(&mut state, &aims).expect("rendered");

            // An identical-content push still bumps the epoch: the line
            // is re-rendered (cache miss), and the bytes come out equal
            // — the epoch is a validator, not a staleness guess.
            let cached_entries = state.answers.entries.len();
            let same_rows = state.files["/p/lib/a.dart"].rows.clone();
            feed_lines(&mut state, &errors("/p/lib/a.dart", &same_rows));
            let second = rendered(&mut state, &aims).expect("rendered");
            assert_eq!(first, second, "identical map, identical line");
            assert_eq!(
                state.answers.entries.len(),
                cached_entries,
                "one aims shape, one entry (re-inserted)"
            );

            // A content change lands in the re-rendered line.
            feed_lines(
                &mut state,
                &errors("/p/lib/a.dart", &[row("ERROR", "COMPILE_TIME_ERROR", "/p/lib/a.dart", 99, "new")]),
            );
            let third = rendered(&mut state, &aims).expect("rendered");
            assert_ne!(first, third, "changed map, changed line");
            assert_eq!(Some(third.as_slice()), scratch(&state, &aims).as_deref());

            // A flush drops the file from the line.
            apply(
                &mut state,
                serde_json::from_str(
                    r#"{"event":"analysis.flushResults","params":{"files":["/p/lib/deep/b.dart"]}}"#,
                )
                .expect("message"),
                r#"{"event":"analysis.flushResults","params":{"files":["/p/lib/deep/b.dart"]}}"#,
            );
            let fourth = rendered(&mut state, &aims).expect("rendered");
            assert_eq!(Some(fourth.as_slice()), scratch(&state, &aims).as_deref());
            assert!(
                !String::from_utf8(fourth).expect("utf8").contains("b.dart"),
                "the flushed file left the line"
            );
        }

        fn feed_lines(state: &mut State, line: &str) {
            apply(state, serde_json::from_str(line).expect("message"), line);
        }

        /// The aims cache: order-free canonicalization hits one entry,
        /// distinct shapes each get one, and the LRU cap holds at eight.
        #[test]
        fn the_answer_cache_keys_canonical_aims_shapes_under_an_lru_cap() {
            let mut state = sample_map();
            let dir = crate::Aim::Dir(PathBuf::from("/p/lib"));
            let file = crate::Aim::File(PathBuf::from("/p/lib/a.dart"));
            // Same coverage, different order and a duplicate → one entry.
            rendered(&mut state, &[dir.clone(), file.clone()]);
            rendered(&mut state, &[file.clone(), dir.clone(), file.clone()]);
            assert_eq!(state.answers.entries.len(), 1);
            // Distinct shapes accumulate to the cap, then evict LRU.
            for index in 0..10 {
                let aims = vec![crate::Aim::Dir(PathBuf::from(format!("/p/d{index}")))];
                rendered(&mut state, &aims);
            }
            assert_eq!(state.answers.entries.len(), AnswerCache::CAP);
            // The earliest shape was evicted (re-render recomputes, still
            // byte-identical).
            let first_aims = vec![dir];
            assert_eq!(
                rendered(&mut state, &first_aims).as_deref(),
                scratch(&state, &first_aims).as_deref()
            );
            assert_eq!(state.answers.entries.len(), AnswerCache::CAP);
        }

        /// The per-file splice: random subsets of files re-pushed, the
        /// rendered line always equals a from-scratch serialization — the
        /// whole point of caching fragments instead of the whole report.
        #[test]
        fn a_subset_edit_splices_byte_identically() {
            let mut state = sample_map();
            let aims: Vec<crate::Aim> = Vec::new();
            let names: Vec<Arc<str>> = state.files.keys().cloned().collect();
            // A deterministic xorshift walk over subset edits.
            let mut seed = 0x9E3779B97F4A7C15u64;
            let mut next = || {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed
            };
            for round in 0..64 {
                let pick = (next() as usize) % names.len();
                let file = names[pick].clone();
                let rows = vec![row(
                    ["ERROR", "WARNING", "INFO"][(next() as usize) % 3],
                    "LINT",
                    &file,
                    (next() as u32) % 100,
                    &format!("edit {round}"),
                )];
                feed_lines(&mut state, &errors(&file, &rows));
                assert_eq!(
                    rendered(&mut state, &aims).as_deref(),
                    scratch(&state, &aims).as_deref(),
                    "round {round} editing {file}",
                );
                // Scoped renders agree too.
                let scoped = vec![crate::Aim::Dir(PathBuf::from("/p/lib"))];
                assert_eq!(
                    rendered(&mut state, &scoped).as_deref(),
                    scratch(&state, &scoped).as_deref(),
                    "scoped round {round}",
                );
            }
        }

        /// A `.dart` file aim the map never heard of is `None` — the
        /// per-run fallback — while a file aim the map lists (even with
        /// an empty error list) renders normally.
        #[test]
        fn an_unanalyzed_dart_file_aim_is_not_served() {
            let mut state = sample_map();
            let missing = vec![crate::Aim::File(PathBuf::from("/p/lib/gone.dart"))];
            assert_eq!(rendered(&mut state, &missing), None);
            let listed = vec![crate::Aim::File(PathBuf::from("/p/lib/empty.dart"))];
            assert_eq!(
                rendered(&mut state, &listed).as_deref(),
                scratch(&state, &listed).as_deref()
            );
        }

        // ---------------------------------------------------------------
        // The face render: same rows, the same dartdev order, the
        // `Answer::Face` header-plus-payload wire — the byte pin is the
        // writer's own test on the dartq-commands side; these pin the
        // wire shape, the row set the writer sees, and the cache lanes.
        // ---------------------------------------------------------------

        /// A stub renderer that echoes the rows it was given, so the pin
        /// is on the daemon's row set/order and the wire frame — not on
        /// the writer's bytes.
        fn stub_render(rows: &[&Row], face: &crate::face::Face) -> crate::face::FaceRender {
            let mut bytes = Vec::new();
            for row in rows {
                bytes.extend_from_slice(row.location.file.as_bytes());
                bytes.push(b'|');
                bytes.extend_from_slice(row.message.as_bytes());
                bytes.push(b';');
            }
            bytes.extend_from_slice(format!("fmt:{:?}", face.format).as_bytes());
            crate::face::FaceRender {
                meta: crate::face::FaceMeta {
                    has_error: rows.iter().any(|row| row.severity == "ERROR"),
                    ..crate::face::FaceMeta::default()
                },
                bytes,
            }
        }

        /// Parses `Answer::Face`'s header + payload back out of the wire.
        fn face_parts(wire: &[u8]) -> (crate::daemon::Answer, Vec<u8>) {
            let newline = wire.iter().position(|b| *b == b'\n').expect("header newline");
            let header = std::str::from_utf8(&wire[..newline]).expect("header utf8");
            let answer: crate::daemon::Answer =
                serde_json::from_str(header).expect("header parses");
            (answer, wire[newline + 1..].to_vec())
        }

        /// The face wire carries the header's counts and the writer's
        /// bytes verbatim; the row set the writer saw is the report's
        /// kept rows in dartdev order.
        #[test]
        fn the_face_wire_is_header_then_payload_over_dartdev_order() {
            let mut state = sample_map();
            let aims: Vec<crate::Aim> = Vec::new();
            let face = crate::face::Face {
                format: crate::face::FaceFormat::Lon,
                root: Some(PathBuf::from("/p")),
            };
            let wire = state
                .render_face(&aims, &face, stub_render)
                .expect("rendered")
                .to_vec();
            let (answer, payload) = face_parts(&wire);
            let crate::daemon::Answer::Face { meta, bytes } = answer else {
                panic!("a face answer: {answer:?}");
            };
            assert_eq!(bytes as usize, payload.len(), "announced length");
            assert!(meta.has_error);
            // The kept rows in dartdev order: severity first, the file's
            // own kept order next — the priority-file ERROR row split off.
            let text = String::from_utf8(payload).expect("utf8");
            let errors = text.matches("boom").count() + text.matches("bad").count();
            assert_eq!(errors, 2, "the error rows reached the writer");
            assert!(!text.contains("priority"), "priority rows split off");
            assert!(!text.contains("todo dropped"), "todo rows dropped");
        }

        /// The face cache lanes: (aims, face) keying means one face's
        /// render is not another's hit, the epoch still invalidates, and
        /// the byte cap holds the total.
        #[test]
        fn the_face_cache_keys_faces_separately_and_stays_bounded() {
            let mut state = sample_map();
            let aims: Vec<crate::Aim> = Vec::new();
            let lon = crate::face::Face {
                format: crate::face::FaceFormat::Lon,
                root: None,
            };
            let json = crate::face::Face {
                format: crate::face::FaceFormat::Json,
                root: None,
            };
            let first = state.render_face(&aims, &lon, stub_render).expect("rendered");
            let again = state.render_face(&aims, &lon, stub_render).expect("rendered");
            assert_eq!(first, again, "same (aims, face) hits");
            let other = state.render_face(&aims, &json, stub_render).expect("rendered");
            assert_ne!(first, other, "a different face misses");
            // The report line and the faces share the one cache.
            state.render(&aims).expect("rendered");
            assert_eq!(state.answers.entries.len(), 3, "lon + json + report");
            // An epoch bump misses every lane.
            let rows = state.files["/p/lib/a.dart"].rows.clone();
            feed_lines(&mut state, &errors("/p/lib/a.dart", &rows));
            let re = state.render_face(&aims, &lon, stub_render).expect("rendered");
            assert_eq!(re, first, "identical map re-renders identical bytes");
        }
    }
}
