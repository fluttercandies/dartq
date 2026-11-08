//! The serving side: one daemon per [`Spec`], one server per daemon.
//!
//! The daemon binds its socket before the server starts, so clients that
//! arrive during the first analysis connect and wait. An accept thread
//! hands connections to the one worker, which owns the server: requests
//! run one after another, and the server's stdin has a single writer.
//! Each request is one stat walk; an unchanged tree answers from the error
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
//! * a server start (the whole analysis, the memory peak) takes a ticket
//!   of `dartq analyze`'s machine-wide run queue, the one per-run analyses
//!   take, so at most one full analysis runs at a time;
//! * before a start, idle peers are released, least recently used first,
//!   while the live servers plus this one's expected peak exceed
//!   [`MEMORY_SHARE`] of physical memory. A released peer restarts warm
//!   from its cache on its next request.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::report::Report;
use crate::server::{Fault, Launch, Server};
use crate::snapshot::Snapshot;
use crate::support::{Sdk, Support, Version, support};
use crate::{Files, Spec};

/// One client request line.
#[derive(Serialize, Deserialize)]
pub(crate) struct Ask {
    pub roots: Vec<PathBuf>,
}

/// One daemon answer line. [`Answer::Note`] lines (queue waits, released
/// peers) precede the one final line.
#[derive(Serialize, Deserialize)]
pub(crate) enum Answer {
    Report(Report),
    Fault(String),
    Budget(String),
    Note(String),
}

/// The share of physical memory the live servers may hold together before
/// idle peers are released (3/4: the rest is the user's editor, browser
/// and build tools).
const MEMORY_SHARE: (u64, u64) = (3, 4);

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

