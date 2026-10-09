//! Oracle-side driver: runs `dart analyze` and parses its output into a diagnostic set.
//! Exit code semantics (facts §3): {0,1,2,3} are valid; 4 = crash, 64 = usage error -> harness itself fails.

#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::machine::{MachineDiag, OutputFormat, parse_output};

/// Wall-clock budget per driver invocation. `dart analyze` takes seconds on
/// the largest corpus; a hang (daemon/watchdog wedge) must fail the run, not
/// block the harness forever. Overridable via `DARTIQ_HARNESS_ORACLE_TIMEOUT_MS`.
const DEFAULT_TIMEOUT_MS: u64 = 120_000;

/// `try_wait` poll interval (the timeout resolution floor).
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// `dart analyze` driver. `dart` is the executable path (defaults to dart on PATH).
/// `--no-fatal-warnings` makes the exit code reflect only 0/3 (facts §7.9); diagnostic output is unaffected.
pub(crate) struct OracleDriver {
    pub(crate) dart: String,
}

impl OracleDriver {
    pub(crate) fn analyze(
        &self,
        corpus_dir: &std::path::Path,
        format: OutputFormat,
    ) -> Result<Vec<MachineDiag>, String> {
        let args = vec![
            "analyze".to_string(),
            format.flag().to_string(),
            "--no-fatal-warnings".to_string(),
            corpus_dir.display().to_string(),
        ];
        run_driver(DriverSide::Oracle, &self.dart, &args, format)
    }
}

/// Which child program a driver talks to; the side determines the exit-code contract.
#[derive(Debug, Clone, Copy)]
pub(crate) enum DriverSide {
    /// `dart analyze` (the oracle side; also the e2e self-check's dartiq-side process)
    Oracle,
    /// the dartiq CLI (measurement side)
    Dartiq,
}

impl DriverSide {
    fn as_str(self) -> &'static str {
        match self {
            Self::Oracle => "oracle",
            Self::Dartiq => "dartiq",
        }
    }

    /// Exit codes that count as a measurement result. Oracle (facts §3):
    /// {0,1,2,3} are valid; 4 = crash, 64 = usage error. The dartiq CLI uses
    /// 2 for usage errors of its own (e.g. a malformed `--dartiq-cmd`
    /// template): folding exit 2 into "valid + empty output" would score a
    /// meaningless Jaccard 0, so it fails the run up front like 4/64. The e2e
    /// self-check pins dart analyze to exits {0,3} via `--no-fatal-warnings`
    /// (facts §7.9), so the stricter dartiq contract cannot misfire there.
    fn valid_exit(self, code: i32) -> bool {
        match self {
            Self::Oracle => (0..=3).contains(&code),
            Self::Dartiq => matches!(code, 0 | 1 | 3),
        }
    }

    fn valid_exits_desc(self) -> &'static str {
        match self {
            Self::Oracle => "0-3",
            Self::Dartiq => "0, 1, 3 (2 = usage error)",
        }
    }
}

/// Capture the oracle's reference version once per run (`dart --version`, one line on stderr) —
/// recorded in every report per PRD §19.1 (the embedded analyzer version rides the SDK).
pub(crate) fn dart_version(dart: &str) -> Result<String, String> {
    let timeout = driver_timeout_from_env()?;
    let output = run_captured("oracle", dart, &["--version".to_string()], None, timeout)?;
    if !output.status.success() {
        let tail = stderr_tail(&output.stderr);
        return Err(format!(
            "[oracle] `{dart} --version` exited with {:?}; stderr tail:\n{tail}",
            output.status.code()
        ));
    }
    let stderr = String::from_utf8(output.stderr)
        .map_err(|e| format!("[oracle] `{dart} --version` stderr is not valid UTF-8: {e}"))?;
    parse_dart_version(&stderr)
}

