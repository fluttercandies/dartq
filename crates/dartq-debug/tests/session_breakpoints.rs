//! Session tests for the handshake, breakpoint lifecycle, pause-mode
//! control, and the read-only query surface.

mod common;

use std::time::Duration;

use common::{
    Control, MockVm, Reply, default_reply, err, instance_ref, isolate_ref, ok, source_location,
    stack_with_vars,
};
use dartq_debug::{
    BreakpointLocationSpec, BreakpointSpec, DebugSession, Error, ExceptionPauseMode, SessionConfig,
};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(5);

fn fast_config() -> SessionConfig {
    SessionConfig {
        pause_timeout: Duration::from_secs(5),
        rpc_timeout: Duration::from_secs(5),
        ..SessionConfig::default()
    }
}

fn line_spec(line: u32) -> BreakpointSpec {
    BreakpointSpec {
        isolate: None,
        location: BreakpointLocationSpec::Line {
            script_uri: "package:app/main.dart".to_owned(),
            line,
            column: None,
        },
        condition: None,
    }
}

fn resolved_breakpoint(id: &str) -> Value {
    json!({
        "type": "Breakpoint",
        "id": id,
        "breakpointNumber": 1,
        "enabled": true,
        "resolved": true,
        "location": source_location("scripts/1", "package:app/main.dart", 7, 3),
    })
}

#[test]
fn attach_performs_version_streams_then_vm() {
    let vm = MockVm::start_default();
    let session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let mut methods = Vec::new();
    let mut streams = Vec::new();
    for _ in 0..8 {
        let request = vm.requests.recv_timeout(WAIT).expect("handshake request");
        if request.method == "streamListen" {
            streams.push(
                request.params["streamId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            );
        }
        methods.push(request.method);
    }
    assert_eq!(
        methods,
        [
            "getVersion",
            "streamListen",
            "streamListen",
            "streamListen",
            "streamListen",
            "streamListen",
            "streamListen",
            "getVM",
        ]
    );
    // Attach mode subscribes the output streams too (a launched
    // session reads the child's pipes instead), and the Service stream
    // carries tool-service registrations (DDS replays them — how a
    // Flutter session finds the tool's reload/restart services).
    assert_eq!(
        streams,
        ["Debug", "Isolate", "Logging", "Service", "Stdout", "Stderr"]
    );
    assert_eq!(session.main_isolate(), Some("isolates/1"));
    assert_eq!(session.isolates().count(), 1);
    assert_eq!(session.config().pause_timeout, Duration::from_secs(5));
}

#[test]
fn attach_rejects_incompatible_major_version() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "getVersion" => ok(json!({ "type": "Version", "major": 3, "minor": 5 })),
        _ => default_reply(request),
    });
    let error = DebugSession::attach_with(&vm.ws_uri, fast_config())
        .err()
        .expect("major 3 must be rejected");
    match error {
        Error::VersionIncompatible {
            expected_major,
            found_major,
            found_minor,
        } => {
            assert_eq!(expected_major, 4);
            assert_eq!(found_major, 3);
            assert_eq!(found_minor, 5);
        }
        other => panic!("expected VersionIncompatible, got {other:?}"),
    }
}

#[test]
fn attach_tolerates_already_subscribed_streams() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "streamListen" => err(103, "Stream already subscribed"),
        _ => default_reply(request),
    });
    let session = DebugSession::attach_with(&vm.ws_uri, fast_config())
        .expect("103 is benign during handshake");
    assert_eq!(session.main_isolate(), Some("isolates/1"));
}

#[test]
fn add_line_breakpoint_round_trip() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "addBreakpointWithScriptUri" => ok(resolved_breakpoint("breakpoints/1")),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let mut spec = line_spec(7);
    spec.location = BreakpointLocationSpec::Line {
        script_uri: "package:app/main.dart".to_owned(),
        line: 7,
        column: Some(3),
    };
    let handle = session.add_breakpoint(spec).expect("add breakpoint");
    assert_eq!(handle.id, "breakpoints/1");
    assert_eq!(handle.isolate, "isolates/1");
    assert!(handle.resolved);
    assert_eq!(session.breakpoints().count(), 1);

    let request = vm.expect_request("addBreakpointWithScriptUri");
    assert_eq!(request.params["isolateId"], "isolates/1");
    assert_eq!(request.params["scriptUri"], "package:app/main.dart");
    assert_eq!(request.params["line"], 7);
    assert_eq!(request.params["column"], 3);
}

