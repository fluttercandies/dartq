//! Regression tests for the #2532 production-grade audit: each test
//! pins one defect that the earlier suite did not cover — per-isolate
//! breakpoint identity, attach-time pause seeding, stale-pause
//! rejection, isolate exit during a wait, step reasons, condition
//! truthiness, wire-exact event gaps, paging, bounded expansion, and
//! RPC timeout hygiene.

mod common;

use std::time::Duration;

use common::{
    Control, MockVm, Reply, default_reply, instance_ref, isolate_ref, ok, pause_breakpoint_event,
    source_location, stack_with_vars,
};
use dartq_debug::{
    BreakpointLocationSpec, BreakpointSpec, ConditionalResumeLimit, DebugSession, Error,
    SessionConfig, StoppedReason, VmClient,
};
use serde_json::{Value, json};

fn fast_config() -> SessionConfig {
    SessionConfig {
        pause_timeout: Duration::from_secs(5),
        rpc_timeout: Duration::from_secs(5),
        ..SessionConfig::default()
    }
}

fn event(stream_id: &str, event: Value) -> Control {
    Control::Event {
        stream_id: stream_id.to_owned(),
        event,
    }
}

fn breakpoint(id: &str) -> Value {
    json!({
        "type": "Breakpoint",
        "id": id,
        "breakpointNumber": 1,
        "enabled": true,
        "resolved": true,
        "location": source_location("scripts/1", "package:app/main.dart", 7, 3),
    })
}

fn line_spec(isolate: &str, condition: Option<&str>) -> BreakpointSpec {
    BreakpointSpec {
        isolate: Some(isolate.to_owned()),
        location: BreakpointLocationSpec::Line {
            script_uri: "package:app/main.dart".to_owned(),
            line: 7,
            column: None,
        },
        condition: condition.map(str::to_owned),
    }
}

fn two_isolate_vm() -> Value {
    json!({
        "type": "VM",
        "name": "mock-vm",
        "isolates": [
            isolate_ref("isolates/1", "1", "main"),
            isolate_ref("isolates/2", "2", "worker"),
        ],
        "systemIsolates": [],
    })
}

fn resume_event(n: i64) -> Value {
    json!({ "type": "Event", "kind": "Resume", "timestamp": n })
}

/// The VM numbers breakpoints per isolate, so `breakpoints/1` exists
/// in two isolates at once. Both handles must survive, and removing
/// one must address the right isolate and leave the other armed.
#[test]
fn same_breakpoint_id_in_two_isolates_is_tracked_separately() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "getVM" => ok(two_isolate_vm()),
        "addBreakpointWithScriptUri" => ok(breakpoint("breakpoints/1")),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    session
        .add_breakpoint(line_spec("isolates/1", None))
        .expect("bp on isolate 1");
    session
        .add_breakpoint(line_spec("isolates/2", Some("ready")))
        .expect("bp on isolate 2");
    assert_eq!(session.breakpoints().count(), 2, "ids collide across isolates");

    session
        .remove_breakpoint(Some("isolates/2"), "breakpoints/1")
        .expect("remove isolate 2's breakpoint");
    let request = vm.expect_request("removeBreakpoint");
    assert_eq!(request.params["isolateId"], "isolates/2");
    let remaining: Vec<_> = session.breakpoints().collect();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].isolate, "isolates/1");
    assert_eq!(remaining[0].condition, None);
}

/// A rejected `removeBreakpoint` keeps the handle so the caller can
/// retry; the old code forgot it before the RPC answered.
#[test]
fn failed_remove_keeps_the_handle() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "addBreakpointWithScriptUri" => ok(breakpoint("breakpoints/1")),
        "removeBreakpoint" => common::err(-32602, "Invalid params"),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    session
        .add_breakpoint(line_spec("isolates/1", None))
        .expect("add");
    let error = session
        .remove_breakpoint(None, "breakpoints/1")
        .expect_err("VM rejects the removal");
    assert!(matches!(error, Error::Rpc { code: -32602, .. }), "got {error:?}");
    assert_eq!(session.breakpoints().count(), 1);
}

