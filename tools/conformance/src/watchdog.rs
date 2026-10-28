//! Process execution under the resource guards: one analyzer process at a
//! time (the callers run chunks sequentially), a hard RSS ceiling and a
//! wall-clock timeout, both killing the child and reporting a crash.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const RSS_LIMIT_KB: u64 = 8 * 1024 * 1024; // 8 GB
pub const CHUNK_TIMEOUT: Duration = Duration::from_secs(30 * 60);

pub struct RunResult {
    /// Killed by the watchdog or exited non-JSON-crash is decided by the
    /// caller; this flag is the watchdog's kill decision.
    pub killed: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// Peak resident set size observed while running (KB).
    pub peak_rss_kb: u64,
}

/// Runs a child to completion under the watchdog. Polls `ps` once per
/// second; above the RSS ceiling or past the timeout the child is killed
/// and `killed` is set.
pub fn run(command: &mut Command, timeout: Duration, rss_limit_kb: u64) -> std::io::Result<RunResult> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let pid = child.id();
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let out_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });

    let started = Instant::now();
    let mut killed = false;
    let mut peak: u64 = 0;
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if let Some(rss) = current_rss_kb(pid) {
            peak = peak.max(rss);
            if rss > rss_limit_kb {
                kill(&mut child);
                killed = true;
                break;
            }
        }
        if started.elapsed() >= timeout {
            kill(&mut child);
            killed = true;
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    let status = child.wait()?;
    let _ = status; // exit codes are interpreted by the callers via output
    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    Ok(RunResult {
        killed,
        stdout,
        stderr,
        peak_rss_kb: peak,
    })
}

fn kill(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// `ps -o rss=` reports KB on macOS and Linux.
fn current_rss_kb(pid: u32) -> Option<u64> {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_to_completion_and_captures_output() {
        let result = run(
            Command::new("/bin/sh").arg("-c").arg("echo hello; echo err >&2"),
            Duration::from_secs(30),
            RSS_LIMIT_KB,
        )
        .expect("runs");
        assert!(!result.killed);
        assert_eq!(String::from_utf8_lossy(&result.stdout).trim(), "hello");
        assert_eq!(String::from_utf8_lossy(&result.stderr).trim(), "err");
    }

    #[test]
    fn timeout_kills_a_hanging_child() {
        let result = run(
            Command::new("/bin/sh").arg("-c").arg("sleep 30"),
            Duration::from_secs(1),
            RSS_LIMIT_KB,
        )
        .expect("runs");
        assert!(result.killed);
    }
}
