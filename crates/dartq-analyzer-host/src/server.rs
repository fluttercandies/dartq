//! One official analysis server child, driven exactly as `dart analyze`
//! drives it: the same launch flags, `server.setSubscriptions[STATUS]`,
//! `analysis.setAnalysisRoots` over the realpath'd targets, and analysis
//! complete at the `server.status` that turns `isAnalyzing` false. Per
//! file, the latest `analysis.errors` list replaces the previous one, which
//! is the error map dartdev reports from.
//!
//! Between requests the child stays up. A stat diff ([`crate::snapshot`])
//! tells the host what changed on disk; [`Server::changed`] hands Dart
//! edits to the server as `RemoveContentOverlay` entries (each one is a
//! `changeFile` in the server whether or not an overlay existed), and
//! Additions, deletions and configuration edits get a fresh server (the
//! daemon restarts it): `analysis.reanalyze` rebuilds the contexts
//! asynchronously after its response, and the server's own file watcher
//! interleaves flushes and analysis cycles of its own, so no event marks
//! the rebuilt analysis reliably. A fresh server is `dart analyze` by
//! construction, and its `--cache` keeps the restart near a warm start.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use rustc_hash::FxHashMap;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::report::{Report, Row};
use crate::support::{Sdk, Version};

/// How long the server may take to announce itself.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
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

/// Why a server cannot answer. Every variant but [`Fault::Budget`] and
/// [`Fault::Memory`] sends the caller to a per-run analysis.
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
        }
    }
}

/// The fixed launch facts of one server.
#[derive(Clone, Debug)]
pub struct Launch {
    pub sdk: Sdk,
    pub cache: PathBuf,
    pub packages: Option<PathBuf>,
}

impl Launch {
    /// dartdev's argument list (`AnalysisServer._startProcess`).
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
            .arg(&self.sdk.root)
            .arg(format!("--cache={}", self.cache.display()));
        if let Some(packages) = &self.packages {
            command.arg(format!("--packages={}", packages.display()));
        }
        command
    }
}

#[derive(Default)]
struct State {
    files: FxHashMap<String, Vec<Row>>,
    analyzing: bool,
    busy: u64,
    idle: u64,
    responses: FxHashMap<u64, Result<(), String>>,
    connected: Option<String>,
    fault: Option<String>,
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
}

impl Server {
    /// Starts the server, sets its roots and waits for the first analysis.
    /// `budget` (bytes, `None` unbounded) caps the process tree's RSS
    /// while analyses run.
    ///
    /// # Errors
    ///
    /// [`Fault`] when the server does not start, the handshake fails, or
    /// the first analysis does not finish.
    pub fn start(launch: &Launch, roots: &[PathBuf], budget: Option<u64>) -> Result<Self, Fault> {
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
        let mut server = Self {
            child,
            stdin,
            shared,
            next_id: 0,
            budget,
            peak: 0,
        };
        let result = server.handshake(launch.sdk.version, roots);
        if let Err(fault) = result {
            server.kill();
            return Err(fault);
        }
        // The full analysis just ended: the heap is near its peak even when
        // the analysis was shorter than one sample period.
        server.peak = server.peak.max(tree_rss(server.pid()));
        Ok(server)
    }

    fn handshake(&mut self, sdk: Version, roots: &[PathBuf]) -> Result<(), Fault> {
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        {
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
        }
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
        self.settle(mark)
    }

    /// Hands changed Dart files to the server and waits for the analysis.
    ///
    /// # Errors
    ///
    /// [`Fault`] as in [`Server::start`].
    pub fn changed(&mut self, files: &[PathBuf]) -> Result<(), Fault> {
        let changes: serde_json::Map<String, Value> = files
            .iter()
            .map(|file| (file.display().to_string(), json!({"type": "remove"})))
            .collect();
        let mark = self.mark();
        self.call("analysis.updateContent", json!({"files": changes}))?;
        self.settle(mark)
    }

    /// Waits out an analysis the server started on its own (its file
    /// watcher), so a report never reads a map mid-analysis.
    ///
    /// # Errors
    ///
    /// [`Fault`] as in [`Server::start`].
    pub fn wait_idle(&mut self) -> Result<(), Fault> {
        let mark = self.mark();
        if !self.shared.state.lock().analyzing {
            return Ok(());
        }
        self.settle(Mark {
            busy: mark.busy,
            idle: mark.idle,
        })
    }

    /// dartdev's report over the current error map.
    #[must_use]
    pub fn report(&self) -> Report {
        let state = self.shared.state.lock();
        Report::from_files(
            state
                .files
                .iter()
                .map(|(file, rows)| (file.as_str(), rows.as_slice())),
        )
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
        let mut state = self.shared.state.lock();
        loop {
            if let Some(response) = state.responses.remove(&id) {
                return response
                    .map_err(|error| Fault::Server(format!("{method} failed: {error}")));
            }
            if let Some(fault) = &state.fault {
                return Err(Fault::Server(fault.clone()));
            }
            self.shared.changed.wait(&mut state);
        }
    }

    /// Waits until the analysis started after `mark` is complete, sampling
    /// the tree's RSS once per [`RSS_POLL`] for the peak and the budget.
    /// The cadence is by the clock, not per wake-up: a large analysis
    /// sends a notification per file.
    fn settle(&mut self, mark: Mark) -> Result<(), Fault> {
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

#[derive(Deserialize)]
struct Message {
    id: Option<String>,
    event: Option<String>,
    #[serde(default)]
    params: Value,
    error: Option<Value>,
}

/// The stdout reader: every protocol line updates the shared state.
fn read_messages(stdout: impl Read, shared: &Shared) {
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Message>(&line) else {
            continue;
        };
        let mut state = shared.state.lock();
        apply(&mut state, message);
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

fn apply(state: &mut State, message: Message) {
    let params = &message.params;
    match message.event.as_deref() {
        Some("server.connected") => {
            state.connected = Some(params["version"].as_str().unwrap_or_default().to_owned());
        }
        Some("server.status") => {
            if let Some(analyzing) = params["analysis"]["isAnalyzing"].as_bool() {
                state.analyzing = analyzing;
                if analyzing {
                    state.busy += 1;
                } else {
                    state.idle += 1;
                }
            }
        }
        Some("analysis.errors") => {
            let file = params["file"].as_str().unwrap_or_default().to_owned();
            match Vec::<Row>::deserialize(&params["errors"]) {
                Ok(rows) => {
                    state.files.insert(file, rows);
                }
                Err(error) => {
                    state.fault = Some(format!("unreadable analysis.errors for {file}: {error}"));
                }
            }
        }
        Some("analysis.flushResults") => {
            for file in params["files"].as_array().into_iter().flatten() {
                if let Some(file) = file.as_str() {
                    state.files.remove(file);
                }
            }
        }
        Some(event @ ("server.error" | "server.pluginError")) => {
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
        apply(state, serde_json::from_str(line).expect("message"));
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
        assert_eq!(state.files["/p/a.dart"].len(), 2);
        feed(
            &mut state,
            &format!(
                r#"{{"event":"analysis.errors","params":{{"file":"/p/a.dart","errors":[{row}]}}}}"#
            ),
        );
        assert_eq!(
            state.files["/p/a.dart"].len(),
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
}
