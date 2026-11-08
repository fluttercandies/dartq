//! A resident official analysis server behind `dartq analyze`.
//!
//! `dart analyze` starts an analysis server, analyzes everything, prints,
//! and shuts the server down: every run pays the server's start and the
//! whole analysis. This crate keeps one server per analysis key alive in a
//! small daemon, so a repeated `dartq analyze` costs one stat walk when
//! nothing changed and one incremental analysis when something did. The
//! result is the report `dart analyze` itself would print: the same server
//! protocol, the same request sequence, the same filter, order and exit
//! tiers ([`report`]).
//!
//! * [`support`] gates the SDKs the resident path serves;
//! * [`snapshot`] is the request-time freshness check;
//! * [`server`] drives one server child;
//! * [`client`] is the `dartq analyze` side: find or start the daemon,
//!   ask, and fall back to a per-run `dart analyze` on any failure;
//! * [`daemon`] is the serving side.
//!
//! One daemon serves one key: the SDK (root and version), the dartq binary,
//! the analysis roots and the package config override. Plugins are the
//! server's own decision, read from the options files as `dart analyze`
//! reads them.
//! Clients of one key queue on the daemon's single server, the server's
//! stdin has one writer, and the daemon exits after its idle timeout.

pub mod client;
#[cfg(unix)]
pub mod daemon;
pub mod report;
pub mod server;
pub mod snapshot;
pub mod support;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use client::{Outcome, Request, analyze};
pub use report::{Report, Row};
pub use support::{Sdk, Support, Version, support};

/// The hidden `dartq analyze` flag that runs a daemon over the spec in the
/// directory it names.
pub const SERVE_FLAG: &str = "--resident-serve";

/// What a daemon serves, written by the client that starts it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spec {
    pub sdk_root: PathBuf,
    pub sdk_version: String,
    pub roots: Vec<PathBuf>,
    pub packages: Option<PathBuf>,
    /// The server tree's RSS budget in bytes; `0` is unbounded.
    pub budget: u64,
    pub idle_secs: u64,
    /// The run queue root (`dartq analyze`'s machine-wide queue): server
    /// starts take a ticket there. `None` starts unqueued.
    pub queue: Option<PathBuf>,
}

impl Spec {
    /// The daemon directory of this spec:
    /// `<cache>/dartq/analyzer/daemons/<key>`. The key is the answer's
    /// key ([`Spec::cache_dir`]'s) plus the dartq binary: a rebuilt or
    /// upgraded dartq gets a fresh daemon over the same server cache. The
    /// budget, idle timeout and queue are the starter's and do not split
    /// daemons.
    #[must_use]
    pub fn dir(&self) -> PathBuf {
        let mut hasher = self.answer_key();
        add(&mut hasher, binary_identity().as_bytes());
        cache_root()
            .join(DAEMONS)
            .join(hex::encode(&hasher.finalize().as_bytes()[..8]))
    }

    /// The server's `--cache`: `<cache>/dartq/analyzer/caches/<key>`, keyed
    /// by the SDK, the roots and the package config. One per worktree: the
    /// analyzer's result cache keys some entries by package URI, which two
    /// worktrees share, and on Dart 3.8 a shared cache hands one worktree
    /// the context-message paths of another. The server bounds each cache
    /// (`EvictingFileByteStore`, 4 GB), and [`sweep`] drops the caches of
    /// worktrees that no longer exist.
    #[must_use]
    pub fn cache_dir(&self) -> PathBuf {
        cache_root()
            .join(CACHES)
            .join(hex::encode(&self.answer_key().finalize().as_bytes()[..8]))
    }

    fn answer_key(&self) -> blake3::Hasher {
        let mut hasher = blake3::Hasher::new();
        add(&mut hasher, self.sdk_root.as_os_str().as_encoded_bytes());
        add(&mut hasher, self.sdk_version.as_bytes());
        for root in &self.roots {
            add(&mut hasher, root.as_os_str().as_encoded_bytes());
        }
        add(
            &mut hasher,
            self.packages
                .as_deref()
                .map_or(&[][..], |p| p.as_os_str().as_encoded_bytes()),
        );
        hasher
    }

