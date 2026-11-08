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

/// One daemon answer line. [`Answer::Note`] lines (gate waits, released
/// peers) precede the one final line.
#[derive(Serialize, Deserialize)]
pub(crate) enum Answer {
    Report(Report),
    Fault(String),
    Budget(String),
    Note(String),
}

/// How long a server start waits for host memory before the request fails
/// (with the holders named, and no per-run retry: a per-run analysis
/// would wait for the same memory).
const MEMORY_WAIT: Duration = Duration::from_secs(30 * 60);

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
            host.stop();
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
    host.stop();
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
    server: Option<Server>,
    snapshot: Snapshot,
    files: Files,
    /// The socket this daemon bound.
    socket: Inode,
    /// The server's job slot and memory lease, held while the server
    /// lives.
    lease: Option<dartq_toolchain::ledger::Lease>,
    max_percent: u8,
    max_concurrent: u32,
    /// The ledger's peak key of this server: its cache directory, one per
    /// worktree.
    peak_key: String,
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
            server: None,
            snapshot: Snapshot::default(),
            files: Files::of(dir),
            socket,
            lease: None,
            max_percent: spec.max_percent,
            max_concurrent: spec.max_concurrent,
            peak_key: cache.display().to_string(),
        }
    }

    /// Shuts the server down and returns its lease.
    fn stop(&mut self) {
        if let Some(server) = self.server.take() {
            server.shutdown();
        }
        self.lease = None;
    }

    /// Forgets a server that already died (the budget killed it, or it
    /// faulted) and returns its lease.
    fn forget(&mut self) {
        self.server = None;
        self.lease = None;
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
        if let Some(lease) = &self.lease {
            lease.touch();
        }
        // The lease's mtime is the last answer: the ledger releases the
        // least recently used resident first. A daemon whose socket path
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
        if diff.rebuild {
            self.stop();
        }
        let outcome = match self.server.as_mut() {
            Some(server) if !diff.changed.is_empty() => server.changed(&diff.changed),
            Some(server) => server.wait_idle(),
            None => Err(Fault::Server("no server yet".into())),
        };
        let outcome = match outcome {
            Ok(()) => Ok(()),
            Err(Fault::Budget { budget, measured }) => {
                self.forget();
                return Answer::Budget(Fault::Budget { budget, measured }.to_string());
            }
            Err(_) => {
                // Restart once: a fresh server reads the tree as it is now.
                self.forget();
                self.start(&launch, client)
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
                    Some(lease) => lease.record_peak(&self.peak_key, peak),
                    None => dartq_toolchain::ledger::record_peak(
                        &self.peak_key,
                        dartq_toolchain::ledger::Face::AnalysisServer,
                        peak,
                    ),
                }
                Answer::Report(server.report())
            }
            Err(fault @ Fault::Budget { .. }) => Answer::Budget(fault.to_string()),
            Err(Fault::Memory(message)) => Answer::Budget(message),
            Err(Fault::Server(message)) => self.fail(message),
        }
    }

    /// Starts the server once the job gate admits it.
    fn start(&mut self, launch: &Launch, client: &UnixStream) -> Result<(), Fault> {
        let lease = self.lease_slot(client)?;
        let server = Server::start(launch, &self.roots, self.budget)?;
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
        let (bytes, sized) = ledger::sizing(&self.peak_key, Face::AnalysisServer);
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

#[cfg(test)]
mod tests {
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
}
