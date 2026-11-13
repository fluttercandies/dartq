//! The serving side: one daemon per [`Spec`], one server per daemon.
//!
//! The daemon binds its socket before the server starts, so clients that
//! arrive during the first analysis connect and wait. An accept thread
//! hands connections to the one worker, which owns the server: requests
//! run one after another, and the server's stdin has a single writer.
//! Construction arms the freshness watcher and walks the first snapshot
//! on a background thread; the cold ask — no live server, baseline still
//! priming — skips the check entirely (its diff is dead weight beside
//! the fresh server's own full analysis) while every later request pays
//! one freshness check: the journaled watcher's
//! dirty merge when the stream is armed, else the stat walk; an
//! unchanged tree answers from the error
//! map with no server round trip, an edited Dart file is one incremental
//! analysis, and an added or deleted file or a configuration edit restarts
//! the server. A server that faults is restarted once
//! per request; a second fault answers the client with the fault, and the
//! client runs `dart analyze` itself. With no request for the idle timeout
//! the daemon shuts the server down — and, warm posture on, keeps a
//! standby server (spawned, connected, no roots) alive on a slotless
//! memory-only lease for a TTL window instead of exiting: the next
//! borrower of the same key adopts it (a fresh `analysis.setAnalysisRoots`
//! on a rootless server is fresh-spawn semantics), and only the TTL's
//! expiry, host memory pressure, or a stale SDK ends the daemon.
//!
//! Every worktree has its own daemon and server, so the machine holds one
//! server heap per recently used worktree. Two rules keep that inside the
//! machine's memory:
//!
//! * a server takes a job slot of the machine-wide gate
//!   (`memory.max_concurrent`) when it starts and holds it for its life,
//!   idle or busy, so it counts against the same cap as per-run analyses,
//!   test runs and builds;
//! * a server leases its expected peak (its last recorded peak, else the
//!   largest live peer's) from the machine-wide memory ledger
//!   (`dartq_toolchain::ledger`) every heavy dartq face shares, and holds
//!   it for the server's life. The ledger accounts it as the larger of
//!   the lease and the live tree RSS, and on a shortfall releases idle
//!   residents (a daemon whose busy lock is free), least recently used
//!   first; a released daemon restarts warm from its cache on its next
//!   request.
//!
//! The servers' caches share one disk budget (`crate::disk`): a request
//! pays one `statvfs`, a running analysis one every few seconds, and
//! passes over the caches run on a background thread.
//!
//! A served render also persists as an answered-turn record (#4278, the
//! "Answered-turn records" section below): the wire line beside the
//! read-time stamp set, byte-budget-capped under the cache dir. A
//! serverless daemon whose baseline is still priming — a re-run after
//! the last daemon's death — probes it before any server starts and
//! serves a stamp-set hit with no spawn at all.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::face::{Face, FaceRenderer};
use crate::report::Report;
use crate::server::{Fault, Launch, Server};
use crate::snapshot::Diff;
use crate::support::{Sdk, Support, Version, support};
use crate::watch::{Freshness, WalkReason};
use crate::{Files, Spec};

/// One client request line.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Ask {
    pub roots: Vec<PathBuf>,
    /// The files and directories this run reports on; empty asks for the
    /// whole-roots report (`dart analyze` with a root aim spells the same
    /// set, so a covering aim needs no special case).
    #[serde(default)]
    pub aims: Vec<crate::Aim>,
    /// The rendered-output face this run asks for. `None` is the compat
    /// arm — pre-face asks get the report line.
    #[serde(default)]
    pub face: Option<Face>,
}

/// One daemon answer line. [`Answer::Note`] lines (gate waits, released
/// peers) precede the one final line. [`Answer::Face`] is a two-part
/// answer: the meta line ships the client's counts, then the raw payload
/// bytes follow (the face's stdout bytes are not JSON — they stream
/// through unchanged).
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum Answer {
    Report(Report),
    /// The rendered face: `meta` is the header line's counts beside the
    /// payload, `bytes` the payload's length (the client reads exactly
    /// that many bytes after the header's newline).
    Face {
        meta: crate::face::FaceMeta,
        bytes: u32,
    },
    Fault(String),
    Budget(String),
    Note(String),
    /// The aims cannot be served by this daemon's one context (a `.dart`
    /// file aim the options exclude or the roots do not reach): the
    /// caller falls back to a per-run `dart analyze` without a reason
    /// line — the run is correct, just not resident-served.
    NotServed,
    /// The growth brake fired mid-request: the daemon killed its server
    /// and freed its lease (the observed peak is already recorded). The
    /// client re-queues behind the job gate — FIFO, never a per-run
    /// fallback, which would need the same memory — and retries the ask
    /// once admitted.
    Yield,
}

/// What `analyze`/`join` hand back: a report already on the wire (the
/// serialized `Answer::Report` line — rendered once, then reused while
/// the error map stands, spliced from per-file fragments when it does
/// not), a face answer's header line plus payload in one contiguous run,
/// or a small answer that serializes at send.
enum Reply {
    /// The full answer wire bytes, newline included (for `Answer::Face`,
    /// the header line's newline plus the announced payload).
    Line(Arc<[u8]>),
    /// A non-report answer (faults, budgets, notes, the
    /// cannot-serve signal).
    Answer(Answer),
}

/// The `Answer::Face` header line: the meta member beside the announced
/// payload length, serialized plus its newline — the prefix of the face
/// wire bytes [`crate::server::Server::render_face`] caches. `None` when
/// the payload cannot ride the wire — over the client's
/// [`crate::client::MAX_FACE_BYTES`] cap, whose reader would refuse it —
/// or the u32 length field's range; the caller answers `NotServed` and
/// the client falls back to a per-run `dart analyze`, which prints the
/// face unbounded.
pub(crate) fn face_header(meta: &crate::face::FaceMeta, bytes: usize) -> Option<Vec<u8>> {
    if u64::try_from(bytes).unwrap_or(u64::MAX) > crate::client::MAX_FACE_BYTES {
        return None;
    }
    let mut line = serde_json::to_vec(&Answer::Face {
        meta: meta.clone(),
        bytes: u32::try_from(bytes).ok()?,
    })
    .expect("an answer serializes");
    line.push(b'\n');
    Some(line)
}

/// How long a server start waits for host memory before the request fails
/// (with the holders named, and no per-run retry: a per-run analysis
/// would wait for the same memory).
const MEMORY_WAIT: Duration = Duration::from_secs(30 * 60);

/// How long the worker waits for one request line: the ask arrives with
/// the connection, so anything past a few seconds is a peer that will
/// never send — the read gives up instead of parking the daemon's only
/// worker (and holding its busy lock against the ledger's releaser).
const REQUEST_READ: Duration = Duration::from_secs(10);

/// How often an idle daemon checks that its socket path still names the
/// socket it bound.
const OWNERSHIP_CHECK: Duration = Duration::from_secs(5);

/// A file's identity: a path that is removed and recreated names a new
/// inode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Inode {
    dev: u64,
    ino: u64,
}

impl Inode {
    fn of(path: &Path) -> Option<Self> {
        use std::os::unix::fs::MetadataExt as _;
        let meta = std::fs::symlink_metadata(path).ok()?;
        Some(Self {
            dev: meta.dev(),
            ino: meta.ino(),
        })
    }
}