/// `dart --version` prints a single banner line to stderr, e.g.
/// `Dart SDK version: 3.9.4 (stable) (Tue Sep 9 12:00:00 2025 +0000) on "macos_arm64"`.
/// The full trimmed line is the reference version (build date + platform included);
/// no format strictness beyond non-empty: the field records whatever banner the
/// pinned oracle prints, and an empty banner is a harness error rather than a
/// silent fallback that would leave reports unattributable.
fn parse_dart_version(stderr: &str) -> Result<String, String> {
    stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "[oracle] `dart --version` printed no version banner".to_string())
}

/// Pipeline shared by both drivers: "run command under a time budget -> validate exit code -> parse output".
/// `side` selects the exit-code contract (see [`DriverSide::valid_exit`]).
/// Diagnostic lines always go to stdout (facts §3); stderr is ignored on valid exit codes, and its tail is attached to aid locating failures on abnormal exits.
pub(crate) fn run_driver(
    side: DriverSide,
    program: &str,
    args: &[String],
    format: OutputFormat,
) -> Result<Vec<MachineDiag>, String> {
    let name = side.as_str();
    let timeout = driver_timeout_from_env()?;
    let output = run_captured(name, program, args, None, timeout)?;
    let exit = output.status.code();
    if !exit.is_some_and(|code| side.valid_exit(code)) {
        let tail = stderr_tail(&output.stderr);
        return Err(format!(
            "[{name}] `{program}` exited with {exit:?} (valid: {}); stderr tail:\n{tail}",
            side.valid_exits_desc()
        ));
    }
    // Strict decode: diagnostic text is UTF-8 (facts §1/§2); lossy decoding
    // would silently mangle messages, so a non-UTF-8 stream is a harness error.
    let stdout = String::from_utf8(output.stdout)
        .map_err(|e| format!("[{name}] `{program}` stdout is not valid UTF-8: {e}"))?;
    parse_output(&stdout, format).map_err(|e| format!("[{name}] {e}"))
}

/// Shared budget for every child driver (`dart analyze` and corpus
/// `dart pub get`): one env override, one default, one validation.
pub(crate) fn driver_timeout_from_env() -> Result<Duration, String> {
    driver_timeout(
        std::env::var("DARTIQ_HARNESS_ORACLE_TIMEOUT_MS")
            .ok()
            .as_deref(),
    )
}

/// Timeout budget: default [`DEFAULT_TIMEOUT_MS`], overridable via
/// `DARTIQ_HARNESS_ORACLE_TIMEOUT_MS` (milliseconds, must be > 0). An
/// unparseable value is a harness error, not a silent fallback to the default.
fn driver_timeout(raw: Option<&str>) -> Result<Duration, String> {
    let Some(raw) = raw else {
        return Ok(Duration::from_millis(DEFAULT_TIMEOUT_MS));
    };
    let ms: u64 = raw.trim().parse().map_err(|_| {
        format!("invalid DARTIQ_HARNESS_ORACLE_TIMEOUT_MS `{raw}`; expected milliseconds > 0")
    })?;
    if ms == 0 {
        return Err("DARTIQ_HARNESS_ORACLE_TIMEOUT_MS must be > 0".into());
    }
    Ok(Duration::from_millis(ms))
}

