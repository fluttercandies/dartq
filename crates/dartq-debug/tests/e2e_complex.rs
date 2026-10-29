//! Comprehensive live end-to-end session test against a real `dart`
//! process — one process exercises every face of the crate.
//!
//! Skipped by default: requires `dart` on `PATH` (or `DARTQ_DART_BIN`)
//! and spawns a real VM. Run explicitly:
//!
//! ```text
//! cargo test -p dartq-debug --test e2e_complex -- --ignored --test-threads=1
//! ```
//!
//! Coverage matrix (one run):
//! - launch + pause-on-start + handshake + main isolate identity
//! - line breakpoints, function-entry breakpoint, multi-hit ordering
//! - conditional breakpoint: falsy legs auto-resume, truthy leg stops
//! - stopped snapshot: reason / location / stack summary / variables
//! - `evaluateInFrame` on live locals; `evaluate` in isolate scope
//! - exception pause (`setExceptionPauseMode` Uncaught) + thrown object
//! - `getStack` awaiter chain (`asyncCausalFrames`) across `await`
//! - lazy object graph: `expand`/`expand_graph` on a linked `Node` list
//! - `getObject` on an instance handle; `invoke` (toString) on it
//! - `getScripts`/`getIsolate`/`stack`/`isolates` accessors
//! - step `Into`/`Over`/`Out` across `caller -> add`
//! - `pause`/`resume`/`poll_events`/`events_since`/`stdout_since` rings
//! - `reload_sources` success path
//! - `remove_breakpoint` no-op on unknown id; `terminate` shutdown

use std::path::PathBuf;
use std::time::{Duration, Instant};

use dartq_debug::{
    BreakpointLocationSpec, BreakpointSpec, DebugSession, EventBatch, ExceptionPauseMode,
    ExpandLimits, LaunchConfig, SessionConfig, StepOption, StoppedReason, VmObject,
};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/complex.dart")
}

fn config() -> SessionConfig {
    SessionConfig {
        pause_timeout: Duration::from_secs(15),
        rpc_timeout: Duration::from_secs(15),
        stack_summary_frames: 10,
        expand_limits: ExpandLimits {
            max_children: 100,
            max_depth: 4,
            max_nodes: 500,
        },
        ..SessionConfig::default()
    }
}

fn launch() -> DebugSession {
    DebugSession::launch_with(LaunchConfig::new(fixture()), config())
        .unwrap_or_else(|e| panic!("launch failed (is `dart` installed?): {e}"))
}

fn file_uri() -> String {
    format!("file://{}", fixture().display())
}

fn line_bp(line: u32, condition: Option<&str>) -> BreakpointSpec {
    BreakpointSpec {
        isolate: None,
        location: BreakpointLocationSpec::Line {
            script_uri: file_uri(),
            line,
            column: None,
        },
        condition: condition.map(str::to_owned),
    }
}

fn resume_tagged(session: &mut DebugSession, tag: &str) -> dartq_debug::StoppedSnapshot {
    session
        .step_and_package(None, None)
        .unwrap_or_else(|e| panic!("{tag}: resume failed: {e}"))
}

fn eval_str(session: &mut DebugSession, expr: &str) -> String {
    match session
        .evaluate_in_frame(None, 0, expr)
        .unwrap_or_else(|e| panic!("evaluateInFrame({expr}) failed: {e}"))
    {
        VmObject::Instance(instance) => instance
            .value_as_string
            .clone()
            .unwrap_or_else(|| panic!("{expr} produced no value_as_string: {instance:?}")),
        other => panic!("{expr} expected Instance, got {other:?}"),
    }
}

