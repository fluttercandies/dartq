//! Host-wide coordination shared by every dartq process of one user.
//!
//! Two facts live here, both keyed on the user rather than on a checkout:
//!
//! - **The shared cache directory** ([`cache_dir`]). Parallel agents work in
//!   sibling `git worktree`s or unrelated copies of one repository; a store
//!   under each package's `.dart_tool` would make every worktree pay its own
//!   cold run. Content-addressed entries are portable across checkouts, so
//!   one per-user store serves all of them.
//! - **The memory lease** ([`Lease`]). One runner invocation holds a
//!   compiler and the runner VM ([`RUN_UNITS`]) plus a share per concurrent
//!   suite ([`WORKER_UNITS`]). A per-process scheduler only sees its own
//!   children, so N agents starting together would each size themselves
//!   against the same free memory. The lease draws from one host-wide pool
//!   of [`UNIT`]-sized slots instead: slots are lock files under the cache
//!   directory held with `flock`, so the kernel releases them when a
//!   process exits, crashed or not.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// One lease slot: the resident memory one test isolate host needs.
pub const UNIT: u64 = 512 * 1024 * 1024;

/// Slots one runner invocation needs before any suite runs: the
/// `frontend_server` plus the runner VM. Measured on a 171-suite package
/// (10-core host): 3.1 GB peak at the runner's default concurrency, 5.0 GB
/// at three concurrent suites, 6.9 GB at seven.
pub const RUN_UNITS: usize = 6;

/// Slots per concurrent suite (see [`RUN_UNITS`] for the measurement).
pub const WORKER_UNITS: usize = 2;

/// Share of physical memory the lease pool may hand out.
const POOL_SHARE: (u64, u64) = (6, 10);

/// Pause between attempts while the pool is exhausted.
const RETRY: Duration = Duration::from_millis(250);

/// The per-user dartq cache directory: `$DARTQ_CACHE_DIR`, else the
/// platform cache location (`$XDG_CACHE_HOME/dartq`, `~/Library/Caches/dartq`
/// on macOS, `~/.cache/dartq`, `%LOCALAPPDATA%\dartq`). `None` when the
/// environment names no home at all.
#[must_use]
pub fn cache_dir() -> Option<PathBuf> {
    let var = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from);
    if let Some(dir) = var("DARTQ_CACHE_DIR") {
        return Some(dir);
    }
    if let Some(dir) = var("XDG_CACHE_HOME") {
        return Some(dir.join("dartq"));
    }
    if cfg!(windows) {
        return var("LOCALAPPDATA").map(|dir| dir.join("dartq"));
    }
    let home = var("HOME")?;
    Some(if cfg!(target_os = "macos") {
        home.join("Library").join("Caches").join("dartq")
    } else {
        home.join(".cache").join("dartq")
    })
}

/// Physical memory of the host.
#[must_use]
pub fn total_memory_bytes() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let mut bytes: u64 = 0;
        let mut len = std::mem::size_of::<u64>();
        // SAFETY: `hw.memsize` is a u64; the out-pointer and length match it.
        let rc = unsafe {
            libc::sysctlbyname(
                c"hw.memsize".as_ptr(),
                (&raw mut bytes).cast(),
                &raw mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        (rc == 0 && bytes > 0).then_some(bytes)
    }
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let line = text.lines().find_map(|line| line.strip_prefix("MemTotal:"))?;
        let kib: u64 = line.trim().trim_end_matches("kB").trim().parse().ok()?;
        Some(kib * 1024)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

/// Slots in the host-wide pool: [`POOL_SHARE`] of physical memory, never
/// fewer than one run with one worker. On an 8 GB host that is 9 slots:
/// one stock run with one worker (8), or one grouped shard with up to seven
/// concurrent suites; a second run waits.
#[must_use]
pub fn capacity_units() -> usize {
    let pool = total_memory_bytes().map_or(16, |total| {
        usize::try_from(total / POOL_SHARE.1 * POOL_SHARE.0 / UNIT).unwrap_or(usize::MAX)
    });
    pool.max(RUN_UNITS + WORKER_UNITS)
}

/// The memory profile of one kind of run: what a runner invocation holds
/// before its first suite, and what each concurrent suite adds, in
/// [`UNIT`]s. Seeded from measurements and refined from the peak resident
/// size of real runs ([`Profile::observe`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profile {
    pub base: usize,
    pub per_worker: usize,
}

