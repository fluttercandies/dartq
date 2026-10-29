//! Live integration test against a real `dart` process.
//!
//! Skipped by default: running it requires a `dart` executable on
//! `PATH` (or `DARTQ_DART_BIN`) and spawns a real VM. Run explicitly:
//!
//! ```text
//! cargo test -p dartq-debug --test real_dart -- --ignored
//! ```
//!
//! Expected shape when it passes:
//!
//! ```text
//! running 1 test
//! test live_debug_session ... ok
//! test result: ok. 1 passed; 0 failed; 0 ignored
//! ```

use std::path::PathBuf;
use std::time::Duration;

use dartq_debug::{
    BreakpointLocationSpec, BreakpointSpec, DebugSession, LaunchConfig, SessionConfig, StepOption,
    StoppedReason, VmObject,
};

#[test]
#[ignore = "requires a real `dart` executable on PATH (or DARTQ_DART_BIN)"]
fn live_debug_session() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hello.dart");
    assert!(fixture.is_file(), "fixture missing: {}", fixture.display());

    let settings = SessionConfig {
        pause_timeout: Duration::from_secs(10),
        rpc_timeout: Duration::from_secs(10),
        ..SessionConfig::default()
    };
    let mut session = DebugSession::launch_with(LaunchConfig::new(&fixture), settings)
        .unwrap_or_else(|error| panic!("launch failed (is `dart` installed?): {error}"));

    assert!(session.main_isolate().is_some(), "no main isolate");

    let script_uri = format!("file://{}", fixture.display());
    let handle = session
        .add_breakpoint(BreakpointSpec {
            isolate: None,
            location: BreakpointLocationSpec::Line {
                script_uri: script_uri.clone(),
                // Line 6 is past the `var counter = 0;` declaration, so
                // `counter` is live in the frame when the VM pauses here.
                line: 6,
                column: None,
            },
            condition: None,
        })
        .unwrap_or_else(|error| panic!("add breakpoint failed: {error}"));
    assert_eq!(handle.isolate, session.main_isolate().unwrap_or_default());

    // The isolate is paused on start; resume and wait for the hit.
    let snapshot = session
        .step_and_package(None, None)
        .unwrap_or_else(|error| panic!("resume to breakpoint failed: {error}"));
    match &snapshot.reason {
        // The first stop after a resume may be a synthetic
        // `PauseBreakpoint` carrying no breakpoint handles (the VM's
        // internal step-over machinery reports it the same way); a
        // second resume lands on the armed breakpoint.
        StoppedReason::Breakpoint { breakpoints }
            if breakpoints.iter().any(|id| id == &handle.id) => {}
        _ => {
            let snapshot = session
                .step_and_package(None, None)
                .unwrap_or_else(|error| panic!("second resume failed: {error}"));
            match &snapshot.reason {
                StoppedReason::Breakpoint { breakpoints } => {
                    assert!(
                        breakpoints.iter().any(|id| id == &handle.id),
                        "expected hit on {}, got {breakpoints:?}",
                        handle.id
                    );
                }
                other => panic!("expected breakpoint hit, got {other:?}"),
            }
        }
    }
    assert_eq!(
        snapshot.location.script_uri.as_deref(),
        Some(script_uri.as_str())
    );
    assert_eq!(snapshot.location.line, Some(6));
    // The spec marks `Frame.vars` as optional — the VM may omit locals
    // for a paused frame. When it does enumerate them, `counter` must
    // be among them; when it omits the slot entirely the snapshot is
    // still valid (authoritative variable access goes through
    // `evaluateInFrame` below).
    if !snapshot.variables.is_empty() {
        assert!(
            snapshot
                .variables
                .iter()
                .any(|variable| variable.name == "counter"),
            "`counter` local present but missing from snapshot: {:?}",
            snapshot
                .variables
                .iter()
                .map(|variable| variable.name.as_str())
                .collect::<Vec<_>>()
        );
    }

    // Frame evaluation sees the local.
    let value = session
        .evaluate_in_frame(None, 0, "counter")
        .expect("evaluateInFrame");
    match value {
        VmObject::Instance(instance) => {
            assert_eq!(instance.value_as_string.as_deref(), Some("0"));
        }
        other => panic!("expected Int instance, got {other:?}"),
    }

    // One step lands us somewhere sane and still paused.
    let stepped = session
        .step_and_package(None, Some(StepOption::Over))
        .expect("step over");
    // The VM reports a completed step as a breakpoint-less
    // `PauseBreakpoint`, which the session surfaces as `Step`.
    assert_eq!(
        stepped.reason,
        StoppedReason::Step,
        "step over landed on {stepped:?}"
    );
    assert_eq!(stepped.location.line, Some(7), "step over from line 6");

    session
        .remove_breakpoint(None, &handle.id)
        .expect("remove breakpoint");

    // The remaining program output shows up in the stdout ring.
    session.resume(None).expect("final resume");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut output = Vec::new();
    while !output
        .windows(b"counter=1".len())
        .any(|window| window == b"counter=1")
    {
        session.poll_events().expect("poll output");
        let batch = session.stdout_since(0, usize::MAX);
        output = batch.bytes;
        if std::time::Instant::now() >= deadline {
            panic!(
                "program output `counter=1` never arrived; saw {:?}",
                String::from_utf8_lossy(&output)
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    session.terminate().expect("terminate");
}
