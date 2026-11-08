//! The cross-process run queue: executions run first come, first served,
//! machine-wide.
//!
//! The worktrees of one monorepo — parallel agents, each running `dartq
//! test` — each spawn a toolchain (a `frontend_server` compile, test
//! processes) whose memory nothing bounds, and together they wedge the
//! host. The queue caps how many executions run at once; the rest wait
//! their turn in arrival order.
//!
//! No daemon owns the queue — the file system and `flock(2)` do, so a run
//! leaves it however it ends:
//!
//! * **Joining.** A contender creates `t-<seq>-<rand>.lock` under
//!   `<root>/queue/<name>/tickets/`, `seq` one past the highest present
//!   (two racers may read the same one; the random suffix keeps their
//!   files apart), and holds an exclusive lock on it for its whole queue
//!   life, waiting and running alike.
//! * **Classifying.** Every 200 ms a waiter orders the tickets by (seq,
//!   rand) and probes each with one non-blocking lock attempt on a fresh
//!   descriptor, closed at once. A ticket is *active* while its lock is
//!   held; *pending* while unlocked and younger than 60 s — a contender
//!   between creating its file and locking it, counted so nobody is
//!   admitted past it; *dead* once unlocked and 60 s old. Nothing writes a
//!   ticket and a lock never touches it, so its mtime is its creation.
//! * **Admission.** A contender runs once fewer than the cap of active or
//!   pending tickets precede its own, and holds its lock until it ends.
//! * **Release.** Dropping the guard unlinks the ticket, then closes the
//!   descriptor: an unlinked file is invisible to every scan, so a
//!   released ticket never lingers as a pending barrier.
//! * **Collection.** A dead ticket is a holder that exited without
//!   releasing — killed or crashed. Whoever finds one reports it first
//!   ([`Event::Dead`]: the dead run's children may outlive it), then
//!   unlinks it.
//!
//! The module is unix-only: elsewhere there is no `flock`, and callers run
//! unqueued — the queue is best-effort there.

use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

/// How often a waiter re-checks its admission.
const POLL: Duration = Duration::from_millis(200);

/// The queue's clock: how old an unlocked ticket must be to be dead, and
/// how often a waiter repeats that it still waits. A parameter so tests
/// need not sit out a minute or ten.
#[derive(Clone, Copy)]
struct Clock {
    stale: Duration,
    notice: Duration,
}

const CLOCK: Clock = Clock {
    stale: Duration::from_secs(60),
    notice: Duration::from_secs(10 * 60),
};

/// What a contender reports to its caller on the way in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The first poll that did not admit: `ahead` active or pending
    /// tickets precede this one. Reported once.
    Queued { ahead: usize },
    /// Every ten minutes of waiting after that: `active` tickets ahead
    /// still hold their locks; the contender joined `waited` ago.
    Still { active: usize, waited: Duration },
    /// Dead tickets were found: holders gone without releasing — a
    /// SIGKILL, an OOM kill — whose children (compilers, test processes)
    /// may outlive them, reparented and still holding their memory.
    /// `created` is the oldest such ticket's creation: everything those
    /// runs spawned is younger. Reported before the tickets are collected
    /// and before admission is decided, so the caller can sweep the
    /// orphans before its own run starts beside them.
    Dead { created: SystemTime },
}

/// An admitted contender's place in the queue: its ticket and the lock on
/// it, held until the guard drops.
#[derive(Debug)]
#[must_use = "dropping the guard leaves the queue at once"]
pub struct RunGuard {
    ticket: PathBuf,
    _lock: File,
    waited: Duration,
}

impl RunGuard {
    /// How long the contender waited between joining and admission.
    #[must_use]
    pub fn waited(&self) -> Duration {
        self.waited
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        // Unlink while the lock is still held: a released file left in
        // place would read as a fresh unlocked ticket — a pending barrier
        // — for a minute. A failed unlink leaves residue the queue
        // collects once it is dead. The descriptor closes after this
        // body, releasing the lock.
        let _ = fs::remove_file(&self.ticket);
    }
}

