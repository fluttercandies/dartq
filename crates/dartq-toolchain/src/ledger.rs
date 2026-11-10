//! The machine-wide job gate: one ledger every heavy dartq face leases
//! from — resident analysis servers and session daemons, per-run official
//! analyses, in-process engine runs, test runs and Flutter builds — for
//! two limits at once: how many jobs run (`memory.max_concurrent`) and how
//! much memory they hold together (`memory.max_percent`).
//!
//! A face that only sees its own children sizes itself against memory
//! another face is about to take; two budgets that ignore each other add
//! up past the machine. The ledger is the one place they meet.
//!
//! * **Holders.** Each lease is a file `h-<pid>-<rand>.json` under
//!   `<cache>/dartq/ledger/`, exclusively `flock`ed by its owner for the
//!   lease's life. The record names the bytes leased, the process tree to
//!   measure, the face, whether it holds a job slot (and the cap its owner
//!   declared), and whether (and how) the holder may be released. A file
//!   whose lock is free belongs to a dead owner: the kernel dropped the
//!   lock at exit, so a crashed holder frees its lease by itself, and the
//!   next scan removes the file.
//! * **Slots.** A job is admitted only while `holders + 1` fits the
//!   strictest cap among the requester and every slot holder: a worktree
//!   that declares 1 never runs beside anything. A job without a recorded
//!   peak (nor a peer's to borrow) is *unsized*; one unsized job runs at a
//!   time, so first runs on a fresh machine serialize until peaks exist.
//!   A holder that measures its peak marks its own record sized at once
//!   ([`Lease::record_peak`]), which frees the first-run slot, and a
//!   waiting request re-reads its sizing on every poll, so it borrows that
//!   peak as soon as it is recorded. Only a first run under way occupies
//!   the slot: an unsized resident that sits idle (releasable, busy lock
//!   free) does not. The first-run slot never releases anyone: it only
//!   waits.
//! * **Peaks.** Each face records its peaks under `peaks/`, one file per
//!   key holding the face and the bytes. A job without a peak of its own
//!   borrows the largest of its face: a live sized holder's or a recorded
//!   one under any key.
//! * **Accounting.** A holder counts `max(lease, live tree RSS)`, so one
//!   that grew past its lease is not undercounted. Trees nest (a debug
//!   app inside a session daemon's tree): a tree stops at another
//!   holder's root, whose subtree that holder counts.
//! * **Budget.** `max_percent` of physical memory (default 90). A request
//!   larger than the whole budget is clamped to it: it runs alone instead
//!   of waiting forever.
//! * **Availability.** The request plus the growth current holders still
//!   owe (`Σ max(0, lease − RSS)`) must also fit in the memory the OS
//!   reports available, minus a reserve of `max((100 − max_percent)% of
//!   physical, 2 GiB)`: what accounts for everything outside the ledger
//!   (the user's IDE, the Gradle daemons a build leaves behind, the OS
//!   itself on a small machine). An unreadable figure skips the check.
//! * **Admission.** Under one gate lock. Slot requests wait in arrival
//!   order (waiter tickets `w-<seq>-<pid>.lock`, locked by their waiter).
//!   On a memory or count shortfall, idle releasable holders are released
//!   least recently used first, only as far as the shortfall needs, never
//!   one whose busy lock is held; otherwise (and always for the first-run
//!   slot) the requester waits — one note, polling,
//!   bounded by its deadline. A wait that times out names the holders.
//! * **Nesting.** A slot holder passes its token to its descendants
//!   ([`TOKEN_ENV`]); a request carrying a live holder's token joins that
//!   slot instead of waiting behind it.
//! * **Bookings.** Debug apps and interactive `flutter run` hold no slot
//!   and never wait: [`book`] records their memory so every other
//!   admission sees it.
//!
//! Waits never hold the gate. The module is unix-only (`flock`);
//! elsewhere callers run ungated.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Seek as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Overrides physical memory, in bytes: a deterministic budget for tests.
pub const TOTAL_ENV: &str = "DARTQ_MEMORY_TOTAL_BYTES";
/// Overrides the OS-reported available memory, in bytes.
pub const AVAILABLE_ENV: &str = "DARTQ_MEMORY_AVAILABLE_BYTES";
/// Overrides the ledger directory (an isolated ledger for tests and CI).
pub const DIR_ENV: &str = "DARTQ_LEDGER_DIR";
/// Overrides `memory.max_concurrent` (`0` is unlimited).
pub const CAP_ENV: &str = "DARTQ_MAX_CONCURRENT";
/// The token a slot holder passes to its descendants.
pub const TOKEN_ENV: &str = "DARTQ_LEDGER_TOKEN";
/// The default `memory.max_percent`.
pub const DEFAULT_MAX_PERCENT: u8 = 90;
/// The lease of a face with no recorded peak and no peer to borrow from.
pub const DEFAULT_LEASE: u64 = 1 << 30;
/// The reserve's floor: `max_percent` may describe a big machine's share,
/// but a host always keeps at least this much for itself — the OS, the
/// desktop, the editors the ledger does not see. 2 GiB is what keeps a
/// memory-tight Mac responsive under swap pressure.
pub const RESERVE_FLOOR: u64 = 2 << 30;

/// How often a waiter re-checks admission.
const POLL: Duration = Duration::from_millis(250);
/// How long a released holder is given to exit before its lease is
/// treated as freed anyway (its file is still locked by a dying process).
const RELEASE_GRACE: Duration = Duration::from_secs(10);

/// `--no-queue`: this process bypasses the gate.
static BYPASS: AtomicBool = AtomicBool::new(false);
/// The tokens of the slots this process holds.
static HELD: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// The start of the oldest dead holder a scan collected since the last
/// [`take_dead`].
static DEAD: Mutex<Option<SystemTime>> = Mutex::new(None);

/// What a holder is, for notes and timeouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Face {
    AnalysisServer,
    Analyze,
    Engine,
    Session,
    Test,
    Build,
    Debug,
}

impl Face {
    fn noun(self) -> &'static str {
        match self {
            Self::AnalysisServer => "resident analyzer",
            Self::Analyze => "analyze run",
            Self::Engine => "engine run",
            Self::Session => "session daemon",
            Self::Test => "test run",
            Self::Build => "flutter build",
            Self::Debug => "debug app",
        }
    }
}

/// How a resident holder is released: SIGTERM to `pid` once `busy` (a
/// lock file the holder holds while it serves a request) is free.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Releasable {
    pub pid: u32,
    pub busy: PathBuf,
}

/// One lease request.
#[derive(Clone, Debug)]
pub struct Request {
    pub face: Face,
    /// What the holder serves, for notes (a worktree, a project).
    pub label: String,
    /// The bytes wanted.
    pub bytes: u64,
    /// The root of the process tree the lease accounts for; `None` until
    /// the holder spawns it ([`Lease::track`]).
    pub pid: Option<u32>,
    /// `Some` for a resident that may be released while idle.
    pub release: Option<Releasable>,
    /// Whether the job takes a slot (`false`: memory only).
    pub slot: bool,
    /// The requester's `memory.max_concurrent`; `0` is unlimited.
    pub cap: u32,
    /// Whether `bytes` is a measured peak ([`sizing`]).
    pub sized: bool,
    /// The key whose sizing an unsized slot request re-reads on every poll
    /// of its wait ([`sizing`]): it is admitted sized as soon as a peak of
    /// its own or of a peer of its face is recorded.
    pub peak_key: Option<String>,
    /// The analyzed Dart source size — [`sizing_with_source`]'s estimate
    /// input. `None` for callers that don't measure it.
    pub source_bytes: Option<u64>,
}

/// One holder file's record.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Record {
    face: Face,
    label: String,
    bytes: u64,
    pid: Option<u32>,
    release: Option<Releasable>,
    #[serde(default)]
    slot: bool,
    #[serde(default)]
    cap: u32,
    #[serde(default)]
    sized: bool,
    /// The owning process.
    #[serde(default)]
    owner: u32,
    /// When the lease was granted, in seconds since the epoch.
    #[serde(default)]
    since: u64,
}

/// A live holder as admission sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Holder {
    pub face: Face,
    pub label: String,
    pub lease: u64,
    pub rss: u64,
    /// Last use: the holder file's mtime ([`Lease::touch`]).
    pub used: SystemTime,
    /// `Some(idle)` for a releasable holder: whether its busy lock is free.
    pub idle: Option<bool>,
    pub slot: bool,
    pub cap: u32,
    pub sized: bool,
    pub owner: u32,
    pub since: u64,
}

impl Holder {
    /// What the holder counts against the budget.
    fn counted(&self) -> u64 {
        self.lease.max(self.rss)
    }

    /// The growth it may still take inside its lease.
    fn owed(&self) -> u64 {
        self.lease.saturating_sub(self.rss)
    }
}

/// The machine's figures for one admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Figures {
    pub total: u64,
    /// `None` when the OS figure is unreadable: the check is skipped.
    pub available: Option<u64>,
    pub max_percent: u8,
}

impl Figures {
    pub(crate) fn budget(self) -> u64 {
        percent(self.total, self.max_percent)
    }

    /// The memory kept free for everything outside the ledger (the IDE,
    /// the Gradle daemons a build leaves behind): `(100 − max_percent)%`
    /// of physical, never below [`RESERVE_FLOOR`] — 90% of a 16 GB machine
    /// leaves a 1.6 GB reserve, which is under what a frozen desktop needs.
    fn reserve(self) -> u64 {
        percent(self.total, 100 - self.max_percent).max(RESERVE_FLOOR)
    }
}

fn percent(bytes: u64, percent: u8) -> u64 {
    u64::try_from(u128::from(bytes) * u128::from(percent) / 100).unwrap_or(u64::MAX)
}

/// What one admission asks of the snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Ask {
    /// The least bytes the job runs with, already clamped to the budget.
    pub min: u64,
    pub slot: bool,
    pub cap: u32,
    pub sized: bool,
}

/// Why a request waits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shortfall {
    Memory,
    /// `running` slot holders already fill `cap`.
    Count {
        running: usize,
        cap: u32,
    },
    /// Another unsized job runs.
    FirstRun,
}

impl Shortfall {
    /// What the waiter waits for, as the note and the timeout say it.
    fn what(self, min: u64) -> String {
        match self {
            Self::Memory => format!("host memory ({} wanted)", gib(min)),
            Self::Count { running, cap } => {
                format!("a job slot ({running} of {cap} running)")
            }
            Self::FirstRun => {
                "the first-run slot (one job without a recorded peak at a time)".into()
            }
        }
    }

    /// The timeout's spelling: what was waited for, without the figures.
    fn resource(self) -> &'static str {
        match self {
            Self::Memory => "host memory",
            Self::Count { .. } => "a job slot",
            Self::FirstRun => "the first-run slot",
        }
    }
}

/// The admission decision over a snapshot.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    /// The ask fits after releasing these holders (indices, release
    /// order); `headroom` is the memory that fits then, at least `min`.
    Admit { release: Vec<usize>, headroom: u64 },
    /// The ask does not fit even with every idle releasable holder that
    /// would help gone.
    Wait(Shortfall),
}

/// The admission policy. Memory: what fits is the smaller of the budget
/// left after the counted holders and — with an available figure —
/// available memory minus the reserve minus the growth the holders still
/// owe. A slot ask also needs a free slot under the strictest cap, and an
/// unsized one no first run under way (an unsized slot holder other than
/// an idle resident). While short, the least recently used idle
/// releasable holder that would help goes: any for memory (it frees what
/// it counts and what it holds now plus what it owed), a slot holder for
/// the count. The first-run slot releases no one: the first run under way
/// ends by recording its peak or by exiting. A busy or non-releasable
/// holder always stays.
pub(crate) fn decide(holders: &[Holder], ask: Ask, figures: Figures) -> Decision {
    let budget = figures.budget();
    let room = figures
        .available
        .map(|available| available.saturating_sub(figures.reserve()));
    let mut gone = vec![false; holders.len()];
    let mut idle: Vec<usize> = (0..holders.len())
        .filter(|&index| holders[index].idle == Some(true))
        .collect();
    idle.sort_by_key(|&index| holders[index].used);
    let mut release = Vec::new();
    loop {
        let live = holders
            .iter()
            .zip(&gone)
            .filter(|(_, gone)| !**gone)
            .map(|(holder, _)| holder);
        let counted: u64 = live.clone().map(Holder::counted).sum();
        let owed: u64 = live.clone().map(Holder::owed).sum();
        let freed: u64 = holders
            .iter()
            .zip(&gone)
            .filter(|(_, gone)| **gone)
            .map(|(holder, _)| holder.rss)
            .sum();
        let left = budget.saturating_sub(counted);
        let headroom = room.map_or(left, |room| {
            left.min(room.saturating_add(freed).saturating_sub(owed))
        });
        let shortfall = if headroom < ask.min {
            Some(Shortfall::Memory)
        } else if ask.slot {
            slot_shortfall(live, ask)
        } else {
            None
        };
        let Some(shortfall) = shortfall else {
            return Decision::Admit { release, headroom };
        };
        let helps = |holder: &Holder| match shortfall {
            Shortfall::Memory => true,
            Shortfall::Count { .. } => holder.slot,
            Shortfall::FirstRun => false,
        };
        let Some(at) = idle
            .iter()
            .position(|&index| !gone[index] && helps(&holders[index]))
        else {
            return Decision::Wait(shortfall);
        };
        let index = idle.remove(at);
        gone[index] = true;
        release.push(index);
    }
}

