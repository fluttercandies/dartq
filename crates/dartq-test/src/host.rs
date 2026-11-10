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
//!   against the same free memory. The lease is booked in the machine-wide
//!   memory ledger (`dartq_toolchain::ledger`) every heavy dartq face
//!   shares, in [`UNIT`]s: a test run waits for the analyzers, builds and
//!   other runs already holding memory, and they for it.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// One lease slot: the resident memory one test isolate host needs.
pub const UNIT: u64 = 512 * 1024 * 1024;

/// Slots one runner invocation needs before any suite runs: the
/// `frontend_server` plus the runner VM. Measured on a 171-suite package
/// (10-core host): 3.1 GB peak at the runner's default concurrency, 5.0 GB
/// at three concurrent suites, 6.9 GB at seven.
pub const RUN_UNITS: usize = 6;

/// Slots per concurrent suite (see [`RUN_UNITS`] for the measurement).
pub const WORKER_UNITS: usize = 2;

/// The per-user dartq cache directory: `$DARTQ_CACHE_DIR`, else the
/// platform cache location (`$XDG_CACHE_HOME/dartq`, `~/Library/Caches/dartq`
/// on macOS, `~/.cache/dartq`, `%LOCALAPPDATA%\dartq`). `None` when the
/// environment names no home at all.
#[must_use]
pub fn cache_dir() -> Option<PathBuf> {
    let var = |name: &str| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
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

/// The memory profile of one kind of run: what a runner invocation holds
/// before its first suite, and what each concurrent suite adds, in
/// [`UNIT`]s. Seeded from measurements and refined from real runs'
/// heaviest worker process ([`Profile::observe`]).
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
    pub const FLUTTER: Self = Self {
        base: 2,
        per_worker: 1,
    };

    /// One grouped shard process: the shared program and heap (2.9 GB
    /// peak at seven concurrent suites, 3.5 GB at ten, on the same
    /// package), with no compiler resident while suites run.
    pub const GROUPED: Self = Self {
        base: 2,
        per_worker: 1,
    };

    /// The grouped Flutter pass: one runner VM and one resident
    /// `frontend_server` (1.0–1.4 GB together at the base-side peak), plus
    /// one reused `flutter_tester` per concurrent suite. A tester's peak
    /// depends on its suite (0.3–0.65 GB measured across packages), and
    /// workers are reused across suites, so the per-worker share covers the
    /// heaviest measured tester, not the average: two units.
    pub const FLUTTER_GROUPED: Self = Self {
        base: 3,
        per_worker: 2,
    };

    /// `self` with the per-worker share raised to any larger value recorded
    /// for `kind` by earlier runs on this host ([`Self::observe`]); the
    /// built-in seed is the floor. A record folds the peak of the single
    /// heaviest worker process a run spawned (or, where a run's workers
    /// share one process, the tree peak spread across them) — a bound on
    /// one worker, not the tree. Below the seed a record is
    /// under-measurement, not a lighter host: booking under the seed would
    /// break the lease's bound where booking over it only costs
    /// parallelism.
    #[must_use]
    pub fn calibrated(self, kind: &str) -> Self {
        let recorded = cache_dir()
            .and_then(|dir| {
                std::fs::read_to_string(dir.join("test").join(format!("profile-{kind}"))).ok()
            })
            .and_then(|text| text.trim().parse::<usize>().ok());
        self.refine(recorded)
    }

    /// [`Self::calibrated`]'s fold: the record applies only above the seed.
    #[must_use]
    fn refine(self, recorded: Option<usize>) -> Self {
        Self {
            per_worker: recorded.map_or(self.per_worker, |units| units.max(self.per_worker)),
            ..self
        }
    }

    /// Fold one run's measurement into the per-worker share recorded for
    /// `kind`. A run's workers are its concurrent suites; their honest
    /// bound is the peak of the heaviest worker process the run spawned
    /// (`tester_peak`, sampled alongside the tree peak — a tester's own
    /// maximum, undiluted by the rest of the tree). Where workers share
    /// one process (`dart test` grouped shards: isolates, not processes)
    /// no tester exists and the measurement falls back to the trees' peak
    /// above every process's base, spread across the workers. Nothing is
    /// recorded when neither was sampled.
    pub fn observe(
        self,
        kind: &str,
        peak: u64,
        tester_peak: u64,
        workers: usize,
        processes: usize,
    ) {
        let Some(dir) = cache_dir() else {
            return;
        };
        let Some(measured) = self.measurement(peak, tester_peak, workers, processes) else {
            return;
        };
        let next = self.absorb(measured).per_worker;
        let _ = crate::stamp::write_atomic(
            &dir.join("test").join(format!("profile-{kind}")),
            format!("{next}\n").as_bytes(),
        );
    }

    /// One run's per-worker measurement in [`UNIT`]s, `None` when the run
    /// sampled nothing ([`Self::observe`]'s no-op).
    #[must_use]
    fn measurement(
        self,
        peak: u64,
        tester_peak: u64,
        workers: usize,
        processes: usize,
    ) -> Option<usize> {
        if tester_peak > 0 {
            return Some(
                usize::try_from(tester_peak.div_ceil(UNIT))
                    .unwrap_or(usize::MAX)
                    .max(1),
            );
        }
        if peak == 0 {
            return None;
        }
        let units = usize::try_from(peak.div_ceil(UNIT)).unwrap_or(usize::MAX);
        Some(
            units
                .saturating_sub(self.base * processes.max(1))
                .div_ceil(workers.max(1))
                .max(1),
        )
    }

    /// The run fold: a measurement above the current share books in full —
    /// one heavy run converges the record — while a lighter one moves the
    /// share only halfway down, so one quiet run cannot undersize the
    /// next. The booking floor stays the built-in seed ([`Self::refine`]).
    #[must_use]
    fn absorb(self, measured: usize) -> Self {
        Self {
            per_worker: measured
                .max((self.per_worker + measured).div_ceil(2))
                .max(1),
            ..self
        }
    }
}