#[test]
fn add_entry_breakpoint_uses_function_rpc() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "addBreakpointAtEntry" => ok(json!({
            "type": "Breakpoint",
            "id": "breakpoints/2",
            "breakpointNumber": 2,
            "enabled": true,
            "resolved": false,
            "location": {
                "type": "UnresolvedSourceLocation",
                "scriptUri": "package:app/main.dart",
                "line": 11,
            },
        })),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let handle = session
        .add_breakpoint(BreakpointSpec {
            isolate: None,
            location: BreakpointLocationSpec::Function {
                function_id: "functions/9".to_owned(),
            },
            condition: None,
        })
        .expect("add entry breakpoint");
    assert!(!handle.resolved);

    let request = vm.expect_request("addBreakpointAtEntry");
    assert_eq!(request.params["functionId"], "functions/9");
    assert_eq!(request.params["isolateId"], "isolates/1");
}

#[test]
fn add_breakpoint_rpc_error_102_is_structured() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "addBreakpointWithScriptUri" => err(102, "Cannot add breakpoint"),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let error = session
        .add_breakpoint(line_spec(7))
        .expect_err("102 must surface");
    assert!(
        matches!(error, Error::Rpc { code: 102, .. }),
        "got {error:?}"
    );
    assert_eq!(session.breakpoints().count(), 0);
}

#[test]
fn breakpoint_resolved_event_updates_handle() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "addBreakpointWithScriptUri" => ok(json!({
            "type": "Breakpoint",
            "id": "breakpoints/3",
            "breakpointNumber": 3,
            "enabled": true,
            "resolved": false,
        })),
        "getStack" => Reply::Prefixed {
            prelude: vec![Control::Event {
                stream_id: "Debug".to_owned(),
                event: json!({
                    "type": "Event",
                    "kind": "BreakpointResolved",
                    "isolate": isolate_ref("isolates/1", "1", "main"),
                    "timestamp": 1_i64,
                    "breakpoint": resolved_breakpoint("breakpoints/3"),
                }),
            }],
            result: stack_with_vars(json!([])),
        },
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let handle = session
        .add_breakpoint(line_spec(7))
        .expect("add breakpoint");
    assert!(!handle.resolved);

    session.stack(None, 1).expect("barrier rpc");
    session.poll_events().expect("poll events");
    let tracked = session
        .breakpoints()
        .find(|tracked| tracked.id == "breakpoints/3")
        .expect("breakpoint tracked");
    assert!(tracked.resolved, "BreakpointResolved must flip the handle");
}

#[test]
fn breakpoint_removed_event_forgets_handle() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "addBreakpointWithScriptUri" => ok(resolved_breakpoint("breakpoints/4")),
        "getStack" => Reply::Prefixed {
            prelude: vec![Control::Event {
                stream_id: "Debug".to_owned(),
                event: json!({
                    "type": "Event",
                    "kind": "BreakpointRemoved",
                    "isolate": isolate_ref("isolates/1", "1", "main"),
                    "timestamp": 1_i64,
                    "breakpoint": resolved_breakpoint("breakpoints/4"),
                }),
            }],
            result: stack_with_vars(json!([])),
        },
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    session
        .add_breakpoint(line_spec(7))
        .expect("add breakpoint");
    session.stack(None, 1).expect("barrier rpc");
    session.poll_events().expect("poll events");
    assert_eq!(session.breakpoints().count(), 0);
}