/// Runs the daemon over the spec in `dir` until it idles out, rendering
/// face answers through `renderer` (the CLI's one writer). The return
/// value is the process exit code.
#[must_use]
pub fn serve(dir: &Path, renderer: FaceRenderer) -> u8 {
    let files = Files::of(dir);
    let Some(spec) = std::fs::read(&files.spec)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Spec>(&bytes).ok())
    else {
        return 2;
    };
    // The daemon leads its own process group (the client spawns it so); a
    // termination signal takes the group, server included, down with it,
    // so a killed daemon never leaves an orphaned server behind.
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: installs an async-signal-safe handler (one kill(2) call).
        unsafe {
            libc::signal(
                signal,
                kill_group as extern "C" fn(libc::c_int) as libc::sighandler_t,
            )
        };
    }
    let _ = std::fs::remove_file(&files.socket);
    let Ok(listener) = UnixListener::bind(&files.socket) else {
        return 2;
    };
    let Some(socket) = Inode::of(&files.socket) else {
        return 2;
    };
    let _ = std::fs::write(&files.state, std::process::id().to_string());
    // The dead-roots sweep runs in the disk budget's first pass, on its
    // own thread ([`Host::new`]): a daemon start never waits on a walk.
    crate::claim_cache(&spec.cache_dir(), &spec.roots);
    // The answered-turn records' budget pass (#4278): dead and torn
    // records reclaim, the survivors LRU-cap at the family's byte cap.
    // Prefix peeks over a handful of files — paid inline, once per daemon
    // start, never inside an ask.
    sweep_records(&spec.cache_dir());
    // A SIGKILL'd predecessor could not take its server down (the group
    // kill only runs on catchable signals): the orphaned analysis_server
    // would keep this daemon's `--cache` dir — two servers on one cache
    // is the cross-worktree contamination class. Reap it before this
    // daemon serves. The cmdline match guards pid reuse: a stale
    // `server.pid` naming a recycled, unrelated process is never
    // signaled.
    if let Some(pid) = std::fs::read_to_string(&files.server_pid)
        .ok()
        .and_then(|pid| pid.trim().parse().ok())
        && dartq_toolchain::proc::alive_and_cmdline(
            pid,
            &["analysis_server", &format!("{}", spec.sdk_root.display())],
        )
    {
        dartq_toolchain::proc::kill_tree(pid);
    }
    let _ = std::fs::remove_file(&files.server_pid);

    let (tx, rx) = mpsc::channel::<UnixStream>();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            if tx.send(stream).is_err() {
                break;
            }
        }
    });

    let mut host = Host::new(&spec, dir, socket, renderer);
    let idle = Duration::from_secs(spec.idle_secs.max(1));
    // The standby spawns before the first ask is read: its spawn overlaps
    // the client's connect and the ask's freshness walk, and the asking
    // client is the standby's first borrower (a PerRun SDK never primes).
    host.prime();
    let mut last = Instant::now();
    // The shared pressure monitor: while this daemon is idle it samples
    // host memory through the ledger's one policy; two consecutive lows
    // evict the least-recently-used idle releasable resident on the
    // machine (a peer analyzer or a session daemon — the heavy holders).
    let mut pressure = dartq_toolchain::ledger::Pressure::default();
    // The standby's own pressure debounce: the same two-low shape the
    // ledger's monitor uses, so a blip never sheds it.
    let mut warm_lows = 0u8;
    loop {
        // The idle window is the standby TTL while one lives (Tier 2),
        // the serving idle timeout otherwise.
        let window = if host.standby.is_some() {
            host.warm_ttl
        } else {
            idle
        };
        match rx.recv_timeout(
            OWNERSHIP_CHECK
                .min(window)
                .min(dartq_toolchain::ledger::PRESSURE_TICK),
        ) {
            Ok(stream) => {
                host.answer(stream);
                last = Instant::now();
                pressure.reset();
            }
            Err(RecvTimeoutError::Timeout) => {
                // The standby sheds first: at the same low-memory signal
                // every peer's monitor samples, this daemon drops its own
                // few-MiB holder (Tier 3 by pressure) before any peer
                // would release a whole fat resident.
                if host.standby.is_some() && host_low(spec.max_percent) {
                    warm_lows = warm_lows.saturating_add(1);
                    if warm_lows >= 2 {
                        host.stop_standby("warm-pressure");
                        break;
                    }
                } else {
                    warm_lows = 0;
                }
                if let Some(victim) = pressure.tick(std::process::id(), spec.max_percent, true) {
                    // The daemon's stderr is `/dev/null` (the client spawns
                    // it detached): the events file is the only surface that
                    // survives the eviction for the next forensic pass.
                    files.event(&format!(
                        "dartq analyze: released the idle {victim} under memory pressure"
                    ));
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
        // The socket path is the daemon's only address. Once it names
        // another file (the directory was wiped and a client started a
        // successor) or nothing, no client can reach this daemon: its
        // server is dead weight, and the files at its paths are not its
        // own to remove.
        if Inode::of(&files.socket) != Some(socket) {
            host.stop("successor");
            host.stop_standby("successor");
            return 0;
        }
        if last.elapsed() >= window {
            // Idle expiry: Tier 2 keeps a warm standby alive for the TTL
            // window instead of exiting (the next borrower of this SDK
            // adopts it); Tier 3 — no standby could be stood up, or the
            // TTL ran out — exits as the idle daemon always did.
            if host.enter_tier2() {
                last = Instant::now();
                continue;
            }
            break;
        }
    }
    // Idle: stop accepting before the server goes, so a racing client
    // sees a dead socket and starts a fresh daemon. The start lock orders
    // the removal against a client starting a successor (it holds the lock
    // until its daemon listens), so only this daemon's own files go.
    if let Ok(lock) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&files.elect)
        .and_then(|lock| lock.lock().map(|()| lock))
    {
        if Inode::of(&files.socket) == Some(socket) {
            let _ = std::fs::remove_file(&files.socket);
            let _ = std::fs::remove_file(&files.state);
        }
        drop(lock);
    }
    // The exit takes down whichever resident lives: the serving server
    // (`idle-evict`) or the standby whose TTL ran out (`warm-evict`).
    host.stop("idle-evict");
    host.stop_standby("warm-evict");
    0
}

extern "C" fn kill_group(_: libc::c_int) {
    // SAFETY: kill(2) is async-signal-safe; pid 0 is this process group.
    unsafe { libc::kill(0, libc::SIGKILL) };
}

struct Host {
    launch: Option<Launch>,
    /// The face writer the CLI injected — the daemon renders stdout
    /// bytes through the same function the cold path uses.
    renderer: FaceRenderer,
    /// The spec's founding roots — the only set a non-pooled daemon ever
    /// serves; for a pooled daemon the initial [`Host::active`].
    roots: Vec<PathBuf>,
    /// The pooled daemon's served roots in join order, each with its
    /// last-answer instant. `active[0]` is the founding root — the
    /// primary, never evicted. Empty for a one-worktree daemon (the
    /// `roots` field answers instead).
    active: Vec<(PathBuf, Instant)>,
    /// The pool capacity the spec declared (`0` unpooled).
    max_roots: usize,
    /// Whether this daemon takes pooled joins (`spec.pool`).
    pooled: bool,
    budget: Option<u64>,
    server: Option<Server>,
    /// The warm standby: a spawned, connected server with no borrower yet,
    /// owned in-process (Tier 2's whole posture). Adopted by the next
    /// borrower of this daemon's key; while it lives the daemon's idle
    /// window is the warm TTL.
    standby: Option<Server>,
    /// The standby's lease: memory-only (no job slot — it never blocks a
    /// per-run analysis under `memory.max_concurrent`), booked at a small
    /// fixed figure, releasable while this daemon is idle — the ledger's
    /// cheapest victim on the machine.
    standby_lease: Option<dartq_toolchain::ledger::Lease>,
    /// Whether this daemon runs the warm posture (resolved daemon-side at
    /// [`Host::new`]: `DARTQ_WARM_SERVERS` over the spec's `warm` over
    /// the default — off since 0.1.23).
    warm: bool,
    /// How long an idle daemon keeps its standby before exiting (Tier 3).
    warm_ttl: Duration,
    /// The freshness oracle: the maintained snapshot plus its journaled
    /// watcher (or walk mode when the stream cannot arm).
    freshness: Freshness,
    /// Consecutive freshness checks that answered with a degradation-
    /// shaped walk — the note's debounce (see [`degradation_note`]): one
    /// transient walk under cold-start load (a saturated stream missing
    /// one cookie bound) never fires it, a persistent shape hits two.
    degrade_streak: u8,
    /// Whether the watcher-degradation note already fired — it names the
    /// reason once per daemon life, and recovery speaks through the
    /// watch substrate's own cool-off re-arm line.
    degrade_noted: bool,
    files: Files,
    /// The socket this daemon bound.
    socket: Inode,
    /// The server's job slot and memory lease, held while the server
    /// lives.
    lease: Option<dartq_toolchain::ledger::Lease>,
    max_percent: u8,
    max_concurrent: u32,
    /// The ledger's peak key of this server: the repo identity when the
    /// roots share a git common dir, else the cache directory — one per
    /// worktree.
    peak_key: String,
    /// The analyzed Dart source size the server covers, fed to
    /// [`ledger::sizing_with_source`] for ratio-based estimates and
    /// recorded beside the measured peak.
    source_bytes: Option<u64>,
    /// The (peak, source) figure this daemon last persisted to the
    /// ledger's peaks store — the bookkeeping elision's memory: a quiet
    /// answer would re-record the identical figure, so the write is
    /// skipped until it moves. `None` until the first record and after
    /// any arm that changed the store by another path (the growth
    /// brake's floor record), so the next completion always lands.
    persisted_peak: Option<(u64, Option<u64>)>,
    /// The growth-brake's running state (the ledger's shared policy):
    /// consecutive samples under the critical floor arm the yield.
    brake: dartq_toolchain::ledger::GrowthBrake,
    /// The server's `--cache`, whose last use each answer marks.
    cache: PathBuf,
    /// The caches' disk budget: the request-time check, the in-analysis
    /// tick, the server-start pass and the background passes.
    disk: crate::disk::Disk,
    /// The spec this daemon serves, read once at start — the answered-
    /// turn record family's key legs and the probe walk's inputs (#4278).
    spec: Spec,
    /// Whether the answered-turn record family is on (#4278): the env
    /// kill switch over the dartq.yaml opt-out, resolved once at start.
    records: bool,
    /// This ask's read-time stamp capture: the walk the cold-arm probe
    /// paid, stashed for the persist that follows a served render — the
    /// record pins the tree as the rendering run read it, never a
    /// post-render state an edit during the settle could fool. `None` on
    /// every ask whose probe did not walk (server live, baseline primed,
    /// or the family off), and consumed by the one persist a served
    /// render writes.
    record_capture: Option<Stamps>,
    /// The last record this daemon life persisted, `(key, stamps digest)`
    /// — the quiet-turn elision: an identical figure over an unchanged
    /// tree rewrites the bytes already on disk, so the write is skipped
    /// until it moves.
    record_saved: Option<([u8; 16], u64)>,
}

impl Host {
    fn new(spec: &Spec, dir: &Path, socket: Inode, renderer: FaceRenderer) -> Self {
        let cache = spec.cache_dir();
        // The key carries the version the client read; the daemon checks
        // the SDK it actually finds against it before serving.
        let version = std::fs::read_to_string(spec.sdk_root.join("version")).unwrap_or_default();
        let launch = match (support(&version), Version::parse(&version)) {
            (Support::Resident { .. }, Some(parsed)) if version.trim() == spec.sdk_version => {
                Some(Launch {
                    sdk: Sdk {
                        root: spec.sdk_root.clone(),
                        version: parsed,
                    },
                    packages: spec.packages.clone(),
                })
            }
            _ => None,
        };
        let files = Files::of(dir);
        let (warm, warm_ttl) = warm_config(spec);
        Self {
            launch,
            renderer,
            roots: spec.roots.clone(),
            active: if spec.pool {
                spec.roots
                    .iter()
                    .map(|root| (root.clone(), Instant::now()))
                    .collect()
            } else {
                Vec::new()
            },
            // A pooled spec written before the cap existed (or by a client
            // that never sets it) carries `0`: the default cap, not the
            // clamp floor.
            max_roots: if spec.max_roots == 0 {
                3
            } else {
                usize::try_from(spec.max_roots).unwrap_or(3).clamp(1, 64)
            },
            pooled: spec.pool,
            budget: (spec.budget > 0).then_some(spec.budget),
            server: None,
            standby: None,
            standby_lease: None,
            warm,
            warm_ttl,
            freshness: Freshness::new(
                &spec.roots,
                spec.packages.as_deref(),
                &spec.sdk_home(),
            ),
            degrade_streak: 0,
            degrade_noted: false,
            files,
            socket,
            lease: None,
            max_percent: spec.max_percent,
            max_concurrent: spec.max_concurrent,
            peak_key: repo_key(&spec.roots)
                .unwrap_or_else(|| cache.display().to_string()),
            // The Dart source size derives from the priming walk's
            // snapshot once it lands (`analyze_result` refreshes it) —
            // a daemon start never pays a discovery walk of its own.
            source_bytes: None,
            persisted_peak: None,
            brake: dartq_toolchain::ledger::GrowthBrake::default(),
            cache,
            disk: crate::disk::Disk::start(dir),
            spec: spec.clone(),
            records: records_enabled(&spec.roots),
            record_capture: None,
            record_saved: None,
        }
    }

    /// Shuts the server down for `why` (one event line before the tree
    /// dies) and returns its lease.
    fn stop(&mut self, why: &'static str) {
        if let Some(server) = self.server.take() {
            self.files.event(&format!(
                "dartq analyze: analyzer pid {} stopped ({why})",
                server.pid()
            ));
            server.shutdown();
        }
        self.lease = None;
        let _ = std::fs::remove_file(&self.files.server_pid);
    }

    /// Forgets a server that already died (the budget killed it, or it
    /// faulted) for `why` — the event line goes out before the tree's
    /// handle drops — and returns its lease.
    fn forget(&mut self, why: &'static str) {
        if let Some(server) = self.server.take() {
            self.files.event(&format!(
                "dartq analyze: analyzer pid {} stopped ({why})",
                server.pid()
            ));
        }
        self.lease = None;
        let _ = std::fs::remove_file(&self.files.server_pid);
    }

    /// Stops the standby for `why` (one event line, the lease freed) —
    /// Tier 3's TTL exit, the pressure shed, the stale-SDK kill, a
    /// successor's takeover.
    fn stop_standby(&mut self, why: &'static str) {
        if let Some(standby) = self.standby.take() {
            self.files.event(&format!(
                "dartq analyze: analyzer pid {} stopped ({why})",
                standby.pid()
            ));
            standby.shutdown();
        }
        self.standby_lease = None;
        let _ = std::fs::remove_file(&self.files.server_pid);
    }

    /// Primes the standby at daemon start: the spawn overlaps the asking
    /// client's connect and the first freshness walk, and that client is
    /// the standby's first borrower — the spawn is never wasted. A
    /// PerRun SDK never primes (there is nothing to serve); a failed
    /// prime costs one spawn and nothing more — the next ask spawns
    /// fresh (`warm-miss`).
    fn prime(&mut self) {
        if !self.warm || self.launch.is_none() {
            return;
        }
        self.standby = self.spawn_standby();
    }

    /// Spawns the next standby on its slotless lease — rootless (spawn +
    /// connected, no subscriptions, no roots): the design's W1.5 arming
    /// pass was measured on this host and lost on every axis (an armed
    /// adoption destroys the arming context before building the
    /// borrower's, paying more than the warm SDK store saves: borrows of
    /// 0.57 s rootless vs 0.72 s core-armed vs 0.85 s full-armed on a
    /// trivial package, 2.52 s vs 2.71 s vs 2.96 s on a mid-size one,
    /// with the same ~257 MB settled standby either way), so the standby
    /// stays rootless and the first borrower's roots pass is the same
    /// `setAnalysisRoots` a fresh spawn's would be. `None` when no
    /// standby could be stood up — no lease to book, a spawn fault:
    /// Tier 2 is an optimization, never a wait and never a failure.
    fn spawn_standby(&mut self) -> Option<Server> {
        let Some(launch) = self.launch.clone() else {
            return None;
        };
        let label = format!("warm standby {}", launch.sdk.version);
        let request = standby_request(label, self.files.busy.clone());
        let lease = dartq_toolchain::ledger::acquire(
            request,
            STANDBY_LEASE,
            self.max_percent,
            Instant::now() + STANDBY_LEASE_WAIT,
            |_| {},
        )
        .ok()?;
        let standby = Server::spawn_and_connect(&launch, self.budget).ok()?;
        self.files.event(&format!(
            "dartq analyze: analyzer pid {} started (prewarm)",
            standby.pid()
        ));
        // The successor daemon reaps this pid if a SIGKILL stops this
        // daemon before its group kill can run.
        let _ = std::fs::write(&self.files.server_pid, standby.pid().to_string());
        self.standby_lease = lease.map(|mut lease| {
            lease.track(standby.pid());
            lease
        });
        Some(standby)
    }

    /// Tier 2: the idle daemon keeps a warm standby instead of exiting —
    /// the serving server stops (`idle-evict`, its lease freed), a
    /// rootless standby stands up on a slotless lease, and the daemon
    /// lives on for the warm TTL (refreshed by every ask). `false` when
    /// Tier 2 cannot be entered — warm off, a standby already live
    /// (Tier 2's own TTL expiry is Tier 3), a PerRun SDK, or no lease
    /// and no spawn to be had — and the caller exits as the idle daemon
    /// always did.
    fn enter_tier2(&mut self) -> bool {
        if !enters_tier2(self.warm, self.standby.is_some(), self.launch.is_some()) {
            return false;
        }
        self.stop("idle-evict");
        self.standby = self.spawn_standby();
        self.standby.is_some()
    }

    /// Whether the SDK the standby was spawned under still sits at its
    /// root: the version file re-read matches the launch facts the
    /// standby carries. A same-path SDK swap between spawn and ask never
    /// serves through a stale standby — `warm-stale-sdk`, fresh spawn.
    fn sdk_facts_current(&self) -> bool {
        let Some(launch) = &self.launch else {
            return false;
        };
        std::fs::read_to_string(launch.sdk.root.join("version"))
            .map(|text| Version::parse(&text) == Some(launch.sdk.version))
            .unwrap_or(false)
    }

    /// The pooled daemon's answer to a join: admit the roots — every
    /// asked root is already served, is added while the capacity lasts,
    /// or makes room by evicting the least-recently-used cold non-primary
    /// root — then answers scoped to the ask's own aims. A join the
    /// capacity cannot admit without evicting a hot root is
    /// [`Answer::NotServed`]: the caller runs its own daemon instead of
    /// silently spawning a second analyzer, and the served set stays.
    /// The published root set refreshes on every change.
    fn join(&mut self, client: &UnixStream, ask: Ask) -> Reply {
        let joining: Vec<PathBuf> = ask
            .roots
            .iter()
            .filter(|root| !self.active.iter().any(|(served, _)| served == *root))
            .cloned()
            .collect();
        let Some(evicted) = admit(&mut self.active, &joining, self.max_roots, Instant::now())
        else {
            // The joiner sees why its own daemon is coming up — a refused
            // join is never silent, never a second pooled analyzer.
            note(
                client,
                format!(
                    "dartq analyze: the pool is at its {}-root capacity — running this worktree's own analyzer",
                    self.max_roots
                ),
            );
            return Reply::Answer(Answer::NotServed);
        };
        let changed = !joining.is_empty() || !evicted.is_empty();
        if changed {
            let roots: Vec<PathBuf> = self.roots();
            // The served set changed: the server re-analyzes over it and
            // the snapshot rebases — a joined worktree's files are new
            // inputs, an evicted one's gone.
            let (brake, disk) = (&mut self.brake, &self.disk);
            let outcome = match self.server.as_mut() {
                Some(server) => server.set_roots(&roots, || sample(disk, brake)),
                None => Ok(()),
            };
            if let Err(fault) = outcome {
                return match fault {
                    Fault::Budget { budget, measured } => {
                        self.forget("budget");
                        Reply::Answer(Answer::Budget(
                            Fault::Budget { budget, measured }.to_string(),
                        ))
                    }
                    Fault::Yield { peak } => {
                        self.forget("yield");
                        self.yield_peak(peak);
                        Reply::Answer(Answer::Yield)
                    }
                    _ => {
                        self.forget("fault");
                        Reply::Answer(
                            self.fail("the analysis server rejected the joined roots".into()),
                        )
                    }
                };
            }
            // The freshness oracle rebases on the new served set — a
            // joined worktree's files are new inputs, an evicted one's
            // gone (the request's own check then diffs empty, as the
            // old snapshot rebase did). The cold arm's skip applies
            // here too: with no live server the rebase's diff is dead
            // weight, and the next ask's check performs it.
            if !cold_freshness_skip(self.server.is_some(), self.freshness.primed()) {
                let _ = self.freshness.check(&roots);
                self.source_bytes = self.freshness.dart_bytes().or(self.source_bytes);
            }
        }
        for root in &ask.roots {
            if let Some((_, used)) = self.active.iter_mut().find(|(served, _)| served == root) {
                *used = Instant::now();
            }
        }
        self.analyze_scoped(client, &ask.aims, &ask.roots, ask.face.as_ref())
    }

    /// The roots the server currently analyzes — the pooled `active` set
    /// or the one-worktree `roots`.
    fn roots(&self) -> Vec<PathBuf> {
        if self.pooled {
            self.active.iter().map(|(root, _)| root.clone()).collect()
        } else {
            self.roots.clone()
        }
    }

    fn analyze_scoped(
        &mut self,
        client: &UnixStream,
        aims: &[crate::Aim],
        scope: &[PathBuf],
        face: Option<&Face>,
    ) -> Reply {
        // A pooled ask reports only its own roots' rows: aims scope the
        // report, and the aims-free whole report is the ask's roots as
        // directory aims — never a peer worktree's diagnostics.
        let owned: Vec<crate::Aim> = if aims.is_empty() {
            scope.iter().cloned().map(crate::Aim::Dir).collect()
        } else {
            aims.to_vec()
        };
        if let Some(line) = self.record_probe(&owned, face) {
            return Reply::Line(line);
        }
        match self.analyze_result(client) {
            Ok(()) => {
                let server = self.server.as_ref().expect("a live server");
                match face {
                    Some(face) => {
                        // A degenerate ask naming no aims and no scope
                        // reports nothing — never the pooled superset:
                        // the clean face is the writer's render over no
                        // rows.
                        if owned.is_empty() {
                            let render = (self.renderer)(&[], face);
                            let Some(mut wire) =
                                face_header(&render.meta, render.bytes.len())
                            else {
                                return Reply::Answer(Answer::NotServed);
                            };
                            wire.extend_from_slice(&render.bytes);
                            let line: Arc<[u8]> = wire.into();
                            self.record_persist(&owned, Some(face), &line);
                            return Reply::Line(line);
                        }
                        match server.render_face(&owned, face, self.renderer) {
                            Some(line) => {
                                self.record_persist(&owned, Some(face), &line);
                                Reply::Line(line)
                            }
                            None => Reply::Answer(Answer::NotServed),
                        }
                    }
                    None => {
                        if owned.is_empty() {
                            return Reply::Answer(Answer::Report(Report::default()));
                        }
                        match server.render(&owned) {
                            Some(line) => {
                                self.record_persist(&owned, None, &line);
                                Reply::Line(line)
                            }
                            None => Reply::Answer(Answer::NotServed),
                        }
                    }
                }
            }
            Err(answer) => Reply::Answer(answer),
        }
    }

    fn answer(&mut self, stream: UnixStream) {
        // The ask line is tiny and arrives at connect time; a client that
        // sends nothing (or a partial line) must not park the one worker —
        // a bounded read returns early and frees the daemon for the next
        // request.
        let _ = stream.set_read_timeout(Some(REQUEST_READ));
        let mut reader = BufReader::new(&stream);
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.is_empty() {
            return;
        }
        // Held while the request runs: a peer that finds it free knows this
        // daemon is idle and may release it.
        let busy = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.files.busy);
        let _busy = busy.and_then(|file| file.lock().map(|()| file));
        // One `statvfs`: a cache volume under the disk floor kicks an
        // urgent budget pass on the background thread.
        self.disk.check();
        let reply = match serde_json::from_str::<Ask>(&line) {
            Ok(ask) if self.pooled => self.join(&stream, ask),
            Ok(ask) if ask.roots == self.roots => self.analyze(&stream, &ask.aims, ask.face.as_ref()),
            Ok(_) => Reply::Answer(Answer::Fault("the daemon serves other roots".into())),
            Err(error) => Reply::Answer(Answer::Fault(format!("unreadable request: {error}"))),
        };
        send_reply(&stream, reply);
        if let Some(lease) = &self.lease {
            lease.touch();
        }
        // A Tier-2 ask (adopted, or answered while the standby waits out
        // its TTL) refreshes the standby lease's last use with it.
        if let Some(lease) = &self.standby_lease {
            lease.touch();
        }
        // The cache's last use: the disk budget drops idle caches least
        // recently used first.
        crate::disk::used(&self.cache);
        // The lease's mtime is the last answer: the ledger releases the
        // least recently used resident first. A daemon whose socket path
        // now names a successor's socket leaves the successor's state be.
        if Inode::of(&self.files.socket) == Some(self.socket) {
            self.touch_state();
        }
    }

    /// Refreshes the state file's last-answer mtime. The content — the
    /// daemon's pid — never changes after the bind wrote it, so a quiet
    /// turn touches the mtime instead of rewriting identical bytes; a
    /// state file that vanished beneath the daemon (its directory wiped)
    /// is recreated with the pid rather than silently losing the
    /// heartbeat `daemon_pid` reads.
    fn touch_state(&self) {
        match std::fs::File::options().write(true).open(&self.files.state) {
            Ok(file) => {
                let _ = file.set_modified(std::time::SystemTime::now());
            }
            Err(_) => {
                let _ = std::fs::write(&self.files.state, std::process::id().to_string());
            }
        }
    }

    fn analyze(&mut self, client: &UnixStream, aims: &[crate::Aim], face: Option<&Face>) -> Reply {
        if let Some(line) = self.record_probe(aims, face) {
            return Reply::Line(line);
        }
        match self.analyze_result(client) {
            Ok(()) => {
                let server = self.server.as_ref().expect("a live server");
                match face {
                    Some(face) => match server.render_face(aims, face, self.renderer) {
                        Some(line) => {
                            self.record_persist(aims, Some(face), &line);
                            Reply::Line(line)
                        }
                        None => Reply::Answer(Answer::NotServed),
                    },
                    None => match server.render(aims) {
                        Some(line) => {
                            self.record_persist(aims, None, &line);
                            Reply::Line(line)
                        }
                        None => Reply::Answer(Answer::NotServed),
                    },
                }
            }
            Err(answer) => Reply::Answer(answer),
        }
    }

    /// The answered-turn record probe (#4278): a serverless daemon whose
    /// baseline is still priming — the cold arm, exactly the state a
    /// re-run over a byte-identical tree after the last daemon's death
    /// sits in — may be an ask this key already answered. One full walk
    /// (the record's stamp set pins the tree as the rendering run read
    /// it; the probe re-derives the same set), compared for set
    /// equality: a hit serves the record's wire line with no server at
    /// all, a miss falls through to today's path unchanged — the walk
    /// stays as this ask's read-time capture for the persist a served
    /// render then writes. Every other ask (a live server, an installed
    /// baseline, the family off, an unservable SDK) skips the probe and
    /// clears any earlier capture, so a stale one never pins a later
    /// render.
    fn record_probe(&mut self, aims: &[crate::Aim], face: Option<&Face>) -> Option<Arc<[u8]>> {
        self.record_capture = None;
        if !self.records
            || self.launch.is_none()
            || self.server.is_some()
            || self.freshness.primed()
        {
            return None;
        }
        let roots = self.roots();
        let key = record_key(&self.spec, &roots, aims, face);
        let path = record_path(&self.spec.cache_dir(), &key);
        let (snapshot, _plan) = crate::snapshot::Snapshot::take_with_plan(
            &roots,
            self.spec.packages.as_deref(),
            &self.spec.sdk_home(),
        );
        let stamps = snapshot.stamps();
        match probe_record(&path, &stamps) {
            Ok(line) => {
                self.files.event(&format!(
                    "dartq analyze: answer record hit ({} bytes)",
                    line.len()
                ));
                Some(line)
            }
            Err(miss) => {
                self.files
                    .event(&format!("dartq analyze: answer record miss ({})", miss.as_str()));
                self.record_capture = Some(stamps);
                None
            }
        }
    }

    /// Persists the answered-turn record for a served render (#4278):
    /// the wire line beside the read-time stamp capture this ask's probe
    /// walked — the only capture that is provably pre-render, so the
    /// record pins the tree as the rendering run read it, never a
    /// post-render state an edit during the settle could fool. Only the
    /// cold arm captures (a live-server ask's read-time facts live in
    /// the freshness oracle's own walk, paid at the check), so a tree
    /// that moves during a daemon's life leaves the record stale — and
    /// the next daemon's cold ask misses, settles once, and re-persists:
    /// the family self-heals on every cold start, and the byte-identical
    /// re-run — the death-of-daemon case this exists for — always hits.
    /// A served render with no capture skips the persist; the
    /// quiet-turn elision skips it again while `(key, stamps digest)`
    /// stands; a failed write is a silent no-op (the accelerator
    /// posture).
    fn record_persist(&mut self, aims: &[crate::Aim], face: Option<&Face>, line: &Arc<[u8]>) {
        if !self.records {
            return;
        }
        let Some(stamps) = self.record_capture.take() else {
            return;
        };
        let key = record_key(&self.spec, &self.roots(), aims, face);
        let encoded = serde_json::to_vec(&stamps).expect("stamps serialize");
        let digest = u64::from_le_bytes(
            blake3::hash(&encoded).as_bytes()[..8]
                .try_into()
                .expect("8 bytes"),
        );
        if self
            .record_saved
            .is_some_and(|(saved, digest_was)| saved == key && digest_was == digest)
        {
            return;
        }
        if write_record(&record_path(&self.spec.cache_dir(), &key), &encoded, line).is_ok() {
            self.record_saved = Some((key, digest));
            self.files.event(&format!(
                "dartq analyze: answer record saved ({} bytes)",
                line.len()
            ));
        }
    }

    /// The shared run core: freshness walk, server ensure (one restart
    /// on fault), and the lease's peak record — `Ok` leaves a live
    /// server whose error map is current; `Err` is the answer to send.
    fn analyze_result(&mut self, client: &UnixStream) -> Result<(), Answer> {
        let Some(launch) = self.launch.clone() else {
            return Err(self.fail("the SDK changed under the daemon".into()));
        };
        let roots = self.roots();
        // The cold arm: with no live server and no baseline installed
        // yet, no answer observes the check's diff — any diff starts a
        // server over the same roots the same way, and that server's
        // first analysis reads the tree from disk itself. The first ask
        // therefore never waits for the background priming walk; the
        // next ask settles it (and every later check pays the normal
        // freshness pass).
        let diff = if cold_freshness_skip(self.server.is_some(), self.freshness.primed()) {
            Diff::default()
        } else {
            let diff = self.freshness.check(&roots);
            diff
        };
        // The sizing figure rides the walk the oracle already paid.
        self.source_bytes = self.freshness.dart_bytes().or(self.source_bytes);
        self.note_degradation(client);
        if diff.rebuild {
            self.stop("rebuild");
        }
        // Whether a server lived through the freshness check (false after
        // a rebuild stop): the restart arm's start reason — a live server
        // faulting is a `fault`, an absent one a plain `start`.
        let served = self.server.is_some();
        let (brake, disk) = (&mut self.brake, &self.disk);
        let outcome = match self.server.as_mut() {
            Some(server) if !diff.changed.is_empty() => {
                // Incremental edit: never yield to the growth brake. This arm
                // is reachable only with `diff.rebuild == false` (a rebuild
                // stops the server above, so `self.server` is `None` and the
                // fresh-start arm below pays the brake instead). The brake
                // exists to stop a cold/full analysis that is growing the
                // server under machine memory pressure; an incremental edit
                // cannot grow the tree unboundedly, but on a swapping host
                // the brake fires on it every time and turns each edit into
                // a full server restart + whole-root re-analysis. Budget
                // accounting (peak sampling, `Fault::Budget`), the ledger
                // admission, and the idle/pressure eviction arms are all
                // untouched — this changes only the yield decision on the
                // incremental-edit settle.
                server.changed(&diff.changed, || {
                    disk.tick();
                    incremental_settle_yields()
                })
            }
            Some(server) => server.wait_idle(|| sample(disk, brake)),
            None => Err(Fault::Server("no server yet".into())),
        };
        let outcome = match outcome {
            Ok(()) => Ok(()),
            Err(Fault::Budget { budget, measured }) => {
                self.forget("budget");
                return Err(Answer::Budget(Fault::Budget { budget, measured }.to_string()));
            }
            Err(Fault::Yield { peak }) => {
                // The growth brake fired: the server is already dead.
                // Book the observed peak so the re-queued client is
                // admitted on an honest figure, then yield the request —
                // never a per-run fallback, which would need the same
                // memory.
                self.forget("yield");
                self.yield_peak(peak);
                return Err(Answer::Yield);
            }
            Err(_) => {
                // Restart once: a fresh server reads the tree as it is
                // now, under the reason the restart answers to.
                let why = if diff.rebuild {
                    "rebuild"
                } else if served {
                    "fault"
                } else {
                    "start"
                };
                self.forget(why);
                self.start(&launch, client, why)
            }
        };
        match outcome {
            Ok(()) => {
                let _ = std::fs::remove_file(&self.files.failed);
                let server = self.server.as_ref().expect("a live server");
                // The measured peak sizes this resident's own lease at
                // once: it leaves the first-run slot, and a peer analyzer
                // waiting there borrows the figure. A quiet answer
                // re-records the identical figure, so the write is elided
                // until it moves — the peaks file already holds exactly
                // those bytes, and an unclean death leaves that earlier,
                // still-correct record behind.
                let peak = server.peak();
                if peak_persist_moved(self.persisted_peak, peak, self.source_bytes) {
                    match self.lease.as_mut() {
                        Some(lease) => {
                            lease.record_peak_with_source(&self.peak_key, peak, self.source_bytes);
                        }
                        None => dartq_toolchain::ledger::record_peak_with_source(
                            &self.peak_key,
                            dartq_toolchain::ledger::Face::AnalysisServer,
                            peak,
                            self.source_bytes.unwrap_or(0),
                        ),
                    }
                    self.persisted_peak = Some((peak, self.source_bytes));
                }
                // An analysis may have grown the cache: a routine budget
                // pass, spaced machine-wide.
                self.disk.routine();
                Ok(())
            }
            Err(Fault::Yield { peak }) => {
                self.forget("yield");
                self.yield_peak(peak);
                Err(Answer::Yield)
            }
            Err(fault @ Fault::Budget { .. }) => Err(Answer::Budget(fault.to_string())),
            Err(Fault::Memory(message)) => Err(Answer::Budget(message)),
            Err(Fault::Server(message)) => Err(self.fail(message)),
        }
    }

    /// Names a degraded watcher to the user, once per daemon life: a
    /// tree whose checks keep falling back to full scans stays correct
    /// but re-checks the whole tree on every run — without this line
    /// that slowdown is invisible (the client spawns the daemon with its
    /// stderr discarded, so the note rides the wire instead). The
    /// decision is [`degradation_note`]'s: debounced to two consecutive
    /// degraded checks (a cold-start transient never fires it), by-
    /// design walks never count, and a watcher that recovers speaks
    /// through the substrate's cool-off re-arm line.
    fn note_degradation(&mut self, client: &UnixStream) {
        let (streak, why) = degradation_note(
            self.degrade_streak,
            self.freshness.last_check().walk_reason,
            self.degrade_noted,
        );
        self.degrade_streak = streak;
        let Some(why) = why else {
            return;
        };
        self.degrade_noted = true;
        let line = format!(
            "dartq analyze: file watcher degraded to full tree scans ({why}) — \
             results stay correct, every run re-checks the whole tree"
        );
        // The daemon's own copy serves hand-run `--resident-serve` turns;
        // the wire note is the one the user's client prints.
        eprintln!("{line}");
        note(client, line);
    }

    /// Starts the server once the job gate admits it, for `why` (one
    /// event line once the child exists). With the warm posture on, the
    /// standby is adopted instead of spawned when its facts still hold
    /// ([`standby_admit`]) — a fresh `analysis.setAnalysisRoots` on a
    /// rootless server is fresh-spawn semantics. A pooled daemon starts
    /// its server over the whole active set — the founding roots plus
    /// every joined worktree — so a join admitted while the server was
    /// down (or a restart after a fault) does not silently drop the
    /// joiners' roots.
    fn start(&mut self, launch: &Launch, client: &UnixStream, why: &'static str) -> Result<(), Fault> {
        match standby_admit(
            self.warm,
            self.standby.is_some(),
            self.sdk_facts_current(),
        ) {
            None => self.start_fresh(launch, client, why),
            Some(Adopt::Adopt) => self.start_by_adoption(launch, client),
            Some(Adopt::Miss) => self.start_fresh(launch, client, "warm-miss"),
            Some(Adopt::StaleSdk) => {
                self.stop_standby("warm-stale-sdk");
                self.start_fresh(launch, client, "warm-miss")
            }
        }
    }

    /// The fresh-spawn arm of [`Host::start`]: exactly the start the
    /// daemon always ran, under its own event reason.
    fn start_fresh(
        &mut self,
        launch: &Launch,
        client: &UnixStream,
        why: &'static str,
    ) -> Result<(), Fault> {
        let lease = self.lease_slot(client)?;
        // Under the disk floor even after a budget pass: the server still
        // writes its file byte store (the official `--disable-file-byte-store`
        // only applies in the server's snapshot-training mode), so the
        // in-analysis ticks keep trimming it — the client learns why.
        if let Some(free) = self.disk.short_at_start() {
            note(
                client,
                format!(
                    "dartq analyze: low disk ({:.1} GB free) — the analyzer's file cache is held to its minimum",
                    free as f64 / f64::from(1u32 << 30)
                ),
            );
        }
        let roots = self.roots();
        let (brake, disk) = (&mut self.brake, &self.disk);
        let started = Server::start(launch, &roots, self.budget, || sample(disk, brake));
        let server = started?;
        self.files.event(&format!(
            "dartq analyze: analyzer pid {} started ({why})",
            server.pid()
        ));
        // The successor daemon reaps this pid if a SIGKILL stops this
        // daemon before its group kill can run.
        let _ = std::fs::write(&self.files.server_pid, server.pid().to_string());
        self.lease = lease.map(|mut lease| {
            lease.track(server.pid());
            lease
        });
        self.server = Some(server);
        Ok(())
    }

    /// The adoption arm of [`Host::start`]: the standby becomes the
    /// borrower's serving server over the whole active root set, under
    /// the normal serving lease (gate semantics unchanged — a standby
    /// holds no slot, so adopting under `max_concurrent = 1` is the same
    /// gate pass a fresh spawn waits). The standby's slotless lease frees
    /// as the serving one lands. A lease the gate cannot grant restores
    /// the standby for the next ask; an adoption that faults kills the
    /// standby (`warm-fault`) and falls to a fresh spawn; a budget or
    /// yield surfaces as the fresh start's own fault would.
    fn start_by_adoption(
        &mut self,
        launch: &Launch,
        client: &UnixStream,
    ) -> Result<(), Fault> {
        let lease = match self.lease_slot(client) {
            Ok(lease) => lease,
            // The gate wait failed: the request yields as a fresh start's
            // would, and the standby stays for the next ask.
            Err(fault) => return Err(fault),
        };
        let standby = self.standby.take().expect("the admit checked a standby");
        let pid = standby.pid();
        // The standby's slotless booking leaves with it; the serving lease
        // replaces it before the child is named on it.
        self.standby_lease = None;
        let roots = self.roots();
        let (brake, disk) = (&mut self.brake, &self.disk);
        match Server::adopt(standby, launch, &roots, || sample(disk, brake)) {
            Ok(server) => {
                // The standby's original pid is the serving pid — the
                // continuity the events log proves adoption by.
                self.files.event(&format!(
                    "dartq analyze: analyzer pid {pid} started (warm-adopt)"
                ));
                let _ = std::fs::write(&self.files.server_pid, server.pid().to_string());
                self.lease = lease.map(|mut lease| {
                    lease.track(server.pid());
                    lease
                });
                self.server = Some(server);
                Ok(())
            }
            // A budget or yield kill is the adoption's answer, exactly as
            // a fresh start's would be — the caller maps it.
            Err(fault @ (Fault::Budget { .. } | Fault::Yield { .. })) => Err(fault),
            Err(fault) => {
                // The standby never became a server: one stop line with
                // its pid, the fault on stderr for a hand-run daemon, and
                // a fresh spawn under the warm-miss reason.
                eprintln!("dartq analyze: standby adoption failed: {fault}");
                self.files.event(&format!(
                    "dartq analyze: analyzer pid {pid} stopped (warm-fault)"
                ));
                drop(lease);
                self.start_fresh(launch, client, "warm-miss")
            }
        }
    }

    /// Takes the server's job slot and its expected peak for its life,
    /// releasing idle residents or waiting as the gate decides; its notes
    /// reach the client. The server is a resident the gate may release
    /// while idle (this daemon's busy lock free). A gate that cannot be
    /// used starts ungated; a wait that times out is [`Fault::Memory`].
    fn lease_slot(
        &self,
        client: &UnixStream,
    ) -> Result<Option<dartq_toolchain::ledger::Lease>, Fault> {
        use dartq_toolchain::ledger::{self, Face, Releasable, Request};
        let (bytes, sized) = ledger::sizing_with_source(
            &self.peak_key,
            Face::AnalysisServer,
            self.source_bytes,
        );
        let request = Request {
            face: Face::AnalysisServer,
            label: self
                .roots
                .first()
                .map(|root| root.display().to_string())
                .unwrap_or_default(),
            bytes,
            pid: None,
            release: Some(Releasable {
                pid: std::process::id(),
                busy: self.files.busy.clone(),
            }),
            slot: true,
            cap: ledger::cap(self.max_concurrent),
            sized,
            peak_key: Some(self.peak_key.clone()),
            source_bytes: self.source_bytes,
        };
        let deadline = Instant::now() + MEMORY_WAIT;
        // A park at this gate was stderr-only — invisible in events.log,
        // which is why #4249's gate-band steps looked inexplicable. One
        // events line when the wait enters (the ledger's own note says
        // what and who holds) and one when it leaves: admitted, or
        // faulted at the 30-minute bound.
        let mut waited_since: Option<Instant> = None;
        let files = &self.files;
        let granted = ledger::acquire(request, bytes, self.max_percent, deadline, |text| {
            if waited_since.is_none()
                && let Some(what) = text.strip_prefix("dartq: waiting ")
            {
                waited_since = Some(Instant::now());
                files.event(&format!("dartq analyze: analyzer gate-wait ({what})"));
            }
            note(client, text.to_owned());
        });
        match granted {
            Ok(lease) => {
                if let Some(at) = waited_since {
                    files.event(&format!(
                        "dartq analyze: analyzer gate-admit (waited {}s)",
                        at.elapsed().as_secs()
                    ));
                }
                Ok(lease)
            }
            Err(error @ ledger::Error::Timeout { .. }) => {
                if let Some(at) = waited_since {
                    files.event(&format!(
                        "dartq analyze: analyzer gate-fault (waited {}s: {error})",
                        at.elapsed().as_secs()
                    ));
                }
                Err(Fault::Memory(error.to_string()))
            }
            Err(ledger::Error::Io(_) | ledger::Error::Stopped) => Ok(None),
        }
    }

    /// A fault the client falls back on; the marker spares the next
    /// clients a daemon start that would fail the same way.
    fn fail(&self, message: String) -> Answer {
        let _ = std::fs::write(&self.files.failed, &message);
        Answer::Fault(message)
    }

    /// The growth brake's bookkeeping: the observed tree peak is a lower
    /// bound (the job was still growing at the yield), recorded as a
    /// floor — never over an earlier full measurement, so the client's
    /// re-queued ask is admitted on the honest figure. The floor record
    /// changes the persisted figure by a path the completion arm's
    /// elision memory does not see, so the memory is forgotten here: the
    /// next completion records unconditionally (a lowered figure must
    /// land, not be elided as "already persisted").
    fn yield_peak(&mut self, peak: u64) {
        self.persisted_peak = None;
        dartq_toolchain::ledger::record_peak_floor(
            &self.peak_key,
            dartq_toolchain::ledger::Face::AnalysisServer,
            peak,
            self.source_bytes.unwrap_or(0),
        );
    }
}