/// Lease one runner invocation of up to `workers` concurrent suites from
/// the machine-wide memory ledger, waiting until at least one suite fits.
/// Returns the lease and the concurrency it covers. `max_percent` is the
/// governing `memory.max_percent`. Without a ledger (no cache directory,
/// a non-unix host) the run gets `workers` unleased.
///
/// # Errors
///
/// A timeout naming the holders when `deadline` passes first; I/O
/// failures on the ledger.
pub fn lease_run(
    workers: usize,
    max_percent: u8,
    deadline: Instant,
) -> io::Result<(Option<Lease>, usize)> {
    lease(Profile::STOCK, workers, max_percent, deadline)
}

/// [`lease_run`] for an explicit memory profile.
///
/// # Errors
///
/// As [`lease_run`].
pub fn lease(
    profile: Profile,
    workers: usize,
    max_percent: u8,
    deadline: Instant,
) -> io::Result<(Option<Lease>, usize)> {
    lease_processes(profile, workers, 1, max_percent, deadline)
}

/// [`lease`] for a run spread over up to `processes` processes, each paying
/// `profile.base` once: the request covers every process's base, the grant
/// at least one process with one suite. The ledger grants as much of the
/// request as fits (it also checks the memory the OS reports available,
/// which accounts for everything outside dartq). The caller sizes its
/// process count from [`Lease::units`].
///
/// # Errors
///
/// As [`lease_run`].
pub fn lease_processes(
    profile: Profile,
    workers: usize,
    processes: usize,
    max_percent: u8,
    deadline: Instant,
) -> io::Result<(Option<Lease>, usize)> {
    let workers = workers.max(1);
    let processes = processes.clamp(1, workers);
    let (want, min) = request_units(profile, workers, processes);
    #[cfg(unix)]
    {
        use dartq_toolchain::ledger;
        let label = std::env::current_dir()
            .map(|dir| dir.display().to_string())
            .unwrap_or_default();
        // Memory only: the run's job slot is the invocation's, taken once
        // at its start; a shard's memory books beside it.
        let request = ledger::Request {
            face: ledger::Face::Test,
            label,
            bytes: want as u64 * UNIT,
            pid: Some(std::process::id()),
            release: None,
            slot: false,
            cap: 0,
            sized: true,
            peak_key: None,
            source_bytes: None,
        };
        let granted = ledger::acquire(request, min as u64 * UNIT, max_percent, deadline, |note| {
            eprintln!("{note}");
        })
        .map_err(io::Error::other)?;
        Ok(match granted {
            Some(inner) => {
                let units = units_of(inner.bytes());
                let (_, suites) = split(profile, units, workers, processes);
                (
                    Some(Lease {
                        _inner: inner,
                        units,
                    }),
                    suites,
                )
            }
            None => (None, workers),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (want, min, max_percent, deadline);
        Ok((None, workers))
    }
}

/// The units a run of `workers` suites over `processes` processes asks
/// for, and the least it runs with (one process with one suite).
fn request_units(profile: Profile, workers: usize, processes: usize) -> (usize, usize) {
    (
        profile.base * processes + workers * profile.per_worker,
        profile.base + profile.per_worker,
    )
}

/// Whole units in a granted byte count.
fn units_of(bytes: u64) -> usize {
    usize::try_from(bytes / UNIT).unwrap_or(usize::MAX).max(1)
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
    let suites =
        (units.saturating_sub(shards * profile.base) / profile.per_worker).clamp(shards, workers);
    (shards, suites)
}

/// Memory booked in the ledger by this process. Dropping the lease
/// returns it.
#[derive(Debug)]
pub struct Lease {
    #[cfg(unix)]
    _inner: dartq_toolchain::ledger::Lease,
    units: usize,
}

impl Lease {
    /// The [`UNIT`]s held.
    #[must_use]
    pub fn units(&self) -> usize {
        self.units
    }
}

/// An executable on `PATH` (`dart`, `flutter`), as `which` finds it, never
/// this binary itself: `dartq init` puts a `flutter` link to dartq first on
/// `PATH`, while a run needs the toolchain behind the link.
#[must_use]
pub fn find(name: &str) -> Option<PathBuf> {
    dartq_toolchain::find_on_path(name)
}

/// The `flutter` a run spawns, resolved as every dartq face resolves it:
/// `explicit` (the `build_cache.flutter` pin), then `$DARTQ_FLUTTER`, then
/// [`find`]'s scan — so the SDK a user pins for the shim is the one the
/// test faces run.
#[must_use]
pub fn find_flutter(explicit: Option<&Path>) -> Option<PathBuf> {
    dartq_toolchain::find_flutter(explicit)
}

/// The `dart` a run spawns, through the same resolution: `explicit`, then
/// `$DARTQ_DART`, then [`find`]'s scan.
#[must_use]
pub fn find_dart(explicit: Option<&Path>) -> Option<PathBuf> {
    dartq_toolchain::find_dart(explicit)
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
    // The scan behind [`find`]: the lookup tests drive it over injected
    // directories, as `find` drives it over `PATH`.
    use dartq_toolchain::scan_in;

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

    /// A run asks for every process's base plus every suite's share, runs
    /// with one process and one suite at least, and a grant converts back
    /// to whole units that size it.
    #[test]
    fn requests_and_grants_convert_through_units() {
        let grouped = Profile::GROUPED;
        assert_eq!(request_units(grouped, 7, 3), (2 * 3 + 7, 3));
        assert_eq!(request_units(Profile::STOCK, 4, 1), (6 + 4 * 2, 8));
        assert_eq!(units_of(9 * UNIT + UNIT / 2), 9);
        assert_eq!(units_of(1), 1, "a grant never sizes below one unit");
        assert_eq!(split(grouped, units_of(13 * UNIT), 7, 3), (3, 7));
    }

    /// The grouped Flutter booking covers the measured peaks: the base
    /// holds the runner VM and the resident compiler at their coincident
    /// peak, and one worker's share holds the heaviest measured tester.
    #[test]
    fn flutter_grouped_booking_covers_measured_peaks() {
        let profile = Profile::FLUTTER_GROUPED;
        // Round-1 captures, 92-suite widget-test package: runner VM +
        // frontend_server together at 1.0 GB coincident; the heaviest
        // flutter_tester at 647 MB.
        assert!(profile.base as u64 * UNIT >= 1024 * 1024 * 1024);
        assert!(profile.per_worker as u64 * UNIT >= 647 * 1024 * 1024);
    }

    /// A recorded per-worker share refines the booking above its built-in
    /// seed, never below it: a record is the heaviest worker process a run
    /// measured, the seed the heaviest measured worker at seeding time.
    #[test]
    fn calibration_refines_upward_only() {
        let grouped = Profile::FLUTTER_GROUPED;
        assert_eq!(grouped.refine(None).per_worker, grouped.per_worker);
        assert_eq!(grouped.refine(Some(0)).per_worker, grouped.per_worker);
        assert_eq!(grouped.refine(Some(1)).per_worker, grouped.per_worker);
        assert_eq!(grouped.refine(Some(4)).per_worker, 4);
        // Every profile keeps its own seed as the floor.
        assert_eq!(
            Profile::STOCK.refine(Some(1)).per_worker,
            Profile::STOCK.per_worker
        );
        assert_eq!(Profile::FLUTTER.refine(Some(3)).per_worker, 3);
        assert_eq!(
            Profile::GROUPED.refine(Some(1)).per_worker,
            Profile::GROUPED.per_worker
        );
    }

    /// The run fold converges in one run: a run whose heaviest worker
    /// process is N units records N, not the halfway average, while a
    /// lighter run only decays the share halfway (one quiet run cannot
    /// undersize the next).
    #[test]
    fn worker_peak_converges_in_one_run() {
        let grouped = Profile::FLUTTER_GROUPED;
        // 3 units of heaviest tester over the 2-unit seed: 4 recorded,
        // where the round-2 half-life fold would have written 3.
        assert_eq!(grouped.absorb(4).per_worker, 4);
        // Decay from above stays halfway: 5 recorded, a 2-unit run moves
        // the share to 4, not to 2.
        let heavy = Profile {
            per_worker: 5,
            ..grouped
        };
        assert_eq!(heavy.absorb(2).per_worker, 4);
        // A measurement at the seed holds it.
        assert_eq!(grouped.absorb(2).per_worker, grouped.per_worker);
    }

    /// The measurement is the heaviest tester's own peak where the run
    /// spawned testers, and the trees' peak above every base spread across
    /// the workers where it did not (grouped `dart test`: workers are
    /// isolates); nothing sampled, nothing measured.
    #[test]
    fn measurement_prefers_the_worker_process() {
        let grouped = Profile::FLUTTER_GROUPED;
        let unit = UNIT;
        // One heavy tester (2 units) among four workers: recorded as 2,
        // where the tree average would dilute it below the seed.
        assert_eq!(
            grouped.measurement(6 * unit, 3 * unit / 2 + 1, 4, 1),
            Some(2)
        );
        // No testers (dart isolates): the round-2 tree division.
        assert_eq!(grouped.measurement(6 * unit, 0, 4, 1), Some(1));
        // Seven workers over two shard processes, twelve units of tree:
        // twelve above the two bases, spread across seven, rounds to 2.
        assert_eq!(Profile::GROUPED.measurement(12 * unit, 0, 7, 2), Some(2));
        // Nothing sampled.
        assert_eq!(grouped.measurement(0, 0, 4, 1), None);
    }

    /// `PATH` directories for the lookup tests: `shim` holds a `flutter`
    /// link to this test binary (the `dartq init` shape), `sdk` and `other`
    /// a real `flutter` each, `dangling` a `flutter` link to nothing.
    struct Dirs {
        _tmp: tempfile::TempDir,
        shim: PathBuf,
        dangling: PathBuf,
        sdk: PathBuf,
        other: PathBuf,
    }

    fn dirs() -> Dirs {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = |name: &str| {
            let dir = tmp.path().join(name);
            std::fs::create_dir_all(&dir).expect("dir");
            dir
        };
        let shim = dir("shim");
        let dangling = dir("dangling");
        let sdk = dir("sdk");
        let other = dir("other");
        let exe = std::env::current_exe().expect("test binary");
        std::os::unix::fs::symlink(&exe, shim.join("flutter")).expect("shim link");
        std::os::unix::fs::symlink(tmp.path().join("gone"), dangling.join("flutter"))
            .expect("dangling link");
        std::fs::write(sdk.join("flutter"), "").expect("sdk tool");
        std::fs::write(other.join("flutter"), "").expect("other tool");
        Dirs {
            _tmp: tmp,
            shim,
            dangling,
            sdk,
            other,
        }
    }

    /// The running binary's canonical path, as [`find`] resolves it.
    fn this() -> PathBuf {
        std::env::current_exe()
            .and_then(std::fs::canonicalize)
            .expect("test binary resolves")
    }

    /// The `dartq init` state: a `flutter` link to this very binary first
    /// on `PATH` is passed over for the real tool behind it.
    #[test]
    fn find_skips_a_link_to_this_binary() {
        let dirs = dirs();
        let found = scan_in(
            [dirs.shim.clone(), dirs.sdk.clone()],
            "flutter",
            Some(this().as_path()),
        );
        assert_eq!(found, Some(dirs.sdk.join("flutter")));
    }

    /// With nothing resolving to this binary the scan is `which`'s: the
    /// first directory holding a file of the name wins, and a link that
    /// resolves nowhere neither matches nor stops the scan.
    #[test]
    fn find_without_a_self_match_takes_the_first_file() {
        let dirs = dirs();
        let found = scan_in(
            [dirs.dangling.clone(), dirs.sdk.clone(), dirs.other.clone()],
            "flutter",
            Some(this().as_path()),
        );
        assert_eq!(found, Some(dirs.sdk.join("flutter")));
        let alone = scan_in([dirs.dangling.clone()], "flutter", Some(this().as_path()));
        assert_eq!(alone, None);
    }

    /// Where the running binary does not resolve, nothing is skipped: the
    /// lookup degrades to the plain scan, link to this binary included.
    #[test]
    fn find_without_this_binary_skips_nothing() {
        let dirs = dirs();
        let found = scan_in([dirs.shim.clone(), dirs.sdk.clone()], "flutter", None);
        assert_eq!(found, Some(dirs.shim.join("flutter")));
    }
}
