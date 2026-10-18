//! File-watch for `dartq daemon --watch` (mode 3 — the optional
//! convenience layer; explicit push and `ContentId` verification stay the
//! truth).
//!
//! # Shape (a substrate seam + the hand-wired debouncer)
//!
//! The watch face is a [`WatchSource`] seam carrying two substrates into
//! the same drain-debounce-hand-off cycle: the **OS-event substrate**
//! (the default — `notify`'s recommended watcher, FSEvents on macOS and
//! inotify on Linux, pinned end to end by the spawned-thread test) and the
//! **bounded-interval directory poll** — the watch face's proven shape,
//! kept as the `DARTQ_WATCH_SUBSTRATE=poll` escape hatch and the
//! init-failure fallback. The poll source
//! re-walks the workspace per drain (reusing
//! `dartq_commands::discover` for exact walk-rule parity with the CLI's
//! discovery), fingerprints each `.dart` file by `(mtime, len)` — plus the
//! three non-`.dart` context inputs ([`CONFIG_INPUTS`], the files a context
//! assembly consumes) — and diffs
//! the walk against the previous tick — the tick's walk *is* the on-disk
//! set, so the diff reports content changes, files that appeared, and files
//! that vanished (re-discovery: what the session's `queue_disk_changes`
//! turns into the same set the sweep's discovery takes, or a removal; a
//! context input instead refreshes the contexts at intake — their bytes
//! never stage as analysis content, and the batch re-evaluates the
//! session's workspace advisory).
//!
//! A burst coalesces through the [`Debounce`]
//! state machine until a changeless tick (the quiet period is one poll
//! interval) or the max-hold deadline, then one batch of touched paths goes
//! to the main loop over the reader channel — which enqueues the
//! `dartq/sync`-equivalent overlay syncs at `Tier::Normal` (see
//! `Session::queue_disk_changes`). Per drain with changes, the release
//! watch-latency probe ([`WatchMetrics`]) records the stalest event's wait
//! — change time to the drain that observed it (the poll source stamps the
//! file's mtime; the OS substrate stamps delivery, the best known time it
//! has) — and logs it the way the daemon logs everything operational.
//!
//! Bounded by construction: the thread's state is the substrate (the poll
//! source's one fingerprint map, replaced per tick; the OS substrate's
//! event queue, drained non-blocking every tick) plus the pending set —
//! no per-event accumulation in our own state, no timer thread, and a
//! bounded intake queue (the channel item is one batch; a full queue hands
//! the batch back to the accumulator instead of stalling the drain). Watch
//! events are hints, not facts: a deleted file whose base copy the engine
//! holds resurfaces on overlay removal, and a failed read is logged and
//! skipped — the next analysis's content check is authoritative — and a
//! substrate rescan notice (lost or coalesced history) says so and lets
//! the next batch's re-discovery self-heal.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use crossbeam_channel::{Sender, TrySendError};
use rustc_hash::FxHashMap;

use dartq_commands::discover::{DiscoverError, discover};

use crate::server::Incoming;

/// The watch cadence: one substrate drain + debounce cycle per interval —
/// and the poll substrate's walk cadence, polling being that substrate's
/// documented cost.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// The non-`.dart` context inputs tracked beside the `.dart` walk: the
/// files a context assembly consumes (the package manifest, the resolved
/// package graph, the analysis options — the reload surface). They
/// are fingerprinted like `.dart` files so a change (or appearance, or
/// deletion) surfaces in the tick diff, but they never stage as analysis
/// content — [`is_config_input`] routes them to the context refresh at
/// intake.
const CONFIG_INPUTS: [&str; 3] = [
    "pubspec.yaml",
    ".dart_tool/package_config.json",
    "analysis_options.yaml",
];

/// Whether `path` is one of the watched non-`.dart` context inputs under
/// `root`: its change routes to the context refresh (re-discovery +
/// re-analysis), never to the content sync — a pubspec's bytes are not Dart
/// source, and staging them would intern a phantom analysis file.
pub(crate) fn is_config_input(path: &Path, root: &Path) -> bool {
    CONFIG_INPUTS
        .iter()
        .any(|relative| path == root.join(relative))
}

/// A change burst may hold the batch at most this long before it flushes
/// mid-burst — a writer that never quiets (a build tool, a formatter loop)
/// must not starve re-analysis.
const MAX_HOLD: Duration = Duration::from_secs(2);

/// How an observed change surfaced. The poll substrate derives it from the
/// fingerprint diff; the OS substrate reports it directly. Metadata for the
/// latency probe and the tests — the intake batch stays a path batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EventKind {
    Create,
    Modify,
    Delete,
    Rename,
}

/// One observed change. `at` is the best known time the change happened —
/// the file's mtime for the poll substrate, the delivery time for the OS
/// substrate (FSEvents/inotify expose no per-event timestamp) — the
/// denominator of the release watch-latency probe.
#[derive(Debug)]
struct SourceEvent {
    path: PathBuf,
    /// The change's shape: today's intake batch is path-only (the session
    /// re-discovers per batch), so the tests are the field's reader until a
    /// substrate-aware intake consumes it.
    #[cfg_attr(not(test), allow(dead_code))]
    kind: EventKind,
    at: SystemTime,
}

