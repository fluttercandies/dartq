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

/// The byte length beyond which a worktree-local unix-domain socket
/// path relocates to the per-user short directory (#2592): `sun_path`
/// caps at 104 bytes on the BSDs/macOS and 108 on Linux; 92 leaves
/// margin for every host while keeping ordinary checkouts worktree-
/// local. Past it the old behavior was a silent bind failure — the
/// serve process died with stderr nulled, and `session start` retried
/// into "never listened" forever.
const SOCKET_PATH_LIMIT: usize = 92;

/// The session artifacts of one worktree: `.dart_tool/dartq/`.
#[derive(Clone, Debug)]
pub struct Layout {
    /// The session directory: `<root>/.dart_tool/dartq/`.
    pub dir: PathBuf,
    /// The daemon-protocol socket: worktree-local when its path fits
    /// the kernel's `sun_path`, else the per-user relocation (see
    /// [`Layout::new`]).
    pub socket: PathBuf,
    /// The liveness state (pid, heartbeat, triple).
    pub state: PathBuf,
    /// The cold-start election lock.
    pub elect: PathBuf,
}

impl Layout {
    /// The layout under `root`. Computed, not probed — except the
    /// relocation branch (#2592), which canonicalizes the root once so
    /// every spelling of one worktree lands on the same relocated
    /// socket. Ordinary (short-path) checkouts touch nothing.
    #[must_use]
    pub fn new(root: &Path) -> Self {
        let dir = root.join(".dart_tool").join("dartq");
        Self {
            socket: socket_path(&dir, root),
            state: dir.join("state.json"),
            elect: dir.join("elect.lock"),
            dir,
        }
    }

    /// Creates the session directory; `Err` carries the degradation
    /// reason (a read-only checkout or an unwritable `.dart_tool`).
    /// A relocated socket also gets its per-user directory and a
    /// pointer file in the session directory naming the socket's real
    /// path (discovery stays deterministic; the pointer is for humans
    /// and tools reading the worktree).
    pub fn create(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|error| format!("cannot create {}: {error}", self.dir.display()))?;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.elect)
            .map_err(|error| format!("cannot write {}: {error}", self.elect.display()))?;
        let pointer = self.dir.join("session.sock.point");
        if self.socket.parent() == Some(self.dir.as_path()) {
            // Worktree-local: no relocation, no pointer.
            let _ = std::fs::remove_file(&pointer);
            return Ok(());
        }
        let parent = self
            .socket
            .parent()
            .ok_or_else(|| format!("socket path has no directory: {}", self.socket.display()))?;
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        std::fs::write(&pointer, self.socket.as_os_str().as_encoded_bytes())
            .map_err(|error| format!("cannot write {}: {error}", pointer.display()))?;
        Ok(())
    }
}

/// The worktree-local socket path when it fits the kernel's `sun_path`
/// budget, else `<cache>/dartq/sessions/sockets/<hash>.sock`. The
/// fit-or-relocate decision is keyed on the *canonical* root's path —
/// two spellings of one worktree (one through a symlink) must make the
/// same decision, or they would split into two sockets (and two
/// engines). The relocation hash is the canonical root too, so every
/// spelling agrees on the destination as well.
fn socket_path(dir: &Path, root: &Path) -> PathBuf {
    let canonical = stable_canonical(root);
    let canonical_local = canonical
        .join(".dart_tool")
        .join("dartq")
        .join("session.sock");
    if canonical_local.as_os_str().len() <= SOCKET_PATH_LIMIT {
        return dir.join("session.sock");
    }
    let digest: [u8; 8] = blake3::hash(canonical.as_os_str().as_encoded_bytes()).as_bytes()
        [..8]
        .try_into()
        .expect("8 bytes");
    cache_root().join("sockets").join(format!("{}.sock", hex::encode(digest)))
}

/// The canonical form of `root`, stable whether or not the path exists
/// yet: `fs::canonicalize` answers `None` for a missing tail, and the
/// naive fallback (the path as given) hashes differently from the
/// post-creation canonical form — two spellings of one worktree would
/// then land on two relocated sockets (#2592's split-brain). The
/// deepest *existing* ancestor is canonicalized and the missing tail
/// re-attached, so before and after `mkdir` agree.
fn stable_canonical(root: &Path) -> PathBuf {
    if let Ok(full) = std::fs::canonicalize(root) {
        return full;
    }
    let mut prefix = root.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    while let Some(name) = prefix.file_name() {
        let parent = prefix.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
        if parent.as_os_str().is_empty() {
            break;
        }
        match std::fs::canonicalize(&parent) {
            Ok(real) => {
                let mut path = real;
                path.push(name);
                for part in tail.iter().rev() {
                    path.push(part);
                }
                return path;
            }
            Err(_) => {
                tail.push(name.to_owned());
                prefix = parent;
            }
        }
    }
    root.to_path_buf()
}

