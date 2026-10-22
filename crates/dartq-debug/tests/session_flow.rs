//! Session tests for stepping/pause packaging, conditional
//! breakpoints, evaluation, object-graph expansion, reloads, and
//! bounded event buffering.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use common::{
    Control, MockVm, Reply, default_reply, err, instance_ref, pause_breakpoint_event,
    stack_with_vars,
};
use dartq_debug::{
    BreakpointLocationSpec, BreakpointSpec, DebugSession, Error, EventKind, ExpandLimits,
    InstanceKind, SessionConfig, StepOption, StoppedReason, VmObject,
};
use serde_json::{Value, json};

fn fast_config() -> SessionConfig {
    SessionConfig {
        pause_timeout: Duration::from_secs(5),
        rpc_timeout: Duration::from_secs(5),
        ..SessionConfig::default()
    }
}

fn line_spec(condition: Option<&str>) -> BreakpointSpec {
    BreakpointSpec {
        isolate: None,
        location: BreakpointLocationSpec::Line {
            script_uri: "package:app/main.dart".to_owned(),
            line: 7,
            column: None,
        },
        condition: condition.map(str::to_owned),
    }
}

fn resolved_breakpoint(id: &str) -> Value {
    json!({
        "type": "Breakpoint",
        "id": id,
        "breakpointNumber": 1,
        "enabled": true,
        "resolved": true,
    })
}

fn pause_event(stream_id: &str, event: Value) -> Control {
    Control::Event {
        stream_id: stream_id.to_owned(),
        event,
    }
}

fn interrupt_event() -> Value {
    json!({
        "type": "Event",
        "kind": "PauseInterrupted",
        "isolate": { "type": "@Isolate", "id": "isolates/1", "number": "1", "name": "main" },
        "timestamp": 1_i64,
    })
}


#[test]
fn step_over_packages_stopped_snapshot() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "addBreakpointWithScriptUri" => {
            Reply::Ok(resolved_breakpoint("breakpoints/1"))
        }
        "resume" => Reply::Prefixed {
            prelude: vec![pause_event(
                "Debug",
                pause_breakpoint_event("isolates/1", &["breakpoints/1"]),
            )],
            result: json!({ "type": "Success" }),
        },
        "getStack" => Reply::Ok(stack_with_vars(json!([
            {
                "name": "count",
                "value": instance_ref("objects/7", "Int", "3"),
                "staticType": instance_ref("objects/8", "Int", "3"),
            },
            {
                "name": "gone",
                "value": { "type": "Sentinel", "kind": "Collected", "valueAsString": "<collected>" },
            },
        ]))),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    session.add_breakpoint(line_spec(None)).expect("add breakpoint");
    let snapshot = session
        .step_and_package(None, Some(StepOption::Over))
        .expect("step over");

    assert_eq!(
        snapshot.reason,
        StoppedReason::Breakpoint {
            breakpoints: vec!["breakpoints/1".to_owned()],
        }
    );
    assert_eq!(snapshot.isolate, "isolates/1");
    assert_eq!(snapshot.location.line, Some(7));
    assert_eq!(snapshot.location.column, Some(3));
    assert_eq!(
        snapshot.location.script_uri.as_deref(),
        Some("package:app/main.dart")
    );
    assert_eq!(snapshot.location.script_id.as_deref(), Some("scripts/1"));
    assert_eq!(snapshot.location.function.as_deref(), Some("main"));
    assert_eq!(snapshot.stack.len(), 1);
    assert_eq!(snapshot.stack[0].function.as_deref(), Some("main"));
    assert!(!snapshot.at_async_suspension);

    let count = snapshot
        .variables
        .iter()
        .find(|variable| variable.name == "count")
        .expect("count variable");
    assert_eq!(count.value, "3");
    assert_eq!(count.handle.as_deref(), Some("objects/7"));
    let gone = snapshot
        .variables
        .iter()
        .find(|variable| variable.name == "gone")
        .expect("gone variable");
    assert_eq!(gone.value, "<collected>");
    assert_eq!(gone.handle, None);

    let resume = vm.expect_request("resume");
    assert_eq!(resume.params["step"], "Over");
}