/// The release watch-latency probe: per drain with changes, the stalest
/// event's wait — change time to the drain that observed it — and the count
/// of such drains. Surfaced the way the daemon surfaces everything
/// operational, one log line per observed drain; the atomics are the
/// recorded values the tests read.
#[derive(Default)]
#[cfg_attr(not(test), allow(dead_code))] // the atomics are the tests' read face
struct WatchMetrics {
    observed_drains: AtomicU64,
    last_max_latency_micros: AtomicU64,
}

impl WatchMetrics {
    /// Records one drain. The stalest event's wait is the drain's latency
    /// — the freshest event would under-report a burst's true delay — and
    /// a future-stamped event (a backwards clock) contributes no wait
    /// rather than a negative one.
    fn observe(&self, events: &[SourceEvent], now: SystemTime) {
        if events.is_empty() {
            return;
        }
        let stalest = events
            .iter()
            .filter_map(|event| now.duration_since(event.at).ok())
            .max()
            .unwrap_or(Duration::ZERO);
        self.observed_drains.fetch_add(1, Ordering::Relaxed);
        self.last_max_latency_micros.store(
            u64::try_from(stalest.as_micros()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        eprintln!(
            "dartq-daemon: watch: {} changed paths observed, max latency {}ms",
            events.len(),
            stalest.as_millis(),
        );
    }
}

/// The watch face's substrate: whatever turns on-disk changes into
/// [`SourceEvent`]s. Two implementations — the OS-event substrate and the
/// bounded-interval poll — behind one seam: the loop and the debounce are
/// substrate-agnostic, and a substrate swap never touches a call site.
trait WatchSource: Send {
    /// Drains the changes observed since the last call. Non-blocking: the
    /// loop's cadence stays the poll interval on every substrate.
    fn drain(&mut self) -> Vec<SourceEvent>;
}

/// The poll substrate: the watch face's original behavior verbatim, one
/// walk plus one fingerprint diff per drain.
struct PollSource {
    root: PathBuf,
    previous: FxHashMap<PathBuf, Fingerprint>,
}

impl PollSource {
    /// The baseline walk. `None` when it fails — the walk already logged
    /// the reason, and the watch thread exits as it always has.
    fn new(root: &Path) -> Option<Self> {
        let previous = scan(root)?;
        Some(Self {
            root: root.to_path_buf(),
            previous,
        })
    }
}

impl WatchSource for PollSource {
    fn drain(&mut self) -> Vec<SourceEvent> {
        let Some(current) = scan(&self.root) else {
            return Vec::new();
        };
        let events = diff(&self.previous, &current)
            .into_iter()
            .map(|path| match current.get(&path) {
                // The mtime is the write time the latency probe measures
                // from; a vanished file leaves no mtime, so its stamp is
                // the drain itself.
                Some(&(modified, _)) => SourceEvent {
                    kind: if self.previous.contains_key(&path) {
                        EventKind::Modify
                    } else {
                        EventKind::Create
                    },
                    path,
                    at: modified,
                },
                None => SourceEvent {
                    kind: EventKind::Delete,
                    path,
                    at: SystemTime::now(),
                },
            })
            .collect();
        self.previous = current;
        events
    }
}

/// The OS-event substrate: `notify`'s recommended watcher — FSEvents on
/// macOS, inotify on Linux. Gated to the two platforms the daemon ships
/// for; the Linux face is the same platform-agnostic wrapper (notify
/// selects the backend) and is compile-checked there, not on this host.
#[cfg(any(target_os = "macos", target_os = "linux"))]
struct NotifySource {
    /// The workspace root as given: event paths arrive under the
    /// canonicalized watch root (FSEvents reports resolved paths), and the
    /// intake batch must carry the walk's path forms.
    root: PathBuf,
    /// The canonical watch root, stripped off incoming event paths.
    canonical: PathBuf,
    /// Keep-alive: dropping the watcher stops the OS stream and its
    /// delivery thread.
    _watcher: notify::RecommendedWatcher,
    receiver: crossbeam_channel::Receiver<Result<notify::Event, notify::Error>>,
    /// Set once the substrate's sender is gone: the drain goes silent
    /// instead of reporting the disconnect every tick.
    dead: bool,
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl NotifySource {
    /// Arms the OS stream over `root`, recursively. `Err` carries the
    /// substrate's own reason — the caller logs it and falls back to poll.
    fn new(root: &Path) -> Result<Self, String> {
        let canonical = root.canonicalize().map_err(|error| error.to_string())?;
        let (sender, receiver) = crossbeam_channel::unbounded();
        // The trait method's namesake trait, imported anonymously: our own
        // `Watcher` handle owns the module's name.
        use notify::Watcher as _;
        let mut watcher = notify::recommended_watcher(sender).map_err(|error| error.to_string())?;
        watcher
            .watch(&canonical, notify::RecursiveMode::Recursive)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            root: root.to_path_buf(),
            canonical,
            _watcher: watcher,
            receiver,
            dead: false,
        })
    }

    /// Event paths arrive under the canonical root; the batch must carry
    /// the walk's forms (`/var/...`, not `/private/var/...`). Paths outside
    /// the root (mount noise) and the root itself (no specific file — a
    /// root path in a batch would sync the directory as a file) drop.
    fn normalize(&self, path: PathBuf) -> Option<PathBuf> {
        let normalized = match path.strip_prefix(&self.canonical) {
            Ok(relative) => self.root.join(relative),
            Err(_) if path.starts_with(&self.root) => path,
            Err(_) => return None,
        };
        (normalized != self.root).then_some(normalized)
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl WatchSource for NotifySource {
    fn drain(&mut self) -> Vec<SourceEvent> {
        if self.dead {
            return Vec::new();
        }
        // The substrate stamps no per-event time: delivery (this drain) is
        // the best known time, sampled once before the queue drains so a
        // queued event never stamps after the drain that observed it.
        let at = SystemTime::now();
        let mut events = Vec::new();
        loop {
            match self.receiver.try_recv() {
                Ok(Ok(event)) => {
                    // The substrate lost track (dropped or coalesced
                    // history): say so — the next batch's re-discovery
                    // self-heals — and carry no phantom paths.
                    if event.need_rescan() {
                        eprintln!(
                            "dartq-daemon: watch substrate lost events; \
                             changes re-discover on the next batch"
                        );
                        continue;
                    }
                    let Some(kind) = classify(&event.kind) else {
                        continue;
                    };
                    events.extend(event.paths.into_iter().filter_map(|path| {
                        let path = self.normalize(path)?;
                        // The intake contract is the poll substrate's walk
                        // surface: `.dart` files under non-hidden names
                        // (the walk's dot-prefix rule) plus the watched
                        // context inputs. Directory events and the rest of
                        // the tree (`.dart_tool` churn, editor scratch) are
                        // not change hints — staging them would intern dead
                        // paths against the session's live-path ceiling.
                        let watched_dart = path.extension().is_some_and(|ext| ext == "dart")
                            && path.strip_prefix(&self.root).is_ok_and(|relative| {
                                !relative.components().any(|component| {
                                    component.as_os_str().as_encoded_bytes().starts_with(b".")
                                })
                            });
                        if watched_dart || is_config_input(&path, &self.root) {
                            Some(SourceEvent { path, kind, at })
                        } else {
                            None
                        }
                    }));
                }
                Ok(Err(error)) => eprintln!("dartq-daemon: watch substrate error: {error}"),
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.dead = true;
                    eprintln!("dartq-daemon: watch substrate ended; no further events");
                    break;
                }
            }
        }
        events
    }
}

/// The substrate's event kinds to the watch face's. Access events are
/// noise (inotify reports them; nothing re-analyzes on a read), the
/// substrate's `Other` carries no actionable change, and an `Any` — the
/// non-precise backends' catch-all — is a change: `Modify`.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn classify(kind: &notify::EventKind) -> Option<EventKind> {
    use notify::EventKind as Backend;
    match kind {
        Backend::Create(_) => Some(EventKind::Create),
        Backend::Modify(notify::event::ModifyKind::Name(_)) => Some(EventKind::Rename),
        Backend::Modify(_) => Some(EventKind::Modify),
        Backend::Remove(_) => Some(EventKind::Delete),
        Backend::Any => Some(EventKind::Modify),
        Backend::Access(_) | Backend::Other => None,
    }
}

/// Which substrate feeds the watch loop. `Os` is the shipped default —
/// event-driven, no per-tick walk; `Poll` is the proven fallback kept as
/// the escape hatch (`DARTQ_WATCH_SUBSTRATE=poll`). The env read happens
/// once at spawn; tests pin the substrate through the value, not the
/// process-wide variable.
#[derive(Clone, Copy)]
enum Substrate {
    /// The OS-event backend — constructed only where `notify` ships one.
    #[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
    Os,
    Poll,
}

impl Substrate {
    /// The resolution table, env-free so the tests pin it without racing
    /// libtest's shared process environment: `DARTQ_WATCH_SUBSTRATE=poll`
    /// selects the poll substrate; anything else — unset, `os`, or noise —
    /// takes the OS substrate, with the poll fallback inside
    /// [`init_source`] when the OS stream cannot arm.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn parse(value: Option<&std::ffi::OsStr>) -> Self {
        if value.is_some_and(|v| v == "poll") {
            Self::Poll
        } else {
            Self::Os
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn from_env() -> Self {
        Self::parse(std::env::var_os("DARTQ_WATCH_SUBSTRATE").as_deref())
    }

    /// Off the shipped platforms there is no OS-event backend: the env
    /// knob is inert and poll is the only substrate.
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    fn from_env() -> Self {
        Self::Poll
    }
}

/// The substrate for this build. The OS-event substrate is the default —
/// delivery, kinds, and path forms are pinned by the spawned-thread test —
/// and the poll substrate remains as the `DARTQ_WATCH_SUBSTRATE=poll`
/// escape hatch plus the init-failure fallback. `None` means the watch
/// face cannot come up at all — the baseline walk failed, already logged
/// — and the watch thread exits as it always has.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn init_source(root: &Path, substrate: Substrate) -> Option<Box<dyn WatchSource>> {
    if let Substrate::Poll = substrate {
        return PollSource::new(root).map(|source| Box::new(source) as Box<dyn WatchSource>);
    }
    match NotifySource::new(root) {
        Ok(source) => {
            eprintln!("dartq-daemon: watch: OS-event substrate active");
            Some(Box::new(source))
        }
        Err(error) => {
            eprintln!("dartq-daemon: watch: OS-event substrate unavailable ({error}); polling");
            PollSource::new(root).map(|source| Box::new(source) as Box<dyn WatchSource>)
        }
    }
}

/// Off the shipped platforms there is no OS-event backend: poll is the
/// substrate.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn init_source(root: &Path, _substrate: Substrate) -> Option<Box<dyn WatchSource>> {
    PollSource::new(root).map(|source| Box::new(source) as Box<dyn WatchSource>)
}

