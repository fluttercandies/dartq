//! The serving side: one daemon per [`Spec`], one server per daemon.
//!
//! The daemon binds its socket before the server starts, so clients that
//! arrive during the first analysis connect and wait. An accept thread
//! hands connections to the one worker, which owns the server: requests
//! run one after another, and the server's stdin has a single writer.
//! Each request pays one freshness check — the journaled watcher's
//! dirty merge when the stream is armed, else the stat walk; an
//! unchanged tree answers from the error
//! map with no server round trip, an edited Dart file is one incremental
//! analysis, and an added or deleted file or a configuration edit restarts
//! the server. A server that faults is restarted once
//! per request; a second fault answers the client with the fault, and the
//! client runs `dart analyze` itself. With no request for the idle timeout
//! the daemon shuts the server down and exits.
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
    let mut last = Instant::now();
    // The shared pressure monitor: while this daemon is idle it samples
    // host memory through the ledger's one policy; two consecutive lows
    // evict the least-recently-used idle releasable resident on the
    // machine (a peer analyzer or a session daemon — the heavy holders).
    let mut pressure = dartq_toolchain::ledger::Pressure::default();
    loop {
        match rx.recv_timeout(
            OWNERSHIP_CHECK
                .min(idle)
                .min(dartq_toolchain::ledger::PRESSURE_TICK),
        ) {
            Ok(stream) => {
                host.answer(stream);
                last = Instant::now();
                pressure.reset();
            }
            Err(RecvTimeoutError::Timeout) => {
                if let Some(victim) = pressure.tick(std::process::id(), spec.max_percent, true) {
                    eprintln!("dartq analyze: released the idle {victim} under memory pressure");
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
            return 0;
        }
        if last.elapsed() >= idle {
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
    host.stop("idle-evict");
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
    /// The growth-brake's running state (the ledger's shared policy):
    /// consecutive samples under the critical floor arm the yield.
    brake: dartq_toolchain::ledger::GrowthBrake,
    /// The server's `--cache`, whose last use each answer marks.
    cache: PathBuf,
    /// The caches' disk budget: the request-time check, the in-analysis
    /// tick, the server-start pass and the background passes.
    disk: crate::disk::Disk,
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
            freshness: Freshness::new(&spec.roots, spec.packages.as_deref(), &spec.sdk_home()),
            degrade_streak: 0,
            degrade_noted: false,
            files,
            socket,
            lease: None,
            max_percent: spec.max_percent,
            max_concurrent: spec.max_concurrent,
            peak_key: repo_key(&spec.roots).unwrap_or_else(|| cache.display().to_string()),
            source_bytes: source_bytes(&spec.roots),
            brake: dartq_toolchain::ledger::GrowthBrake::default(),
            cache,
            disk: crate::disk::Disk::start(dir),
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
            // old snapshot rebase did).
            let _ = self.freshness.check(&roots);
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
                            let Some(mut wire) = face_header(&render.meta, render.bytes.len())
                            else {
                                return Reply::Answer(Answer::NotServed);
                            };
                            wire.extend_from_slice(&render.bytes);
                            let line: Arc<[u8]> = wire.into();
                            return Reply::Line(line);
                        }
                        match server.render_face(&owned, face, self.renderer) {
                            Some(line) => Reply::Line(line),
                            None => Reply::Answer(Answer::NotServed),
                        }
                    }
                    None => {
                        if owned.is_empty() {
                            return Reply::Answer(Answer::Report(Report::default()));
                        }
                        match server.render(&owned) {
                            Some(line) => Reply::Line(line),
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
            Ok(ask) if ask.roots == self.roots => {
                self.analyze(&stream, &ask.aims, ask.face.as_ref())
            }
            Ok(_) => Reply::Answer(Answer::Fault("the daemon serves other roots".into())),
            Err(error) => Reply::Answer(Answer::Fault(format!("unreadable request: {error}"))),
        };
        send_reply(&stream, reply);
        if let Some(lease) = &self.lease {
            lease.touch();
        }
        // The cache's last use: the disk budget drops idle caches least
        // recently used first.
        crate::disk::used(&self.cache);
        // The lease's mtime is the last answer: the ledger releases the
        // least recently used resident first. A daemon whose socket path
        // now names a successor's socket leaves the successor's state be.
        if Inode::of(&self.files.socket) == Some(self.socket) {
            let _ = std::fs::write(&self.files.state, std::process::id().to_string());
        }
    }

    fn analyze(&mut self, client: &UnixStream, aims: &[crate::Aim], face: Option<&Face>) -> Reply {
        match self.analyze_result(client) {
            Ok(()) => {
                let server = self.server.as_ref().expect("a live server");
                match face {
                    Some(face) => match server.render_face(aims, face, self.renderer) {
                        Some(line) => Reply::Line(line),
                        None => Reply::Answer(Answer::NotServed),
                    },
                    None => match server.render(aims) {
                        Some(line) => Reply::Line(line),
                        None => Reply::Answer(Answer::NotServed),
                    },
                }
            }
            Err(answer) => Reply::Answer(answer),
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
        let diff = self.freshness.check(&roots);
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
                    false
                })
            }
            Some(server) => server.wait_idle(|| sample(disk, brake)),
            None => Err(Fault::Server("no server yet".into())),
        };
        let outcome = match outcome {
            Ok(()) => Ok(()),
            Err(Fault::Budget { budget, measured }) => {
                self.forget("budget");
                return Err(Answer::Budget(
                    Fault::Budget { budget, measured }.to_string(),
                ));
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
                // waiting there borrows the figure.
                let peak = server.peak();
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
    /// event line once the child exists). A pooled daemon starts its
    /// server over the whole active set — the founding roots plus every
    /// joined worktree — so a join admitted while the server was down (or
    /// a restart after a fault) does not silently drop the joiners' roots.
    fn start(
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
        let server = Server::start(launch, &roots, self.budget, || sample(disk, brake))?;
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
        let (bytes, sized) =
            ledger::sizing_with_source(&self.peak_key, Face::AnalysisServer, self.source_bytes);
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
        match ledger::acquire(request, bytes, self.max_percent, deadline, |text| {
            note(client, text.to_owned());
        }) {
            Ok(lease) => Ok(lease),
            Err(error @ ledger::Error::Timeout { .. }) => Err(Fault::Memory(error.to_string())),
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
    /// re-queued ask is admitted on the honest figure.
    fn yield_peak(&self, peak: u64) {
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
        Some(WalkReason::First | WalkReason::Rebase | WalkReason::CookieTimeout) | None => {
            return (0, None);
        }
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

/// The analyzed Dart source size under `roots`: a bounded stat walk
/// (`.dart` files only, no reads). Cheap beside a minutes-long cold
/// analysis; feeds the ledger's ratio estimate.
fn source_bytes(roots: &[PathBuf]) -> Option<u64> {
    let mut total = 0u64;
    for root in roots {
        sum_dart_bytes(root, &mut total);
    }
    (total > 0).then_some(total)
}

fn sum_dart_bytes(dir: &Path, total: &mut u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            sum_dart_bytes(&entry.path(), total);
        } else if entry.path().extension().is_some_and(|ext| ext == "dart") {
            *total += meta.len();
        }
    }
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
        let mut small: Vec<(PathBuf, Instant)> = vec![(PathBuf::from("/w/primary"), at(1500))];
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
        assert_eq!(
            served,
            [
                Path::new("/w/primary"),
                Path::new("/w/a"),
                Path::new("/w/c")
            ]
        );

        // Two joiners need two places; one cold root cannot give both.
        let mut mixed: Vec<(PathBuf, Instant)> = vec![
            (PathBuf::from("/w/primary"), at(30)),
            (PathBuf::from("/w/cold"), at(POOL_HOT.as_secs() * 2)),
            (PathBuf::from("/w/hot"), at(5)),
        ];
        let before = mixed.clone();
        assert_eq!(
            admit(
                &mut mixed,
                &[PathBuf::from("/w/x"), PathBuf::from("/w/y")],
                3,
                now
            ),
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
        std::fs::write(
            linked.join(".git"),
            format!("gitdir: {}\n", gitdir.display()),
        )
        .expect("linked .git");
        let common = main.join(".git").canonicalize().expect("common dir");
        let key = |roots: &[PathBuf]| repo_key(roots);
        assert_eq!(
            key(&[main.clone()]),
            Some(format!("{}:", common.display())),
            "the main worktree's top"
        );
        assert_eq!(
            key(&[linked.clone()]),
            key(&[main.clone()]),
            "a linked sibling"
        );
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
        std::fs::write(
            submodule.join(".git"),
            format!("gitdir: {}\n", module.display()),
        )
        .expect("submodule .git");
        assert_ne!(
            key(&[submodule]),
            key(&[main.clone()]),
            "its own common dir"
        );

        let other = tmp.path().join("other");
        std::fs::create_dir_all(other.join(".git")).expect("other repo");
        assert_eq!(key(&[main, other]), None, "two repos share no key");
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
        let over = usize::try_from(crate::client::MAX_FACE_BYTES).expect("cap fits usize") + 1;
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
            (
                WalkReason::CookieBlocked,
                "its sync files cannot be written",
            ),
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
        assert_eq!(
            degradation_note(1, None, false),
            (0, None),
            "a journal answer"
        );
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
}