/// Runs the daemon over the spec in `dir` until it idles out. The return
/// value is the process exit code.
#[must_use]
pub fn serve(dir: &Path) -> u8 {
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
    crate::sweep(dir);
    crate::claim_cache(&spec.cache_dir(), &spec.roots);

    let (tx, rx) = mpsc::channel::<UnixStream>();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            if tx.send(stream).is_err() {
                break;
            }
        }
    });

    let mut host = Host::new(&spec, dir, socket);
    let idle = Duration::from_secs(spec.idle_secs.max(1));
    let mut last = Instant::now();
    loop {
        match rx.recv_timeout(OWNERSHIP_CHECK.min(idle)) {
            Ok(stream) => {
                host.answer(stream);
                last = Instant::now();
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        // The socket path is the daemon's only address. Once it names
        // another file (the directory was wiped and a client started a
        // successor) or nothing, no client can reach this daemon: its
        // server is dead weight, and the files at its paths are not its
        // own to remove.
        if Inode::of(&files.socket) != Some(socket) {
            if let Some(server) = host.server.take() {
                server.shutdown();
            }
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
    if let Some(server) = host.server.take() {
        server.shutdown();
    }
    0
}

extern "C" fn kill_group(_: libc::c_int) {
    // SAFETY: kill(2) is async-signal-safe; pid 0 is this process group.
    unsafe { libc::kill(0, libc::SIGKILL) };
}

struct Host {
    launch: Option<Launch>,
    roots: Vec<PathBuf>,
    packages: Option<PathBuf>,
    sdk_home: PathBuf,
    budget: Option<u64>,
    queue: Option<PathBuf>,
    server: Option<Server>,
    snapshot: Snapshot,
    dir: PathBuf,
    files: Files,
    /// The socket this daemon bound.
    socket: Inode,
    /// The server tree's peak RSS, persisted beside its cache: the
    /// expected peak of the next start.
    peak: PathBuf,
}

impl Host {
    fn new(spec: &Spec, dir: &Path, socket: Inode) -> Self {
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
                    cache: cache.clone(),
                    packages: spec.packages.clone(),
                })
            }
            _ => None,
        };
        Self {
            launch,
            roots: spec.roots.clone(),
            packages: spec.packages.clone(),
            sdk_home: spec.sdk_home(),
            budget: (spec.budget > 0).then_some(spec.budget),
            queue: spec.queue.clone(),
            server: None,
            snapshot: Snapshot::default(),
            dir: dir.to_path_buf(),
            files: Files::of(dir),
            socket,
            peak: cache.join("peak"),
        }
    }

    fn answer(&mut self, stream: UnixStream) {
        let mut reader = BufReader::new(&stream);
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            return;
        }
        // Held while the request runs: a peer that finds it free knows this
        // daemon is idle and may release it.
        let busy = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.files.busy);
        let _busy = busy.and_then(|file| file.lock().map(|()| file));
        let answer = match serde_json::from_str::<Ask>(&line) {
            Ok(ask) if ask.roots == self.roots => self.analyze(&stream),
            Ok(_) => Answer::Fault("the daemon serves other roots".into()),
            Err(error) => Answer::Fault(format!("unreadable request: {error}")),
        };
        send(&stream, &answer);
        // The mtime of the state file is the last answer: peers release
        // the least recently used daemon first. A daemon whose socket path
        // now names a successor's socket leaves the successor's state be.
        if Inode::of(&self.files.socket) == Some(self.socket) {
            let _ = std::fs::write(&self.files.state, std::process::id().to_string());
        }
    }

    fn analyze(&mut self, client: &UnixStream) -> Answer {
        let Some(launch) = self.launch.clone() else {
            return self.fail("the SDK changed under the daemon".into());
        };
        let fresh = Snapshot::take(&self.roots, self.packages.as_deref(), &self.sdk_home);
        let diff = self.snapshot.diff(&fresh);
        self.snapshot = fresh;
        if diff.rebuild
            && let Some(server) = self.server.take()
        {
            server.shutdown();
        }
        let outcome = match self.server.as_mut() {
            Some(server) if !diff.changed.is_empty() => server.changed(&diff.changed),
            Some(server) => server.wait_idle(),
            None => Err(Fault::Server("no server yet".into())),
        };
        let outcome = match outcome {
            Ok(()) => Ok(()),
            Err(Fault::Budget { budget, measured }) => {
                self.server = None;
                return Answer::Budget(Fault::Budget { budget, measured }.to_string());
            }
            Err(_) => {
                // Restart once: a fresh server reads the tree as it is now.
                self.server = None;
                self.start(&launch, client)
            }
        };
        match outcome {
            Ok(()) => {
                let _ = std::fs::remove_file(&self.files.failed);
                let server = self.server.as_ref().expect("a live server");
                if server.peak() > 0 {
                    let _ = std::fs::write(&self.peak, server.peak().to_string());
                }
                Answer::Report(server.report())
            }
            Err(fault @ Fault::Budget { .. }) => Answer::Budget(fault.to_string()),
            Err(Fault::Server(message)) => self.fail(message),
        }
    }

    /// Starts the server inside a run queue ticket, after making room.
    fn start(&mut self, launch: &Launch, client: &UnixStream) -> Result<(), Fault> {
        let _ticket = self.queue.as_deref().and_then(|root| {
            let mut queued = false;
            let ticket = dartq_toolchain::run_queue::acquire_with("analyze", root, 1, |event| {
                if let dartq_toolchain::run_queue::Event::Queued { ahead } = event {
                    queued = true;
                    note(
                        client,
                        format!(
                            "dartq analyze: {ahead} other analyze(s) in the queue — waiting \
                             (first come, first served)"
                        ),
                    );
                }
            });
            if queued && ticket.is_ok() {
                note(client, "dartq analyze: admitted".into());
            }
            // A queue that cannot be entered starts unqueued, as a per-run
            // analysis does.
            ticket.ok()
        });
        let expected = std::fs::read_to_string(&self.peak)
            .ok()
            .and_then(|text| text.trim().parse().ok());
        for released in make_room(
            &self.dir,
            expected,
            dartq_toolchain::proc::physical_memory(),
        ) {
            note(
                client,
                format!(
                    "dartq analyze: released the idle analyzer of {} to make room",
                    released.display()
                ),
            );
        }
        self.server = Some(Server::start(launch, &self.roots, self.budget)?);
        Ok(())
    }

    /// A fault the client falls back on; the marker spares the next
    /// clients a daemon start that would fail the same way.
    fn fail(&self, message: String) -> Answer {
        let _ = std::fs::write(&self.files.failed, &message);
        Answer::Fault(message)
    }
}

fn send(stream: &UnixStream, answer: &Answer) {
    let mut out = serde_json::to_vec(answer).expect("an answer serializes");
    out.push(b'\n');
    // A client that went away just misses its answer.
    let _ = (&*stream).write_all(&out);
}

