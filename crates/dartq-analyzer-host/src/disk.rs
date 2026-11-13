//! The analyzer caches' disk discipline.
//!
//! Each server bounds its own `--cache` byte store (the official
//! `EvictingFileByteStore`, 4 GiB), and there is one store per worktree
//! ([`crate::Spec::cache_dir`]): N worktrees are N stores, and nothing
//! bounded the volume. A budget pass keeps the caches inside two figures:
//!
//! * the caches no live daemon serves hold at most [`IDLE_CACHE_CAP`]
//!   together, the least recently used deleted first;
//! * the cache volume keeps its floor free ([`floor_of`]): idle caches go
//!   first, then the live byte stores lose their oldest files (by mtime —
//!   atime is unreliable on macOS), largest store first, never below
//!   [`LIVE_CACHE_MIN`] each. A live cache is never deleted whole; a file
//!   deleted under a live server is a cache miss, as when the server's own
//!   evictor deletes it.
//!
//! Passes run on a daemon's background thread, machine-wide single-flight
//! (`caches/.budget.lock`, `.budget.stamp`): after a daemon start and
//! after an answered analysis at most once per [`BUDGET_SPACING`], and at
//! once when a request's one `statvfs` — or, while an analysis runs, a
//! check every [`URGENT_SPACING`] — finds the volume under the floor: a
//! cold analysis writes its store for minutes with no request to check
//! on. A server that would start under the floor gets a pass first. The
//! server offers no working way to run without its store (its
//! `--disable-file-byte-store` only applies in snapshot-training mode),
//! so trimming is the whole defense.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use crate::{CACHES, DAEMONS, Files, ROOTS_FILE, Spec};

/// The most the caches no live daemon serves hold together.
const IDLE_CACHE_CAP: u64 = 8 << 30;
/// A live byte store is never trimmed below this.
const LIVE_CACHE_MIN: u64 = 256 << 20;
/// The floor's bounds around 2% of the volume.
const FLOOR_MIN: u64 = 4 << 30;
const FLOOR_MAX: u64 = 16 << 30;
/// Overrides the disk floor, in bytes.
pub(crate) const FLOOR_ENV: &str = "DARTQ_DISK_FLOOR_BYTES";
/// Routine passes run at most this often, machine-wide.
const BUDGET_SPACING: Duration = Duration::from_secs(60);
/// Urgent passes skip the routine spacing but keep this one: a volume that
/// stays under the floor (every cache already trimmed) never walks the
/// caches back to back.
const URGENT_SPACING: Duration = Duration::from_secs(5);
/// How long a server start waits out a peer's pass before deciding on the
/// space it finds.
const START_WAIT: Duration = Duration::from_secs(30);
/// The server's byte store inside its `--cache`: the only files a live
/// trim deletes (the plugin manager's state beside it is not a cache).
const STORE: &str = ".analysis-driver";

/// One volume's space, in bytes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Space {
    /// What an unprivileged writer may still claim (`f_bavail`).
    pub free: u64,
    /// The volume's size (`f_blocks`).
    pub total: u64,
}

/// The space probe: [`statvfs`] in production, a fixture in tests — no
/// test depends on the host's disk.
type Probe = fn(&Path) -> Option<Space>;

/// The space of the volume holding `path`, from `statvfs(3)`. `None` when
/// the call fails: passes then bound the idle caches only.
pub(crate) fn statvfs(path: &Path) -> Option<Space> {
    use std::os::unix::ffi::OsStrExt as _;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: all-zero is a valid `statvfs` (integer fields only).
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is NUL-terminated and `stats` outlives the call; it
    // is read only after a zero return.
    if unsafe { libc::statvfs(path.as_ptr(), &mut stats) } != 0 {
        return None;
    }
    // The block counts are per-platform unsigned aliases (32-bit on
    // macOS, 64-bit on Linux) in `f_frsize` units.
    let unit = stats.f_frsize as u64;
    Some(Space {
        free: (stats.f_bavail as u64).saturating_mul(unit),
        total: (stats.f_blocks as u64).saturating_mul(unit),
    })
}

