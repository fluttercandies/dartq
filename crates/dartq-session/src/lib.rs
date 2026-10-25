//! The resident session layer: one long-lived analysis server per
//! worktree, converged on by every face (CLI verbs, the events stream,
//! MCP tools) through the same identity.
//!
//! # Identity — the triple
//!
//! A session is keyed on [`SessionTriple`]: the resolution root's
//! realpath, the SDK identity (resolved root, its `version` file bytes,
//! and the worktree's `.fvmrc` when present), and the running dartq
//! binary's content hash. Two faces agree on one engine exactly when
//! their triples agree — an FVM switch or a dartq upgrade is a different
//! session, never a stale attach.
//!
//! # Layout and election
//!
//! The session's artifacts live in the worktree: `.dart_tool/dartq/`
//! holds `session.sock` (the daemon protocol over a Unix socket), and
//! `state.json` (pid, heartbeat, triple). `.dart_tool` is a
//! resolved-state location — writing session artifacts there never
//! defines the package boundary (the discovery layer's rule), and a
//! read-only checkout simply degrades: [`ensure`] answers
//! [`Degrade::Standalone`] and the caller runs in-process, exactly the
//! pre-session behavior.
//!
//! Two simultaneous cold starters elect through an exclusive `flock` on
//! `.dart_tool/dartq/elect.lock`: the winner takes over (a stale socket
//! whose pid is gone is unlinked and re-spawned) and brings the server
//! up; the loser waits for the socket and attaches. A live server with
//! a matching triple is attached, never duplicated.
//!
//! # Registry and lifecycle
//!
//! A per-user registry (`~/.cache/dartq/sessions/`, one JSON file per
//! session, keyed by the triple's hash) backs `dartq session status`,
//! grouped by checkout realpath. Lifecycle on the server side: the
//! heartbeat updates `state.json` while it serves; an idle reaper exits
//! after [`IDLE_TIMEOUT`] with no client; the starter enforces
//! [`SESSION_CAP`] live sessions per user by evicting the
//! least-recently-heartbeat first (an evicted worktree's next call is a
//! cold restart — its stamp caches under `.dart_tool/dartq/` keep that
//! restart cheap), and sweeps orphans from a previous binary on every
//! start and status (a different binary hash means a different protocol
//! generation: kill, never talk).
//!
//! # Windows
//!
//! The design is socket-transport-first: the Windows mapping is a named
//! pipe at the same layout path, `LockFile` for the election, and no
//! self-pipe tricks (the serve loop's accept-timeout tick replaces
//! them). The Unix socket and flock plumbing is `cfg(unix)`; on other
//! hosts [`ensure`] degrades to standalone with a pointer here — the
//! named-pipe transport lands with the Windows bring-up.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// How often the serve loop rewrites `state.json` (the liveness signal).
pub const HEARTBEAT: Duration = Duration::from_secs(5);
/// No connected client for this long: the server exits (the idle reaper).
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// A cold start must produce a listening socket within this window.
pub const START_TIMEOUT: Duration = Duration::from_secs(30);
/// Live sessions per user; the least-recently-heartbeat is evicted past it.
pub const SESSION_CAP: usize = 5;

// ---------------------------------------------------------------------------
// The triple
// ---------------------------------------------------------------------------

/// One session's identity: (resolution root realpath, SDK identity,
/// binary content hash).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTriple {
    /// The resolution root, symlink-resolved.
    pub root: PathBuf,
    /// The resolved SDK root.
    pub sdk_root: PathBuf,
    /// The SDK's `version` file bytes (identity of the toolchain).
    pub sdk_version: String,
    /// The worktree's `.fvmrc` content when present (an FVM switch is a
    /// different session).
    pub fvmrc: Option<String>,
    /// The running binary's blake3 hash (an upgrade is a different
    /// protocol generation).
    pub binary_hash: String,
}