/// Run `program` to completion under a wall-clock budget (shared by the
/// oracle driver and corpus `pub get` — one kill-and-report semantics for
/// all child drivers). `cwd` sets the child's working directory (`None` =
/// inherit the harness's). stdout/stderr are
/// captured to temp files instead of OS pipes: a verbose driver (the merged
/// corpus emits megabytes of diagnostics) would fill the pipe buffer, block on
/// write, never exit, and trip a false timeout. On timeout the driver's whole
/// process group is killed (unix; see [`kill_driver`]) and the stderr tail is
/// attached to the harness error.
pub(crate) fn run_captured(
    name: &str,
    program: &str,
    args: &[String],
    cwd: Option<&std::path::Path>,
    timeout: Duration,
) -> Result<Output, String> {
    let capture = TempCapture::new();
    let stdout =
        std::fs::File::create(&capture.stdout_path).map_err(|e| format!("[{name}] {e}"))?;
    let stderr =
        std::fs::File::create(&capture.stderr_path).map_err(|e| format!("[{name}] {e}"))?;
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    // Own process group for the child (unix): `dart` spawns its analysis server
    // as a grandchild, and killing the direct child alone would leave that
    // server burning a core past the timeout (the exact wedge this budget
    // exists for). A setpgid failure is ignored -- worst case the timeout kill
    // falls back to the direct child only.
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            let _ = libc::setpgid(0, 0);
            Ok(())
        });
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("[{name}] failed to launch `{program}`: {e}"))?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    kill_driver(&mut child);
                    let _ = child.wait();
                    let tail = stderr_tail(&capture.stderr_bytes());
                    return Err(format!(
                        "[{name}] `{program}` timed out after {}ms and was killed; stderr tail:\n{tail}",
                        timeout.as_millis()
                    ));
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(e) => return Err(format!("[{name}] waiting for `{program}`: {e}")),
        }
    };
    Ok(Output {
        status,
        stdout: capture.stdout_bytes(),
        stderr: capture.stderr_bytes(),
    })
}

/// Kill a timed-out driver. On unix the child runs in its own process group
/// (`setpgid(0, 0)` pre-exec), so the whole group is signalled: `dart` runs
/// its analysis server as a grandchild that a plain `child.kill()` leaves
/// behind, burning a core past the budget. The trailing `child.kill()` covers
/// the fork-to-setpgid window in which the group does not exist yet (kill(2)
/// fails with ESRCH); both are no-ops on an already-exited target. Windows has
/// no std-level process-group equivalent, so only the direct child is killed
/// there.
fn kill_driver(child: &mut std::process::Child) {
    #[cfg(unix)]
    if let Ok(pid) = i32::try_from(child.id()) {
        // SAFETY: kill(2) with a negative PID signals the process group whose
        // leader is our own just-spawned child (see the pre_exec hook).
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
}

/// Last 5 stderr lines (diagnostic tail for failure reports).
fn stderr_tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(5);
    lines[start..].join("\n")
}

/// Temp capture files for one driver invocation; removed on drop (all exit paths).
struct TempCapture {
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

static NEXT_CAPTURE_ID: AtomicU64 = AtomicU64::new(0);

impl TempCapture {
    fn new() -> Self {
        let id = NEXT_CAPTURE_ID.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!("dq_harness_{}_{}", std::process::id(), id));
        Self {
            stdout_path: base.with_extension("out"),
            stderr_path: base.with_extension("err"),
        }
    }

    fn stdout_bytes(&self) -> Vec<u8> {
        std::fs::read(&self.stdout_path).unwrap_or_default()
    }

    fn stderr_bytes(&self) -> Vec<u8> {
        std::fs::read(&self.stderr_path).unwrap_or_default()
    }
}