/// The free space the caches leave on a volume of `total` bytes:
/// [`FLOOR_ENV`] when set, else 2% of the volume within 4–16 GiB.
fn disk_floor(total: u64) -> u64 {
    floor_of(
        total,
        std::env::var(FLOOR_ENV)
            .ok()
            .and_then(|value| value.trim().parse().ok()),
    )
}

fn floor_of(total: u64, set: Option<u64>) -> u64 {
    set.filter(|&bytes| bytes > 0)
        .unwrap_or_else(|| (total / 50).clamp(FLOOR_MIN, FLOOR_MAX))
}

/// `Some(free)` when the volume is under the floor; an unreadable figure
/// is never short.
fn short(free: Option<u64>, floor: Option<u64>) -> Option<u64> {
    let free = free?;
    (free < floor?).then_some(free)
}

/// Marks `cache` used now: its roots file's mtime is the idle caches' LRU
/// clock.
pub(crate) fn used(cache: &Path) {
    let _ = std::fs::File::options()
        .write(true)
        .open(cache.join(ROOTS_FILE))
        .and_then(|file| file.set_modified(SystemTime::now()));
}

/// Why a pass runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Pass {
    /// A daemon start, a server start or an answered analysis.
    Routine,
    /// A request found the volume under the floor.
    Urgent,
    /// A server start under the floor: waits out a peer's pass, then runs
    /// unless the volume recovered meanwhile.
    Start,
}

/// Whether a pass is due, `since` the last pass started: routine passes
/// keep [`BUDGET_SPACING`], urgent ones [`URGENT_SPACING`] and only under
/// the floor, and a start pass runs whenever the volume is under it.
fn due(pass: Pass, short: bool, since: Duration) -> bool {
    match pass {
        Pass::Routine => since >= BUDGET_SPACING,
        Pass::Urgent => short && since >= URGENT_SPACING,
        Pass::Start => short,
    }
}

/// One machine-wide, single-flight pass over the caches under `root`
/// (`caches/.budget.lock`; the `.budget.stamp` mtime is the last pass's
/// start): the dead-roots [`crate::sweep`] (keeping `own`, the calling
/// daemon's directory), then [`enforce`]. A routine or urgent pass skips
/// while a peer's pass holds the lock; a start pass waits it out.
fn budget(root: &Path, own: &Path, pass: Pass, probe: Probe) {
    let caches = root.join(CACHES);
    if std::fs::create_dir_all(&caches).is_err() {
        return;
    }
    let Ok(lock) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(caches.join(".budget.lock"))
    else {
        return;
    };
    let deadline = Instant::now() + START_WAIT;
    while lock.try_lock().is_err() {
        if pass != Pass::Start || Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let stamp = caches.join(".budget.stamp");
    let since = std::fs::metadata(&stamp)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|at| at.elapsed().ok())
        .unwrap_or(Duration::MAX);
    let space = probe(&caches);
    let floor = space.map(|space| disk_floor(space.total));
    let short = short(space.map(|space| space.free), floor).is_some();
    if !due(pass, short, since) {
        return;
    }
    let _ = std::fs::write(&stamp, b"");
    crate::sweep(root, own);
    let limits = Limits {
        floor: floor.unwrap_or(0),
        idle_cap: IDLE_CACHE_CAP,
        live_min: LIVE_CACHE_MIN,
    };
    // The sweep may have freed space: the pass works from a fresh figure.
    let free = probe(&caches).map(|space| space.free);
    enforce(&caches, &live_caches(root), free, limits);
}

/// The caches live daemons serve: a daemon whose pid lives, or one being
/// started (its start lock held). Never deleted whole.
fn live_caches(root: &Path) -> Vec<PathBuf> {
    crate::entries(&root.join(DAEMONS))
        .into_iter()
        .filter(|dir| crate::daemon_pid(dir).is_some() || starting(dir))
        .filter_map(|dir| {
            let spec = std::fs::read(Files::of(&dir).spec).ok()?;
            let spec: Spec = serde_json::from_slice(&spec).ok()?;
            Some(root.join(CACHES).join(spec.cache_key()))
        })
        .collect()
}

fn starting(dir: &Path) -> bool {
    std::fs::File::open(Files::of(dir).elect).is_ok_and(|lock| lock.try_lock().is_err())
}