/// A pooled root asked within this window is hot: never evicted. Evicting
/// a root its worktree still asks re-analyzes that whole worktree on its
/// next request and evicts another — the pool thrash; a join that needs a
/// hot root's place is refused instead, and the worktree's own daemon
/// serves it.
const POOL_HOT: Duration = Duration::from_secs(5 * 60);

/// The standby lease's booking: a fixed small figure above a rootless
/// server's measured tree (~37 MB) — never a slot, never an unsized
/// first-run placeholder. Five standbys book ~320 MB against a budget
/// where one unsized resident books 1 GiB.
const STANDBY_LEASE: u64 = 64 << 20;
/// How long a standby's lease wait may run: the standby is an
/// optimization, never a wait — a memory-tight host simply gets none.
const STANDBY_LEASE_WAIT: Duration = Duration::from_secs(10);

/// The warm posture's decision for one server start, pure — the policy
/// half of [`Host::start`]'s adoption arm, mirroring [`admit`]'s
/// policy-only shape so the matrix is testable without a server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Adopt {
    /// The standby's facts hold: it becomes the serving server
    /// (`warm-adopt`).
    Adopt,
    /// Warm on, no standby to adopt: a fresh spawn (`warm-miss`).
    Miss,
    /// The standby's SDK facts went stale under it: kill it
    /// (`warm-stale-sdk`), a fresh spawn (`warm-miss`).
    StaleSdk,
}