/// Joins the queue `name` under `root` and blocks until fewer than
/// `max_concurrent` (at least one) tickets ahead of this one are active
/// or pending. The guard holds the place until it drops. Waiting has no
/// timeout: a run ahead that takes hours is a real run.
///
/// # Errors
///
/// The ticket directory or the ticket cannot be created, locked or
/// scanned; the ticket is removed again before the error returns.
pub fn acquire(name: &str, root: &Path, max_concurrent: usize) -> io::Result<RunGuard> {
    acquire_with(name, root, max_concurrent, |_| {})
}

/// [`acquire`], reporting the way in through `on_event` — the hook a face
/// prints its waiting lines and sweeps a dead run's orphans from.
///
/// # Errors
///
/// As [`acquire`].
pub fn acquire_with(
    name: &str,
    root: &Path,
    max_concurrent: usize,
    on_event: impl FnMut(Event),
) -> io::Result<RunGuard> {
    acquire_in(name, root, max_concurrent, CLOCK, on_event)
}

/// [`acquire_with`] on an explicit [`Clock`].
fn acquire_in(
    name: &str,
    root: &Path,
    max_concurrent: usize,
    clock: Clock,
    on_event: impl FnMut(Event),
) -> io::Result<RunGuard> {
    let dir = root.join("queue").join(name).join("tickets");
    fs::create_dir_all(&dir)?;
    let mine = join(&dir)?;
    let path = dir.join(mine.file_name());
    // A contender that fails once joined takes its ticket with it rather
    // than leave a pending barrier behind for a minute.
    wait(&dir, mine, &path, max_concurrent.max(1), clock, on_event).inspect_err(|_| {
        let _ = fs::remove_file(&path);
    })
}

/// Locks `mine` (the file at `path`) and polls until fewer than `max`
/// active or pending tickets precede it.
fn wait(
    dir: &Path,
    mine: Ticket,
    path: &Path,
    max: usize,
    clock: Clock,
    mut on_event: impl FnMut(Event),
) -> io::Result<RunGuard> {
    let joined = Instant::now();
    // Another contender's probe may hold the ticket for an instant: a
    // refused lock waits one poll and tries again.
    let lock = loop {
        let file = File::open(path)?;
        match file.try_lock() {
            Ok(()) => break file,
            Err(TryLockError::WouldBlock) => thread::sleep(POLL),
            Err(TryLockError::Error(error)) => return Err(error),
        }
    };
    let mut queued = false;
    let mut notice = joined + clock.notice;
    loop {
        let tickets = scan(dir, clock.stale)?;
        let oldest_dead = tickets
            .iter()
            .filter_map(|found| match found.state {
                State::Dead(created) => Some(created),
                State::Active | State::Pending => None,
            })
            .min();
        if let Some(created) = oldest_dead {
            on_event(Event::Dead { created });
            for found in &tickets {
                if matches!(found.state, State::Dead(_)) {
                    // Best effort: an unlink that fails is retried next pass.
                    let _ = fs::remove_file(dir.join(found.ticket.file_name()));
                }
            }
        }
        // `mine` is always present: its own lock keeps it active.
        let ahead = &tickets[..tickets
            .iter()
            .position(|found| found.ticket == mine)
            .unwrap_or(tickets.len())];
        let waiting = ahead
            .iter()
            .filter(|found| !matches!(found.state, State::Dead(_)))
            .count();
        if waiting < max {
            return Ok(RunGuard {
                ticket: path.to_path_buf(),
                _lock: lock,
                waited: joined.elapsed(),
            });
        }
        if !queued {
            queued = true;
            on_event(Event::Queued { ahead: waiting });
        } else if Instant::now() >= notice {
            notice = Instant::now() + clock.notice;
            on_event(Event::Still {
                active: ahead
                    .iter()
                    .filter(|found| found.state == State::Active)
                    .count(),
                waited: joined.elapsed(),
            });
        }
        thread::sleep(POLL);
    }
}