/// One pass's figures.
#[derive(Clone, Copy, Debug)]
struct Limits {
    /// The free space the volume keeps.
    floor: u64,
    /// The most the idle caches hold together.
    idle_cap: u64,
    /// The size no live byte store is trimmed below.
    live_min: u64,
}

/// One pass over the caches under `caches`, given the volume's `free`
/// bytes (`None`: unknown — the idle cap alone binds). Idle caches go
/// least recently used first until they fit the cap and the volume clears
/// the floor; still under the floor, the live byte stores lose their
/// oldest files, largest store first, down to the minimum each. `live`
/// lists the caches live daemons serve: never deleted whole. Freed bytes
/// count toward the free figure as they go — one probe per pass.
fn enforce(caches: &Path, live: &[PathBuf], free: Option<u64>, limits: Limits) {
    let (served, idle): (Vec<PathBuf>, Vec<PathBuf>) = crate::entries(caches)
        .into_iter()
        .filter(|dir| dir.is_dir())
        .partition(|dir| live.contains(dir));
    let mut idle: Vec<(SystemTime, u64, PathBuf)> = idle
        .into_iter()
        .map(|dir| (last_use(&dir), tree_bytes(&dir), dir))
        .collect();
    idle.sort_by_key(|(used, ..)| *used);
    let mut idle_total: u64 = idle.iter().map(|(_, bytes, _)| bytes).sum();
    let mut free = free.unwrap_or(u64::MAX);
    for (_, bytes, dir) in idle {
        if idle_total <= limits.idle_cap && free >= limits.floor {
            break;
        }
        if std::fs::remove_dir_all(&dir).is_ok() {
            idle_total -= bytes;
            free = free.saturating_add(bytes);
        }
    }
    if free >= limits.floor {
        return;
    }
    // Sized first, listed one store at a time: a pass never holds every
    // live store's file list at once.
    let mut stores: Vec<(u64, PathBuf)> = served
        .into_iter()
        .map(|dir| {
            let store = dir.join(STORE);
            (tree_bytes(&store), store)
        })
        .collect();
    stores.sort_by(|a, b| b.0.cmp(&a.0));
    for (mut size, store) in stores {
        if free >= limits.floor {
            return;
        }
        let mut files = Vec::new();
        list_files(&store, &mut files);
        files.sort_by_key(|(mtime, ..)| *mtime);
        for (_, bytes, file) in files {
            if size <= limits.live_min || free >= limits.floor {
                break;
            }
            if std::fs::remove_file(&file).is_ok() {
                size = size.saturating_sub(bytes);
                free = free.saturating_add(bytes);
            }
        }
    }
}

/// A cache's last use: its roots file's mtime ([`used`] touches it per
/// answer), else the directory's own.
fn last_use(dir: &Path) -> SystemTime {
    std::fs::metadata(dir.join(ROOTS_FILE))
        .or_else(|_| std::fs::metadata(dir))
        .and_then(|meta| meta.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

/// The bytes of the files under `dir`; symlinks are not followed.
fn tree_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| match entry.metadata() {
            Ok(meta) if meta.is_dir() => tree_bytes(&entry.path()),
            Ok(meta) => meta.len(),
            Err(_) => 0,
        })
        .sum()
}

/// The files under `dir` with their mtimes and lengths.
fn list_files(dir: &Path, files: &mut Vec<(SystemTime, u64, PathBuf)>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            list_files(&entry.path(), files);
        } else {
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            files.push((mtime, meta.len(), entry.path()));
        }
    }
}

/// One daemon's disk discipline: the request-time check, the in-analysis
/// tick, the server-start pass, and the background pass worker.
pub(crate) struct Disk {
    root: PathBuf,
    own: PathBuf,
    /// The cache volume's floor; `None` when the volume cannot be read
    /// (every pass still bounds the idle caches).
    floor: Option<u64>,
    kick: mpsc::Sender<Pass>,
    /// The last in-analysis check.
    ticked: Cell<Option<Instant>>,
}

