//! Connection-level tests: version negotiation, id-matched responses,
//! RPC errors, timeouts, disconnects, and event delivery over the
//! in-process mock VM service.

mod common;

use std::time::Duration;

use common::{Control, MockVm, Reply, default_reply, err, err_with_details, ok};
use dartq_debug::{DebugSession, Error, EventKind, SessionConfig, VmClient};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(5);

fn event(kind: &str) -> Value {
    json!({ "type": "Event", "kind": kind, "timestamp": 1_i64 })
}

#[test]
fn connect_negotiates_protocol_version() {
    let vm = MockVm::start_default();
    let client = VmClient::connect(&vm.ws_uri).expect("connect to mock");
    let version = client.get_version().expect("getVersion");
    assert_eq!(version.major, 4);
    assert_eq!(version.minor, 23);
}

#[test]
fn rpc_error_is_structured() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "addBreakpointWithScriptUri" => err(102, "Cannot add breakpoint"),
        _ => default_reply(request),
    });
    let client = VmClient::connect(&vm.ws_uri).expect("connect to mock");
    let error = client
        .call(
            "addBreakpointWithScriptUri",
            json!({ "isolateId": "isolates/1", "scriptUri": "package:app/main.dart", "line": 3 }),
            WAIT,
        )
        .expect_err("102 must fail");
    match error {
        Error::Rpc {
            code,
            message,
            details,
        } => {
            assert_eq!(code, 102);
            assert_eq!(message, "Cannot add breakpoint");
            assert_eq!(details, None);
        }
        other => panic!("expected Rpc error, got {other:?}"),
    }
}

#[test]
fn rpc_error_details_are_captured() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "evaluateInFrame" => {
            err_with_details(113, "Expression compilation error", "undefined name 'x'")
        }
        _ => default_reply(request),
    });
    let client = VmClient::connect(&vm.ws_uri).expect("connect to mock");
    let error = client
        .call("evaluateInFrame", json!({}), WAIT)
        .expect_err("113 must fail");
    match error {
        Error::Rpc { code, details, .. } => {
            assert_eq!(code, 113);
            assert_eq!(details.as_deref(), Some("undefined name 'x'"));
        }
        other => panic!("expected Rpc error, got {other:?}"),
    }
}

#[test]
fn rpc_timeout_is_reported_and_connection_survives() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "hang" => Reply::Silent,
        _ => default_reply(request),
    });
    let client = VmClient::connect(&vm.ws_uri).expect("connect to mock");
    let error = client
        .call("hang", json!({}), Duration::from_millis(150))
        .expect_err("silent peer must time out");
    assert!(matches!(error, Error::Timeout(_)), "got {error:?}");
    assert_eq!(
        client.get_version().expect("connection still alive").major,
        4
    );
}

#[test]
fn connection_close_answers_pending_call() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "hang" => Reply::Silent,
        _ => default_reply(request),
    });
    let client = VmClient::connect(&vm.ws_uri).expect("connect to mock");
    let pending = client.clone();
    let (answer_tx, answer_rx) = crossbeam_channel::bounded(1);
    std::thread::spawn(move || {
        let result = pending.call("hang", json!({}), WAIT);
        let _ = answer_tx.send(result);
    });
    vm.expect_request("hang");
    vm.control()
        .send(Control::Close)
        .expect("request mock close");
    let result = answer_rx
        .recv_timeout(WAIT)
        .expect("pending call must be answered on close");
    assert!(matches!(result, Err(Error::Closed)), "got {result:?}");
}

#[test]
fn close_fails_subsequent_calls() {
    let vm = MockVm::start_default();
    let client = VmClient::connect(&vm.ws_uri).expect("connect to mock");
    client.close();
    let error = client
        .call("getVersion", json!({}), Duration::from_secs(2))
        .expect_err("closed client must not answer");
    assert!(matches!(error, Error::Closed), "got {error:?}");
}

#[test]
fn stray_response_id_is_discarded() {
    let vm = MockVm::start_default();
    let client = VmClient::connect(&vm.ws_uri).expect("connect to mock");
    vm.control()
        .send(Control::Raw(
            json!({ "jsonrpc": "2.0", "id": "99999", "result": { "type": "Version" } }).to_string(),
        ))
        .expect("send stray response");
    assert_eq!(client.get_version().expect("connection survives").major, 4);
}