impl SessionTriple {
    /// Computes the triple for `root` and `sdk_root`: the realpath of
    /// each, the SDK's `version` file, the worktree's `.fvmrc`, and the
    /// current executable's content hash. Files that cannot be read key
    /// as empty strings — a missing `version` file is a fact of the SDK,
    /// not a failure to identify it.
    #[must_use]
    pub fn compute(root: &Path, sdk_root: &Path) -> Self {
        Self {
            root: realpath(root),
            sdk_root: realpath(sdk_root),
            sdk_version: read_or_empty(&sdk_root.join("version")),
            fvmrc: std::fs::read_to_string(root.join(".fvmrc")).ok(),
            binary_hash: binary_hash().unwrap_or_default(),
        }
    }

    /// The registry key: the triple's blake3 digest, hex.
    #[must_use]
    pub fn identity(&self) -> String {
        let json = serde_json::to_string(self).unwrap_or_default();
        let digest: [u8; 8] = blake3::hash(json.as_bytes()).as_bytes()[..8].try_into().expect("8 bytes");
        hex::encode(digest)
    }
}

/// The realpath: canonicalized when possible, the input otherwise.
fn realpath(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn read_or_empty(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// The current executable's content hash — one read of the binary.
fn binary_hash() -> io::Result<String> {
    let bytes = std::fs::read(std::env::current_exe()?)?;
    let digest: [u8; 8] = blake3::hash(&bytes).as_bytes()[..8].try_into().expect("8 bytes");
    Ok(hex::encode(digest))
}

// ---------------------------------------------------------------------------
// Layout and state
// ---------------------------------------------------------------------------

/// The session artifacts of one worktree: `.dart_tool/dartq/`.
#[derive(Clone, Debug)]
pub struct Layout {
    /// The session directory: `<root>/.dart_tool/dartq/`.
    pub dir: PathBuf,
    /// The daemon-protocol socket.
    pub socket: PathBuf,
    /// The liveness state (pid, heartbeat, triple).
    pub state: PathBuf,
    /// The cold-start election lock.
    pub elect: PathBuf,
}

impl Layout {
    /// The layout under `root` — computed, not probed: constructing it
    /// touches nothing.
    #[must_use]
    pub fn new(root: &Path) -> Self {
        let dir = root.join(".dart_tool").join("dartq");
        Self {
            socket: dir.join("session.sock"),
            state: dir.join("state.json"),
            elect: dir.join("elect.lock"),
            dir,
        }
    }

    /// Creates the session directory; `Err` carries the degradation
    /// reason (a read-only checkout or an unwritable `.dart_tool`).
    pub fn create(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|error| format!("cannot create {}: {error}", self.dir.display()))?;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.elect)
            .map_err(|error| format!("cannot write {}: {error}", self.elect.display()))?;
        Ok(())
    }
}

/// The on-disk liveness state, written by the serve loop.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionState {
    pub pid: u32,
    /// The last heartbeat, UNIX milliseconds.
    pub heartbeat_ms: u128,
    pub triple: SessionTriple,
}

impl SessionState {
    /// Reads the layout's state file; `None` when absent or malformed
    /// (a malformed state is a stale state — takeover, not a failure).
    #[must_use]
    pub fn read(layout: &Layout) -> Option<Self> {
        serde_json::from_str(&std::fs::read_to_string(&layout.state).ok()?).ok()
    }

    /// Writes the state file atomically (temp + rename).
    ///
    /// # Errors
    ///
    /// I/O failure writing the state.
    pub fn write(&self, layout: &Layout) -> io::Result<()> {
        let temp = layout.dir.join(".state.json.tmp");
        std::fs::write(&temp, serde_json::to_vec(self).unwrap_or_default())?;
        std::fs::rename(&temp, &layout.state)
    }

    /// Now, in UNIX milliseconds.
    #[must_use]
    pub fn now_ms() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_millis())
            .unwrap_or_default()
    }
}

/// Whether `pid` is alive (`kill(pid, 0)`); a gone pid is `false`. On
/// non-unix hosts every pid reports alive (no signal substrate — the
/// socket connect is the liveness truth there).
#[must_use]
pub fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: `kill` with signal 0 performs the permission and
        // existence check only — no signal is delivered.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Whether a process should be terminated (the `stop` verb and the
/// eviction/sweep paths): SIGTERM, unix only — the Windows bring-up maps
/// it to `TerminateProcess`.
pub fn terminate(pid: u32) {
    #[cfg(unix)]
    {
        // SAFETY: signal delivery to a pid we own by registry contract.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
    }
}