    /// The installation the SDK belongs to: a Flutter checkout for its
    /// bundled SDK (`<flutter>/bin/cache/dart-sdk`), else the SDK itself.
    #[must_use]
    pub fn sdk_home(&self) -> PathBuf {
        let bundled = self
            .sdk_root
            .ends_with(Path::new("bin").join("cache").join("dart-sdk"));
        match self.sdk_root.ancestors().nth(3) {
            Some(flutter) if bundled => flutter.to_path_buf(),
            _ => self.sdk_root.clone(),
        }
    }
}

fn add(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

const DAEMONS: &str = "daemons";
const CACHES: &str = "caches";
/// The roots a server cache was built for, written beside it for [`sweep`].
const ROOTS_FILE: &str = "roots.json";

/// The files of one daemon directory.
pub(crate) struct Files {
    pub socket: PathBuf,
    pub spec: PathBuf,
    /// The daemon's pid; its mtime is the daemon's last answer.
    pub state: PathBuf,
    pub elect: PathBuf,
    pub failed: PathBuf,
    /// Locked while the daemon answers a request.
    pub busy: PathBuf,
}

impl Files {
    pub(crate) fn of(dir: &Path) -> Self {
        Self {
            socket: dir.join("s.sock"),
            spec: dir.join("spec.json"),
            state: dir.join("state.json"),
            elect: dir.join("elect.lock"),
            failed: dir.join("failed"),
            busy: dir.join("busy.lock"),
        }
    }
}

/// Records the roots `cache` serves, so [`sweep`] can tell when they are
/// gone.
pub(crate) fn claim_cache(cache: &Path, roots: &[PathBuf]) {
    if std::fs::create_dir_all(cache).is_ok() {
        let roots = serde_json::to_vec(roots).expect("paths serialize");
        let _ = std::fs::write(cache.join(ROOTS_FILE), roots);
    }
}

/// Removes the server caches whose roots no longer exist (a deleted
/// worktree's cache is up to 4 GB nobody will read again) and the daemon
/// directories no daemon owns (an old dartq binary's). A directory whose
/// start lock is held is a daemon being started and is left alone.
#[cfg(unix)]
pub(crate) fn sweep(own: &Path) {
    let root = cache_root();
    for cache in entries(&root.join(CACHES)) {
        let gone = std::fs::read(cache.join(ROOTS_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Vec<PathBuf>>(&bytes).ok())
            .is_some_and(|roots| roots.iter().any(|root| !root.exists()));
        if gone {
            let _ = std::fs::remove_dir_all(&cache);
        }
    }
    for dir in entries(&root.join(DAEMONS)) {
        // A failure marker is the cooldown of its key: kept.
        if dir == own || daemon_pid(&dir).is_some() || Files::of(&dir).failed.exists() {
            continue;
        }
        let Ok(lock) = std::fs::File::open(Files::of(&dir).elect) else {
            let _ = std::fs::remove_dir_all(&dir);
            continue;
        };
        if lock.try_lock().is_ok() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

/// The live daemons other than `own`: directory, pid and last answer.
#[cfg(unix)]
pub(crate) fn peers(own: &Path) -> Vec<(PathBuf, u32, std::time::SystemTime)> {
    entries(&cache_root().join(DAEMONS))
        .into_iter()
        .filter(|dir| dir != own)
        .filter_map(|dir| {
            let pid = daemon_pid(&dir)?;
            let used = std::fs::metadata(Files::of(&dir).state)
                .and_then(|meta| meta.modified())
                .ok()?;
            Some((dir, pid, used))
        })
        .collect()
}

/// The pid of the live daemon of `dir`.
#[cfg(unix)]
fn daemon_pid(dir: &Path) -> Option<u32> {
    let pid = std::fs::read_to_string(Files::of(dir).state)
        .ok()?
        .trim()
        .parse()
        .ok()?;
    dartq_toolchain::proc::alive(pid).then_some(pid)
}

#[cfg(unix)]
fn entries(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .collect()
}

/// `$XDG_CACHE_HOME|~/.cache` + `dartq/analyzer`.
fn cache_root() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|home| home.join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("dartq")
        .join("analyzer")
}

/// The running binary's identity: path, size and mtime. A rebuilt or
/// upgraded dartq gets fresh daemons without hashing its bytes per run.
fn binary_identity() -> String {
    let exe = std::env::current_exe().unwrap_or_default();
    let meta = std::fs::metadata(&exe).ok();
    let mtime = meta
        .as_ref()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos());
    format!(
        "{}:{}:{mtime}",
        exe.display(),
        meta.map_or(0, |meta| meta.len())
    )
}