/// A ticket's state (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Active,
    Pending,
    /// Unlocked and stale; the ticket's creation time.
    Dead(SystemTime),
}

/// One ticket as a scan found it.
struct Found {
    ticket: Ticket,
    state: State,
}

/// Every ticket under `dir` in queue order, each classified; a ticket
/// that vanishes mid-scan is skipped.
fn scan(dir: &Path, stale: Duration) -> io::Result<Vec<Found>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let Some(ticket) = Ticket::parse(&entry.file_name()) else {
            continue;
        };
        if let Some(state) = classify(&entry.path(), stale)? {
            found.push(Found { ticket, state });
        }
    }
    found.sort_unstable_by_key(|found| found.ticket);
    Ok(found)
}

/// One ticket's state from a probe: a fresh descriptor, one non-blocking
/// lock attempt, closed on return. `None` when the ticket is gone.
fn classify(path: &Path, stale: Duration) -> io::Result<Option<State>> {
    let probe = match File::open(path) {
        Ok(probe) => probe,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    match probe.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(Some(State::Active)),
        Err(TryLockError::Error(error)) => return Err(error),
    }
    let created = probe.metadata()?.modified()?;
    Ok(Some(if created.elapsed().unwrap_or_default() >= stale {
        State::Dead(created)
    } else {
        State::Pending
    }))
}

/// Creates this contender's ticket: one past the highest seq present, and
/// a random suffix so racers that read the same seq still make distinct
/// files. `create_new` refuses an existing name; a collision draws again.
fn join(dir: &Path) -> io::Result<Ticket> {
    loop {
        let mut seq = 0;
        for entry in fs::read_dir(dir)? {
            if let Some(ticket) = Ticket::parse(&entry?.file_name()) {
                seq = seq.max(ticket.seq);
            }
        }
        let ticket = Ticket {
            seq: seq + 1,
            rand: random_u64(),
        };
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(ticket.file_name()))
        {
            Ok(_) => return Ok(ticket),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
}

/// Eight bytes of the system's entropy pool; without `/dev/urandom`, the
/// process id and the clock — still distinct for every contender that
/// could race this one.
pub(crate) fn random_u64() -> u64 {
    let mut bytes = [0; 8];
    if File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut bytes))
        .is_ok()
    {
        return u64::from_ne_bytes(bytes);
    }
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    (u64::from(std::process::id()) << 32) | u64::from(nanos)
}

/// A ticket's identity, ordered as the queue is: seq, then the random
/// suffix that breaks a tie between racers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Ticket {
    seq: u64,
    rand: u64,
}

impl Ticket {
    /// The ticket a file name spells (`t-<seq>-<rand>.lock`); `None` for
    /// any other file.
    fn parse(name: &OsStr) -> Option<Self> {
        let (seq, rand) = name
            .to_str()?
            .strip_prefix("t-")?
            .strip_suffix(".lock")?
            .split_once('-')?;
        Some(Self {
            seq: seq.parse().ok()?,
            rand: rand.parse().ok()?,
        })
    }