// ---------------------------------------------------------------------------
// Election
// ---------------------------------------------------------------------------

/// The outcome of [`ensure`].
pub enum Ensured {
    /// A live session was found or brought up; the socket is ready for a
    /// client connection.
    Attached(Box<SessionState>),
    /// No resident session is possible here; the caller runs standalone
    /// (in-process, the pre-session behavior). The reason is navigation
    /// text.
    Standalone(String),
}

/// Brings up (or finds) the session for `root`/`sdk_root`.
///
/// The election: under an exclusive `flock` on the layout's lock file, a
/// live server with a matching triple is attached; a stale one (dead
/// pid, dead socket, foreign triple) is taken over (socket unlinked,
/// registry entry dropped) and `spawn` — the caller's bring-up action,
/// typically `dartq session serve` detached — runs; without the lock the
/// loser waits for the socket within [`START_TIMEOUT`].
///
/// `spawn` runs while the lock is held and returns once the serve
/// process has been *launched* (not necessarily listening).
///
/// On non-unix hosts or an unwritable layout the answer is
/// [`Ensured::Standalone`] — never an error.
pub fn ensure(
    root: &Path,
    sdk_root: &Path,
    spawn: &dyn Fn(&Layout, &SessionTriple) -> io::Result<()>,
) -> Ensured {
    if !cfg!(unix) {
        return Ensured::Standalone(
            "resident sessions need the Unix socket substrate; the Windows named-pipe \
             transport is designed (crate docs) but not wired — running standalone"
                .into(),
        );
    }
    let layout = Layout::new(root);
    if let Err(reason) = layout.create() {
        return Ensured::Standalone(reason);
    }
    let triple = SessionTriple::compute(root, sdk_root);

    #[cfg(unix)]
    {
        match flock_exclusive(&layout.elect) {
            // The winner: attach what is live, take over what is stale.
            Ok(guard) => {
                let outcome = under_election(&layout, &triple, spawn);
                drop(guard);
                outcome
            }
            // The loser: the winner is bringing the server up; wait for
            // the socket, then attach.
            Err(ElectError::Contended) => {
                if wait_for_socket(&layout, START_TIMEOUT) && let Some(state) = SessionState::read(&layout) {
                    return Ensured::Attached(Box::new(state));
                }
                Ensured::Standalone(
                    "another starter held the election but no session appeared in time — \
                     running standalone"
                        .into(),
                )
            }
            Err(ElectError::Io(error)) => Ensured::Standalone(format!("election failed: {error}")),
        }
    }
}

/// The winner's path, lock held: a live matching server is attached; a
/// stale or foreign one is taken over and `spawn` brings up the new one.
fn under_election(
    layout: &Layout,
    triple: &SessionTriple,
    spawn: &dyn Fn(&Layout, &SessionTriple) -> io::Result<()>,
) -> Ensured {
    if let Some(state) = SessionState::read(layout) {
        let live = state.triple == *triple
            && pid_alive(state.pid)
            && socket_open(layout);
        if live {
            return Ensured::Attached(Box::new(state));
        }
        // Take over: the socket file, the state, the registry entry.
        let _ = std::fs::remove_file(&layout.socket);
        let _ = std::fs::remove_file(&layout.state);
        Registry::open().remove(&state.triple.root);
    }
    match spawn(layout, triple) {
        Ok(()) => {
            if wait_for_socket(layout, START_TIMEOUT)
                && let Some(state) = SessionState::read(layout)
            {
                return Ensured::Attached(Box::new(state));
            }
            Ensured::Standalone("the serve process never listened — running standalone".into())
        }
        Err(error) => Ensured::Standalone(format!("spawning the serve process failed: {error}")),
    }
}

