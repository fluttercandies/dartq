//! Launch-failure hygiene with a stand-in `dart` executable: a child
//! that never prints the service banner is killed and reaped on
//! timeout, and a child that exits first is reported as such. Both
//! cases share one test because they mutate `DARTQ_DART_BIN`.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use dartq_debug::{DebugSession, Error, LaunchConfig};

fn fake_dart(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write fake dart");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

#[test]
fn failed_launches_reap_the_child() {
    let dir = std::env::temp_dir().join(format!("dartq-debug-launch-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    // A unique sleep duration tags the child in the process table.
    let tag = format!(
        "{}.{}",
        600 + std::process::id() % 1000,
        std::process::id() % 97
    );
    let silent = fake_dart(&dir, "silent-dart", &format!("exec sleep {tag}"));
    let exits = fake_dart(&dir, "exiting-dart", "echo no banner here\nexit 3");

    let mut config = LaunchConfig::new("main.dart");
    // Generous on purpose: a shebang script pays fork+exec+interpreter
    // startup that can stretch past a few hundred milliseconds on a
    // loaded or SIP-throttled host, and the test is about the error
    // variant, not the timing.
    config.launch_timeout = Duration::from_secs(5);

    // SAFETY: this test binary holds exactly one test, so nothing else
    // reads or writes the environment concurrently.
    unsafe { std::env::set_var("DARTQ_DART_BIN", &silent) };
    let timed_out = DebugSession::launch(config.clone());
    unsafe { std::env::set_var("DARTQ_DART_BIN", &exits) };
    let exited = DebugSession::launch(config);
    unsafe { std::env::remove_var("DARTQ_DART_BIN") };

    assert!(
        matches!(timed_out, Err(Error::LaunchTimeout)),
        "got {:?}",
        timed_out.err()
    );
    let table = std::process::Command::new("ps")
        .args(["-Ao", "args"])
        .output()
        .expect("run ps");
    let table = String::from_utf8_lossy(&table.stdout);
    assert!(
        !table
            .lines()
            .any(|line| line.trim() == format!("sleep {tag}")),
        "the silent child (`sleep {tag}`) must be killed and reaped"
    );

    assert!(
        matches!(exited, Err(Error::ProcessExited(Some(3)))),
        "got {:?}",
        exited.err()
    );
    std::fs::remove_dir_all(&dir).expect("clean temp dir");
}