impl Profile {
    /// The stock runner: one `frontend_server` (2.9–3.2 GB on a 171-suite
    /// package) plus one test isolate group per concurrent suite.
    pub const STOCK: Self = Self {
        base: RUN_UNITS,
        per_worker: WORKER_UNITS,
    };

    /// One `flutter test` process: the tool's own VM and its
    /// `frontend_server` (~1 GB together once a 300-suite widget-test
    /// package is compiled), plus one `flutter_tester` per concurrent suite
    /// (~0.2 GB each). Measured: one process at `--concurrency=5` 1.85 GB
    /// peak; three at two each 3.1 GB.
    pub const FLUTTER: Self = Self { base: 2, per_worker: 1 };

    /// One grouped shard process: the shared program and heap (2.9 GB
    /// peak at seven concurrent suites, 3.5 GB at ten, on the same
    /// package), with no compiler resident while suites run.
    pub const GROUPED: Self = Self { base: 2, per_worker: 1 };

    /// The grouped Flutter pass: one runner VM and one resident
    /// `frontend_server` (~1.4 GB together on a 3000-suite package), plus
    /// one reused `flutter_tester` per concurrent suite (0.24–0.40 GB each,
    /// one compiled chunk loaded). Measured: five testers 2.4 GB peak.
    pub const FLUTTER_GROUPED: Self = Self { base: 3, per_worker: 1 };

    /// `self` with the per-worker share recorded for `kind` by earlier runs
    /// on this host ([`Self::observe`]), or unchanged when none is recorded.
    #[must_use]
    pub fn calibrated(self, kind: &str) -> Self {
        let recorded = cache_dir()
            .and_then(|dir| std::fs::read_to_string(dir.join("test").join(format!("profile-{kind}"))).ok())
            .and_then(|text| text.trim().parse::<usize>().ok())
            .filter(|&units| units >= 1);
        Self {
            per_worker: recorded.unwrap_or(self.per_worker),
            ..self
        }
    }

    /// Fold one run's measured peak (`peak` bytes resident across the run's
    /// `processes` process trees, with `workers` concurrent suites leased
    /// under `self`) into the per-worker share recorded for `kind`:
    /// re-derived from the peak above every process's base, rounded up, and
    /// moved halfway from the current share toward it, so one quiet run
    /// cannot undersize the next while a heavy package's real footprint is
    /// learned within a few runs.
    pub fn observe(self, kind: &str, peak: u64, workers: usize, processes: usize) {
        let Some(dir) = cache_dir() else {
            return;
        };
        if peak == 0 {
            return;
        }
        let units = usize::try_from(peak.div_ceil(UNIT)).unwrap_or(usize::MAX);
        let measured = units
            .saturating_sub(self.base * processes.max(1))
            .div_ceil(workers.max(1))
            .max(1);
        let next = (self.per_worker + measured).div_ceil(2).max(1);
        let _ = crate::stamp::write_atomic(
            &dir.join("test").join(format!("profile-{kind}")),
            format!("{next}\n").as_bytes(),
        );
    }
}

/// Lease one runner invocation of up to `workers` concurrent suites from
/// the host-wide pool, waiting until at least one suite fits. Returns the
/// lease and the concurrency it covers; `Ok(None)` when `deadline` passes
/// first. Without a cache directory there is no pool to share: the run
/// gets `workers` unleased.
///
/// # Errors
///
/// I/O failures on the slot files.
pub fn lease_run(workers: usize, deadline: Instant) -> io::Result<Option<(Option<Lease>, usize)>> {
    lease(Profile::STOCK, workers, deadline)
}