    fn file_name(self) -> String {
        format!("t-{}-{}.lock", self.seq, self.rand)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    use super::*;

    /// Several polls: long enough for a contender that could be admitted
    /// to have been.
    const SETTLE: Duration = Duration::from_millis(600);
    /// The bound on every wait a passing test performs.
    const BOUND: Duration = Duration::from_secs(10);

    /// The `test` queue's ticket directory under `root`.
    fn tickets(root: &Path) -> PathBuf {
        root.join("queue").join("test").join("tickets")
    }

    /// One contender waits while another holds the only place, and is
    /// admitted once the holder drops it.
    #[test]
    fn serializes_two_contenders_in_one_process() {
        let root = tempfile::tempdir().expect("queue root");
        let (holder_admitted, holder_in) = mpsc::channel();
        let (release, held) = mpsc::channel::<()>();
        let path = root.path().to_path_buf();
        let holder = thread::spawn(move || {
            let guard = acquire("test", &path, 1).expect("the holder is admitted");
            holder_admitted.send(()).expect("report the admission");
            held.recv().expect("hold until released");
            drop(guard);
        });
        holder_in
            .recv_timeout(BOUND)
            .expect("the holder is admitted");
        let (waiter_admitted, waiter_in) = mpsc::channel();
        let path = root.path().to_path_buf();
        let waiter = thread::spawn(move || {
            let guard = acquire("test", &path, 1).expect("the waiter is admitted");
            waiter_admitted.send(()).expect("report the admission");
            drop(guard);
        });
        assert!(
            waiter_in.recv_timeout(SETTLE).is_err(),
            "the second contender waits while the holder keeps the only place"
        );
        release.send(()).expect("release the holder");
        waiter_in
            .recv_timeout(BOUND)
            .expect("the second contender is admitted once the holder drops");
        holder.join().expect("holder");
        waiter.join().expect("waiter");
    }

    /// With two places, two contenders run at once and the third waits
    /// until one of them leaves.
    #[test]
    fn admits_up_to_max_concurrent() {
        let root = tempfile::tempdir().expect("queue root");
        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let (admitted, admissions) = mpsc::channel();
        let mut releases = Vec::new();
        let mut contenders = Vec::new();
        for index in 0..3 {
            let (release, held) = mpsc::channel::<()>();
            releases.push(release);
            let path = root.path().to_path_buf();
            let (live, peak, admitted) = (Arc::clone(&live), Arc::clone(&peak), admitted.clone());
            contenders.push(thread::spawn(move || {
                let guard = acquire("test", &path, 2).expect("every contender is admitted");
                peak.fetch_max(live.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                admitted.send(index).expect("report the admission");
                held.recv().expect("hold until released");
                live.fetch_sub(1, Ordering::SeqCst);
                drop(guard);
            }));
        }
        let first = admissions.recv_timeout(BOUND).expect("a first admission");
        let second = admissions.recv_timeout(BOUND).expect("a second admission");
        assert!(
            admissions.recv_timeout(SETTLE).is_err(),
            "the third contender waits while two run"
        );
        assert_eq!(peak.load(Ordering::SeqCst), 2, "two run at once");
        releases[first].send(()).expect("release one runner");
        let third = admissions
            .recv_timeout(BOUND)
            .expect("the third contender is admitted once one leaves");
        let mut order = [first, second, third];
        order.sort_unstable();
        assert_eq!(order, [0, 1, 2], "each contender is admitted once");
        for index in [second, third] {
            releases[index].send(()).expect("release the rest");
        }
        for contender in contenders {
            contender.join().expect("contender");
        }
        assert_eq!(
            peak.load(Ordering::SeqCst),
            2,
            "never more than two at once"
        );
    }

    /// A ticket unlocked and past the stale age is a dead holder: the
    /// contender reports it, collects it and is admitted straight past it.
    #[test]
    fn skips_a_dead_ticket_and_gcs_it() {
        let root = tempfile::tempdir().expect("queue root");
        let dir = tickets(root.path());
        fs::create_dir_all(&dir).expect("tickets dir");
        let dead = dir.join("t-1-1.lock");
        fs::write(&dead, b"").expect("a dead holder's ticket");
        let created = fs::metadata(&dead)
            .and_then(|meta| meta.modified())
            .expect("mtime");
        let mut events = Vec::new();
        let clock = Clock {
            stale: Duration::ZERO,
            ..CLOCK
        };
        let guard = acquire_in("test", root.path(), 1, clock, |event| events.push(event))
            .expect("admitted past the dead ticket");
        assert_eq!(
            events,
            [Event::Dead { created }],
            "reported, and never queued behind it"
        );
        assert!(!dead.exists(), "the dead ticket is collected");
        drop(guard);
        assert_eq!(
            fs::read_dir(&dir).expect("tickets dir").count(),
            0,
            "the guard takes its own ticket along when it drops"
        );
    }

    /// A fresh unlocked ticket is a contender between creating its file
    /// and locking it: it counts, so nobody is admitted past it.
    #[test]
    fn a_fresh_unlocked_ticket_blocks_instead_of_being_skipped() {
        let root = tempfile::tempdir().expect("queue root");
        let dir = tickets(root.path());
        fs::create_dir_all(&dir).expect("tickets dir");
        let pending = dir.join("t-1-1.lock");
        fs::write(&pending, b"").expect("a contender setting up");
        let (admitted, admissions) = mpsc::channel();
        let path = root.path().to_path_buf();
        let waiter = thread::spawn(move || {
            let guard = acquire("test", &path, 1).expect("admitted once the barrier clears");
            admitted.send(()).expect("report the admission");
            drop(guard);
        });
        assert!(
            admissions.recv_timeout(SETTLE).is_err(),
            "the pending ticket holds the only place"
        );
        fs::remove_file(&pending).expect("the barrier clears");
        admissions
            .recv_timeout(BOUND)
            .expect("admitted once the barrier is gone");
        waiter.join().expect("waiter");
    }

    /// A holder that dies without releasing leaves its ticket unlocked:
    /// the waiter reports it dead — the moment to sweep the dead run's
    /// orphans — before it is admitted past it.
    #[test]
    fn reports_a_crashed_holder_before_admitting_past_it() {
        let root = tempfile::tempdir().expect("queue root");
        let dir = tickets(root.path());
        fs::create_dir_all(&dir).expect("tickets dir");
        let crashed = dir.join("t-1-1.lock");
        let holder = File::create(&crashed).expect("the holder's ticket");
        holder.lock().expect("the holder runs");
        let created = holder
            .metadata()
            .and_then(|meta| meta.modified())
            .expect("mtime");
        let (events, seen) = mpsc::channel();
        let (admitted, admissions) = mpsc::channel();
        let path = root.path().to_path_buf();
        let clock = Clock {
            stale: Duration::ZERO,
            ..CLOCK
        };
        let waiter = thread::spawn(move || {
            let guard = acquire_in("test", &path, 1, clock, |event| {
                events.send(event).expect("report the event");
            })
            .expect("admitted past the crashed holder");
            admitted.send(()).expect("report the admission");
            drop(guard);
        });
        assert_eq!(
            seen.recv_timeout(BOUND).expect("queued"),
            Event::Queued { ahead: 1 }
        );
        // The holder dies: its lock goes with its descriptor, its file stays.
        drop(holder);
        assert_eq!(
            seen.recv_timeout(BOUND)
                .expect("the dead holder is reported"),
            Event::Dead { created }
        );
        admissions
            .recv_timeout(BOUND)
            .expect("admitted past the dead holder");
        assert!(!crashed.exists(), "the dead ticket is collected");
        waiter.join().expect("waiter");
    }

    /// A waiter repeats that it still waits every notice period, counting
    /// the runs that still hold their places ahead of it.
    #[test]
    fn repeats_the_waiting_report_every_notice_period() {
        let root = tempfile::tempdir().expect("queue root");
        let holder = acquire("test", root.path(), 1).expect("the holder is admitted");
        let (events, seen) = mpsc::channel();
        let path = root.path().to_path_buf();
        let clock = Clock {
            notice: Duration::from_millis(250),
            ..CLOCK
        };
        let waiter = thread::spawn(move || {
            acquire_in("test", &path, 1, clock, |event| {
                // Reports past the ones this test reads go unread.
                let _ = events.send(event);
            })
            .map(drop)
        });
        assert_eq!(
            seen.recv_timeout(BOUND).expect("queued"),
            Event::Queued { ahead: 1 }
        );
        let mut last = Duration::ZERO;
        for _ in 0..2 {
            match seen.recv_timeout(BOUND).expect("a repeated report") {
                Event::Still { active, waited } => {
                    assert_eq!(active, 1, "the holder still runs ahead");
                    assert!(waited > last, "the wait grows report by report");
                    last = waited;
                }
                other => panic!("expected a still-waiting report, got {other:?}"),
            }
        }
        drop(holder);
        waiter
            .join()
            .expect("waiter")
            .expect("admitted once the holder leaves");
    }
}