impl Disk {
    /// Starts the pass worker of the daemon in `own` and kicks the
    /// daemon-start pass (the dead-roots sweep included).
    pub(crate) fn start(own: &Path) -> Self {
        let root = crate::cache_root();
        let floor = statvfs(&root).map(|space| disk_floor(space.total));
        let (kick, passes) = mpsc::channel::<Pass>();
        let (worker_root, worker_own) = (root.clone(), own.to_path_buf());
        std::thread::spawn(move || {
            while let Ok(pass) = passes.recv() {
                // Kicks queued while a pass ran coalesce into one pass:
                // the most urgent wins.
                let pass = passes.try_iter().fold(pass, Pass::max);
                budget(&worker_root, &worker_own, pass, statvfs);
            }
        });
        let _ = kick.send(Pass::Routine);
        Self {
            root,
            own: own.to_path_buf(),
            floor,
            kick,
            ticked: Cell::new(None),
        }
    }

    /// The request-time check: one `statvfs`; a volume under the floor
    /// kicks an urgent pass.
    pub(crate) fn check(&self) {
        if short(self.free(), self.floor).is_some() {
            let _ = self.kick.send(Pass::Urgent);
        }
    }

    /// The in-analysis check, called once per sample of a running
    /// analysis and spaced to [`URGENT_SPACING`]: the store an analysis
    /// writes is trimmed while it runs, not after.
    pub(crate) fn tick(&self) {
        let now = Instant::now();
        if self
            .ticked
            .get()
            .is_some_and(|at| now.duration_since(at) < URGENT_SPACING)
        {
            return;
        }
        self.ticked.set(Some(now));
        self.check();
    }

    /// Kicks a routine pass — spaced machine-wide, so an answered
    /// analysis costs a channel send.
    pub(crate) fn routine(&self) {
        let _ = self.kick.send(Pass::Routine);
    }

    /// The server-start check: under the floor a pass runs first (on the
    /// caller's thread — a cold start's path, never a warm answer's);
    /// `Some(free)` when the volume is still under the floor.
    pub(crate) fn short_at_start(&self) -> Option<u64> {
        short(self.free(), self.floor)?;
        budget(&self.root, &self.own, Pass::Start, statvfs);
        short(self.free(), self.floor)
    }