/// [`lease_run`] for an explicit memory profile.
///
/// The request is capped by memory free right now as well as by the pool:
/// the pool accounts for dartq runs, free memory for everything else on
/// the host (an IDE, a browser, another test runner). Free memory is read
/// once, when the lease is taken; the pool bound holds for the whole run.
///
/// # Errors
///
/// I/O failures on the slot files.
pub fn lease(profile: Profile, workers: usize, deadline: Instant) -> io::Result<Option<(Option<Lease>, usize)>> {
    lease_processes(profile, workers, 1, deadline)
}

/// [`lease`] for a run spread over up to `processes` processes, each paying
/// `profile.base` once: the request covers every process's base, the grant
/// at least one process with one suite. The caller sizes its process count
/// from [`Lease::units`].
///
/// # Errors
///
/// I/O failures on the slot files.
pub fn lease_processes(
    profile: Profile,
    workers: usize,
    processes: usize,
    deadline: Instant,
) -> io::Result<Option<(Option<Lease>, usize)>> {
    let workers = workers.max(1);
    let processes = processes.clamp(1, workers);
    let Some(dir) = cache_dir() else {
        return Ok(Some((None, workers)));
    };
    let free = free_memory_bytes().map(|free| usize::try_from(free / UNIT).unwrap_or(usize::MAX));
    let fit = free
        .map_or(workers, |units| units.saturating_sub(profile.base) / profile.per_worker)
        .clamp(1, workers);
    let want = (profile.base * processes + fit * profile.per_worker)
        .min(free.unwrap_or(usize::MAX).max(profile.base + profile.per_worker));
    let min = profile.base + profile.per_worker;
    Ok(Lease::acquire(&dir.join("test"), want, min, capacity_units(), deadline)?.map(|lease| {
        let (_, granted) = split(profile, lease.units(), workers, processes);
        (Some(lease), granted)
    }))
}

/// How `units` lease units divide into processes and concurrent suites
/// under `profile`: as many of the wanted `processes` as leave at least one
/// suite per process after every process's base, and the rest as suites
/// (at most `workers`). Always at least one process with one suite.
#[must_use]
pub fn split(profile: Profile, units: usize, workers: usize, processes: usize) -> (usize, usize) {
    let workers = workers.max(1);
    let mut shards = processes.clamp(1, workers);
    while shards > 1 && units < shards * (profile.base + profile.per_worker) {
        shards -= 1;
    }
    let suites = (units.saturating_sub(shards * profile.base) / profile.per_worker).clamp(shards, workers);
    (shards, suites)
}

/// Slots held by this process. Dropping the lease closes the lock files,
/// which returns the slots to the pool.
#[derive(Debug)]
pub struct Lease {
    /// Held only for their locks; closing them releases the slots.
    #[cfg(unix)]
    _slots: Vec<File>,
    units: usize,
}

impl Lease {
    /// The slots held.
    #[must_use]
    pub fn units(&self) -> usize {
        self.units
    }

