//! The `dartq analyze` side of the resident server: find the daemon of the
//! request's key, start it when none is live, ask, and answer. Every
//! failure is an [`Outcome::PerRun`]: the caller runs `dart analyze` per
//! run, so the resident path can only make a run faster, never fail it.

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
const FAILED_COOLDOWN: Duration = Duration::from_secs(10 * 60);
/// The most one answer may take once the request is written: covers the
/// daemon's own worst wait (the 30-minute job-gate lease wait of
/// `daemon::MEMORY_WAIT`) plus slack. A wedged daemon hits the deadline
/// and the caller falls back instead of hanging forever.
#[cfg(unix)]
const ANSWER_TIMEOUT: Duration = Duration::from_secs(35 * 60);

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
    /// The output face this run prints. `Some` asks the daemon for the
    /// rendered stdout bytes (the face path); `None` keeps the report
    /// line (the compat arm — the pooled join never carries one).
    pub face: Option<Face>,
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
    let roots: Result<Vec<PathBuf>, _> =
        request.roots.iter().map(std::fs::canonicalize).collect();
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
    };
    #[cfg(unix)]
    {
        let face = request.face.clone();
        let asked = if spec.pool {
            let mut own = spec.clone();
            own.pool = false;
            if up(&own.dir()) {
                // A worktree the pool refused runs its own daemon, and
                // keeps it while it lives: re-asking the pool would queue
                // every request behind the pool's own work just to be
                // refused again. Once the daemon idles out, the next run
                // asks the pool first.
                ask_gated(&own, &aims, face.as_ref())
            } else {
                // The pool-first path: join the live pooled daemon, or
                // found it. A refused join falls back to this worktree's
                // own daemon — started on demand, never silently a second
                // pooled server.
                match pool_join(&spec, &aims, face.as_ref()) {
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
                        ask_gated(&own, &aims, face.as_ref())
                    }
                }
            }
        } else {
            ask_gated(&spec, &aims, face.as_ref())
        };
        match asked {
            Ok(Answered::Report(report)) => Outcome::Report { report, sdk },
            Ok(Answered::Face { stdout, meta }) => Outcome::Face { stdout, meta, sdk },
            Ok(Answered::Budget(message)) => Outcome::Budget(message),
            // `Yielded` is handled inside `ask_gated`'s re-queue loop; a
            // bare `ask` (the pool path) maps it to `Unavailable` there.
            Ok(Answered::Yielded) | Ok(Answered::Cooling) | Ok(Answered::NotServed) => {
                Outcome::PerRun {
                    reason: None,
                    sdk: Some(sdk),
                }
            }
            Err(reason) => Outcome::PerRun {
                reason: Some(format!(
                    "dartq analyze: resident analyzer unavailable ({reason}) — running dart analyze"
                )),
                sdk: Some(sdk),
            },
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
    Face { stdout: Vec<u8>, meta: FaceMeta },
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
const YIELD_REQUEUE: Duration = Duration::from_secs(30 * 60);

/// [`ask`] with the growth brake's re-queue: an `Answer::Yield` means the
/// daemon's server was the youngest heavy holder on a critically low host
/// and has been killed with its peak recorded — the client waits a beat,
/// then re-issues the whole ask (the daemon's `lease_slot` re-enters the
/// job gate FIFO; its notes stream to stderr as usual). Bounded by
/// [`YIELD_REQUEUE`] — past it the run is a [`Outcome::Budget`]-shaped
/// failure, never a per-run `dart analyze`, which would need the same
/// memory the ledger just refused.
#[cfg(unix)]
fn ask_gated(spec: &Spec, aims: &[crate::Aim], face: Option<&Face>) -> Result<Answered, String> {
    let deadline = std::time::Instant::now() + YIELD_REQUEUE;
    let mut yields = 0u32;
    loop {
        match ask(spec, aims, face)? {
            Answered::Yielded => {
                yields += 1;
                if std::time::Instant::now() + Duration::from_millis(500) >= deadline {
                    return Ok(Answered::Budget(format!(
                        "dartq analyze: host memory stayed critically low through {} yields — \
                         the analyzer never got its memory back",
                        yields
                    )));
                }
                if yields == 1 {
                    let free = dartq_toolchain::ledger::available_memory()
                        .map(|bytes| format!("{:.1} GB free", bytes as f64 / f64::from(1u32 << 30)))
                        .unwrap_or_else(|| "low memory".into());
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
    Face { stdout: Vec<u8>, meta: FaceMeta },
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
fn pool_join(spec: &Spec, aims: &[crate::Aim], face: Option<&Face>) -> Option<PoolAnswer> {
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
    })
    .expect("serializes");
    line.push(b'\n');
    (&stream).write_all(&line).ok()?;
    let mut reader = BufReader::new(&stream);
    let mut answer = String::new();
    loop {
        answer.clear();
        reader.read_line(&mut answer).ok()?;
        match serde_json::from_str::<Answer>(&answer) {
            Ok(Answer::Note(note)) => eprintln!("{note}"),
            Ok(Answer::Report(report)) => return Some(PoolAnswer::Report(report)),
            Ok(Answer::Face { meta, bytes }) => {
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
            Ok(Answer::NotServed) | Ok(Answer::Yield) | Ok(Answer::Fault(_)) | Err(_) => {
                return Some(PoolAnswer::Unavailable);
            }
        }
    }
}

/// The unix transport: election, start, request.
#[cfg(unix)]
fn ask(spec: &Spec, aims: &[crate::Aim], face: Option<&Face>) -> Result<Answered, String> {
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
    })
    .expect("serializes");
    line.push(b'\n');
    (&stream)
        .write_all(&line)
        .map_err(|error| format!("request failed: {error}"))?;
    let mut reader = BufReader::new(&stream);
    let mut answer = String::new();
    loop {
        answer.clear();
        reader
            .read_line(&mut answer)
            .map_err(|error| format!("no answer: {error}"))?;
        match serde_json::from_str::<Answer>(&answer) {
            Ok(Answer::Note(note)) => eprintln!("{note}"),
            Ok(Answer::Report(report)) => return Ok(Answered::Report(report)),
            Ok(Answer::Face { meta, bytes }) => {
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
    let dir_name = dir.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    if let Some(pid) = std::fs::read_to_string(&files.state)
        .ok()
        .and_then(|pid| pid.trim().parse().ok())
        && dartq_toolchain::proc::alive_and_cmdline(
            pid,
            &[crate::SERVE_FLAG, dir_name],
        )
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

    use super::{MAX_FACE_BYTES, read_face, up};
    use crate::Files;

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
        assert_eq!(
            refusing.position(),
            0,
            "a refused read consumes nothing"
        );

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
}
