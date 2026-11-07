//! The exit-naming order, hermetically: a stand-in `dart` announces an
//! in-process mock VM service and exits only when released, so each
//! state a failed call can name is reachable on demand — the addressed
//! isolate exited, the transport closed, the launched process exited —
//! and the order between them is observable. One test, because it
//! mutates `DARTQ_DART`.

#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use common::{Control, MockVm, Reply, default_reply, isolate_ref, ok};
use dartq_debug::{DebugSession, Error, LaunchConfig, SessionConfig};
use serde_json::{Value, json};

/// The VM's answer for an isolate it no longer has.
fn collected() -> Value {
    json!({ "type": "Sentinel", "kind": "Collected", "valueAsString": "<collected>" })
}

/// Waits until the mock's serve loop has ended (its request sender drops
/// with it), draining the requests it already saw.
fn wait_for_disconnect(vm: &MockVm) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match vm.requests.recv_deadline(deadline) {
            Ok(_) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => panic!("the mock never closed"),
        }
    }
}

#[test]
fn failed_calls_name_their_exit_in_order() {
    let vm = MockVm::start(|request| {
        let isolate = request.params["isolateId"].as_str().unwrap_or_default();
        match (request.method.as_str(), isolate) {
            ("getVM", _) => ok(json!({
                "type": "VM",
                "isolates": [
                    isolate_ref("isolates/1", "1", "main"),
                    isolate_ref("isolates/2", "2", "worker"),
                ],
                "systemIsolates": [],
            })),
            // The worker's exit reaches the stream ahead of the VM's
            // answer for it.
            ("getStack", "isolates/2") => Reply::Prefixed {
                prelude: vec![Control::Event {
                    stream_id: "Isolate".to_owned(),
                    event: json!({
                        "type": "Event",
                        "kind": "IsolateExit",
                        "isolate": isolate_ref("isolates/2", "2", "worker"),
                        "timestamp": 1_i64,
                    }),
                }],
                result: collected(),
            },
            // An exit the stream never showed.
            ("pause", "isolates/1") => ok(collected()),
            _ => default_reply(request),
        }
    });

    let dir = std::env::temp_dir().join(format!("dartq-debug-exit-state-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let release = dir.join("release");
    // The announced `http://HOST:PORT/` converts to the mock's
    // `ws://HOST:PORT/ws`.
    let authority = vm
        .ws_uri
        .trim_start_matches("ws://")
        .trim_end_matches("/ws");
    let dart = dir.join("dart");
    std::fs::write(
        &dart,
        format!(
            "#!/bin/sh\n\
             echo 'The Dart VM service is listening on http://{authority}/'\n\
             while [ ! -e '{}' ]; do sleep 0.01; done\n\
             exit 7\n",
            release.display()
        ),
    )
    .expect("write the stand-in dart");
    std::fs::set_permissions(&dart, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    let config = LaunchConfig {
        launch_timeout: Duration::from_secs(10),
        ..LaunchConfig::new("main.dart")
    };
    let settings = SessionConfig {
        rpc_timeout: Duration::from_secs(5),
        pause_timeout: Duration::from_secs(5),
        ..SessionConfig::default()
    };
    // SAFETY: this test binary holds exactly one test, so nothing else
    // reads or writes the environment concurrently.
    unsafe { std::env::set_var("DARTQ_DART", &dart) };
    let launched = DebugSession::launch_with(config, settings);
    unsafe { std::env::remove_var("DARTQ_DART") };
    let mut session = launched.expect("launch against the mock");

    // The process lives on: an isolate whose exit the stream showed is
    // named; a sentinel the stream never explained stands.
    let error = session
        .stack(Some("isolates/2"), 1)
        .expect_err("the worker is gone");
    assert!(
        matches!(error, Error::IsolateExited(ref id) if id == "isolates/2"),
        "got {error:?}"
    );
    let error = session
        .pause(Some("isolates/1"))
        .expect_err("the VM answered a sentinel");
    assert!(
        matches!(error, Error::Sentinel { ref kind, .. } if kind == "Collected"),
        "got {error:?}"
    );

    // The transport closes, the process still lives: the worker's exit
    // is still the answer for it, a bare transport loss for the main
    // isolate.
    vm.control().send(Control::Close).expect("close the mock");
    wait_for_disconnect(&vm);
    let error = session
        .stack(Some("isolates/1"), 1)
        .expect_err("the transport is closed");
    assert!(matches!(error, Error::Closed), "got {error:?}");
    let error = session
        .stack(Some("isolates/2"), 1)
        .expect_err("the transport is closed");
    assert!(
        matches!(error, Error::IsolateExited(ref id) if id == "isolates/2"),
        "got {error:?}"
    );

    // The process exits: its status outranks every other reading, for
    // every isolate, on every later call.
    std::fs::write(&release, b"").expect("release the stand-in");
    let deadline = Instant::now() + Duration::from_secs(10);
    let exited = loop {
        match session.stack(Some("isolates/1"), 1) {
            // Still on its way out.
            Err(Error::Closed) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            other => break other,
        }
    };
    assert!(
        matches!(exited, Err(Error::ProcessExited(Some(7)))),
        "got {exited:?}"
    );
    let error = session
        .stack(Some("isolates/2"), 1)
        .expect_err("the process exited");
    assert!(
        matches!(error, Error::ProcessExited(Some(7))),
        "got {error:?}"
    );
    let error = session
        .evaluate_in_root_lib(None, "1 + 1")
        .expect_err("the process exited");
    assert!(
        matches!(error, Error::ProcessExited(Some(7))),
        "got {error:?}"
    );
    assert_eq!(error.to_string(), "process exited with code 7");
    let error = session.pause(None).expect_err("the process exited");
    assert!(
        matches!(error, Error::ProcessExited(Some(7))),
        "got {error:?}"
    );

    drop(session);
    std::fs::remove_dir_all(&dir).expect("clean temp dir");
}