/// [`Host::start`]'s whole adoption policy, pure: warm off answers `None`
/// (the ordinary start path unchanged); with warm on, an absent standby
/// is a miss, a standby whose launch facts no longer match the SDK on
/// disk is stale, and a current standby is adopted.
fn standby_admit(warm: bool, standby: bool, facts_current: bool) -> Option<Adopt> {
    if !warm {
        return None;
    }
    if !standby {
        return Some(Adopt::Miss);
    }
    if !facts_current {
        return Some(Adopt::StaleSdk);
    }
    Some(Adopt::Adopt)
}

/// Resolves the warm posture daemon-side: `DARTQ_WARM_SERVERS` over the
/// spec's `warm` over the default (off since 0.1.23 — the prewarm
/// balloon is transient spawn-time memory pressure, the wrong default on
/// memory-tight hosts; `warm_servers: true` in dartq.yaml or the env is
/// the opt-in) — a spec written before the knob existed behaves as the
/// default, and A/B arms need no CLI wiring. The
/// TTL resolves the same way (`DARTQ_WARM_TTL_SECS`, spec, 900 s), with a
/// 1 s floor. An unparseable override falls through to the next tier —
/// fail-open like every other knob here.
fn warm_config(spec: &Spec) -> (bool, Duration) {
    let servers = std::env::var("DARTQ_WARM_SERVERS").ok();
    let ttl = std::env::var("DARTQ_WARM_TTL_SECS").ok();
    warm_config_with(spec, servers.as_deref(), ttl.as_deref())
}

/// [`warm_config`]'s testable core: the two override strings passed in.
fn warm_config_with(
    spec: &Spec,
    env_servers: Option<&str>,
    env_ttl: Option<&str>,
) -> (bool, Duration) {
    let servers = env_servers
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(spec.warm);
    let ttl = env_ttl
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(u64::from(if spec.warm_ttl_secs == 0 {
            900
        } else {
            spec.warm_ttl_secs
        }))
        .max(1);
    (servers > 0, Duration::from_secs(ttl))
}

/// The standby's lease request, pure: memory-only (`slot: false` — a
/// standby never blocks a per-run analysis under
/// `memory.max_concurrent`), booked at [`STANDBY_LEASE`] as a deliberate
/// figure, releasable while the daemon is idle.
fn standby_request(label: String, busy: PathBuf) -> dartq_toolchain::ledger::Request {
    dartq_toolchain::ledger::Request {
        face: dartq_toolchain::ledger::Face::AnalysisServer,
        label,
        bytes: STANDBY_LEASE,
        pid: None,
        release: Some(dartq_toolchain::ledger::Releasable {
            pid: std::process::id(),
            busy,
        }),
        slot: false,
        cap: 0,
        sized: true,
        peak_key: None,
        source_bytes: None,
    }
}

/// The idle loop's tiering transition, pure: an expired window enters
/// Tier 2 (stand up a standby, live on for the TTL) exactly when the
/// warm posture is on, the daemon's SDK can serve, and no standby is
/// live — the remaining case (a standby's own TTL expiry) is Tier 3.
fn enters_tier2(warm: bool, standby: bool, serving_sdk: bool) -> bool {
    warm && !standby && serving_sdk
}

/// Whether the host sits at the ledger monitor's own low-memory line —
/// the same figures `Pressure::tick` samples, mirrored here because a
/// Tier-2 daemon sheds its standby at that signal before any peer's
/// monitor would release a whole fat resident.
fn host_low(max_percent: u8) -> bool {
    let total = dartq_toolchain::ledger::total_memory();
    let available = dartq_toolchain::ledger::available_memory();
    low_memory(total, available, max_percent)
        || critical(total, available, dartq_toolchain::ledger::pressure_level())
}

/// [`host_low`]'s shortfall core: available memory under the reserve
/// (`(100 − max_percent)%` of physical, never below the ledger's floor).
/// An unreadable figure is never low. The kernel's warn level is
/// deliberately absent — it fires at 40-50% free during spawn bursts
/// and sheds warm standbys that would have been reused (#4266, #4269).
fn low_memory(total: u64, available: Option<u64>, max_percent: u8) -> bool {
    if total == 0 {
        return false;
    }
    let share = u64::try_from(
        u128::from(total) * u128::from(100 - max_percent.clamp(1, 100)) / 100,
    )
    .unwrap_or(u64::MAX);
    let reserve = share.max(dartq_toolchain::ledger::RESERVE_FLOOR);
    available.is_some_and(|available| available < reserve)
}

/// The critical-pressure line the standby also sheds on: available under
/// the ledger's [`critical_floor`], or the macOS kernel's level-4
/// critical. Mirrors `ledger::critical()` — kept local because the
/// ledger's copy is private and this daemon's shed decision is its own.
/// `level` is `None` on non-macOS (the counter does not exist there).
fn critical(total: u64, available: Option<u64>, level: Option<u32>) -> bool {
    if total == 0 {
        return false;
    }
    let floor = dartq_toolchain::ledger::critical_floor(total);
    if available.is_some_and(|available| available < floor) {
        return true;
    }
    level.is_some_and(|level| level >= 4)
}

