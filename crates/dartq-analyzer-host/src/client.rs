//! The `dartq analyze` side of the resident server: find the daemon of the
//! request's key, start it when none is live, ask, and answer. A daemon
//! that cannot serve — a cooldown window after a hard failure, a start or
//! transport fault — falls to the per-run direct drive below before any
//! delegation: one analysis server, spawned and driven by this process
//! through the same [`crate::server::Launch`] the daemon serves (no
//! `dart`/dartdev wrapper in the chain), under the same memory gate. Only
//! what that drive cannot serve is an [`Outcome::PerRun`]: the caller runs
//! `dart analyze` per run, so the resident path can only make a run
//! faster, never fail it.

#![allow(clippy::semicolon_if_nothing_returned)]
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::face::{Face, FaceMeta};
use crate::report::Report;
use crate::support::{Sdk, Support, support};
use crate::{Files, SERVE_FLAG, Spec};

/// How long a fresh daemon may take to bind its socket.
#[cfg(unix)]
const BIND_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a failure marker sends runs straight to the per-run path.
#[cfg(unix)]
const FAILED_COOLDOWN: Duration = Duration::from_mins(10);
/// The most one answer may take once the request is written: covers the
/// daemon's own worst wait (the 30-minute job-gate lease wait of
/// `daemon::MEMORY_WAIT`) plus slack. A wedged daemon hits the deadline
/// and the caller falls back instead of hanging forever.
#[cfg(unix)]
const ANSWER_TIMEOUT: Duration = Duration::from_mins(35);

/// The most bytes one `Answer::Face` header may announce. The daemon's own
/// face cache caps payloads far below this (64 MiB), so a header claiming
/// more is corrupt or from a mismatched peer: the reader refuses it
/// instead of honoring an unbounded, wire-driven allocation. The daemon's
/// `face_header` refuses on the same line — an over-cap face is a
/// `NotServed`, never a panic.
#[cfg(unix)]
pub(crate) const MAX_FACE_BYTES: u64 = 1 << 30;

/// Reads one face payload of a header-announced `bytes`. `Ok(None)` is an
/// over-cap announcement: nothing is allocated and nothing is read — the
/// stream stays where the header ended and the caller takes its ordinary
/// failure path (the same outcome a short read produces). `usize::try_from`
/// keeps a 32-bit target honest; the cap makes it unreachable on 64-bit.
#[cfg(unix)]
fn read_face(reader: &mut impl std::io::Read, bytes: u32) -> std::io::Result<Option<Vec<u8>>> {
    if u64::from(bytes) > MAX_FACE_BYTES {
        return Ok(None);
    }
    let mut payload = vec![0u8; usize::try_from(bytes).unwrap_or(0)];
    reader.read_exact(&mut payload)?;
    Ok(Some(payload))
}

/// One resident analysis request.
#[derive(Clone, Debug)]
pub struct Request {
    /// The `dart` binary the toolchain resolved.
    pub dart: PathBuf,
    /// The analysis targets, existing files or directories — the run's
    /// report aims (`dart analyze <paths>` semantics: a directory aim
    /// covers every file under it, a `.dart` file aim covers that file).
    pub targets: Vec<PathBuf>,
    /// The workspace roots the targets resolve under — what the resident
    /// server analyzes. They key the daemon, so two runs over the same
    /// workspace share one server regardless of the aims they name.
    pub roots: Vec<PathBuf>,
    pub packages: Option<PathBuf>,
    /// The server tree's RSS budget in bytes; `0` is unbounded.
    pub budget: u64,
    pub idle: Duration,
    /// `memory.max_percent`, the memory ledger's share.
    pub max_percent: u8,
    /// `memory.max_concurrent`, the job cap the server's slot declares.
    pub max_concurrent: u32,
    /// The shared-pool posture (`DARTQ_POOL` or `analyzer.pool.enabled`):
    /// `0` opts out — one daemon per worktree. `Some(cap)` asks for the
    /// pooled daemon (founding it when none is live), the cap being the
    /// most worktrees it serves; a refused join falls back to the
    /// worktree's own daemon.
    pub pool: Option<u32>,
    /// The warm-standby posture the spec carries (`analyzer.pool.
    /// warm_servers`): `0` is not-declared — the daemon's own default
    /// resolves (off since 0.1.23) — and `> 0` warm on; the spec has no
    /// off spelling (the `DARTQ_WARM_SERVERS` env stays that arm).
    pub warm: u32,
    /// How long an idle daemon keeps its standby (`analyzer.pool.
    /// warm_ttl_secs`); `0` is not-declared — the daemon's default.
    pub warm_ttl_secs: u32,
    /// The output face this run prints. `Some` asks the daemon for the
    /// rendered stdout bytes (the face path); `None` keeps the report
    /// line (the compat arm — the pooled join never carries one).
    pub face: Option<Face>,
    /// Ask the daemon for the per-leg timing breakdown — the
    /// `--metrics-json` surface. The legs arrive on a separate
    /// `Answer::Metrics` line and land in the face's `meta.legs`; a
    /// report-arm or per-run answer carries none.
    pub metrics: bool,
}