    /// Take between `min` and `want` slots from the pool under `dir`
    /// (`capacity` slots in total), waiting while fewer than `min` are free.
    /// `Ok(None)` when `deadline` passes first.
    ///
    /// Taking is serialized by a gate lock, so two processes never split
    /// the last free slots between them and both come up short. Without
    /// `flock` (non-unix hosts) the lease is granted unbounded at once.
    ///
    /// # Errors
    ///
    /// I/O failures creating or locking the slot files.
    pub fn acquire(
        dir: &Path,
        want: usize,
        min: usize,
        capacity: usize,
        deadline: Instant,
    ) -> io::Result<Option<Self>> {
        let want = want.min(capacity).max(min.min(capacity));
        let min = min.min(want);
        #[cfg(unix)]
        {
            let dir = dir.join("slots");
            std::fs::create_dir_all(&dir)?;
            loop {
                let gate = open_lock(&dir.join("gate.lock"))?;
                lock(&gate, false)?;
                let mut slots = Vec::with_capacity(want);
                for index in 0..capacity {
                    if slots.len() == want {
                        break;
                    }
                    let slot = open_lock(&dir.join(format!("slot-{index}.lock")))?;
                    if lock(&slot, true)? {
                        slots.push(slot);
                    }
                }
                drop(gate);
                if slots.len() >= min {
                    let units = slots.len();
                    return Ok(Some(Self { _slots: slots, units }));
                }
                drop(slots);
                if Instant::now() + RETRY > deadline {
                    return Ok(None);
                }
                std::thread::sleep(RETRY);
            }
        }
        #[cfg(not(unix))]
        {
            let _ = (dir, deadline);
            Ok(Some(Self { units: want }))
        }
    }
}

#[cfg(unix)]
fn open_lock(path: &Path) -> io::Result<File> {
    std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(path)
}

/// `flock(LOCK_EX)`; with `try_only`, `Ok(false)` when another process
/// holds it.
#[cfg(unix)]
fn lock(file: &File, try_only: bool) -> io::Result<bool> {
    use std::os::fd::AsRawFd;
    let operation = if try_only {
        libc::LOCK_EX | libc::LOCK_NB
    } else {
        libc::LOCK_EX
    };
    loop {
        // SAFETY: plain syscall on a descriptor this function borrows.
        if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        match error.kind() {
            io::ErrorKind::Interrupted => {}
            io::ErrorKind::WouldBlock => return Ok(false),
            _ => return Err(error),
        }
    }
}

/// An executable on `PATH` (`dart`, `flutter`), as `which` finds it.
#[must_use]
pub fn find(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let names: &[String] = &if cfg!(windows) {
        vec![format!("{name}.exe"), format!("{name}.bat"), format!("{name}.cmd")]
    } else {
        vec![name.to_owned()]
    };
    std::env::split_paths(&path).find_map(|dir| names.iter().map(|name| dir.join(name)).find(|candidate| candidate.is_file()))
}

/// Memory the host can hand out now: available memory on Linux
/// (`MemAvailable`), free plus inactive plus speculative pages on macOS
/// (what Activity Monitor calls available).
#[must_use]
pub fn free_memory_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let line = text.lines().find_map(|line| line.strip_prefix("MemAvailable:"))?;
        let kib: u64 = line.trim().trim_end_matches("kB").trim().parse().ok()?;
        Some(kib * 1024)
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("vm_stat").stderr(std::process::Stdio::null()).output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let page: u64 = text
            .lines()
            .next()?
            .split("page size of")
            .nth(1)?
            .split_whitespace()
            .next()?
            .parse()
            .ok()?;
        let pages = |label: &str| -> u64 {
            text.lines()
                .find_map(|line| line.strip_prefix(label))
                .and_then(|count| count.trim().trim_end_matches('.').parse().ok())
                .unwrap_or(0)
        };
        let bytes = (pages("Pages free:") + pages("Pages inactive:") + pages("Pages speculative:")) * page;
        (bytes > 0).then_some(bytes)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

// ---------------------------------------------------------------------------
// Runner process groups
// ---------------------------------------------------------------------------

/// Every runner runs in a process group of its own (so a watchdog can kill
/// its whole tree), which also keeps the terminal's interrupt from reaching
/// it. The groups alive right now, so an interrupt of this process takes
/// them down with it.
static GROUPS: parking_lot::Mutex<Vec<u32>> = parking_lot::Mutex::new(Vec::new());

/// A live runner group, registered for the interrupt handler while held.
pub struct Group(u32);

impl Group {
    /// Register the group led by `pid`, installing the handler on first use.
    #[must_use]
    pub fn enter(pid: u32) -> Self {
        install_signal_handler();
        GROUPS.lock().push(pid);
        Self(pid)
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        GROUPS.lock().retain(|&pid| pid != self.0);
    }
}

