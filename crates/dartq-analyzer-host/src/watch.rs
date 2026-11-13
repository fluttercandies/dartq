//! The watch substrate behind the daemon's per-request freshness: an OS
//! watcher feeding a journaled dirty set, plus the sync cookie that makes
//! a request's view complete.
//!
//! # Why
//!
//! [`Snapshot::take`] stats every file an analysis can read on every
//! request — the only O(repo) cost left in the daemon's answer path. The
//! watcher turns freshness into O(changed paths): a recursive OS stream
//! over the walked roots (FSEvents through the crate's own binding on
//! macOS, `notify`'s inotify watcher elsewhere) records touched paths
//! into a deduplicated journal, and each request re-stats only that set
//! (plus the handful of out-of-root config files) through
//! [`Snapshot::take_into`], the same code the full walk runs — so the
//! answer is byte-identical to walk mode, never stale.
//!
//! # Guarantees
//!
//! * **Completeness — the sync cookie.** Watcher delivery is
//!   asynchronous: an edit saved a millisecond before `dartq analyze`
//!   may still be in flight. Before reading the journal each request
//!   drops a uniquely named dot-file inside every watched tree and waits
//!   on a condvar until the watcher has delivered that cookie's event.
//!   Per-stream ordering then means every change that happened before
//!   the cookie existed is in the journal. The wait is adaptive —
//!   half the running average walk cost, clamped — and a timed-out
//!   cookie keeps listening: its late delivery proves the stream
//!   alive, only expiring undelivered counts. Repeated expiries cool
//!   the watcher off for a bounded backoff, then re-arm — a transient
//!   storm costs minutes of walking, never the daemon's life.
//!
//! * **Quiet turns — the health trust window.** The barrier is the
//!   only multi-millisecond wait an otherwise idle daemon pays, and a
//!   quiet tree pays it for nothing. Trust is earned by one proven
//!   round trip: the stream's first cookie delivery observed after the
//!   last arm or stream restart (a young stream is an unproven stream
//!   until it delivers — no age clock warms it up, because every ask
//!   re-checks the conditions that matter). With that delivery on
//!   record, no strike pending, no straggler under evaluation, the
//!   liveness proof fresh and the journal drained empty, a request
//!   serves from the journal verdict alone: it still writes the next
//!   cookies (their delivery, off the request path, is what the
//!   following request's trust keys on) and still re-stats the
//!   unwatched configs, but the fseventsd round trip leaves the answer
//!   path. A missed wait bound is latency, not doubt — the straggler
//!   it leaves behind adjudicates liveness (delivery clears, expiry
//!   strikes), and the window re-opens the ask after it delivers
//!   rather than re-earning a punctuality streak. An edit whose event
//!   has delivered is in the journal and the full path serves it; an
//!   edit still in flight defers until a cookie created after it
//!   delivers — at most one ask on a stream that keeps up with the ask
//!   cadence, and never longer than [`TRUST_WINDOW`] machinery allows,
//!   since sustained trust requires a delivery inside that window. A
//!   stream that silently stops delivering loses trust inside
//!   [`TRUST_WINDOW`]: the barrier returns, and its timeout and strike
//!   machinery with it.
//!
//! * **Fallbacks, all to the same walk.** `walk_required` is raised on
//!   the stream's rescan flags (FSEvents
//!   MustScanSubDirs/UserDropped/KernelDropped, inotify Q_OVERFLOW), on
//!   any watcher error, on journal overflow (the dirty set is capped at
//!   [`DIRTY_CAP`] paths), on cookie timeout, on any dirty path the
//!   journal cannot safely merge (outside the watched set, link paths,
//!   symlinked trees — [`Snapshot::take_into`] decides), once after the
//!   arm (the initial take predates the stream, and no substrate
//!   replays what predates its own start), and on macOS once after a
//!   changed watch set (an FSEvents stream's paths are fixed, so a new
//!   set is a new stream and the old one's in-flight events die with
//!   it). A root-set change (pool join/evict) re-plans and re-walks once.
//!
//! * **Defense in depth.** When the daemon has been idle at least
//!   [`VERIFY_IDLE`] and at most once per [`VERIFY_PERIOD`], a request
//!   runs a verification walk: the journal-maintained snapshot is
//!   compared against a fresh [`Snapshot::take`]; a mismatch is logged
//!   and the watcher cools off — a latent watcher bug becomes a perf
//!   loss, never a lasting stale view.
//!
//! * **Boundedness.** The dirty set is capped (overflow clears it and
//!   walks); the stream's thread exits with the [`Freshness`] handle;
//!   every wait is bounded.
//!
//! Symlinked directories: the walk follows them (once per canonical
//! target); the watcher arms an extra recursive watch on each target
//! the last walk recorded ([`crate::snapshot::WalkPlan::links`]), and
//! any dirty path touching a link falls back to the walk — reachability
//! under a link's spelling is never guessed.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use rustc_hash::FxHashSet;

use crate::snapshot::{Diff, Snapshot, WalkPlan, merge_target, recorded};

#[cfg(target_os = "macos")]
use fsevents::Stream;
#[cfg(not(target_os = "macos"))]
use portable::Stream;

/// The journal's dirty-path ceiling: a flood (a checkout, a build)
/// clears the set and walks once — bounded memory, never a loss.
const DIRTY_CAP: usize = 65_536;

/// The sync-cookie wait's bounds: the per-request wait is adaptive —
/// half the running EWMA of measured full-walk durations, clamped into
/// [`WAIT_MIN`]..=[`WAIT_MAX`]. A stream that delivers nothing inside
/// even the cap is not delivering; the adaptive floor keeps a slow
/// first delivery on a large tree from costing a full second plus a
/// walk every request.
const WAIT_MIN: Duration = Duration::from_millis(10);
const WAIT_MAX: Duration = Duration::from_secs(1);

/// How long a cookie that outlived its own barrier keeps listening: a
/// straggler's create OR delete event inside this window still proves
/// its stream alive (the strike count resets); only expiring
/// undelivered is evidence against the stream. Generous next to the
/// wait bound — the proof is liveness, not speed.
#[allow(dead_code)] // production-only; the tests use a shortened expiry
const LATE_EXPIRY: Duration = Duration::from_secs(10);

/// Cookies expiring undelivered in a row before the watcher cools off:
/// a stream that cannot prove ordering (a network mount without
/// events) is worse than the walk it was sparing — but it may recover,
/// so the loss is a cool-off, not the daemon's life.
const COOKIE_TIMEOUTS: u32 = 3;

/// The cool-off after an abandon: `COOL_OFF * 2^strikes` capped at
/// [`COOL_OFF_MAX`]; the strike count clears after [`HEALTHY_FOR`]
/// without one. A transient error costs minutes of walking, never the
/// daemon's life. (`HEALTHY_FOR` is a cool-off-domain constant: the
/// health trust window no longer ages a stream through it — see
/// [`Freshness::trust_ready`].)
#[allow(dead_code)] // production-only; the tests use a shortened cool-off
const COOL_OFF: Duration = Duration::from_secs(60);
const COOL_OFF_MAX: Duration = Duration::from_secs(30 * 60);
const HEALTHY_FOR: Duration = Duration::from_secs(10 * 60);

/// How fresh the stream's last proven delivery (a cookie event the
/// drain observed) must be for a request to serve from the journal
/// verdict alone. Every trusted serve writes its own cookies — their
/// delivery milliseconds later re-stamps the proof — so an engaged
/// window feeds itself while turns continue, and a stream that
/// silently stops delivering is caught within one window plus one ask
/// (the next request's barrier takes the timeout path). Five seconds:
/// far above any healthy delivery, far below the deferral a dead
/// stream could otherwise serve.
const TRUST_WINDOW: Duration = Duration::from_secs(5);

/// A late cookie's expiry deadline in production; the tests substitute
/// a shorter one so a strike is exercised in milliseconds. Absolute
/// instead of remaining — the sweep compares it against `Instant::now`.
#[cfg(not(test))]
fn late_expiry() -> Duration {
    LATE_EXPIRY
}

/// The test's late-cookie expiry — every exercised expiry is injected
/// with a back-dated deadline, so this only gates how long a REAL
/// straggler may take before counting against the stream. Long enough
/// that parallel test load never turns a slow OS delivery into a
/// spurious strike.
#[cfg(test)]
fn late_expiry() -> Duration {
    Duration::from_secs(5)
}

/// The abandon cool-off in production.
#[cfg(not(test))]
fn cool_off() -> Duration {
    COOL_OFF
}

/// The test's cool-off — a re-arm is exercised in milliseconds.
#[cfg(test)]
fn cool_off() -> Duration {
    Duration::from_millis(120)
}

/// How long the daemon must be idle before a request may pay for a

/// How long the daemon must be idle before a request may pay for a
/// verification walk — the check exists for long-lived residents, and a
/// busy daemon never sees it.
const VERIFY_IDLE: Duration = Duration::from_secs(30);

/// The minimum gap between verification walks.
const VERIFY_PERIOD: Duration = Duration::from_secs(5 * 60);

/// The cookie file's name prefix inside a watched root (dot-named: the
/// walk skips it, and even an unfiltered event for it is only ever a
/// conservative extra walk).
const COOKIE_PREFIX: &str = ".dartq-fresh-";

/// How old a stray cookie may be before the arm sweep removes it — a
/// crashed request's leftovers, never a live barrier's (one hour bounds
/// a cookie's whole life by orders of magnitude).
const COOKIE_STALE: Duration = Duration::from_secs(60 * 60);

/// One watched stream: the canonical path the OS stream is armed on and
/// the directory its sync cookie is written to — `<dir>/.dart_tool/dartq/`
/// (dartq's artifact dir, kept out of the worktree's visible root),
/// the watched dir itself only when that cannot be created. The
/// recursive watch covers the cookie either way: only the spelling
/// moves, never the stream.
struct Watch {
    dir: PathBuf,
    cookie_dir: PathBuf,
    /// Whether `dir` is a walked root — those get their
    /// `.dart_tool/dartq/` created even when `.dart_tool` does not
    /// exist yet; a link target only reuses an existing one.
    root: bool,
}

/// A sync barrier's three outcomes: every cookie delivered (the journal
/// is complete), the wait bound hit (the stream cannot prove ordering —
/// watcher-dead evidence), or a cookie could not even be created (an
/// environment fact — this request walks but the watcher stands).
#[derive(Debug, PartialEq, Eq)]
enum Sync {
    Held,
    Timeout,
    Blocked,
}

/// How the last [`Freshness::check`] answered — the observable the
/// daemon surfaces when a tree's watcher degrades to full-tree scans.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CheckDiag {
    /// journal (streamed events) or walk (full re-stat fallback).
    pub(crate) mode: CheckMode,
    /// Why a walk ran; `None` on the journal path.
    pub(crate) walk_reason: Option<WalkReason>,
    /// The answering path's duration: the walk's when one ran, else the
    /// journal pass's (barrier plus merge), milliseconds.
    pub(crate) elapsed_ms: u32,
    /// Paths the check handed the server as changed.
    pub(crate) changed: u32,
}

/// Which freshness path answered a request.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CheckMode {
    /// The journal: streamed events merged behind the sync cookie.
    #[default]
    Journal,
    /// The full stat-walk fallback.
    Walk,
}

    /// Why a check took the walk path. `First` and `Rebase` are by design —
    /// never a degradation; the rest are the shapes that make a daemon walk
    /// every request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WalkReason {
    /// A by-design covering walk: the one a macOS stream restart over a
    /// changed watch set flags (the predecessor's in-flight events die
    /// with it — construction's included, when the priming install
    /// moves the stream to the discovered surface).
    First,
    /// The sync cookie could not be created (an unwritable tree) — an
    /// environment fact, not watcher evidence.
    CookieBlocked,
    /// The sync cookie was not delivered inside the adaptive wait bound.
    CookieTimeout,
    /// The stream flagged lost events (a rescan flag, a stream error,
    /// journal overflow) — the journal can no longer prove completeness.
    StreamLost,
    /// The watcher is not armed (never armed, cooled off, or abandoned).
    Unarmed,
    /// The served root set changed (pool join/evict) — re-plan and
    /// re-walk once.
    Rebase,
}

/// A duration in whole milliseconds, saturating rather than wrapping past
/// the u32 range (a 49-day walk reads as the cap, never as zero).
fn elapsed_ms(duration: Duration) -> u32 {
    u32::try_from(duration.as_millis()).unwrap_or(u32::MAX)
}

/// A diff's changed-path count, saturating rather than wrapping.
fn count_changed(diff: &Diff) -> u32 {
    u32::try_from(diff.changed.len()).unwrap_or(u32::MAX)
}

/// The cookie directory of one watched dir: `<dir>/.dart_tool/dartq/`
/// when it exists or can be created — a walked `root` creates the tree
/// outright (every Dart template gitignores `.dart_tool`, and dartq's
/// own session layout already lives there), a link target only reuses
/// an existing `.dart_tool` — else `dir` itself. Sweeping stale
/// `.dartq-fresh-*` files in the chosen dir clears a crashed request's
/// leftovers.
fn cookie_dir(dir: &Path, root: bool) -> PathBuf {
    let tool = dir.join(".dart_tool").join("dartq");
    let chosen = if tool.is_dir()
        || ((root || dir.join(".dart_tool").is_dir()) && fs::create_dir_all(&tool).is_ok())
    {
        tool
    } else {
        dir.to_path_buf()
    };
    if let Ok(entries) = fs::read_dir(&chosen) {
        let stale = std::time::SystemTime::now() - COOKIE_STALE;
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_str().is_some_and(|name| name.starts_with(COOKIE_PREFIX))
                && entry
                    .metadata()
                    .and_then(|meta| meta.modified())
                    .is_ok_and(|modified| modified < stale)
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    chosen
}

/// The shared journal between the drain thread and the request thread.
/// `dirty` is deduplicated; `walk` marks any loss or unsafe input;
/// `cookies` holds the pending sync-cookie paths — the drain thread
/// removes each as its event arrives, and a request's barrier ends when
/// its own cookies are all gone. `late` holds the cookie file NAMES of
/// barriers that already timed out — a straggler's create or delete
/// event inside its expiry still proves the stream alive. `proven` is
/// the instant a cookie's event was last observed delivered — the
/// liveness proof the health trust window keys on, stamped by the
/// drain thread on any cookie-name event.
struct Journal {
    dirty: FxHashSet<PathBuf>,
    walk: bool,
    cookies: FxHashSet<PathBuf>,
    late: FxHashSet<String>,
    proven: Option<Instant>,
}

impl Journal {
    fn dirty_path(&mut self, path: PathBuf) {
        if self.dirty.len() >= DIRTY_CAP {
            self.dirty.clear();
            self.walk = true;
        } else {
            self.dirty.insert(path);
        }
    }
}

/// The drain-thread state: the journal, the condvar cookie waiters
/// block on, the canonical→walked spelling map events are normalized
/// through, and the watched-path set the sync barrier drops cookies in.
struct Shared {
    journal: Mutex<Journal>,
    delivered: Condvar,
    /// `(canonical watched path, plan spelling)` pairs — the watcher is
    /// armed on canonicalized trees (the OS resolves them anyway) while
    /// the snapshot speaks the plan's spelling; event paths are rewritten
    /// through the longest matching canonical prefix.
    map: Mutex<Vec<(PathBuf, PathBuf)>>,
    /// Every stream the watcher carries — walked dirs plus the symlink
    /// targets' canonical paths, each with the dir its sync cookie is
    /// written to. The sync barrier drops one cookie per stream:
    /// per-stream ordering proves a delivered cookie, so all cookies
    /// landing means every earlier change on every stream is in the
    /// journal.
    watched: Mutex<Vec<Watch>>,
}

impl Shared {
    /// The walked-set spelling of an event path: the longest watched
    /// canonical prefix is swapped back to the plan's own spelling (the
    /// daemon's roots arrive canonicalized, so most events pass
    /// through unchanged — the map exists for the walk's non-canonical
    /// package roots and the recorded symlink targets).
    fn normalize(&self, path: &Path) -> PathBuf {
        let map = self.map.lock();
        let mut best: Option<&(PathBuf, PathBuf)> = None;
        for pair in map.iter() {
            if path.starts_with(&pair.0)
                && best.is_none_or(|b| pair.0.as_os_str().len() > b.0.as_os_str().len())
            {
                best = Some(pair);
            }
        }
        match best {
            Some((canonical, spelling)) => {
                let rest = path.strip_prefix(canonical).expect("prefix checked");
                spelling.join(rest)
            }
            None => path.to_path_buf(),
        }
    }

    /// Folds one batch of delivered paths into the journal — the one
    /// fold both substrates share: a cookie name resolves its pending
    /// barrier (a timed-out barrier's name still proves the stream
    /// alive — its create OR delete event delivered the straggler) and
    /// stamps the liveness proof; every other path joins the dirty set
    /// (capped). Wakes the waiters.
    fn record<'a>(&self, paths: impl IntoIterator<Item = &'a Path>) {
        let mut journal = self.journal.lock();
        for path in paths {
            if let Some(name) = cookie_name(path) {
                journal.late.remove(&name);
                journal.cookies.remove(path);
                journal.proven = Some(Instant::now());
            } else {
                journal.dirty_path(self.normalize(path));
            }
        }
        drop(journal);
        self.delivered.notify_all();
    }

    /// The stream lost events (a rescan flag, a stream error, a dead
    /// stream): the journal can no longer prove completeness, so the
    /// next request walks.
    fn lost(&self) {
        self.journal.lock().walk = true;
        self.delivered.notify_all();
    }
}

/// Whether a path is one of ours: sync cookies resolve pending barriers
/// and never join the dirty set (even an unfiltered one would only be a
/// conservative extra walk — the name is a dot-file the walk skips).
/// The cookie file name a path carries — names, not paths: a barrier
/// writes every stream's cookie with the same name in its own dir, and
/// any stream's delivery of that name is the liveness proof.
fn cookie_name(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| name.starts_with(COOKIE_PREFIX))
        .map(str::to_owned)
}

/// The watcher generation counter — every spawned stream takes the
/// next id, so the tests can prove a walk never respawns.
#[cfg(test)]
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// The watcher handle: ONE OS [`Stream`] kept for the [`Freshness`]'s
/// life — a walk or rebase reconciles its watch set in place instead of
/// rebuilding the watcher, so a re-arm costs the delta, not a thread
/// teardown. Dropping it stops the stream and joins its thread — no
/// leaked threads or fds outlive the daemon.
struct Watcher {
    /// The live stream; `reconcile` moves its watch set.
    stream: Stream,
    /// A unique id per spawned stream — the tests' evidence that a
    /// walk or rebase reconciles the ONE long-lived watcher instead of
    /// rebuilding it (the generation never changes under `check`).
    #[cfg(test)]
    stream_id: u64,
    /// The watch-call error count — the deterministic invariant the
    /// generation tests assert against: the stream id changes ONLY
    /// when a reconcile recorded an error, never spuriously.
    #[cfg(test)]
    errors: u64,
}

impl Watcher {
    /// Starts the stream's thread — the only place a [`Stream`] is
    /// built; it watches nothing until the first `reconcile`. `None`
    /// when the stream or its thread cannot start; the daemon then walks
    /// every request.
    fn spawn(shared: &Arc<Shared>) -> Option<Self> {
        Some(Self {
            stream: Stream::spawn(shared)?,
            #[cfg(test)]
            stream_id: GENERATION.fetch_add(1, AtomicOrdering::Relaxed),
            #[cfg(test)]
            errors: 0,
        })
    }