fn note(stream: &UnixStream, text: String) {
    send(stream, &Answer::Note(text));
}

/// Releases idle peer daemons, least recently used first, while the live
/// servers plus `expected` (this server's last peak; unknown is the
/// largest live peer's) exceed [`MEMORY_SHARE`] of `physical`. A peer is
/// idle when its busy lock is free. Answers the released peers' roots.
fn make_room(own: &Path, expected: Option<u64>, physical: u64) -> Vec<PathBuf> {
    if physical == 0 {
        return Vec::new();
    }
    let peers: Vec<_> = crate::peers(own)
        .into_iter()
        .map(|(dir, pid, used)| (dir, pid, used, dartq_toolchain::proc::tree_rss_bytes(pid)))
        .collect();
    let expected = expected.unwrap_or_else(|| peers.iter().map(|peer| peer.3).max().unwrap_or(0));
    let usage: Vec<_> = peers.iter().map(|peer| (peer.2, peer.3)).collect();
    let limit = physical / MEMORY_SHARE.1 * MEMORY_SHARE.0;
    let mut released = Vec::new();
    release(&usage, expected, limit, |index| {
        let (dir, pid, ..) = &peers[index];
        let files = Files::of(dir);
        // Held across the signal: a request that reaches the peer meanwhile
        // blocks on the lock and dies with it; its client falls back to a
        // per-run analysis.
        let Some(_lock) = std::fs::File::open(&files.busy)
            .ok()
            .filter(|lock| lock.try_lock().is_ok())
        else {
            return false;
        };
        let Ok(pid) = libc::pid_t::try_from(*pid) else {
            return false;
        };
        // SAFETY: SIGTERM to a daemon pid read from its own state file and
        // checked alive by `peers`; its handler takes its process group,
        // the server included, down with it.
        unsafe { libc::kill(pid, libc::SIGTERM) };
        released.push(
            std::fs::read(&files.spec)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Spec>(&bytes).ok())
                .and_then(|spec| spec.roots.into_iter().next())
                .unwrap_or_else(|| dir.clone()),
        );
        true
    });
    released
}

/// The release policy over `peers` (last use, RSS): least recently used
/// first, while their RSS plus `expected` exceeds `limit`. `try_release`
/// answers whether the peer went (a busy one stays and still counts).
fn release(
    peers: &[(std::time::SystemTime, u64)],
    expected: u64,
    limit: u64,
    mut try_release: impl FnMut(usize) -> bool,
) {
    let mut held = peers
        .iter()
        .map(|peer| peer.1)
        .fold(expected, u64::saturating_add);
    let mut order: Vec<usize> = (0..peers.len()).collect();
    order.sort_by_key(|&index| peers[index].0);
    for index in order {
        if held <= limit {
            break;
        }
        if try_release(index) {
            held -= peers[index].1;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

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

    /// Within the share nothing is released; over it, the least recently
    /// used idle peers go until the rest fits, and a busy peer is skipped.
    #[test]
    fn peers_are_released_least_recently_used_first_only_over_the_share() {
        const GIB: u64 = 1 << 30;
        let at = |secs| UNIX_EPOCH + Duration::from_secs(secs);
        let peers = [(at(30), 6 * GIB), (at(10), 8 * GIB), (at(20), 2 * GIB)];
        let run = |expected, busy: Option<usize>| {
            let mut tried = Vec::new();
            let mut gone = Vec::new();
            release(&peers, expected, 18 * GIB, |index| {
                tried.push(index);
                let ok = busy != Some(index);
                if ok {
                    gone.push(index);
                }
                ok
            });
            (tried, gone)
        };
        assert_eq!(run(2 * GIB, None), (vec![], vec![]), "16 + 2 fits 18");
        assert_eq!(run(3 * GIB, None), (vec![1], vec![1]), "the oldest frees 8");
        assert_eq!(
            run(3 * GIB, Some(1)),
            (vec![1, 2], vec![2]),
            "busy is skipped"
        );
        assert_eq!(
            run(20 * GIB, None),
            (vec![1, 2, 0], vec![1, 2, 0]),
            "an expected peak past the share releases every idle peer"
        );
    }
}