/// An isolate's exit drops its breakpoints from the registry — a
/// daemon-lifetime VM spawning many isolates must not accumulate
/// handles for dead ones.
#[test]
fn isolate_exit_drops_its_breakpoints() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "getVM" => ok(two_isolate_vm()),
        "addBreakpointWithScriptUri" => ok(breakpoint("breakpoints/1")),
        "getStack" => Reply::Prefixed {
            prelude: vec![event(
                "Isolate",
                json!({
                    "type": "Event",
                    "kind": "IsolateExit",
                    "isolate": isolate_ref("isolates/2", "2", "worker"),
                    "timestamp": 1_i64,
                }),
            )],
            result: stack_with_vars(json!([])),
        },
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    session.add_breakpoint(line_spec("isolates/1", None)).expect("bp 1");
    session.add_breakpoint(line_spec("isolates/2", None)).expect("bp 2");
    session.stack(Some("isolates/1"), 1).expect("barrier");
    session.poll_events().expect("poll");
    let remaining: Vec<_> = session.breakpoints().map(|bp| bp.isolate.as_str()).collect();
    assert_eq!(remaining, ["isolates/1"]);
    assert_eq!(session.isolates().count(), 1);
}

/// Attaching to an isolate that is already paused must report that
/// pause — the VM never re-sends a pause event to a new client.
#[test]
fn attach_seeds_existing_pause_state() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "getIsolate" => ok(json!({
            "type": "Isolate",
            "id": "isolates/1",
            "number": "1",
            "name": "main",
            "runnable": true,
            "pauseEvent": {
                "type": "Event",
                "kind": "PauseStart",
                "isolate": isolate_ref("isolates/1", "1", "main"),
                "timestamp": 5_i64,
            },
            "rootLib": { "type": "@Library", "id": "libraries/7", "uri": "package:app/main.dart" },
        })),
        "getStack" => ok(stack_with_vars(json!([]))),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let snapshot = session
        .wait_for_pause("isolates/1")
        .expect("seeded pause reports immediately");
    assert_eq!(snapshot.reason, StoppedReason::Start);
}

/// A pause that arrived *before* a resume (another client's stop, or
/// a stale record) must not be returned as that resume's result.
#[test]
fn stale_pause_before_resume_is_not_reported() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        // The barrier queues an old PauseInterrupted ahead of time.
        "getStack" if request.params["limit"] == json!(1) => Reply::Prefixed {
            prelude: vec![event(
                "Debug",
                json!({
                    "type": "Event",
                    "kind": "PauseInterrupted",
                    "isolate": isolate_ref("isolates/1", "1", "main"),
                    "timestamp": 1_i64,
                }),
            )],
            result: stack_with_vars(json!([])),
        },
        "resume" => Reply::Prefixed {
            prelude: vec![event(
                "Debug",
                pause_breakpoint_event("isolates/1", &["breakpoints/9"]),
            )],
            result: json!({ "type": "Success" }),
        },
        "getStack" => ok(stack_with_vars(json!([]))),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    session.stack(None, 1).expect("barrier queues the stale pause");
    let snapshot = session.step_and_package(None, None).expect("resume");
    assert_eq!(
        snapshot.reason,
        StoppedReason::Breakpoint {
            breakpoints: vec!["breakpoints/9".to_owned()],
        },
        "the stop must be the one after the resume"
    );
}

/// Waiting on an isolate that exits fails fast with `IsolateExited`
/// instead of burning the whole pause timeout.
#[test]
fn wait_for_pause_fails_fast_when_the_isolate_exits() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "resume" => Reply::Prefixed {
            prelude: vec![event(
                "Isolate",
                json!({
                    "type": "Event",
                    "kind": "IsolateExit",
                    "isolate": isolate_ref("isolates/1", "1", "main"),
                    "timestamp": 2_i64,
                }),
            )],
            result: json!({ "type": "Success" }),
        },
        _ => default_reply(request),
    });
    let settings = SessionConfig {
        pause_timeout: Duration::from_secs(30),
        ..fast_config()
    };
    let mut session = DebugSession::attach_with(&vm.ws_uri, settings).expect("attach");
    let started = std::time::Instant::now();
    let error = session
        .step_and_package(None, None)
        .expect_err("exited isolate cannot pause");
    assert!(matches!(error, Error::IsolateExited(ref id) if id == "isolates/1"), "got {error:?}");
    assert!(started.elapsed() < Duration::from_secs(5), "waited {:?}", started.elapsed());
}