#[test]
fn remove_breakpoint_sends_rpc_and_forgets_handle() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "addBreakpointWithScriptUri" => ok(resolved_breakpoint("breakpoints/5")),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let handle = session
        .add_breakpoint(line_spec(7))
        .expect("add breakpoint");
    session
        .remove_breakpoint(None, &handle.id)
        .expect("remove breakpoint");
    assert_eq!(session.breakpoints().count(), 0);

    let request = vm.expect_request("removeBreakpoint");
    assert_eq!(request.params["breakpointId"], "breakpoints/5");
    assert_eq!(request.params["isolateId"], "isolates/1");

    session
        .remove_breakpoint(None, "breakpoints/unknown")
        .expect("removing an unknown id is a no-op");
}

#[test]
fn set_exception_pause_mode_prefers_forward_rpc() {
    let vm = MockVm::start_default();
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    session
        .set_exception_pause_mode(None, ExceptionPauseMode::All)
        .expect("set pause mode");
    let request = vm.expect_request("setIsolatePauseMode");
    assert_eq!(request.params["isolateId"], "isolates/1");
    assert_eq!(request.params["exceptionPauseMode"], "All");
    assert!(
        vm.requests.try_recv().is_err(),
        "the deprecated RPC must not be called when the forward RPC works"
    );
}

#[test]
fn set_exception_pause_mode_falls_back_to_deprecated_rpc() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "setIsolatePauseMode" => err(-32601, "Method not found"),
        "setExceptionPauseMode" => ok(json!({ "type": "Success" })),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    session
        .set_exception_pause_mode(None, ExceptionPauseMode::Unhandled)
        .expect("fallback succeeds");
    let forward = vm.expect_request("setIsolatePauseMode");
    assert_eq!(forward.params["exceptionPauseMode"], "Unhandled");
    let deprecated = vm.expect_request("setExceptionPauseMode");
    assert_eq!(deprecated.params["mode"], "Unhandled");
}

#[test]
fn stack_returns_frames() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "getStack" => ok(stack_with_vars(json!([
            { "name": "count", "value": instance_ref("objects/7", "Int", "3") }
        ]))),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let stack = session.stack(None, 20).expect("getStack");
    assert_eq!(stack.frames.len(), 1);
    let frame = &stack.frames[0];
    assert_eq!(
        frame.function.as_ref().and_then(|f| f.name.as_deref()),
        Some("main")
    );
    let location = frame.location.as_ref().expect("location");
    assert_eq!(location.line, Some(7));
    assert_eq!(location.column, Some(3));
    assert_eq!(frame.vars[0].name, "count");

    let request = vm.expect_request("getStack");
    assert_eq!(request.params["limit"], 20);
}

#[test]
fn scripts_lists_scripts() {
    let vm = MockVm::start_default();
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let scripts = session.scripts(None).expect("getScripts");
    assert_eq!(scripts.len(), 1);
    assert_eq!(scripts[0].id, "scripts/1");
    assert_eq!(scripts[0].uri.as_deref(), Some("package:app/main.dart"));
}

#[test]
fn isolate_fetch_returns_object() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "getIsolate" => ok(json!({
            "type": "Isolate",
            "id": "isolates/1",
            "number": "1",
            "name": "main",
            "runnable": true,
            "breakpoints": [resolved_breakpoint("breakpoints/1")],
            "exceptionPauseMode": "All",
            "rootLib": { "type": "@Library", "id": "libraries/1", "uri": "package:app/main.dart" },
        })),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let isolate = session.isolate(None).expect("getIsolate");
    assert!(isolate.runnable);
    assert_eq!(isolate.breakpoints.len(), 1);
    assert_eq!(isolate.exception_pause_mode, Some(ExceptionPauseMode::All));
    assert_eq!(
        isolate.root_lib.map(|library| library.id),
        Some("libraries/1".to_owned())
    );
}

