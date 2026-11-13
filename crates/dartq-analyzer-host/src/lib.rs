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
//! * [`daemon`] is the serving side; `disk` keeps the servers' caches
//!   inside the machine's disk budget.
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
#[cfg(unix)]
mod disk;
pub mod face;
pub mod report;
pub mod server;
pub mod snapshot;
pub mod support;
#[cfg(unix)]
pub(crate) mod watch;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use client::{Outcome, Request, analyze};
pub use face::{Face, FaceFormat, FaceMeta, FaceRender, FaceRenderer};
pub use report::{Report, Row};
pub use support::{Sdk, Support, Version, support};

/// The hidden `dartq analyze` flag that runs a daemon over the spec in the
/// directory it names.
pub const SERVE_FLAG: &str = "--resident-serve";

/// One `dartq analyze` rest argument, resolved to its on-disk kind — the
/// oracle's target classification (`dart analyze` treats an existing
/// directory as a directory target, an existing file as a file target).
/// Paths arrive canonicalized, in the same spelling universe the server's
/// `analysis.errors` file keys use.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Aim {
    /// A directory target: the report emits every diagnostic whose file
    /// lies under it.
    Dir(PathBuf),
    /// A file target: the report emits only that file's rows (and only
    /// when it is a `.dart` file — a file aim's context root never
    /// analyzes anything else).
    File(PathBuf),
}

/// Whether `path` is covered by one of the `aims` — under a directory aim
/// or identical to a `.dart` file aim. This is the emit filter of
/// `dart analyze <paths>`: the analyzer's context covers the whole roots
/// set while the oracle prints only the covered files' rows.
#[must_use]
pub fn covered(path: &Path, aims: &[Aim]) -> bool {
    aims.iter().any(|aim| match aim {
        Aim::Dir(dir) => path.starts_with(dir),
        Aim::File(file) => {
            path == file && file.extension().is_some_and(|ext| ext == "dart")
        }
    })
}

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
    /// `memory.max_percent`: the machine-wide memory ledger's share.
    pub max_percent: u8,
    /// `memory.max_concurrent`: the job cap the server's slot declares
    /// (`0` unlimited).
    pub max_concurrent: u32,
    /// The shared-pool posture: a pooled daemon serves several worktrees'
    /// roots under one server (`Ask.roots` joins; the directory hash
    /// drops the roots so every compatible worktree lands on the same
    /// daemon directory and the same election). `false` is the
    /// one-worktree daemon.
    #[serde(default)]
    pub pool: bool,
    /// The most worktrees a pooled daemon serves (1–64; `0` defaults 3).
    #[serde(default)]
    pub max_roots: u32,
    /// The warm-standby posture: `0` (the serde default, i.e. unset by an
    /// older client) resolves daemon-side to the default — warm on. The
    /// daemon also honors `DARTQ_WARM_SERVERS` over whatever the spec
    /// carries, so A/B arms need no CLI wiring. More than one standby is
    /// meaningless for a daemon with one server at a time; `> 0` is on.
    #[serde(default)]
    pub warm: u32,
    /// How long an idle daemon keeps its standby before exiting (Tier 3);
    /// `0` resolves daemon-side to the default (900 s).
    #[serde(default)]
    pub warm_ttl_secs: u32,
}

impl Spec {
    /// The daemon directory of this spec:
    /// `<cache>/dartq/analyzer/daemons/<key>`. The key is the answer's
    /// key ([`Spec::cache_dir`]'s) plus the dartq binary: a rebuilt or
    /// upgraded dartq gets a fresh daemon over the same server cache. The
    /// budget, idle timeout, memory share and job cap are the starter's and
    /// do not split daemons.
    #[must_use]
    pub fn dir(&self) -> PathBuf {
        let mut hasher = self.answer_key();
        add(&mut hasher, binary_identity().as_bytes());
        cache_root()
            .join(DAEMONS)
            .join(hex::encode(&hasher.finalize().as_bytes()[..8]))
    }

    /// The daemon's per-key state dir: `<cache>/dartq/analyzer/caches/<key>`,
    /// keyed by the SDK, the roots and the package config. It no longer feeds
    /// the server's `--cache` — that flag priced an empty dir into a fresh
    /// `EvictingFileByteStore` (~10× cold wall on a 50k-file corpus vs the
    /// shared `~/.dartServer` the stock oracle uses). The dir still carries
    /// the roots file [`claim_cache`] writes for [`sweep`], the disk-budget
    /// LRU marker [`crate::disk::used`] refreshes on each answer, and the
    /// `peak_key` fallback when the worktree's git identity is unreadable.
    #[must_use]
    pub fn cache_dir(&self) -> PathBuf {
        cache_root().join(CACHES).join(self.cache_key())
    }

