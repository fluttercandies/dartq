//! Monorepo-scale boundary tests: many isolates paused at once,
//! per-isolate conditional-breakpoint accounting, a breakpoint
//! registry shared across isolates, event-feed saturation, and
//! forward-compat event kinds.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use common::{
    Control, MockVm, Reply, default_reply, instance_ref, isolate_ref, ok, pause_breakpoint_event,
    source_location, stack_with_vars,
};
use dartq_debug::{
    BreakpointLocationSpec, BreakpointSpec, ConditionalResumeLimit, DebugSession, EventKind,
    SessionConfig, StoppedReason,
};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(10);

fn fast_config() -> SessionConfig {
    SessionConfig {
        pause_timeout: Duration::from_secs(10),
        rpc_timeout: Duration::from_secs(5),
        ..SessionConfig::default()
    }
}

/// A `getVM` reply advertising `count` user isolates
/// (`isolates/1..=count`, no system isolates).
fn vm_with_isolates(count: usize) -> Value {
    let isolates: Vec<Value> = (1..=count)
        .map(|n| isolate_ref(&format!("isolates/{n}"), &n.to_string(), &format!("iso{n}")))
        .collect();
    json!({
        "type": "VM",
        "name": "mock-vm",
        "architectureBits": 64,
        "pid": 4242,
        "isolates": isolates,
        "systemIsolates": [],
    })
}

fn event(stream_id: &str, event: Value) -> Control {
    Control::Event {
        stream_id: stream_id.to_owned(),
        event,
    }
}

/// A resolved `Breakpoint` payload (for RPC results and
/// `BreakpointResolved`/`BreakpointRemoved` events).
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

/// A `PauseBreakpoint` event on `isolate` naming untracked breakpoint
/// ids — the session treats unknown ids as unconditional stops.
fn untracked_pause(isolate: &str) -> Value {
    pause_breakpoint_event(isolate, &[&format!("breakpoints/foreign-{isolate}")])
}

