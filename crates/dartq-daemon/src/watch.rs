//! File-watch for `dartq daemon --watch` (PRD §16.7, mode 3 — the optional
//! convenience layer; explicit push and ContentId verification stay the
//! truth).
//!
//! # Shape (the runtime model's hand-wired notifier)
//!
//! No OS-event substrate exists in the dependency tree, so the watcher is a
//! **bounded-interval directory poll**, documented as such: one thread
//! re-walks the workspace every [`POLL_INTERVAL`] (reusing
//! `dartq_commands::discover` for exact walk-rule parity with the CLI's
//! discovery), fingerprints each `.dart` file by `(mtime, len)`, and diffs
//! the walk against the previous tick — the tick's walk *is* the on-disk
//! set, so the diff reports content changes, files that appeared, and files
//! that vanished (re-discovery: what the session's `queue_disk_changes`
//! turns into the same set the sweep's discovery takes, or a removal).
//! A burst coalesces through the [`Debounce`]
//! state machine until a changeless tick (the quiet period is one poll
//! interval) or the max-hold deadline, then one batch of touched paths goes
//! to the main loop over the reader channel — which enqueues the
//! didChange-equivalent overlay syncs at `Tier::Normal`. Overlay precedence
//! is decided at that intake, not here: client-opened documents keep their
//! content over the disk (see `Session::queue_disk_changes`).
//!
//! Bounded by construction: the thread's state is one fingerprint map plus
//! the pending set, both replaced/drained per tick — one walk plus one diff
//! per interval, no per-event accumulation, no timer thread, no unbounded
//! queue (the channel item is one batch). Watch events are hints, not
//! facts: a deleted file whose base copy the engine holds resurfaces on
//! overlay removal, and a failed read is logged and skipped — the next
//! analysis's content check is authoritative (§16.7).

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use rustc_hash::FxHashMap;

use dartq_commands::discover::{DiscoverError, discover};

use crate::server::Incoming;

/// The poll cadence: one workspace walk per interval. Polling is the
/// documented cost of the convenience layer (§16.7 watch is optional).
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// A change burst may hold the batch at most this long before it flushes
/// mid-burst — a writer that never quiets (a build tool, a formatter loop)
/// must not starve re-analysis (the §16.5 starvation-protection posture).
const MAX_HOLD: Duration = Duration::from_millis(2_000);

/// Where the watcher polls and how fast. `poll_interval` is injectable for
/// tests; production uses [`POLL_INTERVAL`] via [`Self::new`].
pub struct WatchOptions {
    /// The package directory to walk (the daemon's workspace root).
    pub(crate) root: PathBuf,
    /// One walk + diff per interval; also the quiet period.
    pub(crate) poll_interval: Duration,
}

impl WatchOptions {
    /// The production watch over `root` at the default poll cadence.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            poll_interval: POLL_INTERVAL,
        }
    }
}

/// The debounced batch accumulator: changes accumulate from tick to tick and
/// flush as one sorted, deduplicated batch on the first changeless tick, or
/// at the max-hold deadline inside a burst that never quiets.
#[derive(Default)]
struct Debounce {
    pending: Vec<PathBuf>,
    first_change: Option<Instant>,
}

impl Debounce {
    /// One poll tick: `touched` is what the walk diff reported. Returns the
    /// batch to enqueue, if any.
    fn tick(&mut self, touched: Vec<PathBuf>, now: Instant) -> Option<Vec<PathBuf>> {
        let quiet = touched.is_empty();
        if !quiet {
            self.pending.extend(touched);
            self.first_change.get_or_insert(now);
        }
        let held_too_long = self
            .first_change
            .is_some_and(|first| now - first >= MAX_HOLD);
        if !(quiet || held_too_long) || self.pending.is_empty() {
            return None;
        }
        let mut batch = std::mem::take(&mut self.pending);
        batch.sort_unstable();
        batch.dedup();
        self.first_change = None;
        Some(batch)
    }
}

/// A per-file poll fingerprint: cheap hints, not content identities (the
/// engine's ContentId discipline stays the truth, §16.7).
type Fingerprint = (SystemTime, u64);

/// One workspace walk: every `.dart` file by the discovery walk rules, with
/// its fingerprint. `None` when the walk itself fails — the tick is skipped
/// with the previous map kept, so a transient walk error can never masquerade
/// as a mass deletion.
fn scan(root: &std::path::Path) -> Option<FxHashMap<PathBuf, Fingerprint>> {
    let files = match discover(&[root.to_path_buf()]) {
        Ok(files) => files,
        Err(DiscoverError::Missing(path)) => {
            eprintln!("dartq-daemon: watch root vanished: {path}");
            return None;
        }
        Err(DiscoverError::Io(error)) => {
            eprintln!("dartq-daemon: watch walk failed: {error}");
            return None;
        }
    };
    Some(
        files
            .into_iter()
            .filter_map(|path| {
                let meta = fs::metadata(&path).ok()?;
                Some((path, (meta.modified().ok()?, meta.len())))
            })
            .collect(),
    )
}

