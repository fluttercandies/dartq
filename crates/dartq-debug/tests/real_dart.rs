//! Live integration tests against a real `dart` process.
//!
//! Skipped by default: running them requires a `dart` executable on
//! `PATH` (or `DARTQ_DART`) and spawns a real VM. Run explicitly:
//!
//! ```text
//! cargo test -p dartq-debug --test real_dart -- --ignored
//! ```
//!
//! Expected shape when they pass:
//!
//! ```text
//! running 2 tests
//! test finished_program_names_its_exit ... ok
//! test live_debug_session ... ok
//! test result: ok. 2 passed; 0 failed; 0 ignored
//! ```

use std::path::PathBuf;
use std::time::Duration;

use dartq_debug::{
    BreakpointLocationSpec, BreakpointSpec, DebugSession, Error, LaunchConfig, SessionConfig,
    StepOption, StoppedReason, VmObject,
};

#[test]
#[ignore = "requires a real `dart` executable on PATH (or DARTQ_DART)"]
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

/// A program run to completion names its exit on every later call: the
/// snapshot that waited the run out, an evaluation, and a breakpoint all
/// answer the process exit with its code — never the closed transport.
#[test]
#[ignore = "requires a real `dart` executable on PATH (or DARTQ_DART)"]
fn finished_program_names_its_exit() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/exits.dart");
    assert!(fixture.is_file(), "fixture missing: {}", fixture.display());

    let settings = SessionConfig {
        pause_timeout: Duration::from_secs(10),
        rpc_timeout: Duration::from_secs(10),
        ..SessionConfig::default()
    };
    let mut session = DebugSession::launch_with(LaunchConfig::new(&fixture), settings)
        .unwrap_or_else(|error| panic!("launch failed (is `dart` installed?): {error}"));
    let main = session.main_isolate().expect("main isolate").to_owned();

    // A breakpoint in a script that never loads stays latent, so nothing
    // stops the run.
    let never_loaded = |isolate: Option<String>| BreakpointSpec {
        isolate,
        location: BreakpointLocationSpec::Line {
            script_uri: "package:never_loaded/never_loaded.dart".to_owned(),
            line: 1,
            column: None,
        },
        condition: None,
    };
    let handle = session
        .add_breakpoint(never_loaded(None))
        .expect("add a latent breakpoint");
    assert!(!handle.resolved, "a never-loaded script cannot resolve");
    session.resume(None).expect("resume into the exit");

    // The snapshot waits the run out and answers its exit.
    let snapshot = session.wait_for_pause(&main);
    assert!(
        matches!(snapshot, Err(Error::ProcessExited(Some(0)))),
        "snapshot after the run: {snapshot:?}"
    );
    let error = session
        .evaluate_in_root_lib(None, "1 + 1")
        .expect_err("evaluate after the exit");
    assert!(
        matches!(error, Error::ProcessExited(Some(0))),
        "evaluate after the exit: {error:?}"
    );
    assert_eq!(error.to_string(), "process exited with code 0");
    // The reaped status keeps answering.
    let error = session
        .add_breakpoint(never_loaded(Some(main.clone())))
        .expect_err("breakpoint after the exit");
    assert!(
        matches!(error, Error::ProcessExited(Some(0))),
        "breakpoint after the exit: {error:?}"
    );
    let snapshot = session.wait_for_pause(&main);
    assert!(
        matches!(snapshot, Err(Error::ProcessExited(Some(0)))),
        "second snapshot: {snapshot:?}"
    );

    session.terminate().expect("terminate after the exit");
}