/// Eight isolates paused concurrently resolve independently: each
/// `wait_for_pause` consumes exactly its own isolate's pause record.
#[test]
fn eight_isolates_pause_and_resolve_independently() {
    const ISOLATES: usize = 8;
    let prelude: Vec<Control> = (1..=ISOLATES)
        .map(|n| event("Debug", untracked_pause(&format!("isolates/{n}"))))
        .collect();
    let vm = MockVm::start(move |request| match request.method.as_str() {
        "getVM" => ok(vm_with_isolates(ISOLATES)),
        "getStack" => Reply::Prefixed {
            prelude: prelude.clone(),
            result: stack_with_vars(json!([])),
        },
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    assert_eq!(session.isolates().count(), ISOLATES);
    assert_eq!(session.main_isolate(), Some("isolates/1"));

    // The barrier guarantees all eight pause events are queued in the
    // client's feed before the first wait begins.
    session.stack(None, 1).expect("barrier rpc");
    for n in 1..=ISOLATES {
        let isolate = format!("isolates/{n}");
        let snapshot = session
            .wait_for_pause(&isolate)
            .unwrap_or_else(|error| panic!("wait_for_pause({isolate}): {error:?}"));
        assert_eq!(snapshot.isolate, isolate);
        assert_eq!(
            snapshot.reason,
            StoppedReason::Breakpoint {
                breakpoints: vec![format!("breakpoints/foreign-isolates/{n}")],
            }
        );
    }
}

/// Conditional-breakpoint auto-resume accounting is per isolate: a
/// runaway condition on isolate A must not advance isolate B's
/// counter toward `ConditionalResumeLimit`. Regression test for the
/// shared `consecutive_auto_resumes` counter: with the old session-
/// wide counter, A's auto-resumes (left non-zero when A's wait timed
/// out without a reported stop) leaked into B's next classification
/// and produced a false `ConditionalResumeLimit` stop on B.
#[test]
fn conditional_auto_resumes_are_isolated_per_isolate() {
    let resumes = Arc::new(AtomicUsize::new(0));
    let resume_counter = Arc::clone(&resumes);
    let vm = MockVm::start(move |request| match request.method.as_str() {
        "getVM" => ok(vm_with_isolates(2)),
        "addBreakpointWithScriptUri" => {
            let isolate = request.params["isolateId"].as_str().unwrap_or_default();
            let id = if isolate == "isolates/1" {
                "breakpoints/a"
            } else {
                "breakpoints/b"
            };
            ok(resolved_breakpoint(id))
        }
        // Every condition evaluates falsy.
        "evaluateInFrame" => Reply::Ok(instance_ref("objects/1", "Bool", "false")),
        "resume" => {
            resume_counter.fetch_add(1, Ordering::SeqCst);
            ok(json!({ "type": "Success" }))
        }
        "getStack" => Reply::Ok(stack_with_vars(json!([]))),
        _ => default_reply(request),
    });
    let settings = SessionConfig {
        pause_timeout: Duration::from_millis(300),
        rpc_timeout: Duration::from_secs(5),
        conditional_resume_limit: ConditionalResumeLimit {
            max_auto_resumes: 1,
        },
        ..SessionConfig::default()
    };
    let mut session = DebugSession::attach_with(&vm.ws_uri, settings).expect("attach");
    for isolate in ["isolates/1", "isolates/2"] {
        session
            .add_breakpoint(BreakpointSpec {
                isolate: Some(isolate.to_owned()),
                location: BreakpointLocationSpec::Line {
                    script_uri: "package:app/main.dart".to_owned(),
                    line: 7,
                    column: None,
                },
                condition: Some("neverTrue".to_owned()),
            })
            .expect("add conditional breakpoint");
    }

    // Isolate A hits once: falsy condition -> auto-resume (counter 1,
    // under the limit of 1 -> no report), then A goes silent so the
    // wait times out with the counter left at 1.
    vm.control()
        .send(event(
            "Debug",
            pause_breakpoint_event("isolates/1", &["breakpoints/a"]),
        ))
        .expect("push pause for A");
    let error = session
        .wait_for_pause("isolates/1")
        .expect_err("A produces no further pause -> timeout");
    assert!(
        matches!(error, dartq_debug::Error::Timeout(_)),
        "got {error:?}"
    );

    // A's auto-resume RPC is already served and queued in the
    // mock's request log — drain it so the interrupter below cannot
    // mistake it for B's.
    let resume_a = vm.expect_request("resume");
    assert_eq!(resume_a.params["isolateId"], "isolates/1");

    // Isolate B now hits its own conditional breakpoint. With a
    // session-wide counter B's first auto-resume would read 2 > 1 and
    // falsely report ConditionalResumeLimit; per-isolate accounting
    // counts it as B's first and issues a resume RPC for B. A tripped
    // limit would skip that resume — the interrupter thread below
    // would never see a `resume` request and panic.
    vm.control()
        .send(event(
            "Debug",
            pause_breakpoint_event("isolates/2", &["breakpoints/b"]),
        ))
        .expect("push pause for B");

    // Push B's PauseInterrupted only after the mock has served B's
    // auto-resume — otherwise it could overwrite B's pause record in
    // `pause_events` before the condition is even evaluated.
    let interrupt = json!({
        "type": "Event",
        "kind": "PauseInterrupted",
        "isolate": isolate_ref("isolates/2", "2", "worker"),
        "timestamp": 2_i64,
    });
    let control = vm.control();
    let requests = vm.requests.clone();
    let interrupter = std::thread::spawn(move || {
        loop {
            match requests.recv_timeout(Duration::from_secs(5)) {
                Ok(request) if request.method == "resume" => break,
                Ok(_) => continue,
                Err(_) => panic!("mock never served B's auto-resume"),
            }
        }
        control
            .send(event("Debug", interrupt))
            .expect("push interrupt for B");
    });
    let snapshot = session
        .wait_for_pause("isolates/2")
        .expect("B's interrupt is a real stop");
    interrupter.join().expect("interrupter thread");
    assert_eq!(snapshot.isolate, "isolates/2");
    assert_eq!(
        snapshot.reason,
        StoppedReason::Interrupted,
        "B must not inherit A's auto-resume budget"
    );
    // A resumed once, B resumed once — two resume RPCs total.
    assert_eq!(resumes.load(Ordering::SeqCst), 2);
}

/// The breakpoint registry tracks handles across isolates without
/// cross-talk: resolve/remove events apply to the named breakpoint
/// only, wherever its isolate lives.
#[test]
fn breakpoint_registry_is_per_isolate_correct() {
    let vm = MockVm::start(|request| match request.method.as_str() {
        "getVM" => ok(vm_with_isolates(2)),
        "addBreakpointWithScriptUri" => {
            let isolate = request.params["isolateId"].as_str().unwrap_or_default();
            let id = match isolate {
                "isolates/1" => "breakpoints/1",
                _ => "breakpoints/2",
            };
            ok(json!({
                "type": "Breakpoint",
                "id": id,
                "breakpointNumber": 1,
                "enabled": true,
                "resolved": false,
                "location": {
                    "type": "UnresolvedSourceLocation",
                    "scriptUri": "package:app/main.dart",
                    "line": 7,
                },
            }))
        }
        "addBreakpointAtEntry" => ok(json!({
            "type": "Breakpoint",
            "id": "breakpoints/3",
            "breakpointNumber": 3,
            "enabled": true,
            "resolved": true,
            "location": source_location("scripts/1", "package:app/main.dart", 11, 1),
        })),
        "getStack" => Reply::Prefixed {
            prelude: vec![
                event(
                    "Debug",
                    json!({
                        "type": "Event",
                        "kind": "BreakpointResolved",
                        "isolate": isolate_ref("isolates/2", "2", "worker"),
                        "timestamp": 1_i64,
                        "breakpoint": resolved_breakpoint("breakpoints/2"),
                    }),
                ),
                event(
                    "Debug",
                    json!({
                        "type": "Event",
                        "kind": "BreakpointRemoved",
                        "isolate": isolate_ref("isolates/1", "1", "main"),
                        "timestamp": 1_i64,
                        "breakpoint": resolved_breakpoint("breakpoints/3"),
                    }),
                ),
            ],
            result: stack_with_vars(json!([])),
        },
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let line = |isolate: &str| BreakpointSpec {
        isolate: Some(isolate.to_owned()),
        location: BreakpointLocationSpec::Line {
            script_uri: "package:app/main.dart".to_owned(),
            line: 7,
            column: None,
        },
        condition: None,
    };
    let bp1 = session.add_breakpoint(line("isolates/1")).expect("bp1");
    let bp2 = session.add_breakpoint(line("isolates/2")).expect("bp2");
    let bp3 = session
        .add_breakpoint(BreakpointSpec {
            isolate: Some("isolates/1".to_owned()),
            location: BreakpointLocationSpec::Function {
                function_id: "functions/9".to_owned(),
            },
            condition: None,
        })
        .expect("bp3");
    assert_eq!(bp1.id, "breakpoints/1");
    assert_eq!(bp1.isolate, "isolates/1");
    assert!(!bp1.resolved);
    assert_eq!(bp2.isolate, "isolates/2");
    assert!(!bp2.resolved);
    assert_eq!(bp3.id, "breakpoints/3");
    assert!(bp3.resolved);

    session.stack(None, 1).expect("barrier rpc");
    session.poll_events().expect("poll events");

    let tracked: Vec<_> = session.breakpoints().cloned().collect();
    assert_eq!(tracked.len(), 2, "bp3 was removed by BreakpointRemoved");
    let bp1 = tracked
        .iter()
        .find(|bp| bp.id == "breakpoints/1")
        .expect("bp1 still tracked");
    assert_eq!(bp1.isolate, "isolates/1");
    assert!(!bp1.resolved, "bp1 got no resolve event");
    let bp2 = tracked
        .iter()
        .find(|bp| bp.id == "breakpoints/2")
        .expect("bp2 still tracked");
    assert_eq!(bp2.isolate, "isolates/2");
    assert!(bp2.resolved, "bp2 resolved via BreakpointResolved");
}

/// Fifteen thousand stream events overrun the bounded client feed
/// (capacity 10_000): the session ring plus the client-drop
/// accounting must still account for every event — nothing is
/// silently lost.
#[test]
fn event_feed_saturation_accounts_for_every_event() {
    const TOTAL: usize = 15_000;
    let prelude: Vec<Control> = (0..TOTAL)
        .map(|n| {
            event(
                "Debug",
                json!({ "type": "Event", "kind": "Resume", "timestamp": n as i64 }),
            )
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
    // Keep draining until the connection thread has delivered or
    // dropped every frame — the count must converge to TOTAL.
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        let batch = session.events_since(0, usize::MAX);
        let accounted = batch.events.len() as u64 + batch.dropped;
        if accounted == TOTAL as u64 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "accounted {accounted} of {TOTAL}"
        );
        session.poll_events().expect("poll events");
        std::thread::sleep(Duration::from_millis(1));
    }
    let batch = session.events_since(0, usize::MAX);
    assert_eq!(batch.events.len() as u64 + batch.dropped, TOTAL as u64);
    assert!(batch.dropped > 0, "the overrun must surface as drops");
}

/// Forward-compat event kinds ride the feed without disturbing known
/// traffic: an unrecognized `kind` lands in the ring as
/// `EventKind::Other` and the following `PauseBreakpoint` still
/// pauses its isolate.
#[test]
fn unknown_event_kinds_do_not_break_the_pump() {
    let prelude = vec![
        event(
            "Debug",
            json!({
                "type": "Event",
                "kind": "FutureEventKind2099",
                "isolate": isolate_ref("isolates/1", "1", "main"),
                "timestamp": 1_i64,
            }),
        ),
        event("Debug", untracked_pause("isolates/1")),
    ];
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
    assert_eq!(batch.events.len(), 2);
    assert_eq!(
        batch.events[0].event.kind,
        EventKind::Other("FutureEventKind2099".to_owned())
    );
    // Unknown kinds are not modeled by the typed `Event`, so the raw
    // JSON is retained (see `SessionEvent::raw`).
    assert_eq!(
        batch.events[0].raw.as_ref().expect("raw kept")["kind"],
        "FutureEventKind2099"
    );
    assert_eq!(batch.events[1].event.kind, EventKind::PauseBreakpoint);

    let snapshot = session
        .wait_for_pause("isolates/1")
        .expect("the pause after the unknown event still resolves");
    assert!(matches!(snapshot.reason, StoppedReason::Breakpoint { .. }));
}

/// Regression: a conditional breakpoint hit on a NON-main isolate
/// must evaluate its condition against that isolate's top frame, not
/// the session's main isolate. Before the fix `classify_pause` passed
/// `None` to `evaluate_in_frame`, which resolved to `isolates/1` —
/// on `isolates/2` the condition either saw the wrong frame's locals
/// or failed with RPC 113 and the falsy result auto-resumed a stop
/// that should have been reported.
#[test]
fn conditional_breakpoint_evaluates_in_the_paused_isolate() {
    let vm = MockVm::start(move |request| match request.method.as_str() {
        "getVM" => ok(vm_with_isolates(2)),
        "addBreakpointWithScriptUri" => ok(resolved_breakpoint("breakpoints/w")),
        // Falsy condition -> auto-resume, then a PauseInterrupted
        // stops the wait so the test can check the evaluate RPC.
        "evaluateInFrame" => Reply::Ok(instance_ref("objects/1", "Bool", "false")),
        "resume" => Reply::Prefixed {
            prelude: vec![event(
                "Debug",
                json!({
                    "type": "Event",
                    "kind": "PauseInterrupted",
                    "isolate": isolate_ref("isolates/2", "2", "worker"),
                    "timestamp": 3_i64,
                }),
            )],
            result: json!({ "type": "Success" }),
        },
        "getStack" => Reply::Ok(stack_with_vars(json!([]))),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    session
        .add_breakpoint(BreakpointSpec {
            isolate: Some("isolates/2".to_owned()),
            location: BreakpointLocationSpec::Line {
                script_uri: "package:app/worker.dart".to_owned(),
                line: 7,
                column: None,
            },
            condition: Some("ready".to_owned()),
        })
        .expect("add conditional breakpoint on isolates/2");
    vm.control()
        .send(event(
            "Debug",
            pause_breakpoint_event("isolates/2", &["breakpoints/w"]),
        ))
        .expect("push pause for isolates/2");

    let snapshot = session
        .wait_for_pause("isolates/2")
        .expect("interrupt after the falsy auto-resume");
    assert_eq!(snapshot.isolate, "isolates/2");
    assert_eq!(snapshot.reason, StoppedReason::Interrupted);

    let evaluate = vm.expect_request("evaluateInFrame");
    assert_eq!(
        evaluate.params["isolateId"], "isolates/2",
        "condition must evaluate in the isolate that paused"
    );
    assert_eq!(evaluate.params["expression"], "ready");
}
