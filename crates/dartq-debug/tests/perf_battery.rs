//! Performance battery for the debug session's event path. All tests
//! are `#[ignore]`d by default — run with
//! `cargo test -p dartq-debug --test perf_battery -- --ignored --nocapture`.
//!
//! Every test prints its measured numbers (visible with
//! `--nocapture`); the only hard asserts are regression floors called
//! out per test.

mod common;

use std::time::{Duration, Instant};

use common::{
    Control, MockVm, Reply, default_reply, instance_ref, pause_breakpoint_event, stack_with_vars,
};
use dartq_debug::{DebugSession, SessionConfig};
use serde_json::{Value, json};

fn fast_config() -> SessionConfig {
    SessionConfig {
        pause_timeout: Duration::from_secs(30),
        rpc_timeout: Duration::from_secs(10),
        ..SessionConfig::default()
    }
}

fn event(stream_id: &str, event: Value) -> Control {
    Control::Event {
        stream_id: stream_id.to_owned(),
        event,
    }
}

/// A small `Resume` notification.
fn resume_event(n: usize) -> Value {
    json!({ "type": "Event", "kind": "Resume", "timestamp": n as i64 })
}

/// A fat `Logging` record (~1 KiB payload) — the heavy end of real
/// stream traffic.
fn logging_event(n: usize) -> Value {
    let message = "x".repeat(1024);
    json!({
        "type": "Event",
        "kind": "Logging",
        "timestamp": n as i64,
        "logRecord": {
            "type": "LogRecord",
            "message": instance_ref(&format!("objects/{n}"), "String", &message),
            "level": 800,
            "sequenceNumber": n as i64,
            "loggerName": instance_ref("objects/l", "String", "perf"),
            "time": 1_700_000_000_000_i64,
        },
    })
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let index = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

/// End-to-end event throughput: the mock writes 10_000
/// `streamNotify` frames back-to-back as a getStack prelude; the
/// client's connection thread plus the session drain must keep up.
/// Floor: > 10_000 events/s (localhost should land far above).
#[test]
#[ignore]
fn perf_event_throughput() {
    const TOTAL: usize = 10_000;
    let prelude: Vec<Control> = (0..TOTAL)
        .map(|n| event("Debug", resume_event(n)))
        .collect();
    let vm = MockVm::start(move |request| match request.method.as_str() {
        "getStack" => Reply::Prefixed {
            prelude: prelude.clone(),
            result: stack_with_vars(json!([])),
        },
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");

    let start = Instant::now();
    session.stack(None, 1).expect("barrier rpc");
    loop {
        session.poll_events().expect("poll events");
        let batch = session.events_since(0);
        let accounted = batch.events.len() as u64 + batch.dropped;
        if accounted >= TOTAL as u64 {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "accounted {accounted} of {TOTAL} in 30s"
        );
    }
    let elapsed = start.elapsed();
    let per_sec = TOTAL as f64 / elapsed.as_secs_f64();
    println!(
        "perf_event_throughput: {TOTAL} events in {elapsed:?} -> {per_sec:.0} events/s"
    );
    assert!(
        per_sec > 10_000.0,
        "throughput floor missed: {per_sec:.0} events/s"
    );
}

/// `wait_for_pause` wake latency, two cuts:
///
/// * `wire`: one `PauseBreakpoint` per iteration pushed through the
///   mock's control channel, timed send-to-snapshot. This includes
///   the mock's control tick plus the `getStack` RPC the snapshot
///   performs (two transport round trips through the tick loops), so
///   it bounds the whole pipeline, not just the wait.
/// * `queued`: the pause event is delivered as a getStack prelude,
///   so it sits in the client feed before `wait_for_pause` starts —
///   this isolates the session-side wake and must stay in the low
///   single-digit ms (the old 5 ms sleep floor could not go below
///   ~5 ms; the event-driven wait has no floor of its own).
#[test]
#[ignore]
fn perf_wait_for_pause_wake_latency() {
    const ITERATIONS: usize = 200;
    let queued_pause = event(
        "Debug",
        pause_breakpoint_event("isolates/1", &["breakpoints/foreign"]),
    );
    let vm = MockVm::start(move |request| match request.method.as_str() {
        "getStack" if request.params["limit"] == json!(1) => Reply::Prefixed {
            prelude: vec![queued_pause.clone()],
            result: stack_with_vars(json!([])),
        },
        "getStack" => Reply::Ok(stack_with_vars(json!([]))),
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");
    let control = vm.control();

    let mut wire = Vec::with_capacity(ITERATIONS);
    let mut queued = Vec::with_capacity(ITERATIONS);
    for i in 0..ITERATIONS {
        if i % 2 == 0 {
            // Wire path: push through the mock control channel, then
            // wait for the pause to arrive over the socket.
            let sent = Instant::now();
            control
                .send(event(
                    "Debug",
                    pause_breakpoint_event("isolates/1", &["breakpoints/foreign"]),
                ))
                .expect("push pause");
            let snapshot = session
                .wait_for_pause("isolates/1")
                .expect("pause snapshot");
            wire.push(sent.elapsed());
            assert!(matches!(
                snapshot.reason,
                dartq_debug::StoppedReason::Breakpoint { .. }
            ));
            // Re-arm: clearing the consumed pause record mirrors what
            // a real resume does before the next wait.
            session.resume(Some("isolates/1")).expect("resume");
        } else {
            // Queued path: `stack(limit=1)` carries the next pause as
            // a prelude, so it is already in the feed when the wait
            // begins — pure session-side wake latency.
            session
                .stack(Some("isolates/1"), 1)
                .expect("prelude barrier");
            let start = Instant::now();
            let snapshot = session
                .wait_for_pause("isolates/1")
                .expect("pause snapshot");
            queued.push(start.elapsed());
            assert!(matches!(
                snapshot.reason,
                dartq_debug::StoppedReason::Breakpoint { .. }
            ));
            session.resume(Some("isolates/1")).expect("resume");
        }
    }
    wire.sort();
    queued.sort();
    let report = |name: &str, samples: &[Duration]| {
        println!(
            "perf_wait_for_pause_wake_latency[{name}]: n={} min={:?} p50={:?} p95={:?} p99={:?} max={:?}",
            samples.len(),
            samples[0],
            percentile(samples, 0.50),
            percentile(samples, 0.95),
            percentile(samples, 0.99),
            samples[samples.len() - 1],
        );
    };
    report("wire", &wire);
    report("queued", &queued);
    // The queued path's floor is one `getStack` RPC round trip (the
    // client drains writes every ~1 ms READ_TICK + the mock's own
    // 10 ms tick); anything above two round trips is a regression. The
    // wire path adds the mock's control tick and one more round trip.
    assert!(
        percentile(&queued, 0.95) < Duration::from_millis(8),
        "queued p95 regressed"
    );
    assert!(
        percentile(&wire, 0.95) < Duration::from_millis(80),
        "wire p95 regressed"
    );
}

/// Session-side absorb cost: 10_000 mixed events (small `Resume`,
/// ~1 KiB `Logging`, `PauseBreakpoint` with 10 breakpoint hits)
/// queued in the client feed, then a single `poll_events` drain
/// timed end-to-end. Prints ns/event; no hard floor beyond the
/// accounting invariant.
#[test]
#[ignore]
fn perf_absorb_mixed_events() {
    const TOTAL: usize = 10_000;
    let fat_pause: Vec<&str> = vec!["breakpoints/foreign"; 10];
    let prelude: Vec<Control> = (0..TOTAL)
        .map(|n| match n % 3 {
            0 => event("Debug", resume_event(n)),
            1 => event("Logging", logging_event(n)),
            _ => event("Debug", pause_breakpoint_event("isolates/1", &fat_pause)),
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

    // In-order delivery: when the getStack response arrived, every
    // prelude frame was already decoded and queued in the client
    // feed (10_000 events == feed capacity, so nothing dropped).
    // One poll_events drains all of them — this is the session-side
    // per-event absorb cost (ring push + isolate/breakpoint/pause
    // bookkeeping), measured on its own.
    let drain_start = Instant::now();
    session.poll_events().expect("timed drain");
    let elapsed = drain_start.elapsed();
    let batch = session.events_since(0);
    println!(
        "perf_absorb_mixed_events: {TOTAL} mixed events drained in {elapsed:?} -> {:.0} ns/event (ring {} retained, {} dropped)",
        elapsed.as_nanos() as f64 / TOTAL as f64,
        batch.events.len(),
        batch.dropped,
    );
    assert_eq!(batch.events.len() as u64 + batch.dropped, TOTAL as u64);
}

/// Ring eviction under saturation: 15_000 events against a 10_000
/// slot feed — the invariant is that seen + dropped == sent, however
/// the loss splits between the client feed and the session ring.
#[test]
#[ignore]
fn perf_ring_eviction_consistency() {
    const TOTAL: usize = 15_000;
    let prelude: Vec<Control> = (0..TOTAL)
        .map(|n| event("Debug", resume_event(n)))
        .collect();
    let vm = MockVm::start(move |request| match request.method.as_str() {
        "getStack" => Reply::Prefixed {
            prelude: prelude.clone(),
            result: stack_with_vars(json!([])),
        },
        _ => default_reply(request),
    });
    let mut session = DebugSession::attach_with(&vm.ws_uri, fast_config()).expect("attach");

    let start = Instant::now();
    session.stack(None, 1).expect("barrier rpc");
    let batch = loop {
        session.poll_events().expect("poll events");
        let batch = session.events_since(0);
        if batch.events.len() as u64 + batch.dropped >= TOTAL as u64 {
            break batch;
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "accounting never converged: {} of {TOTAL}",
            batch.events.len() as u64 + batch.dropped
        );
    };
    let retained = batch.events.len() as u64;
    let dropped = batch.dropped;
    println!(
        "perf_ring_eviction_consistency: {TOTAL} sent -> {retained} retained + {dropped} dropped in {:?}",
        start.elapsed()
    );
    assert_eq!(retained + dropped, TOTAL as u64);
    assert!(dropped > 0);
}