#[test]
fn every_step_option_is_sent_on_resume() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "resume" => Reply::Prefixed {
            prelude: vec![pause_event(
                "Debug",
                pause_breakpoint_event("isolates/1", &[]),
            )],
            result: success_result(),
        },
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let steps = [
        StepOption::Into,
        StepOption::Over,
        StepOption::OverAsyncSuspension,
        StepOption::Out,
        StepOption::Rewind,
    ];
    for step in steps {
        session
            .step_and_package(None, Some(step))
            .unwrap_or_else(|error| panic!("step {step:?} failed: {error:?}"));
    }
    let resumes = vm.expect_requests("resume", steps.len());
    for (request, step) in resumes.iter().zip(steps) {
        assert_eq!(request.params["step"], step.as_str());
    }
}

fn success_result() -> Value {
    json!({ "type": "Success" })
}

#[test]
fn resume_without_waiting_sends_rpc() {
    let vm = MockVm::start_default();
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    session.resume(None).expect("resume");
    let request = vm.expect_request("resume");
    assert_eq!(request.params["isolateId"], "isolates/1");
    assert!(request.params.get("step").is_none());
}

#[test]
fn wait_for_pause_reports_pause_start() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "getStack" => Reply::Prefixed {
            prelude: vec![pause_event(
                "Debug",
                json!({
                    "type": "Event",
                    "kind": "PauseStart",
                    "isolate": { "type": "@Isolate", "id": "isolates/1", "number": "1", "name": "main" },
                    "timestamp": 1_i64,
                }),
            )],
            result: stack_with_vars(json!([])),
        },
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    session.stack(None, 1).expect("barrier rpc");
    session.poll_events().expect("poll events");
    let snapshot = session
        .wait_for_pause("isolates/1")
        .expect("pause start snapshot");
    assert_eq!(snapshot.reason, StoppedReason::Start);
    assert_eq!(snapshot.location.line, Some(7));
}

#[test]
fn wait_for_pause_times_out() {
    let vm = MockVm::start_default();
    let settings = SessionConfig {
        pause_timeout: Duration::from_millis(200),
        rpc_timeout: Duration::from_secs(5),
        ..SessionConfig::default()
    };
    let mut session = DebugSession::attach_with(&vm.ws_uri, settings).expect("attach");
    let error = session
        .wait_for_pause("isolates/1")
        .expect_err("no pause ever arrives");
    assert!(matches!(error, Error::Timeout(_)), "got {error:?}");
}

#[test]
fn pause_packages_interrupted_snapshot() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "pause" => Reply::Prefixed {
            prelude: vec![pause_event("Debug", interrupt_event())],
            result: success_result(),
        },
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    session.pause(None).expect("pause rpc");
    let snapshot = session.wait_for_pause("isolates/1").expect("snapshot");
    assert_eq!(snapshot.reason, StoppedReason::Interrupted);
}

#[test]
fn exception_pause_snapshot_carries_locals() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "getStack" => Reply::Prefixed {
            prelude: vec![pause_event(
                "Debug",
                json!({
                    "type": "Event",
                    "kind": "PauseException",
                    "isolate": { "type": "@Isolate", "id": "isolates/1", "number": "1", "name": "main" },
                    "timestamp": 1_i64,
                    "exception": instance_ref("objects/9", "String", "boom"),
                }),
            )],
            result: stack_with_vars(json!([
                { "name": "message", "value": instance_ref("objects/10", "String", "boom") }
            ])),
        },
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    session.stack(None, 1).expect("barrier rpc");
    session.poll_events().expect("poll events");
    let snapshot = session.wait_for_pause("isolates/1").expect("snapshot");
    assert_eq!(snapshot.reason, StoppedReason::Exception);
    assert_eq!(snapshot.variables.len(), 1);
    assert_eq!(snapshot.variables[0].name, "message");
    assert_eq!(snapshot.variables[0].value, "boom");
}