/// The spec's single-step pause is a `PauseBreakpoint` with no
/// breakpoints; it is reported as `Step`, not a breakpoint hit.
#[test]
fn breakpointless_pause_reports_step() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "resume" => Reply::Prefixed {
            prelude: vec![event("Debug", pause_breakpoint_event("isolates/1", &[]))],
            result: json!({ "type": "Success" }),
        },
        "getStack" => ok(stack_with_vars(json!([]))),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let snapshot = session
        .step_and_package(None, Some(dartq_debug::StepOption::Over))
        .expect("step");
    assert_eq!(snapshot.reason, StoppedReason::Step);
}

/// Condition truthiness follows the Dart DAP adapter: only `true` and
/// non-zero numbers stop; a non-empty String or an object reference
/// is falsy (the old rule stopped on any non-null object).
#[test]
fn condition_truthiness_matches_the_dap_adapter() {
    for (kind, value, stops) in [
        ("Bool", "true", true),
        ("Bool", "false", false),
        ("Int", "0", false),
        ("Int", "7", true),
        ("Double", "0.0", false),
        ("Double", "0.5", true),
        ("String", "yes", false),
        ("PlainInstance", "", false),
        ("Null", "null", false),
    ] {
        let vm = MockVm::start(move |request| match request.method.as_str() {
            "addBreakpointWithScriptUri" => ok(breakpoint("breakpoints/1")),
            "evaluateInFrame" => Reply::Ok(instance_ref("objects/1", kind, value)),
            "resume" => Reply::Prefixed {
                prelude: vec![event(
                    "Debug",
                    json!({
                        "type": "Event",
                        "kind": "PauseInterrupted",
                        "isolate": isolate_ref("isolates/1", "1", "main"),
                        "timestamp": 3_i64,
                    }),
                )],
                result: json!({ "type": "Success" }),
            },
            "getStack" => ok(stack_with_vars(json!([]))),
            _ => default_reply(request),
        });
        let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
        session
            .add_breakpoint(line_spec("isolates/1", Some("cond")))
            .expect("conditional bp");
        vm.control()
            .send(event(
                "Debug",
                pause_breakpoint_event("isolates/1", &["breakpoints/1"]),
            ))
            .expect("push hit");
        let snapshot = session.wait_for_pause("isolates/1").expect("snapshot");
        let stopped_at_breakpoint = matches!(snapshot.reason, StoppedReason::Breakpoint { .. });
        assert_eq!(
            stopped_at_breakpoint, stops,
            "{kind}({value}) -> {:?}",
            snapshot.reason
        );
    }
}

