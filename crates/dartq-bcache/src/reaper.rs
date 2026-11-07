//! The orphaned frontend_server reaper (#4100).
//!
//! A flutter_tools defect measured on 3.44.0: every modeled `flutter
//! build|run|test` left one `dartaotruntime … frontend_server[_aot]
//! .dart.snapshot …` process behind when it exited — reparented to pid
//! 1, idle, and leaked for good (flutter_tools never waited on it; the
//! 3.44.9 and 3.47.9 SDKs on hand do not reproduce it, so the sweep
//! finds nothing there and costs one `ps`). One leak per build is
//! invisible and one per minute is a dead machine, so after each run's
//! child exits the shim sweeps once: enumerate the process table with a
//! single `ps` spawn and SIGTERM every process that (a) is parented by
//! pid 1 — a live frontend_server is always a child of its
//! flutter_tools, so this alone separates leaks from the one a
//! concurrent flutter session is still using, (b) names
//! `frontend_server` in its command line (the substring covers both the
//! JIT snapshot and the `_aot` form), and (c) is no older than the run
//! — compared through `ps`'s whole-second elapsed-time column against
//! the run's own elapsed time, which admits at most a same-second
//! process and only widens the sweep, never narrows it. `ps` missing or
//! failing is noted and never fatal; so is a kill that lost the race
//! with an already-dead pid. The sweep changes no process group and
//! sends no signal to anything still owned by a live parent — the
//! interactive `run` face's tty semantics are untouched.

use std::fmt::Display;
use std::process::Command;
use std::time::SystemTime;

fn note(message: &(impl Display + ?Sized)) {
    eprintln!("dartq-bcache: {message}");
}

/// One parsed `ps` row.
struct ProcessRow<'a> {
    pid: libc::pid_t,
    ppid: libc::pid_t,
    elapsed_secs: u64,
    command: &'a str,
}

/// Splits one `ps -axo pid=,ppid=,etime=,command=` line; `None` for the
/// header-less blank or any row ps formats differently than asked.
fn parse_row(line: &str) -> Option<ProcessRow<'_>> {
    let mut rest = line.trim_start();
    let mut fields: [&str; 3] = ["", "", ""];
    for field in &mut fields {
        let (head, tail) = rest.split_once(char::is_whitespace)?;
        *field = head;
        rest = tail.trim_start();
    }
    Some(ProcessRow {
        pid: fields[0].parse().ok()?,
        ppid: fields[1].parse().ok()?,
        elapsed_secs: etime_secs(fields[2])?,
        command: rest,
    })
}

/// `ps` elapsed time, `[[dd-]hh:]mm:ss`, to whole seconds.
fn etime_secs(etime: &str) -> Option<u64> {
    let (days, clock) = match etime.split_once('-') {
        Some((days, clock)) => (days.parse::<u64>().ok()?, clock),
        None => (0, etime),
    };
    let clock_secs = clock.split(':').try_fold(0u64, |total, field| {
        Some(
            total
                .checked_mul(60)?
                .checked_add(field.parse::<u64>().ok()?)?,
        )
    })?;
    Some(days.checked_mul(86_400)?.checked_add(clock_secs)?)
}