/// The per-user session cache root, shared by the registry and the
/// relocated sockets: `$XDG_CACHE_HOME|~/.cache` + `dartq/sessions`.
fn cache_root() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|home| home.join(".cache")))
        .unwrap_or_default()
        .join("dartq")
        .join("sessions")
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
        Self { dir: cache_root() }
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

    /// #2592: a socket path that fits the kernel's `sun_path` stays
    /// worktree-local with no pointer; a too-long one relocates to the
    /// per-user short directory deterministically — both spellings of
    /// one worktree (through a symlink) land on the same socket, the
    /// same property the worktree-local path had — and `create` leaves
    /// the pointer naming the real path.
    #[test]
    fn long_socket_paths_relocate_deterministically() {
        // Short: worktree-local, no pointer.
        let short = temp("short-sock");
        let layout = Layout::new(&short);
        assert_eq!(layout.socket, layout.dir.join("session.sock"));
        layout.create().expect("create");
        assert!(!layout.dir.join("session.sock.point").exists());

        // Long: relocates into the per-user sockets directory, fits the
        // budget, and both spellings agree.
        let segment = "d".repeat(64);
        let long = PathBuf::from("/tmp").join(format!("dqs-longsock-{}-{segment}", std::process::id()));
        let _ = std::fs::remove_dir_all(&long);
        std::fs::create_dir_all(&long).expect("dir");
        assert!(
            long.join(".dart_tool/dartq/session.sock").as_os_str().len() > 104,
            "the fixture must exceed sun_path"
        );
        let layout = Layout::new(&long);
        assert_ne!(layout.socket, layout.dir.join("session.sock"));
        assert!(
            layout.socket.as_os_str().len() <= 104,
            "the relocated path fits sun_path: {}",
            layout.socket.display()
        );
        assert!(
            layout.socket.components().any(|part| part.as_os_str() == "sockets"),
            "the relocation lands under the per-user sockets directory: {}",
            layout.socket.display()
        );
        let through_link = PathBuf::from("/tmp").join(format!("dqs-longsock-link-{}", std::process::id()));
        let _ = std::fs::remove_file(&through_link);
        std::os::unix::fs::symlink(&long, &through_link).expect("symlink");
        assert_eq!(
            Layout::new(&through_link).socket,
            layout.socket,
            "both spellings of one worktree share the relocated socket"
        );
        // The stability pin: a layout computed over a MISSING tail must
        // hash the same canonical form as after the tail exists —
        // `ensure` builds layouts pre-`create`, and a spelling drift
        // there split one worktree into two sockets (#2592).
        let deep = long.join("deep");
        let before = Layout::new(&deep).socket;
        std::fs::create_dir_all(&deep).expect("deep dir");
        assert_eq!(
            before,
            Layout::new(&deep).socket,
            "pre-creation and post-creation spellings agree"
        );
        let _ = std::fs::remove_dir_all(&deep);
        layout.create().expect("create");
        let pointer = std::fs::read_to_string(layout.dir.join("session.sock.point")).expect("pointer");
        assert_eq!(pointer.as_bytes(), layout.socket.as_os_str().as_encoded_bytes());
        assert!(layout.socket.exists() || layout.socket.parent().is_some_and(|dir| dir.exists()), "the sockets directory exists");

        let _ = std::fs::remove_file(&through_link);
        let _ = std::fs::remove_file(&layout.socket);
        let _ = std::fs::remove_dir_all(&long);
        let _ = std::fs::remove_dir_all(&short);
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

    /// Unreadable identity inputs key as facts, not failures: an SDK root
    /// without a `version` file keys the empty string (stably — the SDK
    /// identity is "no version file", shared by every such SDK), no
    /// `.fvmrc` keys `None`, and the running binary always hashes (the
    /// test binary is readable).
    #[test]
    fn unreadable_identity_inputs_key_as_empty_facts() {
        let root = temp("bare-sdk");
        let sdk = temp("sdk-noversion");
        let triple = SessionTriple::compute(&root, &sdk);
        assert_eq!(triple.sdk_version, "", "a missing version file keys empty");
        assert_eq!(triple.fvmrc, None, "no .fvmrc keys None");
        assert!(!triple.binary_hash.is_empty(), "the binary always hashes");
        assert_eq!(
            triple.identity(),
            SessionTriple::compute(&root, &sdk).identity(),
            "the facts are stable"
        );
        // An empty .fvmrc is a present fact — a different identity from
        // an absent one.
        write(&root.join(".fvmrc"), "");
        assert_ne!(triple.identity(), SessionTriple::compute(&root, &sdk).identity());
    }

    /// The root leg of the triple is the realpath: two spellings of one
    /// worktree (one through a symlink) are one session, never two.
    #[test]
    fn triple_root_resolves_symlinks() {
        let real = temp("real-root");
        let link = PathBuf::from("/tmp").join(format!("dqs-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&link);
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        let sdk = temp("sdk-link");
        let through_link = SessionTriple::compute(&link, &sdk);
        assert_eq!(through_link.root, real.canonicalize().expect("canonical"));
        assert_eq!(
            through_link.identity(),
            SessionTriple::compute(&real, &sdk).identity(),
            "both spellings are one session"
        );
        let _ = std::fs::remove_file(&link);
    }

    /// The state file round-trips through JSON; a malformed or absent
    /// state is `None` — stale, the takeover path, never a failure.
    #[test]
    fn state_round_trips_and_malformed_state_is_stale() {
        let root = temp("state");
        let layout = Layout::new(&root);
        layout.create().expect("layout");
        assert!(SessionState::read(&layout).is_none(), "absent state");
        let state = SessionState {
            pid: std::process::id(),
            heartbeat_ms: 123_456,
            triple: SessionTriple::compute(&root, &root),
        };
        state.write(&layout).expect("write");
        assert_eq!(SessionState::read(&layout).as_ref(), Some(&state));
        // No temp file residue from the atomic write.
        assert!(!layout.dir.join(".state.json.tmp").exists());
        write(&layout.state, "{ not json");
        assert!(SessionState::read(&layout).is_none(), "malformed state is stale");
        // A heartbeat is a positive millisecond timestamp.
        assert!(SessionState::now_ms() > 1_000_000_000_000);
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
        // A *file* where the session directory must be: `.dart_tool` as a
        // file (a read-only checkout's reliable, portable stand-in).
        let parent = temp("ro");
        std::fs::create_dir_all(&parent).expect("mkdir");
        write(&parent.join(".dart_tool"), "a file");
        match ensure(&parent, &parent, &|_, _| Ok(())) {
            Ensured::Standalone(reason) => assert!(reason.contains("cannot"), "{reason}"),
            Ensured::Attached(_) => panic!("no session can live in this tree"),
        }
    }

    /// The loser of a live election: another starter holds the lock and is
    /// bringing the server up; the loser waits for the socket and attaches
    /// to the winner's state — it never spawns a second server.
    #[test]
    fn election_loser_waits_for_the_winner_and_attaches() {
        let root = temp("loser");
        let layout = Layout::new(&root);
        layout.create().expect("layout");
        let guard = match flock_exclusive(&layout.elect) {
            Ok(guard) => guard,
            Err(_) => panic!("the test must hold the election lock"),
        };
        let winner_dir = root.clone();
        let winner = std::thread::spawn(move || {
            // The winner's bring-up takes a moment; the loser must poll
            // the socket, not fail fast.
            std::thread::sleep(Duration::from_millis(300));
            let triple = SessionTriple::compute(&winner_dir, &winner_dir);
            let layout = Layout::new(&winner_dir);
            bring_up(&layout, &triple);
        });
        let spawns = std::cell::Cell::new(0);
        match ensure(&root, &root, &|_, _| {
            spawns.set(spawns.get() + 1);
            Ok(())
        }) {
            Ensured::Attached(state) => {
                assert_eq!(state.triple, SessionTriple::compute(&root, &root));
                assert_eq!(state.pid, std::process::id());
            }
            Ensured::Standalone(reason) => panic!("the loser attaches: {reason}"),
        }
        assert_eq!(spawns.get(), 0, "the loser never spawns a rival server");
        winner.join().expect("winner thread");
        drop(guard);
    }

    /// A live pid under a *foreign* triple (an upgraded binary, an FVM
    /// switch) and a live pid over a dead socket are both stale sessions:
    /// taken over — socket and state replaced, the new server spawned
    /// exactly once.
    #[test]
    fn ensure_takes_over_foreign_triples_and_dead_sockets() {
        // A live pid, but a triple from another binary generation.
        let root = temp("foreign");
        let layout = Layout::new(&root);
        layout.create().expect("layout");
        let current = SessionTriple::compute(&root, &root);
        let foreign = SessionTriple { binary_hash: "another-binary".to_owned(), ..current.clone() };
        // A socket answers (a live server), but the state names a foreign
        // triple: an upgrade left the old server behind.
        bring_up(&layout, &current);
        SessionState { pid: std::process::id(), heartbeat_ms: 0, triple: foreign }
            .write(&layout)
            .expect("state");
        let spawns = std::cell::Cell::new(0);
        match ensure(&root, &root, &|spawn_layout, spawn_triple| {
            spawns.set(spawns.get() + 1);
            bring_up(spawn_layout, spawn_triple);
            Ok(())
        }) {
            Ensured::Attached(state) => assert_eq!(state.triple, current),
            Ensured::Standalone(reason) => panic!("takeover: {reason}"),
        }
        assert_eq!(spawns.get(), 1, "the foreign session was replaced once");

        // A live pid with a matching triple but a socket nobody answers.
        let root = temp("deadsock");
        let layout = Layout::new(&root);
        layout.create().expect("layout");
        let triple = SessionTriple::compute(&root, &root);
        SessionState { pid: std::process::id(), heartbeat_ms: 0, triple: triple.clone() }
            .write(&layout)
            .expect("state");
        write(&layout.socket, ""); // a socket file, no listener
        match ensure(&root, &root, &|spawn_layout, spawn_triple| {
            bring_up(spawn_layout, spawn_triple);
            Ok(())
        }) {
            Ensured::Attached(state) => assert_eq!(state.triple, triple),
            Ensured::Standalone(reason) => panic!("takeover: {reason}"),
        }
    }

    /// A spawn that never listens degrades to standalone with the reason
    /// (the caller runs in-process); a spawn error carries the error.
    #[test]
    fn ensure_degrades_when_the_spawn_never_listens_or_fails() {
        let root = temp("nolisten");
        let layout = Layout::new(&root);
        layout.create().expect("layout");
        match ensure(&root, &root, &|_, _| Ok(())) {
            Ensured::Standalone(reason) => {
                assert!(reason.contains("never listened"), "{reason}")
            }
            Ensured::Attached(_) => panic!("nothing listened"),
        }
        let root = temp("spawnfail");
        Layout::new(&root).create().expect("layout");
        match ensure(&root, &root, &|_, _| Err(io::Error::other("no exec"))) {
            Ensured::Standalone(reason) => {
                assert!(reason.contains("spawning") && reason.contains("no exec"), "{reason}")
            }
            Ensured::Attached(_) => panic!("the spawn failed"),
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

    /// The handle namespaces the log: `session`, `test:<id>`, `debug:<id>`
    /// events coexist; the read filters on the exact handle string.
    #[test]
    fn event_log_namespaces_by_handle() {
        let root = temp("handles");
        let layout = Layout::new(&root);
        layout.create().expect("layout");
        let log = EventLog::new(&layout);
        for (handle, event) in [("session", "serving"), ("test:1", "started"), ("test:2", "started"), ("debug:1", "launched"), ("session", "client_attached")] {
            log.append(handle, event, serde_json::json!({})).expect("append");
        }
        let of = |handle: &str| {
            log.read(Some(handle), 0, 10)
                .into_iter()
                .map(|(seq, value)| (seq, value["event"].as_str().unwrap_or_default().to_owned()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            of("session"),
            [(1, "serving".to_owned()), (5, "client_attached".to_owned())]
        );
        assert_eq!(of("test:1"), [(2, "started".to_owned())]);
        assert_eq!(of("test:2"), [(3, "started".to_owned())]);
        assert_eq!(of("debug:1"), [(4, "launched".to_owned())]);
        // An exact-string filter: no prefix or substring accidents.
        assert!(of("test:").is_empty() && of("debug").is_empty());
    }

    /// Past [`EVENT_LOG_MAX_BYTES`] the log rewrites keeping the newest
    /// half: sequences stay monotonic, a cursor inside the kept half
    /// survives, and the dropped half is gone.
    #[test]
    fn event_log_trims_to_the_newest_half() {
        let root = temp("trim");
        let layout = Layout::new(&root);
        layout.create().expect("layout");
        let log = EventLog::new(&layout);
        let blob = "x".repeat(60_000);
        let mut last = 0;
        for _ in 0..20 {
            last = log.append("session", "noise", serde_json::json!({"blob": &blob})).expect("append");
        }
        assert_eq!(log.head_seq(), last, "sequence numbers survive the rewrite");
        let size = std::fs::metadata(&layout.dir.join("events.jsonl")).expect("meta").len();
        assert!(size <= EVENT_LOG_MAX_BYTES, "the trim brought the log under the cap ({size})");
        let kept = log.read(None, 0, usize::MAX);
        let first = kept.first().expect("the newest half survives").0;
        assert!(first > 1, "the older half was dropped (first kept seq {first})");
        assert_eq!(kept.last().expect("tail").0, last);
        assert_eq!(kept.len() as u64, last - first + 1, "a contiguous newest half");
        // A cursor inside the kept half still paginates from itself.
        assert_eq!(log.read(None, first, usize::MAX).len(), kept.len() - 1);
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

    /// The derivation branches of the trace walk: garbage lines and
    /// non-analysis events never count, `file://` spellings derive the
    /// package root like plain paths, the toolchain's own `lib/` carries
    /// no package signal (the last derivable package stays), and a
    /// URI-less event updates nothing but the scan count.
    #[test]
    fn progress_walks_only_analysis_events() {
        let dir = temp("branches");
        let trace = dir.join("trace.jsonl");
        std::fs::write(
            &trace,
            "this line is not json
             {\"event\":\"OverlayApplied\",\"uri\":\"package:nobody/overlay.dart\"}
             {\"event\":\"LibraryAnalyzed\",\"uri\":\"file:///w/p/lib/a.dart\",\"from_cache\":true}
             {\"event\":\"LibraryAnalyzed\",\"uri\":\"/sdk/lib/core/core.dart\"}
             {\"event\":\"LibraryAnalyzed\",\"uri\":\"package:x/y.dart\",\"from_cache\":true}
             {\"event\":\"LibraryAnalyzed\",\"from_cache\":false}
            ",
        )
        .expect("write");
        let progress = Progress::from_trace(&trace);
        assert_eq!(progress.files_scanned, 4, "every LibraryAnalyzed event counts");
        assert_eq!(progress.from_cache, 2);
        assert_eq!(
            progress.current.as_deref(),
            Some("package:x/y.dart"),
            "the URI-less event updated neither current nor the package"
        );
        assert_eq!(
            progress.current_package.as_deref(),
            Some("x"),
            "the sdk's own lib carried no package signal; the last derivable one stays"
        );

        // The `sdk` filter is by path segment: a workspace package named
        // anything else under a lib/ still derives.
        std::fs::write(
            &trace,
            "{\"event\":\"LibraryAnalyzed\",\"uri\":\"/w/sdk/lib/a.dart\"}
             {\"event\":\"LibraryAnalyzed\",\"uri\":\"/w/my_sdk/lib/b.dart\"}
            ",
        )
        .expect("rewrite");
        let progress = Progress::from_trace(&trace);
        assert_eq!(progress.current_package.as_deref(), Some("my_sdk"), "only the exact segment `sdk` is filtered");
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

    /// The session cap: past [`SESSION_CAP`] live sessions the
    /// least-recently-heartbeat dies first, in heartbeat order, and its
    /// record leaves the registry; the survivors stay.
    #[test]
    fn registry_evicts_least_recently_heartbeat_past_the_cap() {
        let home = temp("cap");
        let registry = Registry::at(&home.join("sessions"));
        let record = |tag: &str, heartbeat: u128| {
            let root = temp(tag);
            let layout = Layout::new(&root);
            layout.create().expect("layout");
            let triple = SessionTriple { root: root.clone(), sdk_root: root.clone(), sdk_version: String::new(), fvmrc: None, binary_hash: "bin".to_owned() };
            let pid = bring_up_child(&layout, &triple);
            Record { id: triple.identity(), state: SessionState { pid, heartbeat_ms: heartbeat, triple } }
        };
        // Six live sessions; the oldest heartbeat is the eviction.
        let records: Vec<_> = (0..SESSION_CAP + 1)
            .map(|index| record(&format!("cap-{index}"), 1_000 + u128::try_from(index).expect("index") * 100))
            .collect();
        for record in &records {
            registry.upsert(record).expect("upsert");
        }
        let evicted = registry.evict_past_cap();
        assert_eq!(evicted, [records[0].state.triple.root.clone()], "the oldest heartbeat is evicted, in order");
        assert_eq!(registry.list().len(), SESSION_CAP, "the cap holds");
        // The evicted child died for real (reaped while polling — a zombie
        // still answers kill(pid, 0)).
        let pid = records[0].state.pid;
        let mut terminated = false;
        for _ in 0..50 {
            SESSION_CHILDREN.with(|children| {
                for child in children.borrow_mut().iter_mut() {
                    let _ = child.try_wait();
                }
            });
            if !pid_alive(pid) {
                terminated = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(terminated, "the evicted server was terminated");
        // Under the cap: nothing to evict.
        assert!(registry.evict_past_cap().is_empty());
        // Leave no sleeping children behind the test.
        for record in &records[1..] {
            terminate(record.state.pid);
        }
    }
}