/// Whether the live slot holders leave `ask` a slot: the strictest
/// declared cap, then the one-unsized-job rule.
fn slot_shortfall<'a>(live: impl Iterator<Item = &'a Holder>, ask: Ask) -> Option<Shortfall> {
    let slots: Vec<&Holder> = live.filter(|holder| holder.slot).collect();
    let cap = slots
        .iter()
        .map(|holder| holder.cap)
        .chain([ask.cap])
        .filter(|&cap| cap > 0)
        .min();
    if let Some(cap) = cap
        && slots.len() >= cap as usize
    {
        return Some(Shortfall::Count {
            running: slots.len(),
            cap,
        });
    }
    if !ask.sized && slots.iter().any(|holder| first_run(holder)) {
        return Some(Shortfall::FirstRun);
    }
    None
}

/// Whether `holder` occupies the first-run slot: unsized and running — a
/// non-releasable job, or a resident whose busy lock is held. An idle
/// unsized resident has no first run under way.
fn first_run(holder: &Holder) -> bool {
    !holder.sized && holder.idle != Some(true)
}

/// Physical memory: [`TOTAL_ENV`] when set, else the OS figure. `0` when
/// neither answers.
#[must_use]
pub fn total_memory() -> u64 {
    env_number(TOTAL_ENV)
        .filter(|&bytes| bytes > 0)
        .unwrap_or_else(crate::proc::physical_memory)
}

/// Memory the OS reports available: [`AVAILABLE_ENV`] when set, else
/// `MemAvailable` (Linux) or `kern.memorystatus_level` percent of physical
/// memory (macOS, the figure `memory_pressure` prints as "System-wide
/// memory free percentage"). `None` when unreadable.
#[must_use]
pub fn available_memory() -> Option<u64> {
    if let Some(bytes) = env_number(AVAILABLE_ENV).filter(|&bytes| bytes > 0) {
        return Some(bytes);
    }
    available_memory_os()
}

#[cfg(target_os = "linux")]
fn available_memory_os() -> Option<u64> {
    let text = fs::read_to_string("/proc/meminfo").ok()?;
    let line = text
        .lines()
        .find_map(|line| line.strip_prefix("MemAvailable:"))?;
    let kib: u64 = line.trim().trim_end_matches("kB").trim().parse().ok()?;
    Some(kib.saturating_mul(1024))
}