/// SIGTERMs every orphaned frontend_server this run could have leaked
/// and returns their pids, `run_started` being the moment the child was
/// spawned. One `ps` spawn, one stderr line per run that reaped
/// anything; every failure is a note, never an error.
pub fn reap_orphaned_frontend_servers(run_started: SystemTime) -> Vec<libc::pid_t> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,ppid=,etime=,command="])
        .output();
    let Ok(output) = output else {
        note("orphan sweep skipped: ps could not be spawned");
        return Vec::new();
    };
    if !output.status.success() {
        note(&format!(
            "orphan sweep skipped: ps exited {}",
            output.status.code().unwrap_or(-1)
        ));
        return Vec::new();
    }
    let run_elapsed = SystemTime::now()
        .duration_since(run_started)
        .map_or(0, |span| span.as_secs());
    let table = String::from_utf8_lossy(&output.stdout);
    let pids: Vec<_> = table
        .lines()
        .filter_map(parse_row)
        .filter(|row| {
            row.ppid == 1
                && row.command.contains("frontend_server")
                && row.elapsed_secs <= run_elapsed
        })
        .map(|row| row.pid)
        .collect();
    let reaped: Vec<_> = pids
        .iter()
        .copied()
        .filter(|&pid| unsafe { libc::kill(pid, libc::SIGTERM) } == 0)
        .collect();
    if !reaped.is_empty() {
        let listed = reaped
            .iter()
            .map(|pid| pid.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        note(&format!(
            "reaped {} orphaned frontend_server process(es) flutter left behind (pids {listed})",
            reaped.len()
        ));
    }
    reaped
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::thread;
    use std::time::{Duration, SystemTime};

    use super::etime_secs;

    /// One fixture: a disowned `frontend_server_fixture_<pid>` process
    /// the reaper must see exactly like flutter's real leak — argv
    /// names frontend_server, ppid is 1 once the spawning shell exits.
    /// The per-binary tag keeps the poll from adopting a leftover
    /// fixture of an earlier, failed run.
    fn spawn_fixture() -> (u32, String) {
        let tag = format!("frontend_server_fixture_{}", std::process::id());
        Command::new("sh")
            .arg("-c")
            .arg(format!("exec -a {tag} sleep 60 &"))
            .status()
            .expect("spawn fixture");
        let deadline = SystemTime::now() + Duration::from_secs(10);
        loop {
            let Ok(table) = Command::new("ps")
                .args(["-axo", "pid=,ppid=,command="])
                .output()
            else {
                panic!("ps");
            };
            let text = String::from_utf8_lossy(&table.stdout);
            for line in text.lines() {
                if !line.contains(&tag) {
                    continue;
                }
                let mut fields = line.split_whitespace();
                let pid: u32 = fields.next().unwrap().parse().unwrap();
                let ppid: u32 = fields.next().unwrap().parse().unwrap();
                if ppid == 1 {
                    return (pid, tag);
                }
            }
            assert!(
                SystemTime::now() < deadline,
                "fixture never reparented to pid 1"
            );
            thread::sleep(Duration::from_millis(100));
        }
    }

    fn alive(pid: u32) -> bool {
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }

    #[test]
    fn etime_parses_every_shape_ps_prints() {
        assert_eq!(etime_secs("00:37"), Some(37));
        assert_eq!(etime_secs("12:03:04"), Some(43_384));
        assert_eq!(etime_secs("1-02:03:04"), Some(93_784));
        assert_eq!(etime_secs("junk"), None);
    }

    /// The sweep reaps a run-aged orphan, leaves a pre-run process
    /// alone, and a second pass finds nothing.
    #[test]
    fn reaper_takes_only_run_aged_orphans() {
        let (pid, tag) = spawn_fixture();
        // The age gate sees whole seconds, so "older than the run" must
        // be older by a full column second — the same-second slack the
        // sweep admits by design (it widens the window, never narrows).
        // The margin also covers a loaded ps: the gate compares against
        // the run's own elapsed time, taken after ps returns.
        thread::sleep(Duration::from_secs(3));

        // The fixture predates this "run" — excluded by the age gate.
        let started_now = SystemTime::now();
        assert!(
            !super::reap_orphaned_frontend_servers(started_now).contains(&(pid as libc::pid_t)),
            "a process older than the run is never touched"
        );
        assert!(alive(pid), "fixture {pid} ({tag}) died un-swept");

        // The same fixture inside a run that began an hour ago: gone.
        let an_hour_ago = SystemTime::now() - Duration::from_secs(3_600);
        let reaped = super::reap_orphaned_frontend_servers(an_hour_ago);
        assert!(reaped.contains(&(pid as libc::pid_t)));
        let deadline = SystemTime::now() + Duration::from_secs(10);
        while alive(pid) {
            assert!(SystemTime::now() < deadline, "fixture survived SIGTERM");
            thread::sleep(Duration::from_millis(50));
        }

        // Nothing left for a second pass.
        assert!(
            !super::reap_orphaned_frontend_servers(an_hour_ago).contains(&(pid as libc::pid_t))
        );
    }
}