/// Whether the socket accepts a connection right now.
#[must_use]
pub fn socket_open(layout: &Layout) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::net::UnixStream::connect(&layout.socket).is_ok()
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Polls the socket until it accepts or `timeout` elapses.
fn wait_for_socket(layout: &Layout, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if socket_open(layout) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// The election error: `Contended` is the loser's signal.
enum ElectError {
    Contended,
    Io(io::Error),
}

/// An exclusive non-blocking `flock`; the guard releases on drop.
#[cfg(unix)]
fn flock_exclusive(path: &Path) -> Result<FlockGuard, ElectError> {
    use std::os::unix::io::AsRawFd;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(ElectError::Io)?;
    // SAFETY: `flock` on an owned fd; LOCK_EX | LOCK_NB never blocks.
    let locked = unsafe {
        libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0
    };
    if locked {
        Ok(FlockGuard { file: Some(file) })
    } else {
        Err(ElectError::Contended)
    }
}

/// Releases the flock on drop.
#[cfg(unix)]
struct FlockGuard {
    file: Option<std::fs::File>,
}

#[cfg(unix)]
impl Drop for FlockGuard {
    fn drop(&mut self) {
        // The fd's close releases the flock (flock(2)).
        self.file = None;
    }
}

// ---------------------------------------------------------------------------
// The per-user registry
// ---------------------------------------------------------------------------

/// The per-user session registry: `~/.cache/dartq/sessions/`, one JSON
/// file per live session, named by the triple's identity hash. No home
/// directory (nowhere to put it) is an empty registry — sessions still
/// run, just unlisted.
pub struct Registry {
    dir: PathBuf,
}

/// One registry record — the state file's facts plus the identity key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// The triple's identity hash (the file name).
    pub id: String,
    pub state: SessionState,
}

impl Registry {
    /// The per-user registry at `$XDG_CACHE_HOME|~/.cache/dartq/sessions`.
    #[must_use]
    pub fn open() -> Self {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::home_dir().map(|home| home.join(".cache")))
            .unwrap_or_default();
        Self { dir: base.join("dartq").join("sessions") }
    }

    /// The registry rooted at `dir` (tests).
    #[must_use]
    pub fn at(dir: &Path) -> Self {
        Self { dir: dir.to_path_buf() }
    }

    /// Upserts one session's record.
    ///
    /// # Errors
    ///
    /// I/O failure writing the record.
    pub fn upsert(&self, record: &Record) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(
            self.dir.join(format!("{}.json", record.id)),
            serde_json::to_vec(record).unwrap_or_default(),
        )
    }

    /// Removes every record rooted at `root` (takeover and shutdown
    /// both drop the stale entry).
    pub fn remove(&self, root: &Path) {
        for record in self.list() {
            if record.state.triple.root == root {
                let _ = std::fs::remove_file(self.dir.join(format!("{}.json", record.id)));
            }
        }
    }

    /// Every readable record, dead ones swept (their files removed): a
    /// gone pid or a dead socket leaves the registry on read.
    #[must_use]
    pub fn list(&self) -> Vec<Record> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let Ok(text) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let Ok(record) = serde_json::from_str::<Record>(&text) else {
                let _ = std::fs::remove_file(entry.path());
                continue;
            };
            let layout = Layout::new(&record.state.triple.root);
            if !pid_alive(record.state.pid) || !socket_open(&layout) {
                let _ = std::fs::remove_file(entry.path());
                continue;
            }
            out.push(record);
        }
        out.sort_by(|a, b| b.state.heartbeat_ms.cmp(&a.state.heartbeat_ms));
        out
    }

    /// Kills and drops every record whose binary hash is not `binary`:
    /// the upgrade orphan sweep (a different binary speaks a different
    /// protocol generation — kill, never talk). Returns the swept roots.
    #[must_use]
    pub fn sweep_upgrades(&self, binary: &str) -> Vec<PathBuf> {
        let mut swept = Vec::new();
        for record in self.list() {
            if record.state.triple.binary_hash != binary {
                terminate(record.state.pid);
                let _ = std::fs::remove_file(self.dir.join(format!("{}.json", record.id)));
                swept.push(record.state.triple.root.clone());
            }
        }
        swept
    }

    /// Enforces [`SESSION_CAP`]: kills the least-recently-heartbeat
    /// records past the cap and returns the evicted roots (each
    /// worktree's next call is a cold restart — its stamp caches keep
    /// that cheap).
    #[must_use]
    pub fn evict_past_cap(&self) -> Vec<PathBuf> {
        let live = self.list();
        let mut evicted = Vec::new();
        for record in live.iter().skip(SESSION_CAP) {
            terminate(record.state.pid);
            let _ = std::fs::remove_file(self.dir.join(format!("{}.json", record.id)));
            evicted.push(record.state.triple.root.clone());
        }
        evicted
    }
}