/// Where the watcher polls and how fast. `poll_interval` is injectable for
/// tests; production uses `POLL_INTERVAL` via [`Self::new`].
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

    /// Returns an unsent batch to the accumulator: a full intake queue is
    /// backpressure, not a loss. The paths merge with any later touches
    /// (the sort+dedup flush coalesces same-file repeats into one entry)
    /// and retry on the next quiet tick or max-hold deadline — one send
    /// attempt per poll interval, never a spin and never a blocked walk.
    fn requeue(&mut self, batch: Vec<PathBuf>, now: Instant) {
        self.pending.extend(batch);
        self.first_change.get_or_insert(now);
    }
}

/// A per-file poll fingerprint: cheap hints, not content identities (the
/// engine's `ContentId` discipline stays the truth).
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
            // The context inputs ride the same fingerprint map: present
            // files are tracked, absent ones are not, so a later creation
            // or deletion diffs as a touch like any file event.
            .chain(CONFIG_INPUTS.iter().filter_map(|relative| {
                let path = root.join(relative);
                let meta = fs::metadata(&path).ok()?;
                Some((path, (meta.modified().ok()?, meta.len())))
            }))
            .collect(),
    )
}

/// The paths whose fingerprint changed, plus the paths that vanished —
/// sorted, so the `Incoming` batch order is deterministic at the source
/// instead of inheriting the fingerprint map's hash-iteration order (the
/// engine's intake re-sort stays the truth; this makes the watch face
/// order-stable on its own).
fn diff(
    previous: &FxHashMap<PathBuf, Fingerprint>,
    current: &FxHashMap<PathBuf, Fingerprint>,
) -> Vec<PathBuf> {
    let mut touched: Vec<PathBuf> = current
        .iter()
        .filter(|(path, fingerprint)| previous.get(*path) != Some(*fingerprint))
        .map(|(path, _)| path.clone())
        .chain(
            previous
                .keys()
                .filter(|path| !current.contains_key(*path))
                .cloned(),
        )
        .collect();
    touched.sort_unstable();
    touched
}