#[test]
fn conditional_breakpoint_false_then_true_stops() {
    let evaluations = Arc::new(AtomicUsize::new(0));
    let resumes = Arc::new(AtomicUsize::new(0));
    let eval_counter = Arc::clone(&evaluations);
    let resume_counter = Arc::clone(&resumes);
    let vm = MockVm::start(move |request| match request.method.as_str() {
        "addBreakpointWithScriptUri" => Reply::Ok(resolved_breakpoint("breakpoints/1")),
        "evaluateInFrame" => {
            let call = eval_counter.fetch_add(1, Ordering::SeqCst);
            let value = if call == 0 { "false" } else { "true" };
            Reply::Ok(instance_ref("objects/11", "Bool", value))
        }
        "resume" => {
            resume_counter.fetch_add(1, Ordering::SeqCst);
            Reply::Prefixed {
                prelude: vec![pause_event(
                    "Debug",
                    pause_breakpoint_event("isolates/1", &["breakpoints/1"]),
                )],
                result: success_result(),
            }
        }
        "getStack" => Reply::Ok(stack_with_vars(json!([]))),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    session
        .add_breakpoint(line_spec(Some("count > 3")))
        .expect("add conditional breakpoint");
    vm.control()
        .send(pause_event(
            "Debug",
            pause_breakpoint_event("isolates/1", &["breakpoints/1"]),
        ))
        .expect("push first pause");

    let snapshot = session.wait_for_pause("isolates/1").expect("snapshot");
    assert_eq!(
        snapshot.reason,
        StoppedReason::Breakpoint {
            breakpoints: vec!["breakpoints/1".to_owned()],
        }
    );
    assert_eq!(evaluations.load(Ordering::SeqCst), 2);
    assert_eq!(resumes.load(Ordering::SeqCst), 1);
    let resume = vm.expect_request("resume");
    assert_eq!(resume.params["isolateId"], "isolates/1");
}

/// A condition that cannot be evaluated must not silently disarm its
/// breakpoint: the stop is reported with the evaluation error and no
/// auto-resume is issued.
#[test]
fn conditional_breakpoint_eval_error_reports_condition_error() {
    let evaluations = Arc::new(AtomicUsize::new(0));
    let resumes = Arc::new(AtomicUsize::new(0));
    let eval_counter = Arc::clone(&evaluations);
    let resume_counter = Arc::clone(&resumes);
    let vm = MockVm::start(move |request| match request.method.as_str() {
        "addBreakpointWithScriptUri" => Reply::Ok(resolved_breakpoint("breakpoints/1")),
        "evaluateInFrame" => {
            eval_counter.fetch_add(1, Ordering::SeqCst);
            err(113, "Expression compilation error")
        }
        "resume" => {
            let call = resume_counter.fetch_add(1, Ordering::SeqCst);
            let event = if call == 0 {
                pause_breakpoint_event("isolates/1", &["breakpoints/1"])
            } else {
                interrupt_event()
            };
            Reply::Prefixed {
                prelude: vec![pause_event("Debug", event)],
                result: success_result(),
            }
        }
        "getStack" => Reply::Ok(stack_with_vars(json!([]))),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    session
        .add_breakpoint(line_spec(Some("undefinedName > 0")))
        .expect("add conditional breakpoint");
    vm.control()
        .send(pause_event(
            "Debug",
            pause_breakpoint_event("isolates/1", &["breakpoints/1"]),
        ))
        .expect("push first pause");

    let snapshot = session.wait_for_pause("isolates/1").expect("snapshot");
    match snapshot.reason {
        StoppedReason::ConditionError {
            breakpoints,
            message,
        } => {
            assert_eq!(breakpoints, ["breakpoints/1"]);
            assert!(message.contains("113"), "message: {message}");
        }
        other => panic!("expected ConditionError, got {other:?}"),
    }
    assert_eq!(evaluations.load(Ordering::SeqCst), 1);
    assert_eq!(resumes.load(Ordering::SeqCst), 0);
}

#[test]
fn conditional_resume_limit_reports_stop() {
    let evaluations = Arc::new(AtomicUsize::new(0));
    let resumes = Arc::new(AtomicUsize::new(0));
    let eval_counter = Arc::clone(&evaluations);
    let resume_counter = Arc::clone(&resumes);
    let vm = MockVm::start(move |request| match request.method.as_str() {
        "addBreakpointWithScriptUri" => Reply::Ok(resolved_breakpoint("breakpoints/1")),
        "evaluateInFrame" => {
            eval_counter.fetch_add(1, Ordering::SeqCst);
            Reply::Ok(instance_ref("objects/12", "Bool", "false"))
        }
        "resume" => {
            resume_counter.fetch_add(1, Ordering::SeqCst);
            Reply::Prefixed {
                prelude: vec![pause_event(
                    "Debug",
                    pause_breakpoint_event("isolates/1", &["breakpoints/1"]),
                )],
                result: success_result(),
            }
        }
        "getStack" => Reply::Ok(stack_with_vars(json!([]))),
        _ => default_reply(request),
    });
    let settings = SessionConfig {
        pause_timeout: Duration::from_secs(5),
        rpc_timeout: Duration::from_secs(5),
        conditional_resume_limit: dartq_debug::ConditionalResumeLimit { max_auto_resumes: 3 },
        ..SessionConfig::default()
    };
    let mut session = DebugSession::attach_with(&vm.ws_uri, settings).expect("attach");
    session
        .add_breakpoint(line_spec(Some("count > 1000")))
        .expect("add conditional breakpoint");
    vm.control()
        .send(pause_event(
            "Debug",
            pause_breakpoint_event("isolates/1", &["breakpoints/1"]),
        ))
        .expect("push first pause");

    let snapshot = session.wait_for_pause("isolates/1").expect("snapshot");
    assert_eq!(
        snapshot.reason,
        StoppedReason::ConditionalResumeLimit { auto_resumes: 4 }
    );
    assert_eq!(evaluations.load(Ordering::SeqCst), 4);
    assert_eq!(resumes.load(Ordering::SeqCst), 3);
}

#[test]
fn evaluate_in_frame_returns_instance() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "evaluateInFrame" => Reply::Ok(instance_ref("objects/5", "Int", "2")),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let value = session
        .evaluate_in_frame(None, 0, "1 + 1")
        .expect("evaluateInFrame");
    match value {
        VmObject::Instance(instance) => {
            assert_eq!(instance.kind, InstanceKind::Int);
            assert_eq!(instance.value_as_string.as_deref(), Some("2"));
        }
        other => panic!("expected instance, got {other:?}"),
    }
    let request = vm.expect_request("evaluateInFrame");
    assert_eq!(request.params["frameIndex"], 0);
    assert_eq!(request.params["expression"], "1 + 1");
    assert_eq!(request.params["isolateId"], "isolates/1");
}

#[test]
fn evaluate_in_frame_reports_compile_error() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "evaluateInFrame" => err(113, "Expression compilation error"),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let error = session
        .evaluate_in_frame(None, 0, "not dart(")
        .expect_err("compile error must surface");
    assert!(matches!(error, Error::Rpc { code: 113, .. }), "got {error:?}");
}

#[test]
fn evaluate_targets_library() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "evaluate" => Reply::Ok(instance_ref("objects/6", "String", "hello")),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let value = session
        .evaluate(None, "libraries/1", "'hello'")
        .expect("evaluate");
    assert!(matches!(value, VmObject::Instance(_)));
    let request = vm.expect_request("evaluate");
    assert_eq!(request.params["targetId"], "libraries/1");
    assert_eq!(request.params["expression"], "'hello'");
}

#[test]
fn evaluate_in_root_lib_targets_the_root_library() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "getIsolate" => Reply::Ok(json!({
            "type": "Isolate",
            "id": "isolates/1",
            "number": "1",
            "name": "main",
            "runnable": true,
            "rootLib": { "type": "@Library", "id": "libraries/9", "uri": "package:app/main.dart" },
        })),
        "evaluate" => Reply::Ok(instance_ref("objects/1", "Int", "5")),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let value = session
        .evaluate_in_root_lib(None, "add(2, 3)")
        .expect("evaluate_in_root_lib");
    assert!(matches!(value, VmObject::Instance(_)));
    // A second call reuses the cached root-lib id — no second
    // `getIsolate` round-trip.
    session
        .evaluate_in_root_lib(None, "add(4, 5)")
        .expect("cached evaluate_in_root_lib");
    let mut get_isolate_calls = 0;
    let mut evaluate_params = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while evaluate_params.len() < 2 && std::time::Instant::now() < deadline {
        match vm
            .requests
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
        {
            Ok(request) if request.method == "getIsolate" => get_isolate_calls += 1,
            Ok(request) if request.method == "evaluate" => evaluate_params.push(request.params),
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert_eq!(get_isolate_calls, 1, "the root lib must be cached");
    assert_eq!(evaluate_params.len(), 2);
    for params in &evaluate_params {
        assert_eq!(params["isolateId"], "isolates/1");
        assert_eq!(params["targetId"], "libraries/9");
    }
    assert_eq!(evaluate_params[0]["expression"], "add(2, 3)");
    assert_eq!(evaluate_params[1]["expression"], "add(4, 5)");
}

#[test]
fn evaluate_in_root_lib_without_root_lib_is_protocol_error() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "getIsolate" => Reply::Ok(json!({
            "type": "Isolate",
            "id": "isolates/1",
            "number": "1",
            "name": "main",
            "runnable": false,
        })),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let error = session
        .evaluate_in_root_lib(None, "1 + 1")
        .expect_err("no root library must fail");
    match error {
        Error::Protocol(message) => {
            assert!(message.contains("no root library"), "got {message}");
            assert!(message.contains("isolates/1"), "got {message}");
        }
        other => panic!("expected Protocol error, got {other:?}"),
    }
    // The failure happens before the evaluate RPC is issued.
    let evaluate_sent = vm
        .requests
        .try_iter()
        .any(|request| request.method == "evaluate");
    assert!(!evaluate_sent, "evaluate must not be issued without a root library");
}

#[test]
fn invoke_sends_selector_and_arguments() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "invoke" => Reply::Ok(instance_ref("objects/13", "String", "42")),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let value = session
        .invoke(None, "objects/5", "toString", &[])
        .expect("invoke");
    assert!(matches!(value, VmObject::Instance(_)));
    let request = vm.expect_request("invoke");
    assert_eq!(request.params["targetId"], "objects/5");
    assert_eq!(request.params["selector"], "toString");
    assert_eq!(request.params["argumentIds"], json!([]));
}