/// The answer of the resident path.
#[derive(Debug)]
pub enum Outcome {
    /// dartdev's report, with the SDK version that produced it.
    Report { report: Report, sdk: Sdk },
    /// The daemon-rendered face: the final stdout bytes and the counts
    /// the client finishes the run with, with the SDK that produced them.
    Face {
        stdout: Vec<u8>,
        meta: FaceMeta,
        sdk: Sdk,
    },
    /// The server was killed over the RSS budget, or never got its memory
    /// lease: the error the caller prints, with no per-run retry (it would
    /// hit the same budget, or wait for the same memory).
    Budget(String),
    /// Run `dart analyze` per run; `reason` is the one stderr line, `None`
    /// when the SDK is simply not served (no line).
    PerRun {
        reason: Option<String>,
        sdk: Option<Sdk>,
    },
}

/// Asks the resident server for `request`'s report.
#[must_use]
pub fn analyze(request: &Request) -> Outcome {
    let Some(sdk) = Sdk::of_dart(&request.dart) else {
        return Outcome::PerRun {
            reason: None,
            sdk: None,
        };
    };
    let version = sdk.version.to_string();
    if support(&version) == Support::PerRun {
        return Outcome::PerRun {
            reason: None,
            sdk: Some(sdk),
        };
    }
    let roots: Result<Vec<PathBuf>, _> = request.roots.iter().map(std::fs::canonicalize).collect();
    let Ok(mut roots) = roots else {
        return Outcome::PerRun {
            reason: None,
            sdk: Some(sdk),
        };
    };
    if roots.is_empty() {
        return Outcome::PerRun {
            reason: None,
            sdk: Some(sdk),
        };
    }
    // One daemon per workspace: the roots key the spec, so the spelling
    // must be order-free — sorted and deduplicated.
    roots.sort();
    roots.dedup();
    let aims: Vec<crate::Aim> = request
        .targets
        .iter()
        .map(|target| {
            if target.is_dir() {
                crate::Aim::Dir(target.clone())
            } else {
                crate::Aim::File(target.clone())
            }
        })
        .collect();
    let spec = Spec {
        sdk_root: sdk.root.clone(),
        sdk_version: version,
        roots,
        packages: request.packages.as_deref().map(absolute),
        budget: request.budget,
        idle_secs: request.idle.as_secs(),
        max_percent: request.max_percent,
        max_concurrent: request.max_concurrent,
        pool: request.pool.is_some(),
        max_roots: request.pool.unwrap_or(0),
        // The warm posture resolves daemon-side (env over spec over
        // default); the CLI passes the resolved YAML knobs through, `0`
        // meaning "not declared", never "off".
        warm: request.warm,
        warm_ttl_secs: request.warm_ttl_secs,
    };
    #[cfg(unix)]
    {
        let face = request.face.clone();
        let metrics = request.metrics;
        let asked = if spec.pool {
            let mut own = spec.clone();
            own.pool = false;
            if up(&own.dir()) {
                // A worktree the pool refused runs its own daemon, and
                // keeps it while it lives: re-asking the pool would queue
                // every request behind the pool's own work just to be
                // refused again. Once the daemon idles out, the next run
                // asks the pool first.
                ask_gated(&own, &aims, face.as_ref(), metrics)
            } else {
                // The pool-first path: join the live pooled daemon, or
                // found it. A refused join falls back to this worktree's
                // own daemon — started on demand, never silently a second
                // pooled server.
                match pool_join(&spec, &aims, face.as_ref(), metrics) {
                    Some(PoolAnswer::Report(report)) => return Outcome::Report { report, sdk },
                    Some(PoolAnswer::Face { stdout, meta }) => {
                        return Outcome::Face { stdout, meta, sdk };
                    }
                    Some(PoolAnswer::Budget(message)) => return Outcome::Budget(message),
                    // A refused join or a transport failure alike falls
                    // to the worktree's own daemon — `None` and
                    // `Unavailable` are one path. A yielded pooled server
                    // is the same: the worktree's own daemon re-queues
                    // the job.
                    Some(PoolAnswer::Unavailable) | None => {
                        ask_gated(&own, &aims, face.as_ref(), metrics)
                    }
                }
            }
        } else {
            ask_gated(&spec, &aims, face.as_ref(), metrics)
        };
        match asked {
            Ok(Answered::Report(report)) => Outcome::Report { report, sdk },
            Ok(Answered::Face { stdout, meta }) => Outcome::Face { stdout, meta, sdk },
            Ok(Answered::Budget(message)) => Outcome::Budget(message),
            // `Yielded` is handled inside `ask_gated`'s re-queue loop; a
            // bare `ask` (the pool path) maps it to `Unavailable` there.
            // `NotServed` keeps the child: the aims fall outside the roots'
            // one analysis context, and the child builds the file's own
            // context root exactly as the oracle does — a fresh server over
            // the same roots would not.
            Ok(Answered::Yielded | Answered::NotServed) => Outcome::PerRun {
                reason: None,
                sdk: Some(sdk),
            },
            // The daemon cannot serve, but the SDK and roots it resolved
            // are serveable facts: drive one server directly, per run —
            // the dartdev child only if even that fails. The drive runs
            // under the worktree's own spelling (`pool` off) so its
            // cache-dir peak-key fallback is the own-daemon family's.
            unserved @ (Ok(Answered::Cooling) | Err(_)) => {
                let mut own = spec.clone();
                own.pool = false;
                direct_drive(&own, &sdk, &aims, unserved.err())
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = spec;
        Outcome::PerRun {
            reason: None,
            sdk: Some(sdk),
        }
    }
}

fn absolute(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// How long the direct drive may wait at the memory gate: the daemon's own
/// lease bound, so a one-shot waiter gets every honest wait a daemon-start
/// waiter gets.
#[cfg(unix)]
const GATE_WAIT: Duration = Duration::from_mins(30);

/// The per-run direct drive: one analysis server, spawned by this process
/// under the same [`crate::server::Launch`] the daemon serves — the SDK's
/// own snapshot against the resolved roots, no `dart`/dartdev wrapper in
/// the chain — analyzed to completion, scoped to `aims`, and shut down.
/// The dartdev cost class (`dart analyze`'s wrapper process, its envelope
/// re-parse) is what this arm exists to skip: a run the daemon cannot
/// serve still cold-starts at the server's own wall, the dana-class cold.
///
/// `daemon_fault` is why the daemon did not serve (the stderr line's first
/// half; `None` on a silent cooldown). The drive holds the same
/// `AnalysisServer` ledger lease a daemon server-start holds — same gate,
/// same sizing history over the same peak key — so concurrent one-shots
/// serialize under `memory.max_concurrent` exactly as daemon starts do.
#[cfg(unix)]
fn direct_drive(
    spec: &Spec,
    sdk: &Sdk,
    aims: &[crate::Aim],
    daemon_fault: Option<String>,
) -> Outcome {
    use dartq_toolchain::ledger::{self, Face};

    let launch = crate::server::Launch {
        sdk: sdk.clone(),
        packages: spec.packages.clone(),
    };
    let peak_key = crate::daemon::repo_key(&spec.roots)
        .unwrap_or_else(|| spec.cache_dir().display().to_string());
    let (bytes, sized) = ledger::sizing_with_source(&peak_key, Face::AnalysisServer, None);
    let mut lease = match ledger::acquire(
        direct_lease_request(&peak_key, spec, bytes, sized),
        bytes,
        spec.max_percent,
        std::time::Instant::now() + GATE_WAIT,
        |text| eprintln!("{text}"),
    ) {
        Ok(lease) => lease,
        Err(error) => {
            return Outcome::PerRun {
                reason: fallback_line(
                    daemon_fault,
                    Some(format!(
                        "the direct server never got its memory lease: {error}"
                    )),
                ),
                sdk: Some(sdk.clone()),
            };
        }
    };
    let budget = (spec.budget > 0).then_some(spec.budget);
    let started = crate::server::Server::start(&launch, &spec.roots, budget, || false);
    let server = match started {
        Ok(server) => server,
        // A budget kill is not retried per run: the child would need the
        // same memory the ledger just refused — the daemon arm's own rule.
        Err(fault @ crate::server::Fault::Budget { .. }) => {
            return Outcome::Budget(fault.to_string());
        }
        Err(fault) => {
            return Outcome::PerRun {
                reason: fallback_line(daemon_fault, Some(fault.to_string())),
                sdk: Some(sdk.clone()),
            };
        }
    };
    if let Some(lease) = lease.as_mut() {
        lease.track(server.pid());
    }
    let report = server.report_scoped(aims);
    let peak = server.peak();
    server.shutdown();
    if let Some(mut lease) = lease {
        lease.record_peak(&peak_key, peak);
    }
    match report {
        Some(report) => Outcome::Report {
            report,
            sdk: sdk.clone(),
        },
        // The aims fall outside the roots' one analysis context (the
        // `NotServed` case): the child builds the file's own context root
        // exactly as the oracle does — silently, as that arm always was.
        None => Outcome::PerRun {
            reason: None,
            sdk: Some(sdk.clone()),
        },
    }
}

/// The direct drive's lease request: the `AnalysisServer` face, a slot
/// under the run's own job cap, and the peak key the daemon's servers of
/// the same roots book under — one gate, one sizing history, never a
/// second ungated server per run. Pure: the bytes and `sized` legs are
/// the caller's `sizing_with_source` answer.
#[cfg(unix)]
fn direct_lease_request(
    peak_key: &str,
    spec: &Spec,
    bytes: u64,
    sized: bool,
) -> dartq_toolchain::ledger::Request {
    dartq_toolchain::ledger::Request {
        face: dartq_toolchain::ledger::Face::AnalysisServer,
        label: spec
            .roots
            .first()
            .map(|root| root.display().to_string())
            .unwrap_or_default(),
        bytes,
        pid: None,
        // No graceful release handle: a one-shot dies with its user, and
        // the ledger's dead scan reaps the booking — the same posture as
        // the `dart analyze` child it replaces, which offers none either.
        release: None,
        slot: true,
        cap: dartq_toolchain::ledger::cap(spec.max_concurrent),
        sized,
        peak_key: Some(peak_key.to_owned()),
        source_bytes: None,
    }
}

/// The per-run fallback's stderr line: why the daemon did not serve, and
/// — when the direct drive ran — why it too fell short. `None` keeps the
/// silent fallback the cooling arm always had.
#[cfg(unix)]
fn fallback_line(daemon_fault: Option<String>, drive_fault: Option<String>) -> Option<String> {
    match (daemon_fault, drive_fault) {
        (None, None) => None,
        (Some(daemon), None) => Some(format!(
            "dartq analyze: resident analyzer unavailable ({daemon}) — running dart analyze"
        )),
        (None, Some(drive)) => Some(format!(
            "dartq analyze: the direct analysis server failed ({drive}) — running dart analyze"
        )),
        (Some(daemon), Some(drive)) => Some(format!(
            "dartq analyze: resident analyzer unavailable ({daemon}); \
             the direct server failed too ({drive}) — running dart analyze"
        )),
    }
}

/// Whether the daemon of `dir` is up: its state names a live pid and its
/// socket file exists. No connection is made — a probe connection counts
/// as a request and would hold the daemon past its idle timeout. A stale
/// answer is harmless: the ask then starts the daemon as usual.
#[cfg(unix)]
fn up(dir: &Path) -> bool {
    crate::daemon_pid(dir).is_some() && Files::of(dir).socket.exists()
}

#[cfg(unix)]
enum Answered {
    Report(Report),
    /// The face answer's payload and counts.
    Face {
        stdout: Vec<u8>,
        meta: FaceMeta,
    },
    Budget(String),
    Cooling,
    /// The aims fall outside the daemon's one analysis context (a
    /// `.dart` file aim the options exclude or the roots do not reach): a
    /// silent per-run fallback — no stderr line.
    NotServed,
    /// The growth brake fired mid-answer: the daemon killed its server,
    /// freed its lease and recorded the observed peak. The client
    /// re-queues behind the job gate — FIFO — and retries the ask once
    /// admitted; never a per-run fallback (it would need the same
    /// memory).
    Yielded,
}

/// How long the client keeps re-queuing on `Answer::Yield`: the gate
/// deadline the daemon's own lease wait uses, so a memory-tight host
/// gets every honest wait, and a host that never frees still bounds.
#[cfg(unix)]
const YIELD_REQUEUE: Duration = Duration::from_mins(30);

/// [`ask`] with the growth brake's re-queue: an `Answer::Yield` means the
/// daemon's server was the youngest heavy holder on a critically low host
/// and has been killed with its peak recorded — the client waits a beat,
/// then re-issues the whole ask (the daemon's `lease_slot` re-enters the
/// job gate FIFO; its notes stream to stderr as usual). Bounded by
/// [`YIELD_REQUEUE`] — past it the run is a [`Outcome::Budget`]-shaped
/// failure, never a per-run `dart analyze`, which would need the same
/// memory the ledger just refused.
#[cfg(unix)]
fn ask_gated(
    spec: &Spec,
    aims: &[crate::Aim],
    face: Option<&Face>,
    metrics: bool,
) -> Result<Answered, String> {
    let deadline = std::time::Instant::now() + YIELD_REQUEUE;
    let mut yields = 0u32;
    loop {
        match ask(spec, aims, face, metrics)? {
            Answered::Yielded => {
                yields += 1;
                if std::time::Instant::now() + Duration::from_millis(500) >= deadline {
                    return Ok(Answered::Budget(format!(
                        "dartq analyze: host memory stayed critically low through {yields} yields — \
                         the analyzer never got its memory back"
                    )));
                }
                if yields == 1 {
                    let free = dartq_toolchain::ledger::available_memory().map_or_else(
                        || "low memory".into(),
                        |bytes| format!("{:.1} GB free", bytes as f64 / f64::from(1u32 << 30)),
                    );
                    eprintln!(
                        "dartq analyze: the analyzer yielded to keep the machine alive ({free}); \
                         re-queuing {} behind the job gate",
                        spec.roots
                            .first()
                            .map(|root| root.display().to_string())
                            .unwrap_or_default()
                    );
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            other => return Ok(other),
        }
    }
}

/// A pooled join's three ways out.
#[cfg(unix)]
enum PoolAnswer {
    Report(Report),
    Face {
        stdout: Vec<u8>,
        meta: FaceMeta,
    },
    Budget(String),
    /// No pooled serving happened: the join was refused at capacity, the
    /// pooled daemon never materialized, or the aims are not the pool's
    /// to answer — the caller falls back to the worktree's own daemon.
    Unavailable,
}

/// The opt-in pool path: connect to the pooled daemon — or elect-start
/// it (the requesting spec carries `pool`), then ask. The pooled spec's
/// directory key drops the roots, so every compatible worktree lands on
/// the same daemon directory and the same `elect.lock` election; no
/// registry file is needed. A connection that dies or a join the daemon
/// refuses answers [`PoolAnswer::Unavailable`], never a second pooled
/// analyzer.
#[cfg(unix)]
fn pool_join(
    spec: &Spec,
    aims: &[crate::Aim],
    face: Option<&Face>,
    metrics: bool,
) -> Option<PoolAnswer> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    use crate::daemon::{Answer, Ask};

    let dir = spec.dir();
    let files = Files::of(&dir);
    if let Ok(meta) = std::fs::metadata(&files.failed)
        && meta
            .modified()
            .ok()
            .and_then(|at| at.elapsed().ok())
            .is_some_and(|age| age < FAILED_COOLDOWN)
    {
        return Some(PoolAnswer::Unavailable);
    }
    let stream = match UnixStream::connect(&files.socket) {
        Ok(stream) => stream,
        Err(_) => start(spec, &dir, &files).ok()?,
    };
    // A wedged daemon gets a bounded answer window: the read deadline is
    // the same `ANSWER_TIMEOUT` the unpooled ask uses.
    stream.set_read_timeout(Some(ANSWER_TIMEOUT)).ok();
    let mut line = serde_json::to_vec(&Ask {
        roots: spec.roots.clone(),
        aims: aims.to_vec(),
        face: face.cloned(),
        metrics,
    })
    .expect("serializes");
    line.push(b'\n');
    (&stream).write_all(&line).ok()?;
    let mut reader = BufReader::new(&stream);
    let mut answer = String::new();
    let mut legs = None;
    loop {
        answer.clear();
        reader.read_line(&mut answer).ok()?;
        match serde_json::from_str::<Answer>(&answer) {
            Ok(Answer::Note(note)) => eprintln!("{note}"),
            Ok(Answer::Metrics(found)) => legs = Some(found),
            Ok(Answer::Report(report)) => return Some(PoolAnswer::Report(report)),
            Ok(Answer::Face { meta, bytes }) => {
                let mut meta = meta;
                meta.legs = legs.take();
                let stdout = read_face(&mut reader, bytes).ok()?;
                return Some(PoolAnswer::Face {
                    stdout: stdout?,
                    meta,
                });
            }
            Ok(Answer::Budget(message)) => return Some(PoolAnswer::Budget(message)),
            // A refused join or a serving fault alike is the caller's own
            // daemon — the pool never silently spawns a second analyzer.
            // A yielded pooled server is the same path: the worktree's
            // own daemon re-queues the job.
            Ok(Answer::NotServed | Answer::Yield | Answer::Fault(_)) | Err(_) => {
                return Some(PoolAnswer::Unavailable);
            }
        }
    }
}

/// The unix transport: election, start, request.
#[cfg(unix)]
fn ask(
    spec: &Spec,
    aims: &[crate::Aim],
    face: Option<&Face>,
    metrics: bool,
) -> Result<Answered, String> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    use crate::daemon::{Answer, Ask};

    let dir = spec.dir();
    let files = Files::of(&dir);
    if let Ok(meta) = std::fs::metadata(&files.failed)
        && meta
            .modified()
            .ok()
            .and_then(|at| at.elapsed().ok())
            .is_some_and(|age| age < FAILED_COOLDOWN)
    {
        return Ok(Answered::Cooling);
    }
    let stream = match UnixStream::connect(&files.socket) {
        Ok(stream) => stream,
        Err(_) => start(spec, &dir, &files)?,
    };
    // The answer may legitimately take the job-gate wait on a cold start;
    // anything past the deadline is a wedged daemon — fall back.
    stream
        .set_read_timeout(Some(ANSWER_TIMEOUT))
        .map_err(|error| format!("socket setup failed: {error}"))?;
    let mut line = serde_json::to_vec(&Ask {
        roots: spec.roots.clone(),
        aims: aims.to_vec(),
        face: face.cloned(),
        metrics,
    })
    .expect("serializes");
    line.push(b'\n');
    (&stream)
        .write_all(&line)
        .map_err(|error| format!("request failed: {error}"))?;
    let mut reader = BufReader::new(&stream);
    let mut answer = String::new();
    let mut legs = None;
    loop {
        answer.clear();
        reader
            .read_line(&mut answer)
            .map_err(|error| format!("no answer: {error}"))?;
        match serde_json::from_str::<Answer>(&answer) {
            Ok(Answer::Note(note)) => eprintln!("{note}"),
            Ok(Answer::Metrics(found)) => legs = Some(found),
            Ok(Answer::Report(report)) => return Ok(Answered::Report(report)),
            Ok(Answer::Face { meta, bytes }) => {
                let mut meta = meta;
                meta.legs = legs.take();
                let stdout = read_face(&mut reader, bytes)
                    .map_err(|error| format!("no face payload: {error}"))?
                    .ok_or_else(|| "the daemon announced an oversized face".to_string())?;
                return Ok(Answered::Face { stdout, meta });
            }
            Ok(Answer::Budget(message)) => return Ok(Answered::Budget(message)),
            Ok(Answer::NotServed) => return Ok(Answered::NotServed),
            Ok(Answer::Yield) => return Ok(Answered::Yielded),
            Ok(Answer::Fault(fault)) => return Err(fault),
            Err(_) => return Err("the daemon exited mid-request".into()),
        }
    }
}

/// Starts the daemon under the key's election lock (a racing client waits
/// on the lock, then finds the socket live) and connects.
#[cfg(unix)]
fn start(spec: &Spec, dir: &Path, files: &Files) -> Result<std::os::unix::net::UnixStream, String> {
    use std::os::unix::io::AsRawFd as _;
    use std::os::unix::net::UnixStream;
    use std::os::unix::process::CommandExt as _;
    use std::process::{Command, Stdio};

    std::fs::create_dir_all(dir)
        .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&files.elect)
        .map_err(|error| format!("cannot open {}: {error}", files.elect.display()))?;
    // SAFETY: a blocking exclusive flock on an owned fd; released when
    // `lock` drops.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err("the start lock failed".into());
    }
    if let Ok(stream) = UnixStream::connect(&files.socket) {
        return Ok(stream);
    }
    // A daemon whose socket is dead but whose pid lives is wedged: reap
    // it. The cmdline match guards pid reuse — a recycled pid that now
    // names an unrelated process is never signaled; only this directory's
    // own `--resident-serve` daemon qualifies.
    let dir_name = dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if let Some(pid) = std::fs::read_to_string(&files.state)
        .ok()
        .and_then(|pid| pid.trim().parse().ok())
        && dartq_toolchain::proc::alive_and_cmdline(pid, &[crate::SERVE_FLAG, dir_name])
    {
        dartq_toolchain::proc::kill_tree(pid);
    }
    let _ = std::fs::remove_file(&files.socket);
    let spec_bytes = serde_json::to_vec(spec).expect("a spec serializes");
    std::fs::write(&files.spec, spec_bytes)
        .map_err(|error| format!("cannot write the spec: {error}"))?;
    let exe = std::env::current_exe().map_err(|error| format!("cannot locate dartq: {error}"))?;
    let mut command = Command::new(exe);
    // The daemon outlives this client: it takes its own job slot.
    dartq_toolchain::ledger::detach(&mut command);
    command
        .args(["analyze", SERVE_FLAG])
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env_remove("DARTQ_SHIM")
        .process_group(0)
        .spawn()
        .map_err(|error| format!("cannot start the daemon: {error}"))?;
    let deadline = std::time::Instant::now() + BIND_TIMEOUT;
    loop {
        if let Ok(stream) = UnixStream::connect(&files.socket) {
            return Ok(stream);
        }
        if std::time::Instant::now() >= deadline {
            return Err("the daemon never listened".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::io::Cursor;
    use std::path::PathBuf;

    use super::{MAX_FACE_BYTES, direct_lease_request, fallback_line, read_face, up};
    use crate::Files;
    use crate::Spec;

    /// A refused worktree keeps its own daemon only while that daemon is
    /// up — a live pid in its state and its socket file present. An
    /// idled-out daemon (no socket, no state) or a dead pid sends the next
    /// run back to the pool, whose refusal starts the daemon again.
    #[test]
    fn a_worktrees_own_daemon_is_up_only_while_it_lives() {
        let dir = tempfile::tempdir().expect("daemon dir");
        let files = Files::of(dir.path());
        assert!(!up(dir.path()), "never started");
        std::fs::write(&files.state, std::process::id().to_string()).expect("state");
        assert!(!up(dir.path()), "no socket: idled out");
        std::fs::write(&files.socket, b"").expect("socket file");
        assert!(up(dir.path()), "a live pid and its socket");
        std::fs::write(&files.state, "999999999").expect("dead pid");
        assert!(!up(dir.path()), "a dead pid");
    }

    /// The wire-announced length gates the allocation: an over-cap header
    /// refuses before a byte is reserved or read (the cursor never moves),
    /// while an honest small payload round-trips byte-for-byte.
    #[test]
    fn read_face_refuses_over_cap_without_allocating_and_reads_honest_payloads() {
        let over_cap = u32::try_from(MAX_FACE_BYTES + 1).unwrap_or(u32::MAX);
        let mut refusing = Cursor::new(Vec::<u8>::new());
        assert_eq!(
            read_face(&mut refusing, over_cap).expect("an over-cap read is not an io error"),
            None,
            "an over-cap announcement is refused before any allocation or read"
        );
        assert_eq!(refusing.position(), 0, "a refused read consumes nothing");

        let payload = b"{v:1 diags:[]}\n".to_vec();
        let mut honest = Cursor::new(payload.clone());
        assert_eq!(
            read_face(&mut honest, u32::try_from(payload.len()).expect("len"))
                .expect("an honest payload reads"),
            Some(payload),
            "an in-cap payload round-trips"
        );

        let mut short = Cursor::new(b"ab".to_vec());
        assert!(
            read_face(&mut short, 16).is_err(),
            "a short read surfaces as the io error the caller falls back on"
        );
    }

    /// The per-run fallback's stderr line: the daemon's fault, the direct
    /// drive's, or both — and silence only when neither has a story (the
    /// cooling arm's silent child).
    #[test]
    fn the_fallback_line_names_every_fault_it_carries() {
        assert_eq!(fallback_line(None, None), None);
        assert_eq!(
            fallback_line(Some("the daemon never listened".into()), None).as_deref(),
            Some(
                "dartq analyze: resident analyzer unavailable \
                 (the daemon never listened) — running dart analyze"
            )
        );
        assert_eq!(
            fallback_line(None, Some("cannot start the analysis server".into())).as_deref(),
            Some(
                "dartq analyze: the direct analysis server failed \
                 (cannot start the analysis server) — running dart analyze"
            )
        );
        let both = fallback_line(
            Some("no answer".into()),
            Some("the analysis server never connected".into()),
        )
        .expect("both faults name both");
        assert!(
            both.contains("resident analyzer unavailable (no answer)"),
            "{both}"
        );
        assert!(
            both.contains("the direct server failed too (the analysis server never connected)"),
            "{both}"
        );
        assert!(both.ends_with("— running dart analyze"), "{both}");
    }

    /// The direct drive's lease request: the daemon server's own face,
    /// peak-key family and slot posture — one gate and one sizing history
    /// for a worktree's servers, never an ungated second server per run.
    /// No release handle: a one-shot dies with its user, the dead scan
    /// reaps the booking.
    #[test]
    fn the_direct_drive_books_the_daemons_gate_not_a_second_one() {
        let spec = Spec {
            sdk_root: PathBuf::from("/sdks/a"),
            sdk_version: "3.13.7".to_owned(),
            roots: vec![PathBuf::from("/w/a"), PathBuf::from("/w/b")],
            packages: None,
            budget: 4 << 30,
            idle_secs: 900,
            max_percent: 90,
            max_concurrent: 2,
            pool: false,
            max_roots: 0,
            warm: 0,
            warm_ttl_secs: 0,
        };
        let request = direct_lease_request("/repo:a", &spec, 1 << 30, true);
        assert_eq!(
            request.face,
            dartq_toolchain::ledger::Face::AnalysisServer,
            "the same face the daemon's servers book under"
        );
        assert_eq!(request.label, "/w/a");
        assert_eq!(request.bytes, 1 << 30);
        assert!(request.sized);
        assert_eq!(request.peak_key.as_deref(), Some("/repo:a"));
        assert!(
            request.slot,
            "a one-shot takes the same slot a daemon server-start does"
        );
        assert_eq!(request.cap, dartq_toolchain::ledger::cap(2));
        assert_eq!(request.release, None);
        assert_eq!(
            request.pid, None,
            "the server pid lands on the lease only after track"
        );
        assert_eq!(request.source_bytes, None);
    }
}