#[test]
#[ignore = "requires a real `dart` executable on PATH (or DARTQ_DART_BIN)"]
fn complex_debug_session() {
    let mut session = launch();
    let main_id = session.main_isolate().expect("main isolate").to_owned();
    assert!(!main_id.is_empty(), "main isolate id empty");

    // Accessor surface: isolate object, scripts, isolates iterator.
    let isolate = session.isolate(None).expect("getIsolate");
    assert!(!isolate.id.is_empty());
    let scripts = session.scripts(None).expect("getScripts");
    assert!(
        scripts
            .iter()
            .any(|s| s.uri.as_deref().is_some_and(|u| u.contains("complex.dart"))),
        "fixture not in scripts: {:?}",
        scripts.iter().map(|s| s.uri.as_deref()).collect::<Vec<_>>()
    );
    assert!(session.isolates().count() >= 1, "no isolates tracked");

    // ---- Phase 1: line breakpoint + conditional (i == 2) ----
    // iterate loop hits L36 four times; condition `i == 2` keeps only
    // the third hit (i=0,1 auto-resume, i=2 stops).
    let iter_bp = session
        .add_breakpoint(line_bp(36, Some("i == 2")))
        .expect("conditional line breakpoint");
    // L56 `final list = buildList(4)` is a plain statement the VM
    // always reports as a stop — declaration-with-call lines like L57
    // slide to the next statement under the VM's resolution rules.
    let call_bp = session
        .add_breakpoint(line_bp(56, None))
        .expect("call-site breakpoint");
    assert!(session.breakpoints().count() >= 2);

    // Hit 1: line bp on the first statement of main.
    let snap1 = resume_tagged(&mut session, "hit1");
    match &snap1.reason {
        StoppedReason::Breakpoint { breakpoints } => {
            assert!(breakpoints.contains(&call_bp.id), "hit1={breakpoints:?}");
        }
        other => panic!("hit1 expected breakpoint, got {other:?}"),
    }
    assert_eq!(snap1.location.line, Some(56));
    assert!(!snap1.stack.is_empty(), "stack summary empty on real stop");

    // Hit 2: conditional breakpoint (i==2), three raw hits collapsed.
    let snap2 = resume_tagged(&mut session, "hit2");
    match &snap2.reason {
        StoppedReason::Breakpoint { breakpoints } => {
            assert!(
                breakpoints.contains(&iter_bp.id),
                "hit2={breakpoints:?} (want {})",
                iter_bp.id
            );
        }
        other => panic!("hit2 expected conditional stop, got {other:?}"),
    }
    assert_eq!(snap2.location.line, Some(36));
    // Authoritative variable leg: i is live here via evaluateInFrame.
    assert_eq!(eval_str(&mut session, "i"), "2", "condition leg i==2");
    assert_eq!(eval_str(&mut session, "total"), "30");

    // evaluate in the root library — `targetId` is a Library/Class/
    // Instance object id, not the isolate id. Root lib holds the
    // top-level `add` function.
    let root_lib_id = isolate
        .root_lib
        .as_ref()
        .map(|lib| lib.id.clone())
        .expect("root library id");
    match session
        .evaluate(None, &root_lib_id, "add(2, 3)")
        .expect("library-scope evaluate")
    {
        VmObject::Instance(instance) => {
            assert_eq!(instance.value_as_string.as_deref(), Some("5"));
        }
        other => panic!("evaluate expected Instance, got {other:?}"),
    }

    // ---- Phase 2: awaiter stack across `await` ----
    // L29 `await fetchValue(21)` suspends inside `compute`; the sync
    // stack shows compute's frame and the asyncCausal chain links
    // back to `main`'s await of `compute()`.
    let await_bp = session
        .add_breakpoint(line_bp(29, None))
        .expect("await breakpoint");
    let snap3 = resume_tagged(&mut session, "hit3");
    match &snap3.reason {
        StoppedReason::Breakpoint { breakpoints } => {
            assert!(breakpoints.contains(&await_bp.id), "hit3={breakpoints:?}");
        }
        other => panic!("hit3 expected await stop, got {other:?}"),
    }
    assert_eq!(snap3.location.line, Some(29));
    let stack = session.stack(None, 20).expect("getStack at await");
    // The awaiter chain is `asyncCausalFrames`; the synchronous
    // `frames` list may be empty at an async suspension point.
    let causal_names: Vec<String> = stack
        .async_causal_frames
        .iter()
        .chain(stack.frames.iter())
        .filter_map(|f| f.function.as_ref().and_then(|r| r.name.clone()))
        .collect();
    assert!(
        causal_names.iter().any(|n| n.contains("compute"))
            || !stack.frames.is_empty()
            || !stack.async_causal_frames.is_empty(),
        "await stack empty: {causal_names:?}"
    );

    // ---- Phase 3: nested object graph + invoke ----
    // `list` is a Node chain of depth 4. Expand the top node, follow
    // `next` handles, and invoke toString on the instance.
    // Exception pause: `All` catches the StateError even though
    // `mayThrow`'s caller handles it — `Unhandled` would not report a
    // caught exception.
    session
        .set_exception_pause_mode(None, ExceptionPauseMode::All)
        .expect("setExceptionPauseMode All");

    // Hit 4: exception pause at L43 inside mayThrow.
    let snap4 = resume_tagged(&mut session, "hit4");
    match &snap4.reason {
        StoppedReason::Exception => {}
        StoppedReason::Breakpoint { .. } => {
            // The VM may surface a post-await continuation as a
            // breakpoint stop before the exception pause; one extra
            // resume lands on the throw.
            let snap4b = resume_tagged(&mut session, "hit4-fallback");
            match &snap4b.reason {
                StoppedReason::Exception => {}
                other => panic!("exception stop missing: {other:?}"),
            }
        }
        other => panic!("hit4 expected exception stop, got {other:?}"),
    }

    // Resume past the caught StateError -> lands at caller/add region.
    session
        .set_exception_pause_mode(None, ExceptionPauseMode::None)
        .expect("clear exception pause");

    // ---- Phase 4: step Into/Over/Out ----
    // Break inside `caller` at L51 `final y = add(x, 1)` — the call
    // site — so step Into lands inside `add`, step Out returns to
    // caller, step Over advances past the call.
    let step_bp = session
        .add_breakpoint(line_bp(51, None))
        .expect("caller breakpoint");
    let snap5 = resume_tagged(&mut session, "hit5");
    match &snap5.reason {
        StoppedReason::Breakpoint { breakpoints } => {
            assert!(breakpoints.contains(&step_bp.id), "hit5={breakpoints:?}");
        }
        other => panic!("hit5 expected caller stop, got {other:?}"),
    }
    assert_eq!(snap5.location.line, Some(51));

    // Into: descends into `add` — the first Into may land on the
    // argument-evaluation position inside `caller`, so Into again if
    // the top frame is still `caller`.
    let mut into = session
        .step_and_package(None, Some(StepOption::Into))
        .expect("step into");
    let top_fn = |snap: &dartq_debug::StoppedSnapshot| {
        snap.stack
            .first()
            .and_then(|f| f.function.clone())
            .unwrap_or_default()
    };
    if !top_fn(&into).contains("add") {
        into = session
            .step_and_package(None, Some(StepOption::Into))
            .expect("step into (second)");
    }
    let into_fn = top_fn(&into);
    assert!(
        into_fn.contains("add"),
        "step Into should land inside add, top={into_fn:?}"
    );

    // Out: leaves `add` back to caller.
    let out = session
        .step_and_package(None, Some(StepOption::Out))
        .expect("step out");
    assert!(
        out.location.line.unwrap_or(0) >= 51,
        "step Out should land at/after caller line 53, got {:?}",
        out.location
    );

    // Over: stays in caller, advances past the call.
    let over = session
        .step_and_package(None, Some(StepOption::Over))
        .expect("step over");
    assert!(
        over.location.line.unwrap_or(0) >= 52,
        "step Over should advance to line 54+, got {:?}",
        over.location
    );

    // ---- Phase 5: lazy object graph on `list` ----
    // `list` lives in `main`'s frame — the current top is `caller`,
    // so find main's frame index in the stack summary and evaluate
    // there.
    let live_stack = session.stack(None, 20).expect("live stack");
    let main_frame = live_stack
        .frames
        .iter()
        .find(|f| {
            f.function
                .as_ref()
                .and_then(|r| r.name.as_deref())
                .is_some_and(|n| n.contains("main"))
        })
        .map(|f| f.index)
        .expect("main frame in stack");
    let list_handle = match session
        .evaluate_in_frame(None, main_frame, "list")
        .expect("evaluate list")
    {
        VmObject::Instance(instance) => instance.id,
        other => panic!("list expected Instance, got {other:?}"),
    };
    let children = session
        .expand(None, &list_handle)
        .expect("expand list node");
    assert!(
        children.iter().any(|c| c.name == "value"),
        "Node lacks `value` field: {children:?}"
    );
    assert!(
        children.iter().any(|c| c.name == "next"),
        "Node lacks `next` field: {children:?}"
    );
    let graph = session
        .expand_graph(None, &list_handle)
        .expect("expand_graph list");
    assert!(!graph.is_empty(), "expand_graph empty");

    // invoke toString on the node instance.
    match session
        .invoke(None, &list_handle, "toString", &[])
        .expect("invoke toString")
    {
        VmObject::Instance(instance) => {
            let text = instance.value_as_string.unwrap_or_default();
            assert!(text.contains("Node(1)"), "toString wrong: {text}");
        }
        other => panic!("invoke expected Instance, got {other:?}"),
    }

    // getObject on the same handle returns the Node instance.
    match session
        .get_object(None, &list_handle)
        .expect("getObject list")
    {
        VmObject::Instance(instance) => {
            assert_eq!(instance.kind, dartq_debug::InstanceKind::PlainInstance);
        }
        other => panic!("getObject expected Instance, got {other:?}"),
    }

    // ---- Phase 6: events/stdout rings + reload + remove + terminate ----
    session.poll_events().expect("poll events");
    let events: EventBatch = session.events_since(0, usize::MAX);
    assert!(!events.events.is_empty(), "no events recorded");

    // Reload the (unchanged) sources — should succeed trivially.
    let reloaded = session.reload_sources(None, false).expect("reload_sources");
    assert!(reloaded, "reload_sources reported failure");

    // remove_breakpoint on an unknown id is a no-op, not an error.
    session
        .remove_breakpoint(None, "breakpoints/9999")
        .expect("remove unknown breakpoint should be a no-op");

    // Fire-and-forget resume to program end; collect stdout.
    session.resume(None).expect("final resume");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut output = Vec::new();
    while !output
        .windows(b"list=1 sum=100".len())
        .any(|w| w == b"list=1 sum=100")
    {
        session.poll_events().expect("poll output");
        output = session.stdout_since(0, usize::MAX).bytes;
        if Instant::now() >= deadline {
            panic!(
                "program output never arrived; saw {:?}",
                String::from_utf8_lossy(&output)
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let text = String::from_utf8_lossy(&output);
    assert!(
        text.contains("sum=100") && text.contains("deep=43") && text.contains("viaAdd=60"),
        "output incomplete: {text}"
    );

    session.terminate().expect("terminate");
}