    /// Brings the watch set to `plan`'s surface on the live stream:
    /// every walked dir (canonicalized) plus every symlinked-dir
    /// canonical target outside them. The stream moves to the new set
    /// ([`Watcher::apply`]), and the shared canonical→spelling map and
    /// cookie dirs are rewritten. `Failed` on any stream error — partial
    /// coverage is worse than none (a silent hole would serve stale).
    fn reconcile(&mut self, plan: &WalkPlan, shared: &Arc<Shared>) -> Arm {
        // The target set: walked dirs' canonical spellings (the OS
        // reports canonical) plus link targets, deduped by prefix
        // coverage — a candidate is covered iff its immediate sorted
        // predecessor is a prefix. `root` marks walked dirs — only
        // their cookie dir may create `.dart_tool` outright.
        let mut targets: Vec<(PathBuf, PathBuf, bool)> = Vec::new();
        for root in &plan.walked {
            if root.is_dir() {
                let canonical = fs::canonicalize(root).unwrap_or_else(|_| root.clone());
                targets.push((canonical, root.clone(), true));
            }
        }
        for (canonical, _) in &plan.links {
            if canonical.is_dir() {
                targets.push((canonical.clone(), canonical.clone(), false));
            }
        }
        targets.sort_by(|a, b| a.0.cmp(&b.0));
        let mut want: Vec<PathBuf> = Vec::new();
        let mut map: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut roots: Vec<bool> = Vec::new();
        for (canonical, spelling, root) in targets {
            match want.binary_search_by(|dir| dir.as_path().cmp(canonical.as_path())) {
                Ok(_) => continue,
                Err(index) if index > 0 && canonical.starts_with(&want[index - 1]) => continue,
                _ => {}
            }
            let index = want.partition_point(|dir| dir < &canonical);
            want.insert(index, canonical.clone());
            map.push((canonical, spelling));
            roots.push(root);
        }
        // The dirs the live stream carries now — empty for a fresh
        // watcher (a cool-off's re-arm included: the abandon cleared the
        // lost stream's set).
        let have: Vec<PathBuf> = shared.watched.lock().iter().map(|w| w.dir.clone()).collect();
        // macOS FSEvents only enrolls directories that exist when the
        // stream starts; a cookie dir created after its root was armed
        // would never report the barrier's writes, timing every sync
        // out. Create every cookie dir BEFORE the stream is touched so
        // the (re)started stream covers them from birth.
        let cookie_dirs: Vec<PathBuf> = want
            .iter()
            .zip(roots.iter())
            .map(|(dir, root)| cookie_dir(dir, *root))
            .collect();
        let Some(dropped) = self.apply(&have, &want, shared) else {
            return Arm::Failed;
        };
        let mut watched: Vec<Watch> = Vec::new();
        let mut kept: Vec<PathBuf> = Vec::new();
        for (index, (dir, root)) in want.iter().zip(roots.iter()).enumerate() {
            if dropped.contains(dir) {
                continue;
            }
            kept.push(dir.clone());
            watched.push(Watch {
                cookie_dir: cookie_dirs[index].clone(),
                dir: dir.clone(),
                root: *root,
            });
        }
        let map = map
            .into_iter()
            .filter(|(canonical, _)| kept.contains(canonical))
            .collect();
        *shared.map.lock() = map;
        *shared.watched.lock() = watched;
        if dropped.is_empty() {
            Arm::Ok
        } else {
            let mut journal = shared.journal.lock();
            for dir in dropped {
                let inside = plan
                    .walked
                    .iter()
                    .any(|root| root.is_dir() && dir.starts_with(root));
                match (inside, dir.parent()) {
                    (true, Some(parent)) => journal.dirty_path(parent.to_path_buf()),
                    _ => journal.walk = true,
                }
            }
            Arm::Dropped
        }
    }

    /// macOS: moves the ONE FSEvents stream from `have` to `want`.
    /// Returns the wanted dirs that vanished since the plan (dropped,
    /// not stream evidence — FSEvents would take a missing path without
    /// complaint, so the check is ours), `None` on a stream failure. An
    /// unchanged set leaves the stream alone.
    #[cfg(target_os = "macos")]
    fn apply(&mut self, have: &[PathBuf], want: &[PathBuf], shared: &Shared) -> Option<Vec<PathBuf>> {
        let (kept, dropped): (Vec<PathBuf>, Vec<PathBuf>) = want
            .iter()
            .cloned()
            .partition(|dir| have.contains(dir) || dir.exists());
        for dir in &dropped {
            eprintln!("dartq-analyzer-host: watch: {} vanished — dropping it", dir.display());
        }
        if kept.len() == have.len() && kept.iter().all(|dir| have.contains(dir)) {
            return Some(dropped);
        }
        if let Err(error) = self.stream.watch(kept) {
            #[cfg(test)]
            {
                self.errors += 1;
            }
            eprintln!("dartq-analyzer-host: watch stream failed: {error}");
            return None;
        }
        // An FSEvents stream's paths are fixed (the public API has no
        // scope update), so the new set is a new stream: it starts
        // before its predecessor stops, yet whatever the predecessor
        // still had in flight dies with it — events for roots already
        // watched are lost inside that window. The next request walks
        // once; that walk's own reconcile is a no-op and the stream
        // converges.
        if !have.is_empty() {
            shared.lost();
        }
        Some(dropped)
    }

    /// Elsewhere: one recursive `notify` watch per dir, reconciled in
    /// place — unwatch what left the set, watch what joined, never both
    /// ends of an unchanged dir (inotify adds and removes watches
    /// without disturbing the others). Returns the wanted dirs that
    /// vanished before their watch could arm, `None` on a real watch
    /// failure.
    #[cfg(not(target_os = "macos"))]
    fn apply(&mut self, have: &[PathBuf], want: &[PathBuf], _shared: &Shared) -> Option<Vec<PathBuf>> {
        use notify::Watcher as _;
        for dir in have.iter().filter(|dir| !want.contains(dir)) {
            if let Err(error) = self.stream.watcher.unwatch(dir) {
                #[cfg(test)]
                {
                    self.errors += 1;
                }
                eprintln!("dartq-analyzer-host: unwatch {} failed: {error}", dir.display());
            }
        }
        let mut dropped: Vec<PathBuf> = Vec::new();
        for dir in want.iter().filter(|dir| !have.contains(dir)) {
            if let Err(error) = self.stream.watcher.watch(dir, notify::RecursiveMode::Recursive) {
                if !dir.exists() {
                    // The target vanished between the plan and the arm
                    // — not stream evidence. Drop it and keep the rest
                    // armed; the next plan refresh restores it. Its
                    // parent joins the dirty set (or the request walks
                    // when outside every walked root) so the removal is
                    // re-covered.
                    eprintln!(
                        "dartq-analyzer-host: watch: {} vanished — dropping it",
                        dir.display()
                    );
                    dropped.push(dir.clone());
                    continue;
                }
                #[cfg(test)]
                {
                    self.errors += 1;
                }
                eprintln!("dartq-analyzer-host: watch {} failed: {error}", dir.display());
                return None;
            }
        }
        Some(dropped)
    }
}

/// Reconcile's three outcomes: the stream matches the plan's surface;
/// the stream matches except targets that vanished between the plan
/// and the arm (dropped, their parents flagged for a re-covering
/// merge — the next plan refresh restores them); or a real stream
/// failure — the caller cools off, partial coverage being worse than
/// none.
#[derive(Debug)]
enum Arm {
    Ok,
    /// True stream failure — partial coverage would serve stale.
    Failed,
    /// Watch calls failed only for targets that no longer exist: they
    /// were dropped; each dropped target's parent dir joined `dirty`
    /// when inside a walked root, else the request walks.
    Dropped,
}

/// The background first take's landing cell: one result, consumed by
/// exactly one [`Freshness::settle_priming`]. The outer `Option` is the
/// arrival (the thread landed or not yet); the inner one carries a
/// failed take (`None` — the take panicked mid-walk), which the
/// settling check answers with an inline walk.
#[derive(Default)]
struct Priming {
    state: Mutex<Option<Option<Primed>>>,
    done: Condvar,
}

/// One completed first take: the baseline snapshot, its companion plan,
/// and the take's own duration (the adaptive wait bound's seed).
struct Primed {
    snapshot: Snapshot,
    plan: WalkPlan,
    take: Duration,
}

impl Priming {
    /// The thread's side: publish the take — or its failure — and wake
    /// the settling check.
    fn land(&self, primed: Option<Primed>) {
        *self.state.lock() = Some(primed);
        self.done.notify_all();
    }

    /// The settling check's side: block until the thread lands.
    fn wait(&self) -> Option<Primed> {
        let mut state = self.state.lock();
        loop {
            if let Some(primed) = state.take() {
                return primed;
            }
            self.done.wait(&mut state);
        }
    }
}

/// The daemon's freshness oracle: the maintained [`Snapshot`], its
/// companion [`WalkPlan`], the journaled dirty set, and the watcher —
/// or none, when the daemon runs in walk mode (watcher unarmed or
/// abandoned). `check` answers the same [`Diff`] the previous snapshot
/// would diff against a fresh [`Snapshot::take`], at O(changed paths)
/// plus one sync barrier.
pub(crate) struct Freshness {
    snapshot: Snapshot,
    plan: WalkPlan,
    shared: Arc<Shared>,
    watcher: Option<Watcher>,
    /// The first take, running on its own thread since construction
    /// armed the stream over the pre-plan surface: the daemon's cold ask
    /// never waits for it (its diff is dead weight with no live
    /// server), and the first `check` installs it. `None` once
    /// installed; the cell always exists at construction (a thread that
    /// cannot start lands the take inline before construction returns).
    priming: Option<Arc<Priming>>,
    /// Whether the baseline is installed — the daemon's cold arm reads
    /// this to skip a check whose diff no answer observes.
    primed: bool,
    /// Config files re-stat'ed every request because no watch covers
    /// them (out-of-root ancestors) plus the walked set's non-directory
    /// entries (a file root is its own re-stat).
    unwatched: Vec<PathBuf>,
    /// Directories the last merge subtree-walked, held dirty for
    /// exactly one more request: a new dir's own recursive watch may
    /// only be armed after its first merge (inotify adds it once the
    /// dispatch batch drains), so a file created in that window would
    /// never be reported.
    carry_dirs: Vec<PathBuf>,
    /// Cookies that outlived their own barrier — the names are kept
    /// until delivery or expiry: a straggler's event proves its stream
    /// alive (and resets `timeouts`), a cookie expiring undelivered is
    /// the strike. Names, not paths: any stream's delivery of the name
    /// is the proof.
    late: Vec<(String, Instant)>,
    /// Consecutive expired late cookies — [`COOKIE_TIMEOUTS`] in a row
    /// cools the watcher off (it may recover; the cool-off re-arms).
    timeouts: u32,
    /// Cool-off state: the watcher was lost (`watcher` is `None`) and
    /// re-arms when `since + COOL_OFF * 2^strikes` (capped
    /// [`COOL_OFF_MAX`]) passes. `strikes` grows per consecutive
    /// abandon and clears after [`HEALTHY_FOR`] without one.
    cool_off: Option<(Instant, u32)>,
    /// The instant the cool-off was entered — the healthy clock's
    /// baseline. `None` while armed.
    since: Option<Instant>,
    /// The instant of the last arm or stream restart — the doubt
    /// boundary a liveness proof must postdate before the trust window
    /// may open. A stream's first delivery observed after it proves
    /// that (new) stream; a cookie that merely misses its wait bound
    /// does NOT move it (lateness adjudicates through the straggler
    /// set, and the straggler's delivery re-stamps the proof on the
    /// far side of the unchanged boundary).
    health_since: Option<Instant>,
    /// The last trusted serve's cookie files — unlinked (and their
    /// pending registrations dropped) by the next request, whose own
    /// barrier or trusted cookies replace them.
    pending_cookies: Vec<PathBuf>,
    /// How many requests served from the journal verdict alone — the
    /// tests' and measurements' evidence the window engaged.
    trusted: usize,
    /// The adaptive sync-wait bound: `0.5 * EWMA(walk)` clamped into
    /// [`WAIT_MIN`]..=[`WAIT_MAX`], seeded by the first measured walk —
    /// nanos, kept fractional so the average never floors to zero.
    wait_ns: f64,
    /// Walk mode forever: stream dead or timeout cap reached.
    abandoned: bool,
    /// The cookie sequence counter — unique per request, so a stale
    /// event can never resolve a later barrier.
    seq: u64,
    /// The last request's finish — the verification walk's idle clock.
    last_request: Instant,
    /// The last verification walk.
    last_verify: Instant,
    /// Verification mismatch count, logged once per occurrence.
    mismatches: u64,
    /// How many requests took the full-walk path — the tests' evidence
    /// a fallback fired (and the measurements' walk counter).
    walks: usize,
    /// How the last `check` answered — one Copy write per request, read
    /// by the daemon to make a degraded watcher visible.
    last_check: CheckDiag,
    /// A by-design covering walk is pending: a macOS stream restart over
    /// a changed set (the predecessor's in-flight events die with it —
    /// including the priming install's own move to the discovered
    /// surface). The walk-flag fallback it causes reports
    /// [`WalkReason::First`] instead of [`WalkReason::StreamLost`]. Any
    /// full take consumes it.
    first_walk: bool,
    /// Test hook: the journal's inputs arrive by injection, not by
    /// stream proofs — the sync barrier resolves at once. A saturated
    /// `fseventsd` delays delivery past any barrier bound (seconds were
    /// measured on loaded hosts), so a barrier may never hold there; the
    /// merge-and-carry logic a test exercises this way does not depend
    /// on live delivery.
    #[cfg(test)]
    injected_journal: bool,
}