    fn free(&self) -> Option<u64> {
        statvfs(&self.root).map(|space| space.free)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const GIB: u64 = 1 << 30;

    /// A cache dir holding one `bytes`-long store file and a 2-byte roots
    /// file (`[]`), last used `age` seconds before `now`.
    fn cache(caches: &Path, name: &str, bytes: usize, now: SystemTime, age: u64) -> PathBuf {
        let dir = caches.join(name);
        fs::create_dir_all(dir.join(STORE).join("aa")).expect("store dir");
        fs::write(dir.join(STORE).join("aa").join("key"), vec![0u8; bytes]).expect("store file");
        let roots = dir.join(ROOTS_FILE);
        fs::write(&roots, b"[]").expect("roots file");
        fs::File::options()
            .write(true)
            .open(&roots)
            .and_then(|file| file.set_modified(now - Duration::from_secs(age)))
            .expect("last use");
        dir
    }

    /// One store file of `bytes`, written `age` seconds before `now`.
    fn store_file(cache: &Path, name: &str, bytes: usize, now: SystemTime, age: u64) -> PathBuf {
        let path = cache.join(STORE).join("bb").join(name);
        fs::create_dir_all(path.parent().expect("shard")).expect("shard dir");
        fs::write(&path, vec![0u8; bytes]).expect("store file");
        fs::File::options()
            .write(true)
            .open(&path)
            .and_then(|file| file.set_modified(now - Duration::from_secs(age)))
            .expect("mtime");
        path
    }

    /// Idle caches over the cap go least recently used first, and only
    /// until the rest fits: the newest idle cache stays warm.
    #[test]
    fn idle_caches_go_least_recently_used_first() {
        let root = tempfile::tempdir().expect("caches");
        let now = SystemTime::now();
        // 100 bytes each: a 98-byte store file and the 2-byte roots file.
        let oldest = cache(root.path(), "a", 98, now, 300);
        let middle = cache(root.path(), "b", 98, now, 200);
        let newest = cache(root.path(), "c", 98, now, 100);
        let limits = Limits {
            floor: 0,
            idle_cap: 150,
            live_min: 0,
        };
        enforce(root.path(), &[], None, limits);
        assert!(!oldest.exists() && !middle.exists(), "the two LRU caches go");
        assert!(newest.exists(), "100 bytes fit the 150-byte cap");

        // Under the floor the idle caches go LRU first too, until the
        // freed bytes clear it.
        let a = cache(root.path(), "d", 98, now, 300);
        let b = cache(root.path(), "e", 98, now, 200);
        let limits = Limits {
            floor: 1000,
            idle_cap: u64::MAX,
            live_min: 0,
        };
        // 1000 − 950 = 50 short: the LRU cache (100 bytes) clears it.
        enforce(root.path(), &[], Some(950), limits);
        assert!(!a.exists(), "the LRU idle cache goes");
        assert!(b.exists() && newest.exists(), "the floor is clear: the rest stay");
    }

    /// A live cache is never deleted whole, however short the volume: its
    /// byte store is trimmed, its directory and roots file stay.
    #[test]
    fn a_live_cache_is_trimmed_never_deleted() {
        let root = tempfile::tempdir().expect("caches");
        let now = SystemTime::now();
        // A fresh 500-byte store file (`aa/key`) beside an old one.
        let live = cache(root.path(), "live", 500, now, 10);
        let old = store_file(&live, "old", 500, now, 500);
        let idle = cache(root.path(), "idle", 50, now, 1000);
        let limits = Limits {
            floor: u64::MAX,
            idle_cap: u64::MAX,
            live_min: 600,
        };
        enforce(root.path(), std::slice::from_ref(&live), Some(0), limits);
        assert!(!idle.exists(), "the idle cache goes first");
        assert!(!old.exists(), "the live store's oldest file goes");
        assert!(
            live.join(STORE).join("aa").join("key").exists(),
            "1000 → 500 bytes reaches the 600-byte minimum: the fresh file stays"
        );
        assert!(live.join(ROOTS_FILE).exists(), "the live cache is never deleted");
    }

    /// The live trim deletes the oldest files first, stops at the store
    /// minimum, and stops as soon as the floor clears — the largest store
    /// first.
    #[test]
    fn the_live_trim_takes_the_oldest_files_and_stops_at_the_minimum() {
        let root = tempfile::tempdir().expect("caches");
        let now = SystemTime::now();
        let live = root.path().join("live");
        let old = store_file(&live, "old", 100, now, 300);
        let mid = store_file(&live, "mid", 100, now, 200);
        let new = store_file(&live, "new", 100, now, 100);
        let limits = Limits {
            floor: u64::MAX,
            idle_cap: u64::MAX,
            live_min: 150,
        };
        // Hopelessly short: 300 → 200 → 100 bytes, the minimum binds.
        enforce(root.path(), std::slice::from_ref(&live), Some(0), limits);
        assert!(!old.exists() && !mid.exists(), "oldest first");
        assert!(new.exists(), "never below the 150-byte minimum");

        // Two live stores, 50 bytes short: the largest store's oldest file
        // clears the floor and the trim stops there.
        let big = root.path().join("big");
        let big_old = store_file(&big, "old", 100, now, 300);
        let big_new = store_file(&big, "new", 100, now, 100);
        let small = root.path().join("small");
        let small_old = store_file(&small, "old", 100, now, 900);
        let limits = Limits {
            floor: 1000,
            idle_cap: u64::MAX,
            live_min: 0,
        };
        enforce(root.path(), &[live, big, small], Some(950), limits);
        assert!(!big_old.exists(), "the largest store's oldest file");
        assert!(big_new.exists() && small_old.exists(), "the floor is clear");
    }

    /// The floor: 2% of the volume within 4–16 GiB, the override verbatim
    /// (`0` is unset).
    #[test]
    fn the_floor_is_two_percent_within_four_and_sixteen_gib() {
        assert_eq!(floor_of(100 * GIB, None), 4 * GIB, "2 GiB → the 4 GiB floor");
        assert_eq!(floor_of(500 * GIB, None), 10 * GIB, "2% of 500 GiB");
        assert_eq!(floor_of(2048 * GIB, None), 16 * GIB, "40 GiB → the 16 GiB cap");
        assert_eq!(floor_of(0, None), 4 * GIB, "an unread volume floors");
        assert_eq!(floor_of(500 * GIB, Some(12_345)), 12_345, "the override");
        assert_eq!(floor_of(500 * GIB, Some(0)), 10 * GIB, "0 is unset");
    }

    /// The volume is short only under a known floor; an unreadable free
    /// figure or floor is never short.
    #[test]
    fn the_volume_is_short_only_under_a_known_floor() {
        assert_eq!(short(Some(GIB), Some(4 * GIB)), Some(GIB));
        assert_eq!(short(Some(4 * GIB), Some(4 * GIB)), None, "at the floor");
        assert_eq!(short(None, Some(4 * GIB)), None, "unread space");
        assert_eq!(short(Some(GIB), None), None, "unread floor");
    }

    fn starved(_: &Path) -> Option<Space> {
        Some(Space {
            free: 0,
            total: 1 << 40,
        })
    }

    fn roomy(_: &Path) -> Option<Space> {
        Some(Space {
            free: u64::MAX / 2,
            total: 1 << 40,
        })
    }

    /// The pass glue over a whole cache root: a start pass under the floor
    /// sweeps and frees the idle caches but keeps a live daemon's cache;
    /// routine passes keep the minute's spacing and urgent ones the short
    /// spacing, and an urgent pass runs only under the floor.
    #[test]
    fn passes_keep_their_spacing_and_spare_live_caches() {
        let root = tempfile::tempdir().expect("cache root");
        let caches = root.path().join(CACHES);
        let now = SystemTime::now();
        // A live daemon (this process's pid) and the cache its spec names.
        let daemon = root.path().join(DAEMONS).join("live");
        fs::create_dir_all(&daemon).expect("daemon dir");
        let spec = Spec {
            sdk_root: PathBuf::from("/sdk"),
            sdk_version: "3.13.7".to_owned(),
            roots: vec![root.path().to_path_buf()],
            packages: None,
            budget: 0,
            idle_secs: 60,
            max_percent: 90,
            max_concurrent: 0,
            pool: false,
            max_roots: 0,
            warm: 0,
            warm_ttl_secs: 0,
        };
        let files = Files::of(&daemon);
        fs::write(&files.spec, serde_json::to_vec(&spec).expect("spec")).expect("spec file");
        fs::write(&files.state, std::process::id().to_string()).expect("state");
        let live = cache(&caches, &spec.cache_key(), 10, now, 5000);
        let idle = cache(&caches, "idle", 10, now, 100);

        budget(root.path(), &daemon, Pass::Start, starved);
        assert!(!idle.exists(), "a start pass under the floor frees idle caches");
        assert!(live.join(ROOTS_FILE).exists(), "the live daemon's cache stays");

        let next = cache(&caches, "next", 10, now, 100);
        budget(root.path(), &daemon, Pass::Routine, starved);
        budget(root.path(), &daemon, Pass::Urgent, starved);
        assert!(next.exists(), "both spacings hold right after a pass");

        let stamp = caches.join(".budget.stamp");
        let backdate = |age: u64| {
            fs::File::options()
                .write(true)
                .open(&stamp)
                .and_then(|file| file.set_modified(SystemTime::now() - Duration::from_secs(age)))
                .expect("backdated stamp");
        };
        backdate(10);
        budget(root.path(), &daemon, Pass::Urgent, roomy);
        assert!(next.exists(), "an urgent pass runs only under the floor");
        budget(root.path(), &daemon, Pass::Urgent, starved);
        assert!(!next.exists(), "past the short spacing an urgent pass runs");

        let kept = cache(&caches, "kept", 10, now, 100);
        backdate(120);
        budget(root.path(), &daemon, Pass::Routine, roomy);
        assert!(kept.exists(), "a routine pass with room keeps idle caches under the cap");
        assert!(live.join(ROOTS_FILE).exists());
    }

    /// Queued kicks coalesce to the most urgent pass.
    #[test]
    fn the_most_urgent_kick_wins() {
        assert_eq!(
            [Pass::Routine, Pass::Urgent, Pass::Routine]
                .into_iter()
                .fold(Pass::Routine, Pass::max),
            Pass::Urgent
        );
        assert!(due(Pass::Start, true, Duration::ZERO));
        assert!(!due(Pass::Start, false, Duration::MAX));
        assert!(!due(Pass::Routine, true, URGENT_SPACING));
    }
}