/// Write end of the pipe the signal handler wakes the teardown thread on.
#[cfg(unix)]
static SIGNAL_PIPE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

/// On `SIGINT`/`SIGTERM`/`SIGHUP`: kill every live runner group, then die
/// of the signal as an unhandled one would.
#[cfg(unix)]
fn install_signal_handler() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let mut fds = [0 as libc::c_int; 2];
        // SAFETY: `fds` is a valid two-element buffer.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return;
        }
        SIGNAL_PIPE.store(fds[1], std::sync::atomic::Ordering::Release);
        let reader = fds[0];
        std::thread::spawn(move || {
            let mut byte = 0u8;
            // SAFETY: one-byte read into a local; retried on interruption.
            while unsafe { libc::read(reader, std::ptr::addr_of_mut!(byte).cast(), 1) } != 1 {}
            for &pid in GROUPS.lock().iter() {
                if let Ok(pid) = libc::pid_t::try_from(pid) {
                    // SAFETY: plain syscall; a gone group fails with ESRCH.
                    unsafe { libc::kill(-pid, libc::SIGKILL) };
                }
            }
            let signal = libc::c_int::from(byte);
            // SAFETY: restore the default disposition and re-raise.
            unsafe {
                libc::signal(signal, libc::SIG_DFL);
                libc::raise(signal);
            }
        });
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            // SAFETY: `on_signal` performs a single async-signal-safe write.
            unsafe { libc::signal(signal, on_signal as *const () as libc::sighandler_t) };
        }
    });
}

#[cfg(unix)]
extern "C" fn on_signal(signal: libc::c_int) {
    let fd = SIGNAL_PIPE.load(std::sync::atomic::Ordering::Acquire);
    let byte = u8::try_from(signal).unwrap_or(0);
    // SAFETY: `write` is async-signal-safe; the byte lives on this stack.
    unsafe { libc::write(fd, std::ptr::addr_of!(byte).cast(), 1) };
}

#[cfg(not(unix))]
fn install_signal_handler() {}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A weak host gets one process; extra processes only when the units
    /// pay for their bases.
    #[test]
    fn split_follows_memory() {
        let grouped = Profile::GROUPED;
        // 4-core / 8 GB: pool of 9 units, 3 suites wanted over 1 process.
        assert_eq!(split(grouped, 9, 3, 1), (1, 3));
        // 10-core host: 7 suites over 3 processes, plenty of units.
        assert_eq!(split(grouped, 2 * 3 + 7, 7, 3), (3, 7));
        // Units for two bases only: two processes, the rest as suites.
        assert_eq!(split(grouped, 2 * 2 + 4, 7, 3), (2, 4));
        // Below one base plus one suite still runs one suite.
        assert_eq!(split(grouped, 1, 7, 3), (1, 1));
    }

    /// Leases from one pool never overlap, a short pool times out, and a
    /// dropped lease returns its slots.
    #[test]
    fn lease_pool_is_shared_and_bounded() {
        let tmp = tempfile::tempdir().expect("tmp");
        let far = Instant::now() + Duration::from_secs(5);
        let first = Lease::acquire(tmp.path(), 6, 2, 8, far).expect("io").expect("granted");
        assert_eq!(first.units(), 6);
        // Two slots left: granted at the minimum, not the request.
        let second = Lease::acquire(tmp.path(), 6, 2, 8, far).expect("io").expect("granted");
        assert_eq!(second.units(), 2);
        // Pool empty: waits, then gives up at the deadline.
        let soon = Instant::now() + Duration::from_millis(300);
        assert!(Lease::acquire(tmp.path(), 1, 1, 8, soon).expect("io").is_none());
        drop(first);
        let third = Lease::acquire(tmp.path(), 6, 1, 8, far).expect("io").expect("granted");
        assert_eq!(third.units(), 6, "released slots return to the pool");
        drop(second);
    }
}