    /// The cache directory's name.
    fn cache_key(&self) -> String {
        hex::encode(&self.answer_key().finalize().as_bytes()[..8])
    }

    fn answer_key(&self) -> blake3::Hasher {
        let mut hasher = blake3::Hasher::new();
        add(&mut hasher, self.sdk_root.as_os_str().as_encoded_bytes());
        add(&mut hasher, self.sdk_version.as_bytes());
        if !self.pool {
            for root in &self.roots {
                add(&mut hasher, root.as_os_str().as_encoded_bytes());
            }
            add(
                &mut hasher,
                self.packages
                    .as_deref()
                    .map_or(&[][..], |p| p.as_os_str().as_encoded_bytes()),
            );
        } else {
            // A pooled daemon is one per (SDK, binary, packages
            // override) — the roots join through `set_roots`, but a
            // `--packages` override is a server-launch flag: a joiner
            // with a different override cannot share the founder's
            // server. `None` pools with `None`; `Some` keys on the
            // override's bytes so the join lands on a different pool
            // (and a worktree without the override never sees it).
            add(&mut hasher, b"pool");
            if let Some(packages) = self.packages.as_deref() {
                add(&mut hasher, packages.as_os_str().as_encoded_bytes());
            }
        }
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

/// The length-prefixed hash leg the key derivations share ([`Spec`]'s
/// daemon and cache keys, and the answered-turn record key #4278).
pub(crate) fn add(hasher: &mut blake3::Hasher, bytes: &[u8]) {
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
    /// The live analysis server child's pid — the successor daemon's
    /// orphan reap when a SIGKILL'd predecessor could not take its server
    /// down with it (the group kill only runs on catchable signals).
    pub server_pid: PathBuf,
    /// The daemon's observability log ([`Files::event`]).
    pub events: PathBuf,
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
            server_pid: dir.join("server.pid"),
            events: dir.join("events.log"),
        }
    }

    /// Emits one always-on observability line (an analysis server's
    /// start or stop with its reason, or a job-gate wait's enter/exit —
    /// `gate-wait` / `gate-admit` / `gate-fault`): to stderr for a
    /// hand-run foreground `--resident-serve`, and appended to the
    /// daemon dir's `events.log` — the client spawns the daemon with its
    /// stderr discarded, so the file is the only copy that reaches
    /// anyone. The log restarts with the current line once it passes
    /// 64 KiB: bounded, no rotation machinery. Both sinks are best
    /// effort; an event that cannot be written never blocks the daemon.
    pub(crate) fn event(&self, line: &str) {
        use std::io::Write as _;
        eprintln!("{line}");
        const EVENTS_CAP: u64 = 64 * 1024;
        let restart = std::fs::metadata(&self.events).is_ok_and(|meta| meta.len() >= EVENTS_CAP);
        let line = format!("{line}\n");
        let _ = if restart {
            std::fs::write(&self.events, line)
        } else {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.events)
                .and_then(|mut file| file.write_all(line.as_bytes()))
        };
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

/// Removes the server caches under `root` whose roots no longer exist (a
/// deleted worktree's cache is up to 4 GB nobody will read again) and the
/// daemon directories no daemon owns (an old dartq binary's). A directory
/// whose start lock is held is a daemon being started and is left alone,
/// as is `own`, the calling daemon's directory.
#[cfg(unix)]
pub(crate) fn sweep(root: &Path, own: &Path) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The emit filter of `dart analyze <paths>`: directory aims cover
    /// by prefix (never a same-named sibling — `lib` does not cover
    /// `lib2`), file aims cover their own `.dart` file only.
    #[test]
    fn covered_is_the_oracles_emit_filter() {
        let dir = Aim::Dir(PathBuf::from("/p/lib"));
        let file = Aim::File(PathBuf::from("/p/lib/a.dart"));
        let yaml = Aim::File(PathBuf::from("/p/pubspec.yaml"));
        assert!(covered(Path::new("/p/lib/a.dart"), &[dir.clone()]));
        assert!(covered(Path::new("/p/lib/deep/b.dart"), std::slice::from_ref(&dir)));
        assert!(!covered(Path::new("/p/lib2/c.dart"), &[dir.clone()]));
        assert!(!covered(Path::new("/p/test/t.dart"), std::slice::from_ref(&dir)));
        assert!(covered(Path::new("/p/lib/a.dart"), std::slice::from_ref(&file)));
        assert!(!covered(Path::new("/p/lib/b.dart"), std::slice::from_ref(&file)));
        assert!(covered(Path::new("/p/pubspec.yaml"), &[Aim::Dir(PathBuf::from("/p"))]));
        assert!(!covered(Path::new("/p/pubspec.yaml"), std::slice::from_ref(&yaml)));
    }