/// The paths whose fingerprint changed, plus the paths that vanished.
fn diff(
    previous: &FxHashMap<PathBuf, Fingerprint>,
    current: &FxHashMap<PathBuf, Fingerprint>,
) -> Vec<PathBuf> {
    current
        .iter()
        .filter(|(path, fingerprint)| previous.get(*path) != Some(*fingerprint))
        .map(|(path, _)| path.clone())
        .chain(
            previous
                .keys()
                .filter(|path| !current.contains_key(*path))
                .cloned(),
        )
        .collect()
}

/// A handle to the one watch thread: dropping it stops the poll and joins
/// the thread, so session end shuts the watcher down cleanly (and a send
/// failure exits the thread on its own once the main loop is gone).
pub(crate) struct Watcher {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Watcher {
    /// Spawns the poll thread (one thread per watcher, by construction).
    /// A failed spawn only loses the convenience layer — the daemon logs and
    /// serves without watch.
    pub(crate) fn spawn(options: WatchOptions, sender: Sender<Vec<Incoming>>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let join = thread::Builder::new()
            .name("dartq-daemon-watch".into())
            .spawn({
                let stop = Arc::clone(&stop);
                move || watch_loop(&options, sender, &stop)
            })
            .inspect_err(|error| eprintln!("dartq-daemon: watch thread unavailable: {error}"))
            .ok();
        Self { stop, join }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// The poll loop: baseline scan, then one sleep-walk-diff-debounce cycle per
/// interval. Batches ride the reader channel as main-loop intake; a dead
/// receiver ends the thread.
fn watch_loop(options: &WatchOptions, sender: Sender<Vec<Incoming>>, stop: &AtomicBool) {
    let Some(mut previous) = scan(&options.root) else {
        return;
    };
    let mut debounce = Debounce::default();
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(options.poll_interval);
        let Some(current) = scan(&options.root) else {
            continue;
        };
        let touched = diff(&previous, &current);
        previous = current;
        if let Some(batch) = debounce.tick(touched, Instant::now()) {
            let item = vec![Incoming::Watch { touched: batch }];
            if sender.send(item).is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::sync::mpsc;
    use std::time::Duration;

    /// A workspace with one tracked file and nothing else in it. The
    /// sequence suffix makes every call a unique tree: two callers passing
    /// the same `tag`, or a crashed prior run whose pid was recycled, can
    /// never share state — the `tag` stays in the path only for
    /// diagnosability of leftovers (mirroring `store_root`).
    fn workspace(tag: &str) -> (PathBuf, PathBuf) {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "dq_watch_{tag}_{}_{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        let _ = fs::remove_dir_all(&root);
        let lib = root.join("lib");
        fs::create_dir_all(&lib).expect("mkdir");
        let main = lib.join("main.dart");
        fs::write(&main, "var a = 1;\n").expect("seed");
        (root, main)
    }

    /// A burst across several ticks coalesces into one batch on the first
    /// quiet tick; the batch is sorted and deduplicated.
    #[test]
    fn tick_coalesces_bursts_until_a_quiet_tick() {
        let t0 = Instant::now();
        let mut debounce = Debounce::default();
        let touch = |name: &str| vec![PathBuf::from(name)];

        assert!(
            debounce.tick(touch("b.dart"), t0).is_none(),
            "no flush mid-burst"
        );
        assert!(debounce.tick(touch("a.dart"), t0 + POLL_INTERVAL).is_none());
        assert!(
            debounce
                .tick(touch("b.dart"), t0 + 2 * POLL_INTERVAL)
                .is_none()
        );

        let batch = debounce
            .tick(Vec::new(), t0 + 3 * POLL_INTERVAL)
            .expect("the quiet tick flushes");
        assert_eq!(
            batch,
            vec![PathBuf::from("a.dart"), PathBuf::from("b.dart")],
            "one sorted, deduplicated batch"
        );
        assert!(
            debounce.tick(Vec::new(), t0 + 4 * POLL_INTERVAL).is_none(),
            "a clean tick stays silent"
        );
    }

    /// A burst that never quiets flushes at the max-hold deadline instead of
    /// starving re-analysis.
    #[test]
    fn tick_flushes_at_max_hold_inside_a_never_quiet_burst() {
        let t0 = Instant::now();
        let mut debounce = Debounce::default();
        let touch = |n: usize| vec![PathBuf::from(format!("f{n}.dart"))];

        // Ticks 0..3 keep accumulating: under the hold deadline, change
        // ticks never flush.
        for n in 0_usize..4 {
            let at = t0 + POLL_INTERVAL * n as u32;
            assert!(debounce.tick(touch(n), at).is_none(), "tick {n}");
        }
        // The change tick at the hold deadline flushes mid-burst.
        let batch = debounce
            .tick(touch(4), t0 + MAX_HOLD)
            .expect("max hold forces the flush");
        assert_eq!(batch.len(), 5, "everything accumulated so far: {batch:?}");
        assert!(
            debounce
                .tick(touch(5), t0 + MAX_HOLD + POLL_INTERVAL)
                .is_none(),
            "the accumulator restarted clean"
        );
    }

    /// The walk diff reports content changes, additions, and removals; a
    /// failed walk (vanished root) yields `None` so no phantom mass deletion
    /// can be derived from it.
    #[test]
    fn scan_and_diff_track_change_addition_removal() {
        let (root, main) = workspace("scan-diff");
        let first = scan(&root).expect("baseline");
        assert_eq!(first.len(), 1, "the seeded file");

        // Rewrite with a different length: even if mtime granularity ties,
        // the len half of the fingerprint moves.
        fs::write(&main, "var renamed = 12;\n").expect("rewrite");
        let second = scan(&root).expect("after rewrite");
        let touched = diff(&first, &second);
        assert_eq!(touched, vec![main.clone()], "the rewrite is the only touch");

        // An addition and a removal in one tick.
        let extra = root.join("lib/extra.dart");
        fs::write(&extra, "var b = 2;\n").expect("add");
        fs::remove_file(&main).expect("remove");
        let third = scan(&root).expect("after add+remove");
        let mut touched = diff(&second, &third);
        touched.sort_unstable();
        assert_eq!(
            touched,
            vec![extra.clone(), main.clone()],
            "addition and removal both touch"
        );

        // A vanished root: the walk fails, the tick is skipped.
        fs::remove_dir_all(&root).expect("rm -rf");
        assert!(scan(&root).is_none(), "no map, no phantom deletion");
    }

    /// The spawned thread delivers one debounced batch for a real write and
    /// shuts down cleanly on drop (stop + join; the channel then reports
    /// disconnect). One watcher, one thread — the lifecycle never leaks.
    #[test]
    fn watch_thread_delivers_batch_and_stops_on_drop() {
        let (root, main) = workspace("thread");
        let (sender, receiver) = mpsc::channel::<Vec<Incoming>>();
        let watcher = Watcher::spawn(
            WatchOptions {
                root: root.clone(),
                poll_interval: Duration::from_millis(25),
            },
            sender,
        );

        // Let the baseline scan settle, then make exactly one change.
        thread::sleep(Duration::from_millis(120));
        fs::write(&main, "var edited = 42;\n").expect("write");

        let batch = receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("the write surfaced within the window");
        let item = batch.into_iter().next().expect("one intake item");
        let Incoming::Watch { touched } = item else {
            panic!("watch intake is a Watch item");
        };
        assert_eq!(touched, vec![main], "the touched file, debounced");

        // Clean shutdown: drop joins the thread; the disconnected channel
        // proves it exited.
        drop(watcher);
        assert!(
            receiver.recv_timeout(Duration::from_secs(2)).is_err(),
            "the watch thread exited on stop"
        );
        fs::remove_dir_all(&root).ok();
    }

    /// Re-discovery rides the real thread: a file created after the
    /// baseline scan surfaces as a touched path (the batch that makes the
    /// session stage the set discovery takes), and its later deletion
    /// surfaces too (the removal batch). Together with the session-level
    /// sync tests this closes the chain "new files enter analysis, removals
    /// drop".
    #[test]
    fn watch_thread_reports_creations_and_deletions() {
        let (root, _main) = workspace("thread-rediscover");
        let (sender, receiver) = mpsc::channel::<Vec<Incoming>>();
        let watcher = Watcher::spawn(
            WatchOptions {
                root: root.clone(),
                poll_interval: Duration::from_millis(25),
            },
            sender,
        );

        // Let the baseline scan settle, then create and delete a file —
        // each event is one debounced Watch batch.
        thread::sleep(Duration::from_millis(120));
        let extra = root.join("lib/extra.dart");
        fs::write(&extra, "var b = 2;\n").expect("create");
        let created = receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("the creation surfaced within the window");
        assert_eq!(
            watch_paths(created),
            vec![extra.clone()],
            "the new file is the batch"
        );

        fs::remove_file(&extra).expect("delete");
        let deleted = receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("the deletion surfaced within the window");
        assert_eq!(
            watch_paths(deleted),
            vec![extra],
            "the removal is the batch"
        );

        drop(watcher);
        fs::remove_dir_all(&root).ok();
    }

    /// Unwraps a channel item into its touched-path batch.
    fn watch_paths(item: Vec<Incoming>) -> Vec<PathBuf> {
        match item.as_slice() {
            [Incoming::Watch { touched }] => touched.clone(),
            _ => panic!("watch intake is one Watch item"),
        }
    }
}