#[test]
fn isolate_exit_reelects_main_and_clears_tracking() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "getVM" => ok(json!({
            "type": "VM",
            "name": "mock-vm",
            "isolates": [
                isolate_ref("isolates/1", "1", "main"),
                isolate_ref("isolates/2", "2", "worker"),
            ],
            "systemIsolates": [],
        })),
        "getStack" => {
            // First barrier: main exits; second: worker exits.
            static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let call = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let exiting = if call == 0 {
                ("isolates/1", "1", "main")
            } else {
                ("isolates/2", "2", "worker")
            };
            Reply::Prefixed {
                prelude: vec![Control::Event {
                    stream_id: "Isolate".to_owned(),
                    event: json!({
                        "type": "Event",
                        "kind": "IsolateExit",
                        "isolate": isolate_ref(exiting.0, exiting.1, exiting.2),
                        "timestamp": 1_i64,
                    }),
                }],
                result: stack_with_vars(json!([])),
            }
        }
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    assert_eq!(session.main_isolate(), Some("isolates/1"));
    assert_eq!(session.isolates().count(), 2);

    session.stack(None, 1).expect("barrier rpc");
    session.poll_events().expect("poll events");
    assert_eq!(session.main_isolate(), Some("isolates/2"));
    assert_eq!(session.isolates().count(), 1);

    session.stack(None, 1).expect("barrier rpc");
    session.poll_events().expect("poll events");
    assert_eq!(session.main_isolate(), None);
    assert_eq!(session.isolates().count(), 0);
    let error = session
        .add_breakpoint(line_spec(7))
        .expect_err("no isolate left");
    assert!(matches!(error, Error::Protocol(_)), "got {error:?}");
}

#[test]
fn no_isolate_is_a_protocol_error() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "getVM" => ok(json!({ "type": "VM", "name": "empty", "isolates": [] })),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let error = session
        .add_breakpoint(line_spec(7))
        .expect_err("no isolate to target");
    assert!(
        matches!(error, Error::Protocol(ref message) if message.contains("no isolate")),
        "got {error:?}"
    );
}

#[test]
fn terminate_closes_the_connection() {
    let vm = MockVm::start_default();
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    session.terminate().expect("terminate attached session");
    let error = session
        .stack(None, 1)
        .expect_err("closed connection must fail");
    assert!(matches!(error, Error::Closed), "got {error:?}");
}

/// Waits until the mock's request sender disconnects — i.e. the
/// serve loop has exited — draining already-queued requests first.
fn wait_for_mock_disconnect(vm: &MockVm) {
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match vm.requests.recv_timeout(remaining) {
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
            Ok(_) | Err(crossbeam_channel::RecvTimeoutError::Timeout)
                if std::time::Instant::now() < deadline => {}
            other => panic!("expected mock serve loop to exit, got {other:?}"),
        }
    }
}

#[test]
fn drop_without_terminate_closes_the_connection() {
    let vm = MockVm::start_default();
    let session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    // No `terminate` call — Drop must still tear down the client:
    // the last handle pushes `Wire::Close` and joins the connection
    // thread, and the mock's serve loop exits on the WebSocket close.
    drop(session);
    wait_for_mock_disconnect(&vm);
}

#[test]
fn terminate_then_drop_is_safe() {
    let vm = MockVm::start_default();
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    session.terminate().expect("terminate");
    // Double teardown: terminate's close plus Drop's terminate must
    // not panic, deadlock, or error.
    drop(session);
    wait_for_mock_disconnect(&vm);
}

#[test]
fn unknown_direction_events_are_tolerated() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "getStack" => Reply::Prefixed {
            prelude: vec![Control::Event {
                stream_id: "Service".to_owned(),
                event: json!({
                    "type": "Event",
                    "kind": "SomethingAddedInAFutureProtocol",
                    "timestamp": 1_i64,
                }),
            }],
            result: stack_with_vars(json!([])),
        },
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    session.stack(None, 1).expect("barrier rpc");
    session.poll_events().expect("unknown event must not fail");
    let batch = session.events_since(0, usize::MAX);
    assert_eq!(batch.events.len(), 1);
    assert_eq!(batch.events[0].stream_id, "Service");
    assert_eq!(batch.next_cursor, 1);
    // The session keeps working after an unknown event.
    assert_eq!(session.scripts(None).expect("scripts").len(), 1);
}