#[cfg(target_os = "macos")]
fn available_memory_os() -> Option<u64> {
    let mut level: u32 = 0;
    let mut len = std::mem::size_of::<u32>();
    // SAFETY: `kern.memorystatus_level` is a u32 percentage; the
    // out-pointer and length match it.
    let rc = unsafe {
        libc::sysctlbyname(
            c"kern.memorystatus_level".as_ptr(),
            (&raw mut level).cast(),
            &raw mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    u8::try_from(level)
        .ok()
        .filter(|&level| rc == 0 && (1..=100).contains(&level))
        .map(|level| percent(crate::proc::physical_memory(), level))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn available_memory_os() -> Option<u64> {
    None
}

/// The OS's own memory-pressure level — `kern.memorystatus_vm_pressure_level`
/// on macOS (1 = normal, 2 = warn, 4 = critical), `None` where the counter
/// does not exist or cannot be read. The kernel computes it from free +
/// file-backed pages against the memorystatus tables, so it sees pressure
/// a free-bytes figure misses (compressor churn, cache starvation).
#[must_use]
pub fn pressure_level() -> Option<u32> {
    #[cfg(target_os = "macos")]
    {
        let mut level: u32 = 0;
        let mut len = std::mem::size_of::<u32>();
        // SAFETY: `kern.memorystatus_vm_pressure_level` is a u32; the
        // out-pointer and length match it.
        let rc = unsafe {
            libc::sysctlbyname(
                c"kern.memorystatus_vm_pressure_level".as_ptr(),
                (&raw mut level).cast(),
                &raw mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        (rc == 0).then_some(level)
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// The job cap in force for a requester that declared `configured`:
/// [`CAP_ENV`] when set to a number, else `configured` (`0` unlimited).
#[must_use]
pub fn cap(configured: u32) -> u32 {
    env_number(CAP_ENV)
        .and_then(|cap| u32::try_from(cap).ok())
        .unwrap_or(configured)
}

fn env_number(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.trim().parse().ok()
}

/// Makes every later request of this process run ungated (`--no-queue`).
pub fn bypass() {
    BYPASS.store(true, Ordering::Relaxed);
}

/// [`DIR_ENV`] when set, else `$XDG_CACHE_HOME|~/.cache` +
/// `dartq/ledger`: one ledger per user, the directory every dartq binary
/// of the user agrees on.
fn ledger_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(DIR_ENV).filter(|dir| !dir.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|home| home.join(".cache")))
        .map(|cache| cache.join("dartq").join("ledger"))
}

/// The recorded peak of `key` (a face's stable identity: a project and
/// target, a worktree), in bytes.
#[must_use]
pub fn peak(key: &str) -> Option<u64> {
    peak_in(&ledger_dir()?, key)
}

fn peak_in(dir: &Path, key: &str) -> Option<u64> {
    read_peak(&dir.join("peaks").join(peak_file(key))).map(|peak| peak.bytes)
}

/// One recorded peak file. A file in any other shape (truncated,
/// foreign) is not a peak: `None`.
fn read_peak(path: &Path) -> Option<Peak> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// The recorded peaks of `face`, under any key.
fn face_peaks(dir: &Path, face: Face) -> impl Iterator<Item = u64> {
    face_peak_entries(dir, face).map(|(bytes, _)| bytes)
}

/// The recorded peaks of `face`, under any key, with their source sizes.
fn face_peak_entries(dir: &Path, face: Face) -> impl Iterator<Item = (u64, Option<u64>)> {
    fs::read_dir(dir.join("peaks"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .filter_map(move |entry| {
            let peak = read_peak(&entry.path())?;
            (peak.face == face).then_some((peak.bytes, peak.source_bytes))
        })
}

/// A peaks-store file.
#[derive(serde::Serialize, serde::Deserialize)]
struct Peak {
    face: Face,
    bytes: u64,
    /// The analyzed Dart source size the peak was measured on, for the
    /// ratio estimate that sizes unrecorded jobs of the same face.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_bytes: Option<u64>,
}

/// Records `bytes` as the peak of `key`, a job of `face` (the latest
/// run's, so a project that shrank books less next time).
pub fn record_peak(key: &str, face: Face, bytes: u64) {
    if let Some(dir) = ledger_dir() {
        record_peak_in(&dir, key, face, bytes, None);
    }
}

/// [`record_peak`] with the analyzed source size the peak was measured
/// on — the ratio estimate's input for unrecorded jobs of the same face.
pub fn record_peak_with_source(key: &str, face: Face, bytes: u64, source_bytes: u64) {
    if let Some(dir) = ledger_dir() {
        record_peak_in(&dir, key, face, bytes, Some(source_bytes));
    }
}

/// [`record_peak_with_source`] that never lowers a recorded peak: the
/// growth brake's observed figure is a lower bound (the job was still
/// growing when it yielded), so a re-queued job must be admitted on at
/// least the earlier full measurement. A normal completion still calls
/// [`record_peak_with_source`], so a project that shrank books less.
pub fn record_peak_floor(key: &str, face: Face, bytes: u64, source_bytes: u64) {
    if let Some(dir) = ledger_dir() {
        let floor = peak_in(&dir, key).unwrap_or(0).max(bytes);
        record_peak_in(&dir, key, face, floor, Some(source_bytes));
    }
}

/// The peaks store's file cap: unsized waiters re-scan `face_peak_entries`
/// every poll, so the store stays small. On record, files past
/// [`PEAK_FILES_MAX`] are deleted oldest-mtime-first down to the cap.
const PEAK_FILES_MAX: usize = 512;

fn record_peak_in(dir: &Path, key: &str, face: Face, bytes: u64, source_bytes: Option<u64>) {
    if bytes == 0 {
        return;
    }
    let dir = dir.join("peaks");
    let text = serde_json::to_string(&Peak {
        face,
        bytes,
        source_bytes,
    })
    .expect("a peak serializes");
    if fs::create_dir_all(&dir).is_ok() {
        let temp = dir.join(format!(".{}.{}", peak_file(key), std::process::id()));
        if fs::write(&temp, text).is_ok() {
            let _ = fs::rename(&temp, dir.join(peak_file(key)));
            trim_peaks(&dir);
        }
    }
}

/// Deletes the oldest peak files while more than [`PEAK_FILES_MAX`]
/// exist — mtime order, so fresh records and the just-written file stay.
fn trim_peaks(dir: &Path) {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|entry| {
            entry
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .map(|mtime| (mtime, entry.path()))
        })
        .collect();
    let excess = entries.len().saturating_sub(PEAK_FILES_MAX);
    if excess == 0 {
        return;
    }
    entries.sort_by_key(|(mtime, _)| *mtime);
    for (_, path) in entries.into_iter().take(excess) {
        let _ = fs::remove_file(path);
    }
}

/// A key as a file name: alphanumerics kept, everything else `_`, plus a
/// hash of the whole key so two keys never share a file.
fn peak_file(key: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in key.bytes() {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    }
    let tail: String = key
        .chars()
        .rev()
        .take(48)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("{tail}-{hash:016x}")
}

/// The lease to book for `key` and whether it is a measurement: its
/// recorded peak, else the largest peak of `face` — a live sized holder's
/// or one recorded under any key — else [`DEFAULT_LEASE`], unsized: a
/// first run, which admission serializes with every other first run. An
/// unsized request names `key` as its [`Request::peak_key`] to be sized
/// while it waits.
///
/// For [`Face::AnalysisServer`], when `source_bytes` is `Some`, a peak's
/// `peak/source_bytes` ratio estimates this job's size. An estimated job
/// is sized: it runs in parallel with other sized/estimated jobs when
/// the budget admits, waits FIFO when it doesn't.
#[must_use]
pub fn sizing(key: &str, face: Face) -> (u64, bool) {
    sizing_with_source(key, face, None)
}

/// [`sizing`] with the analyzed source size for ratio-based estimates.
#[must_use]
pub fn sizing_with_source(key: &str, face: Face, source_bytes: Option<u64>) -> (u64, bool) {
    let Some(dir) = ledger_dir() else {
        return (DEFAULT_LEASE, false);
    };
    let holders: Vec<Holder> = scan(&dir).into_iter().map(|(_, holder)| holder).collect();
    sized_in(&dir, Some(key), face, &holders, source_bytes)
        .map_or((DEFAULT_LEASE, false), |bytes| (bytes, true))
}

/// The measured lease of a job: `key`'s recorded peak under `dir`, else
/// a ratio-based estimate when `source_bytes` is given and analyzer
/// peaks carry their own source sizes, else the largest peak of `face`,
/// live or recorded (a peer's). The face-peak borrow is skipped for a
/// [`Face::AnalysisServer`] job with known source bytes: a big peer's
/// peak under-estimates a small job's fixed analyzer base, so this job
/// falls through to `None` — unsized, the first-run slot.
fn sized_in(
    dir: &Path,
    key: Option<&str>,
    face: Face,
    holders: &[Holder],
    source_bytes: Option<u64>,
) -> Option<u64> {
    key.and_then(|key| peak_in(dir, key))
        .or_else(|| estimate(dir, face, source_bytes))
        .or_else(|| {
            if face == Face::AnalysisServer && source_bytes.is_some() {
                return None;
            }
            holders
                .iter()
                .filter(|holder| holder.face == face && holder.sized && holder.lease > 0)
                .map(|holder| holder.lease)
                .chain(face_peaks(dir, face))
                .max()
        })
}

/// The ratio estimate over comparable-scale recorded peaks: only a peak
/// whose source size is within `[S/4, 4·S]` of this job's `S` counts —
/// the analyzer's fixed base (SDK summaries) makes a tiny project's
/// peak/source ratio huge and a huge project's ratio small. Each
/// comparable peak gives the bound `peak × max(1, S / S_i)`: a bigger
/// project's peak is never scaled down (the base makes downward scaling
/// an under-estimate), a smaller one's is scaled up by at most 4×. The
/// estimate is the largest bound; `None` when no peak is comparable or
/// the bound could never be admitted (`> total − reserve`, a figure no
/// machine runs — the job is then unsized, the first-run slot).
fn estimate(dir: &Path, face: Face, source_bytes: Option<u64>) -> Option<u64> {
    if face != Face::AnalysisServer {
        return None;
    }
    let my_source = source_bytes?;
    if my_source == 0 {
        return None;
    }
    let bound = face_peak_entries(dir, face)
        .filter_map(|(peak, src)| src.map(|src| (peak, src)))
        .filter(|(_, src)| {
            *src > 0 && *src >= my_source.div_ceil(4) && *src <= my_source.saturating_mul(4)
        })
        .map(|(peak, src)| {
            let scale = (my_source as u128).div_ceil(src as u128);
            peak as u128 * scale.max(1)
        })
        .max()?;
    let estimate = u64::try_from(bound).unwrap_or(u64::MAX);
    let cap = total_memory().saturating_sub(
        Figures {
            total: total_memory(),
            available: None,
            max_percent: DEFAULT_MAX_PERCENT,
        }
        .reserve(),
    );
    (estimate <= cap).then_some(estimate)
}

/// The live leases: face, label and bytes of each holder (the view
/// `dartq` status faces and the enrolled faces' tests read).
#[must_use]
pub fn leases() -> Vec<(Face, String, u64)> {
    ledger_dir()
        .map(|dir| scan(&dir))
        .unwrap_or_default()
        .into_iter()
        .map(|(_, holder)| (holder.face, holder.label, holder.lease))
        .collect()
}

/// The start of the oldest holder that died without returning its lease
/// since the last call: its children (compilers, test processes) may
/// outlive it, reparented and still holding their memory, and everything
/// they spawned is younger. The test face sweeps such orphans.
#[must_use]
pub fn take_dead() -> Option<SystemTime> {
    DEAD.lock().ok()?.take()
}

/// Whether this process runs inside a slot: one it holds, or one its
/// ancestor holds right now and passed down through [`TOKEN_ENV`].
#[must_use]
pub fn nested() -> bool {
    inherited_token().is_some()
}

/// The token this process passes to its children: its own slot's, else
/// the inherited one while its holder still holds a slot.
fn inherited_token() -> Option<String> {
    if let Some(token) = HELD.lock().ok().and_then(|held| held.last().cloned()) {
        return Some(token);
    }
    env_token(&ledger_dir()?)
}

/// The [`TOKEN_ENV`] token when it names a live holder under `dir` that
/// holds a slot now. A holder that left its slot (a session daemon hosting
/// a debug session) shelters nothing: a nested request then takes a slot
/// of its own, so the count cap still holds.
fn env_token(dir: &Path) -> Option<String> {
    let token = std::env::var(TOKEN_ENV).ok().filter(|token| {
        token.starts_with("h-") && token.ends_with(".json") && !token.contains('/')
    })?;
    let mut file = File::open(dir.join(&token)).ok()?;
    // A held lock is a live holder; a free one is a dead holder's.
    if try_lock(&file).unwrap_or(true) {
        return None;
    }
    // The holder rewrites its record in place (truncate, then write), so
    // a read can land between the two: an unparsable read is retried.
    for _ in 0..3 {
        let mut text = String::new();
        if file.seek(std::io::SeekFrom::Start(0)).is_ok()
            && file.read_to_string(&mut text).is_ok()
            && let Ok(record) = serde_json::from_str::<Record>(&text)
        {
            return record.slot.then_some(token);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    None
}

/// Passes this process's slot to `command`'s child, so a dartq request
/// in that child joins the slot instead of waiting behind it.
pub fn inherit(command: &mut std::process::Command) {
    if let Some(token) = inherited_token() {
        command.env(TOKEN_ENV, token);
    }
}

/// Keeps a detached resident `command` would start out of this process's
/// slot: it outlives the slot, so it takes its own.
pub fn detach(command: &mut std::process::Command) {
    command.env_remove(TOKEN_ENV);
}

/// One flock attempt: `Ok(true)` taken, `Ok(false)` held elsewhere.
fn try_lock(file: &File) -> io::Result<bool> {
    match file.try_lock() {
        Ok(()) => Ok(true),
        Err(fs::TryLockError::WouldBlock) => Ok(false),
        Err(fs::TryLockError::Error(error)) => Err(error),
    }
}

fn unix_secs(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// The live holders under `dir`, each with its tree measured from one
/// process snapshot. A holder file whose lock is free is a dead owner's:
/// it is removed, freeing its lease, and its start is kept for
/// [`take_dead`].
fn scan(dir: &Path) -> Vec<(PathBuf, Holder)> {
    let mut records = Vec::new();
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let is_holder = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("h-") && name.ends_with(".json"));
        if !is_holder {
            continue;
        }
        let Ok(mut file) = File::open(&path) else {
            continue;
        };
        let live = match try_lock(&file) {
            Ok(taken) => !taken,
            Err(_) => continue,
        };
        let mut text = String::new();
        let record = file
            .read_to_string(&mut text)
            .ok()
            .and_then(|_| serde_json::from_str::<Record>(&text).ok());
        if !live {
            // The owner is gone; the lock is ours for the removal.
            let _ = fs::remove_file(&path);
            if let (Some(record), Ok(mut dead)) = (record, DEAD.lock()) {
                let since = UNIX_EPOCH + Duration::from_secs(record.since);
                *dead = Some(dead.map_or(since, |oldest| oldest.min(since)));
            }
            continue;
        }
        let Some(record) = record else {
            continue;
        };
        let used = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .unwrap_or(UNIX_EPOCH);
        records.push((path, record, used));
    }
    let rows = crate::proc::table();
    let roots: Vec<u32> = records
        .iter()
        .filter_map(|(_, record, _)| record.pid)
        .collect();
    records
        .into_iter()
        .map(|(path, record, used)| {
            let rss = record.pid.map_or(0, |pid| {
                crate::proc::rss_of(&rows, &crate::proc::tree_excluding(&rows, pid, &roots))
            });
            let idle = record.release.as_ref().map(|release| {
                File::open(&release.busy)
                    .ok()
                    .is_some_and(|busy| try_lock(&busy).unwrap_or(false))
            });
            let holder = Holder {
                face: record.face,
                label: record.label,
                lease: record.bytes,
                rss,
                used,
                idle,
                slot: record.slot,
                cap: record.cap,
                sized: record.sized,
                owner: record.owner,
                since: record.since,
            };
            (path, holder)
        })
        .collect()
}

/// A held lease. Dropping it removes the holder file and releases the
/// lock; a crashed owner's lock is released by the kernel.
#[derive(Debug)]
#[must_use = "dropping the lease frees it at once"]
pub struct Lease {
    path: PathBuf,
    file: File,
    record: Record,
    dir: PathBuf,
    /// Whether [`Lease::leave_slot`] gave a slot up that
    /// [`Lease::retake_slot`] has not taken back.
    left: bool,
}

impl Lease {
    /// The bytes leased.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.record.bytes
    }

    /// Whether the lease holds a job slot.
    #[must_use]
    pub fn holds_slot(&self) -> bool {
        self.record.slot
    }

    /// Whether the lease books a measured peak.
    #[must_use]
    pub fn sized(&self) -> bool {
        self.record.sized
    }

    /// Records `peak` as `key`'s peak ([`record_peak`], under the lease's
    /// face) and books it in
    /// this lease at once: the record turns sized with `max(lease, peak)`
    /// bytes, so the holder leaves the first-run slot and a peer of its
    /// face can borrow the figure while it still runs.
    pub fn record_peak(&mut self, key: &str, peak: u64) {
        self.record_peak_with_source(key, peak, None);
    }

    /// [`record_peak`] with the analyzed source size — the ratio
    /// estimate's input for unrecorded jobs of the same face.
    pub fn record_peak_with_source(&mut self, key: &str, peak: u64, source_bytes: Option<u64>) {
        record_peak_in(&self.dir, key, self.record.face, peak, source_bytes);
        if peak == 0 || (self.record.sized && self.record.bytes >= peak) {
            return;
        }
        // Rewritten under the gate lock: an admission scan never reads
        // the record between its truncation and its write.
        let gate = open_gate(&self.dir).and_then(|gate| gate.lock().map(|()| gate));
        self.record.sized = true;
        self.record.bytes = self.record.bytes.max(peak);
        self.write();
        drop(gate);
    }

    /// Names the process tree this lease accounts for, once spawned.
    pub fn track(&mut self, pid: u32) {
        self.record.pid = Some(pid);
        self.write();
    }

    /// Marks the holder used now: release order is least recently used.
    pub fn touch(&self) {
        let _ = self.file.set_modified(SystemTime::now());
    }

    /// Gives the slot up while keeping the memory booked (a session daemon
    /// that starts hosting a debug session).
    pub fn leave_slot(&mut self) {
        if self.record.slot {
            self.record.slot = false;
            self.left = true;
            self.write();
            unregister(&self.path);
        }
    }

    /// Takes the slot [`Lease::leave_slot`] gave up back without waiting (a
    /// session daemon whose last debug session ended): `false` when every
    /// slot is taken — the strictest cap, or the first-run slot of an
    /// unsized lease — and the
    /// lease stays slotless. A lease that left no slot has nothing to
    /// retake: `true`.
    ///
    /// # Errors
    ///
    /// The gate lock cannot be taken.
    pub fn retake_slot(&mut self) -> io::Result<bool> {
        if !self.left || BYPASS.load(Ordering::Relaxed) {
            return Ok(true);
        }
        let gate = open_gate(&self.dir)?;
        gate.lock()?;
        let others: Vec<Holder> = scan(&self.dir)
            .into_iter()
            .filter(|(path, _)| *path != self.path)
            .map(|(_, holder)| holder)
            .collect();
        let ask = Ask {
            min: 0,
            slot: true,
            cap: self.record.cap,
            sized: self.record.sized,
        };
        if slot_shortfall(others.iter(), ask).is_some() {
            return Ok(false);
        }
        self.record.slot = true;
        self.left = false;
        self.write();
        register(&self.path);
        Ok(true)
    }

    fn write(&mut self) {
        use std::os::unix::fs::FileExt as _;
        let bytes = serde_json::to_vec(&self.record).expect("a record serializes");
        let _ = self.file.set_len(0);
        let _ = self.file.write_all_at(&bytes, 0);
        let _ = self.file.set_modified(SystemTime::now());
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        unregister(&self.path);
        // Removed while still locked, so no scan sees a free file.
        let _ = fs::remove_file(&self.path);
    }
}

fn token_of(path: &Path) -> Option<String> {
    path.file_name()?.to_str().map(str::to_owned)
}

fn register(path: &Path) {
    if let (Some(token), Ok(mut held)) = (token_of(path), HELD.lock()) {
        held.push(token);
    }
}

fn unregister(path: &Path) {
    if let (Some(token), Ok(mut held)) = (token_of(path), HELD.lock()) {
        held.retain(|held| *held != token);
    }
}

/// Why no lease was granted.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("timed out waiting for {what} held by other dartq runs: {holders}")]
    Timeout { what: &'static str, holders: String },
    #[error("job gate unavailable: {0}")]
    Io(#[from] io::Error),
    /// The caller's stop flag rose while it waited.
    #[error("stopped while waiting for the job gate")]
    Stopped,
}

/// Leases `request.bytes` (at least `min`, at most the request, both
/// clamped to the budget) from the ledger, waiting until `deadline`: for a
/// slot request, also a job slot. The grant is as large as fits.
/// `max_percent` is `memory.max_percent`; `on_note` receives the waiting
/// and release notes. `Ok(None)` runs ungated: `--no-queue`
/// ([`bypass`]), no ledger (no cache directory, no figure for physical
/// memory), or a slot request inside another process's slot (it joins
/// that slot; its memory is that holder's tree). A slot request of a
/// process that already holds a slot books memory only.
///
/// # Errors
///
/// [`Error::Timeout`] when `deadline` passes first; [`Error::Io`] when the
/// ledger directory cannot be used.
pub fn acquire(
    request: Request,
    min: u64,
    max_percent: u8,
    deadline: Instant,
    on_note: impl FnMut(&str),
) -> Result<Option<Lease>, Error> {
    acquire_or_stop(
        request,
        min,
        max_percent,
        deadline,
        &AtomicBool::new(false),
        on_note,
    )
}

/// [`acquire`] for a waiter that must stay responsive to a signal: the
/// wait ends with [`Error::Stopped`] once `stop` is set.
///
/// # Errors
///
/// As [`acquire`], plus [`Error::Stopped`].
pub fn acquire_or_stop(
    mut request: Request,
    min: u64,
    max_percent: u8,
    deadline: Instant,
    stop: &AtomicBool,
    on_note: impl FnMut(&str),
) -> Result<Option<Lease>, Error> {
    if BYPASS.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let Some(dir) = ledger_dir() else {
        return Ok(None);
    };
    let total = total_memory();
    if total == 0 {
        return Ok(None);
    }
    if joins(&mut request, &dir) {
        return Ok(None);
    }
    let stopped = || stop.load(Ordering::Relaxed);
    acquire_in(&dir, request, min, deadline, &stopped, on_note, || {
        Figures {
            total,
            available: available_memory(),
            max_percent: max_percent.clamp(1, 100),
        }
    })
}

/// The nesting rule: a slot request of a process that holds a slot itself
/// books memory only; one carrying the token of an ancestor that holds a
/// slot now joins that slot (`true`: no lease of its own).
fn joins(request: &mut Request, dir: &Path) -> bool {
    if !request.slot {
        return false;
    }
    if HELD.lock().map(|held| !held.is_empty()).unwrap_or(false) {
        request.slot = false;
        return false;
    }
    env_token(dir).is_some()
}

/// Books `request.bytes` at once, without waiting and without a slot:
/// memory a debug app or an interactive `flutter run` takes whatever the
/// gate says, recorded so every other admission sees it. Never released.
/// `None` when the host has no ledger or the process bypasses it.
#[must_use]
pub fn book(mut request: Request) -> Option<Lease> {
    if BYPASS.load(Ordering::Relaxed) {
        return None;
    }
    let dir = ledger_dir()?;
    fs::create_dir_all(&dir).ok()?;
    request.slot = false;
    request.release = None;
    let gate = open_gate(&dir).ok()?;
    gate.lock().ok()?;
    hold(&dir, &request, request.bytes).ok()
}

fn open_gate(dir: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("gate.lock"))
}

/// [`acquire`] over an explicit directory and figures source (the tests'
/// seam).
fn acquire_in(
    dir: &Path,
    mut request: Request,
    mut min: u64,
    deadline: Instant,
    stop: &dyn Fn() -> bool,
    mut on_note: impl FnMut(&str),
    figures: impl Fn() -> Figures,
) -> Result<Option<Lease>, Error> {
    fs::create_dir_all(dir)?;
    let gate = open_gate(dir)?;
    let mut waiting: Option<Shortfall> = None;
    let mut released: Vec<(PathBuf, Instant)> = Vec::new();
    let mut ticket: Option<Ticket> = None;
    loop {
        gate.lock()?;
        if request.slot && ticket.is_none() {
            ticket = Some(Ticket::join(dir)?);
        }
        // A holder just released keeps its file until it exits; within the
        // grace it counts as gone (its memory is on the way out).
        released.retain(|(path, at)| path.exists() && at.elapsed() < RELEASE_GRACE);
        let holders: Vec<(PathBuf, Holder)> = scan(dir)
            .into_iter()
            .filter(|(path, _)| !released.iter().any(|(gone, _)| gone == path))
            .collect();
        let snapshot: Vec<Holder> = holders.iter().map(|(_, holder)| holder.clone()).collect();
        // An unsized slot request takes a peak recorded while it waited
        // (its own key's, or a peer's of its face) and leaves the
        // first-run line.
        if request.slot
            && !request.sized
            && let Some(bytes) = sized_in(
                dir,
                request.peak_key.as_deref(),
                request.face,
                &snapshot,
                request.source_bytes,
            )
        {
            request.sized = true;
            request.bytes = bytes;
            min = min.min(bytes);
        }
        let figures = figures();
        let budget = figures.budget();
        // The clamp: a request past the whole budget runs alone.
        let want = request.bytes.min(budget);
        let ask = Ask {
            min: min.min(want),
            slot: request.slot,
            cap: request.cap,
            sized: request.sized,
        };
        let ahead = ticket.as_ref().map_or(0, |ticket| ticket.ahead(dir));
        let decision = if ahead > 0 {
            None
        } else {
            Some(decide(&snapshot, ask, figures))
        };
        match decision {
            Some(Decision::Admit { release, headroom }) => {
                let mut pending = false;
                for index in release {
                    let (path, holder) = &holders[index];
                    if release_holder(path) {
                        on_note(&format!(
                            "dartq: released the idle {} of {} to make room",
                            holder.face.noun(),
                            holder.label
                        ));
                        released.push((path.clone(), Instant::now()));
                        pending = true;
                    }
                }
                if !pending {
                    let bytes = want.min(headroom).max(ask.min);
                    let lease = hold(dir, &request, bytes);
                    drop(ticket);
                    drop(gate);
                    if waiting.is_some() && lease.is_ok() {
                        on_note("dartq: admitted");
                    }
                    return lease.map(Some).map_err(Error::Io);
                }
                // Released holders free their memory as they exit: the
                // next pass re-measures before admitting.
            }
            Some(Decision::Wait(shortfall)) => {
                if waiting.is_none() {
                    on_note(&format!(
                        "dartq: waiting for {}; held by {}",
                        shortfall.what(ask.min),
                        describe(&snapshot)
                    ));
                }
                waiting = Some(shortfall);
            }
            None => {
                if waiting.is_none() {
                    on_note(&format!(
                        "dartq: waiting in line ({ahead} ahead, first come first served); \
                         held by {}",
                        describe(&snapshot)
                    ));
                }
                waiting = Some(waiting.unwrap_or(Shortfall::Count {
                    running: snapshot.iter().filter(|holder| holder.slot).count(),
                    cap: request.cap,
                }));
            }
        }
        drop(gate.unlock());
        if stop() {
            return Err(Error::Stopped);
        }
        if Instant::now() + POLL > deadline {
            let holders: Vec<Holder> = scan(dir).into_iter().map(|(_, holder)| holder).collect();
            return Err(Error::Timeout {
                what: waiting.map_or("host memory", Shortfall::resource),
                holders: describe(&holders),
            });
        }
        std::thread::sleep(POLL);
    }
}

/// A slot waiter's place in line: `w-<seq>-<rand>.lock`, locked by its
/// waiter until admitted or gone. Served by seq, then rand; an unlocked
/// ticket is a dead waiter's and is skipped and removed.
#[derive(Debug)]
struct Ticket {
    path: PathBuf,
    seq: u64,
    rand: u64,
    _file: File,
}

impl Ticket {
    /// Joins the line under the gate lock: one past the highest seq.
    fn join(dir: &Path) -> io::Result<Self> {
        let seq = Self::all(dir)
            .iter()
            .map(|(seq, _, _)| *seq)
            .max()
            .unwrap_or(0)
            + 1;
        let rand = crate::run_queue::random_u64();
        let path = dir.join(format!("w-{seq}-{rand}.lock"));
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        file.lock()?;
        Ok(Self {
            path,
            seq,
            rand,
            _file: file,
        })
    }

    fn all(dir: &Path) -> Vec<(u64, u64, PathBuf)> {
        fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name();
                let (seq, rand) = name
                    .to_str()?
                    .strip_prefix("w-")?
                    .strip_suffix(".lock")?
                    .split_once('-')?;
                Some((seq.parse().ok()?, rand.parse().ok()?, entry.path()))
            })
            .collect()
    }

    /// The live waiters ahead of this one; dead tickets are removed.
    fn ahead(&self, dir: &Path) -> usize {
        Self::all(dir)
            .into_iter()
            .filter(|(seq, rand, _)| (*seq, *rand) < (self.seq, self.rand))
            .filter(|(_, _, path)| {
                let live = File::open(path)
                    .ok()
                    .is_some_and(|file| !try_lock(&file).unwrap_or(true));
                if !live {
                    let _ = fs::remove_file(path);
                }
                live
            })
            .count()
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Writes and locks this process's holder file. It is written and locked
/// under a temporary name and renamed into place, so no scan ever sees a
/// holder file that is not yet locked (and would take it for a dead one).
fn hold(dir: &Path, request: &Request, bytes: u64) -> io::Result<Lease> {
    let name = format!(
        "h-{}-{:016x}.json",
        std::process::id(),
        crate::run_queue::random_u64()
    );
    let temp = dir.join(format!(".{name}"));
    let file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&temp)?;
    file.lock()?;
    let mut lease = Lease {
        path: temp,
        file,
        record: Record {
            face: request.face,
            label: request.label.clone(),
            bytes,
            pid: request.pid,
            release: request.release.clone(),
            slot: request.slot,
            cap: request.cap,
            sized: request.sized,
            owner: std::process::id(),
            since: unix_secs(SystemTime::now()),
        },
        dir: dir.to_path_buf(),
        left: false,
    };
    lease.write();
    let path = dir.join(name);
    fs::rename(&lease.path, &path)?;
    lease.path = path;
    if lease.record.slot {
        register(&lease.path);
    }
    Ok(lease)
}

/// Releases the resident behind `path`: SIGTERM to its pid while its busy
/// lock is held by this process, so a request reaching it meanwhile waits
/// on the lock and dies with it (its client falls back). `false` when the
/// holder turned busy or its record is unreadable.
fn release_holder(path: &Path) -> bool {
    let Some(release) = fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Record>(&bytes).ok())
        .and_then(|record| record.release)
    else {
        return false;
    };
    let Some(_busy) = File::open(&release.busy)
        .ok()
        .filter(|busy| try_lock(busy).unwrap_or(false))
    else {
        return false;
    };
    let Ok(pid) = libc::pid_t::try_from(release.pid) else {
        return false;
    };
    if pid <= 1 || !crate::proc::alive(release.pid) {
        return false;
    }
    // SAFETY: SIGTERM to the pid a live holder recorded for itself; its
    // handler shuts its own tree down.
    unsafe { libc::kill(pid, libc::SIGTERM) };
    true
}

fn gib(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / f64::from(1u32 << 30))
}

/// How long ago `since` (seconds since the epoch) was, as `1h02m`, `4m05s`
/// or `12s`.
fn age(since: u64) -> String {
    let secs = unix_secs(SystemTime::now()).saturating_sub(since);
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m{:02}s", secs / 60, secs % 60),
        _ => format!("{}h{:02}m", secs / 3600, secs % 3600 / 60),
    }
}

/// The holders for a note: face, worktree, pid, how long it has run and
/// what it counts, largest first.
fn describe(holders: &[Holder]) -> String {
    if holders.is_empty() {
        return "nothing dartq holds; the host itself is short".into();
    }
    let mut sorted: Vec<&Holder> = holders.iter().collect();
    sorted.sort_by_key(|holder| std::cmp::Reverse(holder.counted()));
    sorted
        .iter()
        .map(|holder| {
            format!(
                "{} {} (pid {}, for {}, {})",
                holder.face.noun(),
                holder.label,
                holder.owner,
                age(holder.since),
                gib(holder.counted())
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// Idle-pressure eviction (G1): one shared policy every resident face uses —
// the session daemon and every analyzer-host daemon run the same monitor so
// a memory-tight host sheds idle heavy holders before the machine freezes.
// ---------------------------------------------------------------------------

/// How often an idle resident samples host memory — a caller's event-loop
/// wake bound (the daemon's recv timeout takes `min(PRESSURE_TICK, …)` so
/// the monitor never sleeps past its cadence).
pub const PRESSURE_TICK: Duration = Duration::from_secs(2);
const PRESSURE_SAMPLE: Duration = PRESSURE_TICK;
/// Two consecutive low samples arm one eviction — a single blip never does.
const PRESSURE_LOW_SAMPLES: u8 = 2;
/// The machine-wide spacing between two evictions (the ledger's file, not
/// one resident's clock — five daemons may not shed five holders at once).
const PRESSURE_COOLDOWN: Duration = Duration::from_secs(10);
/// An idle resident is evictable only after this much silence — an
/// eviction must never cut a daemon whose next request is mid-flight.
const PRESSURE_UNUSED: Duration = Duration::from_secs(10);
/// A macOS pressure level that already counts as low regardless of the
/// free figure: 2 = warn, 4 = critical.
#[cfg(target_os = "macos")]
const PRESSURE_LEVEL_LOW: u32 = 2;

/// One resident's pressure-monitor state (per daemon, cheap to keep).
#[derive(Default)]
pub struct Pressure {
    next_sample: Option<Instant>,
    lows: u8,
}

impl Pressure {
    /// Clears the arm when this resident went busy — a daemon mid-request
    /// neither evicts nor is evicted.
    pub fn reset(&mut self) {
        self.next_sample = None;
        self.lows = 0;
    }

    /// The monitor's one tick. On cadence ([`PRESSURE_SAMPLE`]) reads the
    /// host's pressure; on [`PRESSURE_LOW_SAMPLES`] consecutive lows under
    /// the cooldown picks the least-recently-used idle releasable ledger
    /// holder that is not `own_pid` and releases it (SIGTERM — the holder's
    /// own shutdown ladder). Returns the released holder's label for the
    /// caller's note/log, `None` while idle or under cooldown.
    ///
    /// `max_percent` is the resident's configured `memory.max_percent`.
    /// `self_evictable` marks the caller's own state: an analyzer daemon
    /// in its idle loop, or a session daemon hosting no client and not
    /// busy — pass `true` only then. Under CRITICAL pressure (free below
    /// [`critical_floor`], or macOS pressure level at 4) with no other
    /// victim, a self-evictable idle resident releases itself through the
    /// same graceful path rather than keep the last idle resident warm.
    pub fn tick(&mut self, own_pid: u32, max_percent: u8, self_evictable: bool) -> Option<String> {
        let now = Instant::now();
        let due = self.next_sample.get_or_insert(now + PRESSURE_SAMPLE);
        if now < *due {
            return None;
        }
        *due = now + PRESSURE_SAMPLE;
        if !memory_low(max_percent) {
            self.lows = 0;
            return None;
        }
        self.lows = self.lows.saturating_add(1);
        if self.lows < PRESSURE_LOW_SAMPLES {
            return None;
        }
        let dir = ledger_dir()?;
        // The cooldown is machine-wide: any resident's last eviction paces
        // every other one's next.
        if under_cooldown(&dir) {
            return None;
        }
        let (path, holder) = pick_victim(&dir, own_pid, self_evictable, critical())?;
        self.lows = 0;
        let _ = fs::write(
            cooldown_file(&dir),
            unix_secs(SystemTime::now()).to_string(),
        );
        if !release_holder(&path) {
            return None;
        }
        Some(format!("{} {}", holder.face.noun(), holder.label))
    }
}

/// Whether the host is under the ledger's memory-pressure line: the free
/// figure under the reserve (`available < reserve`) or, on macOS, the
/// kernel's own pressure level at warn or worse — the compressor and cache
/// state a free-bytes figure misses. An unreadable figure is never low.
fn memory_low(max_percent: u8) -> bool {
    let total = total_memory();
    if total == 0 {
        return false;
    }
    let reserve = Figures {
        total,
        available: None,
        max_percent,
    }
    .reserve();
    if let Some(available) = available_memory() {
        if available < reserve {
            return true;
        }
    }
    #[cfg(target_os = "macos")]
    if let Some(level) = pressure_level()
        && level >= PRESSURE_LEVEL_LOW
    {
        return true;
    }
    false
}

/// Whether the host is at the pressure monitor's critical line: free
/// memory under [`critical_floor`], or the macOS kernel pressure level at
/// critical (4) — the point where the last idle resident sheds itself.
fn critical() -> bool {
    let total = total_memory();
    if total == 0 {
        return false;
    }
    let floor = critical_floor(total);
    if let Some(available) = available_memory()
        && available < floor
    {
        return true;
    }
    #[cfg(target_os = "macos")]
    if let Some(level) = pressure_level()
        && level >= 4
    {
        return true;
    }
    false
}

/// The machine-wide cooldown file's path inside the ledger dir.
fn cooldown_file(dir: &Path) -> PathBuf {
    dir.join("pressure-eviction.ts")
}

fn under_cooldown(dir: &Path) -> bool {
    let Ok(text) = fs::read_to_string(cooldown_file(dir)) else {
        return false;
    };
    let Ok(at) = text.trim().parse::<u64>() else {
        return false;
    };
    let now = unix_secs(SystemTime::now());
    now.saturating_sub(at) < PRESSURE_COOLDOWN.as_secs()
}

// ---------------------------------------------------------------------------
// The active-growth brake (G2): admission protects the machine at the gate;
// a running job whose growth drives the host under the critical floor is the
// other half. While a resident's analysis runs it samples every second;
// `BRAKE_LOW_SAMPLES` consecutive samples under `critical_floor` with this
// holder the youngest active heavy holder yields the job: the daemon kills
// its server, books the observed peak, and the client re-queues.
// ---------------------------------------------------------------------------

/// The brake's critical floor: `max(1 GiB, 4% of physical)` — the level at
/// which a Mac starts swap-thrashing the desktop, not the ledger's
/// accounting reserve. Below it, the youngest heavy job stops growing.
#[must_use]
pub fn critical_floor(total: u64) -> u64 {
    (total / 25).max(1 << 30)
}

/// Consecutive one-second low samples that arm the brake.
const BRAKE_LOW_SAMPLES: u8 = 3;
/// The machine-wide spacing between two brakes — one job per incident.
const BRAKE_COOLDOWN: Duration = Duration::from_secs(10);

/// One resident's growth-brake state. Cheap: three counters the sample
/// loop flips.
#[derive(Default)]
pub struct GrowthBrake {
    lows: u8,
}

impl GrowthBrake {
    /// Clears the arm — a finished analysis leaves no debt behind.
    pub fn reset(&mut self) {
        self.lows = 0;
    }

    /// The one-second sample: `true` when the brake fires — the host sat
    /// under [`critical_floor`] for [`BRAKE_LOW_SAMPLES`] consecutive
    /// samples, `own_pid` owns the youngest active heavy holder in the
    /// ledger, and no brake fired inside [`BRAKE_COOLDOWN`]. A sample the
    /// OS cannot read is never low (fail-open: the budget path still
    /// bounds the tree's own RSS).
    pub fn check(&mut self, own_pid: u32) -> bool {
        let total = total_memory();
        let floor = critical_floor(total);
        let low = total > 0 && available_memory().is_some_and(|available| available < floor);
        if !low {
            self.lows = 0;
            return false;
        }
        self.lows = self.lows.saturating_add(1);
        if self.lows < BRAKE_LOW_SAMPLES {
            return false;
        }
        let Some(dir) = ledger_dir() else {
            return false;
        };
        if under_brake_cooldown(&dir) {
            return false;
        }
        // The youngest active heavy holder brakes — the oldest one
        // closest to done keeps running; a job not running (an idle
        // releasable) is the pressure monitor's victim, not the brake's.
        let youngest = scan(&dir)
            .into_iter()
            .filter(|(_, holder)| holder.slot && holder.idle != Some(true))
            .max_by_key(|(_, holder)| holder.since);
        let Some((_, holder)) = youngest else {
            return false;
        };
        if holder.owner != own_pid {
            self.lows = BRAKE_LOW_SAMPLES - 1;
            return false;
        }
        let _ = fs::write(
            dir.join("growth-brake.ts"),
            unix_secs(SystemTime::now()).to_string(),
        );
        self.lows = 0;
        true
    }
}

fn under_brake_cooldown(dir: &Path) -> bool {
    let Ok(text) = fs::read_to_string(dir.join("growth-brake.ts")) else {
        return false;
    };
    let Ok(at) = text.trim().parse::<u64>() else {
        return false;
    };
    let now = unix_secs(SystemTime::now());
    now.saturating_sub(at) < BRAKE_COOLDOWN.as_secs()
}

/// The monitor's victim: the least-recently-used idle releasable that is
/// not `own_pid`, else — only under `critical` and only when the caller
/// declared itself releasable (`self_evictable`: an analyzer daemon in
/// its idle loop, a session daemon hosting nothing and not busy) — the
/// caller's own idle record, so the last idle resident sheds itself at
/// the critical line rather than stay warm. `None` keeps the resident.
fn pick_victim(
    dir: &Path,
    own_pid: u32,
    self_evictable: bool,
    critical: bool,
) -> Option<(PathBuf, Holder)> {
    if let Some(victim) = evictable_idle(dir, own_pid).into_iter().next() {
        return Some(victim);
    }
    if !(self_evictable && critical) {
        return None;
    }
    scan(dir)
        .into_iter()
        .find(|(_, holder)| holder.owner == own_pid && holder.idle == Some(true))
}

/// The idle releasable ledger holders, least recently used first: a live
/// record, not `own_pid`, releasable with its busy lock free, quiet for
/// [`PRESSURE_UNUSED`] on its lease's last-use mtime.
fn evictable_idle(dir: &Path, own_pid: u32) -> Vec<(PathBuf, Holder)> {
    let mut holders: Vec<(PathBuf, Holder)> = scan(dir)
        .into_iter()
        .filter(|(_, holder)| {
            holder.owner != own_pid
                && holder.idle == Some(true)
                && unix_secs(SystemTime::now()).saturating_sub(unix_secs(holder.used))
                    >= PRESSURE_UNUSED.as_secs()
        })
        .collect();
    holders.sort_by_key(|(_, holder)| holder.used);
    holders
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    const GIB: u64 = 1 << 30;

    /// A host with room for anything the tests ask: only the gate's count
    /// rules decide.
    fn roomy() -> Figures {
        Figures {
            total: 100 * GIB,
            available: None,
            max_percent: 90,
        }
    }

    fn slot(face: Face, label: &str, cap: u32, sized: bool) -> Request {
        Request {
            face,
            label: label.into(),
            bytes: 1 << 20,
            pid: None,
            release: None,
            slot: true,
            cap,
            sized,
            peak_key: None,
            source_bytes: None,
        }
    }

    /// One admission in `dir`, waiting at most `wait`; the notes it wrote.
    fn admit(
        dir: &Path,
        request: Request,
        wait: Duration,
    ) -> (Result<Option<Lease>, Error>, Vec<String>) {
        let mut notes = Vec::new();
        let min = request.bytes;
        let result = acquire_in(
            dir,
            request,
            min,
            Instant::now() + wait,
            &|| false,
            |note| notes.push(note.to_owned()),
            roomy,
        );
        (result, notes)
    }

    /// An unsized slot request of `face` that re-reads `key`'s sizing
    /// while it waits.
    fn first(face: Face, label: &str, key: &str) -> Request {
        Request {
            peak_key: Some(key.into()),
            ..slot(face, label, 0, false)
        }
    }

    fn held(dir: &Path, request: Request) -> Lease {
        admit(dir, request, Duration::from_secs(5))
            .0
            .expect("admitted")
            .expect("a lease")
    }

    fn holder(lease: u64, used: u64, idle: Option<bool>) -> Holder {
        Holder {
            face: Face::AnalysisServer,
            label: "tree".into(),
            lease,
            rss: 0,
            used: UNIX_EPOCH + Duration::from_secs(used),
            idle,
            slot: false,
            cap: 0,
            sized: true,
            owner: 1,
            since: 0,
        }
    }

    fn ask(min: u64, slot: bool, cap: u32, sized: bool) -> Ask {
        Ask {
            min,
            slot,
            cap,
            sized,
        }
    }

    #[test]
    fn a_memory_shortfall_releases_idle_residents_least_recently_used_first() {
        let figures = Figures {
            total: 10 * GIB,
            available: None,
            max_percent: 90,
        };
        // 9 GiB budget: 3 + 3 + 2 held, 3 wanted; one release suffices.
        let holders = [
            holder(3 * GIB, 20, Some(true)),
            holder(3 * GIB, 10, Some(true)),
            holder(2 * GIB, 5, None),
        ];
        match decide(&holders, ask(3 * GIB, false, 0, true), figures) {
            Decision::Admit { release, .. } => assert_eq!(release, [1]),
            other => panic!("expected an admission, got {other:?}"),
        }
        // A busy resident is never released.
        let busy = [holder(8 * GIB, 0, Some(false))];
        assert_eq!(
            decide(&busy, ask(2 * GIB, false, 0, true), figures),
            Decision::Wait(Shortfall::Memory)
        );
    }

    #[test]
    fn the_available_figure_bounds_the_headroom() {
        let figures = Figures {
            total: 32 * GIB,
            available: Some(4 * GIB),
            max_percent: 90,
        };
        // Available 4 GiB minus the 3.2 GiB reserve leaves 0.8 GiB.
        let headroom = 4 * GIB - 32 * GIB / 10;
        assert_eq!(
            decide(&[], ask(headroom, false, 0, true), figures),
            Decision::Admit {
                release: vec![],
                headroom
            }
        );
        assert_eq!(
            decide(&[], ask(headroom + 1, false, 0, true), figures),
            Decision::Wait(Shortfall::Memory)
        );
    }

    /// The reserve keeps a floor of [`RESERVE_FLOOR`]: `90%` of a small
    /// machine would leave under 2 GiB for everything outside the ledger,
    /// which is how a host freezes. On a big machine the percent is
    /// larger and wins.
    #[test]
    fn the_reserve_never_drops_below_its_floor() {
        let at = |total: u64, max_percent: u8| Figures {
            total,
            available: None,
            max_percent,
        };
        // 16 GB at 90% → percent would be 1.6 GiB; the floor binds.
        assert_eq!(at(16 * GIB, 90).reserve(), RESERVE_FLOOR);
        // 24 GB at 90% → 2.4 GiB percent beats the floor.
        assert_eq!(at(24 * GIB, 90).reserve(), 24 * GIB / 10);
        // 32 GB at 95% → 1.6 GiB → floor.
        assert_eq!(at(32 * GIB, 95).reserve(), RESERVE_FLOOR);
        // The budget still follows max_percent.
        assert_eq!(at(24 * GIB, 90).budget(), 24 * GIB * 9 / 10);
    }

    #[test]
    fn a_count_shortfall_releases_only_slot_holders() {
        let mut booking = holder(GIB, 0, Some(true));
        booking.face = Face::Debug;
        let mut resident = holder(GIB, 5, Some(true));
        resident.slot = true;
        resident.cap = 1;
        match decide(&[booking, resident], ask(GIB, true, 1, true), roomy()) {
            Decision::Admit { release, .. } => assert_eq!(release, [1]),
            other => panic!("expected an admission, got {other:?}"),
        }
    }

    #[test]
    fn cap_one_blocks_a_job_of_another_kind_until_the_first_exits() {
        let dir = tempfile::tempdir().expect("ledger dir");
        let first = held(dir.path(), slot(Face::Analyze, "tree-a", 1, true));
        let (sender, receiver) = mpsc::channel();
        let path = dir.path().to_path_buf();
        let waiter = std::thread::spawn(move || {
            let outcome = admit(
                &path,
                slot(Face::Test, "tree-b", 1, true),
                Duration::from_secs(10),
            );
            sender.send(()).expect("send");
            outcome
        });
        assert!(
            receiver.recv_timeout(Duration::from_millis(800)).is_err(),
            "the test run waits while the analyze run holds the only slot"
        );
        drop(first);
        receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("admitted once the analyze run exited");
        let (result, notes) = waiter.join().expect("waiter");
        assert!(result.expect("admitted").is_some());
        assert!(
            notes[0].starts_with(
                "dartq: waiting for a job slot (1 of 1 running); held by analyze run tree-a (pid "
            ),
            "{notes:?}"
        );
        assert_eq!(notes.last().map(String::as_str), Some("dartq: admitted"));
    }

    #[test]
    fn cap_one_releases_an_idle_resident_of_another_worktree_before_a_test() {
        let dir = tempfile::tempdir().expect("ledger dir");
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("sleep");
        let busy = dir.path().join("busy.lock");
        File::create(&busy).expect("busy lock");
        let resident = held(
            dir.path(),
            Request {
                pid: Some(child.id()),
                release: Some(Releasable {
                    pid: child.id(),
                    busy,
                }),
                ..slot(Face::AnalysisServer, "other-tree", 1, true)
            },
        );
        let (result, notes) = admit(
            dir.path(),
            slot(Face::Test, "tree", 1, true),
            Duration::from_secs(5),
        );
        assert!(result.expect("admitted").is_some());
        assert!(
            notes.contains(
                &"dartq: released the idle resident analyzer of other-tree to make room".to_owned()
            ),
            "{notes:?}"
        );
        let status = child.wait().expect("the resident exits");
        assert!(!status.success(), "the resident was signalled");
        drop(resident);
    }

    #[test]
    fn the_strictest_cap_wins() {
        let dir = tempfile::tempdir().expect("ledger dir");
        let strict = held(dir.path(), slot(Face::Analyze, "tree-a", 1, true));
        let (result, _) = admit(
            dir.path(),
            slot(Face::Test, "tree-b", 5, true),
            Duration::from_millis(600),
        );
        let error = result.expect_err("cap 1 of the holder binds the requester");
        assert!(
            error.to_string().starts_with(
                "timed out waiting for a job slot held by other dartq runs: analyze run tree-a"
            ),
            "{error}"
        );
        drop(strict);
        let _loose = held(dir.path(), slot(Face::Analyze, "tree-a", 5, true));
        let (result, _) = admit(
            dir.path(),
            slot(Face::Test, "tree-b", 1, true),
            Duration::from_millis(600),
        );
        assert!(result.is_err(), "the requester's own cap 1 binds too");
    }

    #[test]
    fn unsized_requests_run_one_at_a_time() {
        let dir = tempfile::tempdir().expect("ledger dir");
        let _first = held(dir.path(), slot(Face::Test, "tree-a", 0, false));
        let (result, _) = admit(
            dir.path(),
            slot(Face::Build, "tree-b", 0, false),
            Duration::from_millis(600),
        );
        match result {
            Err(Error::Timeout { what, .. }) => assert_eq!(what, "the first-run slot"),
            other => panic!("expected a timeout, got {other:?}"),
        }
        let (result, _) = admit(
            dir.path(),
            slot(Face::Build, "tree-b", 0, true),
            Duration::from_secs(5),
        );
        assert!(result.expect("a sized job runs beside it").is_some());
    }

    /// A releasable resident of `face` that never measured a peak: an
    /// unsized slot holder, its pid a live `sleep` (the child to signal if
    /// it were released), and its busy lock file — held by the returned
    /// handle (a first run under way) unless `idle`.
    fn unsized_resident(
        dir: &Path,
        face: Face,
        label: &str,
        idle: bool,
    ) -> (Lease, std::process::Child, File) {
        let child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("sleep");
        let busy = dir.join(format!("{label}.busy"));
        let lock = File::create(&busy).expect("busy lock");
        if !idle {
            lock.lock().expect("hold the busy lock");
        }
        let lease = held(
            dir,
            Request {
                pid: Some(child.id()),
                release: Some(Releasable {
                    pid: child.id(),
                    busy,
                }),
                ..slot(face, label, 0, false)
            },
        );
        (lease, child, lock)
    }

    /// The first-run slot only waits: a busy unsized resident is a first
    /// run under way, never released to make room for another one.
    #[test]
    fn a_first_run_wait_never_releases_a_resident() {
        let dir = tempfile::tempdir().expect("ledger dir");
        let (resident, mut child, _busy) =
            unsized_resident(dir.path(), Face::AnalysisServer, "wt1", false);
        let (result, notes) = admit(
            dir.path(),
            slot(Face::Analyze, "wt2", 0, false),
            Duration::from_millis(600),
        );
        match result {
            Err(Error::Timeout { what, .. }) => assert_eq!(what, "the first-run slot"),
            other => panic!("expected a first-run wait, got {other:?}"),
        }
        assert!(
            !notes.iter().any(|note| note.contains("released")),
            "no one is released for the first-run slot: {notes:?}"
        );
        assert!(
            child.try_wait().expect("child").is_none(),
            "the resident lives"
        );
        drop(resident);
        let _ = child.kill();
        let _ = child.wait();
    }

    /// An idle unsized resident (no peak, busy lock free) has no first run
    /// under way: another first run is admitted beside it at once, and the
    /// resident is not released.
    #[test]
    fn an_idle_unsized_resident_leaves_the_first_run_slot_free() {
        let dir = tempfile::tempdir().expect("ledger dir");
        let (resident, mut child, _busy) =
            unsized_resident(dir.path(), Face::AnalysisServer, "wt1", true);
        let started = Instant::now();
        let (result, notes) = admit(
            dir.path(),
            slot(Face::Analyze, "wt2", 0, false),
            Duration::from_secs(5),
        );
        let lease = result.expect("admitted").expect("a lease");
        assert!(!lease.sized(), "admitted as a first run");
        assert!(started.elapsed() < Duration::from_secs(1), "{notes:?}");
        assert!(notes.is_empty(), "no wait and no release: {notes:?}");
        assert!(
            child.try_wait().expect("child").is_none(),
            "the resident lives"
        );
        drop(lease);
        drop(resident);
        let _ = child.kill();
        let _ = child.wait();
    }

    /// Three worktrees' first runs at once under the default config: the
    /// second and third wait (no one is released) and are admitted as soon
    /// as the first resident records its peak; every resident stays alive
    /// and sized, and the waiters borrow the recorded peak.
    #[test]
    fn concurrent_first_runs_keep_every_resident_alive() {
        let dir = tempfile::tempdir().expect("ledger dir");
        let (mut resident, mut child, busy) =
            unsized_resident(dir.path(), Face::AnalysisServer, "wt1", false);
        let waiter = |label: &'static str| {
            let dir = dir.path().to_path_buf();
            std::thread::spawn(move || {
                let started = Instant::now();
                let (result, notes) = admit(
                    &dir,
                    first(Face::AnalysisServer, label, label),
                    Duration::from_secs(10),
                );
                (result, notes, started.elapsed())
            })
        };
        let second = waiter("wt2");
        let third = waiter("wt3");
        std::thread::sleep(Duration::from_millis(700));
        assert!(
            !second.is_finished() && !third.is_finished(),
            "first runs wait their turn"
        );
        resident.record_peak("wt1", 3 * GIB);
        assert!(resident.sized() && resident.holds_slot());
        drop(busy);
        for handle in [second, third] {
            let (result, notes, waited) = handle.join().expect("waiter");
            let lease = result.expect("admitted").expect("a lease");
            assert!(lease.sized(), "the waiter borrowed the recorded peak");
            assert_eq!(lease.bytes(), 3 * GIB);
            assert!(
                waited < Duration::from_secs(5),
                "admitted once the peak landed"
            );
            assert!(
                !notes.iter().any(|note| note.contains("released")),
                "no resident is released: {notes:?}"
            );
            drop(lease);
        }
        assert!(
            child.try_wait().expect("child").is_none(),
            "the first resident lives"
        );
        drop(resident);
        let _ = child.kill();
        let _ = child.wait();
    }

    /// A waiting first run re-reads its sizing on every poll: a peer peak
    /// of its face recorded during the wait admits it, sized with the
    /// peer's figure; its own key's recorded peak does the same.
    #[test]
    fn a_waiting_request_borrows_a_peak_recorded_during_its_wait() {
        let dir = tempfile::tempdir().expect("ledger dir");
        let mut peer = held(dir.path(), slot(Face::Session, "wt1", 0, false));
        let dir_path = dir.path().to_path_buf();
        let waiting = std::thread::spawn(move || {
            admit(
                &dir_path,
                first(Face::Session, "wt2", "session:wt2"),
                Duration::from_secs(10),
            )
        });
        std::thread::sleep(Duration::from_millis(600));
        assert!(!waiting.is_finished(), "the first-run slot is taken");
        peer.record_peak("session:wt1", 2 * GIB);
        let (result, _) = waiting.join().expect("waiter");
        let lease = result.expect("admitted").expect("a lease");
        assert!(lease.sized());
        assert_eq!(lease.bytes(), 2 * GIB, "the peer's peak is borrowed");
        drop(lease);
        drop(peer);
        // Its own key's peak, recorded while it waits, sizes it the same.
        let _blocker = held(dir.path(), slot(Face::Test, "other", 0, false));
        let dir_path = dir.path().to_path_buf();
        let waiting = std::thread::spawn(move || {
            admit(
                &dir_path,
                first(Face::Analyze, "wt3", "analyze:wt3"),
                Duration::from_secs(10),
            )
        });
        std::thread::sleep(Duration::from_millis(600));
        assert!(!waiting.is_finished(), "the first-run slot is taken");
        record_peak_in(dir.path(), "analyze:wt3", Face::Analyze, GIB / 2, None);
        let (result, _) = waiting.join().expect("waiter");
        let lease = result.expect("admitted").expect("a lease");
        assert!(lease.sized());
        assert_eq!(lease.bytes(), GIB / 2, "its own recorded peak");
    }

    /// With no live peer, a peak recorded under another key sizes a
    /// request of the same face (the largest such), and only of that face.
    #[test]
    fn a_recorded_peak_of_the_face_under_another_key_sizes_a_request() {
        let dir = tempfile::tempdir().expect("ledger dir");
        record_peak_in(dir.path(), "session:wt1", Face::Session, 2 * GIB, None);
        record_peak_in(dir.path(), "session:wt2", Face::Session, 3 * GIB, None);
        record_peak_in(dir.path(), "build:wt1", Face::Build, 7 * GIB, None);
        assert_eq!(
            sized_in(dir.path(), Some("session:wt3"), Face::Session, &[], None),
            Some(3 * GIB),
            "the largest recorded peak of the face"
        );
        assert_eq!(
            sized_in(dir.path(), Some("test:wt3"), Face::Test, &[], None),
            None
        );
        let lease = held(dir.path(), first(Face::Session, "wt3", "session:wt3"));
        assert!(lease.sized(), "admitted sized");
        assert_eq!(lease.bytes(), 3 * GIB);
    }

    /// Recording a peak sizes the holder's own record at once, keeping the
    /// larger of its lease and the peak.
    #[test]
    fn recording_a_peak_sizes_the_lease_record() {
        let dir = tempfile::tempdir().expect("ledger dir");
        let mut lease = held(dir.path(), slot(Face::AnalysisServer, "wt", 0, false));
        assert!(!lease.sized());
        lease.record_peak("wt", 1 << 10);
        let record: Record =
            serde_json::from_slice(&fs::read(&lease.path).expect("record")).expect("json");
        assert!(record.sized, "the record is sized");
        assert_eq!(record.bytes, 1 << 20, "max(lease, peak)");
        assert_eq!(peak_in(dir.path(), "wt"), Some(1 << 10));
        lease.record_peak("wt", 3 << 20);
        let record: Record =
            serde_json::from_slice(&fs::read(&lease.path).expect("record")).expect("json");
        assert_eq!(record.bytes, 3 << 20, "a larger peak grows the booking");
    }

    #[test]
    fn a_debug_booking_never_holds_a_slot() {
        let dir = tempfile::tempdir().expect("ledger dir");
        let booking = hold(
            dir.path(),
            &Request {
                slot: false,
                ..slot(Face::Debug, "app", 1, true)
            },
            GIB,
        )
        .expect("booked");
        assert!(!booking.holds_slot());
        let started = Instant::now();
        let (result, notes) = admit(
            dir.path(),
            slot(Face::Test, "tree", 1, true),
            Duration::from_secs(5),
        );
        assert!(result.expect("admitted").is_some());
        assert!(started.elapsed() < Duration::from_secs(1), "{notes:?}");
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn a_slot_left_for_debugging_is_retaken_only_when_free() {
        let dir = tempfile::tempdir().expect("ledger dir");
        let mut daemon = held(dir.path(), slot(Face::Session, "tree-a", 1, true));
        daemon.leave_slot();
        assert!(!daemon.holds_slot());
        let test = held(dir.path(), slot(Face::Test, "tree-b", 1, true));
        assert!(!daemon.retake_slot().expect("gate"), "no slot is free");
        assert!(!daemon.holds_slot());
        drop(test);
        assert!(daemon.retake_slot().expect("gate"));
        assert!(daemon.holds_slot());
        // A memory-only lease left no slot: there is nothing to retake.
        let mut booking = held(
            dir.path(),
            Request {
                slot: false,
                ..slot(Face::Session, "tree-c", 1, true)
            },
        );
        assert!(booking.retake_slot().expect("gate"));
        assert!(!booking.holds_slot());
    }

    #[test]
    fn a_request_past_the_budget_runs_alone_with_the_whole_budget() {
        let dir = tempfile::tempdir().expect("ledger dir");
        let request = Request {
            bytes: 200 * GIB,
            ..slot(Face::Build, "tree", 0, true)
        };
        let lease = acquire_in(
            dir.path(),
            request,
            200 * GIB,
            Instant::now() + Duration::from_secs(5),
            &|| false,
            |_| {},
            roomy,
        )
        .expect("admitted")
        .expect("a lease");
        assert_eq!(lease.bytes(), 90 * GIB);
    }

    #[test]
    fn a_dead_holder_is_reclaimed_and_reported() {
        let dir = tempfile::tempdir().expect("ledger dir");
        let record = Record {
            face: Face::Test,
            label: "gone".into(),
            bytes: 95 * GIB,
            pid: None,
            release: None,
            slot: true,
            cap: 1,
            sized: true,
            owner: 1,
            since: 1_000,
        };
        let path = dir.path().join("h-1-0000000000000000.json");
        fs::write(&path, serde_json::to_vec(&record).expect("json")).expect("holder");
        let _ = take_dead();
        let (result, notes) = admit(
            dir.path(),
            slot(Face::Test, "tree", 1, true),
            Duration::from_secs(5),
        );
        assert!(result.expect("admitted").is_some(), "{notes:?}");
        assert!(!path.exists(), "the unlocked holder file is removed");
        assert_eq!(take_dead(), Some(UNIX_EPOCH + Duration::from_secs(1_000)));
    }

    #[test]
    fn waiters_are_served_first_come_first_served() {
        let dir = tempfile::tempdir().expect("ledger dir");
        let first = Ticket::join(dir.path()).expect("first");
        let second = Ticket::join(dir.path()).expect("second");
        assert_eq!(first.ahead(dir.path()), 0);
        assert_eq!(second.ahead(dir.path()), 1);
        drop(first);
        assert_eq!(second.ahead(dir.path()), 0);
        // An unlocked ticket is a dead waiter's: skipped and removed.
        let dead = dir.path().join("w-0-1.lock");
        File::create(&dead).expect("dead ticket");
        assert_eq!(second.ahead(dir.path()), 0);
        assert!(!dead.exists());
    }

    #[test]
    fn a_nested_request_joins_its_holders_slot() {
        let dir = tempfile::tempdir().expect("ledger dir");
        let outer = held(dir.path(), slot(Face::Test, "tree", 1, true));
        let token = token_of(&outer.path).expect("token");
        // SAFETY: the only test of this binary that reads or writes the
        // token variable.
        unsafe { std::env::set_var(TOKEN_ENV, &token) };
        assert_eq!(env_token(dir.path()), Some(token));
        // Under cap 1 a nested request never takes a second slot: a child
        // process (holding no slot) joins through the live token checked
        // above; this process holds `outer`, so its own request books
        // memory only and is admitted at once.
        let mut request = slot(Face::Analyze, "tree", 1, true);
        if !joins(&mut request, dir.path()) {
            assert!(
                !request.slot,
                "a slot holder's own request books memory only"
            );
            let (result, _) = admit(dir.path(), request, Duration::from_millis(600));
            assert!(
                result
                    .expect("admitted at once")
                    .is_some_and(|lease| !lease.holds_slot())
            );
        }
        drop(outer);
        assert_eq!(
            env_token(dir.path()),
            None,
            "a dead holder's token is ignored"
        );
        unsafe { std::env::remove_var(TOKEN_ENV) };
    }

    /// A holder that left its slot (a session daemon hosting a debug
    /// session) shelters nothing: under cap 1 a request carrying its token
    /// takes a slot of its own at once, and the holder cannot retake its
    /// slot until that run ends. The slots this process holds are
    /// process-wide state, so the body runs alone in a child process.
    #[test]
    fn a_holder_that_left_its_slot_shelters_no_nested_request() {
        const ISOLATED_ENV: &str = "DARTQ_TEST_ISOLATED";
        let name = "ledger::tests::a_holder_that_left_its_slot_shelters_no_nested_request";
        if std::env::var_os(ISOLATED_ENV).is_none() {
            let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
                .args([name, "--exact", "--test-threads", "1"])
                .env(ISOLATED_ENV, "1")
                .env_remove(TOKEN_ENV)
                .output()
                .expect("run the isolated test");
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(
                output.status.success() && stdout.contains("1 passed"),
                "isolated test {name} failed ({}):\n{stdout}\n{}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let dir = tempfile::tempdir().expect("ledger dir");
        let mut daemon = held(dir.path(), slot(Face::Session, "tree", 1, true));
        let token = token_of(&daemon.path).expect("token");
        // SAFETY: this child process runs this test alone.
        unsafe { std::env::set_var(TOKEN_ENV, &token) };
        assert_eq!(env_token(dir.path()).as_deref(), Some(token.as_str()));
        daemon.leave_slot();
        assert_eq!(
            env_token(dir.path()),
            None,
            "a slotless holder shelters nothing"
        );
        let mut request = slot(Face::Test, "tree", 1, true);
        assert!(!joins(&mut request, dir.path()), "nothing to join");
        assert!(request.slot, "the nested request still asks for a slot");
        let (result, notes) = admit(dir.path(), request, Duration::from_millis(600));
        let run = result.expect("admitted at once").expect("a lease");
        assert!(
            run.holds_slot(),
            "the nested run takes the only slot: {notes:?}"
        );
        assert!(
            !daemon.retake_slot().expect("gate"),
            "the slot is the run's now"
        );
        drop(run);
        assert!(
            daemon.retake_slot().expect("gate"),
            "a free slot is retaken"
        );
        assert_eq!(
            env_token(dir.path()),
            Some(token),
            "a slot holder shelters again"
        );
    }

    /// An analyzer peak that carries a comparable source size bounds an
    /// unknown job by scale-up only — a bigger project's peak is never
    /// scaled down, a smaller one's is scaled up by at most 4×.
    #[test]
    fn a_source_sized_peak_estimates_an_unknown_job_by_ratio() {
        let dir = tempfile::tempdir().expect("ledger dir");
        record_peak_in(
            dir.path(),
            "repo:small",
            Face::AnalysisServer,
            1_200_000_000,     // 1.2 GB peak
            Some(250_000_000), // on 250 MB of Dart source
        );
        // A 500 MB Dart source job: 250 MB is inside [125 MB, 2 GB], the
        // bound scales up 2x.
        assert_eq!(
            sized_in(
                dir.path(),
                Some("repo:big"),
                Face::AnalysisServer,
                &[],
                Some(500_000_000),
            ),
            Some(2_400_000_000),
            "2x of the 1.2 GB comparable peak"
        );
        // A bigger recorded peak is never scaled down: a 3 GB peak on
        // 800 MB of source bounds the 500 MB job at 3 GB, not 1.9 GB.
        record_peak_in(
            dir.path(),
            "repo:huge",
            Face::AnalysisServer,
            3_000_000_000,
            Some(800_000_000),
        );
        assert_eq!(
            sized_in(
                dir.path(),
                Some("repo:big"),
                Face::AnalysisServer,
                &[],
                Some(500_000_000),
            ),
            Some(3_000_000_000),
            "the larger bound wins, never scaled down"
        );
        // Without source bytes the estimate is the raw peak.
        assert_eq!(
            sized_in(
                dir.path(),
                Some("repo:big"),
                Face::AnalysisServer,
                &[],
                None
            ),
            Some(3_000_000_000),
            "the recorded peak itself"
        );
    }

    /// A tiny project's ratio is dominated by the analyzer's fixed base:
    /// 900 MB on 10 KB of source must never estimate a 200 MB-source
    /// job — it is outside the comparable window, so the job is unsized
    /// (the first-run slot, never a budget-blocking over-estimate).
    #[test]
    fn a_tiny_peak_never_estimates_a_large_job() {
        let dir = tempfile::tempdir().expect("ledger dir");
        record_peak_in(
            dir.path(),
            "repo:tiny",
            Face::AnalysisServer,
            900_000_000,
            Some(10_000), // 10 KB of Dart source
        );
        assert_eq!(
            sized_in(
                dir.path(),
                Some("repo:mid"),
                Face::AnalysisServer,
                &[],
                Some(200_000_000),
            ),
            None,
            "outside the comparable window — unsized, not an over-estimate"
        );
    }

    /// An estimate the host could never admit — over `total − reserve`
    /// even alone — is unsized, not a permanent gate-blocker.
    #[test]
    fn an_unadmittable_estimate_is_unsized() {
        let dir = tempfile::tempdir().expect("ledger dir");
        // A comparable peak whose bound clears the whole machine.
        record_peak_in(
            dir.path(),
            "repo:peer",
            Face::AnalysisServer,
            u64::MAX / 2,
            Some(250_000_000),
        );
        assert_eq!(
            sized_in(
                dir.path(),
                Some("repo:mid"),
                Face::AnalysisServer,
                &[],
                Some(500_000_000),
            ),
            None,
            "an estimate over total − reserve is unsized"
        );
    }

    /// An analyzer job with known source bytes but no comparable peak is
    /// unsized: the face-peak borrow under-estimates it (the base makes a
    /// big peer's peak small per byte), so it takes the first-run slot
    /// rather than a borrowed lease. The borrow still holds for jobs
    /// whose source size is unknown.
    #[test]
    fn a_sourceless_peak_sizes_by_raw_bytes() {
        let dir = tempfile::tempdir().expect("ledger dir");
        record_peak_in(dir.path(), "repo:old", Face::AnalysisServer, 2 * GIB, None);
        // Known source, no comparable peak → unsized (no borrow).
        assert_eq!(
            sized_in(
                dir.path(),
                Some("repo:new"),
                Face::AnalysisServer,
                &[],
                Some(500_000_000),
            ),
            None,
            "known source, incomparable peak — the first-run slot"
        );
        // Unknown source → the recorded peak stands in as before.
        assert_eq!(
            sized_in(
                dir.path(),
                Some("repo:new"),
                Face::AnalysisServer,
                &[],
                None
            ),
            Some(2 * GIB),
            "the largest recorded peak"
        );
    }

    /// A sibling key's own recorded peak wins over any estimate — the
    /// measured figure is never displaced by a comparable bound.
    #[test]
    fn a_sibling_peak_wins_over_the_estimate() {
        let dir = tempfile::tempdir().expect("ledger dir");
        record_peak_in(
            dir.path(),
            "repo:/common:sibling",
            Face::AnalysisServer,
            4 * GIB,
            Some(300_000_000),
        );
        record_peak_in(
            dir.path(),
            "repo:peer",
            Face::AnalysisServer,
            9 * GIB,
            Some(300_000_000),
        );
        assert_eq!(
            sized_in(
                dir.path(),
                Some("repo:/common:sibling"),
                Face::AnalysisServer,
                &[],
                Some(300_000_000),
            ),
            Some(4 * GIB),
            "the own/sibling peak, not the peer's larger bound"
        );
    }

    /// A yield's observed peak never lowers a recorded full measurement:
    /// the brake's figure is a lower bound, so `record_peak_floor` keeps
    /// the earlier peak while a normal completion still books the latest.
    #[test]
    fn a_yield_never_lowers_a_recorded_peak() {
        let dir = tempfile::tempdir().expect("ledger dir");
        let key = "repo:yield";
        record_peak_in(dir.path(), key, Face::AnalysisServer, 3 * GIB, Some(GIB));
        // A yield observed 2.2 GB: the record stays at 3 GB.
        record_peak_floor_at(dir.path(), key, Face::AnalysisServer, 2_200_000_000, GIB);
        assert_eq!(peak_in(dir.path(), key), Some(3 * GIB));
        // A later normal completion at 1.5 GB still books less.
        record_peak_in(
            dir.path(),
            key,
            Face::AnalysisServer,
            3 * GIB / 2,
            Some(GIB),
        );
        assert_eq!(peak_in(dir.path(), key), Some(3 * GIB / 2));
        // A yield above the record books the higher bound.
        record_peak_floor_at(dir.path(), key, Face::AnalysisServer, 2 * GIB, GIB);
        assert_eq!(peak_in(dir.path(), key), Some(2 * GIB));
    }

    /// `record_peak_floor` on a ledger dir (the test twin of the
    /// env-routed [`record_peak_floor`]).
    fn record_peak_floor_at(dir: &Path, key: &str, face: Face, bytes: u64, source: u64) {
        let floor = peak_in(dir, key).unwrap_or(0).max(bytes);
        record_peak_in(dir, key, face, floor, Some(source));
    }

    /// The peaks store stays bounded: recording past [`PEAK_FILES_MAX`]
    /// drops older files so `face_peak_entries` stays cheap for the
    /// 250 ms unsized-waiter scan.
    #[test]
    fn the_peaks_store_is_capped() {
        let dir = tempfile::tempdir().expect("ledger dir");
        for i in 0..(PEAK_FILES_MAX + 30) {
            record_peak_in(
                dir.path(),
                &format!("repo:key{i:04}"),
                Face::AnalysisServer,
                GIB,
                Some(GIB),
            );
        }
        let count = fs::read_dir(dir.path().join("peaks"))
            .expect("peaks dir")
            .flatten()
            .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
            .count();
        assert_eq!(count, PEAK_FILES_MAX, "the store is capped");
    }

    /// A live holder record on disk: the file locked (the kernel's
    /// liveness), the JSON the scan reads. `owner`/`since`/`used`-mtime
    /// are injected so tests control identities the `hold()` path would
    /// stamp uniformly (one process, one second).
    fn live_record(
        dir: &Path,
        name: &str,
        owner: u32,
        since: u64,
        idle: bool,
        used: SystemTime,
    ) -> File {
        let busy = dir.join(format!("{name}.busy"));
        let busy_file = File::create(&busy).expect("busy lock");
        if !idle {
            busy_file.lock().expect("hold busy");
        }
        let path = dir.join(format!("h-{owner}-{name}.json"));
        let record = Record {
            face: Face::AnalysisServer,
            label: name.into(),
            bytes: 1 << 20,
            pid: None,
            release: Some(Releasable { pid: owner, busy }),
            slot: true,
            cap: 0,
            sized: true,
            owner,
            since,
        };
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .read(true)
            .open(&path)
            .expect("holder file");
        file.lock().expect("live holder lock");
        use std::os::unix::fs::FileExt as _;
        file.write_all_at(serde_json::to_vec(&record).expect("json").as_slice(), 0)
            .expect("record");
        file.set_modified(used).expect("mtime");
        // The busy_file must outlive the scan: return both.
        std::mem::forget(busy_file);
        file
    }

    /// The growth brake's election: only an *active* slot holder is the
    /// youngest heavy holder — an idle releasable (the pressure monitor's
    /// victim) is not, and the oldest running job never yields to the
    /// youngest's brake.
    #[test]
    fn the_brake_only_fires_for_the_youngest_active_holder() {
        let dir = tempfile::tempdir().expect("ledger dir");
        let now = SystemTime::now();
        let secs = |ago: u64| unix_secs(now) - ago;
        // Active running jobs (busy locks held): `older` then `younger`.
        // `idle` is a releasable with a free busy lock — never active.
        let _old = live_record(dir.path(), "older", 101, secs(300), false, now);
        let _idle = live_record(dir.path(), "idle", 102, secs(200), true, now);
        let _young = live_record(dir.path(), "younger", 103, secs(100), false, now);
        let active: Vec<Holder> = scan(dir.path())
            .into_iter()
            .map(|(_, holder)| holder)
            .filter(|holder| holder.slot && holder.idle != Some(true))
            .collect();
        let youngest = active
            .iter()
            .max_by_key(|holder| holder.since)
            .expect("an active holder");
        assert_eq!(youngest.label, "younger", "the youngest active holder");
        let mut labels: Vec<&str> = active.iter().map(|holder| holder.label.as_str()).collect();
        labels.sort_unstable();
        assert_eq!(
            labels,
            ["older", "younger"],
            "an idle releasable is never an active holder"
        );
    }

    /// The brake's critical floor: `4%` of physical, never under 1 GiB —
    /// a 64 GB machine floors at 2.56 GiB, a 16 GB machine at 1 GiB.
    #[test]
    fn the_critical_floor_scales_with_physical_memory() {
        assert_eq!(critical_floor(16 * GIB), GIB, "16 GB → the 1 GiB floor");
        assert_eq!(critical_floor(24 * GIB), GIB, "24 GB → the 1 GiB floor");
        assert_eq!(critical_floor(64 * GIB), 64 * GIB / 25, "64 GB → 4%");
        assert_eq!(critical_floor(0), GIB, "a blind host still floors");
    }

    /// The shared pressure monitor's victim choice over the ledger: a
    /// live, idle, quiet-enough releasable of ANOTHER owner is the LRU
    /// victim — never this daemon's own record, never a busy holder,
    /// never one too fresh.
    #[test]
    fn the_pressure_victim_is_the_lru_idle_releasable() {
        let dir = tempfile::tempdir().expect("ledger dir");
        let now = SystemTime::now();
        let secs = |ago: u64| unix_secs(now) - ago;
        let old = now - PRESSURE_UNUSED - Duration::from_secs(20);
        let _own = live_record(dir.path(), "own", 201, secs(300), true, old);
        let _lru = live_record(dir.path(), "lru", 202, secs(400), true, old);
        let _fresh = live_record(dir.path(), "fresh", 203, secs(100), true, now);
        let _busy = live_record(dir.path(), "busy", 204, secs(500), false, old);
        let victims = evictable_idle(dir.path(), 201);
        let labels: Vec<&str> = victims
            .iter()
            .map(|(_, holder)| holder.label.as_str())
            .collect();
        assert_eq!(labels, ["lru"], "the LRU idle releasable only: {labels:?}");
    }

    /// The last idle resident sheds itself only at the critical line: a
    /// lone idle self is the victim under critical, never under warn;
    /// a self that did not declare itself releasable is never the victim.
    #[test]
    fn critical_sheds_the_last_idle_resident() {
        let dir = tempfile::tempdir().expect("ledger dir");
        let now = SystemTime::now();
        let secs = |ago: u64| unix_secs(now) - ago;
        let old = now - PRESSURE_UNUSED - Duration::from_secs(20);
        let _own = live_record(dir.path(), "own", 301, secs(300), true, old);
        // Critical + self-evictable: own idle record is the victim.
        let victim = pick_victim(dir.path(), 301, true, true);
        assert_eq!(
            victim.map(|(_, holder)| holder.label),
            Some("own".to_string()),
            "the lone idle self under critical"
        );
        // Warn only: self is kept.
        assert!(
            pick_victim(dir.path(), 301, true, false).is_none(),
            "warn never sheds the last idle resident"
        );
        // Critical but the caller is busy/hosting: kept.
        assert!(
            pick_victim(dir.path(), 301, false, true).is_none(),
            "a busy self is never evictable"
        );
        // Critical on a non-idle self: the busy caller never self-evicts;
        // another idle holder is still the victim instead.
        let _busy = live_record(dir.path(), "busy", 302, secs(200), false, old);
        assert_eq!(
            pick_victim(dir.path(), 302, true, true).map(|(_, h)| h.label),
            Some("own".to_string()),
            "the idle other, never the busy self"
        );
    }

    /// The cooldown is machine-wide and time-boxed: a fresh timestamp
    /// blocks, an aged one frees.
    #[test]
    fn the_pressure_cooldown_paces_every_resident() {
        let dir = tempfile::tempdir().expect("ledger dir");
        assert!(!under_cooldown(dir.path()), "no file, no cooldown");
        let now = unix_secs(SystemTime::now());
        fs::write(cooldown_file(dir.path()), now.to_string()).expect("stamp");
        assert!(under_cooldown(dir.path()), "a fresh stamp blocks");
        fs::write(
            cooldown_file(dir.path()),
            (now - PRESSURE_COOLDOWN.as_secs() - 1).to_string(),
        )
        .expect("old stamp");
        assert!(!under_cooldown(dir.path()), "an aged stamp frees");
    }

    /// Three estimated sibling worktrees admitted in parallel when the
    /// estimated total fits the budget — no first-run serialization.
    #[test]
    fn three_estimated_siblings_run_in_parallel() {
        let dir = tempfile::tempdir().expect("ledger dir");
        // A sibling recorded its peak: 3 GB on 300 MB of source.
        record_peak_in(
            dir.path(),
            "repo:/common:m1",
            Face::AnalysisServer,
            3 * GIB,
            Some(300_000_000),
        );
        // Three more sibling worktrees (300 MB source each → 3 GB each).
        let mut leases = Vec::new();
        for i in 0..3 {
            let mut request = slot(Face::AnalysisServer, &format!("wt{i}"), 0, false);
            request.bytes = sized_in(
                dir.path(),
                Some(&format!("repo:/common:wt{i}")),
                Face::AnalysisServer,
                &[],
                Some(300_000_000),
            )
            .expect("estimated");
            request.sized = true;
            request.source_bytes = Some(300_000_000);
            leases.push(held(dir.path(), request));
        }
        assert_eq!(
            leases.iter().filter(|lease| lease.sized()).count(),
            3,
            "all three estimated and admitted"
        );
    }

    #[test]
    fn the_cap_variable_overrides_the_config() {
        // SAFETY: the only test of this binary that touches the variable.
        unsafe { std::env::remove_var(CAP_ENV) };
        assert_eq!(cap(3), 3);
        unsafe { std::env::set_var(CAP_ENV, "1") };
        assert_eq!(cap(3), 1);
        unsafe { std::env::remove_var(CAP_ENV) };
    }
}