// ---------------------------------------------------------------------------
// The event log and cold-analysis progress
// ---------------------------------------------------------------------------

/// The session's append-only event log: `events.jsonl` under the layout,
/// one JSON object per line, sequence-stamped for cursor pagination.
/// Test-run, debug-session, and watch lifecycles that pass through the
/// session all land here — the one stream `dartq events` reads.
///
/// The ring discipline: past [`EVENT_LOG_MAX_BYTES`] the file rewrites
/// keeping the newest half (sequence numbers stay monotonic, cursors
/// survive).
pub struct EventLog {
    path: PathBuf,
}

/// The event log's size cap before the trim-to-half rewrite.
pub const EVENT_LOG_MAX_BYTES: u64 = 1024 * 1024;

impl EventLog {
    /// The log at `layout.dir/events.jsonl`.
    #[must_use]
    pub fn new(layout: &Layout) -> Self {
        Self { path: layout.dir.join("events.jsonl") }
    }

    /// Appends one event; `data` carries the event's own fields. Returns
    /// the assigned sequence number. The handle namespaces the event
    /// (`session`, `test:<id>`, `debug:<id>`) — the read filters on it.
    ///
    /// # Errors
    ///
    /// I/O failure writing the log (an unwritable log drops the event:
    /// callers treat logging as advisory).
    pub fn append(
        &self,
        handle: &str,
        event: &str,
        data: serde_json::Value,
    ) -> io::Result<u64> {
        std::fs::create_dir_all(self.path.parent().unwrap_or(Path::new(".")))?;
        let seq = self.head_seq() + 1;
        let line = serde_json::json!({
            "seq": seq,
            "ts_ms": SessionState::now_ms(),
            "handle": handle,
            "event": event,
            "data": data,
        });
        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&self.path)?;
        writeln!(file, "{line}")?;
        drop(file);
        self.trim();
        Ok(seq)
    }

    /// Reads events after `since` (exclusive), oldest first, at most
    /// `limit`; `handle` `None` reads every handle. Each item carries its
    /// sequence number — the next call's `since`.
    #[must_use]
    pub fn read(
        &self,
        handle: Option<&str>,
        since: u64,
        limit: usize,
    ) -> Vec<(u64, serde_json::Value)> {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        text.lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|value| value["seq"].as_u64().is_some_and(|seq| seq > since))
            .filter(|value| {
                handle.is_none_or(|wanted| value["handle"].as_str() == Some(wanted))
            })
            .take(limit)
            .map(|value| (value["seq"].as_u64().unwrap_or_default(), value))
            .collect()
    }

    /// The highest sequence written (0 on an absent log).
    #[must_use]
    pub fn head_seq(&self) -> u64 {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| {
                text.lines()
                    .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                    .filter_map(|value| value["seq"].as_u64())
                    .max()
            })
            .unwrap_or_default()
    }

    /// Past [`EVENT_LOG_MAX_BYTES`]: rewrite keeping the newest half. A
    /// failed trim is dropped — the log just grows until the next try.
    fn trim(&self) {
        let Ok(meta) = std::fs::metadata(&self.path) else {
            return;
        };
        if meta.len() <= EVENT_LOG_MAX_BYTES {
            return;
        }
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return;
        };
        let lines: Vec<&str> = text.lines().collect();
        let keep = &lines[lines.len() / 2..];
        let _ = std::fs::write(&self.path, keep.join("\n") + "\n");
    }
}