#[test]
fn malformed_frame_does_not_kill_connection() {
    let vm = MockVm::start_default();
    let client = VmClient::connect(&vm.ws_uri).expect("connect to mock");
    vm.control()
        .send(Control::Raw("{not json".to_owned()))
        .expect("send malformed frame");
    assert_eq!(client.get_version().expect("connection survives").major, 4);
}

#[test]
fn unknown_event_kind_is_preserved() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "getVM" => Reply::Prefixed {
            prelude: vec![Control::Event {
                stream_id: "Debug".to_owned(),
                event: event("SomeFuturePauseKind"),
            }],
            result: json!({ "type": "VM", "name": "mock", "isolates": [] }),
        },
        _ => default_reply(request),
    });
    let client = VmClient::connect(&vm.ws_uri).expect("connect to mock");
    client.get_vm().expect("barrier rpc");
    let stream_event = client
        .next_event(WAIT)
        .expect("event feed alive")
        .expect("one event");
    assert_eq!(stream_event.stream_id, "Debug");
    match stream_event.event.kind {
        EventKind::Other(kind) => assert_eq!(kind, "SomeFuturePauseKind"),
        other => panic!("expected Other event kind, got {other:?}"),
    }
    assert_eq!(client.get_version().expect("connection survives").major, 4);
}

#[test]
fn events_arrive_in_order_and_drain() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "getVM" => Reply::Prefixed {
            prelude: vec![
                Control::Event {
                    stream_id: "Isolate".to_owned(),
                    event: event("IsolateStart"),
                },
                Control::Event {
                    stream_id: "Debug".to_owned(),
                    event: event("Resume"),
                },
                Control::Event {
                    stream_id: "Debug".to_owned(),
                    event: event("PauseStart"),
                },
            ],
            result: json!({ "type": "VM", "name": "mock", "isolates": [] }),
        },
        _ => default_reply(request),
    });
    let client = VmClient::connect(&vm.ws_uri).expect("connect to mock");
    client.get_vm().expect("barrier rpc");
    let events = client.drain_events();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].seq, 0);
    assert_eq!(events[1].seq, 1);
    assert_eq!(events[2].seq, 2);
    assert_eq!(events[0].stream_id, "Isolate");
    assert!(matches!(events[0].event.kind, EventKind::IsolateStart));
    assert!(matches!(events[1].event.kind, EventKind::Resume));
    assert!(matches!(events[2].event.kind, EventKind::PauseStart));
    assert!(client.drain_events().is_empty());
}

#[test]
fn stream_listen_and_cancel_round_trip() {
    let vm = MockVm::start_default();
    let client = VmClient::connect(&vm.ws_uri).expect("connect to mock");
    client.stream_listen("Debug").expect("streamListen");
    client.stream_cancel("Debug").expect("streamCancel");
    let request = vm.expect_request("streamListen");
    assert_eq!(request.params["streamId"], "Debug");
}

#[test]
fn stream_listen_reports_duplicate_subscription() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "streamListen" => err(103, "Stream already subscribed"),
        _ => default_reply(request),
    });
    let client = VmClient::connect(&vm.ws_uri).expect("connect to mock");
    let error = client.stream_listen("Debug").expect_err("103 must surface");
    assert!(
        matches!(error, Error::Rpc { code: 103, .. }),
        "got {error:?}"
    );
}

#[test]
fn sentinel_result_fails_call_unit() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "getObject" => ok(json!({
            "type": "Sentinel",
            "kind": "Collected",
            "valueAsString": "<collected>",
        })),
        _ => default_reply(request),
    });
    let client = VmClient::connect(&vm.ws_uri).expect("connect to mock");
    let error = client
        .call_unit("getObject", json!({}), WAIT)
        .expect_err("sentinel must fail");
    match error {
        Error::Sentinel {
            kind,
            value_as_string,
        } => {
            assert_eq!(kind, "Collected");
            assert_eq!(value_as_string.as_deref(), Some("<collected>"));
        }
        other => panic!("expected Sentinel error, got {other:?}"),
    }
}