impl Drop for TempCapture {
    fn drop(&mut self) {
        std::fs::remove_file(&self.stdout_path).ok();
        std::fs::remove_file(&self.stderr_path).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    /// End to end: runs real dart against a mini package under /tmp.
    /// Requires dart on PATH; runs only when `DARTIQ_HARNESS_E2E=1` (opt-in for CI/local runs).
    #[test]
    fn oracle_end_to_end_real_dart() {
        if std::env::var("DARTIQ_HARNESS_E2E").as_deref() != Ok("1") {
            eprintln!("skipped: set DARTIQ_HARNESS_E2E=1 to run the oracle e2e test");
            return;
        }
        let dir = std::env::temp_dir().join(format!("dq_oracle_e2e_{}", std::process::id()));
        fs::create_dir_all(dir.join("lib")).expect("mkdir");
        fs::write(
            dir.join("pubspec.yaml"),
            "name: e2e_mini\nenvironment:\n  sdk: ^3.0.0\n",
        )
        .expect("pubspec");
        fs::write(dir.join("lib").join("bad.dart"), "var x = 1 }\n").expect("bad.dart");
        fs::write(dir.join("lib").join("clean.dart"), "void f() {}\n").expect("clean.dart");

        let driver = OracleDriver {
            dart: "dart".to_string(),
        };
        let diags = driver
            .analyze(&dir, OutputFormat::Machine)
            .unwrap_or_else(|e| panic!("oracle analyze failed: {e}"));

        // `var x = 1 }` -> EXPECTED_TOKEN plus the derived EXPECTED_EXECUTABLE (facts §6 cascaded-line form)
        assert_eq!(
            diags.len(),
            2,
            "mini package should yield exactly two diags"
        );
        assert_eq!(diags[0].code, "EXPECTED_TOKEN");
        assert_eq!(diags[0].severity, crate::machine::Severity::Error);
        assert_eq!(diags[0].line, 1);
        assert_eq!(diags[0].col, 9);
        // oracle emits symlink-resolved canonical paths (facts §7.4); the expectation is normalized the same way
        let expected_file = dir.canonicalize().unwrap().join("lib").join("bad.dart");
        assert_eq!(diags[0].file, expected_file);
        assert_eq!(diags[1].code, "EXPECTED_EXECUTABLE");

        // clean file on its own = no diagnostics, exit 0 (facts §3)
        let diags = driver
            .analyze(&dir.join("lib").join("clean.dart"), OutputFormat::Machine)
            .expect("clean file must analyze");
        assert!(diags.is_empty());

        fs::remove_dir_all(&dir).ok();
    }

    /// facts §3: nonexistent path -> exit 64 -> harness failure, not an empty diagnostic set.
    #[test]
    fn missing_path_is_harness_error() {
        if std::env::var("DARTIQ_HARNESS_E2E").as_deref() != Ok("1") {
            eprintln!("skipped: set DARTIQ_HARNESS_E2E=1 to run the oracle e2e test");
            return;
        }
        let driver = OracleDriver {
            dart: "dart".to_string(),
        };
        let err = driver
            .analyze(
                Path::new("/nonexistent/dq_no_such_dir"),
                OutputFormat::Machine,
            )
            .expect_err("missing path must be a harness error");
        assert!(err.contains("64"), "error should mention exit 64: {err}");
    }

    /// A hung driver is killed and reported within the budget (never blocks
    /// the run forever); the error carries the stderr tail captured so far.
    #[test]
    fn timeout_kills_hung_driver() {
        let started = Instant::now();
        let err = run_captured(
            "oracle",
            "sh",
            &["-c".to_string(), "echo boom >&2; exec sleep 30".to_string()],
            None,
            Duration::from_millis(300),
        )
        .expect_err("hung driver must time out");
        assert!(err.contains("timed out"), "error: {err}");
        assert!(err.contains("300ms"), "budget cited: {err}");
        assert!(err.contains("boom"), "stderr tail attached: {err}");
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "must fail within the test budget, took {:?}",
            started.elapsed()
        );
    }

