//! Process-tree accounting for the analyzer faces: the summed RSS of a
//! process and every descendant, and the SIGKILL of such a tree.
//!
//! The official analyzer's memory lives in one process tree: the analysis
//! server, and on SDKs whose server spawns helpers, its descendants.
//! Analyzer plugins of the `analysis_server_plugin` system are isolates
//! inside the server process, so the server's own RSS already carries
//! them. Both functions read one `ps` snapshot per call; callers sample
//! only while an analysis runs, so an idle resident server costs no
//! sampling at all.

use std::process::Command;

/// `(pid, ppid, rss KiB)` for every process, from one `ps` snapshot.
/// Empty when `ps` cannot answer.
pub(crate) fn table() -> Vec<(u32, u32, u64)> {
    let Ok(output) = Command::new("ps")
        .args(["-axo", "pid=,ppid=,rss="])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((
                fields.next()?.parse().ok()?,
                fields.next()?.parse().ok()?,
                fields.next()?.parse().ok()?,
            ))
        })
        .collect()
}

/// `root` followed by every descendant in `rows`, breadth first.
fn tree(rows: &[(u32, u32, u64)], root: u32) -> Vec<u32> {
    tree_excluding(rows, root, &[])
}

/// [`tree`], not descending into any pid of `stop` other than `root`: the
/// subtree of another accounted root is that root's to count.
pub(crate) fn tree_excluding(rows: &[(u32, u32, u64)], root: u32, stop: &[u32]) -> Vec<u32> {
    let mut members = vec![root];
    let mut next = 0;
    while let Some(&pid) = members.get(next) {
        next += 1;
        members.extend(
            rows.iter()
                .filter(|&&(child, ppid, _)| ppid == pid && child != pid && !stop.contains(&child))
                .map(|&(child, _, _)| child),
        );
    }
    members
}

/// The summed RSS in bytes of `members` in `rows`.
pub(crate) fn rss_of(rows: &[(u32, u32, u64)], members: &[u32]) -> u64 {
    members
        .iter()
        .filter_map(|pid| rows.iter().find(|row| row.0 == *pid))
        .map(|&(_, _, rss)| rss.saturating_mul(1024))
        .sum()
}

/// The summed RSS in bytes of `root` and every descendant. `0` when `ps`
/// cannot answer: a budget never trips on a blind host (fail-open, the
/// run queue's posture).
#[must_use]
pub fn tree_rss_bytes(root: u32) -> u64 {
    let rows = table();
    rss_of(&rows, &tree(&rows, root))
}

/// SIGKILLs every descendant of `root`, then `root`. Best effort: a pid
/// that already exited is skipped by the kernel; when `ps` cannot answer
/// only `root` is signaled.
pub fn kill_tree(root: u32) {
    let rows = table();
    let members = if rows.is_empty() {
        vec![root]
    } else {
        tree(&rows, root)
    };
    for &pid in members.iter().rev() {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            continue;
        };
        if pid > 1 {
            // SAFETY: signal delivery to a pid of a tree this process
            // spawned; a recycled pid is the accepted best-effort risk of
            // every pid-based reaper.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }
}

/// The machine's physical memory in bytes; `0` when the host does not say.
#[must_use]
pub fn physical_memory() -> u64 {
    // SAFETY: sysconf only reads configuration values.
    let (pages, page) = unsafe {
        (
            libc::sysconf(libc::_SC_PHYS_PAGES),
            libc::sysconf(libc::_SC_PAGESIZE),
        )
    };
    match (u64::try_from(pages), u64::try_from(page)) {
        (Ok(pages), Ok(page)) => pages.saturating_mul(page),
        _ => 0,
    }
}

/// The peak RSS in bytes of the heaviest child process this process has
/// waited for (descendants its children waited for included):
/// `getrusage(RUSAGE_CHILDREN)`. A peak of one process, not of a tree, at
/// no sampling cost; `0` when the host does not say.
#[must_use]
pub fn children_peak_rss() -> u64 {
    rusage_peak(libc::RUSAGE_CHILDREN)
}

fn rusage_peak(who: libc::c_int) -> u64 {
    // SAFETY: getrusage fills the zeroed struct it is handed.
    let usage = unsafe {
        let mut usage = std::mem::zeroed::<libc::rusage>();
        if libc::getrusage(who, &raw mut usage) != 0 {
            return 0;
        }
        usage
    };
    let max = u64::try_from(usage.ru_maxrss).unwrap_or(0);
    // Bytes on macOS, KiB elsewhere.
    if cfg!(target_os = "macos") {
        max
    } else {
        max.saturating_mul(1024)
    }
}

/// The peak RSS in bytes of this process itself:
/// `getrusage(RUSAGE_SELF)`; `0` when the host does not say.
#[must_use]
pub fn self_peak_rss() -> u64 {
    rusage_peak(libc::RUSAGE_SELF)
}

/// Whether `pid` names a live process (`kill(pid, 0)`); pid 0 and pids
/// past `pid_t` are never alive.
#[must_use]
pub fn alive(pid: u32) -> bool {
    match libc::pid_t::try_from(pid) {
        // SAFETY: signal 0 performs the existence check only.
        Ok(pid) if pid > 0 => unsafe { libc::kill(pid, 0) == 0 },
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tree walk collects grandchildren and ignores other branches.
    #[test]
    fn tree_collects_every_descendant_only() {
        let rows = [
            (10, 1, 5),
            (11, 10, 5),
            (12, 11, 5),
            (20, 1, 5),
            (13, 10, 5),
        ];
        let mut members = tree(&rows, 10);
        members.sort_unstable();
        assert_eq!(members, [10, 11, 12, 13]);
    }
}