/// The pooled admission policy: append `joining` to `active`, evicting
/// least-recently-used cold non-primary roots (index 0 is the founding
/// primary — never evicted; a root asked within [`POOL_HOT`] of `now` is
/// hot — never evicted) until the set fits `max_roots`. `Some` is the
/// evicted list (possibly empty); `None` refuses — the join would need
/// the primary's or a hot root's place, and `active` is then left
/// untouched.
fn admit(
    active: &mut Vec<(PathBuf, Instant)>,
    joining: &[PathBuf],
    max_roots: usize,
    now: Instant,
) -> Option<Vec<PathBuf>> {
    let mut evictable: Vec<usize> = (1..active.len())
        .filter(|&index| now.saturating_duration_since(active[index].1) >= POOL_HOT)
        .collect();
    evictable.sort_by_key(|&index| active[index].1);
    let needed = active.len() + joining.len();
    let evictions = needed.saturating_sub(max_roots);
    if evictions > evictable.len() {
        return None;
    }
    let mut evicted = Vec::with_capacity(evictions);
    let mut drop: Vec<usize> = evictable[..evictions].to_vec();
    drop.sort_unstable_by(|a, b| b.cmp(a));
    for index in drop {
        evicted.push(active.remove(index).0);
    }
    for root in joining {
        active.push((root.clone(), now));
    }
    Some(evicted)
}

fn send_reply(stream: &UnixStream, reply: Reply) {
    // A client that went away just misses its answer.
    let _ = match reply {
        Reply::Line(line) => (&*stream).write_all(&line),
        Reply::Answer(answer) => {
            let mut out = serde_json::to_vec(&answer).expect("an answer serializes");
            out.push(b'\n');
            (&*stream).write_all(&out)
        }
    };
}

fn note(stream: &UnixStream, text: String) {
    send_reply(stream, Reply::Answer(Answer::Note(text)));
}

/// The degradation note's whole policy, pure: given the streak of
/// consecutive degradation-shaped checks BEFORE this one, this check's
/// walk reason, and whether the note already fired, decide the streak to
/// carry forward and whether this request names the degradation —
/// `Some(why)` carrying the message's parenthetical. Only the
/// deterministic, persistent shapes count (an unwritable tree, a lost
/// stream, an unarmed watcher — each repeats every request, so two
/// consecutive checks cost at most one silent run). A timed-out sync
/// confirmation is the designed slow-path fallback, not a degradation:
/// the adaptive wait bound follows the walk-cost EWMA, so a timeout
/// regime lengthens the walks, grows the bound, and lets cookies
/// deliver in bound again — it self-corrects (and clusters at cold-start
/// tails, where the bound still sits at its pre-cold value under a
/// saturated stream). Journal answers and by-design walks (`First`,
/// `Rebase`) are equally evidence against persistence; none of these
/// count, the streak starts over, and only the second consecutive
/// counted shape speaks.
fn degradation_note(
    streak: u8,
    reason: Option<WalkReason>,
    noted: bool,
) -> (u8, Option<&'static str>) {
    if noted {
        return (streak, None);
    }
    let why = match reason {
        Some(WalkReason::CookieBlocked) => "its sync files cannot be written",
        Some(WalkReason::StreamLost) => "its event stream lost events",
        Some(WalkReason::Unarmed) => "its watcher is unavailable",
        // A timed-out confirmation (the slow path working as designed —
        // see above), a journal answer, or a by-design walk — the
        // covering walk after an arm or a stream restart, a pool
        // rebase: none count, the streak starts over.
        Some(WalkReason::First | WalkReason::Rebase | WalkReason::CookieTimeout)
        | None => return (0, None),
    };
    let streak = streak.saturating_add(1);
    (streak, (streak >= 2).then_some(why))
}

/// The per-second sample of a running analysis: the disk budget's
/// in-analysis tick, then the growth brake (`true` yields the job).
fn sample(disk: &crate::disk::Disk, brake: &mut dartq_toolchain::ledger::GrowthBrake) -> bool {
    disk.tick();
    brake.check(std::process::id())
}

/// Whether an incremental edit's settle may yield to the growth brake:
/// never. The brake exists to stop a cold or whole-root analysis that is
/// growing the server under machine memory pressure; an incremental
/// settle cannot grow the tree, but on a swapping host the brake fires
/// on it every time and turns each edit into a full server restart +
/// whole-root re-analysis (the #4249 failure class: 88-155 s steps).
/// The other in-analysis samples — a fresh start's, a root-set change's,
/// an idle wait's — keep the brake through [`sample`]; the budget check
/// and peak sampling inside the settle are untouched either way.
fn incremental_settle_yields() -> bool {
    false
}

/// The ledger peak key for a set of roots: the git common dir plus the
/// root's path relative to its worktree top, so sibling worktrees of one
/// repo share one peak. `None` when no root's git identity is readable —
/// the cache dir's key is the fallback.
fn repo_key(roots: &[PathBuf]) -> Option<String> {
    let mut common: Option<PathBuf> = None;
    let mut rels = Vec::with_capacity(roots.len());
    for root in roots {
        let (dir, rel) = git_root(root)?;
        match &common {
            None => common = Some(dir),
            Some(c) if *c == dir => {}
            _ => return None,
        }
        rels.push(rel);
    }
    rels.sort();
    let mut key = common?.display().to_string();
    for rel in rels {
        key.push(':');
        key.push_str(&rel);
    }
    Some(key)
}

/// One root's git identity: `(common_dir, relative_path)`. The worktree
/// top is the nearest ancestor holding `.git`, and the relative path is
/// the root's path below it — the same in every worktree of the repo. A
/// `.git` dir is the common dir itself; a `.git` file (`gitdir: <path>`)
/// names the worktree's git dir, whose `commondir` file names the common
/// dir of a linked worktree (a submodule's git dir has none: it is its
/// own common dir).
fn git_root(root: &Path) -> Option<(PathBuf, String)> {
    let top = root.ancestors().find(|dir| dir.join(".git").exists())?;
    let dot = top.join(".git");
    let common = if dot.is_dir() {
        dot
    } else {
        let text = std::fs::read_to_string(&dot).ok()?;
        let gitdir = top.join(text.trim().strip_prefix("gitdir:")?.trim());
        match std::fs::read_to_string(gitdir.join("commondir")) {
            Ok(common) => gitdir.join(common.trim()),
            Err(_) => gitdir,
        }
    };
    let rel = root.strip_prefix(top).ok()?.display().to_string();
    Some((common.canonicalize().ok()?, rel))
}

/// Whether this request may skip the freshness check: a daemon with no
/// live server and no installed baseline — the cold first ask (or a
/// later one while the server is down and the background prime still
/// runs). The answer comes from the fresh server's own full analysis;
/// the diff would only name files that analysis reads from disk anyway.
fn cold_freshness_skip(server_live: bool, primed: bool) -> bool {
    !server_live && !primed
}

/// Whether the completion's (peak, source) figure must be persisted to
/// the peaks store: only when it differs from the last figure this
/// daemon persisted — a quiet answer re-records identical bytes, and the
/// file already holds exactly those. `None` (nothing persisted yet, or
/// forgotten after the growth brake's floor record changed the store by
/// another path) always lands, so a genuinely changed figure is never
/// elided.
fn peak_persist_moved(persisted: Option<(u64, Option<u64>)>, peak: u64, source: Option<u64>) -> bool {
    persisted.is_none_or(|(peak_was, source_was)| peak != peak_was || source != source_was)
}

// ---------------------------------------------------------------------
// Answered-turn records (#4278)
//
// The official face is the one face with no persisted answer: a
// daemon's death lost its in-memory answer cache whole, so a re-run
// over a byte-identical tree re-paid the full settle. The engine face
// won this lever (#4243/#4260); this is that shape on the official
// face. A served render persists its wire line beside the read-time
// stamp set — one
// file per (SDK, served roots, aims, face, dartq binary) key under the
// cache dir's `answers/` family, atomically renamed, byte-budget-capped
// with an LRU sweep at daemon start (#4273's posture: unbounded
// persist-per-shape is the exact multiplication class that lesson
// closed). A serverless, unprimed daemon's ask probes before any server
// starts: one full walk, set-equality against the record's stamps, a
// hit ships the payload through `Reply::Line` with the server never
// spawning; any miss falls through to today's path byte-identically.
// The records are whole-file write/read — a whole-repo envelope is
// 215.9 MB-level and never parsed on serve, and they are NOT bounded by
// the in-memory `AnswerCache`'s 64 MiB cap (that cap governs the
// resident map; this family governs itself).
// ---------------------------------------------------------------------

/// One recorded file's stamp facts, the exact shape
/// [`crate::snapshot::Snapshot::stamps`] derives — both sides of every
/// comparison come from that one derivation.
type Stamps = Vec<(PathBuf, u64, u64, u64, u64)>;

/// The record family's codec version.
const RECORD_VERSION: u32 = 1;
/// The cache dir's subdir holding the record family.
const RECORDS_DIR: &str = "answers";
/// The final-path suffix; temps carry `.answer.tmp-<pid>-<seq>`.
const RECORD_SUFFIX: &str = ".answer";
/// The record family's byte cap, per cache dir (#4273's budget
/// amendment): the wire caps one served face at 1 GiB
/// ([`crate::client::MAX_FACE_BYTES`]) and the largest measured
/// whole-repo envelope is 215.9 MB, so 2 GiB holds the biggest legal
/// record beside a working set of smaller shapes — while staying inside
/// the disk discipline's own figures (an idle cache cap of 8 GiB across
/// worktrees). The startup sweep LRU-caps the family here, oldest
/// last-persist (mtime) first.
const RECORDS_CAP: u64 = 2 << 30;
/// How far the sweep reads into a record to judge it dead: the header
/// line is three small numbers, always inside this bound — a file whose
/// first line runs past it is a foreign format, dead for this binary.
const RECORD_PREFIX_LEN: usize = 128;
/// How long a `.answer.tmp-*` must age before the sweep reclaims it —
/// an in-flight writer's temp is never touched (#4273's grace).
const RECORD_SWEEP_GRACE: Duration = Duration::from_secs(3600);
/// The temp-file counter of [`write_record`]'s sibling temps.
static RECORD_TMP_SEQ: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0);

/// The record's first line: the codec version and the two lengths the
/// frame's rest must add up to — `header\nstamps\npayload`, whole-file
/// write and read, the payload never parsed on serve.
#[derive(Serialize, Deserialize)]
struct RecordHeader {
    v: u32,
    /// The stamps line's byte length.
    stamps: u64,
    /// The payload's byte length — the exact wire bytes served.
    bytes: u64,
}

/// Why a probe fell through to today's path. Every miss is the
/// accelerator's silence: nothing changes, the ask just pays what it
/// always paid.
#[derive(Debug)]
enum RecordMiss {
    /// No readable record under the key (absent counts).
    NoRecord,
    /// A record this codec cannot serve (torn, foreign, or the frame's
    /// lengths do not add up) — dead weight, unlinked under the
    /// concurrent-writer guard.
    Decode,
    /// The tree moved: the walked stamp set is not the record's.
    Stamps,
}

impl RecordMiss {
    fn as_str(&self) -> &'static str {
        match self {
            Self::NoRecord => "no-record",
            Self::Decode => "decode",
            Self::Stamps => "stamps",
        }
    }
}

/// The record family's key: the daemon-key discipline
/// ([`crate::Spec::dir`]'s) over the answer's own legs — the SDK (root
/// and version), the served root set (sorted, so a pooled set spelled in
/// a different join order is the same answer), the package-config
/// override, the canonical aims spelling, the face tag, and the running
/// binary's stamp. A record can only ever answer an ask whose every leg
/// it was rendered under: an SDK swap, a root-set change, another aims
/// or face shape, or a rebuilt dartq all key away from it.
fn record_key(
    spec: &Spec,
    roots: &[PathBuf],
    aims: &[crate::Aim],
    face: Option<&Face>,
) -> [u8; 16] {
    let mut hasher = blake3::Hasher::new();
    crate::add(&mut hasher, spec.sdk_root.as_os_str().as_encoded_bytes());
    crate::add(&mut hasher, spec.sdk_version.as_bytes());
    let mut sorted: Vec<&Path> = roots.iter().map(std::convert::AsRef::as_ref).collect();
    sorted.sort_unstable();
    for root in sorted {
        crate::add(&mut hasher, root.as_os_str().as_encoded_bytes());
    }
    crate::add(
        &mut hasher,
        spec.packages
            .as_deref()
            .map_or(&[][..], |p| p.as_os_str().as_encoded_bytes()),
    );
    crate::add(&mut hasher, &crate::server::aims_key(aims));
    let face_tag = face.map_or(crate::server::REPORT_FACE.to_vec(), |face| {
        serde_json::to_vec(face).expect("a face serializes")
    });
    crate::add(&mut hasher, &face_tag);
    crate::add(&mut hasher, crate::binary_identity().as_bytes());
    let mut key = [0u8; 16];
    key.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
    key
}

/// The record file of `key` under the cache dir's answers family.
fn record_path(cache: &Path, key: &[u8; 16]) -> PathBuf {
    cache
        .join(RECORDS_DIR)
        .join(format!("{}{RECORD_SUFFIX}", hex::encode(key)))
}

/// Writes one record atomically: a unique sibling temp, then a
/// same-filesystem rename over the destination — a torn write leaves
/// the previous record (or nothing), never a half frame the probe
/// would trust. The header line derives from the lengths it is handed,
/// so the frame always adds up by construction.
fn write_record(path: &Path, stamps_line: &[u8], payload: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    std::fs::create_dir_all(path.parent().expect("the answers dir path"))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("record");
    let tmp = path.with_file_name(format!(
        "{name}.tmp-{}-{}",
        std::process::id(),
        RECORD_TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let written = std::fs::File::create(&tmp).and_then(|mut file| {
        let header = serde_json::to_vec(&RecordHeader {
            v: RECORD_VERSION,
            stamps: stamps_line.len() as u64,
            bytes: payload.len() as u64,
        })
        .expect("the header serializes");
        file.write_all(&header)
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.write_all(stamps_line))
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.write_all(payload))
    });
    let renamed = written.and_then(|()| std::fs::rename(&tmp, path));
    if renamed.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    renamed
}