/// Events dropped by the bounded client feed are counted at the exact
/// position they were lost: the ring keeps the wire sequence, a page
/// ending before the loss reports no gap, and pages compose.
#[test]
fn event_gaps_sit_at_the_wire_position_and_pages_compose() {
    let overflow = dartq_debug::EVENT_BUFFER_CAPACITY + 5;
    let prelude: Vec<Control> = (0..overflow)
        .map(|n| event("Debug", resume_event(n as i64)))
        .collect();
    let vm = MockVm::start(move |request| match request.method.as_str() {
        "getStack" => Reply::Prefixed {
            prelude: prelude.clone(),
            result: stack_with_vars(json!([])),
        },
        _ => default_reply(request),
    });
    let settings = SessionConfig {
        event_capacity: 2 * dartq_debug::EVENT_BUFFER_CAPACITY,
        ..fast_config()
    };
    let mut session = DebugSession::attach_with(&vm.ws_uri, settings).expect("attach");
    session.stack(None, 1).expect("barrier");
    session.poll_events().expect("poll");

    // The first page stops well before the dropped tail: no gap yet.
    let first = session.events_since(0, 100);
    assert_eq!(first.events.len(), 100);
    assert_eq!(first.dropped, 0);
    assert_eq!(first.next_cursor, 100);
    assert_eq!(first.events[99].cursor, 99);

    // Paging to the end accounts for every event exactly once.
    let mut cursor = first.next_cursor;
    let mut seen = first.events.len() as u64;
    let mut dropped = first.dropped;
    loop {
        let page = session.events_since(cursor, 1000);
        if page.next_cursor == cursor {
            break;
        }
        seen += page.events.len() as u64;
        dropped += page.dropped;
        cursor = page.next_cursor;
    }
    assert_eq!(dropped, 5, "exactly the feed overrun is missing");
    assert_eq!(seen + dropped, overflow as u64);
    assert_eq!(cursor, overflow as u64);

    // A later event lands at its wire sequence, after the gap.
    vm.control()
        .send(event("Debug", resume_event(99_999)))
        .expect("push one more");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        session.poll_events().expect("poll");
        let tail = session.events_since(cursor, usize::MAX);
        if let Some(last) = tail.events.last() {
            assert_eq!(last.cursor, overflow as u64);
            assert_eq!(tail.dropped, 0);
            break;
        }
        assert!(std::time::Instant::now() < deadline, "late event never arrived");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Output pages honor the byte budget and resume from `next_cursor`.
#[test]
fn output_pages_respect_the_byte_budget() {
    use base64::Engine as _;
    let chunks: Vec<Control> = (0..10)
        .map(|n| {
            event(
                "Stdout",
                json!({
                    "type": "Event",
                    "kind": "WriteEvent",
                    "timestamp": n,
                    "bytes": base64::engine::general_purpose::STANDARD
                        .encode(format!("line{n}\n")),
                }),
            )
        })
        .collect();
    let vm = MockVm::start(move |request| match request.method.as_str() {
        "getStack" => Reply::Prefixed {
            prelude: chunks.clone(),
            result: stack_with_vars(json!([])),
        },
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    session.stack(None, 1).expect("barrier");
    session.poll_events().expect("poll");
    let first = session.stdout_since(0, 12);
    assert_eq!(first.bytes, b"line0\nline1\n");
    assert_eq!(first.next_cursor, 2);
    let rest = session.stdout_since(first.next_cursor, usize::MAX);
    assert!(rest.bytes.starts_with(b"line2\n") && rest.bytes.ends_with(b"line9\n"));
    assert_eq!(rest.dropped, 0);
}

/// `expand_graph` visits each object once — a cycle (`a.self == a`)
/// costs one `getObject`, not `max_children^depth` — and the node
/// budget caps the result.
#[test]
fn expand_graph_breaks_cycles_and_honors_the_node_budget() {
    let vm = MockVm::start(|request| {
        if request.method != "getObject" {
            return default_reply(request);
        }
        let fields: Vec<Value> = (0..5)
            .map(|n| {
                json!({
                    "name": format!("f{n}"),
                    // Every field points back at the root: a cycle.
                    "value": {
                        "type": "@Instance",
                        "id": "objects/root",
                        "kind": "PlainInstance",
                        "class": { "type": "@Class", "id": "classes/1", "name": "Node" },
                    },
                })
            })
            .collect();
        ok(json!({
            "type": "Instance",
            "id": "objects/root",
            "kind": "PlainInstance",
            "fields": fields,
        }))
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let graph = session.expand_graph(None, "objects/root").expect("graph");
    assert_eq!(graph.len(), 5, "cyclic children are leaves: {graph:?}");
    let fetches = vm
        .requests
        .try_iter()
        .filter(|request| request.method == "getObject")
        .count();
    assert_eq!(fetches, 1, "the root is fetched once");

    let tight = SessionConfig {
        expand_limits: dartq_debug::ExpandLimits {
            max_children: 100,
            max_depth: 3,
            max_nodes: 3,
        },
        ..fast_config()
    };
    let vm = MockVm::start(|request| {
        if request.method != "getObject" {
            return default_reply(request);
        }
        let fields: Vec<Value> = (0..10)
            .map(|n| json!({ "name": format!("f{n}"), "value": instance_ref("objects/x", "Int", "1") }))
            .collect();
        ok(json!({ "type": "Instance", "id": "objects/r", "kind": "PlainInstance", "fields": fields }))
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, tight).expect("attach");
    let graph = session.expand_graph(None, "objects/r").expect("graph");
    assert_eq!(graph.len(), 4);
    assert_eq!(graph[3].name, "<truncated>");
}

/// List pages ask the VM for a bounded slice and expose the next
/// offset; element names are absolute indices.
#[test]
fn expand_page_requests_a_bounded_slice() {
    let vm = MockVm::start(|request| {
        if request.method != "getObject" {
            return default_reply(request);
        }
        let offset = request.params["offset"].as_u64().unwrap_or(0);
        let count = request.params["count"].as_u64().unwrap_or(0);
        let elements: Vec<Value> = (offset..offset + count)
            .map(|n| instance_ref(&format!("objects/{n}"), "Int", &n.to_string()))
            .collect();
        ok(json!({
            "type": "Instance",
            "id": "objects/list",
            "kind": "List",
            "length": 1_000_000,
            "offset": offset,
            "count": count,
            "elements": elements,
        }))
    });
    let settings = SessionConfig {
        expand_limits: dartq_debug::ExpandLimits {
            max_children: 3,
            ..dartq_debug::ExpandLimits::default()
        },
        ..fast_config()
    };
    let mut session = DebugSession::attach_with(&vm.ws_uri, settings).expect("attach");
    let page = session
        .expand_page(None, "objects/list", 10)
        .expect("page");
    let names: Vec<&str> = page.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, ["[10]", "[11]", "[12]", "[...]"]);
    assert_eq!(page[3].value, "999987 more (next offset 13)");
    let request = vm.expect_request("getObject");
    assert_eq!(request.params["offset"], 10);
    assert_eq!(request.params["count"], 3);
}

/// A timed-out RPC unregisters its waiter (no pending-table growth
/// against a silent peer) and is reported as `Timeout`; a late answer
/// is discarded and the connection keeps working.
#[test]
fn rpc_timeout_unregisters_and_late_answers_are_discarded() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "hang" => Reply::Silent,
        _ => default_reply(request),
    });
    let client = VmClient::connect(&vm.ws_uri).expect("connect");
    for _ in 0..50 {
        let error = client
            .call("hang", json!({}), Duration::from_millis(5))
            .expect_err("silent peer");
        assert!(matches!(error, Error::Timeout(_)), "got {error:?}");
    }
    let late = vm.expect_request("hang");
    vm.control()
        .send(Control::Raw(
            json!({ "jsonrpc": "2.0", "id": late.id, "result": { "type": "Success" } })
                .to_string(),
        ))
        .expect("late answer");
    assert_eq!(client.get_version().expect("still alive").major, 4);
}

/// A frame the VM forwards as a request for a registered service
/// carries an id from another client's space; it must not answer one
/// of our pending calls. Malformed frames are counted, not silent.
#[test]
fn forwarded_requests_never_answer_pending_calls_and_malformed_frames_count() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "getVersion" => Reply::Prefixed {
            prelude: vec![
                Control::Raw(
                    json!({ "jsonrpc": "2.0", "id": request.id, "method": "ext.foreign", "params": {} })
                        .to_string(),
                ),
                Control::Raw("{broken".to_owned()),
                event("Debug", json!({ "type": "Event", "kind": "Resume" })),
            ],
            result: json!({ "type": "Version", "major": 4, "minor": 23 }),
        },
        _ => default_reply(request),
    });
    let client = VmClient::connect(&vm.ws_uri).expect("connect");
    let version = client.get_version().expect("the real response wins");
    assert_eq!(version.major, 4);
    // The unparseable frame and the timestamp-less event.
    assert_eq!(client.malformed_frames(), 2);
}