impl Freshness {
    /// Builds the freshness oracle over `roots`: the watcher armed over
    /// the discovery-free surface FIRST — every event from here on is
    /// journaled — then the first take walked on its own thread. The
    /// daemon's cold ask answers from the server's own full analysis
    /// while that walk runs; the first `check` installs its result as
    /// the baseline and moves the watch to the surface the walk
    /// discovered. A watcher that cannot arm leaves the daemon in walk
    /// mode (the baseline still primes — the first armed-server check
    /// settles it); a priming thread that cannot start takes inline
    /// here, the serial construction the daemon always paid, never a
    /// worse one.
    pub(crate) fn new(roots: &[PathBuf], packages: Option<&Path>, sdk_home: &Path) -> Self {
        let shared = Arc::new(Shared {
            journal: Mutex::new(Journal {
                dirty: FxHashSet::default(),
                walk: false,
                cookies: FxHashSet::default(),
                late: FxHashSet::default(),
                proven: None,
            }),
            delivered: Condvar::new(),
            map: Mutex::new(Vec::new()),
            watched: Mutex::new(Vec::new()),
        });
        let mut watcher = Watcher::spawn(&shared);
        let plan = Snapshot::preplan(roots, packages, sdk_home);
        let armed = watcher
            .as_mut()
            .is_some_and(|watcher| {
                let armed = !matches!(watcher.reconcile(&plan, &shared), Arm::Failed);
                armed
            });
        if !armed {
            watcher = None;
        }
        // The priming thread: one full take, concurrent with the
        // daemon's serving. `catch_unwind` keeps a panicking take from
        // wedging the first check — a `None` landing settles inline.
        // A thread that cannot start lands the take right here: the
        // serial construction the daemon always paid, never a worse
        // one (the first check installs it like any other landing).
        let priming = Arc::new(Priming::default());
        let spawned = std::thread::Builder::new()
            .name("dartq-analyzer-prime".to_owned())
            .spawn({
                let cell = Arc::clone(&priming);
                let roots = roots.to_vec();
                let packages = packages.map(Path::to_path_buf);
                let sdk_home = sdk_home.to_path_buf();
                move || {
                    let take = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let start = Instant::now();
                        let primed =
                            Snapshot::take_with_plan(&roots, packages.as_deref(), &sdk_home);
                        (primed, start.elapsed())
                    }))
                    .ok()
                    .map(|((snapshot, plan), take)| Primed { snapshot, plan, take });
                    cell.land(take);
                }
            });
        if let Err(error) = spawned {
            eprintln!("dartq-analyzer-host: watch: priming thread unavailable: {error}");
            let start = Instant::now();
            let (snapshot, plan) = Snapshot::take_with_plan(roots, packages, sdk_home);
            priming.land(Some(Primed {
                snapshot,
                plan,
                take: start.elapsed(),
            }));
        }
        let unwatched = unwatched_of(&plan);
        Self {
            snapshot: Snapshot::default(),
            plan,
            shared,
            watcher,
            priming: Some(priming),
            primed: false,
            unwatched,
            carry_dirs: Vec::new(),
            late: Vec::new(),
            timeouts: 0,
            cool_off: if armed {
                None
            } else {
                Some((Instant::now(), 0))
            },
            since: (!armed).then(Instant::now),
            health_since: armed.then(Instant::now),
            pending_cookies: Vec::new(),
            trusted: 0,
            // The adaptive bound seeds at install (the priming take's
            // measured duration, discounted as before) — a journal
            // check never runs before then.
            wait_ns: 1.0,
            abandoned: false,
            seq: 0,
            last_request: Instant::now(),
            last_verify: Instant::now(),
            mismatches: 0,
            walks: 0,
            last_check: CheckDiag::default(),
            first_walk: false,
            #[cfg(test)]
            injected_journal: false,
        }
    }

    /// Whether the watcher currently covers the walked trees — the
    /// journal path is only safe when every `plan.walked` directory is
    /// streamed.
    fn armed(&self) -> bool {
        self.watcher.is_some() && !self.abandoned
    }

    /// Whether the first take's baseline is installed — the daemon's
    /// cold arm reads this: with no live server, a check before the
    /// baseline exists is dead weight (any diff starts the server over
    /// the same roots the same way), so the first ask answers without
    /// waiting for the background walk to land.
    pub(crate) fn primed(&self) -> bool {
        self.primed
    }

    /// The analyzed Dart source size once the baseline is installed —
    /// the ledger's sizing figure off the walk the oracle already paid,
    /// never a second one.
    pub(crate) fn dart_bytes(&self) -> Option<u64> {
        self.primed.then(|| self.snapshot.dart_bytes())
    }

    /// Installs the background first take, exactly once — the first
    /// `check`'s opening step. A failed landing (the take panicked)
    /// walks right here: the serial construction the daemon always
    /// paid.
    fn settle_priming(&mut self) {
        let Some(priming) = self.priming.take() else {
            return;
        };
        let primed = priming.wait();
        self.primed = true;
        let (snapshot, plan, take) = match primed {
            Some(primed) => (primed.snapshot, primed.plan, primed.take),
            None => {
                let roots = self.plan_roots();
                let packages = self.plan_packages();
                let sdk = self.plan_sdk_home();
                let start = Instant::now();
                let (snapshot, plan) =
                    Snapshot::take_with_plan(&roots, packages.as_deref(), &sdk);
                (snapshot, plan, start.elapsed())
            }
        };
        self.install(snapshot, plan, take);
    }

    /// Makes a completed first take the oracle's state: the baseline
    /// snapshot and its companion plan, the adaptive wait bound seeded
    /// from the measured take (discounted — it is planning too, which
    /// later walks never pay), and the watcher moved to the surface the
    /// take discovered. Dirs that joined the watch set were unwatched
    /// while the take read them, so they are marked dirty: the next
    /// merge re-covers that window the way a fresh arm's covering walk
    /// always did, without the walk (a macOS stream move flags its own
    /// covering walk; either shape is the same correctness).
    fn install(&mut self, snapshot: Snapshot, plan: WalkPlan, take: Duration) {
        self.wait_ns = (take.as_nanos() as f64 / 2.0).clamp(1.0, f64::MAX);
        self.snapshot = snapshot;
        self.plan = plan;
        self.carry_dirs.clear();
        let before = self.covered();
        self.reconcile();
        if self.armed() {
            self.since = None;
        }
        self.unwatched = unwatched_of(&self.plan);
        self.mark_newly_covered(&before);
    }

    /// The EWMA's half-life in walks: ~4 samples to halve a stale
    /// bound — quick to follow a growing tree, immune to one outlier.
    const EWMA_ALPHA: f64 = 0.25;

    /// Feeds one measured full-walk duration into the wait bound.
    fn observe_walk(&mut self, duration: Duration) {
        self.wait_ns += (duration.as_nanos() as f64 - self.wait_ns) * Self::EWMA_ALPHA;
    }

    /// This request's sync-cookie wait: half the running walk average,
    /// clamped — proportional to what the fallback it triggers costs.
    fn wait_bound(&self) -> Duration {
        Duration::from_nanos((self.wait_ns / 2.0).clamp(
            WAIT_MIN.as_nanos() as f64,
            WAIT_MAX.as_nanos() as f64,
        ) as u64)
    }

    /// Sweeps the late set: entries whose expiry passed undelivered are
    /// strikes (the stream proved dead); the drain clears delivered
    /// names, so what remains past expiry never arrived. Late delivery
    /// in the same sweep resets the strike count.
    fn sweep_late(&mut self) {
        let now = Instant::now();
        // The drain removes a delivered name from `journal.late`; a name
        // still present at the sweep is still in flight. Delivered
        // entries drop their deadlines and clear the strike count —
        // undelivered-and-expired ones strike.
        let pending = {
            let journal = self.shared.journal.lock();
            self.late
                .iter()
                .map(|(name, _)| journal.late.contains(name))
                .collect::<Vec<bool>>()
        };
        let mut pending = pending.into_iter();
        let mut strikes = 0u32;
        let mut alive = false;
        self.late.retain(|(_, deadline)| {
            if !pending.next().unwrap_or(false) {
                // Its name left `journal.late` — the straggler's event
                // arrived: the stream is alive.
                alive = true;
                return false;
            }
            if now >= *deadline {
                strikes += 1;
                return false;
            }
            true
        });
        if alive {
            self.timeouts = 0;
        }
        if strikes > 0 {
            self.timeouts += strikes;
            if self.timeouts >= COOKIE_TIMEOUTS {
                self.abandon("sync cookies expired undelivered");
            }
        }
    }

    /// Brings the watch set to `self.plan`'s surface on the ONE
    /// long-lived stream: reconcile the delta in place, spawn only when
    /// none exists (initial arm or a cool-off's re-arm — the caller's
    /// own take is the re-arm's covering walk). A reconcile failure
    /// cools the watcher off — partial coverage serves stale; a vanished
    /// target is dropped inside `Watcher::reconcile`, which flags its
    /// re-coverage.
    fn reconcile(&mut self) {
        if self.abandoned {
            return;
        }
        // A cooled-off stream is only re-tried once its backoff passed —
        // a hot cool-off still waits.
        if self.watcher.is_none()
            && let Some((since, _)) = self.cool_off
            && since.elapsed() < cool_off()
        {
            return;
        }
        if self.watcher.is_none() && self.cool_off.is_some() {
            self.cool_off = None;
            eprintln!("dartq-analyzer-host: watch: cool-off elapsed — re-arming");
        }
        // macOS only: the ONE FSEvents stream restarts whenever its path
        // set moves, and the predecessor's in-flight events die with it
        // (see `Watcher::apply`) — the walk flag that raises covers a
        // by-design window, like the arm's own first walk.
        #[cfg(target_os = "macos")]
        let before: Vec<PathBuf> = self
            .shared
            .watched
            .lock()
            .iter()
            .map(|w| w.dir.clone())
            .collect();
        // A spawn (initial arm or a cool-off's re-arm) is an unproven
        // stream: its doubt boundary begins here — the trust window
        // opens only after a delivery observed on the new stream.
        let spawned = self.watcher.is_none();
        let failed = match self.watcher.as_mut() {
            Some(watcher) => watcher.reconcile(&self.plan, &self.shared),
            None => {
                self.watcher = Watcher::spawn(&self.shared);
                self.watcher
                    .as_mut()
                    .map(|watcher| watcher.reconcile(&self.plan, &self.shared))
                    .unwrap_or(Arm::Failed)
            }
        };
        #[cfg(target_os = "macos")]
        if !before.is_empty() && !matches!(failed, Arm::Failed) {
            let after: Vec<PathBuf> = self
                .shared
                .watched
                .lock()
                .iter()
                .map(|w| w.dir.clone())
                .collect();
            if after != before {
                self.first_walk = true;
                // The restarted stream is a new stream: its doubt
                // boundary — and with it the trust window's proof
                // obligation — begins now.
                self.health_since = Some(Instant::now());
            }
        }
        if spawned && self.watcher.is_some() {
            self.health_since = Some(Instant::now());
        }
        if matches!(failed, Arm::Failed) {
            self.abandon("a watch call failed");
        }
    }

    /// Drops the watcher and enters the cool-off — a transient stream
    /// loss (three expired cookies, a watch error, a verification
    /// mismatch) costs `COOL_OFF * 2^strikes` of walking, never the
    /// daemon's life. The strike count resets after [`HEALTHY_FOR`]
    /// without an abandon.
    fn abandon(&mut self, cause: &str) {
        self.watcher = None;
        // The stream is gone: nothing is covered and no cookie can land.
        // The re-arm's reconcile then starts its fresh stream on the
        // whole surface — a stale set would read as "already watched"
        // and arm nothing — and the walk around it marks every armed
        // dir newly covered.
        self.shared.watched.lock().clear();
        self.shared.map.lock().clear();
        let healthy = self.since.is_none_or(|since| since.elapsed() >= HEALTHY_FOR);
        let strikes = match self.cool_off {
            Some((_, strikes)) if !healthy => strikes + 1,
            _ => 0,
        };
        let backoff = cool_off()
            .saturating_mul(1u32 << strikes.min(5))
            .min(COOL_OFF_MAX);
        eprintln!(
            "dartq-analyzer-host: watch: {cause} — walking for {:?}",
            backoff
        );
        self.cool_off = Some((Instant::now(), strikes));
        self.since = Some(Instant::now());
        self.timeouts = 0;
        self.late.clear();
        self.shared.journal.lock().late.clear();
    }

    /// The per-request freshness pass: rebase on a changed root set,
    /// then either the journal path (cookie barrier + dirty merge) or
    /// the full walk. When the idle verification is due it runs at a
    /// synchronized point — AFTER this request's own merge, behind a
    /// second barrier — so an edit in flight is "inconclusive, retry",
    /// never a phantom divergence. Returns the [`Diff`] against the
    /// previous request's snapshot.
    pub(crate) fn check(&mut self, roots: &[PathBuf]) -> Diff {
        self.settle_priming();
        self.sweep_pending_cookies();
        let due = roots == self.plan.roots()
            && self.armed()
            && self.last_request.elapsed() >= VERIFY_IDLE
            && self.last_verify.elapsed() >= VERIFY_PERIOD;
        // The pre-merge snapshot: a conclusive verification mismatch
        // answers `previous.diff(fresh)` — the request still gets the
        // truth the walk saw.
        let previous = due.then(|| self.snapshot.clone());
        let mut diff = if roots != self.plan.roots() {
            self.rebase(roots)
        } else if self.armed() {
            self.journal_check()
        } else {
            // A cooled-off stream whose backoff passed re-arms inside
            // `reconcile` — the walk that follows is its covering walk.
            self.reconcile();
            self.walk_check(WalkReason::Unarmed)
        };
        if let Some(previous) = previous {
            self.last_verify = Instant::now();
            if self.armed()
                && let Some(answer) = self.verify_at_sync(&previous)
            {
                diff = answer;
            }
        }
        self.last_request = Instant::now();
        diff
    }

    /// The dirs the watcher streams right now — the coverage the take
    /// runs under.
    fn covered(&self) -> FxHashSet<PathBuf> {
        self.shared
            .watched
            .lock()
            .iter()
            .map(|w| w.dir.clone())
            .collect()
    }

    /// Marks every newly watched dir dirty: a stream armed since
    /// `before` covers a window the events never saw (the walk ran
    /// without it), so the next request re-walks it. Idempotent — a
    /// re-stat under a covered stream is a no-op.
    fn mark_newly_covered(&self, before: &FxHashSet<PathBuf>) {
        let mut journal = self.shared.journal.lock();
        for dir in self
            .shared
            .watched
            .lock()
            .iter()
            .map(|w| w.dir.clone())
            .filter(|dir| !before.contains(dir))
        {
            journal.dirty_path(dir);
        }
    }

    /// The walk path: one full take over the current plan's roots, the
    /// diff against it, and the watcher re-armed over whatever surface
    /// the take discovered (config moves rename the walked set).
    /// Events stay ordered — the journal drains BEFORE the take (its
    /// entries are the take's own truth), never after: a change landing
    /// mid-walk stays in `dirty` for the next request instead of being
    /// erased under a snapshot that may predate it. Streams armed only
    /// now were unwatched while the take ran — their dirs are marked
    /// dirty so the next request re-covers the window. The take's own
    /// duration feeds the adaptive wait bound, and reaching the walk
    /// proves the daemon alive — the cool-off health clock restarts.
    /// `reason` names the fallback that chose the walk — recorded in
    /// [`Freshness::last_check`] so a degraded tree is observable.
    fn walk_check(&mut self, reason: WalkReason) -> Diff {
        // Any full take covers the construction window, so the arm's
        // by-design first walk is spent whatever arm reached here.
        self.first_walk = false;
        self.walks += 1;
        let before = self.covered();
        {
            let mut journal = self.shared.journal.lock();
            journal.dirty.clear();
            journal.walk = false;
        }
        let roots = self.plan_roots();
        let packages = self.plan_packages();
        let sdk = self.plan_sdk_home();
        let start = Instant::now();
        let (fresh, plan) = Snapshot::take_with_plan(&roots, packages.as_deref(), &sdk);
        let elapsed = start.elapsed();
        self.observe_walk(elapsed);
        let diff = self.snapshot.diff(&fresh);
        self.last_check = CheckDiag {
            mode: CheckMode::Walk,
            walk_reason: Some(reason),
            elapsed_ms: elapsed_ms(elapsed),
            changed: count_changed(&diff),
        };
        self.snapshot = fresh;
        self.plan = plan;
        self.carry_dirs.clear();
        self.reconcile();
        if self.armed() {
            self.since = None;
        }
        self.unwatched = unwatched_of(&self.plan);
        self.mark_newly_covered(&before);
        diff
    }

    /// Re-derives every watch's cookie dir after an environment change
    /// (`flutter clean`, `git clean -fdx` removes `.dart_tool`
    /// wholesale) — a walked root's `.dart_tool/dartq/` is recreated,
    /// so a barrier blocked on the stale path can retry in the same
    /// request.
    fn rearm_cookies(&self) {
        for watch in self.shared.watched.lock().iter_mut() {
            watch.cookie_dir = cookie_dir(&watch.dir, watch.root);
        }
    }

    /// The journal path: drain the journal behind the sync cookie, then
    /// let [`Snapshot::take_into`] merge it — the same code the walk
    /// runs, so the diff is byte-identical to a fresh take's. A cookie
    /// that outlived its barrier keeps listening from `late` — expired
    /// undelivered cookies are the strikes, delivered ones clear them.
    /// A request inside the health trust window skips the barrier wait
    /// entirely ([`Freshness::trust_ready`]).
    fn journal_check(&mut self) -> Diff {
        let start = Instant::now();
        self.sweep_late();
        if !self.armed() {
            return self.walk_check(WalkReason::Unarmed);
        }
        // The health trust window: with the stream's first delivery
        // proven after the last doubt boundary, nothing doubtable
        // pending, the liveness proof fresh and the journal drained
        // empty, the verdict needs no round trip. A cookie that cannot
        // be created falls through — the barrier below reports it
        // (`Blocked`) and walks, exactly as before.
        if self.trust_ready()
            && let Some(diff) = self.trusted_serve()
        {
            return diff;
        }
        let mut retried = false;
        let sync = loop {
            match self.sync() {
                // A deleted `.dart_tool` strands the stored cookie dir:
                // re-derive every watch's and retry the barrier once in
                // the same request — without it every request would
                // walk for the daemon's life.
                Sync::Blocked if !retried => {
                    retried = true;
                    self.rearm_cookies();
                }
                other => break other,
            }
        };
        match sync {
            Sync::Held => {}
            Sync::Blocked => {
                // A cookie could not be created: an environment fact,
                // not watcher evidence — walk this request, keep the
                // watcher (and the strike count) as it is.
                return self.walk_check(WalkReason::CookieBlocked);
            }
            Sync::Timeout => {
                // The adaptive bound elapsed — "slow", never a strike:
                // the request walks, the stragglers keep listening from
                // `late`, and only their expiry undelivered counts
                // against the stream.
                return self.walk_check(WalkReason::CookieTimeout);
            }
        }
        let (dirty, walk) = {
            let mut journal = self.shared.journal.lock();
            let dirty: Vec<PathBuf> = journal.dirty.drain().collect();
            let walk = std::mem::replace(&mut journal.walk, false);
            (dirty, walk)
        };
        if walk {
            // The stream flagged loss — unless this is the arm's own
            // covering walk, the one by-design flag a fresh daemon
            // carries until its first take.
            let first = std::mem::take(&mut self.first_walk);
            return self.walk_check(if first {
                WalkReason::First
            } else {
                WalkReason::StreamLost
            });
        }
        // Dirs the last request merged join the dirty set one more
        // time — a new dir's own watch may not have been armed when its
        // first merge walked it, so a file created in that window must
        // be caught here. A dir carried twice is dropped: exactly one
        // extra pass, never a per-request re-walk.
        let mut dirty = dirty;
        let consumed = std::mem::take(&mut self.carry_dirs);
        dirty.extend(consumed.iter().cloned());
        let (diff, merged) =
            self.snapshot
                .take_into(&mut self.plan, &dirty, &self.unwatched, true);
        self.carry_dirs = merged
            .into_iter()
            .filter(|dir| !consumed.contains(dir))
            .collect();
        // A config move can re-plan the surface inside `take_into`
        // (its retake refreshes the plan): re-arm so the watcher covers
        // the new walked set, and rebuild the per-request re-stat list
        // off whatever plan the pass left behind — the list is cheap.
        // Streams armed only now were unwatched while `take_into` ran —
        // mark them so the next request re-covers the window.
        if self.walked_changed() {
            let before = self.covered();
            self.reconcile();
            self.mark_newly_covered(&before);
        }
        self.unwatched = unwatched_of(&self.plan);
        self.last_check = CheckDiag {
            mode: CheckMode::Journal,
            walk_reason: None,
            elapsed_ms: elapsed_ms(start.elapsed()),
            changed: count_changed(&diff),
        };
        diff
    }

    /// Whether the surface the watcher covers has drifted from the
    /// plan's — a config move inside `take_into` re-plans in place,
    /// and a subtree merge can discover new symlinked dirs whose
    /// canonical targets still need their own stream.
    fn walked_changed(&self) -> bool {
        let watched = self.shared.watched.lock();
        let covered = |path: &PathBuf| {
            let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            watched
                .iter()
                .any(|w| canonical == w.dir || canonical.starts_with(&w.dir))
        };
        !self
            .plan
            .walked
            .iter()
            .filter(|root| root.is_dir())
            .all(&covered)
            || !self
                .plan
                .links
                .iter()
                .all(|(canonical, _)| !canonical.is_dir() || covered(canonical))
    }

    /// The root-set change path (pool join/evict): re-plan, re-walk,
    /// re-arm — the diff is against the fresh snapshot (the joined
    /// roots' files are new inputs, the evicted one's gone). The
    /// journal drains BEFORE the take, never after — an event landing
    /// mid-walk stays for the next request; streams armed only now were
    /// unwatched while it ran, so their dirs are marked dirty to
    /// re-cover the window.
    fn rebase(&mut self, roots: &[PathBuf]) -> Diff {
        let before = self.covered();
        {
            let mut journal = self.shared.journal.lock();
            journal.dirty.clear();
            journal.walk = false;
        }
        let packages = self.plan_packages();
        let sdk = self.plan_sdk_home();
        let start = Instant::now();
        let (fresh, plan) = Snapshot::take_with_plan(roots, packages.as_deref(), &sdk);
        let elapsed = start.elapsed();
        self.observe_walk(elapsed);
        let diff = self.snapshot.diff(&fresh);
        // The full take covers the construction window too — the arm's
        // pending covering walk is spent even when a rebase reached
        // first (a stream restart's own covering flag, if any, is
        // raised by the reconcile further down).
        self.first_walk = false;
        self.last_check = CheckDiag {
            mode: CheckMode::Walk,
            walk_reason: Some(WalkReason::Rebase),
            elapsed_ms: elapsed_ms(elapsed),
            changed: count_changed(&diff),
        };
        self.snapshot = fresh;
        self.plan = plan;
        self.carry_dirs.clear();
        self.reconcile();
        if self.armed() {
            self.since = None;
        }
        self.unwatched = unwatched_of(&self.plan);
        self.mark_newly_covered(&before);
        diff
    }

    /// Registers and creates one uniquely named cookie per watched
    /// stream — the barrier's first half, and the trusted serve's whole
    /// proof obligation. `None` when a create failed: the registrations
    /// are rolled back and the caller decides (the barrier reports
    /// `Blocked`; a trusted serve defers to it).
    fn create_cookies(&mut self) -> Option<(String, Vec<PathBuf>)> {
        self.seq += 1;
        let name = format!("{COOKIE_PREFIX}{}-{}", std::process::id(), self.seq);
        let cookies: Vec<PathBuf> = self
            .shared
            .watched
            .lock()
            .iter()
            .map(|w| w.cookie_dir.join(&name))
            .collect();
        if cookies.is_empty() {
            return None;
        }
        {
            let mut journal = self.shared.journal.lock();
            journal.cookies.extend(cookies.iter().cloned());
        }
        // A cookie that cannot be created proves nothing about its
        // stream's delivery — the barrier cannot be trusted, so this
        // request walks. It is an environment fact (read-only tree,
        // full disk), not evidence the watcher is dead: the caller
        // must not count it toward abandonment.
        if cookies.iter().any(|cookie| fs::write(cookie, b"").is_err()) {
            let mut journal = self.shared.journal.lock();
            for cookie in &cookies {
                journal.cookies.remove(cookie);
                let _ = fs::remove_file(cookie);
            }
            return None;
        }
        Some((name, cookies))
    }

    /// The sync barrier: register one uniquely named cookie path per
    /// watched stream, create the files, then block until the watcher
    /// delivered every one (or the wait bound ends). Per-stream
    /// ordering means every change predating a cookie is in the journal
    /// once it lands. Files are deleted on the way out, success or
    /// timeout; a pending set left at timeout is cleared so a straggler
    /// event can never resolve a later barrier.
    fn sync(&mut self) -> Sync {
        #[cfg(test)]
        if self.injected_journal {
            return Sync::Held;
        }
        if self.shared.watched.lock().is_empty() {
            return Sync::Held;
        }
        let Some((name, cookies)) = self.create_cookies() else {
            return Sync::Blocked;
        };
        let deadline = Instant::now() + self.wait_bound();
        let mut journal = self.shared.journal.lock();
        let held = loop {
            if cookies.iter().all(|c| !journal.cookies.contains(c)) {
                break true;
            }
            if self
                .shared
                .delivered
                .wait_until(&mut journal, deadline)
                .timed_out()
            {
                break false;
            }
        };
        for cookie in &cookies {
            journal.cookies.remove(cookie);
            let _ = fs::remove_file(cookie);
        }
        if held {
            return Sync::Held;
        }
        // The barrier timed out but the cookies may still be in
        // flight — keep the name as a late cookie: its create OR
        // delete event inside the expiry still proves the stream
        // alive, and only expiring undelivered is a strike. Cookie
        // names are unique per sync, so a straggler can never resolve
        // a later barrier. The missed bound itself is latency, not
        // doubt — the health boundary stays put, and the straggler's
        // delivery re-stamps the liveness proof on its far side, so
        // the trust window re-opens on the very next ask instead of
        // re-earning a punctuality streak that loaded hosts never
        // accumulate.
        journal.late.insert(name.clone());
        self.late.push((name, Instant::now() + late_expiry()));
        Sync::Timeout
    }

    /// Whether this request may serve from the journal verdict alone —
    /// the health trust window. All of:
    ///
    /// * no strike is pending (`timeouts == 0`) and no straggler is
    ///   under evaluation (`late` empty);
    /// * no merged dir waits out its one-more-request re-coverage
    ///   (`carry_dirs` empty);
    /// * the stream proved liveness inside [`TRUST_WINDOW`] (a
    ///   cookie's event observed delivered) — and that proof POSTDATES
    ///   the last arm or stream restart (`health_since`): one proven
    ///   round trip on the current stream earns the window, and any
    ///   restart, abandon or re-arm re-earns it the same way. No age
    ///   clock gates this — every ask re-checks the proof's freshness,
    ///   so a stream that goes silent loses the window inside
    ///   [`TRUST_WINDOW`] plus one ask regardless of how long it had
    ///   been trusted before;
    /// * the journal itself is drained empty with no walk flag.
    ///
    /// The staleness contract: an edit whose event has delivered is in
    /// the journal — the peek sees it and this returns `false`, the
    /// full barrier path serves the edit. An edit still in flight
    /// defers until a cookie created after it delivers (per-stream
    /// ordering) — at most one ask on a stream that delivers inside
    /// the ask cadence, and never longer than the proof-freshness
    /// machinery allows, since sustained trust requires a delivery
    /// inside [`TRUST_WINDOW`]. A missed wait bound does not close
    /// this window by itself: the straggler it registers adjudicates
    /// (`late` non-empty holds the window shut while the verdict pends;
    /// its delivery re-stamps the proof and the next ask re-opens;
    /// its expiry is a strike and doubt). Any doubt takes today's path.
    fn trust_ready(&self) -> bool {
        self.timeouts == 0
            && self.late.is_empty()
            && self.carry_dirs.is_empty()
            && {
                let journal = self.shared.journal.lock();
                !journal.walk
                    && journal.dirty.is_empty()
                    && journal.proven.is_some_and(|proven| {
                        proven.elapsed() <= TRUST_WINDOW
                            && self.health_since.is_none_or(|since| proven > since)
                    })
            }
    }

    /// One trusted serve: write the next cookies without waiting for
    /// them (their delivery, off the request path, stamps the liveness
    /// proof the following request keys on), then answer through the
    /// same unwatched re-stat every journal request runs — a config
    /// move re-plans inside [`Snapshot::take_into`] exactly as on the
    /// barrier path. `None` when a cookie could not be created: the
    /// environment fact is the barrier's to report, not the window's.
    fn trusted_serve(&mut self) -> Option<Diff> {
        let start = Instant::now();
        let (_, cookies) = self.create_cookies()?;
        self.pending_cookies = cookies;
        let (diff, _) = self
            .snapshot
            .take_into(&mut self.plan, &[], &self.unwatched, true);
        if self.walked_changed() {
            let before = self.covered();
            self.reconcile();
            self.mark_newly_covered(&before);
        }
        self.unwatched = unwatched_of(&self.plan);
        self.trusted += 1;
        self.last_check = CheckDiag {
            mode: CheckMode::Journal,
            walk_reason: None,
            elapsed_ms: elapsed_ms(start.elapsed()),
            changed: count_changed(&diff),
        };
        Some(diff)
    }

    /// Removes the last trusted serve's cookie files and their pending
    /// registrations. Their create events have delivered (the serve's
    /// own eligibility required a fresh proof) — or never will, and
    /// must not pin the registration set. A straggler's delete event
    /// still resolves the name and still stamps the proof, so even the
    /// sweep pays its way.
    fn sweep_pending_cookies(&mut self) {
        let pending = std::mem::take(&mut self.pending_cookies);
        if pending.is_empty() {
            return;
        }
        let mut journal = self.shared.journal.lock();
        for cookie in &pending {
            journal.cookies.remove(cookie);
            let _ = fs::remove_file(cookie);
        }
    }

    /// The defense-in-depth verification, run only at a synchronized
    /// point — after this request's own merge and behind a second
    /// barrier. `Some(diff)` on a conclusive mismatch: the walk
    /// disagreed with the merged snapshot while the barrier proved no
    /// event in flight — a real journal divergence, so the snapshot is
    /// restored to the walk's truth, the watcher cools off and re-arms
    /// later, and the answer is `previous.diff(fresh)`. `None` on a
    /// match (the common case) or an inconclusive pass — a barrier that
    /// could not close or any event landing mid-walk means the
    /// comparison proved nothing: keep the journal entries for the next
    /// request and retry next period. An inconclusive pass never
    /// abandons.
    fn verify_at_sync(&mut self, previous: &Snapshot) -> Option<Diff> {
        let roots = self.plan_roots();
        let packages = self.plan_packages();
        let sdk = self.plan_sdk_home();
        let start = Instant::now();
        let fresh = Snapshot::take(&roots, packages.as_deref(), &sdk);
        self.observe_walk(start.elapsed());
        // Second barrier: a stream that cannot close (blocked or
        // timed out) proves nothing about events during the walk —
        // inconclusive. Events it delivered stay for the next request;
        // a timeout here is the audit's miss, not a strike.
        if self.sync() != Sync::Held {
            return None;
        }
        // Only dirty paths that can actually move the snapshot void the
        // comparison — the merge's own rule decides: a dot-path the
        // walk ignores (the session heartbeat inside
        // `.dart_tool/dartq/`, `.git` writes) and a non-input file
        // never change entries, while a dir or a vanished path could
        // hold recorded entries and must void.
        let moved = {
            let journal = self.shared.journal.lock();
            journal.walk
                || journal.dirty.iter().any(|path| {
                    match merge_target(&self.plan, path) {
                        Some(target) => {
                            !fs::symlink_metadata(&target).is_ok_and(|m| m.is_file())
                                || recorded(&target)
                                || self.plan.configs.contains(&target)
                        }
                        None => false,
                    }
                })
        };
        if moved {
            return None;
        }
        if fresh == self.snapshot {
            return None;
        }
        self.mismatches += 1;
        eprintln!(
            "dartq-analyzer-host: watch: verification walk disagrees with the \
             journal snapshot ({} entries vs {})",
            fresh.len(),
            self.snapshot.len(),
        );
        let answer = previous.diff(&fresh);
        self.snapshot = fresh;
        self.abandon("the verification walk diverged from the journal");
        Some(answer)
    }

    fn plan_roots(&self) -> Vec<PathBuf> {
        self.plan.roots().to_vec()
    }

    /// How the last `check` answered — which path ran, why, and what it
    /// cost. The daemon reads this to make a degraded watcher (a tree
    /// whose every request walks) visible instead of silently slow.
    #[must_use]
    pub(crate) fn last_check(&self) -> CheckDiag {
        self.last_check
    }

    fn plan_packages(&self) -> Option<PathBuf> {
        self.plan.packages().map(Path::to_path_buf)
    }

    fn plan_sdk_home(&self) -> PathBuf {
        self.plan.sdk_home().to_path_buf()
    }

    /// The maintained snapshot — the daemon diffs against it only
    /// through `check`.
    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// Test hook: inject dirty paths as the drain thread would.
    #[cfg(test)]
    pub(crate) fn inject_dirty(&self, paths: &[PathBuf]) {
        let mut journal = self.shared.journal.lock();
        for path in paths {
            journal.dirty_path(path.clone());
        }
    }

    /// Test hook: raise the walk flag as a rescan/error would.
    #[cfg(test)]
    pub(crate) fn inject_walk(&self) {
        self.shared.journal.lock().walk = true;
    }

    /// Test hook: drive the journal by injection only — every sync
    /// barrier holds without stream proof (see `injected_journal`).
    #[cfg(test)]
    pub(crate) fn journal_by_injection(&mut self) {
        self.injected_journal = true;
    }
}

