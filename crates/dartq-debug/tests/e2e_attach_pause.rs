//! Live e2e for the attach / pause / rewind / hot-restart faces that
//! `e2e_complex` does not reach.
//!
//! Skipped by default: requires `dart` on `PATH`. Run explicitly:
//!
//! ```text
//! cargo test -p dartq-debug --test e2e_attach_pause -- --ignored --test-threads=1
//! ```

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use dartq_debug::{
    BreakpointLocationSpec, BreakpointSpec, DebugSession, EventKind, SessionConfig, StepOption,
    StoppedReason,
};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/complex.dart")
}

fn config() -> SessionConfig {
    SessionConfig {
        pause_timeout: Duration::from_secs(15),
        rpc_timeout: Duration::from_secs(15),
        ..SessionConfig::default()
    }
}

fn file_uri() -> String {
    format!("file://{}", fixture().display())
}

fn line_bp(line: u32) -> BreakpointSpec {
    BreakpointSpec {
        isolate: None,
        location: BreakpointLocationSpec::Line {
            script_uri: file_uri(),
            line,
            column: None,
        },
        condition: None,
    }
}

fn resume_tagged(session: &mut DebugSession, tag: &str) -> dartq_debug::StoppedSnapshot {
    session
        .step_and_package(None, None)
        .unwrap_or_else(|e| panic!("{tag}: resume failed: {e}"))
}

/// Spawns `dart --enable-vm-service=0 --pause-isolates-on-start` so
/// the isolate parks at the entry point and stays alive for attach;
/// returns the child and the ws URI.
fn spawn_free_running() -> (Child, String) {
    let dart = std::env::var("DARTQ_DART_BIN").unwrap_or_else(|_| "dart".to_owned());
    let mut child = Command::new(&dart)
        .arg("--enable-vm-service=0")
        .arg("--pause-isolates-on-start")
        .arg(fixture())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("spawn dart failed: {e}"));
    let stdout = child.stdout.take().expect("stdout pipe");
    let mut reader = std::io::BufReader::new(stdout);
    let mut line = String::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    let uri = loop {
        line.clear();
        use std::io::BufRead;
        match reader.read_line(&mut line) {
            Ok(0) => panic!("dart exited before banner"),
            Ok(_) => {
                if let Some(start) = line.find("http://") {
                    let token = line[start..].trim_end().to_owned();
                    if token.contains("/") {
                        break token;
                    }
                }
            }
            Err(e) => panic!("read banner: {e}"),
        }
        assert!(Instant::now() < deadline, "banner timeout");
    };
    // http://127.0.0.1:PORT/<auth>=/ -> ws://127.0.0.1:PORT/<auth>=/ws
    let ws = uri
        .replacen("http://", "ws://", 1)
        .trim_end_matches('/')
        .to_owned()
        + "/ws";
    (child, ws)
}

#[test]
#[ignore = "requires a real `dart` executable on PATH (or DARTQ_DART_BIN)"]
fn attach_pause_step_rewind() {
    let (mut child, ws_uri) = spawn_free_running();

    // Attach to the paused-at-start isolate.
    let mut session = DebugSession::attach_with(&ws_uri, config())
        .unwrap_or_else(|e| panic!("attach failed: {e}"));
    assert!(session.main_isolate().is_some(), "no main isolate on attach");

    // The isolate is paused at entry — arm a breakpoint mid-program
    // (L57: plenty of execution left after it) and resume.
    let mid_bp = session
        .add_breakpoint(line_bp(57))
        .expect("mid-program breakpoint");
    let snap = resume_tagged(&mut session, "attach-first-resume");
    match &snap.reason {
        StoppedReason::Breakpoint { breakpoints } => {
            assert!(
                breakpoints.contains(&mid_bp.id),
                "first stop={breakpoints:?}"
            );
        }
        other => panic!("expected breakpoint stop, got {other:?}"),
    }
    assert_eq!(snap.location.line, Some(57));

    // `pause` on an already-paused isolate is a legal RPC — the
    // transport must not fault.
    session.pause(None).expect("pause on paused isolate");

    // Rewind: the VM accepts StepOption::Rewind on a paused isolate.
    let _rewound = session
        .step_and_package(None, Some(StepOption::Rewind))
        .expect("rewind");

    // events_since must carry at least the pause + breakpoint events
    // we already consumed.
    session.poll_events().expect("final poll");
    let batch = session.events_since(0, usize::MAX);
    assert!(
        !batch.events.is_empty(),
        "no events recorded after attach session"
    );
    assert!(
        batch
            .events
            .iter()
            .any(|e| matches!(e.event.kind, EventKind::PauseBreakpoint | EventKind::PauseInterrupted)),
        "no pause event in ring"
    );

    // hot_restart on a bare VM: DDS-registered service absent -> Rpc
    // error, not a crash.
    match session.hot_restart() {
        Err(e) => {
            let text = format!("{e}");
            assert!(
                text.contains("-32601") || text.contains("hot restart") || text.contains("rpc"),
                "unexpected hot_restart error: {text}"
            );
        }
        Ok(()) => {} // DDS-connected VM would succeed — acceptable.
    }

    session.terminate().expect("terminate");
    let _ = child.kill();
    let _ = child.wait();
}