/// The cold-analysis progress snapshot, derived from the engine's trace
/// file (`LibraryAnalyzed` lines): how many files the sweep has analyzed
/// so far, how many came from cache, and the file it is on — the
/// anti-hung signal for a monster monorepo's first pass.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    /// `LibraryAnalyzed` lines so far.
    pub files_scanned: u64,
    /// Those served from the per-library cache.
    pub from_cache: u64,
    /// The most recently analyzed URI (file or `package:` spelling).
    pub current: Option<String>,
    /// The package the sweep is on: the `package:` URI's segment, or —
    /// the engine's file-path spellings — the directory carrying the
    /// file's package root (the component before `lib/`). The most
    /// recently derivable one; `None` until a package-shaped file
    /// appears.
    pub current_package: Option<String>,
}

/// The package a analyzed-file URI belongs to: the `package:` URI's
/// first segment, or — for the engine's file-path spellings — the
/// directory carrying `lib/` (the package root's name).
fn package_of(uri: &str) -> Option<&str> {
    if let Some(rest) = uri.strip_prefix("package:") {
        return rest.split('/').next();
    }
    let path = uri.strip_prefix("file://").unwrap_or(uri);
    let components: Vec<&str> = path.split('/').collect();
    components
        .iter()
        .position(|part| *part == "lib")
        .and_then(|index| index.checked_sub(1))
        .and_then(|index| components.get(index))
        .copied()
        .filter(|name| !name.is_empty())
        // `<sdk>/lib/...` is the toolchain's own library face (dart:core
        // and friends), not a workspace package — it carries no
        // "which package is it on" signal.
        .filter(|name| *name != "sdk")
}

impl Progress {
    /// Reads the trace file's tail state. A missing or empty trace is a
    /// zero-progress snapshot (nothing analyzed yet — or nothing running).
    #[must_use]
    pub fn from_trace(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let mut out = Self::default();
        for line in text.lines() {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if value["event"].as_str() != Some("LibraryAnalyzed") {
                continue;
            }
            out.files_scanned += 1;
            if value["from_cache"].as_bool() == Some(true) {
                out.from_cache += 1;
            }
            if let Some(uri) = value["uri"].as_str() {
                out.current = Some(uri.to_owned());
                // The package signal keeps the most recent *derivable*
                // spelling — the SDK's own libraries analyze as file
                // paths after the package's `package:` URIs, and the
                // agent asking "which package is it on" wants the last
                // package-sourced answer, not `None` because dart:core
                // came last.
                if let Some(package) = package_of(uri) {
                    out.current_package = Some(package.to_owned());
                }
            }
        }
        out
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    /// A short-rooted temp tree — unix socket paths cap at ~104 bytes,
    /// and the default temp base alone eats most of it.
    fn temp(label: &str) -> PathBuf {
        let dir = PathBuf::from("/tmp").join(format!("dqs-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    fn write(path: &Path, text: &str) {
        std::fs::write(path, text).expect("write");
    }

    /// A fake serve process: a real listening socket plus the state file
    /// naming a live pid (ours — `kill 0` on self is alive; no one ever
    /// terminates these).
    fn bring_up(layout: &Layout, triple: &SessionTriple) {
        let listener = UnixListener::bind(&layout.socket).expect("bind");
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stream.is_err() {
                    break;
                }
            }
        });
        SessionState {
            pid: std::process::id(),
            heartbeat_ms: SessionState::now_ms(),
            triple: triple.clone(),
        }
        .write(layout)
        .expect("state");
    }