    /// The timeout kill reaches the driver's whole POSIX process group: `dart`'s
    /// analysis server is a grandchild process, and a plain `child.kill()` would
    /// leave it running past the budget. Here the driver is `sh` with a
    /// background `sleep` (the grandchild); after the timeout both must be gone.
    /// Unix only -- the process-group mechanism is POSIX.
    #[cfg(unix)]
    #[test]
    fn timeout_kill_reaches_whole_process_group() {
        let tmp = std::env::temp_dir();
        let child_pid = tmp.join(format!("dq_pg_child_{}.pid", std::process::id()));
        let grand_pid = tmp.join(format!("dq_pg_grand_{}.pid", std::process::id()));
        let script = format!(
            "echo $$ > {child}; sleep 30 & echo $! > {grand}; wait",
            child = child_pid.display(),
            grand = grand_pid.display(),
        );
        let err = run_captured(
            "oracle",
            "sh",
            &["-c".to_string(), script],
            None,
            Duration::from_millis(300),
        )
        .expect_err("hung driver must time out");
        assert!(err.contains("timed out"), "error: {err}");

        let read_pid = |path: &Path| -> i32 {
            std::fs::read_to_string(path)
                .expect("pid file written by the driver")
                .trim()
                .parse()
                .expect("pid number")
        };
        // SIGKILL is immediate, but the reparented grandchild may take a moment
        // to be reaped; poll briefly instead of asserting synchronously.
        let alive = |pid: i32| -> bool {
            // SAFETY: kill(2) with signal 0 probes liveness and delivers nothing.
            unsafe { libc::kill(pid, 0) == 0 }
        };
        let (child, grand) = (read_pid(&child_pid), read_pid(&grand_pid));
        let deadline = Instant::now() + Duration::from_secs(5);
        while (alive(child) || alive(grand)) && Instant::now() < deadline {
            std::thread::sleep(POLL_INTERVAL);
        }
        assert!(!alive(child), "driver sh (pid {child}) must be dead");
        assert!(!alive(grand), "grandchild sleep (pid {grand}) must be dead");
        std::fs::remove_file(&child_pid).ok();
        std::fs::remove_file(&grand_pid).ok();
    }

    /// Capture must not go through OS pipes: a driver emitting more than the
    /// ~64KB pipe buffer would block on write and trip a false timeout.
    #[test]
    fn large_stdout_is_captured_completely() {
        let output = run_captured(
            "oracle",
            "sh",
            &["-c".to_string(), "yes x | head -c 200000".to_string()],
            None,
            Duration::from_secs(30),
        )
        .expect("complete capture");
        assert_eq!(output.stdout.len(), 200_000);
    }

    /// Diagnostic text is UTF-8 (facts §1/§2): lossy decoding would silently
    /// mangle messages, so a non-UTF-8 stdout is a harness error, not mojibake.
    #[test]
    fn invalid_utf8_stdout_is_strict_error() {
        let err = run_driver(
            DriverSide::Oracle,
            "sh",
            &["-c".to_string(), "printf '\\377\\376'".to_string()],
            OutputFormat::Machine,
        )
        .expect_err("invalid UTF-8 stdout must error");
        assert!(err.contains("not valid UTF-8"), "error: {err}");
    }

    /// Timeout budget: default 120s, env override in milliseconds; garbage
    /// and zero are harness errors, not silent fallbacks.
    #[test]
    fn timeout_budget_parsing() {
        assert_eq!(
            driver_timeout(None).unwrap(),
            Duration::from_millis(DEFAULT_TIMEOUT_MS)
        );
        assert_eq!(
            driver_timeout(Some("500")).unwrap(),
            Duration::from_millis(500)
        );
        assert_eq!(
            driver_timeout(Some(" 250 ")).unwrap(),
            Duration::from_millis(250)
        );
        assert!(driver_timeout(Some("abc")).is_err());
        assert!(driver_timeout(Some("0")).is_err());
        assert!(driver_timeout(Some("-5")).is_err());
    }

    /// The reference version is the oracle's full trimmed banner line (build
    /// date + platform included); leading noise lines are tolerated, an empty
    /// banner is a harness error instead of an unattributable report.
    #[test]
    fn dart_version_banner_parsing() {
        let banner =
            "Dart SDK version: 3.9.4 (stable) (Tue Sep 9 12:00:00 2025 +0000) on \"macos_arm64\"";
        assert_eq!(parse_dart_version(banner).unwrap(), banner);
        // Banner arrives with surrounding whitespace / trailing newline
        assert_eq!(
            parse_dart_version(&format!("  {banner}  \n")).unwrap(),
            banner
        );
        assert_eq!(
            parse_dart_version("\nwarning line\n\n").unwrap(),
            "warning line",
            "first non-empty line wins"
        );
        assert!(parse_dart_version("").is_err());
        assert!(
            parse_dart_version("  \n \n").is_err(),
            "blank-only banner must error"
        );
    }
}