#[test]
fn get_object_reports_sentinel() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "getObject" => Reply::Ok(json!({
            "type": "Sentinel",
            "kind": "Expired",
            "valueAsString": "<expired>",
        })),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let error = session
        .get_object(None, "objects/404")
        .expect_err("expired handle must fail");
    match error {
        Error::Sentinel { kind, .. } => assert_eq!(kind, "Expired"),
        other => panic!("expected Sentinel, got {other:?}"),
    }
}

#[test]
fn expand_truncates_children_to_width_limit() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "getObject" => {
            let fields: Vec<Value> = (0..150)
                .map(|index| {
                    json!({
                        "decl": { "type": "@Field", "id": format!("fields/{index}") },
                        "name": format!("f{index}"),
                        "value": instance_ref(&format!("objects/{index}"), "Int", &index.to_string()),
                    })
                })
                .collect();
            Reply::Ok(json!({
                "type": "Instance",
                "id": "objects/root",
                "kind": "PlainInstance",
                "fields": fields,
            }))
        }
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let children = session.expand(None, "objects/root").expect("expand");
    // 100 fields plus the trailing "more" row.
    assert_eq!(children.len(), 101);
    assert_eq!(children[100].name, "[...]");
    assert_eq!(children[100].value, "50 more");
    let request = vm.expect_request("getObject");
    assert_eq!(request.params["count"], 100);
    assert_eq!(request.params["offset"], 0);
    assert_eq!(children[0].name, "f0");
    assert_eq!(children[0].value, "0");
    assert_eq!(children[0].handle.as_deref(), Some("objects/0"));
    assert_eq!(children[99].name, "f99");
}