/// The per-request re-stat set: config files no watch covers (the
/// out-of-root ancestors — `plan.watched[i] == false`) plus the walked
/// set's non-directory entries (a file root gets no recursive stream —
/// its own stat is the coverage). Only consulted on the journal path,
/// which an unarmed daemon never reaches.
fn unwatched_of(plan: &WalkPlan) -> Vec<PathBuf> {
    plan.configs
        .iter()
        .zip(plan.watched.iter())
        .filter_map(|(file, watched)| (!watched).then(|| file.clone()))
        .chain(
            plan.walked
                .iter()
                .filter(|root| !root.is_dir())
                .cloned(),
        )
        .collect()
}

/// The substrate off macOS: `notify`'s recommended watcher (inotify on
/// Linux) — one recursive watch per dir, its events folded into the
/// journal by a drain thread.
#[cfg(not(target_os = "macos"))]
mod portable {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{Receiver, RecvTimeoutError};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    use super::Shared;

    /// The watcher plus its drain thread. Dropping it stops and joins
    /// the drain, then drops the watcher.
    pub(super) struct Stream {
        /// The live watcher; `Watcher::apply` moves its watch set.
        pub(super) watcher: notify::RecommendedWatcher,
        stop: Arc<AtomicBool>,
        drain: Option<JoinHandle<()>>,
    }

    impl Stream {
        /// Builds the watcher (watching nothing yet) and its drain
        /// thread. `None` when either cannot start.
        pub(super) fn spawn(shared: &Arc<Shared>) -> Option<Self> {
            let (sender, receiver) =
                std::sync::mpsc::channel::<Result<notify::Event, notify::Error>>();
            let watcher = notify::recommended_watcher(sender)
                .map_err(|error| {
                    eprintln!("dartq-analyzer-host: watch stream unavailable: {error}");
                    error
                })
                .ok()?;
            let stop = Arc::new(AtomicBool::new(false));
            let drain = thread::Builder::new()
                .name("dartq-analyzer-watch".into())
                .spawn({
                    let shared = Arc::clone(shared);
                    let stop = Arc::clone(&stop);
                    move || drain_loop(&receiver, &shared, &stop)
                })
                .map_err(|error| {
                    eprintln!("dartq-analyzer-host: watch drain unavailable: {error}");
                    error
                })
                .ok()?;
            Some(Self {
                watcher,
                stop,
                drain: Some(drain),
            })
        }
    }

    impl Drop for Stream {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(drain) = self.drain.take() {
                let _ = drain.join();
            }
        }
    }

    /// The event-drain thread: pops `notify` events off the stream
    /// channel and folds them into the journal — rescan flags and stream
    /// errors raise `walk`, every path goes through [`Shared::record`].
    /// Exits when the stream disconnects (the watcher dropped with the
    /// daemon) or `stop` is set.
    fn drain_loop(
        receiver: &Receiver<Result<notify::Event, notify::Error>>,
        shared: &Shared,
        stop: &AtomicBool,
    ) {
        loop {
            // The stop check leads the loop and the wait is short: under
            // a storm `recv_timeout` never times out, so a flag read only
            // on Timeout would never fire — the drop's join would hang
            // the request it ran on.
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let event = match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(event) => event,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    // The watcher is gone — every later request walks.
                    shared.lost();
                    return;
                }
            };
            match event {
                Ok(event) if event.need_rescan() => shared.lost(),
                Ok(event) => shared.record(event.paths.iter().map(PathBuf::as_path)),
                Err(error) => {
                    // A stream error (inotify ENOSPC on
                    // max_user_watches, a vanished root): the journal can
                    // no longer be trusted to be complete.
                    eprintln!("dartq-analyzer-host: watcher error: {error}");
                    shared.lost();
                }
            }
        }
    }
}

/// The macOS substrate: the crate's own FSEvents binding — one stream
/// over the whole watch set, owned by a dedicated CoreFoundation
/// run-loop thread whose callback folds events straight into the
/// journal (no channel, no second thread).
///
/// The stream is created `SinceNow` with latency 0 and
/// `FileEvents | NoDefer`: per-file paths, each forwarded the moment
/// `fseventsd` has it.
///
/// Flag mapping. The journal consumes paths, not event kinds: the merge
/// re-stats every dirty path and tells files, dirs and links apart
/// itself, exactly as it does for inotify's events — so no kind is
/// decoded here.
///
/// | event flags | journal effect |
/// |---|---|
/// | `MustScanSubDirs`, `UserDropped`, `KernelDropped` | `walk` — events were lost |
/// | `HistoryDone` | ignored — its path is meaningless (never sent for `SinceNow`) |
/// | anything else: item created / removed / renamed / modified / inode-meta / finder-info / owner / xattr, is file / dir / symlink / hardlink, cloned, mount / unmount, or no flag at all | the path — a cookie name resolves its barrier, any other path joins `dirty` |
///
/// The public API cannot change a live stream's paths, so a new path
/// set is a new stream: the successor starts before its predecessor
/// stops, and the caller flags a covering walk for the predecessor's
/// in-flight events.
#[cfg(target_os = "macos")]
mod fsevents {
    use std::ffi::{CStr, CString, OsStr, c_char, c_void};
    use std::os::unix::ffi::OsStrExt as _;
    use std::path::{Path, PathBuf};
    use std::ptr;
    use std::sync::Arc;
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    use parking_lot::{Condvar, Mutex};

    use super::Shared;

    type CFIndex = isize;
    /// Any CoreFoundation object: `CFTypeRef`, `CFStringRef`,
    /// `CFArrayRef`, `CFAllocatorRef`.
    type CFRef = *const c_void;
    type RunLoopRef = *mut c_void;
    type StreamRef = *mut c_void;

    /// `CFArrayCallBacks` — only `kCFTypeArrayCallBacks`' address is
    /// ever taken.
    #[repr(C)]
    struct CFArrayCallBacks {
        version: CFIndex,
        retain: *const c_void,
        release: *const c_void,
        copy_description: *const c_void,
        equal: *const c_void,
    }

    /// `FSEventStreamContext`. Only `info` is set: the stream thread
    /// keeps its pointee alive past every stream, so the framework needs
    /// no retain/release callbacks.
    #[repr(C)]
    struct Context {
        version: CFIndex,
        info: *mut c_void,
        retain: *const c_void,
        release: *const c_void,
        copy_description: *const c_void,
    }