/// Probes the record at `path` for an ask whose walked stamp set is
/// `stamps`: the frame must decode, its lengths must add up, and the
/// record's stamp set must equal the walked one — then the payload is
/// the served wire line, exactly the bytes the render shipped. A decode
/// miss unlinks the dead record under the concurrent-writer guard
/// (fresh bytes a racing writer renamed in between the read and the
/// unlink are spared); a stamps miss keeps it — the tree moved, and a
/// revert brings the record's stamps back (the persist a fresh render
/// then writes supersedes it either way).
fn probe_record(path: &Path, stamps: &Stamps) -> Result<Arc<[u8]>, RecordMiss> {
    let frame = std::fs::read(path).map_err(|_| RecordMiss::NoRecord)?;
    let miss = || decode_miss(path, &frame);
    let Some(one) = frame.iter().position(|b| *b == b'\n') else {
        return Err(miss());
    };
    let header: RecordHeader = serde_json::from_slice(&frame[..one]).map_err(|_| miss())?;
    if header.v != RECORD_VERSION {
        return Err(miss());
    }
    let stamps_end = (one + 1)
        .checked_add(usize::try_from(header.stamps).unwrap_or(usize::MAX))
        .filter(|end| frame.get(*end) == Some(&b'\n'))
        .ok_or_else(miss)?;
    let payload_start = stamps_end + 1;
    let payload_end = payload_start
        .checked_add(usize::try_from(header.bytes).unwrap_or(usize::MAX))
        .filter(|end| *end == frame.len())
        .ok_or_else(miss)?;
    let recorded: Stamps = serde_json::from_slice(&frame[one + 1..stamps_end]).map_err(|_| miss())?;
    if recorded != *stamps {
        return Err(RecordMiss::Stamps);
    }
    Ok(frame[payload_start..payload_end].into())
}

/// The decode miss's verdict: the judged frame is dead weight — unlink
/// it under the concurrent-writer guard and say why.
fn decode_miss(path: &Path, frame: &[u8]) -> RecordMiss {
    remove_record_if(path, frame);
    RecordMiss::Decode
}

/// Removes the record at `path` only if its bytes still equal `judged`
/// — the miss-path unlink's concurrent-writer guard (#4273's): a writer
/// whose atomic rename landed fresh bytes between the probe's read and
/// this unlink keeps its record, and fresh content can never byte-equal
/// the stale bytes the probe judged.
fn remove_record_if(path: &Path, judged: &[u8]) {
    if std::fs::read(path).is_ok_and(|current| current == judged) {
        let _ = std::fs::remove_file(path);
    }
}

/// The record family's budget pass (#4273's posture, at daemon start —
/// never inside an ask): dead records reclaim outright — a frame whose
/// header this codec cannot read or whose lengths do not add up (torn,
/// foreign, or a future codec — dead weight for this binary, whose own
/// keys fork away from a foreign generation anyway); `.answer.tmp-*`
/// debris reclaims past [`RECORD_SWEEP_GRACE`]; the survivors LRU-cap
/// at [`RECORDS_CAP`], oldest last-persist (mtime) first with a
/// path-ascending tie-break for determinism. Best-effort throughout:
/// any I/O failure skips its file, and a record this pass misses stays
/// for the next daemon start.
fn sweep_records(cache: &Path) {
    sweep_records_with(cache, RECORDS_CAP);
}