#[test]
fn expand_rejects_non_instance_object() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "getObject" => Reply::Ok(json!({ "type": "Success" })),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let error = session
        .expand(None, "objects/root")
        .expect_err("non-instance is not expandable");
    assert!(
        matches!(error, Error::Protocol(ref message) if message.contains("not an expandable instance")),
        "got {error:?}"
    );
}

#[test]
fn expand_graph_respects_depth_limit() {
    let vm = MockVm::start(|request| {
        if request.method != "getObject" {
            return default_reply(request);
        }
        let id = request.params["objectId"].as_str().unwrap_or_default();
        let (child_id, child_name) = match id {
            "objects/root" => ("objects/l1", "a"),
            "objects/l1" => ("objects/l2", "b"),
            "objects/l2" => ("objects/l3", "c"),
            "objects/l3" => ("objects/l4", "d"),
            _ => {
                return Reply::Ok(json!({
                    "type": "Instance",
                    "id": id,
                    "kind": "PlainInstance",
                    "fields": [],
                }));
            }
        };
        Reply::Ok(json!({
            "type": "Instance",
            "id": id,
            "kind": "PlainInstance",
            "fields": [{
                "decl": { "type": "@Field", "id": "fields/1" },
                "name": child_name,
                "value": instance_ref(child_id, "Int", "1"),
            }],
        }))
    });
    let settings = SessionConfig {
        pause_timeout: Duration::from_secs(5),
        rpc_timeout: Duration::from_secs(5),
        expand_limits: ExpandLimits {
            max_children: 100,
            max_depth: 3,
            max_nodes: 500,
        },
        ..SessionConfig::default()
    };
    let mut session = DebugSession::attach_with(&vm.ws_uri, settings).expect("attach");
    let graph = session.expand_graph(None, "objects/root").expect("expand graph");
    let names: Vec<&str> = graph.iter().map(|variable| variable.name.as_str()).collect();
    assert_eq!(names, ["a", "a.b", "a.b.c"]);
}