    /// The pooled spec's daemon directory is the pool's rendezvous: it
    /// drops the roots from the key (a `--packages` override stays — it
    /// is a server-launch flag the shared server satisfies once), so
    /// every compatible worktree computes the same directory — and the
    /// SDK or the dartq binary changing lands the pool on a different
    /// directory, never the same daemon under a stale key.
    #[test]
    fn a_pooled_specs_dir_ignores_roots_and_separates_sdks() {
        let spec = |sdk: &str, roots: &[&str], pool: bool| Spec {
            sdk_root: PathBuf::from(sdk),
            sdk_version: "3.13.7".to_owned(),
            roots: roots.iter().map(PathBuf::from).collect(),
            packages: None,
            budget: 0,
            idle_secs: 60,
            max_percent: 90,
            max_concurrent: 0,
            pool,
            max_roots: 3,
            warm: 0,
            warm_ttl_secs: 0,
        };
        let a = spec("/sdks/a", &["/w/a"], true);
        let b = spec("/sdks/a", &["/w/b"], true);
        let ab = spec("/sdks/a", &["/w/a", "/w/b"], true);
        // Two worktrees of one SDK pool to one daemon directory.
        assert_eq!(a.dir(), b.dir());
        assert_eq!(a.dir(), ab.dir());
        // Another SDK is another pool — never the same daemon.
        assert_ne!(a.dir(), spec("/sdks/b", &["/w/a"], true).dir());
        // Pooled and one-worktree specs of the same roots never collide.
        assert_ne!(a.dir(), spec("/sdks/a", &["/w/a"], false).dir());
    }

    /// A pooled spec with an explicit `--packages` override never joins
    /// a pool founded without it — the override is a server-launch flag
    /// a shared server cannot satisfy twice, so it keys the pool's
    /// daemon directory. Same override, same pool.
    #[test]
    fn a_pooled_specs_dir_keeps_the_packages_override() {
        let mut plain = Spec {
            sdk_root: PathBuf::from("/sdks/a"),
            sdk_version: "3.13.7".to_owned(),
            roots: vec![PathBuf::from("/w/a")],
            packages: None,
            budget: 0,
            idle_secs: 60,
            max_percent: 90,
            max_concurrent: 0,
            pool: true,
            max_roots: 3,
            warm: 0,
            warm_ttl_secs: 0,
        };
        let mut override_1 = plain.clone();
        override_1.packages = Some(PathBuf::from("/w/a/.dart_tool/package_config.json"));
        let mut override_2 = plain.clone();
        override_2.packages = Some(PathBuf::from("/w/b/.dart_tool/package_config.json"));
        assert_ne!(plain.dir(), override_1.dir());
        assert_ne!(override_1.dir(), override_2.dir());
        // The same override on a different worktree's roots still pools
        // — only roots are dropped, the override stays.
        let mut joined = override_1.clone();
        joined.roots = vec![PathBuf::from("/w/b")];
        assert_eq!(override_1.dir(), joined.dir());
        plain.packages = Some(PathBuf::from("/w/a/.dart_tool/package_config.json"));
        assert_eq!(plain.dir(), override_1.dir());
    }

    /// The warm knobs are absent in every spec an older client writes:
    /// such a spec parses to `0` ("not declared") and round-trips — the
    /// daemon resolves `0` to the warm-on default, so old specs behave
    /// as the new default, never as a silent opt-out.
    #[test]
    fn an_old_spec_parses_without_the_warm_knobs_and_round_trips() {
        let old = r#"{
            "sdk_root": "/sdks/a",
            "sdk_version": "3.13.7",
            "roots": ["/w/a"],
            "packages": null,
            "budget": 0,
            "idle_secs": 60,
            "max_percent": 90,
            "max_concurrent": 0,
            "pool": true,
            "max_roots": 3
        }"#;
        let spec: Spec = serde_json::from_str(old).expect("the old spec parses");
        assert_eq!(spec.warm, 0, "unset means not-declared");
        assert_eq!(spec.warm_ttl_secs, 0);
        let back = serde_json::to_string(&spec).expect("serializes");
        let again: Spec = serde_json::from_str(&back).expect("round-trips");
        assert_eq!(again, spec);
    }
}

/// The running binary's identity: path, size and mtime. A rebuilt or
/// upgraded dartq gets fresh daemons without hashing its bytes per run —
/// and fresh answered-turn records (#4278): the face writer the render
/// ships through is this binary's, so its stamp belongs in the record
/// key.
pub(crate) fn binary_identity() -> String {
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
