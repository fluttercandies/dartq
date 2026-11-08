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
//! * **Accounting.** A holder counts `max(lease, live tree RSS)`, so one
//!   that grew past its lease is not undercounted. Trees nest (a debug
//!   app inside a session daemon's tree): a tree stops at another
//!   holder's root, whose subtree that holder counts.
//! * **Budget.** `max_percent` of physical memory (default 90). A request
//!   larger than the whole budget is clamped to it: it runs alone instead
//!   of waiting forever.
//! * **Availability.** The request plus the growth current holders still
//!   owe (`Σ max(0, lease − RSS)`) must also fit in the memory the OS
//!   reports available, minus a reserve of `100 − max_percent` percent of
//!   physical memory: what accounts for everything outside the ledger (the
//!   user's IDE, the Gradle daemons a build leaves behind). An unreadable
//!   figure skips the check.
//! * **Admission.** Under one gate lock. Slot requests wait in arrival
//!   order (waiter tickets `w-<seq>-<pid>.lock`, locked by their waiter).
//!   On a shortfall, idle releasable holders are released least recently
//!   used first, only as far as the shortfall needs, never one whose busy
//!   lock is held; otherwise the requester waits — one note, polling,
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

    fn reserve(self) -> u64 {
        percent(self.total, 100 - self.max_percent)
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
/// unsized one no other unsized slot holder. While short, the least
/// recently used idle releasable holder that would help goes: any for
/// memory (it frees what it counts and what it holds now plus what it
/// owed), a slot holder for the count, an unsized one for the first-run
/// slot. A busy or non-releasable holder always stays.
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
            Shortfall::FirstRun => holder.slot && !holder.sized,
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
    if !ask.sized && slots.iter().any(|holder| !holder.sized) {
        return Some(Shortfall::FirstRun);
    }
    None
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
    #[cfg(target_os = "linux")]
    {
        let text = fs::read_to_string("/proc/meminfo").ok()?;
        let line = text
            .lines()
            .find_map(|line| line.strip_prefix("MemAvailable:"))?;
        let kib: u64 = line.trim().trim_end_matches("kB").trim().parse().ok()?;
        Some(kib.saturating_mul(1024))
    }
    #[cfg(target_os = "macos")]
    {
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
    let path = ledger_dir()?.join("peaks").join(peak_file(key));
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Records `bytes` as the peak of `key` (the latest run's, so a project
/// that shrank books less next time).
pub fn record_peak(key: &str, bytes: u64) {
    if bytes == 0 {
        return;
    }
    let Some(dir) = ledger_dir().map(|dir| dir.join("peaks")) else {
        return;
    };
    if fs::create_dir_all(&dir).is_ok() {
        let temp = dir.join(format!(".{}.{}", peak_file(key), std::process::id()));
        if fs::write(&temp, bytes.to_string()).is_ok() {
            let _ = fs::rename(&temp, dir.join(peak_file(key)));
        }
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
/// recorded peak, else the largest live sized holder of `face` (a peer's
/// peak), else [`DEFAULT_LEASE`], unsized — a first run, which admission
/// serializes with every other first run.
#[must_use]
pub fn sizing(key: &str, face: Face) -> (u64, bool) {
    if let Some(bytes) = peak(key) {
        return (bytes, true);
    }
    ledger_dir()
        .map(|dir| scan(&dir))
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, holder)| holder.face == face && holder.sized && holder.lease > 0)
        .map(|(_, holder)| holder.lease)
        .max()
        .map_or((DEFAULT_LEASE, false), |bytes| (bytes, true))
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
    /// slot is taken — the strictest cap, or the first-run slot — and the
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
    request: Request,
    min: u64,
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
        // A holder just released keeps its file until it exits; within the
        // grace it counts as gone (its memory is on the way out).
        released.retain(|(path, at)| path.exists() && at.elapsed() < RELEASE_GRACE);
        let holders: Vec<(PathBuf, Holder)> = scan(dir)
            .into_iter()
            .filter(|(path, _)| !released.iter().any(|(gone, _)| gone == path))
            .collect();
        let snapshot: Vec<Holder> = holders.iter().map(|(_, holder)| holder.clone()).collect();
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
            total: 10 * GIB,
            available: Some(2 * GIB),
            max_percent: 90,
        };
        // Available 2 GiB minus the 1 GiB reserve leaves 1 GiB.
        assert_eq!(
            decide(&[], ask(GIB, false, 0, true), figures),
            Decision::Admit {
                release: vec![],
                headroom: GIB
            }
        );
        assert_eq!(
            decide(&[], ask(2 * GIB, false, 0, true), figures),
            Decision::Wait(Shortfall::Memory)
        );
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