/// A session whose conditional breakpoint keeps evaluating falsy on
/// one isolate still honors the auto-resume ceiling per isolate after
/// the counter type change (regression guard for `classify_pause`
/// taking the paused isolate explicitly).
#[test]
fn auto_resume_ceiling_still_applies() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "addBreakpointWithScriptUri" => ok(breakpoint("breakpoints/1")),
        "evaluateInFrame" => Reply::Ok(instance_ref("objects/1", "Bool", "false")),
        "resume" => Reply::Prefixed {
            prelude: vec![event(
                "Debug",
                pause_breakpoint_event("isolates/1", &["breakpoints/1"]),
            )],
            result: json!({ "type": "Success" }),
        },
        "getStack" => ok(stack_with_vars(json!([]))),
        _ => default_reply(request),
    });
    let settings = SessionConfig {
        conditional_resume_limit: ConditionalResumeLimit { max_auto_resumes: 2 },
        ..fast_config()
    };
    let mut session = DebugSession::attach_with(&vm.ws_uri, settings).expect("attach");
    session
        .add_breakpoint(line_spec("isolates/1", Some("never")))
        .expect("bp");
    vm.control()
        .send(event(
            "Debug",
            pause_breakpoint_event("isolates/1", &["breakpoints/1"]),
        ))
        .expect("first hit");
    let snapshot = session.wait_for_pause("isolates/1").expect("ceiling stop");
    assert_eq!(
        snapshot.reason,
        StoppedReason::ConditionalResumeLimit { auto_resumes: 3 }
    );
}