    /// `FSEventStreamCallback`.
    type Callback =
        extern "C" fn(*const c_void, *mut c_void, usize, *mut c_void, *const u32, *const u64);

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFTypeArrayCallBacks: CFArrayCallBacks;
        static kCFRunLoopDefaultMode: CFRef;
        fn CFArrayCreate(
            allocator: CFRef,
            values: *const CFRef,
            count: CFIndex,
            callbacks: *const CFArrayCallBacks,
        ) -> CFRef;
        fn CFStringCreateWithFileSystemRepresentation(
            allocator: CFRef,
            path: *const c_char,
        ) -> CFRef;
        fn CFRelease(object: CFRef);
        fn CFRunLoopGetCurrent() -> RunLoopRef;
        fn CFRunLoopRunInMode(mode: CFRef, seconds: f64, return_after_source_handled: u8) -> i32;
        fn CFRunLoopStop(run_loop: RunLoopRef);
    }

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn FSEventStreamCreate(
            allocator: CFRef,
            callback: Callback,
            context: *const Context,
            paths: CFRef,
            since_when: u64,
            latency: f64,
            flags: u32,
        ) -> StreamRef;
        fn FSEventStreamScheduleWithRunLoop(stream: StreamRef, run_loop: RunLoopRef, mode: CFRef);
        fn FSEventStreamStart(stream: StreamRef) -> u8;
        fn FSEventStreamStop(stream: StreamRef);
        fn FSEventStreamInvalidate(stream: StreamRef);
        fn FSEventStreamRelease(stream: StreamRef);
    }

    /// `kFSEventStreamEventIdSinceNow`.
    const SINCE_NOW: u64 = u64::MAX;
    /// `kFSEventStreamCreateFlagNoDefer | kFSEventStreamCreateFlagFileEvents`.
    const CREATE_FLAGS: u32 = 0x02 | 0x10;
    /// `kFSEventStreamEventFlagMustScanSubDirs | …UserDropped |
    /// …KernelDropped` — the drop flags only ever accompany the first,
    /// any of them means the stream lost events.
    const LOST: u32 = 0x01 | 0x02 | 0x04;
    /// `kFSEventStreamEventFlagHistoryDone`.
    const HISTORY_DONE: u32 = 0x10;
    /// `kCFRunLoopRunFinished`: the mode holds no source.
    const RUN_FINISHED: i32 = 1;

    /// One run-loop slice — the longest a stop or a new path set waits
    /// for the thread when its wake-up misses (CoreFoundation drops a
    /// `CFRunLoopStop` that lands between two runs).
    const SLICE: Duration = Duration::from_millis(50);

    /// How long `watch` waits for the thread to apply a path set:
    /// `FSEventStreamStart` is one synchronous `fseventsd` round trip,
    /// tens of milliseconds even with the daemon saturated.
    const APPLY_MAX: Duration = Duration::from_secs(5);

    /// The handle on the stream thread. Dropping it stops the thread,
    /// which stops, invalidates and releases its stream on its own loop,
    /// and joins it.
    pub(super) struct Stream {
        mailbox: Arc<Mailbox>,
        thread: Option<JoinHandle<()>>,
    }

    /// The handle ↔ thread mailbox: one lock and one condvar carry a
    /// path set in, its outcome out, and the stop bit.
    #[derive(Default)]
    struct Mailbox {
        mail: Mutex<Mail>,
        changed: Condvar,
    }

    #[derive(Default)]
    struct Mail {
        /// The path set the thread streams next.
        want: Option<Vec<PathBuf>>,
        /// The outcome of the last applied set.
        done: Option<Result<(), String>>,
        stop: bool,
        /// The thread's run loop while the thread is alive — the thread
        /// clears it under the lock before it exits, so a wake-up never
        /// reaches a finalized loop.
        run_loop: Option<RunLoop>,
    }

    /// The stream thread's run loop.
    struct RunLoop(RunLoopRef);

    // SAFETY: the pointer only ever reaches `CFRunLoopStop`, which
    // CoreFoundation allows from any thread, and only while its thread
    // is alive (see `Mail::run_loop`).
    unsafe impl Send for RunLoop {}

    impl Stream {
        /// Starts the stream thread; it streams nothing until `watch`.
        /// `None` when the thread cannot start.
        pub(super) fn spawn(shared: &Arc<Shared>) -> Option<Self> {
            let mailbox = Arc::new(Mailbox::default());
            let thread = thread::Builder::new()
                .name("dartq-analyzer-watch".into())
                .spawn({
                    let shared = Arc::clone(shared);
                    let mailbox = Arc::clone(&mailbox);
                    move || serve(&shared, &mailbox)
                })
                .map_err(|error| {
                    eprintln!("dartq-analyzer-host: watch stream thread unavailable: {error}");
                })
                .ok()?;
            Some(Self {
                mailbox,
                thread: Some(thread),
            })
        }

        /// Streams exactly `paths` from now on (an empty set streams
        /// nothing): the thread starts the new stream, then drops the
        /// old one. Bounded by [`APPLY_MAX`]; a failed or timed-out
        /// apply is the caller's cool-off, which drops this handle — a
        /// late outcome can never answer a later call.
        pub(super) fn watch(&self, paths: Vec<PathBuf>) -> Result<(), String> {
            self.post(|mail| {
                mail.want = Some(paths);
                mail.done = None;
            });
            let deadline = Instant::now() + APPLY_MAX;
            let mut mail = self.mailbox.mail.lock();
            loop {
                if let Some(done) = mail.done.take() {
                    return done;
                }
                if self.mailbox.changed.wait_until(&mut mail, deadline).timed_out() {
                    return mail.done.take().unwrap_or_else(|| {
                        Err(format!("the stream thread did not apply within {APPLY_MAX:?}"))
                    });
                }
            }
        }

        /// Updates the mail, then wakes the thread wherever it waits —
        /// inside a run-loop slice or on the condvar.
        fn post(&self, update: impl FnOnce(&mut Mail)) {
            let mut mail = self.mailbox.mail.lock();
            update(&mut mail);
            if let Some(run_loop) = &mail.run_loop {
                // SAFETY: a published loop's thread is alive — it clears
                // the entry under this lock before exiting.
                unsafe { CFRunLoopStop(run_loop.0) };
            }
            drop(mail);
            self.mailbox.changed.notify_all();
        }
    }

    impl Drop for Stream {
        fn drop(&mut self) {
            self.post(|mail| mail.stop = true);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    /// The stream thread: applies posted path sets and runs its loop in
    /// bounded slices — events arrive through [`callback`] — until
    /// stopped. Every stream it starts is released before it returns,
    /// while `shared` (each stream's callback info) is still borrowed.
    fn serve(shared: &Shared, mailbox: &Mailbox) {
        // SAFETY: no precondition — this thread's own loop, created on
        // first use and valid until the thread exits.
        let run_loop = unsafe { CFRunLoopGetCurrent() };
        mailbox.mail.lock().run_loop = Some(RunLoop(run_loop));
        // Declared before `live`, so dropped after it: the loop is
        // unpublished on every exit path, unwinding included, once the
        // stream is released.
        let _published = Published(mailbox);
        let mut live: Option<Live> = None;
        loop {
            let want = {
                let mut mail = mailbox.mail.lock();
                if mail.stop {
                    break;
                }
                mail.want.take()
            };
            if let Some(paths) = want {
                let done = if paths.is_empty() {
                    live = None;
                    Ok(())
                } else {
                    // The assignment drops the predecessor only once its
                    // successor streams.
                    Live::start(&paths, shared, run_loop).map(|next| live = Some(next))
                };
                mailbox.mail.lock().done = Some(done);
                mailbox.changed.notify_all();
                continue;
            }
            // With no stream scheduled the mode is empty and a run
            // returns at once — wait on the mailbox instead of spinning.
            let ran = live.is_some()
                // SAFETY: runs this thread's own loop for one bounded
                // slice in the default mode.
                && unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, SLICE.as_secs_f64(), 0) }
                    != RUN_FINISHED;
            if !ran {
                let mut mail = mailbox.mail.lock();
                if !mail.stop && mail.want.is_none() {
                    mailbox.changed.wait_for(&mut mail, SLICE);
                }
            }
        }
    }

    /// Clears `Mail::run_loop` when the stream thread leaves `serve`.
    struct Published<'a>(&'a Mailbox);

    impl Drop for Published<'_> {
        fn drop(&mut self) {
            self.0.mail.lock().run_loop = None;
        }
    }

    /// A started stream, scheduled on the stream thread's loop. Dropping
    /// it stops, invalidates and releases it — on that thread.
    struct Live(StreamRef);

    impl Live {
        fn start(paths: &[PathBuf], shared: &Shared, run_loop: RunLoopRef) -> Result<Self, String> {
            let paths = path_array(paths)?;
            let context = Context {
                version: 0,
                info: ptr::from_ref(shared).cast_mut().cast(),
                retain: ptr::null(),
                release: ptr::null(),
                copy_description: ptr::null(),
            };
            // SAFETY: `paths` is a live CFArray of CFStrings (the stream
            // copies it), `context` is copied into the stream, `callback`
            // has the FSEventStreamCallback signature, and `info`
            // outlives the stream (see `serve`).
            let stream = unsafe {
                FSEventStreamCreate(
                    ptr::null(),
                    callback,
                    &raw const context,
                    paths.0,
                    SINCE_NOW,
                    0.0,
                    CREATE_FLAGS,
                )
            };
            if stream.is_null() {
                return Err("FSEventStreamCreate failed".into());
            }
            // SAFETY: a fresh stream, scheduled on this thread's loop.
            unsafe { FSEventStreamScheduleWithRunLoop(stream, run_loop, kCFRunLoopDefaultMode) };
            // SAFETY: the stream is scheduled. A stream that failed to
            // start must not be stopped — it is invalidated and released.
            if unsafe { FSEventStreamStart(stream) } == 0 {
                unsafe {
                    FSEventStreamInvalidate(stream);
                    FSEventStreamRelease(stream);
                }
                return Err("FSEventStreamStart failed".into());
            }
            Ok(Self(stream))
        }
    }

    impl Drop for Live {
        fn drop(&mut self) {
            // SAFETY: a started stream this value owns, torn down on the
            // thread whose loop it is scheduled on.
            unsafe {
                FSEventStreamStop(self.0);
                FSEventStreamInvalidate(self.0);
                FSEventStreamRelease(self.0);
            }
        }
    }

    /// An owned CoreFoundation object (the Create rule), released on
    /// drop.
    struct Owned(CFRef);

    impl Owned {
        fn new(object: CFRef) -> Option<Self> {
            (!object.is_null()).then_some(Self(object))
        }
    }

    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: a non-null object this value holds one reference to.
            unsafe { CFRelease(self.0) };
        }
    }

    /// The stream's path list: a CFArray of the paths' CFStrings.
    fn path_array(paths: &[PathBuf]) -> Result<Owned, String> {
        let strings = paths
            .iter()
            .map(|path| {
                let path = CString::new(path.as_os_str().as_bytes()).ok()?;
                // SAFETY: `path` is NUL-terminated and outlives the call.
                Owned::new(unsafe {
                    CFStringCreateWithFileSystemRepresentation(ptr::null(), path.as_ptr())
                })
            })
            .collect::<Option<Vec<Owned>>>()
            .ok_or("a watched path has no CFString form")?;
        let values: Vec<CFRef> = strings.iter().map(|string| string.0).collect();
        // SAFETY: `values` holds `values.len()` live CFStrings; the type
        // callbacks make the array retain its own references.
        Owned::new(unsafe {
            CFArrayCreate(
                ptr::null(),
                values.as_ptr(),
                values.len().cast_signed(),
                &raw const kCFTypeArrayCallBacks,
            )
        })
        .ok_or_else(|| "CFArrayCreate failed".to_owned())
    }

    /// The stream callback: runs only on the stream thread, inside a
    /// run-loop slice of `serve`, which holds the `Shared` behind
    /// `info`. Lost events raise `walk`; every other path is recorded.
    extern "C" fn callback(
        _stream: *const c_void,
        info: *mut c_void,
        count: usize,
        paths: *mut c_void,
        flags: *const u32,
        _ids: *const u64,
    ) {
        if count == 0 {
            return;
        }
        // SAFETY: `info` is the `Shared` `serve` keeps borrowed past
        // every stream; without `UseCFTypes` the framework passes
        // `count` C-string paths and `count` flag words, valid for the
        // whole call.
        let (shared, paths, flags) = unsafe {
            (
                &*info.cast::<Shared>(),
                std::slice::from_raw_parts(paths.cast::<*const c_char>(), count),
                std::slice::from_raw_parts(flags, count),
            )
        };
        if flags.iter().any(|flag| flag & LOST != 0) {
            shared.lost();
        }
        shared.record(
            paths
                .iter()
                .zip(flags)
                .filter(|&(_, &flag)| flag & (LOST | HISTORY_DONE) == 0)
                .map(|(&path, _)| {
                    // SAFETY: a NUL-terminated path the framework keeps
                    // alive for the whole call.
                    Path::new(OsStr::from_bytes(unsafe { CStr::from_ptr(path) }.to_bytes()))
                }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64};
    use std::thread;

    /// A deterministic xorshift64 — the differential test's op stream
    /// is seeded, never random between runs.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }

        fn below(&mut self, n: usize) -> usize {
            usize::try_from(self.next() % n as u64).expect("modulo fits")
        }
    }

    /// A package tree the differential test mutates: an app root with
    /// sources, a nested package, a path dependency outside the root,
    /// and the config files the walk re-stats. `dot` seats the app
    /// under a dot-parent (`<base>/.worktrees/app`) — the AI-worktree
    /// shape the root-relative dot rule exists for.
    struct Tree {
        base: PathBuf,
        app: PathBuf,
        dep: PathBuf,
        sdk: PathBuf,
        /// Relative file candidates the op stream touches.
        files: Vec<PathBuf>,
        dirs: Vec<PathBuf>,
    }

    impl Tree {
        fn build(tag: &str) -> Self {
            Self::build_at(tag, false)
        }

        fn build_dot(tag: &str) -> Self {
            Self::build_at(tag, true)
        }

        fn build_at(tag: &str, dot: bool) -> Self {
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let base = std::env::temp_dir().join(format!(
                "dq_fresh_{}_{}_{}",
                tag,
                std::process::id(),
                SEQ.fetch_add(1, AtomicOrdering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&base);
            fs::create_dir_all(&base).expect("base");
            // Paths in the snapshot are canonical-spelled — resolve
            // the temp root once so fixtures and expectations match.
            let base = base.canonicalize().expect("canonical base");
            let app = if dot {
                base.join(".worktrees/app")
            } else {
                base.join("app")
            };
            let dep = base.join("dep");
            let nested = app.join("nested");
            let sdk = base.join("sdk");
            for dir in [
                app.join("lib"),
                app.join("lib/deep"),
                app.join(".dart_tool"),
                nested.join("lib"),
                dep.join("lib"),
            ] {
                fs::create_dir_all(dir).expect("mkdir");
            }
            fs::create_dir_all(&sdk).expect("sdk");
            fs::write(app.join("pubspec.yaml"), "name: app\n").expect("pubspec");
            fs::write(
                app.join("analysis_options.yaml"),
                "analyzer: {}\n",
            )
            .expect("options");
            fs::write(
                app.join(".dart_tool/package_config.json"),
                r#"{"configVersion":2,"packages":[
                    {"name":"app","rootUri":"../"},
                    {"name":"dep","rootUri":"../../dep"},
                    {"name":"nested","rootUri":"../nested"}
                ]}"#,
            )
            .expect("package config");
            fs::write(app.join("lib/a.dart"), "var a = 1;\n").expect("a");
            fs::write(app.join("lib/deep/b.dart"), "var b = 2;\n").expect("b");
            fs::write(nested.join("lib/n.dart"), "var n = 3;\n").expect("n");
            fs::write(dep.join("lib/d.dart"), "var d = 4;\n").expect("d");
            fs::write(dep.join("pubspec.yaml"), "name: dep\n").expect("dep pubspec");
            let files = vec![
                PathBuf::from("lib/a.dart"),
                PathBuf::from("lib/deep/b.dart"),
                PathBuf::from("lib/c.dart"),
                PathBuf::from("pubspec.yaml"),
                PathBuf::from("analysis_options.yaml"),
                PathBuf::from("README.md"),
            ];
            let dirs = vec![
                PathBuf::from("lib/deep"),
                PathBuf::from("lib/newdir"),
                PathBuf::from("src"),
            ];
            Self {
                base,
                app,
                dep,
                sdk,
                files,
                dirs,
            }
        }

        fn roots(&self) -> Vec<PathBuf> {
            vec![self.app.clone()]
        }
    }

    /// One random mutation plus the dirty paths a watcher would report
    /// for it (a file op reports its file, a tree op its root, a rename
    /// both spellings).
    #[derive(Debug)]
    enum Op {
        Write(PathBuf),
        Remove(PathBuf),
        Rename(PathBuf, PathBuf),
        Mkdir(PathBuf),
        Rmdir(PathBuf),
        WriteConfig(PathBuf),
        LinkCreate(PathBuf, PathBuf),
        LinkRemove(PathBuf),
        WriteThroughLink(PathBuf),
        /// A write the walk must ignore: dot-tree churn (`.git`,
        /// `.idea`, the session's `.dart_tool/dartq/state.json`) or a
        /// non-input file (`build/`, `assets/` json/xml, README).
        WriteIgnored(PathBuf),
        /// Removes `<root>/.dart_tool` wholesale — the `flutter clean`
        /// shape; the merge must re-stat the config it held.
        RemoveDartTool(PathBuf),
        /// Recreates `<root>/.dart_tool/package_config.json`.
        CreateDartTool(PathBuf),
    }

    impl Op {
        fn dirty(&self) -> Vec<PathBuf> {
            match self {
                Self::Write(path)
                | Self::Remove(path)
                | Self::LinkRemove(path)
                | Self::WriteThroughLink(path) => vec![path.clone()],
                Self::Rename(from, to) => vec![from.clone(), to.clone()],
                Self::Mkdir(path)
                | Self::Rmdir(path)
                | Self::WriteConfig(path)
                | Self::WriteIgnored(path)
                | Self::RemoveDartTool(path)
                | Self::CreateDartTool(path) => vec![path.clone()],
                Self::LinkCreate(link, _) => vec![link.clone()],
            }
        }

        fn run(&self, tree: &Tree, rng: &mut Rng) {
            match self {
                Self::Write(path) => {
                    // A write into a missing dir recreates it — the
                    // watcher reports the same dirty file either way.
                    if let Some(parent) = path.parent() {
                        fs::create_dir_all(parent).ok();
                    }
                    let content = format!("var v{} = {};\n", rng.next() % 1000, rng.next());
                    fs::write(path, content).expect("write");
                }
                Self::Remove(path) => {
                    let _ = fs::remove_file(path);
                }
                Self::Rename(from, to) => {
                    // An existing target makes a dir rename fail on
                    // some platforms — only rename into empty space,
                    // like a real move.
                    if from.exists() && !to.exists() {
                        fs::rename(from, to).expect("rename");
                    }
                }
                Self::Mkdir(path) => {
                    let _ = fs::create_dir_all(path);
                }
                Self::Rmdir(path) => {
                    let _ = fs::remove_dir_all(path);
                }
                Self::WriteConfig(config) => {
                    // A config edit naming a different extra root —
                    // the plan's walked set must re-derive.
                    let extra = if rng.below(2) == 0 {
                        r#",{"name":"extra","rootUri":"../../extra"}"#
                    } else {
                        ""
                    };
                    fs::create_dir_all(tree.base.join("extra/lib")).ok();
                    fs::write(tree.base.join("extra/lib/x.dart"), "var x;\n").ok();
                    if let Some(parent) = config.parent() {
                        fs::create_dir_all(parent).ok();
                    }
                    fs::write(
                        config,
                        format!(
                            r#"{{"configVersion":2,"packages":[
                                {{"name":"app","rootUri":"../"}},
                                {{"name":"dep","rootUri":"../../dep"}},
                                {{"name":"nested","rootUri":"../nested"}}
                                {extra}
                            ]}}"#
                        ),
                    )
                    .expect("package config");
                }
                Self::WriteIgnored(path) => {
                    if let Some(parent) = path.parent() {
                        fs::create_dir_all(parent).ok();
                    }
                    fs::write(path, format!("ignored {}\n", rng.next()))
                        .expect("ignored write");
                }
                Self::RemoveDartTool(dir) => {
                    let _ = fs::remove_dir_all(dir);
                }
                Self::CreateDartTool(config) => {
                    if let Some(parent) = config.parent() {
                        fs::create_dir_all(parent).expect("dart_tool");
                    }
                    fs::write(
                        config,
                        r#"{"configVersion":2,"packages":[
                            {"name":"app","rootUri":"../"},
                            {"name":"dep","rootUri":"../../dep"},
                            {"name":"nested","rootUri":"../nested"}
                        ]}"#,
                    )
                    .expect("package config");
                }
                Self::LinkCreate(link, target) => {
                    std::os::unix::fs::symlink(target, link).expect("symlink");
                }
                Self::LinkRemove(link) => {
                    let _ = fs::remove_file(link);
                }
                Self::WriteThroughLink(path) => {
                    // Only meaningful while the link stands — a write
                    // into a missing tree would be a create, not an
                    // edit through the link.
                    if path.parent().is_some_and(|dir| dir.exists()) {
                        fs::write(path, format!("var through = {};\n", rng.next()))
                            .expect("write");
                    }
                }
            }
        }
    }

    /// The seeded differential run: after every random op the
    /// journal-maintained snapshot and its diff must equal a fresh full
    /// walk's, byte for byte. Runs over a plain root and a root seated
    /// under a dot-parent (the AI-worktree shape), and the op stream
    /// mixes dot-tree churn, ignored non-input files, and
    /// `.dart_tool` removal/recreation — the merge must mirror the
    /// walk's own reachability on every round.
    #[test]
    fn journal_diff_matches_fresh_walk_over_random_ops() {
        for tree in [Tree::build("diff"), Tree::build_dot("diffdot")] {
            let roots = tree.roots();
            let (mut snapshot, mut plan) = Snapshot::take_with_plan(&roots, None, &tree.sdk);
            let unwatched = unwatched_of(&plan);
            let link = tree.app.join("lib/linked");
            let ignored = [
                tree.app.join(".git/index"),
                tree.app.join(".idea/workspace.xml"),
                tree.app.join(".dart_tool/dartq/state.json"),
                tree.app.join(".dart_tool/package_graph.json"),
                tree.app.join("build/unit_test_assets/AssetManifest.json"),
                tree.app.join("assets/i18n/en.json"),
                tree.app.join("README.md"),
            ];
            let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
            for round in 0..140 {
                let op = match rng.below(12) {
                    0 => {
                        let file = tree.app.join(&tree.files[rng.below(tree.files.len())]);
                        Op::Write(file)
                    }
                    1 => {
                        let file = tree.app.join(&tree.files[rng.below(4)]);
                        Op::Remove(file)
                    }
                    2 if rng.below(3) == 0 => {
                        // A directory rename — one event pair moves a tree.
                        let from = tree.app.join(&tree.dirs[rng.below(tree.dirs.len())]);
                        let to = tree
                            .app
                            .join(format!("lib/renamed{}", rng.below(3)));
                        Op::Rename(from, to)
                    }
                    2 => {
                        let from = tree.app.join(format!("lib/r{}.dart", rng.below(6)));
                        let to = tree.app.join(format!("lib/r{}.dart", rng.below(6)));
                        Op::Rename(from, to)
                    }
                    3 => Op::Mkdir(tree.app.join(&tree.dirs[rng.below(tree.dirs.len())])),
                    4 => Op::Rmdir(tree.app.join(&tree.dirs[rng.below(2)])),
                    5 => Op::WriteConfig(
                        tree.app.join(".dart_tool/package_config.json"),
                    ),
                    6 if !link.exists() => Op::LinkCreate(link.clone(), tree.dep.clone()),
                    6 => Op::LinkRemove(link.clone()),
                    7 => Op::WriteThroughLink(link.join("lib/d.dart")),
                    8 => Op::WriteIgnored(ignored[rng.below(ignored.len())].clone()),
                    9 if tree.app.join(".dart_tool").is_dir() => {
                        Op::RemoveDartTool(tree.app.join(".dart_tool"))
                    }
                    9 => Op::CreateDartTool(
                        tree.app.join(".dart_tool/package_config.json"),
                    ),
                    10 => Op::WriteIgnored(ignored[rng.below(ignored.len())].clone()),
                    _ => {
                        // An out-of-root dep file — watched through the
                        // package root the plan discovered.
                        Op::Write(tree.dep.join("lib/d.dart"))
                    }
                };
                op.run(&tree, &mut rng);
                let mut dirty = op.dirty();
                // The watcher also reports parent-dir hints on some
                // backends — include them at random to cover both
                // shapes.
                if rng.below(3) == 0
                    && let Some(parent) = dirty.first().and_then(|p| p.parent()) {
                        dirty.push(parent.to_path_buf());
                }
                let previous = snapshot.clone();
                let (diff, _) = snapshot.take_into(&mut plan, &dirty, &unwatched, true);
                let fresh = Snapshot::take(&roots, None, &tree.sdk);
                assert_eq!(
                    snapshot, fresh,
                    "round {round}: maintained snapshot diverged (op {op:?}, dirty {dirty:?})"
                );
                assert_eq!(
                    diff,
                    previous.diff(&fresh),
                    "round {round}: reported diff diverged (op {op:?}, dirty {dirty:?})"
                );
            }
            fs::remove_dir_all(&tree.base).ok();
        }
    }

    /// The fresh-walk answer for the current tree — the oracle every
    /// `check` is compared against.
    fn oracle(tree: &Tree) -> Snapshot {
        Snapshot::take(&tree.roots(), None, &tree.sdk)
    }

    /// Gives the fixture's adaptive barrier real headroom: a tiny
    /// tree's EWMA seeds the wait near the [`WAIT_MIN`] floor — below
    /// real FSEvents latency, so a healthy stream's cookie would time
    /// out spuriously. The priming baseline settles first (its install
    /// re-seeds the bound from the take), then tests that exercise the
    /// journal path (not the timeout path) pin the bound to 500 ms —
    /// the production floor still applies on real trees where delivery
    /// outruns the walk.
    fn generous_wait(freshness: &mut Freshness) {
        freshness.settle_priming();
        freshness.wait_ns = 1_000_000_000.0; // bound = 0.5 s, inside the clamps
    }

    /// Serializes every test that arms a real OS watcher. `fseventsd`
    /// starves a single stream under parallel test churn (verified:
    /// one suite pid got zero live deliveries for 30 s while siblings
    /// delivered), so concurrent live tests throttle each other into
    /// spurious `walk`-flag fallbacks. One guard held for the whole test
    /// keeps delivery deterministic; parking_lot mutexes are unpoisoned,
    /// so a panicking test never wedges the rest.
    static WATCH_SERIAL: Mutex<()> = Mutex::new(());

    /// Takes the watcher-serializing lock for the calling test. Hold the
    /// guard for the test's whole body.
    fn serial() -> parking_lot::MutexGuard<'static, ()> {
        WATCH_SERIAL.lock()
    }

    /// The real OS watcher end to end (FSEvents on this host): an edit
    /// made after the barrier lands must surface in the diff — never a
    /// stale answer — and the journal path must produce the same
    /// snapshot the walk does.
    #[test]
    fn watcher_reports_edits_through_the_journal() {
        let _serial = serial();
        let tree = Tree::build("live");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        assert!(freshness.armed(), "the watcher arms over a real tree");
        let file = tree.app.join("lib/a.dart");

        // A quiet check: nothing changed.
        let diff = freshness.check(&tree.roots());
        assert!(diff.is_empty(), "no edits, no diff");

        // An edit between requests must surface — its delivered event
        // is the journal's to catch, so the check waits for the
        // delivery (an in-flight event may defer exactly one ask
        // under an open trust window; the delivery parks it first).
        fs::write(&file, "var a = 42;\n").expect("edit");
        await_live_delivery(&freshness, &file);
        let diff = freshness.check(&tree.roots());
        assert_eq!(diff.changed, vec![file.clone()], "the edit surfaces");
        assert_eq!(*freshness.snapshot(), oracle(&tree), "journal == walk");

        // A creation is a rebuild, also through the journal.
        let extra = tree.app.join("lib/extra.dart");
        fs::write(&extra, "var e;\n").expect("create");
        await_live_delivery(&freshness, &extra);
        let diff = freshness.check(&tree.roots());
        assert!(diff.rebuild, "an added Dart file rebuilds");
        assert_eq!(*freshness.snapshot(), oracle(&tree));

        // A removal too.
        fs::remove_file(&extra).expect("remove");
        await_live_delivery(&freshness, &extra);
        let diff = freshness.check(&tree.roots());
        assert!(diff.rebuild, "a deleted Dart file rebuilds");
        assert_eq!(*freshness.snapshot(), oracle(&tree));

        drop(freshness);
        fs::remove_dir_all(&tree.base).ok();
    }

    /// How long the live-delivery tests wait on the OS: generous next to
    /// any delivery a loaded host makes — behind a saturated `fseventsd`
    /// the raw FSEvents API was measured at 2.3–3.8 s per event, and a
    /// host running a parallel benchmark lane's watchers was measured
    /// delivering in 20–30 s — and finite, so a substrate that stops
    /// delivering live events still fails these loudly, every run.
    const LIVE_BOUND: Duration = Duration::from_secs(60);

    /// Creates `file` — a name the tree never held — and blocks until
    /// the stream delivers it into the journal's dirty set. A fresh name
    /// can only get there through a live event after the arm: no startup
    /// replay carries it, and nothing else writes `dirty` here (no
    /// injection, no request).
    fn assert_live_delivery(freshness: &Freshness, file: &Path) {
        fs::write(file, "var live = 1;\n").expect("create");
        await_live_delivery(freshness, file);
    }

    /// Blocks until the stream delivers `file` into the journal's dirty
    /// set — the suite's way of giving a real edit its delivery without
    /// a fixed sleep: a saturated `fseventsd` delivers in seconds
    /// (measured), and the wait returns the moment the event lands,
    /// bounded by [`LIVE_BOUND`].
    fn await_live_delivery(freshness: &Freshness, file: &Path) {
        let shared = &freshness.shared;
        let deadline = Instant::now() + LIVE_BOUND;
        let mut journal = shared.journal.lock();
        while !journal.dirty.contains(file) {
            let timed_out = shared.delivered.wait_until(&mut journal, deadline).timed_out();
            assert!(
                !timed_out || journal.dirty.contains(file),
                "the stream never delivered {} within {LIVE_BOUND:?} (walk flag {})",
                file.display(),
                journal.walk,
            );
        }
    }

    /// The real substrate delivers live events, end to end on a temp
    /// tree: files created after the arm reach the journal as dirty
    /// paths — twice, so a stream that only replays its startup, or dies
    /// after its first batch, fails too.
    #[test]
    fn the_substrate_delivers_live_edits() {
        let _serial = serial();
        let tree = Tree::build("live-delivery");
        let freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        assert!(freshness.armed(), "the watcher arms over a real tree");
        assert_live_delivery(&freshness, &tree.app.join("lib/live_one.dart"));
        assert_live_delivery(&freshness, &tree.app.join("lib/deep/live_two.dart"));
        drop(freshness);
        fs::remove_dir_all(&tree.base).ok();
    }

    /// A cool-off's re-arm streams again: the abandon clears the lost
    /// stream's watch set, so the re-armed watcher arms the whole surface
    /// and a file created afterwards reaches the journal. A stale set
    /// made the re-arm see every dir as already watched and arm nothing
    /// — a watcher that never delivered again.
    #[test]
    fn a_rearmed_watcher_delivers_live_edits() {
        let _serial = serial();
        let tree = Tree::build("rearm-live");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        assert!(freshness.armed());
        freshness.abandon("a test-forced strike");
        assert!(!freshness.armed());
        assert!(
            freshness.shared.watched.lock().is_empty(),
            "the lost stream covers nothing"
        );
        // Back-date the cool-off so the next request re-arms at once.
        if let Some((since, _)) = &mut freshness.cool_off {
            *since = Instant::now() - cool_off() * 2;
        }
        let _ = freshness.check(&tree.roots());
        assert!(freshness.armed(), "the cool-off's re-arm restored it");
        assert_live_delivery(&freshness, &tree.app.join("lib/rearmed.dart"));
        drop(freshness);
        fs::remove_dir_all(&tree.base).ok();
    }

    /// The arm predates the first take: construction flags no covering
    /// walk (the old design's mandatory first-request walk is gone),
    /// and the post-baseline window is served by the journal — an edit
    /// made after the baseline arrives as a journal event and merges
    /// without any walk, exactly as the old covering walk's re-take
    /// would have caught it. The journal is driven by injection — a
    /// saturated `fseventsd` can delay delivery past any barrier bound
    /// (see `a_merged_dir_stays_dirty_one_more_request`), and this
    /// test's contract is the design's walk-freedom, not the OS
    /// substrate's latency.
    #[test]
    fn the_arm_predates_the_first_take() {
        let _serial = serial();
        let tree = Tree::build("armwalk");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        assert!(freshness.armed());
        assert!(!freshness.primed(), "the baseline primes in the background");
        assert!(
            !freshness.shared.journal.lock().walk,
            "the arm-before-take leaves no construction window"
        );
        assert!(
            !freshness.first_walk,
            "no by-design covering walk is pending either"
        );
        // The barrier needs headroom before the first check (its
        // install re-seeds the bound from the take).
        freshness.settle_priming();
        freshness.wait_ns = 1_000_000_000.0;
        freshness.journal_by_injection();
        // An edit after the baseline, before the first check: only the
        // armed stream can carry it — the take already ran. The injected
        // dirty entry is the event the stream would have delivered.
        let edit = tree.app.join("lib/after_prime.dart");
        fs::write(&edit, "var p;\n").expect("edit");
        freshness.inject_dirty(&[edit.clone()]);
        let diff = freshness.check(&tree.roots());
        assert_eq!(
            freshness.last_check().mode,
            CheckMode::Journal,
            "the journal path serves the post-baseline edit"
        );
        assert!(diff.rebuild, "a Dart file appeared — the rebuild flag");
        assert_eq!(
            *freshness.snapshot(),
            oracle(&tree),
            "the post-baseline edit arrived — never stale"
        );
        assert!(freshness.primed(), "the first check installed the baseline");
        // The converged tree answers quietly — and the whole scenario,
        // baseline install plus post-baseline edit, never walked once.
        let diff = freshness.check(&tree.roots());
        assert_eq!(freshness.last_check().mode, CheckMode::Journal);
        assert!(diff.is_empty(), "a quiet pass after the merge reports nothing");
        assert_eq!(freshness.walks, 0, "the entire scenario is walk-free");
        fs::remove_dir_all(&tree.base).ok();
    }

    /// The cold posture the daemon's skip arm relies on: `primed` stays
    /// false until a check settles the background take, and the
    /// source-size figure arrives with the baseline — the ledger's
    /// figure off the walk the oracle already paid.
    #[test]
    fn priming_installs_the_baseline_and_the_source_figure() {
        let _serial = serial();
        let tree = Tree::build("primed");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        assert!(!freshness.primed());
        assert_eq!(freshness.dart_bytes(), None, "no figure before the baseline");
        generous_wait(&mut freshness);
        assert!(freshness.primed());
        let expected = oracle(&tree).dart_bytes();
        assert_eq!(freshness.dart_bytes(), Some(expected));
        assert!(expected > 0, "the fixture holds Dart sources");
        fs::remove_dir_all(&tree.base).ok();
    }

    /// The rescan flag / stream error path: a raised `walk` flag
    /// pushes the next request through the full take and resets the
    /// journal.
    #[test]
    fn walk_flag_forces_a_full_take_once() {
        let _serial = serial();
        let tree = Tree::build("flag");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        let file = tree.app.join("lib/a.dart");
        fs::write(&file, "var changed;\n").expect("edit");

        freshness.inject_walk();
        let diff = freshness.check(&tree.roots());
        assert_eq!(diff.changed, vec![file.clone()], "the walk found the edit");
        assert_eq!(freshness.walks, 1, "one fallback walk");
        assert!(
            freshness.armed() || freshness.cool_off.is_some(),
            "the flag never kills the watcher — a transient strike may"
        );

        // The journal continues where the walk left off — a transient
        // cookie strike under parallel load can legitimately cool the
        // watcher off, so the settle accepts a walk pass too. An
        // engaged trust window defers the edit's in-flight event until
        // the stream delivers it — park on the journal until the event
        // lands (bounded by the live bound), then the next check must
        // merge the new bytes: losing the event would be stale
        // forever; deferring one delivery latency is the documented
        // contract.
        fs::write(&file, "var changed2;\n").expect("edit2");
        let settle = Instant::now() + LIVE_BOUND;
        while *freshness.snapshot() != oracle(&tree) {
            let mut journal = freshness.shared.journal.lock();
            if journal.dirty.contains(&file) {
                drop(journal);
                let _ = freshness.check(&tree.roots());
            } else {
                let timed_out = freshness
                    .shared
                    .delivered
                    .wait_until(&mut journal, settle)
                    .timed_out();
                assert!(
                    !timed_out,
                    "the edit's event never delivered — the journal lost it"
                );
            }
        }
        assert_eq!(
            *freshness.snapshot(),
            oracle(&tree),
            "the journal (or its walk fallback) never loses the edit"
        );
        fs::remove_dir_all(&tree.base).ok();
    }

    /// The dirty-cap overflow: flooding the journal clears it and
    /// forces one full take rather than growing unbounded.
    #[test]
    fn dirty_cap_overflow_walks() {
        let _serial = serial();
        let tree = Tree::build("cap");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        let flood: Vec<PathBuf> = (0..DIRTY_CAP + 1)
            .map(|i| tree.app.join(format!("lib/f{i}.dart")))
            .collect();
        freshness.inject_dirty(&flood);
        let diff = freshness.check(&tree.roots());
        assert_eq!(freshness.walks, 1, "the cap forced a walk");
        // The flood's files never existed — the walk reports nothing.
        assert!(diff.is_empty(), "no real change");
        assert!(
            freshness.shared.journal.lock().dirty.is_empty(),
            "the journal cleared"
        );
        fs::remove_dir_all(&tree.base).ok();
    }

    /// A barrier that times out walks but is "slow", never a strike —
    /// the cookie keeps listening from `late`, and only a cookie
    /// expiring undelivered counts against the stream. Three expired
    /// late cookies cool the watcher off; the cool-off's re-arm
    /// restores it on a later request. Deterministic — the late set is
    /// injected with already-expired deadlines, so no real-time wait or
    /// OS event can race the sweep.
    #[test]
    fn expired_late_cookies_cool_off_then_rearm() {
        let _serial = serial();
        let tree = Tree::build("timeout");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        assert!(freshness.armed());
        // Three undelivered late cookies, deadlines already passed —
        // the sweep counts each as a strike.
        for index in 0..3 {
            let name = format!("{COOKIE_PREFIX}expired-{index}");
            freshness.shared.journal.lock().late.insert(name.clone());
            freshness
                .late
                .push((name, Instant::now() - Duration::from_millis(1)));
        }
        freshness.sweep_late();
        assert_eq!(freshness.timeouts, 0, "the abandon cleared the count");
        assert!(
            !freshness.armed(),
            "three expired cookies cooled the watcher off"
        );
        // Back-date the cool-off so the next request re-arms without a
        // real-time wait.
        if let Some((since, _)) = &mut freshness.cool_off {
            *since = Instant::now() - cool_off() * 2;
        }
        let _ = freshness.check(&tree.roots());
        assert!(freshness.armed(), "the cool-off's re-arm restored it");
        fs::remove_dir_all(&tree.base).ok();
    }

    /// A late cookie's delivery proves the stream alive: a barrier that
    /// timed out then delivered inside its expiry clears the strike
    /// count — the stream is slow, not dead.
    #[test]
    fn a_late_cookie_resets_strikes() {
        let _serial = serial();
        let tree = Tree::build("late");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        assert!(freshness.armed());
        freshness.timeouts = 2; // the pre-existing count a delivery clears
        // One pending late cookie and one already delivered — the
        // drain removed the delivered name from `journal.late`.
        let pending = format!("{COOKIE_PREFIX}pending");
        let delivered = format!("{COOKIE_PREFIX}delivered");
        freshness.shared.journal.lock().late.insert(pending.clone());
        let deadline = Instant::now() + late_expiry();
        freshness.late.push((pending, deadline));
        freshness.late.push((delivered, deadline));
        freshness.sweep_late();
        assert_eq!(freshness.timeouts, 0, "the delivery cleared the count");
        assert_eq!(
            freshness.late.len(),
            1,
            "the delivered name left, the pending one stays"
        );
        fs::remove_dir_all(&tree.base).ok();
    }

    /// Opens the health trust window deterministically: the liveness
    /// proof stamped a beat after the last doubt boundary, and the
    /// straggler bookkeeping cleared — a barrier that missed its bound
    /// under parallel-suite load keeps listening from `late` (measured:
    /// cookie delivery in seconds against a 500 ms bound), and that
    /// pending liveness evidence is not the mechanism under test here.
    /// The journal's catch of edits and the cookies' real writes and
    /// deliveries stay real.
    fn open_trust(freshness: &mut Freshness) {
        freshness.timeouts = 0;
        freshness.health_since = Some(Instant::now() - Duration::from_secs(1));
        freshness.late.clear();
        let mut journal = freshness.shared.journal.lock();
        journal.late.clear();
        journal.proven = Some(Instant::now());
    }

    /// The trust window narrows on every doubt: a pending strike, a
    /// straggler under evaluation, a carried dir, an unproven stream
    /// (no delivery observed since the last arm or restart), a stale
    /// liveness proof, a non-empty journal, the walk flag — each ALONE
    /// keeps the request on today's full barrier path. Driven by
    /// injection: eligibility is the unit, the OS is not involved.
    #[test]
    fn the_trust_window_narrows_on_doubt() {
        let _serial = serial();
        let tree = Tree::build("trust-narrow");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        freshness.journal_by_injection();
        assert!(freshness.armed());
        open_trust(&mut freshness);
        assert!(freshness.trust_ready(), "the window opens");
        freshness.timeouts = 1;
        assert!(!freshness.trust_ready(), "a pending strike closes it");
        freshness.timeouts = 0;
        freshness
            .late
            .push((format!("{COOKIE_PREFIX}straggler"), Instant::now() + late_expiry()));
        assert!(!freshness.trust_ready(), "a straggler closes it");
        freshness.late.clear();
        freshness.carry_dirs.push(tree.app.join("lib"));
        assert!(!freshness.trust_ready(), "a carried dir closes it");
        freshness.carry_dirs.clear();
        // A delivery predating the last arm/restart proves the OLD
        // stream, not the current one — the window stays shut until
        // the current stream delivers.
        freshness.health_since = Some(Instant::now());
        assert!(!freshness.trust_ready(), "an unproven stream closes it");
        open_trust(&mut freshness);
        freshness.shared.journal.lock().proven =
            Some(Instant::now() - TRUST_WINDOW - Duration::from_secs(1));
        assert!(!freshness.trust_ready(), "a stale proof closes it");
        freshness.shared.journal.lock().proven = Some(Instant::now());
        freshness.inject_dirty(&[tree.app.join("lib/a.dart")]);
        assert!(!freshness.trust_ready(), "a landed edit closes it");
        freshness.shared.journal.lock().dirty.clear();
        freshness.inject_walk();
        assert!(!freshness.trust_ready(), "the walk flag closes it");
        freshness.shared.journal.lock().walk = false;
        assert!(
            freshness.trust_ready(),
            "the window reopens once the doubt clears"
        );
        fs::remove_dir_all(&tree.base).ok();
    }

    /// Adversarial (edit-between-turns): an edit landing between a
    /// trusted serve and the next ask MUST surface — the journal's
    /// dirty set catches the delivered event and the next ask serves
    /// it, never a stale answer. The journal is driven with real
    /// events and the trusted serve writes real cookies; only the
    /// barrier's OS wait is elided by injection (a saturated
    /// `fseventsd` delivers in seconds — the barrier is not the
    /// mechanism under test, the journal catch is).
    #[test]
    fn a_trusted_serve_serves_edits_between_turns() {
        let _serial = serial();
        let tree = Tree::build("trust-edit");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        freshness.journal_by_injection();
        assert!(freshness.armed());
        // Consume the install's covering walk, then merge the arm's own
        // events; whatever a saturated host still holds in flight is
        // dot-path noise the merge ignores by design — dropped.
        let _ = freshness.check(&tree.roots());
        std::thread::sleep(Duration::from_millis(600));
        let _ = freshness.check(&tree.roots());
        freshness.shared.journal.lock().dirty.clear();
        freshness.carry_dirs.clear();
        open_trust(&mut freshness);
        let diff = freshness.check(&tree.roots());
        assert_eq!(
            freshness.trusted, 1,
            "the quiet turn served from the journal verdict alone"
        );
        assert!(diff.is_empty(), "a quiet turn diffs nothing; {diff:?}");
        // THE edit: between the trusted serve and the next ask. Its
        // delivery is awaited off the ask (the suite's live bound), so
        // the ask under test is the first one after the journal holds
        // the event — the mechanism the window relies on.
        let file = tree.app.join("lib/a.dart");
        fs::write(&file, "var trusted = 1;\n").expect("edit");
        await_live_delivery(&freshness, &file);
        let diff = freshness.check(&tree.roots());
        assert_eq!(
            diff.changed,
            vec![file.clone()],
            "the edit between turns surfaces (trusted {})",
            freshness.trusted
        );
        assert_eq!(
            *freshness.snapshot(),
            oracle(&tree),
            "journal == walk — never stale"
        );
        // The trusted serve's cookie files left with the ask that
        // replaced them: no strays.
        let strays = fs::read_dir(tree.app.join(".dart_tool/dartq"))
            .expect("cookie dir")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(COOKIE_PREFIX))
            .count();
        assert_eq!(strays, 0, "trusted cookies are swept by the next ask");
        fs::remove_dir_all(&tree.base).ok();
    }

    /// Adversarial (rapid-edit cadence): edits chasing asks
    /// back-to-back under an engaged window are never lost — the
    /// deferred ones (events still in flight at their ask) surface at
    /// the first ask after delivery, and the settled tree equals the
    /// walk's, byte for byte. Real events, real cookie writes; the
    /// barrier's OS wait elided by injection (the journal catch is the
    /// mechanism under test).
    #[test]
    fn a_rapid_edit_cadence_is_never_lost_under_trust() {
        let _serial = serial();
        let tree = Tree::build("trust-cadence");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        freshness.journal_by_injection();
        assert!(freshness.armed());
        let _ = freshness.check(&tree.roots());
        std::thread::sleep(Duration::from_millis(600));
        let _ = freshness.check(&tree.roots());
        freshness.shared.journal.lock().dirty.clear();
        freshness.carry_dirs.clear();
        open_trust(&mut freshness);
        let _ = freshness.check(&tree.roots());
        assert_eq!(
            freshness.trusted, 1,
            "the quiet phase opened the window"
        );
        // The cadence: an edit lands with every ask, no spacing — the
        // events chase the requests through the window.
        let file = tree.app.join("lib/a.dart");
        for round in 0..25 {
            fs::write(&file, format!("var cadence{round} = {round};\n")).expect("edit");
            let _ = freshness.check(&tree.roots());
        }
        // Settle: the final edit surfaces at the first ask after its
        // delivery. The wait parks until the journal gains the path
        // (bounded by the suite's live bound); any ask that finds it
        // re-stats the file as it is NOW — the final content — so the
        // maintained snapshot converges to the walk's, byte for byte.
        let deadline = Instant::now() + LIVE_BOUND;
        while *freshness.snapshot() != oracle(&tree) {
            let mut journal = freshness.shared.journal.lock();
            if journal.dirty.contains(&file) {
                drop(journal);
                let _ = freshness.check(&tree.roots());
            } else {
                let timed_out = freshness
                    .shared
                    .delivered
                    .wait_until(&mut journal, deadline)
                    .timed_out();
                assert!(
                    !timed_out,
                    "the cadence lost an edit under the trust window"
                );
            }
        }
        assert_eq!(
            *freshness.snapshot(),
            oracle(&tree),
            "the cadence lost an edit under the trust window"
        );
        fs::remove_dir_all(&tree.base).ok();
    }

    /// Adversarial (stream restart): a moved watch set restarts the
    /// macOS stream (the predecessor's in-flight events die with it)
    /// and drops the trust window to zero immediately — the doubt
    /// boundary moves to the restart, so a liveness proof from the
    /// DEAD stream can never re-open the window; no ask after the
    /// restart serves from the verdict alone until the CURRENT stream
    /// delivers (a proof postdating the restart). The real restart is
    /// the mechanism; the barrier's OS wait is elided by injection
    /// (which also means nothing ever stamps a post-restart proof, so
    /// trust stays shut for the whole test — the strongest form).
    #[cfg(target_os = "macos")]
    #[test]
    fn a_stream_restart_drops_the_trust_window() {
        let _serial = serial();
        let tree = Tree::build("trust-restart");
        let other = Tree::build("trust-restart-b");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        freshness.journal_by_injection();
        assert!(freshness.armed());
        let _ = freshness.check(&tree.roots());
        std::thread::sleep(Duration::from_millis(600));
        let _ = freshness.check(&tree.roots());
        freshness.shared.journal.lock().dirty.clear();
        freshness.carry_dirs.clear();
        open_trust(&mut freshness);
        let _ = freshness.check(&tree.roots());
        assert_eq!(freshness.trusted, 1, "the window was open");
        // The restart shape: the root set moves (a pool join).
        let _ = freshness.check(&other.roots());
        let proven = freshness.shared.journal.lock().proven;
        assert!(
            freshness
                .health_since
                .is_some_and(|since| proven.is_none_or(|proven| proven <= since)),
            "the restart moved the doubt boundary past the old proof — trust is zero"
        );
        // Neither the covering walk's ask nor the quiet one after it
        // trusts.
        let _ = freshness.check(&other.roots());
        let _ = freshness.check(&other.roots());
        assert_eq!(freshness.trusted, 1, "no ask after the restart trusted");
        fs::remove_dir_all(&tree.base).ok();
        fs::remove_dir_all(&other.base).ok();
    }

    /// Adversarial (fresh daemon — one proven round trip earns initial
    /// trust): before the stream's FIRST delivery is observed, no ask
    /// trusts — the proof does not exist. After one delivery lands on
    /// the current stream (a cookie event the drain really recorded,
    /// the same stamp a held barrier's cookie makes), the very next
    /// ask serves from the journal verdict alone — no HEALTHY_FOR-style
    /// warm-up in between — and the window it opened still serves an
    /// edit landing between turns. Deterministic: the barrier is
    /// elided by injection (which also means no barrier ever writes a
    /// cookie, so the ONLY thing that can stamp the liveness proof is
    /// the marker's real delivery), while the journal's catch of edits
    /// and the drain's recording stay real.
    #[test]
    fn a_first_delivery_earns_initial_trust() {
        let _serial = serial();
        let tree = Tree::build("trust-first");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        freshness.journal_by_injection();
        assert!(freshness.armed());
        // The install's stream move and its covering walk consume the
        // first ask; whatever in-flight dot-path noise survived is
        // dropped so the next ask's verdict is the gate's alone.
        let _ = freshness.check(&tree.roots());
        std::thread::sleep(Duration::from_millis(600));
        let _ = freshness.check(&tree.roots());
        freshness.shared.journal.lock().dirty.clear();
        freshness.carry_dirs.clear();
        assert_eq!(freshness.trusted, 0, "nothing proved the stream yet");
        assert!(
            !freshness.trust_ready(),
            "no delivery observed — the window cannot be open"
        );
        // The proof: a cookie-named file inside the watched tree,
        // delivered by the real stream and recorded by the real drain.
        let marker = tree
            .app
            .join(".dart_tool/dartq")
            .join(format!("{COOKIE_PREFIX}{}-proof", std::process::id()));
        fs::write(&marker, b"").expect("proof cookie");
        {
            let shared = &freshness.shared;
            let deadline = Instant::now() + LIVE_BOUND;
            let mut journal = shared.journal.lock();
            while journal.proven.is_none() {
                let timed_out = shared.delivered.wait_until(&mut journal, deadline).timed_out();
                assert!(
                    !timed_out || journal.proven.is_some(),
                    "the stream never delivered the proof cookie within {LIVE_BOUND:?}"
                );
            }
        }
        let _ = fs::remove_file(&marker);
        // One proven round trip: the next ask trusts, with no age gate
        // between the delivery and this ask. The wait window (seconds
        // under load) can admit the arm's straggler redeliveries —
        // FSEvents re-reports the watched roots as dir events with
        // trailing slashes — so drain once more and retry past any
        // that still race the ask; the proof itself stays valid.
        std::thread::sleep(Duration::from_millis(600));
        let _ = freshness.check(&tree.roots());
        let mut diff = Diff::default();
        for _ in 0..4 {
            freshness.shared.journal.lock().dirty.clear();
            freshness.carry_dirs.clear();
            diff = freshness.check(&tree.roots());
            if freshness.trusted > 0 {
                break;
            }
        }
        assert_eq!(
            freshness.trusted, 1,
            "the first delivery after the arm earned initial trust"
        );
        assert!(diff.is_empty(), "a quiet turn diffs nothing; {diff:?}");
        assert_eq!(*freshness.snapshot(), oracle(&tree), "journal == walk");
        // The window it opened still serves edits between turns — the
        // staleness bar rides along (delivered edits never defer).
        let file = tree.app.join("lib/a.dart");
        fs::write(&file, "var first = 1;\n").expect("edit");
        await_live_delivery(&freshness, &file);
        let diff = freshness.check(&tree.roots());
        assert_eq!(
            diff.changed,
            vec![file.clone()],
            "the edit between trusted turns surfaces (trusted {})",
            freshness.trusted
        );
        assert_eq!(*freshness.snapshot(), oracle(&tree), "never stale");
        fs::remove_dir_all(&tree.base).ok();
    }

    /// Adversarial (missed bound — a bound miss the straggler proves
    /// alive is liveness evidence, not doubt): a barrier that misses
    /// its wait bound must NOT move the doubt boundary. The straggler
    /// it registers holds the window shut while it pends, and the ask
    /// after its delivery re-opens it — no streak to re-earn, however
    /// young the boundary. The timeout arm runs for REAL (no
    /// injection): a second watched entry whose dir the stream does
    /// not cover forces the miss deterministically (its cookie can
    /// never deliver, however calm the host), while the real stream
    /// eventually delivers the genuine cookie, whose delete event then
    /// resolves the straggler's name exactly the way a late delivery
    /// does — and that delivery re-stamps the liveness proof on the
    /// far side of the UNMOVED boundary. Initial engagement is opened
    /// by the standard hand hook (`open_trust`); the miss, the pin and
    /// the re-opening are the mechanism under test.
    #[test]
    fn a_missed_bound_then_delivery_reopens_trust() {
        let _serial = serial();
        let tree = Tree::build("trust-miss");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        assert!(freshness.armed());
        // Settle the install's restart and its noise; under a loaded
        // host the settle barriers may miss and leave stragglers — the
        // hand hook clears that bookkeeping (its expiry is not the
        // mechanism under test here).
        let _ = freshness.check(&tree.roots());
        std::thread::sleep(Duration::from_millis(600));
        let _ = freshness.check(&tree.roots());
        freshness.shared.journal.lock().dirty.clear();
        freshness.carry_dirs.clear();
        open_trust(&mut freshness);
        let _ = freshness.check(&tree.roots());
        assert_eq!(freshness.trusted, 1, "the window was open");
        let trusted_before = freshness.trusted;
        let boundary = freshness.health_since;
        // The forced miss: a watched entry outside every streamed dir —
        // its cookie file writes fine but never delivers, so the
        // barrier cannot hold however calm the host is.
        let outside = tree.base.join("outside");
        fs::create_dir_all(&outside).expect("outside dir");
        freshness.shared.watched.lock().push(Watch {
            cookie_dir: outside.clone(),
            dir: outside.clone(),
            root: false,
        });
        let missed = freshness.sync();
        freshness.shared.watched.lock().pop();
        assert_eq!(
            missed,
            Sync::Timeout,
            "the uncovered cookie missed the bound"
        );
        assert_eq!(
            freshness.health_since, boundary,
            "the missed bound did not move the doubt boundary"
        );
        assert_eq!(freshness.timeouts, 0, "a miss is not a strike");
        // The pending straggler holds the window shut...
        assert!(
            !freshness.trust_ready(),
            "the straggler under evaluation closes it"
        );
        // ...and its delivery — the real cookie's delete event
        // resolving the shared name — re-opens it. Wait for the
        // resolution, then poll the GATE itself (a check with the
        // gate still shut would run a real barrier and register a
        // fresh straggler, wedging the loop on a loaded host); stray
        // redeliveries the wait admitted are drained each poll.
        let names: Vec<String> = freshness.late.iter().map(|(name, _)| name.clone()).collect();
        assert_eq!(names.len(), 1, "one straggler pends; {names:?}");
        {
            let shared = &freshness.shared;
            let deadline = Instant::now() + LIVE_BOUND;
            let mut journal = shared.journal.lock();
            while names.iter().any(|name| journal.late.contains(name)) {
                let timed_out = shared.delivered.wait_until(&mut journal, deadline).timed_out();
                assert!(
                    !timed_out,
                    "the straggler never resolved within {LIVE_BOUND:?}"
                );
            }
        }
        let deadline = Instant::now() + LIVE_BOUND;
        while !{
            freshness.sweep_late();
            freshness.shared.journal.lock().dirty.clear();
            freshness.carry_dirs.clear();
            freshness.trust_ready()
        } {
            assert!(
                Instant::now() < deadline,
                "the window never re-opened after the straggler's delivery"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        let diff = freshness.check(&tree.roots());
        assert!(
            freshness.trusted > trusted_before,
            "the ask after the straggler's delivery re-opened the window"
        );
        assert!(diff.is_empty(), "a quiet turn diffs nothing; {diff:?}");
        assert_eq!(*freshness.snapshot(), oracle(&tree), "journal == walk");
        fs::remove_dir_all(&tree.base).ok();
    }

    /// The adaptive wait bound: seeded by the first walk, moved by
    /// every measured take, and clamped into [`WAIT_MIN`]..=[`WAIT_MAX`]
    /// — a large tree's barrier waits toward the cap while a trivial
    /// tree's waits near the floor.
    #[test]
    fn the_wait_bound_adapts_to_walk_cost() {
        let _serial = serial();
        let tree = Tree::build("wait");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        let bound = freshness.wait_bound();
        assert!(
            bound >= WAIT_MIN && bound <= WAIT_MAX,
            "the seeded bound sits inside the clamps; {bound:?}"
        );
        // A measured walk feeds the EWMA: bound = 0.5 * EWMA(walk) —
        // the average moves monotonically toward the observation.
        let ewma_before = freshness.wait_ns;
        freshness.observe_walk(Duration::from_secs(4));
        assert!(
            freshness.wait_ns > ewma_before,
            "a slower measured walk raises the EWMA"
        );
        // The clamp: a walk slower than any request's wait cannot push
        // the bound past the cap.
        for _ in 0..8 {
            freshness.observe_walk(Duration::from_secs(10));
        }
        assert_eq!(
            freshness.wait_bound(),
            WAIT_MAX,
            "the cap holds under a huge EWMA"
        );
        for _ in 0..64 {
            freshness.observe_walk(Duration::from_nanos(1));
        }
        assert_eq!(
            freshness.wait_bound(),
            WAIT_MIN,
            "the floor holds under a tiny EWMA"
        );
        fs::remove_dir_all(&tree.base).ok();
    }

    /// A target deleted between the plan and the arm is dropped, not a
    /// stream failure: the rest of the surface stays armed and the
    /// reconcile reports `Dropped`. Deterministic — the dir is armed
    /// first (a real stream target), then deleted while the plan still
    /// wants it: the next reconcile's `watch` on the missing dir fails
    /// with the target no longer existing, the drop path fires, and the
    /// stream keeps every other target.
    #[test]
    fn a_vanished_reconcile_target_drops_not_abandons() {
        let _serial = serial();
        let tree = Tree::build("vanish");
        let gone = tree.app.join("lib/gonepkg");
        fs::create_dir_all(&gone).expect("gone dir");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        assert!(freshness.armed());
        // Seed the dir into the plan's walked set and arm it.
        freshness.plan.walked.push(gone.clone());
        let armed = freshness.watcher.as_mut().map(|w| {
            w.reconcile(&freshness.plan, &freshness.shared)
        });
        assert!(matches!(armed, Some(Arm::Ok)));
        let watched: Vec<PathBuf> = freshness
            .shared
            .watched
            .lock()
            .iter()
            .map(|w| w.dir.clone())
            .collect();
        assert!(
            watched.iter().any(|dir| gone.starts_with(dir) || dir == &gone
                || gone.starts_with(dir.parent().unwrap_or(dir))),
            "the new target armed; {watched:?}"
        );
        // Delete it while the plan still wants it — the next reconcile
        // rebuilds `want` (its `is_dir` now fails, so it drops off the
        // want list) while `have` still arms it: the unwatch path runs.
        fs::remove_dir_all(&gone).expect("vanish");
        let before = freshness.watcher.as_ref().map(|w| w.errors).unwrap_or(0);
        let outcome = freshness.watcher.as_mut().map(|w| {
            w.reconcile(&freshness.plan, &freshness.shared)
        });
        // Unwatching a deleted dir is not a stream failure — the rest
        // of the surface stays armed.
        assert!(
            matches!(outcome, Some(Arm::Ok | Arm::Dropped)),
            "a vanished target is dropped, not a failure; {outcome:?}"
        );
        assert!(freshness.armed(), "the stream keeps the other roots");
        assert_eq!(
            freshness.watcher.as_ref().map(|w| w.errors),
            Some(before),
            "a vanished target is not a stream error"
        );
        fs::remove_dir_all(&tree.base).ok();
    }

    /// A cookie dir that refuses the create fails the barrier without
    /// touching the strike count — a read-only tree is an environment
    /// fact, not watcher-dead evidence: the request walks, the watcher
    /// stays armed, and the next writable request hits the journal.
    #[test]
    fn unwritable_cookie_dir_walks_once_and_stays_armed() {
        let _serial = serial();
        let tree = Tree::build("blocked");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        assert!(freshness.armed());
        let blocked = tree.base.join("blocked");
        fs::create_dir_all(&blocked).expect("blocked dir");
        let mut permissions = fs::metadata(&blocked).expect("meta").permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&blocked, permissions).expect("readonly");
        freshness.shared.watched.lock().push(Watch {
            cookie_dir: blocked.clone(),
            dir: blocked.clone(),
            root: false,
        });
        let walks = freshness.walks;
        let diff = freshness.check(&tree.roots());
        assert!(diff.is_empty(), "no real change; {diff:?}");
        assert_eq!(freshness.walks, walks + 1, "the blocked barrier walked");
        assert_eq!(freshness.timeouts, 0, "a blocked barrier is not a strike");
        assert!(freshness.armed(), "the watcher stands");
        // The walk's re-arm replaced `watched` with the plan's real
        // streams — the blocked entry is already gone. Restore the dir
        // and the next request hits the journal again.
        let mut permissions = fs::metadata(&blocked).expect("meta").permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        fs::set_permissions(&blocked, permissions).expect("writable");
        let file = tree.app.join("lib/a.dart");
        fs::write(&file, "var after;\n").expect("edit");
        std::thread::sleep(Duration::from_millis(600));
        let diff = freshness.check(&tree.roots());
        assert_eq!(diff.changed, vec![file], "the journal served the edit");
        fs::remove_dir_all(&tree.base).ok();
    }

    /// The diagnostics of a healthy request: the journal path answers
    /// with no walk reason and the dirty count it merged. The journal is
    /// driven by injection (the barrier holds without stream proof), and
    /// the arm's covering walk is drained first — the second request is
    /// the journal answer under test.
    #[test]
    fn the_diag_reports_the_journal_path() {
        let _serial = serial();
        let tree = Tree::build("diag-journal");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        freshness.journal_by_injection();
        assert!(freshness.armed());
        // Drain the arm's own events and its covering walk.
        std::thread::sleep(Duration::from_millis(600));
        let _ = freshness.check(&tree.roots());
        freshness.carry_dirs.clear();

        let file = tree.app.join("lib/a.dart");
        fs::write(&file, "var diag;\n").expect("edit");
        freshness.inject_dirty(&[file.clone()]);
        let diff = freshness.check(&tree.roots());
        let diag = freshness.last_check();
        assert_eq!(diag.mode, CheckMode::Journal);
        assert_eq!(diag.walk_reason, None, "the journal path ran no walk");
        assert_eq!(diag.changed, 1, "one dirty path surfaced; {diff:?}");
        fs::remove_dir_all(&tree.base).ok();
    }

    /// The diag names its by-design covering walks `First` — never a
    /// degradation. Under the arm-before-take design construction flags
    /// NO walk (the stream is armed before the baseline take runs, so
    /// nothing is left uncovered — the old construction-time `First`
    /// walk is gone); the by-design `First` window that remains is the
    /// macOS stream restart a live reconcile forces when the watch set
    /// moves (a root-set change): its in-flight events die with the old
    /// stream, and the next request's covering walk names itself
    /// `First` because the restart was by design, not degradation.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_diag_names_the_first_walk() {
        let _serial = serial();
        let tree = Tree::build("diag-first");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        assert!(freshness.armed());
        assert!(
            !freshness.first_walk,
            "the arm predates the take — no walk is pending at construction"
        );
        // The first check never reports the by-design reason: nothing
        // was left uncovered (its reason is the journal's or a fallback's).
        let _ = freshness.check(&tree.roots());
        assert_ne!(
            freshness.last_check().walk_reason,
            Some(WalkReason::First),
            "construction reports no covering walk"
        );
        // A root-set move restarts the macOS stream — the by-design
        // window. The rebase itself reports `Rebase`; the NEXT request
        // consumes the restart's covering walk as `First`. The journal
        // is driven by injection so the barrier holds without stream
        // proof — this test names the reason, not the OS latency.
        let other = Tree::build("diag-first-b");
        freshness.journal_by_injection();
        let _ = freshness.check(&other.roots());
        let _ = freshness.check(&other.roots());
        let diag = freshness.last_check();
        assert_eq!(diag.mode, CheckMode::Walk);
        assert_eq!(diag.walk_reason, Some(WalkReason::First));
        assert!(!freshness.first_walk, "the walk spent it");
        fs::remove_dir_all(&tree.base).ok();
        fs::remove_dir_all(&other.base).ok();
    }

    /// The unwritable-cookie fixture (see
    /// `unwritable_cookie_dir_walks_once_and_stays_armed`) reports
    /// `CookieBlocked` — a degradation, distinct from the by-design
    /// walks.
    #[test]
    fn the_diag_names_a_blocked_cookie() {
        let _serial = serial();
        let tree = Tree::build("diag-blocked");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        assert!(freshness.armed());
        let blocked = tree.base.join("blocked");
        fs::create_dir_all(&blocked).expect("blocked dir");
        let mut permissions = fs::metadata(&blocked).expect("meta").permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&blocked, permissions).expect("readonly");
        freshness.shared.watched.lock().push(Watch {
            cookie_dir: blocked.clone(),
            dir: blocked.clone(),
            root: false,
        });
        let _ = freshness.check(&tree.roots());
        let diag = freshness.last_check();
        assert_eq!(diag.mode, CheckMode::Walk);
        assert_eq!(diag.walk_reason, Some(WalkReason::CookieBlocked));
        let mut permissions = fs::metadata(&blocked).expect("meta").permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        fs::set_permissions(&blocked, permissions).expect("writable");
        fs::remove_dir_all(&tree.base).ok();
    }

    /// The reasons are distinct values — the daemon's degradation gate
    /// keys on them, so a by-design walk must never read as a refusal.
    #[test]
    fn walk_reasons_are_distinguishable() {
        assert_ne!(WalkReason::First, WalkReason::CookieBlocked);
        assert_ne!(WalkReason::Rebase, WalkReason::StreamLost);
        assert_ne!(WalkReason::CookieBlocked, WalkReason::CookieTimeout);
        assert_ne!(WalkReason::Unarmed, WalkReason::StreamLost);
    }

    /// The root-set change: a pool join rebases — new roots' files are
    /// new inputs, and the watcher re-arms on the new surface.
    #[test]
    fn root_change_rebases_and_rearms() {
        let _serial = serial();
        let tree = Tree::build("rebase");
        let other = Tree::build("rebase2");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        let diff = freshness.check(&other.roots());
        // The new root's files all arrive as additions.
        assert!(diff.rebuild, "a joined root rebuilds");
        assert_eq!(*freshness.snapshot(), oracle(&other));
        // The journal now covers the new root.
        let file = other.app.join("lib/a.dart");
        fs::write(&file, "var rebased;\n").expect("edit");
        let diff = freshness.check(&other.roots());
        assert_eq!(diff.changed, vec![file]);
        fs::remove_dir_all(&tree.base).ok();
        fs::remove_dir_all(&other.base).ok();
    }

    /// The watcher-error path is the same `walk` flag the rescan
    /// carries — `inject_walk` covers it (the drain folds stream
    /// errors into the flag); this test asserts the journal still
    /// resets cleanly after one.
    #[test]
    fn journal_resets_after_error_walk() {
        let _serial = serial();
        let tree = Tree::build("err");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        freshness.inject_walk();
        let _ = freshness.check(&tree.roots());
        assert!(
            !freshness.shared.journal.lock().walk,
            "the flag cleared with the walk"
        );
        fs::remove_dir_all(&tree.base).ok();
    }

    /// macOS FSEvents: a reconcile that watches or unwatches on a live
    /// stream restarts it from `now` and drops the other roots' events
    /// inside the window — the journal must flag a walk so the next
    /// request re-covers once, then converges on a no-op reconcile.
    #[cfg(target_os = "macos")]
    #[test]
    fn reconcile_on_a_live_stream_flags_one_walk() {
        let _serial = serial();
        let tree = Tree::build("gap");
        let other = Tree::build("gap2");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        assert!(freshness.armed());
        let _ = freshness.check(&tree.roots());
        assert!(
            !freshness.shared.journal.lock().walk,
            "a no-op reconcile flags nothing"
        );
        // A rebase rewatches the whole set on the live stream.
        let _ = freshness.check(&other.roots());
        assert!(
            freshness.shared.journal.lock().walk,
            "the stream restart must flag a covering walk"
        );
        let _ = freshness.check(&other.roots());
        assert!(
            !freshness.shared.journal.lock().walk,
            "the covering walk's reconcile is a no-op — converged"
        );
        fs::remove_dir_all(&tree.base).ok();
        fs::remove_dir_all(&other.base).ok();
    }

    /// A directory that appeared in a merge stays dirty for exactly one
    /// more request — the window inotify leaves between the merge's own
    /// subtree walk and the new dir's armed watch. A file created in
    /// that window is caught by the second pass; after it, the dir is
    /// dropped and never re-walked per request. The journal is driven
    /// by injection: the dirty set is the test's own report of what the
    /// watcher saw, and the sync barrier adds nothing to what this
    /// verifies (a saturated `fseventsd` can delay delivery past any
    /// barrier bound).
    #[test]
    fn a_merged_dir_stays_dirty_one_more_request() {
        let _serial = serial();
        let tree = Tree::build("carry");
        let dir = tree.app.join("lib/newdir");
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(dir.join("one.dart"), "var one = 1;\n").expect("write");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        freshness.journal_by_injection();
        assert!(freshness.armed());
        // Drain the arm's own events (the `.dart_tool/dartq` create,
        // plus any pre-start replay macOS redelivers) so the injected
        // merge is the only mover. A replayed ancestor dir would merge
        // as a subtree and carry, poisoning the next request's
        // consumed set — drop that carry.
        std::thread::sleep(Duration::from_millis(600));
        let _ = freshness.check(&tree.roots());
        freshness.carry_dirs.clear();
        // The watcher reports the dir; the merge subtree-walks it. An
        // OS rescan flag racing the request takes the walk instead —
        // re-inject and retry; a merge carry must eventually appear.
        freshness.inject_dirty(&[dir.clone()]);
        let mut carried = false;
        for _ in 0..20 {
            let walks = freshness.walks;
            let _ = freshness.check(&tree.roots());
            if freshness.carry_dirs.iter().any(|d| dir.starts_with(d)) {
                carried = true;
                break;
            }
            assert!(
                freshness.walks > walks,
                "the merge neither carried nor walked — the dirty path was lost"
            );
            freshness.inject_dirty(&[dir.clone()]);
        }
        assert!(
            carried,
            "the merged dir is carried one request; {:?}",
            freshness.carry_dirs
        );
        // The window: a file lands before the dir's own watch could
        // exist — drop every journal entry so only the carry can
        // surface it.
        fs::write(dir.join("two.dart"), "var two = 2;\n").expect("gap write");
        freshness.shared.journal.lock().dirty.clear();
        let diff = freshness.check(&tree.roots());
        assert!(
            diff.rebuild,
            "the carried dir re-merges and finds the gap file"
        );
        assert_eq!(*freshness.snapshot(), oracle(&tree));
        // Consumed carries do not re-carry: carries drain once the
        // events settle — exactly one extra pass per merge, never a
        // per-request re-walk. A late OS event can legitimately carry
        // the dir once more, so allow the drain a few requests.
        for _ in 0..4 {
            if freshness.carry_dirs.is_empty() {
                break;
            }
            freshness.shared.journal.lock().dirty.clear();
            let _ = freshness.check(&tree.roots());
        }
        assert!(
            freshness.carry_dirs.is_empty(),
            "carries drain after their extra pass; {:?}",
            freshness.carry_dirs
        );
        fs::remove_dir_all(&tree.base).ok();
    }

    /// `flutter clean` deletes `.dart_tool` wholesale, stranding the
    /// stored cookie dir — the next barrier re-derives it, recreates
    /// `.dart_tool/dartq` for the walked root, and the request still
    /// takes the journal path instead of walking forever.
    #[test]
    fn a_deleted_dart_tool_rearms_the_cookie_dir() {
        let _serial = serial();
        let tree = Tree::build("rearm");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        assert!(freshness.armed());
        let _ = freshness.check(&tree.roots());
        // The cookie dir itself is swept away (a `flutter clean` or
        // `git clean -fdx` shape) while the package config stays — the
        // plan does not move, so only the re-arm saves the barrier.
        fs::remove_dir_all(tree.app.join(".dart_tool/dartq")).expect("clean");
        // An OS rescan flag can race the request into a walk — allow a
        // few requests, but one of them must leave the dir recreated.
        let mut recreated = false;
        for _ in 0..20 {
            let _ = freshness.check(&tree.roots());
            if tree.app.join(".dart_tool/dartq").is_dir() {
                recreated = true;
                break;
            }
            fs::remove_dir_all(tree.app.join(".dart_tool/dartq")).ok();
        }
        assert!(recreated, "the walked root's cookie dir was recreated");
        fs::remove_dir_all(&tree.base).ok();
    }

    /// A walked root gets `.dart_tool/dartq/` created at arm time even
    /// when `.dart_tool` never existed — cookies never land in the
    /// worktree's visible root.
    #[test]
    fn a_walked_root_gets_its_cookie_dir_created() {
        let _serial = serial();
        let tree = Tree::build("cookiedir");
        // No `.dart_tool` at all — the config files stay absent so no
        // package resolution runs, the root is still walked.
        fs::remove_dir_all(tree.app.join(".dart_tool")).expect("clean");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        assert!(freshness.armed());
        assert!(
            tree.app.join(".dart_tool/dartq").is_dir(),
            "the arm created the root's cookie dir"
        );
        let cookie = tree
            .app
            .join(".dart_tool/dartq")
            .canonicalize()
            .expect("canonical cookie dir");
        assert!(
            freshness
                .shared
                .watched
                .lock()
                .iter()
                .any(|w| w.cookie_dir == cookie),
            "the armed watch points at it"
        );
        fs::remove_dir_all(&tree.base).ok();
    }

    /// A dirty set holding only ignored paths — the session heartbeat's
    /// `.dart_tool/dartq/state.json`, a `.git` write, a build json —
    /// reports no change and leaves the snapshot untouched. Under a
    /// barrier timeout the walk serves the same empty diff (ignored
    /// files never enter the snapshot), so the invariant is the empty
    /// diff and the truth — which path served it is the next test's
    /// concern.
    #[test]
    fn a_heartbeat_only_dirty_set_diffs_empty() {
        let _serial = serial();
        let tree = Tree::build("heartbeat");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        assert!(freshness.armed());
        let _ = freshness.check(&tree.roots());
        let ignored = [
            tree.app.join(".dart_tool/dartq/state.json"),
            tree.app.join(".git/index"),
            tree.app.join("build/unit_test_assets/AssetManifest.json"),
            tree.app.join("assets/i18n/en.json"),
        ];
        for path in &ignored {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).ok();
            }
            fs::write(path, "churn\n").expect("write");
        }
        freshness.inject_dirty(&ignored);
        let diff = freshness.check(&tree.roots());
        assert!(diff.is_empty(), "ignored churn diffs empty; {diff:?}");
        assert_eq!(*freshness.snapshot(), oracle(&tree));
        fs::remove_dir_all(&tree.base).ok();
    }

    /// The concurrent-mutation differential: a writer thread churns
    /// random ops on the fixture WHILE `check` runs back-to-back — the
    /// storm the journal exists for (an AI agent editing during a
    /// request). A 1 500-file tail widens the take window so a walk
    /// overlaps in-flight events; the storm stops, a forced walk runs
    /// through the event tail, and two checks later the maintained
    /// snapshot must equal the fresh walk, byte for byte. The old
    /// post-walk `dirty.clear()` erased the tail events and failed
    /// this; events drained before the take and kept after it settle
    /// clean. Config churn stays out of the op set — the plan moves
    /// it forces are covered by `root_change_rebases_and_rearms`.
    #[test]
    fn check_settles_under_concurrent_mutation() {
        let _serial = serial();
        let tree = Arc::new(Tree::build("storm"));
        // A wide tail: the take's cost is what a walk races against —
        // a few thousand files stretch the window into the event tail.
        let big = tree.app.join("lib/big");
        fs::create_dir_all(&big).expect("big");
        for index in 0..1_500 {
            fs::write(
                big.join(format!("w{index:04}.dart")),
                format!("var w{index} = {index};\n"),
            )
            .expect("wide file");
        }
        let roots = tree.roots();
        let mut freshness = Freshness::new(&roots, None, &tree.sdk);
        generous_wait(&mut freshness);
        assert!(freshness.armed());
        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let tree = Arc::clone(&tree);
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                let link = tree.app.join("lib/linked");
                let mut rng = Rng(0xDEAD_BEEF_CAFE_F00D);
                while !stop.load(AtomicOrdering::Relaxed) {
                    let op = match rng.below(7) {
                        0 => Op::Write(
                            tree.app.join(&tree.files[rng.below(tree.files.len())]),
                        ),
                        1 => Op::Remove(tree.app.join(&tree.files[rng.below(4)])),
                        2 => Op::Rename(
                            tree.app.join(format!("lib/r{}.dart", rng.below(6))),
                            tree.app.join(format!("lib/r{}.dart", rng.below(6))),
                        ),
                        3 => Op::Mkdir(
                            tree.app.join(&tree.dirs[rng.below(tree.dirs.len())]),
                        ),
                        4 => Op::Rmdir(tree.app.join(&tree.dirs[rng.below(2)])),
                        5 if !link.exists() => {
                            Op::LinkCreate(link.clone(), tree.dep.clone())
                        }
                        5 => Op::LinkRemove(link.clone()),
                        _ => Op::Write(
                            big.join(format!("w{:04}.dart", rng.below(1_500))),
                        ),
                    };
                    // Chaos ops may land on a path a previous op
                    // removed — a partial failure still produced real
                    // events (or none), which is all the storm needs.
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        op.run(&tree, &mut rng);
                    }));
                }
            })
        };
        // The storm window: checks run back-to-back while events land.
        let deadline = Instant::now() + Duration::from_secs(4);
        while Instant::now() < deadline {
            let _ = freshness.check(&roots);
        }
        stop.store(true, AtomicOrdering::Relaxed);
        writer.join().expect("writer");
        // The decisive overlap, deterministic this time: create files
        // AND inject their dirty paths WHILE a forced walk stats the
        // tree — the take may run before or after each create, but the
        // dirty entries it races must survive its own drain. A clear
        // after the take erases them under a snapshot that predates
        // the create — the file is then stale forever.
        let shared = Arc::clone(&freshness.shared);
        let mid = tree.app.join("lib/mid");
        fs::create_dir_all(&mid).expect("mid");
        let creator = thread::spawn(move || {
            for index in 0..60 {
                let file = mid.join(format!("m{index:02}.dart"));
                fs::write(&file, format!("var m{index} = {index};\n")).expect("mid write");
                shared.journal.lock().dirty_path(file);
            }
        });
        freshness.inject_walk();
        let _ = freshness.check(&roots);
        creator.join().expect("creator");
        std::thread::sleep(Duration::from_millis(500));
        let _ = freshness.check(&roots);
        assert_eq!(
            *freshness.snapshot(),
            oracle(&tree),
            "the journal lost an event under concurrent mutation"
        );
        fs::remove_dir_all(&tree.base).ok();
    }

    /// The verification only audits at a synchronized point: edits
    /// pending in the journal are the request's own merge, not a
    /// divergence — a ≥30 s-idle request answers the correct diff AND
    /// keeps the watcher. The old pre-merge comparison read every
    /// pending edit as a phantom mismatch and abandoned the watcher on
    /// a normal agent loop.
    #[test]
    fn pending_edits_are_not_a_verification_mismatch() {
        let _serial = serial();
        let tree = Tree::build("verify-pending");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        assert!(freshness.armed());
        // The agent's edits arrived since the last request; the journal
        // (or the stream's own events) holds them.
        let file = tree.app.join("lib/a.dart");
        fs::write(&file, "var edited = 1;\n").expect("edit");
        freshness.last_request = Instant::now() - VERIFY_IDLE - Duration::from_secs(1);
        freshness.last_verify = Instant::now() - VERIFY_PERIOD - Duration::from_secs(1);
        std::thread::sleep(Duration::from_millis(400));
        for _ in 0..8 {
            let _ = freshness.check(&tree.roots());
            if *freshness.snapshot() == oracle(&tree) {
                break;
            }
        }
        assert_eq!(
            *freshness.snapshot(),
            oracle(&tree),
            "the merge answered the edit"
        );
        assert_eq!(
            freshness.mismatches, 0,
            "pending edits are never a mismatch"
        );
        assert!(
            freshness.armed() || freshness.cool_off.is_some(),
            "a pending edit never abandons — only a transient strike may"
        );
        fs::remove_dir_all(&tree.base).ok();
    }

    /// A mid-verification edit makes the audit inconclusive, not a
    /// verdict: the barrier after the walk sees the journal move, the
    /// comparison is void, the watcher stays armed and the event is
    /// still served to the next request.
    #[test]
    fn a_racing_edit_keeps_the_verification_inconclusive() {
        let _serial = serial();
        let tree = Tree::build("verify-race");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        assert!(freshness.armed());
        // The arm's covering walk first — its flag alone would void the
        // audit below, whatever the injected edit does.
        let _ = freshness.check(&tree.roots());
        // The journal moves while the audit would compare: the second
        // barrier's `moved` check must void the verdict.
        let file = tree.app.join("lib/a.dart");
        fs::write(&file, "var racing = 1;\n").expect("race write");
        freshness.inject_dirty(&[file.clone()]);
        let previous = freshness.snapshot.clone();
        assert_eq!(
            freshness.verify_at_sync(&previous),
            None,
            "an event in flight voids the comparison"
        );
        assert!(freshness.armed(), "inconclusive never abandons");
        assert_eq!(freshness.mismatches, 0);
        // The pending entry still serves the next request.
        let diff = freshness.check(&tree.roots());
        assert_eq!(diff.changed, vec![file]);
        fs::remove_dir_all(&tree.base).ok();
    }

    /// The watcher is ONE long-lived stream: walks, rebases and
    /// re-arms reconcile its watch set in place — its generation
    /// changes ONLY when the reconcile recorded a real stream error
    /// (the cool-off's re-spawn), never spuriously. Deterministic: the
    /// assertion compares the generation to the error count, so a
    /// transient host watch hiccup cannot flake it.
    #[test]
    fn walks_and_rebases_never_respawn_the_watcher() {
        let _serial = serial();
        let tree = Tree::build("oneshot");
        let other = Tree::build("oneshot2");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        let generation = freshness.watcher.as_ref().map(|w| w.stream_id);
        assert!(generation.is_some());
        // Forced walks, a root-change rebase, and a dirty merge — all
        // reconcile the same stream.
        freshness.inject_walk();
        let _ = freshness.check(&tree.roots());
        freshness.inject_walk();
        let _ = freshness.check(&tree.roots());
        let _ = freshness.check(&other.roots());
        let _ = freshness.check(&other.roots());
        let (now, errors) = freshness
            .watcher
            .as_ref()
            .map(|w| (w.stream_id, w.errors))
            .unwrap_or((u64::MAX, 0));
        assert!(
            now == generation.unwrap() || errors > 0,
            "the generation changed only on a real stream error; \
             generation {generation:?} -> {now}, errors {errors}"
        );
        fs::remove_dir_all(&tree.base).ok();
        fs::remove_dir_all(&other.base).ok();
    }

    /// Dropping the freshness oracle mid-storm must return — the
    /// stream's thread reads `stop` at least every ≤50 ms slice (and is
    /// woken at once on macOS), so a continuous event stream can never
    /// starve the join.
    #[test]
    fn dropping_under_a_storm_returns_bounded() {
        let _serial = serial();
        let tree = Arc::new(Tree::build("dropstorm"));
        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let tree = Arc::clone(&tree);
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                let mut rng = Rng(0xBADC_0FFE_E0DD_F00D);
                let file = tree.app.join("lib/a.dart");
                while !stop.load(AtomicOrdering::Relaxed) {
                    fs::write(&file, format!("var x = {};\n", rng.next())).ok();
                }
            })
        };
        {
            let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
            assert!(freshness.armed());
            std::thread::sleep(Duration::from_millis(100));
            let _ = freshness.check(&tree.roots());
            let start = Instant::now();
            drop(freshness);
            assert!(
                start.elapsed() < Duration::from_secs(1),
                "the drop joined the drain in {:?}",
                start.elapsed()
            );
        }
        stop.store(true, AtomicOrdering::Relaxed);
        writer.join().expect("writer");
        fs::remove_dir_all(&tree.base).ok();
    }

    /// The idle verification: a snapshot the journal corrupted (a
    /// simulated missed event) is caught by the audit walk and the
    /// daemon stays in walk mode for its life. The journal is driven by
    /// injection — the audit's second barrier (`verify_at_sync`'s sync)
    /// must HOLD for the comparison to conclude, and a saturated
    /// `fseventsd` delivers past any barrier bound (measured at seconds
    /// per event against the 1 s clamp), which starved every retry into
    /// the inconclusive arm without ever touching the audit logic. The
    /// injection removes only the OS-latency variable: the audit's own
    /// walk, comparison, restore, mismatch count, and the drop of the
    /// watcher all run for real.
    #[test]
    fn verify_idle_catches_a_diverged_snapshot() {
        let _serial = serial();
        let tree = Tree::build("verify");
        let mut freshness = Freshness::new(&tree.roots(), None, &tree.sdk);
        generous_wait(&mut freshness);
        freshness.journal_by_injection();
        assert!(freshness.armed());
        // The arm's own `.dart_tool/dartq` create lands as a pending
        // event — drain it so the audit runs at a quiet point (an
        // in-flight event makes the audit inconclusive by design).
        std::thread::sleep(Duration::from_millis(600));
        let _ = freshness.check(&tree.roots());
        freshness.carry_dirs.clear();
        // Simulate a latent watcher bug: the maintained snapshot lost
        // an entry the walk sees.
        freshness.snapshot = Snapshot::default();
        // A late OS event landing mid-audit makes the comparison
        // inconclusive by design — the audit retries next period, so
        // retry the due check until the verdict lands.
        for _ in 0..20 {
            freshness.last_request = Instant::now() - VERIFY_IDLE - Duration::from_secs(1);
            freshness.last_verify = Instant::now() - VERIFY_PERIOD - Duration::from_secs(1);
            let _ = freshness.check(&tree.roots());
            if !freshness.armed() {
                break;
            }
            freshness.snapshot = Snapshot::default();
        }
        assert!(!freshness.armed(), "the mismatch dropped the watcher");
        assert_eq!(*freshness.snapshot(), oracle(&tree), "the audit restored truth");
        // The drop came from the verdict, or — under parallel load — a
        // transient cookie strike cooling the watcher off first; either
        // way the truth is restored and nothing stays silently armed.
        assert!(
            freshness.mismatches >= 1 || freshness.cool_off.is_some(),
            "mismatches {} cool_off {:?}",
            freshness.mismatches,
            freshness.cool_off
        );
        fs::remove_dir_all(&tree.base).ok();
    }

    /// Per-request freshness cost, walk path vs cookie path vs trusted
    /// path, median of `rounds` — run by hand against a real tree:
    /// `DQ_FRESH_BENCH=/path/to/root[,rounds] cargo test -p
    /// dartq-analyzer-host --lib bench_freshness -- --ignored
    /// --nocapture`. The walk leg forces `walk_check` per round
    /// (abandoned), the cookie leg runs the armed journal path with no
    /// changes and then with a one-file edit between rounds, and the
    /// trusted leg opens the health trust window and measures quiet
    /// turns on it.
    #[test]
    #[ignore = "manual benchmark — needs a corpus path in DQ_FRESH_BENCH"]
    fn bench_freshness() {
        let spec = std::env::var("DQ_FRESH_BENCH").expect("DQ_FRESH_BENCH=root[,rounds]");
        let (root, rounds) = spec.split_once(',').unwrap_or((&spec, "9"));
        let rounds: usize = rounds.parse().expect("rounds");
        let root = PathBuf::from(root).canonicalize().expect("canonical root");
        let roots = vec![root];
        let dart = {
            let mut files: Vec<PathBuf> = Vec::new();
            let mut pending = vec![roots[0].clone()];
            while let Some(dir) = pending.pop() {
                for entry in fs::read_dir(&dir).expect("read_dir") {
                    let path = entry.expect("entry").path();
                    if path.is_dir() {
                        pending.push(path);
                    } else if path.extension().is_some_and(|ext| ext == "dart") {
                        files.push(path);
                    }
                }
            }
            files.sort();
            files
        };
        let median = |mut times: Vec<u128>| -> u128 {
            times.sort_unstable();
            times[times.len() / 2]
        };
        // The walk leg: every check takes the full stat walk.
        let mut walk = Vec::new();
        {
            let mut freshness = Freshness::new(&roots, None, &roots[0]);
            freshness.abandoned = true;
            freshness.watcher = None;
            for _ in 0..rounds {
                let start = Instant::now();
                let _ = freshness.check(&roots);
                walk.push(start.elapsed().as_micros());
            }
        }
        // The cookie legs: armed journal, no change then a 1-file edit.
        let (mut idle, mut edit) = (Vec::new(), Vec::new());
        {
            let mut freshness = Freshness::new(&roots, None, &roots[0]);
            assert!(freshness.armed(), "the corpus root must be watchable");
            for _ in 0..rounds {
                let start = Instant::now();
                let _ = freshness.check(&roots);
                idle.push(start.elapsed().as_micros());
            }
            let file = dart.first().expect("a dart file").clone();
            let content = fs::read(&file).expect("read");
            for _ in 0..rounds {
                fs::write(&file, &content).expect("touch");
                // The barrier only orders what the watcher saw — give
                // FSEvents its latency budget off the clock.
                std::thread::sleep(Duration::from_millis(600));
                let start = Instant::now();
                let _ = freshness.check(&roots);
                edit.push(start.elapsed().as_micros());
            }
        }
        // The trusted leg: the health window open — the liveness proof
        // stamped a beat past the last doubt boundary, the straggler
        // bookkeeping of a saturated host cleared (see `open_trust`) —
        // quiet turns serve from the verdict alone.
        let mut trusted = Vec::new();
        {
            let mut freshness = Freshness::new(&roots, None, &roots[0]);
            assert!(freshness.armed(), "the corpus root must be watchable");
            let _ = freshness.check(&roots);
            open_trust(&mut freshness);
            for _ in 0..rounds {
                let start = Instant::now();
                let _ = freshness.check(&roots);
                trusted.push(start.elapsed().as_micros());
            }
            assert_eq!(
                freshness.trusted, rounds,
                "the leg measured the trusted path"
            );
        }
        eprintln!(
            "bench {} ({} dart files, {rounds} rounds, µs median):\n  \
             walk         {}\n  cookie idle  {}\n  cookie edit  {}\n  \
             trusted idle {}",
            roots[0].display(),
            dart.len(),
            median(walk),
            median(idle),
            median(edit),
            median(trusted),
        );
    }
}