#[test]
fn reload_sources_returns_success() {
    let vm = MockVm::start_default();
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    assert!(session.reload_sources(None, true).expect("reload"));
    let request = vm.expect_request("reloadSources");
    assert_eq!(request.params["force"], true);
}

#[test]
fn reload_sources_conflict_is_error() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "reloadSources" => err(108, "Isolate must have a root library"),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let error = session
        .reload_sources(None, false)
        .expect_err("reload conflict must surface");
    assert!(matches!(error, Error::Rpc { code: 108, .. }), "got {error:?}");
}

#[test]
fn hot_restart_surfaces_registration_error() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "hotRestart" => err(-32601, "Method not found"),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let error = session
        .hot_restart()
        .expect_err("a bare VM has no hotRestart service");
    assert!(matches!(error, Error::Rpc { code: -32601, .. }), "got {error:?}");
}

#[test]
fn events_since_marks_gap_and_keeps_cursor_monotonic() {
    let call = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&call);
    let vm = MockVm::start(move |request| match request.method.as_str() {
        "getStack" => {
            let index = counter.fetch_add(1, Ordering::SeqCst);
            let prelude = match index {
                0 => (0..5)
                    .map(|n| {
                        pause_event(
                            "Debug",
                            json!({ "type": "Event", "kind": "Resume", "timestamp": n }),
                        )
                    })
                    .collect(),
                1 => (0..2)
                    .map(|n| {
                        pause_event(
                            "Debug",
                            json!({ "type": "Event", "kind": "Resume", "timestamp": n }),
                        )
                    })
                    .collect(),
                _ => Vec::new(),
            };
            Reply::Prefixed {
                prelude,
                result: stack_with_vars(json!([])),
            }
        }
        _ => default_reply(request),
    });
    let settings = SessionConfig {
        event_capacity: 3,
        pause_timeout: Duration::from_secs(5),
        rpc_timeout: Duration::from_secs(5),
        ..SessionConfig::default()
    };
    let mut session = DebugSession::attach_with(&vm.ws_uri, settings).expect("attach");

    session.stack(None, 1).expect("first barrier");
    session.poll_events().expect("poll events");
    let first = session.events_since(0, usize::MAX);
    assert_eq!(first.dropped, 2);
    assert_eq!(first.events.len(), 3);
    assert_eq!(first.next_cursor, 5);
    let cursors: Vec<u64> = first.events.iter().map(|event| event.cursor).collect();
    assert_eq!(cursors, [2, 3, 4]);
    assert_eq!(session.events_since(5, usize::MAX).events.len(), 0);

    session.stack(None, 1).expect("second barrier");
    session.poll_events().expect("poll events");
    let second = session.events_since(5, usize::MAX);
    assert_eq!(second.dropped, 0);
    assert_eq!(second.events.len(), 2);
    assert_eq!(second.next_cursor, 7);
    let cursors: Vec<u64> = second.events.iter().map(|event| event.cursor).collect();
    assert_eq!(cursors, [5, 6]);
    assert_eq!(session.events_since(0, usize::MAX).dropped, 4);
    assert_eq!(session.events_since(7, usize::MAX).next_cursor, 7);
}

