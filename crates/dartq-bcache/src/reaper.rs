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
//! pid 1 or by one of this process's own live ancestors — either marks
//! a parent that is no longer the frontend_server's flutter_tools: the
//! kernel reparents an orphan to the nearest subreaper up the tree,
//! or pid 1 where there is none, and every subreaper above a process
//! we spawned is by the same tree one of our own ancestors; the
//! frontend_server of a concurrent session is still owned by a live
//! dart/IDE process outside our tree, so it stays protected, (b) names
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

/// True when `ppid` names a parent that no longer owns the process:
/// pid 1 (the classic orphan reparenting target) or any live ancestor
/// of this process. A host with subreapers — CI runners set
/// `PR_SET_CHILD_SUBREAPER` on a process up every tree they own —
/// parks an orphan with the nearest subreaper instead of pid 1, and
/// that subreaper is by construction one of our ancestors, so the two
/// legs together cover both reparenting targets. A frontend_server
/// some live dart/IDE process outside this tree still owns is neither,
/// so it stays protected.
fn is_orphan_ppid(ppid: libc::pid_t, ancestors: &[libc::pid_t]) -> bool {
    ppid == 1 || ancestors.contains(&ppid)
}

/// The pids of this process's live strict ancestors, nearest first,
/// pid 1 included when the walk reaches it. Linux reads them from
/// `/proc/<pid>/stat`; macOS has no `/proc`, but it also has no
/// subreapers — launchd (pid 1) adopts every orphan — so `ppid == 1`
/// alone is the complete rule there and the walk stays Linux-only.
#[cfg(target_os = "linux")]
fn live_ancestor_ppids() -> Vec<libc::pid_t> {
    let mut ancestors = Vec::new();
    let mut current = std::process::id() as libc::pid_t;
    loop {
        // A vanished /proc entry is a parent that died mid-walk (the
        // chain above it is gone with it); ppid 0 is the forest's root
        // past pid 1; a repeat pid is a guard against a cycle the
        // kernel's own forest shape should never allow.
        let Some(ppid) = stat_ppid(current) else {
            break;
        };
        if ppid == 0 || ancestors.contains(&ppid) {
            break;
        }
        ancestors.push(ppid);
        if ppid == 1 {
            break;
        }
        current = ppid;
    }
    ancestors
}

#[cfg(not(target_os = "linux"))]
fn live_ancestor_ppids() -> Vec<libc::pid_t> {
    Vec::new()
}

/// Field 4 (ppid) of one `/proc/<pid>/stat`. The comm field (field 2)
/// can contain spaces and parentheses, so the parse anchors on the
/// last `)`: everything after it is state (field 3), then ppid.
#[cfg(target_os = "linux")]
fn stat_ppid(pid: libc::pid_t) -> Option<libc::pid_t> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let mut fields = stat.rsplit_once(')')?.1.split_whitespace();
    let _state = fields.next()?;
    fields.next()?.parse().ok()
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
    let ancestors = live_ancestor_ppids();
    let table = String::from_utf8_lossy(&output.stdout);
    let pids: Vec<_> = table
        .lines()
        .filter_map(parse_row)
        .filter(|row| {
            is_orphan_ppid(row.ppid, &ancestors)
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
    /// names frontend_server, and once the spawning shell exits the
    /// kernel parks it with pid 1 or, on a subreaper host (the CI
    /// runner), with the nearest subreaper up our own tree.
    /// The per-binary tag keeps the poll from adopting a leftover
    /// fixture of an earlier, failed run.
    fn spawn_fixture() -> (u32, String) {
        let tag = format!("frontend_server_fixture_{}", std::process::id());
        // `bash`, not `sh`: the argv rewrite rides on `exec -a`, a
        // bash extension — dash (ubuntu's /bin/sh) rejects the flag
        // and the fixture never starts. CI images and macOS both ship
        // bash.
        Command::new("bash")
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
                // The sweep's own orphan predicate, so the fixture is
                // adopted exactly when the product would reap it.
                if super::is_orphan_ppid(ppid as libc::pid_t, &super::live_ancestor_ppids()) {
                    return (pid, tag);
                }
            }
            assert!(
                SystemTime::now() < deadline,
                "fixture never reparented to an orphan-holding parent"
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
