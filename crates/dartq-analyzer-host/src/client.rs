//! The `dartq analyze` side of the resident server: find the daemon of the
//! request's key, start it when none is live, ask, and answer. Every
//! failure is an [`Outcome::PerRun`]: the caller runs `dart analyze` per
//! run, so the resident path can only make a run faster, never fail it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::report::Report;
use crate::support::{Sdk, Support, support};
use crate::{Files, SERVE_FLAG, Spec};

/// How long a fresh daemon may take to bind its socket.
#[cfg(unix)]
const BIND_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a failure marker sends runs straight to the per-run path.
#[cfg(unix)]
const FAILED_COOLDOWN: Duration = Duration::from_secs(10 * 60);

/// One resident analysis request.
#[derive(Clone, Debug)]
pub struct Request {
    /// The `dart` binary the toolchain resolved.
    pub dart: PathBuf,
    /// The analysis targets, existing files or directories.
    pub targets: Vec<PathBuf>,
    pub packages: Option<PathBuf>,
    /// The server tree's RSS budget in bytes; `0` is unbounded.
    pub budget: u64,
    pub idle: Duration,
    /// The machine-wide run queue root server starts wait in; `None`
    /// starts unqueued.
    pub queue: Option<PathBuf>,
}

/// The answer of the resident path.
#[derive(Debug)]
pub enum Outcome {
    /// dartdev's report, with the SDK version that produced it.
    Report { report: Report, sdk: Sdk },
    /// The server was killed over the RSS budget: the error the caller
    /// prints, with no per-run retry (it would hit the same budget).
    Budget(String),
    /// Run `dart analyze` per run; `reason` is the one stderr line, `None`
    /// when the SDK is simply not served (no line).
    PerRun {
        reason: Option<String>,
        sdk: Option<Sdk>,
    },
}

/// Asks the resident server for `request`'s report.
#[must_use]
pub fn analyze(request: &Request) -> Outcome {
    let Some(sdk) = Sdk::of_dart(&request.dart) else {
        return Outcome::PerRun {
            reason: None,
            sdk: None,
        };
    };
    let version = sdk.version.to_string();
    if support(&version) == Support::PerRun {
        return Outcome::PerRun {
            reason: None,
            sdk: Some(sdk),
        };
    }
    let roots: Result<Vec<PathBuf>, _> =
        request.targets.iter().map(std::fs::canonicalize).collect();
    let Ok(roots) = roots else {
        return Outcome::PerRun {
            reason: None,
            sdk: Some(sdk),
        };
    };
    let spec = Spec {
        sdk_root: sdk.root.clone(),
        sdk_version: version,
        roots,
        packages: request.packages.as_deref().map(absolute),
        budget: request.budget,
        idle_secs: request.idle.as_secs(),
        queue: request.queue.clone(),
    };
    #[cfg(unix)]
    {
        match ask(&spec) {
            Ok(Answered::Report(report)) => Outcome::Report { report, sdk },
            Ok(Answered::Budget(message)) => Outcome::Budget(message),
            Ok(Answered::Cooling) => Outcome::PerRun {
                reason: None,
                sdk: Some(sdk),
            },
            Err(reason) => Outcome::PerRun {
                reason: Some(format!(
                    "dartq analyze: resident analyzer unavailable ({reason}) — running dart analyze"
                )),
                sdk: Some(sdk),
            },
        }
    }
    #[cfg(not(unix))]
    {
        let _ = spec;
        Outcome::PerRun {
            reason: None,
            sdk: Some(sdk),
        }
    }
}

fn absolute(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(unix)]
enum Answered {
    Report(Report),
    Budget(String),
    Cooling,
}

/// The unix transport: election, start, request.
#[cfg(unix)]
fn ask(spec: &Spec) -> Result<Answered, String> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    use crate::daemon::{Answer, Ask};

    let dir = spec.dir();
    let files = Files::of(&dir);
    if let Ok(meta) = std::fs::metadata(&files.failed)
        && meta
            .modified()
            .ok()
            .and_then(|at| at.elapsed().ok())
            .is_some_and(|age| age < FAILED_COOLDOWN)
    {
        return Ok(Answered::Cooling);
    }
    let stream = match UnixStream::connect(&files.socket) {
        Ok(stream) => stream,
        Err(_) => start(spec, &dir, &files)?,
    };
    let mut line = serde_json::to_vec(&Ask {
        roots: spec.roots.clone(),
    })
    .expect("serializes");
    line.push(b'\n');
    (&stream)
        .write_all(&line)
        .map_err(|error| format!("request failed: {error}"))?;
    let mut reader = BufReader::new(&stream);
    let mut answer = String::new();
    loop {
        answer.clear();
        reader
            .read_line(&mut answer)
            .map_err(|error| format!("no answer: {error}"))?;
        match serde_json::from_str::<Answer>(&answer) {
            Ok(Answer::Note(note)) => eprintln!("{note}"),
            Ok(Answer::Report(report)) => return Ok(Answered::Report(report)),
            Ok(Answer::Budget(message)) => return Ok(Answered::Budget(message)),
            Ok(Answer::Fault(fault)) => return Err(fault),
            Err(_) => return Err("the daemon exited mid-request".into()),
        }
    }
}

/// Starts the daemon under the key's election lock (a racing client waits
/// on the lock, then finds the socket live) and connects.
#[cfg(unix)]
fn start(spec: &Spec, dir: &Path, files: &Files) -> Result<std::os::unix::net::UnixStream, String> {
    use std::os::unix::io::AsRawFd as _;
    use std::os::unix::net::UnixStream;
    use std::os::unix::process::CommandExt as _;
    use std::process::{Command, Stdio};

    std::fs::create_dir_all(dir)
        .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&files.elect)
        .map_err(|error| format!("cannot open {}: {error}", files.elect.display()))?;
    // SAFETY: a blocking exclusive flock on an owned fd; released when
    // `lock` drops.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err("the start lock failed".into());
    }
    if let Ok(stream) = UnixStream::connect(&files.socket) {
        return Ok(stream);
    }
    // A daemon whose socket is dead but whose pid lives is wedged: reap it.
    if let Some(pid) = std::fs::read_to_string(&files.state)
        .ok()
        .and_then(|pid| pid.trim().parse().ok())
        && dartq_toolchain::proc::alive(pid)
    {
        dartq_toolchain::proc::kill_tree(pid);
    }
    let _ = std::fs::remove_file(&files.socket);
    let spec_bytes = serde_json::to_vec(spec).expect("a spec serializes");
    std::fs::write(&files.spec, spec_bytes)
        .map_err(|error| format!("cannot write the spec: {error}"))?;
    let exe = std::env::current_exe().map_err(|error| format!("cannot locate dartq: {error}"))?;
    Command::new(exe)
        .args(["analyze", SERVE_FLAG])
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env_remove("DARTQ_SHIM")
        .process_group(0)
        .spawn()
        .map_err(|error| format!("cannot start the daemon: {error}"))?;
    let deadline = std::time::Instant::now() + BIND_TIMEOUT;
    loop {
        if let Ok(stream) = UnixStream::connect(&files.socket) {
            return Ok(stream);
        }
        if std::time::Instant::now() >= deadline {
            return Err("the daemon never listened".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