#[test]
fn client_feed_overrun_surfaces_in_events_since_dropped() {
    // Events dropped by the client's bounded feed (never delivered to
    // the session) must still show up as a gap — an agent reading
    // `events_since` learns the true count.
    let overflow = dartq_debug::EVENT_BUFFER_CAPACITY + 3;
    let prelude: Vec<Control> = (0..overflow)
        .map(|n| Control::Event {
            stream_id: "Debug".to_owned(),
            event: json!({ "type": "Event", "kind": "Resume", "timestamp": n as i64 }),
        })
        .collect();
    let vm = MockVm::start(move |request| match request.method.as_str() {
        "getStack" => Reply::Prefixed {
            prelude: prelude.clone(),
            result: stack_with_vars(json!([])),
        },
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    session.stack(None, 1).expect("barrier rpc");
    session.poll_events().expect("poll events");
    let batch = session.events_since(0, usize::MAX);
    assert_eq!(batch.dropped, 3);
    assert_eq!(batch.events.len(), dartq_debug::EVENT_BUFFER_CAPACITY);
    // Subsequent reads keep reporting the accumulated gap.
    assert_eq!(session.events_since(0, usize::MAX).dropped, 3);
}

#[test]
fn attach_mode_write_events_feed_stdout_and_stderr() {
    // Attached sessions have no child pipes — process output arrives as
    // base64 `WriteEvent`s on the Stdout/Stderr streams and must land
    // in the same rings launch mode feeds from the pipes.
    let vm = MockVm::start(|request| match request.method.as_str() {
        "getStack" => Reply::Prefixed {
            prelude: vec![
                pause_event(
                    "Stdout",
                    json!({
                        "type": "Event",
                        "kind": "WriteEvent",
                        "timestamp": 1_i64,
                        // "hello\n"
                        "bytes": "aGVsbG8K",
                    }),
                ),
                pause_event(
                    "Stderr",
                    json!({
                        "type": "Event",
                        "kind": "WriteEvent",
                        "timestamp": 2_i64,
                        // "oops!\n"
                        "bytes": "b29wcyEK",
                    }),
                ),
                // Unknown stream ids are ignored (no ring grows).
                pause_event(
                    "Debug",
                    json!({
                        "type": "Event",
                        "kind": "WriteEvent",
                        "timestamp": 3_i64,
                        "bytes": "aGVsbG8K",
                    }),
                ),
                // Malformed base64 skips the event instead of failing.
                pause_event(
                    "Stdout",
                    json!({
                        "type": "Event",
                        "kind": "WriteEvent",
                        "timestamp": 4_i64,
                        "bytes": "!!!not-base64!!!",
                    }),
                ),
            ],
            result: stack_with_vars(json!([])),
        },
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    session.stack(None, 1).expect("barrier rpc");
    session.poll_events().expect("poll events");
    assert_eq!(session.stdout_since(0, usize::MAX).bytes, b"hello\n");
    assert_eq!(session.stderr_since(0, usize::MAX).bytes, b"oops!\n");
}

#[test]
fn output_batches_start_empty_for_attached_session() {
    let vm = MockVm::start_default();
    let session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let stdout = session.stdout_since(0, usize::MAX);
    assert!(stdout.bytes.is_empty());
    assert_eq!(stdout.next_cursor, 0);
    assert_eq!(stdout.dropped, 0);
    let stderr = session.stderr_since(0, usize::MAX);
    assert!(stderr.bytes.is_empty());
    assert_eq!(stderr.next_cursor, 0);
}

#[test]
fn event_payload_keeps_typed_shape_and_drops_raw_for_modeled_kinds() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "getStack" => Reply::Prefixed {
            prelude: vec![pause_event(
                "Debug",
                pause_breakpoint_event("isolates/1", &["breakpoints/9"]),
            )],
            result: stack_with_vars(json!([])),
        },
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    session.stack(None, 1).expect("barrier");
    session.poll_events().expect("poll events");
    let batch = session.events_since(0, usize::MAX);
    assert_eq!(batch.events.len(), 1);
    let event = &batch.events[0];
    assert_eq!(event.stream_id, "Debug");
    assert!(matches!(event.event.kind, EventKind::PauseBreakpoint));
    // `PauseBreakpoint` is fully modeled by the typed `Event`
    // (`pause_breakpoints`, `top_frame`, …), so the ring drops the
    // duplicated raw JSON — see `SessionEvent::raw`.
    assert!(event.raw.is_none());
    assert_eq!(event.event.pause_breakpoints[0].id, "breakpoints/9");
    assert_eq!(
        event.event.top_frame.as_ref().map(|frame| frame.index),
        Some(0)
    );
}