#[test]
fn last_client_drop_closes_the_connection() {
    let vm = MockVm::start_default();
    let client = VmClient::connect(&vm.ws_uri).expect("connect to mock");
    let clone = client.clone();
    clone.get_version().expect("clone shares the connection");
    drop(client);
    // The clone keeps the connection alive — a call must still work.
    assert_eq!(clone.get_version().expect("still connected").major, 4);
    drop(clone);
    // The last handle sends `Wire::Close` and joins the connection
    // thread; the mock's serve loop exits on the WebSocket close and
    // drops its request sender — a disconnect (after any queued
    // requests drain) proves teardown ran.
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        match vm
            .requests
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
        {
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            Ok(_) | Err(crossbeam_channel::RecvTimeoutError::Timeout)
                if std::time::Instant::now() < deadline => {}
            other => panic!("expected mock serve loop to exit, got {other:?}"),
        }
    }
}

#[test]
fn dropped_events_counts_feed_overruns() {
    // Saturating the bounded event feed must be visible, not silent:
    // `dropped_events` counts exactly what could not be delivered.
    let overflow = dartq_debug::EVENT_BUFFER_CAPACITY + 2;
    let prelude: Vec<Control> = (0..overflow)
        .map(|n| Control::Event {
            stream_id: "Debug".to_owned(),
            event: json!({ "type": "Event", "kind": "Resume", "timestamp": n as i64 }),
        })
        .collect();
    let vm = MockVm::start(move |request| match request.method.as_str() {
        "getVM" => Reply::Prefixed {
            prelude: prelude.clone(),
            result: json!({ "type": "VM", "name": "mock", "isolates": [] }),
        },
        _ => default_reply(request),
    });
    let client = VmClient::connect(&vm.ws_uri).expect("connect to mock");
    client.get_vm().expect("barrier rpc");
    let deadline = std::time::Instant::now() + WAIT;
    while client.dropped_events() < 2 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(client.dropped_events(), 2);
    assert_eq!(
        client.drain_events().len(),
        dartq_debug::EVENT_BUFFER_CAPACITY
    );
}

#[test]
fn typed_decoding_error_is_protocol_error() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "getVersion" => ok(json!({ "type": "Version", "major": "four", "minor": 23 })),
        _ => default_reply(request),
    });
    let client = VmClient::connect(&vm.ws_uri).expect("connect to mock");
    let error = client.get_version().expect_err("bad shape must fail");
    assert!(
        matches!(error, Error::Protocol(ref message) if message.contains("getVersion")),
        "got {error:?}"
    );
}

/// #2587: a peer that accepts the TCP connection but never answers the
/// WebSocket upgrade (the stalled-DDS shape) fails within the connect
/// bound — the client never blocks forever on the handshake.
#[test]
fn connect_fails_within_bound_against_a_stalled_handshake() {
    use std::net::TcpListener;

    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let port = listener.local_addr().expect("addr").port();
    // Accept and hold the socket open without writing the upgrade
    // answer — the wedge shape, forever.
    std::thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            std::mem::forget(stream);
        }
    });
    let started = std::time::Instant::now();
    let error = VmClient::connect_bounded(
        &format!("ws://127.0.0.1:{port}/auth=/ws"),
        Duration::from_secs(2),
    )
    .err()
    .expect("a stalled handshake must fail");
    assert!(
        matches!(error, Error::Timeout(ref message) if message.contains("handshake")),
        "the structured timeout: {error:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "the failure is bounded: {:?}",
        started.elapsed()
    );
}

/// #2587: an endpoint that completes the WebSocket handshake but never
/// answers VM Service requests fails within the handshake budget — not
/// `rpc_timeout` per round trip. The budget bounds the whole phase
/// (version, streams, getVM, isolate seeding) however many calls it
/// has left.
#[test]
fn attach_fails_within_the_handshake_budget_when_rpcs_stall() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "getVersion" => Reply::Silent,
        _ => default_reply(request),
    });
    let settings = SessionConfig {
        connect_timeout: Duration::from_secs(2),
        handshake_timeout: Duration::from_secs(3),
        rpc_timeout: Duration::from_secs(30),
        ..SessionConfig::default()
    };
    let started = std::time::Instant::now();
    let error = DebugSession::attach_with(&vm.ws_uri, settings)
        .err()
        .expect("a silent vm must fail");
    assert!(
        matches!(error, Error::Timeout(ref message) if message.contains("getVersion")),
        "the structured timeout names the stalled rpc: {error:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the handshake budget (3s), not the rpc timeout (30s), bounds the failure: {:?}",
        started.elapsed()
    );
}