/// A handle to the one watch thread: dropping it stops the poll and joins
/// the thread, so session end shuts the watcher down cleanly (and a send
/// failure exits the thread on its own once the main loop is gone).
pub(crate) struct Watcher {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Watcher {
    /// Spawns the watch thread (one thread per watcher, by construction;
    /// the substrate is inited on the thread, so a slow OS stream arm never
    /// blocks the caller). A failed spawn only loses the convenience layer —
    /// the daemon logs and serves without watch.
    pub(crate) fn spawn(options: WatchOptions, sender: Sender<Vec<Incoming>>) -> Self {
        Self::spawn_with(options, sender, Substrate::from_env())
    }

    /// `spawn` with the substrate chosen by the caller — the tests pin the
    /// OS substrate this way instead of mutating the process-wide env var
    /// under libtest's parallel worker threads.
    fn spawn_with(
        options: WatchOptions,
        sender: Sender<Vec<Incoming>>,
        substrate: Substrate,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let join = thread::Builder::new()
            .name("dartq-daemon-watch".into())
            .spawn({
                let stop = Arc::clone(&stop);
                let metrics = Arc::new(WatchMetrics::default());
                move || watch_loop(&options, &sender, &stop, &metrics, substrate)
            })
            .inspect_err(|error| eprintln!("dartq-daemon: watch thread unavailable: {error}"))
            .ok();
        Self { stop, join }
    }
}

/// The shutdown site: set the stop flag, then join. The join's latency is
/// bounded by construction — the flag is checked at the top of every poll
/// iteration, the tick's only channel operation is a non-blocking
/// [`Sender::try_send`], and every path out of a tick (a failed walk
/// `continue`s, a disconnected receiver `return`s) leads back to that
/// check — so the join waits at most one in-flight poll interval (the
/// sleep is not interruptible) plus one tick's work (one walk, one diff,
/// one batch hand-off), or the pre-loop baseline walk in the spawn race.
/// Session end is never held unbounded by the watcher's shutdown.
impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// The watch loop: substrate init, then one drain-debounce-hand-off cycle
/// per interval. Batches ride the bounded reader channel as main-loop
/// intake; a dead receiver ends the thread, and a full queue is
/// backpressure without a blocked drain — the batch returns to the
/// accumulator and retries on a later tick. A blocking send would stall
/// the poll on a slow drain and could hold [`Watcher::drop`]'s join at
/// shutdown past the receiver's own drop, so the send never waits.
fn watch_loop(
    options: &WatchOptions,
    sender: &Sender<Vec<Incoming>>,
    stop: &AtomicBool,
    metrics: &WatchMetrics,
    substrate: Substrate,
) {
    let Some(mut source) = init_source(&options.root, substrate) else {
        return;
    };
    let mut debounce = Debounce::default();
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(options.poll_interval);
        let events = source.drain();
        metrics.observe(&events, SystemTime::now());
        let touched: Vec<PathBuf> = events.into_iter().map(|event| event.path).collect();
        if let Some(batch) = debounce.tick(touched, Instant::now()) {
            match sender.try_send(vec![Incoming::Watch { touched: batch }]) {
                Ok(()) => {}
                Err(TrySendError::Disconnected(_)) => return,
                Err(TrySendError::Full(item)) => {
                    // The recovered intake is the one built above; the
                    // impossible non-Watch shape falls out as a dropped
                    // stale hint, which is a sanctioned watch semantic.
                    if let Some(Incoming::Watch { touched }) = item.into_iter().next() {
                        debounce.requeue(touched, Instant::now());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::time::Duration;

    use crossbeam_channel::{Receiver, RecvTimeoutError};

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
    #[expect(clippy::cast_possible_truncation)] // the tick indices are single-digit constants feeding Duration math
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

    /// The diff's touched batch is sorted at the source (determinism audit
    /// R1): paths touched in one tick surface in byte order, never in the
    /// fingerprint map's hash-iteration order — the batch order is a
    /// contract, not an accident the engine's intake re-sort papers over.
    #[test]
    fn diff_reports_touched_paths_sorted() {
        let untouched = (SystemTime::UNIX_EPOCH, 0);
        let touched_now = (SystemTime::UNIX_EPOCH, 1);
        let names = [
            "a.dart", "b.dart", "c.dart", "d.dart", "e.dart", "f.dart", "g.dart", "h.dart",
        ];
        let previous: FxHashMap<PathBuf, Fingerprint> = names
            .into_iter()
            .map(|name| (PathBuf::from(name), untouched))
            .collect();
        let current: FxHashMap<PathBuf, Fingerprint> = names
            .into_iter()
            .map(|name| (PathBuf::from(name), touched_now))
            .collect();
        let expected: Vec<PathBuf> = names.iter().map(PathBuf::from).collect();
        assert_eq!(
            diff(&previous, &current),
            expected,
            "eight simultaneous touches: byte-sorted batch"
        );
    }

    /// The spawned thread delivers one debounced batch for a real write and
    /// shuts down cleanly on drop (stop + join; the channel then reports
    /// disconnect). One watcher, one thread — the lifecycle never leaks.
    #[test]
    fn watch_thread_delivers_batch_and_stops_on_drop() {
        let (root, main) = workspace("thread");
        // The bounded intake shape the server's main loop takes.
        let (sender, receiver) = crossbeam_channel::bounded::<Vec<Incoming>>(2);
        let watcher = Watcher::spawn(
            WatchOptions {
                root: root.clone(),
                poll_interval: Duration::from_millis(25),
            },
            sender,
        );

        // The first write retriggers until the thread answers (the substrate
        // arms asynchronously; a pre-arm change is absorbed), then the
        // batch is the touched file, debounced.
        let (touched, _) = settle_rewrite(&main, "var edited = 42;\n", &receiver);
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

    /// A full intake queue is backpressure, never a stall or a loss: the
    /// batch merges back into the accumulator and retries tick by tick, so
    /// the change surfaces once the queue drains — and the poll loop never
    /// blocks on the send.
    #[test]
    fn watch_thread_full_channel_requeues_and_retries() {
        let (root, main) = workspace("full-channel");
        let (sender, receiver) = crossbeam_channel::bounded::<Vec<Incoming>>(1);
        // Occupy the one slot before the thread exists: the debounced
        // batch below must meet a full queue.
        sender.send(Vec::new()).expect("occupy the slot");
        let watcher = Watcher::spawn(
            WatchOptions {
                root: root.clone(),
                poll_interval: Duration::from_millis(25),
            },
            sender,
        );

        // The occupied slot drains first (the settle skips and counts the
        // filler); the requeued batch retries tick by tick and surfaces once
        // the slot is free — the thread kept walking the whole time (no
        // blocked send, no hang). The first write retriggers until the
        // thread answers (the substrate arms asynchronously).
        let (touched, fillers) = settle_rewrite(&main, "var edited = 9;\n", &receiver);
        assert_eq!(fillers, 1, "the slot's filler, not a watch batch");
        assert_eq!(touched, vec![main], "the requeued batch retried");

        drop(watcher);
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
        // The bounded intake shape the server's main loop takes.
        let (sender, receiver) = crossbeam_channel::bounded::<Vec<Incoming>>(2);
        let watcher = Watcher::spawn(
            WatchOptions {
                root: root.clone(),
                poll_interval: Duration::from_millis(25),
            },
            sender,
        );

        // The creation retriggers until the thread answers (the substrate
        // arms asynchronously; a pre-arm change is absorbed) — the batch is
        // the new file — and the later deletion is provably post-arm.
        let extra = root.join("lib/extra.dart");
        let (created, _) = settle_rewrite(&extra, "var b = 2;\n", &receiver);
        assert_eq!(
            created,
            vec![extra.clone()],
            "the new file is the batch"
        );

        fs::remove_file(&extra).expect("delete");
        // The same bounded window as `settle_rewrite`, minus the retry (a
        // deletion cannot be re-triggered): sliced receives until the
        // removal batch surfaces. One fixed 5-second `recv_timeout`
        // mistook coverage instrumentation's slowdown for a dropped
        // removal; the window is an upper bound, the slices keep the wait
        // interruptible.
        let deadline = Instant::now() + SETTLE_WINDOW;
        let deleted = loop {
            assert!(
                Instant::now() < deadline,
                "the deletion surfaced within the window"
            );
            match receiver.recv_timeout(Duration::from_secs(1)) {
                Ok(item) => break item,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("the watch thread exited before answering")
                }
            }
        };
        assert_eq!(
            watch_paths(&deleted),
            vec![extra],
            "the removal is the batch"
        );

        drop(watcher);
        fs::remove_dir_all(&root).ok();
    }

    /// The non-`.dart` context inputs ride the same fingerprint map as the
    /// `.dart` walk: each of the three triggers the diff on creation,
    /// rewrite, and deletion — the watch trigger set the reload
    /// surface needs.
    #[test]
    fn scan_and_diff_track_each_config_input() {
        let (root, _main) = workspace("config-inputs");
        // The fixture seeds none of the three: the baseline map is the
        // `.dart` set alone.
        let baseline = scan(&root).expect("baseline");
        assert_eq!(baseline.len(), 1, "the seeded file only");
        assert!(
            !baseline.keys().any(|path| is_config_input(path, &root)),
            "no config input is tracked before it exists"
        );

        let pubspec = root.join("pubspec.yaml");
        let options = root.join("analysis_options.yaml");
        let config = root.join(".dart_tool/package_config.json");
        fs::create_dir_all(config.parent().expect("parent")).expect("dir");

        // Creation of the package manifest: one touch.
        fs::write(&pubspec, "name: w\n").expect("write pubspec");
        let after_pubspec = scan(&root).expect("scan");
        assert_eq!(
            diff(&baseline, &after_pubspec),
            vec![pubspec.clone()],
            "the manifest's creation is the batch"
        );

        // Creation of the analysis options: one touch (the manifest's
        // unchanged fingerprint stays out of the diff).
        fs::write(
            &options,
            "analyzer:\n  errors:\n    unused_import: ignore\n",
        )
        .expect("write options");
        let after_options = scan(&root).expect("scan");
        assert_eq!(
            diff(&after_pubspec, &after_options),
            vec![options.clone()],
            "the options file's creation is the batch"
        );

        // Rewrite of the package config: one touch (a differing length
        // moves the fingerprint even under mtime granularity ties).
        fs::write(&config, r#"{"configVersion": 2, "packages": []}"#).expect("write config");
        let after_config = scan(&root).expect("scan");
        assert_eq!(
            diff(&after_options, &after_config),
            vec![config.clone()],
            "the package config's rewrite is the batch"
        );

        // Deletion is a touch too (the vanished half of the diff).
        fs::remove_file(&options).expect("remove options");
        let after_removal = scan(&root).expect("scan");
        let touched = diff(&after_config, &after_removal);
        assert!(
            touched.contains(&options),
            "the deletion is the batch: {touched:?}"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// The intake classifier: exactly the watched set under the root routes
    /// to the context refresh — a same-named file elsewhere is ordinary
    /// (never-staged) territory, and `.dart` paths never classify as config.
    #[test]
    fn config_input_classifier_matches_only_the_watched_set() {
        let (root, main) = workspace("config-classifier");
        assert!(is_config_input(&root.join("pubspec.yaml"), &root));
        assert!(is_config_input(&root.join("analysis_options.yaml"), &root));
        assert!(is_config_input(
            &root.join(".dart_tool/package_config.json"),
            &root
        ));
        assert!(!is_config_input(&main, &root), "dart content syncs");
        assert!(
            !is_config_input(&root.join("tool/pubspec.yaml"), &root),
            "same name outside the watched set is not a context input"
        );
        assert!(!is_config_input(
            &root.join("lib/analysis_options.yaml"),
            &root
        ));
        fs::remove_dir_all(&root).ok();
    }

    /// The poll substrate — the seam's fallback and the watch face's
    /// original behavior — reports the same touches the scan/diff pair
    /// always did, now stamped with kind and time: a rewrite is a Modify
    /// carrying the file's mtime, an addition a Create, a removal a
    /// Delete. The latency probe round-trips through the source: the
    /// measured write-to-drain latency is recorded, positive, and bounded.
    #[test]
    fn poll_source_reports_kinds_and_latency() {
        let (root, main) = workspace("poll-source");
        let mut source = PollSource::new(&root).expect("baseline");

        // A rewrite: one Modify, stamped with the write's mtime — and the
        // write-to-drain latency is positive (the drain ran after the
        // write) and far inside the probe's sane bound.
        fs::write(&main, "var edited = 7;\n").expect("rewrite");
        thread::sleep(Duration::from_millis(5));
        let observed = SystemTime::now();
        let events = source.drain();
        assert_eq!(events.len(), 1, "the rewrite is the only touch");
        assert_eq!(events[0].path, main);
        assert_eq!(events[0].kind, EventKind::Modify);
        let latency = observed
            .duration_since(events[0].at)
            .expect("the mtime precedes the drain");
        assert!(!latency.is_zero(), "latency recorded and > 0");
        assert!(
            latency < Duration::from_secs(2),
            "latency bounded: {latency:?}"
        );

        // An addition and a removal in one drain, each with its kind.
        let extra = root.join("lib/extra.dart");
        fs::write(&extra, "var b = 2;\n").expect("add");
        fs::remove_file(&main).expect("remove");
        let events = source.drain();
        assert_eq!(events.len(), 2, "the addition and the removal: {events:?}");
        let create = events
            .iter()
            .find(|event| event.kind == EventKind::Create)
            .expect("the addition");
        assert_eq!(create.path, extra);
        let delete = events
            .iter()
            .find(|event| event.kind == EventKind::Delete)
            .expect("the removal");
        assert_eq!(delete.path, main);

        // A failed walk (vanished root) is a silent tick, not a phantom
        // mass deletion.
        fs::remove_dir_all(&root).expect("rm -rf");
        assert!(source.drain().is_empty());
    }

    /// The latency probe's recording face: a drain's latency is the stalest
    /// event's wait, recorded and surfaced; a changeless drain records
    /// nothing.
    #[test]
    fn latency_probe_records_the_stalest_wait() {
        let metrics = WatchMetrics::default();
        let stale = SystemTime::now() - Duration::from_millis(150);
        let fresh = SystemTime::now();
        let events = [
            SourceEvent {
                path: PathBuf::from("a.dart"),
                kind: EventKind::Modify,
                at: stale,
            },
            SourceEvent {
                path: PathBuf::from("b.dart"),
                kind: EventKind::Create,
                at: fresh,
            },
        ];
        metrics.observe(&events, SystemTime::now());

        assert_eq!(metrics.observed_drains.load(Ordering::Relaxed), 1);
        let latency =
            Duration::from_micros(metrics.last_max_latency_micros.load(Ordering::Relaxed));
        assert!(
            latency >= Duration::from_millis(150),
            "the stalest event's wait, recorded and > 0: {latency:?}"
        );
        assert!(latency < Duration::from_secs(2), "bounded: {latency:?}");

        // A changeless drain records nothing more.
        metrics.observe(&[], SystemTime::now());
        assert_eq!(metrics.observed_drains.load(Ordering::Relaxed), 1);
    }

    /// The OS-event substrate end to end (FSEvents on this host): a
    /// create, a rewrite, and a removal each surface — with their kind and
    /// in the workspace's own path form — inside a bounded window. The
    /// substrate must start for the test to mean anything: a fallback to
    /// poll would fail loudly at the `expect`, not pass vacuously.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn os_event_source_reports_kinds() {
        let (root, _main) = workspace("os-events");
        let mut source = NotifySource::new(&root).expect("the OS substrate starts");

        let extra = root.join("lib/extra.dart");
        fs::write(&extra, "var b = 2;\n").expect("create");
        let created = wait_for_event(&mut source, EventKind::Create, &extra);
        assert_eq!(created, extra, "the create, in the walk's path form");

        fs::write(&extra, "var bcd = 234;\n").expect("rewrite");
        let modified = wait_for_event(&mut source, EventKind::Modify, &extra);
        assert_eq!(modified, extra);

        fs::remove_file(&extra).expect("remove");
        let removed = wait_for_event(&mut source, EventKind::Delete, &extra);
        assert_eq!(removed, extra);

        drop(source);
        fs::remove_dir_all(&root).ok();
    }

    /// Drains until an event of `kind` for `path` arrives, or the window
    /// closes and the test fails — bounded, never a hang.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn wait_for_event(source: &mut NotifySource, kind: EventKind, path: &Path) -> PathBuf {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            for event in source.drain() {
                if event.kind == kind && event.path == path {
                    return event.path;
                }
            }
            assert!(
                Instant::now() < deadline,
                "no {kind:?} event for {} within the window",
                path.display()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Unwraps a channel item into its touched-path batch.
    fn watch_paths(item: &[Incoming]) -> Vec<PathBuf> {
        match item {
            [Incoming::Watch { touched }] => touched.clone(),
            _ => panic!("watch intake is one Watch item"),
        }
    }

    /// Upper bound for one settle phase (a write or a removal surfacing as a
    /// batch) — only ever reached on a starved or instrumented build
    /// (coverage runs the watch thread an order of magnitude slower under
    /// load); a healthy answer arrives in the first slice, so the bound
    /// costs nothing on normal targets.
    const SETTLE_WINDOW: Duration = Duration::from_secs(15);

    /// Rewrites `path` until the watch thread answers with a Watch batch,
    /// and returns that batch plus the number of empty filler items skipped.
    ///
    /// The substrate arms asynchronously on the watch thread, and a change
    /// landing before the arm is absorbed — the OS stream's since-now and
    /// the poll baseline alike — so the first trigger retriggers until the
    /// thread answers instead of racing a fixed settle sleep. Later triggers
    /// are provably post-arm and never retried. Same-path rewrites dedup in
    /// the debounce, so the answered batch is still one entry per touched
    /// path. Bounded: a thread that never answers fails the test, never
    /// hangs it.
    fn settle_rewrite(
        path: &Path,
        content: &str,
        receiver: &Receiver<Vec<Incoming>>,
    ) -> (Vec<PathBuf>, usize) {
        let deadline = Instant::now() + SETTLE_WINDOW;
        let mut fillers = 0;
        loop {
            assert!(
                Instant::now() < deadline,
                "the write never surfaced within the window"
            );
            fs::write(path, content).expect("write");
            match receiver.recv_timeout(Duration::from_millis(500)) {
                Ok(item) if item.is_empty() => fillers += 1,
                Ok(item) => return (watch_paths(&item), fillers),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("the watch thread exited before answering")
                }
            }
        }
    }

    /// The OS substrate on the real spawned watch thread — the path the
    /// daemon itself takes (`Watcher::spawn` → `watch_loop` →
    /// `init_source`): a real write surfaces as a debounced batch within a
    /// bounded window, and drop still joins cleanly. The substrate is
    /// pinned through `spawn_with`, not the env var — libtest's parallel
    /// workers share the process environment. Out-of-scope paths (a
    /// non-watched file, a hidden-dir `.dart`) must not appear: the batch
    /// carries the walk's surface, no more.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn watch_thread_os_substrate_delivers() {
        let (root, main) = workspace("os-spawned");
        let (sender, receiver) = crossbeam_channel::bounded::<Vec<Incoming>>(4);
        let watcher = Watcher::spawn_with(
            WatchOptions {
                root: root.clone(),
                poll_interval: Duration::from_millis(25),
            },
            sender,
            Substrate::Os,
        );

        // Noise that must never reach the batch: an unwatched extension and
        // a `.dart` file under a hidden directory (the walk's dot rule).
        fs::write(root.join("lib/notes.txt"), "n\n").expect("noise file");
        fs::create_dir_all(root.join(".hidden")).expect("hidden dir");
        fs::write(root.join(".hidden/ghost.dart"), "var g = 0;\n").expect("hidden dart");

        // The first write retriggers until the thread answers (the
        // substrate arms asynchronously; a pre-arm change is absorbed); the
        // answered batch carries the watched file only.
        let (touched, _) = settle_rewrite(&main, "var edited = 42;\n", &receiver);
        assert_eq!(
            touched,
            vec![main.clone()],
            "the watched file only: {touched:?}"
        );

        drop(watcher);
        fs::remove_dir_all(&root).ok();
    }

    /// The substrate resolution table: only the explicit `poll` value
    /// escapes to the fallback substrate — unset, `os`, and noise all take
    /// the OS substrate (pinned through the env-free `parse` seam because
    /// libtest's parallel workers share the process environment).
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn substrate_parse_selects_poll_only_explicitly() {
        use std::ffi::OsStr;
        assert!(matches!(Substrate::parse(None), Substrate::Os));
        assert!(matches!(
            Substrate::parse(Some(OsStr::new("os"))),
            Substrate::Os
        ));
        assert!(matches!(
            Substrate::parse(Some(OsStr::new("noise"))),
            Substrate::Os
        ));
        assert!(matches!(
            Substrate::parse(Some(OsStr::new("poll"))),
            Substrate::Poll
        ));
    }

    /// The poll escape hatch inits and drains quiet on a clean tree — the
    /// `Substrate::Poll` arm of `init_source` stays a live fallback, not a
    /// dead branch.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn substrate_poll_escape_hatch_still_inits() {
        let (root, _main) = workspace("substrate-poll");
        let mut source = init_source(&root, Substrate::Poll).expect("poll inits");
        assert!(source.drain().is_empty(), "a clean tree drains quiet");
        fs::remove_dir_all(&root).ok();
    }
}