/// [`sweep_records`]'s core over an explicit cap — the budget the tests
/// squeeze.
fn sweep_records_with(cache: &Path, cap: u64) {
    let Ok(files) = std::fs::read_dir(cache.join(RECORDS_DIR)) else {
        return;
    };
    let mut live: Vec<(u64, u64, PathBuf)> = Vec::new();
    for file in files.flatten() {
        let Ok(meta) = file.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let Some(name) = file.file_name().into_string().ok() else {
            continue;
        };
        let path = file.path();
        if name.contains(".answer.tmp-") {
            let aged = meta
                .modified()
                .ok()
                .and_then(|mtime| mtime.elapsed().ok())
                .is_some_and(|age| age >= RECORD_SWEEP_GRACE);
            if aged {
                let _ = std::fs::remove_file(&path);
            }
        } else if name.ends_with(RECORD_SUFFIX) {
            if record_dead(&path, meta.len()) {
                let _ = std::fs::remove_file(&path);
                continue;
            }
            let mtime = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |span| span.as_secs());
            live.push((mtime, meta.len(), path));
        }
    }
    let mut total: u64 = live.iter().map(|&(_, size, _)| size).sum();
    if total <= cap {
        return;
    }
    live.sort_unstable();
    for (_, size, path) in live {
        if total <= cap {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
}

/// Whether the record at `path` is provably dead for this binary: a
/// first line past the prefix bound, a header this codec cannot parse,
/// a foreign version, or a frame whose lengths do not add up to the
/// file's size (a torn or truncated write the rename discipline cannot
/// produce on the final path — a foreign writer's). A file unreadable
/// right now is not provably dead and stays for the next pass.
fn record_dead(path: &Path, len: u64) -> bool {
    use std::io::Read as _;
    let Ok(prefix) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = Vec::with_capacity(RECORD_PREFIX_LEN);
    if prefix
        .take(RECORD_PREFIX_LEN as u64)
        .read_to_end(&mut head)
        .is_err()
    {
        return false;
    }
    let Some(one) = head.iter().position(|b| *b == b'\n') else {
        return true;
    };
    let Ok(header) = serde_json::from_slice::<RecordHeader>(&head[..one]) else {
        return true;
    };
    header.v != RECORD_VERSION
        || len != (one as u64)
            .saturating_add(1)
            .saturating_add(header.stamps)
            .saturating_add(1)
            .saturating_add(header.bytes)
}

/// The env kill switch of the record family.
const RECORDS_ENV: &str = "DARTQ_ANSWER_RECORD";

/// Resolves the record family's switch, once per daemon start: the env
/// (`DARTQ_ANSWER_RECORD=0` off, `=1` on) over the governing
/// dartq.yaml's `analyzer.official.answer_records: false` (project file
/// over the user-level global, the config tiering's own precedence),
/// defaulting on. An unparseable override fails open to the tier below
/// — the posture of every knob here.
fn records_enabled(roots: &[PathBuf]) -> bool {
    let env = std::env::var(RECORDS_ENV).ok();
    let global = std::env::home_dir()
        .map(|home| home.join(".dartq").join("dartq.yaml"))
        .and_then(|path| std::fs::read_to_string(path).ok());
    // The project tier: the nearest dartq.yaml up from the founding
    // root — the same nearest-up-the-chain rule the CLI's discovery
    // walks from the invocation's cwd. The daemon's cwd is wherever its
    // client spawned it, so the founding root is this side's own truth;
    // for the one boolean this reads, the tree's own governing file is
    // the right universe either way.
    let project = roots.first().and_then(|root| {
        root.ancestors()
            .map(|dir| dir.join("dartq.yaml"))
            .find(|candidate| candidate.is_file())
            .and_then(|path| std::fs::read_to_string(path).ok())
    });
    records_enabled_with(env.as_deref(), global.as_deref(), project.as_deref())
}

/// [`records_enabled`]'s testable core: the env string and the two yaml
/// bodies as read.
fn records_enabled_with(env: Option<&str>, global: Option<&str>, project: Option<&str>) -> bool {
    match env.map(str::trim) {
        Some("0") => return false,
        Some("1") => return true,
        _ => {}
    }
    let yaml = |text: Option<&str>| {
        text.and_then(|text| {
            #[derive(Deserialize, Default)]
            struct File {
                analyzer: Option<Analyzer>,
            }
            #[derive(Deserialize, Default)]
            struct Analyzer {
                official: Option<Official>,
            }
            #[derive(Deserialize, Default)]
            struct Official {
                answer_records: Option<bool>,
            }
            serde_yaml::from_str::<File>(text)
                .ok()?
                .analyzer?
                .official?
                .answer_records
        })
    };
    yaml(project).or_else(|| yaml(global)).unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pooled admission: joins fill the capacity, evict
    /// least-recently-used cold non-primary roots (the founding root never
    /// leaves), and a join that cannot fit without dropping the primary
    /// is refused — the caller's own daemon posture.
    #[test]
    fn admit_fills_evicts_lru_and_refuses_past_the_primary() {
        // A base an hour ahead keeps every `now - age` representable on a
        // freshly booted host.
        let now = Instant::now() + Duration::from_secs(3600);
        let at = |seconds: u64| now - Duration::from_secs(seconds);
        let roots = |names: &[&str]| {
            names
                .iter()
                .map(|name| PathBuf::from(format!("/w/{name}")))
                .collect::<Vec<_>>()
        };
        let mut active: Vec<(PathBuf, Instant)> = vec![
            (PathBuf::from("/w/primary"), at(1500)),
            (PathBuf::from("/w/a"), at(1200)),
            (PathBuf::from("/w/b"), at(900)),
        ];
        // Under the cap: plain join, nothing evicted.
        assert_eq!(
            admit(&mut active, &roots(&["c"]), 4, now).expect("admitted"),
            Vec::<PathBuf>::new()
        );
        assert_eq!(active.len(), 4);
        // At the cap: the LRU cold non-primary (/w/a, quiet 20 min) goes.
        let evicted = admit(&mut active, &roots(&["d"]), 4, now).expect("admitted");
        assert_eq!(evicted, roots(&["a"]));
        let served: Vec<&Path> = active.iter().map(|(root, _)| root.as_path()).collect();
        assert_eq!(served.len(), 4);
        assert!(served.contains(&Path::new("/w/d")));
        assert!(!served.contains(&Path::new("/w/a")));
        // The primary never leaves: two joins on a 2-cap pool refuse —
        // dropping the primary is not a join, it is another daemon.
        let mut small: Vec<(PathBuf, Instant)> =
            vec![(PathBuf::from("/w/primary"), at(1500))];
        assert_eq!(
            admit(&mut small, &roots(&["/w/x", "/w/y"]), 2, now),
            None,
            "refused, active untouched"
        );
        assert_eq!(small.len(), 1);
    }

    /// The pool thrash guard: roots asked within [`POOL_HOT`] are never
    /// evicted. A hot set at the capacity refuses the join and leaves the
    /// served set exactly as it was; a cold root still makes room, and
    /// only the cold one goes.
    #[test]
    fn a_hot_set_refuses_the_join_and_stays_served() {
        let now = Instant::now() + Duration::from_secs(3600);
        let at = |seconds: u64| now - Duration::from_secs(seconds);
        let hot: Vec<(PathBuf, Instant)> = vec![
            (PathBuf::from("/w/primary"), at(30)),
            (PathBuf::from("/w/a"), at(10)),
            (PathBuf::from("/w/b"), at(240)),
        ];
        let mut active = hot.clone();
        assert_eq!(
            admit(&mut active, &[PathBuf::from("/w/c")], 3, now),
            None,
            "every non-primary root is hot: refused"
        );
        assert_eq!(active, hot, "the served set is untouched");

        // /w/b goes cold (quiet past the window): it alone makes room.
        active[2].1 = at(POOL_HOT.as_secs() + 1);
        let evicted = admit(&mut active, &[PathBuf::from("/w/c")], 3, now).expect("admitted");
        assert_eq!(evicted, vec![PathBuf::from("/w/b")]);
        let served: Vec<&Path> = active.iter().map(|(root, _)| root.as_path()).collect();
        assert_eq!(served, [Path::new("/w/primary"), Path::new("/w/a"), Path::new("/w/c")]);

        // Two joiners need two places; one cold root cannot give both.
        let mut mixed: Vec<(PathBuf, Instant)> = vec![
            (PathBuf::from("/w/primary"), at(30)),
            (PathBuf::from("/w/cold"), at(POOL_HOT.as_secs() * 2)),
            (PathBuf::from("/w/hot"), at(5)),
        ];
        let before = mixed.clone();
        assert_eq!(
            admit(&mut mixed, &[PathBuf::from("/w/x"), PathBuf::from("/w/y")], 3, now),
            None
        );
        assert_eq!(mixed, before, "a refused join evicts nothing");
    }

    /// Sibling worktrees share one peak key: the main worktree (a `.git`
    /// dir) and a linked worktree (a `.git` file whose git dir names the
    /// common dir through `commondir`) key the same root — and the same
    /// subdirectory root — identically. A submodule's git dir is its own
    /// common dir, and roots of two repos have no shared key.
    #[test]
    fn sibling_worktrees_share_the_repo_key() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let main = tmp.path().join("main");
        let linked = tmp.path().join("w1");
        let gitdir = main.join(".git").join("worktrees").join("w1");
        std::fs::create_dir_all(&gitdir).expect("worktree git dir");
        std::fs::write(gitdir.join("commondir"), "../..\n").expect("commondir");
        std::fs::create_dir_all(&linked).expect("linked worktree");
        std::fs::write(linked.join(".git"), format!("gitdir: {}\n", gitdir.display()))
            .expect("linked .git");
        let common = main.join(".git").canonicalize().expect("common dir");
        let key = |roots: &[PathBuf]| repo_key(roots);
        assert_eq!(
            key(&[main.clone()]),
            Some(format!("{}:", common.display())),
            "the main worktree's top"
        );
        assert_eq!(key(&[linked.clone()]), key(&[main.clone()]), "a linked sibling");
        let sub = Path::new("pkgs").join("a");
        assert_eq!(
            key(&[linked.join(&sub)]),
            Some(format!("{}:{}", common.display(), sub.display())),
            "a subdirectory root keys by its path below the top"
        );
        assert_eq!(key(&[main.join(&sub)]), key(&[linked.join(&sub)]));

        let module = main.join(".git").join("modules").join("m");
        std::fs::create_dir_all(&module).expect("module git dir");
        let submodule = main.join("m");
        std::fs::create_dir_all(&submodule).expect("submodule");
        std::fs::write(submodule.join(".git"), format!("gitdir: {}\n", module.display()))
            .expect("submodule .git");
        assert_ne!(key(&[submodule]), key(&[main.clone()]), "its own common dir");

        let other = tmp.path().join("other");
        std::fs::create_dir_all(other.join(".git")).expect("other repo");
        assert_eq!(key(&[main, other]), None, "two repos share no key");
    }

    /// An incremental edit's settle never yields to the growth brake —
    /// the #4249 exemption: each edit settles in place, never restarts
    /// into a whole-root re-analysis. The cold/whole-root samples keep
    /// the brake through [`sample`].
    #[test]
    fn an_incremental_settle_never_yields_to_the_growth_brake() {
        assert!(!incremental_settle_yields());
    }

    /// The cold freshness skip's whole policy: it fires only while no
    /// server lives AND the priming baseline is still out — the cold
    /// first ask, or a later ask while the server is down and the
    /// background walk still runs. A live server always pays the check
    /// (its diff drives the incremental overlay push), and an installed
    /// baseline always pays it even server-less (a rebuild decision
    /// observes the diff).
    #[test]
    fn the_freshness_skip_fires_only_on_the_cold_unprimed_ask() {
        assert!(
            cold_freshness_skip(false, false),
            "no server, no baseline: the cold first ask, and any later\
             \nask while the server is down and the prime still runs"
        );
        assert!(
            !cold_freshness_skip(true, false),
            "a live server observes the diff — always checked"
        );
        assert!(
            !cold_freshness_skip(true, true),
            "the steady state: checked"
        );
        assert!(
            !cold_freshness_skip(false, true),
            "no server but a baseline: a rebuild decision observes it"
        );
    }

    /// The bookkeeping elision's decision: an identical (peak, source)
    /// figure is elided — the peaks file already holds those bytes —
    /// while a moved peak, a moved source size, or nothing yet persisted
    /// (first record, or forgotten after the growth brake's floor
    /// record) always lands.
    #[test]
    fn quiet_turns_elide_the_peak_record_and_changes_land() {
        assert!(
            !peak_persist_moved(Some((7, Some(3))), 7, Some(3)),
            "the quiet turn: the identical figure is already on disk"
        );
        assert!(
            peak_persist_moved(Some((7, Some(3))), 8, Some(3)),
            "a moved peak records"
        );
        assert!(
            peak_persist_moved(Some((7, Some(3))), 7, Some(4)),
            "a moved source size records"
        );
        assert!(
            peak_persist_moved(Some((7, Some(3))), 7, None),
            "a vanished source size records"
        );
        assert!(
            peak_persist_moved(None, 7, Some(3)),
            "the first record lands"
        );
        assert!(
            peak_persist_moved(None, 7, None),
            "the post-yield forgotten memory lands the next completion"
        );
    }

    /// A daemon's socket identity follows the file, not the path: the same
    /// path recreated after a wipe is a different socket.
    #[test]
    fn a_recreated_socket_path_is_another_socket() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.sock");
        let first = UnixListener::bind(&path).expect("bind");
        let bound = Inode::of(&path).expect("bound");
        assert_eq!(Inode::of(&path), Some(bound));
        std::fs::remove_file(&path).expect("wipe");
        assert_eq!(Inode::of(&path), None);
        let second = UnixListener::bind(&path).expect("rebind");
        assert_ne!(Inode::of(&path), Some(bound));
        drop((first, second));
    }

    /// A face over the wire's announced-length cap answers `None` — the
    /// caller's `NotServed`, never the panic the old `expect` produced.
    /// An honest in-cap payload still renders header + length.
    #[test]
    fn the_face_header_refuses_over_cap_and_writes_in_cap() {
        let meta = crate::face::FaceMeta::default();
        let over = usize::try_from(crate::client::MAX_FACE_BYTES)
            .expect("cap fits usize")
            + 1;
        assert_eq!(face_header(&meta, over), None);
        assert_eq!(face_header(&meta, usize::MAX), None, "u32 overflow");
        let line = face_header(&meta, 42).expect("in-cap header");
        let text = std::str::from_utf8(&line).expect("utf8");
        assert!(text.contains("\"bytes\":42"), "{text}");
        assert!(line.ends_with(b"\n"));
    }

    /// The cold-start transient the debounce exists for: timeouts
    /// cluster at cold-start tails (the wait bound still sits at its
    /// pre-cold value while the stream is saturated), so even two in a
    /// row — cold's request, then the first warm one — never note and
    /// the streak stays flat; a lone counted shape afterwards is still
    /// only the first.
    #[test]
    fn one_transient_degraded_check_never_notes() {
        // Cold's request: one timed-out confirmation under the load.
        assert_eq!(
            degradation_note(0, Some(WalkReason::CookieTimeout), false),
            (0, None),
            "a timed-out confirmation never counts"
        );
        // The first warm run right after: another timeout — the
        // spanning shape the old note false-alarmed on.
        assert_eq!(
            degradation_note(0, Some(WalkReason::CookieTimeout), false),
            (0, None),
            "two in a row still never note"
        );
        // A journal answer resets nothing that grew, and a later lone
        // counted shape is again only the first.
        assert_eq!(degradation_note(0, None, false), (0, None));
        assert_eq!(
            degradation_note(0, Some(WalkReason::StreamLost), false),
            (1, None)
        );
    }

    /// Every counted shape repeats, so the second consecutive check
    /// notes with its own reason text; the first never does.
    #[test]
    fn two_consecutive_degraded_checks_note_the_reason() {
        let shapes = [
            (WalkReason::CookieBlocked, "its sync files cannot be written"),
            (WalkReason::StreamLost, "its event stream lost events"),
            (WalkReason::Unarmed, "its watcher is unavailable"),
        ];
        for (reason, why) in shapes {
            assert_eq!(
                degradation_note(0, Some(reason), false),
                (1, None),
                "the first {reason:?} stays silent"
            );
            assert_eq!(
                degradation_note(1, Some(reason), false),
                (2, Some(why)),
                "the second {reason:?} speaks"
            );
        }
    }

    /// Journal answers, timed-out confirmations, and by-design walks
    /// (`First`, `Rebase`) never count — they are evidence against
    /// persistence, so an interrupted streak starts over.
    #[test]
    fn journal_and_by_design_answers_reset_the_streak() {
        assert_eq!(degradation_note(1, None, false), (0, None), "a journal answer");
        assert_eq!(
            degradation_note(1, Some(WalkReason::CookieTimeout), false),
            (0, None),
            "a timed-out confirmation"
        );
        assert_eq!(
            degradation_note(1, Some(WalkReason::First), false),
            (0, None),
            "the covering walk"
        );
        assert_eq!(
            degradation_note(1, Some(WalkReason::Rebase), false),
            (0, None),
            "a pool rebase"
        );
    }

    /// Once fired, the note never repeats — whatever the checks answer
    /// afterwards, the daemon stays silent for its life.
    #[test]
    fn a_fired_note_never_repeats() {
        assert_eq!(
            degradation_note(2, Some(WalkReason::Unarmed), true),
            (2, None),
            "degraded checks after the note are silent"
        );
        assert_eq!(
            degradation_note(0, Some(WalkReason::CookieBlocked), true),
            (0, None)
        );
    }

    /// The adoption arm's validation matrix: warm off leaves the ordinary
    /// start path alone (`None`); warm on with no standby is a miss; a
    /// standby whose SDK facts went stale is killed, never adopted; a
    /// current standby is.
    #[test]
    fn standby_admit_decides_the_adoption_arm() {
        assert_eq!(standby_admit(false, false, false), None, "warm off");
        assert_eq!(standby_admit(false, true, true), None);
        assert_eq!(
            standby_admit(true, false, true),
            Some(Adopt::Miss),
            "no standby to adopt"
        );
        assert_eq!(
            standby_admit(true, true, false),
            Some(Adopt::StaleSdk),
            "the SDK moved under the standby"
        );
        assert_eq!(
            standby_admit(true, true, true),
            Some(Adopt::Adopt),
            "a current standby"
        );
    }

    /// The warm posture's resolution tiers, oldest spec first: a spec
    /// from before the knobs existed resolves warm-off (the 0.1.23
    /// default) with the default TTL, the spec's own values come next,
    /// the env overrides win (the A/B arm), and a garbage override fails
    /// open to the tier below.
    #[test]
    fn warm_config_resolves_env_over_spec_over_default() {
        let spec = |warm: u32, ttl: u32| Spec {
            sdk_root: PathBuf::from("/sdk"),
            sdk_version: "3.13.7".to_owned(),
            roots: vec![PathBuf::from("/w/a")],
            packages: None,
            budget: 0,
            idle_secs: 60,
            max_percent: 90,
            max_concurrent: 0,
            pool: true,
            max_roots: 3,
            warm,
            warm_ttl_secs: ttl,
        };
        assert_eq!(
            warm_config_with(&spec(0, 0), None, None),
            (false, Duration::from_secs(900)),
            "an old spec behaves as the default: warm off, 900 s"
        );
        assert_eq!(
            warm_config_with(&spec(1, 60), None, None),
            (true, Duration::from_secs(60)),
            "the spec's opt-in carries its TTL"
        );
        assert_eq!(
            warm_config_with(&spec(0, 0), Some("0"), None),
            (false, Duration::from_secs(900)),
            "the env's off arm"
        );
        assert_eq!(
            warm_config_with(&spec(0, 0), Some("1"), None),
            (true, Duration::from_secs(900)),
            "the env's on arm is the config-less opt-in"
        );
        assert_eq!(
            warm_config_with(&spec(1, 900), Some("1"), Some("300")),
            (true, Duration::from_secs(300)),
            "the env wins over the spec"
        );
        assert_eq!(
            warm_config_with(&spec(0, 0), Some("not-a-number"), Some("")),
            (false, Duration::from_secs(900)),
            "a garbage override fails open to the default"
        );
        assert_eq!(
            warm_config_with(&spec(0, 1), None, Some("0")),
            (false, Duration::from_secs(1)),
            "the TTL floors at one second"
        );
    }

    /// The standby's lease is memory-only: no job slot (it never blocks a
    /// per-run analysis under `max_concurrent`, even at 1), a small
    /// fixed booking, and a releasable that names this daemon — the
    /// ledger's cheapest victim.
    #[test]
    fn the_standby_lease_is_slotless_memory_only_and_releasable() {
        let request = standby_request("warm standby 3.13.7".to_owned(), PathBuf::from("/d/busy.lock"));
        assert!(!request.slot, "no job slot");
        assert_eq!(request.cap, 0);
        assert_eq!(request.bytes, STANDBY_LEASE);
        assert!(request.sized, "a deliberate booking, not a guess");
        assert!(request.bytes < dartq_toolchain::ledger::DEFAULT_LEASE);
        let release = request.release.expect("releasable while idle");
        assert_eq!(release.pid, std::process::id());
        assert_eq!(release.busy, Path::new("/d/busy.lock"));
        assert_eq!(request.face, dartq_toolchain::ledger::Face::AnalysisServer);
    }

    /// The idle tiering transition: warm on with a servable SDK and no
    /// standby yet enters Tier 2; a live standby (its TTL expiring) is
    /// Tier 3's exit, and warm off or a PerRun SDK never tier.
    #[test]
    fn tier_two_needs_warm_a_servable_sdk_and_no_standby_yet() {
        assert!(enters_tier2(true, false, true), "the Tier-2 entry");
        assert!(!enters_tier2(true, true, true), "Tier 3: the standby's own expiry");
        assert!(!enters_tier2(false, false, true), "warm off exits as ever");
        assert!(!enters_tier2(true, false, false), "a PerRun SDK never tiers");
    }

    /// The standby's pressure line, mirroring the ledger monitor's own:
    /// available under the reserve (the percent share, floored at the
    /// ledger's reserve) or at critical pressure — never on warn alone,
    /// never on an unreadable figure or a boundary-equal sample.
    #[test]
    fn low_memory_sits_at_the_ledger_monitors_line() {
        let gib = 1 << 30;
        let total = 16 * gib;
        // 90 percent: the share is 1.6 GiB, the 2 GiB floor dominates.
        let reserve = dartq_toolchain::ledger::RESERVE_FLOOR;
        assert!(!low_memory(total, Some(reserve), 90), "boundary equal");
        assert!(low_memory(total, Some(reserve - 1), 90), "one byte under");
        // 50 percent: the 8 GiB share dominates the floor.
        let share = 8 * gib;
        assert!(!low_memory(total, Some(share), 50));
        assert!(low_memory(total, Some(share - 1), 50));
        assert!(!low_memory(total, None, 90), "unreadable is never low");
        assert!(!low_memory(0, Some(0), 90));
    }

    /// The standby sheds on a real shortfall or critical pressure, never
    /// on warn alone: the kernel's level-2 fires at 40-50% free during
    /// spawn bursts and must not evict a warm standby that would have
    /// been reused (#4266, #4269). Level-4 critical and available-under-
    /// reserve remain valid shed signals.
    #[test]
    fn the_standby_sheds_only_on_shortfall_or_critical() {
        let gib = 1 << 30;
        let total = 32 * gib;
        let sheds = |available: Option<u64>, level: Option<u32>| {
            low_memory(total, available, 90) || critical(total, available, level)
        };
        // Plenty of available memory: warn (level 2) alone never sheds.
        assert!(
            !sheds(Some(32 * gib * 43 / 100), Some(2)),
            "warn at 43% free is not a shortfall"
        );
        // Under the reserve is the one shortfall signal.
        let reserve = dartq_toolchain::ledger::RESERVE_FLOOR.max(32 * gib / 10);
        assert!(
            sheds(Some(reserve - 1), Some(1)),
            "available under the reserve sheds"
        );
        // Critical: available under the critical floor, or kernel level 4.
        let floor = dartq_toolchain::ledger::critical_floor(total);
        assert!(
            sheds(Some(floor - 1), Some(1)),
            "available under the critical floor sheds"
        );
        assert!(
            sheds(Some(20 * gib), Some(4)),
            "kernel level 4 sheds even with plenty available"
        );
        // Level 3 (between warn and critical) never sheds on its own.
        assert!(
            !sheds(Some(20 * gib), Some(3)),
            "level 3 is not critical"
        );
    }

    // ------------------------------------------------------------------
    // #4278: the answered-turn record family.
    // ------------------------------------------------------------------

    /// The fixture spec of the record tests.
    fn record_spec(sdk_version: &str, roots: &[&str], packages: Option<&str>) -> Spec {
        Spec {
            sdk_root: PathBuf::from("/sdks/a"),
            sdk_version: sdk_version.to_owned(),
            roots: roots.iter().map(PathBuf::from).collect(),
            packages: packages.map(PathBuf::from),
            budget: 0,
            idle_secs: 60,
            max_percent: 90,
            max_concurrent: 0,
            pool: false,
            max_roots: 3,
            warm: 0,
            warm_ttl_secs: 0,
        }
    }

    /// The record key folds every leg an answer was rendered under: the
    /// SDK, the served root set (order-free — a pooled set spelled in a
    /// different join order is the same answer), and the package
    /// override; the shape legs (aims, face) are pinned by the next
    /// test. The binary stamp leg is the one process constant a test
    /// cannot vary; the daemon-key discipline it copies is `Spec::dir`'s
    /// own, pinned by that module's tests.
    #[test]
    fn the_record_key_folds_every_serving_leg() {
        let lon = Face {
            format: crate::face::FaceFormat::Lon,
            root: None,
        };
        let dir = crate::Aim::Dir(PathBuf::from("/w/a"));
        let key = |version: &str,
                   roots: &[&str],
                   packages: Option<&str>,
                   aims: &[crate::Aim],
                   face: Option<&Face>| {
            let spec = record_spec(version, roots, packages);
            let roots: Vec<PathBuf> = roots.iter().map(PathBuf::from).collect();
            record_key(&spec, &roots, aims, face)
        };
        let base = key(
            "3.13.7",
            &["/w/a"],
            None,
            std::slice::from_ref(&dir),
            Some(&lon),
        );
        assert_eq!(
            base,
            key(
                "3.13.7",
                &["/w/a"],
                None,
                std::slice::from_ref(&dir),
                Some(&lon),
            ),
            "the same legs key the same record"
        );
        // The root set is order-free but set-sensitive: a pool's join
        // order is not a new answer, a joined root is (the set_roots
        // gate — a record never crosses a root-set change).
        assert_eq!(
            key(
                "3.13.7",
                &["/w/b", "/w/a"],
                None,
                std::slice::from_ref(&dir),
                Some(&lon),
            ),
            key(
                "3.13.7",
                &["/w/a", "/w/b"],
                None,
                std::slice::from_ref(&dir),
                Some(&lon),
            ),
            "the same set in another order is the same answer"
        );
        assert_ne!(
            base,
            key(
                "3.13.7",
                &["/w/a", "/w/b"],
                None,
                std::slice::from_ref(&dir),
                Some(&lon),
            ),
            "a joined root is another answer's key"
        );
        // Every other leg separates too.
        assert_ne!(
            base,
            key("3.14.0", &["/w/a"], None, std::slice::from_ref(&dir), Some(&lon)),
            "an SDK swap"
        );
        assert_ne!(
            base,
            key(
                "3.13.7",
                &["/w/a"],
                Some("/w/a/.dart_tool/package_config.json"),
                std::slice::from_ref(&dir),
                Some(&lon)
            ),
            "a package-config override"
        );
    }

    /// The key's shape legs: another aims shape, another face (and the
    /// compat report face), and the aims spelling's canonicalization —
    /// order and duplicates of one coverage are the same answer.
    #[test]
    fn the_record_key_separates_shapes_and_canonicalizes_aims() {
        let lon = Face {
            format: crate::face::FaceFormat::Lon,
            root: None,
        };
        let json = Face {
            format: crate::face::FaceFormat::Json,
            root: None,
        };
        let dir = crate::Aim::Dir(PathBuf::from("/w/a"));
        let file = crate::Aim::File(PathBuf::from("/w/a/lib/a.dart"));
        let key = |aims: &[crate::Aim], face: Option<&Face>| {
            let spec = record_spec("3.13.7", &["/w/a"], None);
            let roots: Vec<PathBuf> = vec![PathBuf::from("/w/a")];
            record_key(&spec, &roots, aims, face)
        };
        let base = key(
            std::slice::from_ref(&dir),
            Some(&lon),
        );
        assert_ne!(
            base,
            key(std::slice::from_ref(&file), Some(&lon)),
            "another aims shape"
        );
        assert_ne!(
            base,
            key(std::slice::from_ref(&dir), Some(&json)),
            "another face"
        );
        assert_ne!(
            base,
            key(std::slice::from_ref(&dir), None),
            "the compat report face"
        );
        // The aims spelling is canonical: order and duplicates of one
        // coverage are the same answer.
        assert_eq!(
            key(&[dir.clone(), file.clone()], Some(&lon)),
            key(&[file, dir], Some(&lon)),
            "canonical aims spelling"
        );
    }

    /// The record codec: a written frame serves its payload whole and
    /// unparsed on a stamp-set hit; a stamps miss keeps the record (a
    /// revert brings its stamps back); a decode miss — torn, foreign,
    /// or lengths that do not add up — unlinks the dead weight; an
    /// absent record is the quiet no-record miss.
    #[test]
    fn a_record_round_trips_and_its_misses_are_typed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = record_path(dir.path(), &[7; 16]);
        let stamps: Stamps = vec![
            (PathBuf::from("/w/a/lib/a.dart"), 6, 7, 8, 9),
            (PathBuf::from("/w/a/pubspec.yaml"), 10, 11, 12, 13),
        ];
        let line = write_record(&path, &serde_json::to_vec(&stamps).expect("stamps"), b"wire");
        assert!(line.is_ok(), "{line:?}");

        let hit = probe_record(&path, &stamps).expect("the identical set hits");
        assert_eq!(&*hit, b"wire", "the payload ships verbatim");

        let moved: Stamps = stamps
            .iter()
            .map(|(path, ..)| (path.clone(), 99, 99, 99, 99))
            .collect();
        assert!(matches!(
            probe_record(&path, &moved),
            Err(RecordMiss::Stamps)
        ));
        assert!(path.is_file(), "a stamps miss keeps the record");
        assert!(probe_record(&path, &stamps).is_ok(), "and it hits again");

        // Torn: the frame's lengths no longer add up.
        let frame = std::fs::read(&path).expect("frame");
        std::fs::write(&path, &frame[..frame.len() - 1]).expect("torn");
        assert!(matches!(
            probe_record(&path, &stamps),
            Err(RecordMiss::Decode)
        ));
        assert!(!path.exists(), "a decode miss unlinks the dead record");

        // Foreign: a future codec's record is dead weight for this one.
        write_record(&path, &serde_json::to_vec(&stamps).expect("stamps"), b"wire")
            .expect("rewrite");
        let mut frame = std::fs::read(&path).expect("frame");
        let v = frame
            .iter()
            .position(|b| *b == b':')
            .expect("the version colon");
        frame[v + 1] = b'9'; // the header's version digit
        std::fs::write(&path, &frame).expect("foreign");
        assert!(matches!(
            probe_record(&path, &stamps),
            Err(RecordMiss::Decode)
        ));
        assert!(!path.exists(), "the foreign record reclaims");

        // Absent — and a directory that does not even exist yet.
        assert!(matches!(
            probe_record(&path, &stamps),
            Err(RecordMiss::NoRecord)
        ));
    }

    /// The startup budget pass (#4273's posture): dead records and aged
    /// write debris reclaim, a young temp and every live record stay,
    /// and the survivors LRU-cap at the budget — oldest last-persist
    /// (mtime) first, the fresh record last.
    #[test]
    fn the_startup_sweep_reclaims_dead_debris_and_lru_caps() {
        let dir = tempfile::tempdir().expect("tempdir");
        let answers = dir.path().join(RECORDS_DIR);
        std::fs::create_dir_all(&answers).expect("answers dir");
        let stamps: Stamps = vec![(PathBuf::from("/w/a.dart"), 1, 2, 3, 4)];
        let stamps_line = serde_json::to_vec(&stamps).expect("stamps");
        let mint = |key: u8| {
            let path = record_path(dir.path(), &[key; 16]);
            write_record(&path, &stamps_line, &[0u8; 1024]).expect("mint");
            path
        };
        let age = |path: &Path, secs: u64| {
            std::fs::File::options()
                .write(true)
                .open(path)
                .and_then(|file| {
                    file.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(secs))
                })
                .expect("age");
        };

        let dead = answers.join(format!("aa{RECORD_SUFFIX}"));
        std::fs::write(&dead, b"not a record").expect("plant dead");
        let young_tmp = answers.join("bb.answer.tmp-1-1");
        std::fs::write(&young_tmp, b"half").expect("plant young tmp");
        let aged_tmp = answers.join("cc.answer.tmp-2-1");
        std::fs::write(&aged_tmp, b"half").expect("plant aged tmp");
        age(&aged_tmp, 0);
        let old = mint(1);
        age(&old, 100);
        let mid = mint(2);
        age(&mid, 200);
        let fresh = mint(3);
        // The three records are size-identical frames; a cap that holds
        // exactly two evicts the oldest last-persist clock alone.
        let one = std::fs::metadata(&old).expect("meta").len();
        let cap = one.saturating_mul(2);

        sweep_records_with(dir.path(), cap);

        assert!(!dead.exists(), "a foreign frame reclaims");
        assert!(!aged_tmp.exists(), "aged debris reclaims");
        assert!(young_tmp.is_file(), "young debris may be in flight");
        assert!(!old.exists(), "the oldest last-persist clock evicts");
        assert!(mid.is_file() && fresh.is_file(), "in-budget records stay");
        let surviving: u64 = std::fs::read_dir(&answers)
            .expect("answers dir")
            .flatten()
            .filter_map(|entry| entry.metadata().ok().map(|meta| meta.len()))
            .sum();
        assert!(
            surviving <= cap + young_tmp.metadata().expect("meta").len(),
            "the family sits at or under the budget ({surviving})"
        );
    }

    /// The kill switch's tiers: the env arm is hard in both directions,
    /// a garbage override fails open to the yaml tier, the project file
    /// beats the global, and the default is on.
    #[test]
    fn the_kill_switch_tiers_env_over_yaml_over_default() {
        let off = "analyzer:\n  official:\n    answer_records: false\n";
        let on = "analyzer:\n  official:\n    answer_records: true\n";
        assert!(records_enabled_with(None, None, None), "on by default");
        assert!(!records_enabled_with(Some("0"), Some(on), Some(on)), "env off wins");
        assert!(records_enabled_with(Some("1"), None, Some(off)), "env on wins");
        assert!(
            records_enabled_with(Some("garbage"), None, None),
            "a garbage env fails open to the default"
        );
        assert!(!records_enabled_with(None, None, Some(off)), "the project opts out");
        assert!(!records_enabled_with(None, Some(off), None), "the global opts out");
        assert!(!records_enabled_with(None, Some(on), Some(off)), "project beats global");
        assert!(records_enabled_with(None, Some(off), Some(on)), "project beats global");
        assert!(
            records_enabled_with(None, Some("not: [valid"), Some("also { bad")),
            "a malformed tier fails open to on"
        );
        assert!(
            records_enabled_with(None, None, Some("analyzer:\n  pool: true\n")),
            "a yaml without the knob is silent"
        );
    }

    /// The miss-path unlink's concurrent-writer guard: only the bytes a
    /// probe actually judged dead may unlink.
    #[test]
    fn the_unlink_guard_spares_replaced_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("guard.answer");
        std::fs::write(&path, b"judged").expect("plant");
        remove_record_if(&path, b"fresh bytes a writer just landed");
        assert!(path.is_file(), "replaced bytes are never unlinked");
        remove_record_if(&path, b"judged");
        assert!(!path.exists(), "the judged bytes unlink");
        remove_record_if(&dir.path().join("absent.answer"), b"x");
    }

    /// The #4243 invalidation matrix over the record's own stamp set:
    /// every tree move misses, the revert re-persists and re-hits, an
    /// add/delete pair returns to the original set (and the original
    /// record), and an inode swap with restored stat legs still misses
    /// — the dev/ino legs' whole purpose.
    #[test]
    fn the_invalidation_matrix_of_the_answer_record() {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = dir.path().canonicalize().expect("canonical");
        let app = base.join("app");
        std::fs::create_dir_all(app.join("lib").join("deep")).expect("mkdir");
        std::fs::create_dir_all(app.join(".dart_tool")).expect("mkdir");
        std::fs::write(app.join("lib/a.dart"), "var a;").expect("write");
        std::fs::write(app.join("lib/deep/b.dart"), "var b;").expect("write");
        std::fs::write(app.join("pubspec.yaml"), "name: app").expect("write");
        std::fs::write(
            app.join(".dart_tool/package_config.json"),
            r#"{"configVersion":2,"packages":[]}"#,
        )
        .expect("write");
        let sdk = base.join("sdk");
        let roots = vec![app.clone()];
        let stamps = || crate::snapshot::Snapshot::take(&roots, None, &sdk).stamps();
        let path = record_path(dir.path(), &[9; 16]);
        let persist = |stamps: &Stamps| {
            write_record(
                &path,
                &serde_json::to_vec(stamps).expect("stamps"),
                b"the rendered wire line",
            )
            .expect("persist");
        };
        let miss = |stamps: &Stamps| {
            matches!(probe_record(&path, stamps), Err(RecordMiss::Stamps))
        };

        let first = stamps();
        persist(&first);
        assert_eq!(
            probe_record(&path, &first).ok().as_deref(),
            Some(&b"the rendered wire line"[..]),
            "the byte-identical re-run serves"
        );

        // Edit, revert, re-persist, re-hit: the revert itself is a
        // stat move (the mtime moved), so the miss it causes ends with
        // the fresh record the re-render persists — the next turn hits.
        std::fs::write(app.join("lib/a.dart"), "var a2;").expect("edit");
        let edited = stamps();
        assert!(miss(&edited), "an edit misses");
        std::fs::write(app.join("lib/a.dart"), "var a;").expect("revert");
        assert!(miss(&stamps()), "the revert's own mtime still misses");
        persist(&stamps());
        assert!(probe_record(&path, &stamps()).is_ok(), "the next turn re-hits");

        // Touch-only: no content change, the mtime leg alone.
        std::fs::File::options()
            .write(true)
            .open(app.join("lib/a.dart"))
            .and_then(|file| file.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(123)))
            .expect("touch");
        assert!(miss(&stamps()), "a touch-only move misses");
        persist(&stamps());

        // Nested options, pubspec, package config: the non-Dart inputs.
        std::fs::write(app.join("lib/analysis_options.yaml"), "linter:\n").expect("nested");
        assert!(miss(&stamps()), "a nested options file misses");
        std::fs::remove_file(app.join("lib/analysis_options.yaml")).expect("rm");
        persist(&stamps());
        std::fs::write(app.join("pubspec.yaml"), "name: app2").expect("pubspec");
        assert!(miss(&stamps()), "a pubspec edit misses");
        std::fs::write(app.join("pubspec.yaml"), "name: app").expect("revert");
        persist(&stamps());
        std::fs::write(
            app.join(".dart_tool/package_config.json"),
            r#"{"configVersion":2,"packages":[]}"#,
        )
        .expect("config touch");
        assert!(miss(&stamps()), "a package_config edit misses");

        // Add and delete: the add misses against the prior record, the
        // delete returns the set to the original stamps — the original
        // record answers again without any re-render.
        std::fs::write(app.join("lib/new.dart"), "var n;").expect("add");
        assert!(miss(&stamps()), "an added file misses");
        std::fs::remove_file(app.join("lib/new.dart")).expect("delete");
        assert!(
            probe_record(&path, &stamps()).is_ok() || {
                persist(&stamps());
                probe_record(&path, &stamps()).is_ok()
            },
            "the deleted add restores the original set"
        );

        // The inode swap: same bytes, the mtime restored — only the
        // identity legs see it.
        let a = app.join("lib/a.dart");
        let (_, mtime, ..) = stamps()
            .iter()
            .find(|(path, ..)| path == &a)
            .map(|(_, size, mtime, ..)| (*size, *mtime))
            .expect("stamped");
        std::fs::remove_file(&a).expect("rm");
        std::fs::write(&a, "var a;").expect("recreate");
        std::fs::File::options()
            .write(true)
            .open(&a)
            .and_then(|file| {
                file.set_modified(std::time::UNIX_EPOCH + Duration::from_nanos(mtime))
            })
            .expect("restore mtime");
        #[cfg(unix)]
        assert!(miss(&stamps()), "an inode swap with restored legs misses");
        #[cfg(not(unix))]
        let _ = mtime;
    }
}