    /// A terminable fake serve process: a real `sleep` child holds the
    /// pid, a real listener answers the socket — the sweep/evict paths
    /// can kill it for real.
    fn bring_up_child(layout: &Layout, triple: &SessionTriple) -> u32 {
        let listener = UnixListener::bind(&layout.socket).expect("bind");
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stream.is_err() {
                    break;
                }
            }
        });
        let child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("sleep child");
        let pid = child.id();
        SESSION_CHILDREN.with(|children| children.borrow_mut().push(child));
        SessionState { pid, heartbeat_ms: SessionState::now_ms(), triple: triple.clone() }
            .write(layout)
            .expect("state");
        pid
    }

    thread_local! {
        /// The spawned sleep children, reaped on drop.
        static SESSION_CHILDREN: std::cell::RefCell<Vec<std::process::Child>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    #[test]
    fn triple_identity_is_stable_and_separating() {
        let root = temp("triple");
        let sdk = temp("sdk");
        write(&sdk.join("version"), "3.5.0\n");
        let a = SessionTriple::compute(&root, &sdk);
        assert_eq!(a.identity(), SessionTriple::compute(&root, &sdk).identity());
        let other_sdk = temp("sdk2");
        write(&other_sdk.join("version"), "3.6.0\n");
        assert_ne!(a.identity(), SessionTriple::compute(&root, &other_sdk).identity());
        // An .fvmrc lands in the identity (an FVM switch is a new session).
        write(&root.join(".fvmrc"), "3.9.0\n");
        assert_ne!(a.identity(), SessionTriple::compute(&root, &sdk).identity());
    }

    #[test]
    fn ensure_attaches_a_live_session_without_respawning() {
        let root = temp("live");
        let layout = Layout::new(&root);
        layout.create().expect("layout");
        let triple = SessionTriple::compute(&root, &root);
        bring_up(&layout, &triple);
        let spawns = std::cell::Cell::new(0);
        match ensure(&root, &root, &|_, _| {
            spawns.set(spawns.get() + 1);
            Ok(())
        }) {
            Ensured::Attached(state) => {
                assert_eq!(state.triple, triple);
                assert_eq!(state.pid, std::process::id());
            }
            Ensured::Standalone(reason) => panic!("attached, not standalone: {reason}"),
        }
        assert_eq!(spawns.get(), 0, "a live session is attached, never duplicated");
    }

    #[test]
    fn ensure_takes_over_a_stale_socket() {
        let root = temp("stale");
        let layout = Layout::new(&root);
        layout.create().expect("layout");
        let triple = SessionTriple::compute(&root, &root);
        // A stale state: a dead pid plus a socket file nobody answers.
        SessionState { pid: u32::MAX - 1, heartbeat_ms: 0, triple: triple.clone() }
            .write(&layout)
            .expect("state");
        std::fs::write(&layout.socket, b"").expect("socket file");
        let spawns = std::cell::Cell::new(0);
        match ensure(&root, &root, &|spawn_layout, spawn_triple| {
            spawns.set(spawns.get() + 1);
            bring_up(spawn_layout, spawn_triple);
            Ok(())
        }) {
            Ensured::Attached(state) => assert_eq!(state.pid, std::process::id()),
            Ensured::Standalone(reason) => panic!("takeover: {reason}"),
        }
        assert_eq!(spawns.get(), 1, "the stale session was replaced exactly once");
    }

    #[test]
    fn ensure_degrades_on_an_unwritable_tree() {
        // A *file* where the session directory must be: `.dart_tool` as
        // a file (a read-only checkout's reliable, portable stand-in).
        let parent = temp("ro");
        std::fs::create_dir_all(&parent).expect("mkdir");
        write(&parent.join(".dart_tool"), "a file");
        match ensure(&parent, &parent, &|_, _| Ok(())) {
            Ensured::Standalone(reason) => assert!(reason.contains("cannot"), "{reason}"),
            Ensured::Attached(_) => panic!("no session can live in this tree"),
        }
    }

    #[test]
    fn event_log_appends_reads_and_paginates() {
        let root = temp("log");
        let layout = Layout::new(&root);
        layout.create().expect("layout");
        let log = EventLog::new(&layout);
        assert_eq!(log.head_seq(), 0, "absent log");
        let first = log.append("session", "serving", serde_json::json!({"pid": 1})).expect("append");
        let second = log.append("test:1", "started", serde_json::json!({})).expect("append");
        assert_eq!((first, second), (1, 2), "monotonic sequences");

        // Cursor pagination: everything, then past a cursor, then a
        // handle filter, then a limit.
        let all = log.read(None, 0, 10);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].0, first);
        let tail = log.read(None, first, 10);
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].0, second);
        assert_eq!(log.read(Some("test:1"), 0, 10).len(), 1);
        assert_eq!(log.read(None, 0, 1).len(), 1);
        assert!(log.read(None, second, 10).is_empty(), "nothing past the head");
    }

    #[test]
    fn progress_reads_the_engine_trace() {
        let dir = temp("trace");
        let trace = dir.join("trace.jsonl");
        std::fs::write(
            &trace,
            "{\"event\":\"OverlayApplied\",\"changed\":[]}
             {\"event\":\"LibraryAnalyzed\",\"uri\":\"/w/lib/a.dart\",\"from_cache\":false}
             {\"event\":\"LibraryAnalyzed\",\"uri\":\"package:fx/b.dart\",\"from_cache\":true}
",
        )
        .expect("write");
        let progress = Progress::from_trace(&trace);
        assert_eq!(progress.files_scanned, 2);
        assert_eq!(progress.from_cache, 1);
        assert_eq!(progress.current.as_deref(), Some("package:fx/b.dart"));
        assert_eq!(progress.current_package.as_deref(), Some("fx"));

        // The engine's file-path spellings derive the package root's
        // name (the component before `lib/`); a lib-less path keeps the
        // last known package.
        std::fs::write(
            &trace,
            "{\"event\":\"LibraryAnalyzed\",\"uri\":\"/w/pkgs/a/lib/src/x.dart\",\"from_cache\":false}
             {\"event\":\"LibraryAnalyzed\",\"uri\":\"/w/notes.txt.bak\",\"from_cache\":false}
",
        )
        .expect("rewrite");
        let progress = Progress::from_trace(&trace);
        assert_eq!(progress.current_package.as_deref(), Some("a"), "path-derived, kept");
        // Absent trace: zero progress, not an error.
        assert_eq!(Progress::from_trace(&dir.join("gone.jsonl")), Progress::default());
    }

    #[test]
    fn registry_lists_sweeps_and_evicts() {
        let home = temp("registry");
        let registry = Registry::at(&home.join("sessions"));
        let record = |tag: &str, heartbeat: u128, binary: &str| {
            let root = temp(tag);
            let layout = Layout::new(&root);
            layout.create().expect("layout");
            let triple = SessionTriple { root: root.clone(), sdk_root: root.clone(), sdk_version: String::new(), fvmrc: None, binary_hash: binary.to_owned() };
            let pid = bring_up_child(&layout, &triple);
            Record {
                id: triple.identity(),
                state: SessionState { pid, heartbeat_ms: heartbeat, triple },
            }
        };
        // Two live sessions; the older heartbeat is the LRU.
        let a = record("reg-a", 1_000, "bin1");
        let b = record("reg-b", 2_000, "bin1");
        registry.upsert(&a).expect("upsert");
        registry.upsert(&b).expect("upsert");
        let live = registry.list();
        assert_eq!(live.len(), 2, "both live");
        assert_eq!(live[0].state.heartbeat_ms, 2_000, "sorted recent-heartbeat first");

        // A dead-pid record sweeps on read (no live socket to answer).
        let dead_root = temp("reg-dead");
        let dead = Record {
            id: "dead".to_owned(),
            state: SessionState { pid: u32::MAX - 1, heartbeat_ms: 3_000, triple: SessionTriple { root: dead_root, sdk_root: PathBuf::new(), sdk_version: String::new(), fvmrc: None, binary_hash: "bin1".to_owned() } },
        };
        registry.upsert(&dead).expect("upsert");
        assert_eq!(registry.list().len(), 2, "the dead record swept");

        // The upgrade sweep kills foreign-binary sessions — the sleep
        // child dies for real.
        let old = record("reg-old", 500, "bin-old");
        registry.upsert(&old).expect("upsert");
        let swept = registry.sweep_upgrades("bin1");
        assert_eq!(swept.len(), 1, "the foreign-binary record swept");
        // Reap the child while polling: an un-reaped child stays a
        // zombie, and a zombie still answers `kill(pid, 0)`.
        let mut terminated = false;
        for _ in 0..50 {
            SESSION_CHILDREN.with(|children| {
                for child in children.borrow_mut().iter_mut() {
                    let _ = child.try_wait();
                }
            });
            if !pid_alive(old.state.pid) {
                terminated = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(terminated, "the orphan was terminated");
        assert_eq!(registry.list().len(), 2);

        // remove() drops one root's records.
        registry.remove(&a.state.triple.root);
        assert_eq!(registry.list().len(), 1);
    }
}
