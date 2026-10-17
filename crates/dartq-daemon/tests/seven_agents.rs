//! The M3 seven-agent overlay acceptance (the M3 row: "7 Agent
//! concurrency use case"): seven concurrent agent sessions over one
//! workspace, each holding a whole-file overlay view of the same file,
//! isolated across the declared default + `t` contexts,
//! under interleaved edits, an agent teardown landing mid-batch, and
//! per-agent eviction — over the real RPC wire, no network, no dart SDK.
//!
//! Concurrency shape: the daemon's synchronous core (std threads +
//! crossbeam channel, zero async) serves every client through the batch
//! drain — writer priority, reads coalesced per context — so the
//! tests drive the agents as batched concurrent traffic, the wire shape
//! production clients produce. This artifact pins correctness under the
//! seven-agent shape; the performance numbers are #486's benchmark gate.
//!
//! Per-agent signatures: each agent's buffer replaces `lib/main.dart`
//! wholesale with an import of its own `{tag}_buffer.dart`, so every view's
//! single `uri_does_not_exist` row names its owner — a signature no other
//! view (and not the base fixture's `missing.dart`) can produce, which
//! makes any cross-view leak assertable by message text.

mod common;

use std::fs;
use std::sync::Mutex;

use common::{
    BROKEN_MAIN, CLEAN_MAIN, Harness, Workspace, notification, publishes, request,
    spawn_with_heavy_context, store_root,
};
use serde_json::{Value, json};

/// `Workspace::new` pins one pid-derived temp root; the tests in this
/// binary serialize on this lock so a parallel test's workspace re-create
/// can never yank another test's workspace mid-session.
static WORKSPACE: Mutex<()> = Mutex::new(());

/// Poisoning is recovered deliberately: the guard protects no cross-test
/// state (every test owns a unique workspace and store root), so a
/// panicking holder has already failed loudly and the waiters only need
/// the serialization.
fn workspace_lock() -> std::sync::MutexGuard<'static, ()> {
    WORKSPACE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Agent `i`'s session id (`agent1`..).
fn agent(i: usize) -> String {
    format!("agent{i}")
}

/// The agent's staged whole-file buffer for `lib/main.dart`: its only
/// diagnostic names `{tag}_buffer.dart`, a signature no other view produces.
fn overlay_buffer(tag: &str) -> String {
    format!("import '{tag}_buffer.dart';\nvoid main() {{}}\n")
}

fn overlay_set(id: i64, name: &str, content: &str) -> Value {
    request(
        id,
        "dartq/overlay.set",
        json!({"agentId": name, "files": {"lib/main.dart": content}}),
    )
}

/// The `result` of the answer frame for `id`.
fn answer_result(h: &common::Harness, id: i64) -> Value {
    let transcript = h.transcript();
    transcript
        .iter()
        .find(|frame| frame.get("id") == Some(&json!(id)))
        .unwrap_or_else(|| panic!("request {id} answers: {transcript:?}"))["result"]
        .clone()
}

/// The `error` of the answer frame for `id`.
fn answer_error(h: &common::Harness, id: i64) -> Value {
    let transcript = h.transcript();
    transcript
        .iter()
        .find(|frame| frame.get("id") == Some(&json!(id)))
        .unwrap_or_else(|| panic!("request {id} answers: {transcript:?}"))["error"]
        .clone()
}

/// The `code` spellings of an analyze/check answer's diagnostic rows.
fn codes(result: &Value) -> Vec<&str> {
    result["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .map(|row| row["code"].as_str().expect("code"))
        .collect()
}

/// The rows' `message` texts.
fn messages(result: &Value) -> Vec<String> {
    result["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .map(|row| row["message"].as_str().expect("message").to_owned())
        .collect()
}

/// The answer is exactly the agent's own view: one `uri_does_not_exist` row
/// naming its buffer, and no other live agent's marker anywhere in it.
fn assert_isolated_view(h: &Harness, id: i64, own: &str, live: &[String]) {
    let result = answer_result(h, id);
    assert_eq!(
        codes(&result),
        ["uri_does_not_exist"],
        "agent {own}'s view carries only its own row: {result}"
    );
    let texts = messages(&result);
    let marker = format!("{own}_buffer.dart");
    assert!(
        texts.iter().any(|text| text.contains(&marker)),
        "agent {own}'s own marker must show: {texts:?}"
    );
    for other in live {
        if other.as_str() != own {
            let stray = format!("{other}_buffer.dart");
            assert!(
                texts.iter().all(|text| !text.contains(&stray)),
                "agent {own} must not see agent {other}: {texts:?}"
            );
        }
    }
}

/// The base answers exactly the fixture's own URI error — the disk text,
/// with no agent buffer anywhere in it.
fn assert_base_view(h: &Harness, id: i64) {
    let result = answer_result(h, id);
    assert_eq!(
        codes(&result),
        ["uri_does_not_exist"],
        "the base view: {result}"
    );
    let texts = messages(&result);
    assert!(
        texts.iter().any(|text| text.contains("missing.dart")),
        "the base row names the fixture's missing import: {texts:?}"
    );
    for i in 1..=7 {
        let stray = format!("{}_buffer.dart", agent(i));
        assert!(
            texts.iter().all(|text| !text.contains(&stray)),
            "an agent buffer leaked into the base: {texts:?}"
        );
    }
}

/// The acceptance scenario: seven agents hold isolated views over one
/// workspace and both declared contexts; interleaved writes (two restages
/// plus a base edit in one batch) keep every view consistent; teardown
/// evicts one agent without touching the rest; agent views never publish.
#[test]
#[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
fn seven_agents_hold_isolated_views_over_one_workspace() {
    let _guard = workspace_lock();
    let workspace = Workspace::new(BROKEN_MAIN);
    let main_dart = workspace.path("lib/main.dart");
    let mut h = spawn_with_heavy_context(&workspace, &store_root("seven-agents-isolation"));
    // The startup drain already pushed the sweep's base view (the harness
    // settled it): the seeded `missing.dart` row is the baseline every
    // agent view isolates from.

    // Seven agents stage conflicting whole-file views of the SAME file.
    let tags: Vec<String> = (1..=7).map(agent).collect();
    let sets: Vec<Value> = (1..=7)
        .map(|i| {
            overlay_set(
                100 + i64::try_from(i).unwrap_or(i64::MAX),
                &agent(i),
                &overlay_buffer(&agent(i)),
            )
        })
        .collect();
    h.burst(&sets, 7);

    // The isolation matrix, ONE concurrent batch: each agent's view plus
    // the base on both declared contexts — agents 6 and 7 ride the `t`
    // fork.
    let mut matrix: Vec<Value> = (1..=7)
        .map(|i| {
            let params = if i >= 6 {
                json!({"agentId": agent(i), "context": "t"})
            } else {
                json!({"agentId": agent(i)})
            };
            request(
                200 + i64::try_from(i).unwrap_or(i64::MAX),
                "dartq/analyze",
                params,
            )
        })
        .collect();
    matrix.push(request(210, "dartq/analyze", json!({})));
    matrix.push(request(211, "dartq/analyze", json!({"context": "t"})));
    // Nine answers; nothing publishes — the overlays are agent state, the
    // base never moved.
    h.burst(&matrix, 9);
    for i in 1..=7 {
        assert_isolated_view(
            &h,
            200 + i64::try_from(i).unwrap_or(i64::MAX),
            &agent(i),
            &tags,
        );
    }
    assert_base_view(&h, 210);
    assert_base_view(&h, 211);

    // After every wear has drained: both contexts' base views still read
    // the disk text — the per-turn wear/restore left nothing behind in the
    // shared content pool (the base is the agents' only shared
    // mutable surface, and it never moved).
    h.burst(
        &[
            request(212, "dartq/analyze", json!({})),
            request(213, "dartq/analyze", json!({"context": "t"})),
        ],
        2,
    );
    assert_base_view(&h, 212);
    assert_base_view(&h, 213);

    // Interleaved writes in one batch: two agents restage while the client
    // reports a base edit through `dartq/sync` — intake acks first, then the
    // drain whose publish diff carries the base's clear.
    fs::write(&main_dart, CLEAN_MAIN).expect("rewrite");
    let revised = ["agent3b", "agent7b"];
    h.burst(
        &[
            overlay_set(220, &agent(3), &overlay_buffer(revised[0])),
            overlay_set(221, &agent(7), &overlay_buffer(revised[1])),
            notification("dartq/sync", json!({"paths": ["lib/main.dart"]})),
        ],
        3,
    );

    // The post-interleave matrix: agents 3 and 7 answer their REVISED
    // views, the other five their originals — an interleaved base edit
    // plus two restages left every view consistent — and the base is clean.
    let mut after: Vec<Value> = (1..=7)
        .map(|i| {
            request(
                230 + i64::try_from(i).unwrap_or(i64::MAX),
                "dartq/analyze",
                json!({"agentId": agent(i)}),
            )
        })
        .collect();
    after.push(request(238, "dartq/analyze", json!({})));
    // Eight answers; the unchanged base view stays suppressed.
    h.burst(&after, 8);
    let mut current = tags.clone();
    current[2] = revised[0].to_owned();
    current[6] = revised[1].to_owned();
    for i in 1..=7 {
        assert_isolated_view(
            &h,
            230 + i64::try_from(i).unwrap_or(i64::MAX),
            &current[i - 1],
            &current,
        );
    }
    let base = answer_result(&h, 238);
    assert!(
        codes(&base).is_empty(),
        "the base edit landed; the base view is clean: {base}"
    );

    // Teardown ordering: eviction is per agent, sticks, and is idempotent —
    // the evicted view fails loud, survivors answer untouched.
    h.step(
        &request(240, "dartq/overlay.clear", json!({"agentId": agent(4)})),
        1,
    );
    h.step(
        &request(241, "dartq/analyze", json!({"agentId": agent(4)})),
        1,
    );
    let error = answer_error(&h, 241);
    assert_eq!(
        error["code"], -32602,
        "the evicted agent fails loud: {error}"
    );
    assert!(
        error["message"].as_str().unwrap_or("").contains(&agent(4)),
        "the error names the agent: {error}"
    );
    h.step(
        &request(242, "dartq/overlay.clear", json!({"agentId": agent(4)})),
        1,
    );
    h.step(
        &request(243, "dartq/analyze", json!({"agentId": agent(4)})),
        1,
    );
    assert_eq!(answer_error(&h, 243)["code"], -32602, "eviction sticks");
    h.step(
        &request(244, "dartq/analyze", json!({"agentId": agent(5)})),
        1,
    );
    assert_isolated_view(&h, 244, &agent(5), &current);

    let (code, transcript) = h.join_ended();
    assert_eq!(code, 0);
    // Across the whole session: two publishes, both the base view — the
    // sweep's rows and the sync's clear — and neither ever carries an agent
    // view.
    let pushed = publishes(&transcript);
    assert_eq!(pushed.len(), 2, "agent views never publish: {pushed:?}");
    for tag in &current {
        let marker = format!("{tag}_buffer.dart");
        for push in &pushed {
            assert!(
                !push.to_string().contains(&marker),
                "an agent buffer leaked into a publish: {push}"
            );
        }
    }

    fs::remove_dir_all(&workspace.root).ok();
}

/// Agent close during in-flight work: agent 4's teardown lands mid-batch,
/// between its own queued read and the requests staged behind it. The
/// in-flight view fails loud (never a silent base answer), the post-close
/// request fails at intake, every other concurrent view comes back intact,
/// and a rejoin leaves the shared state uncorrupted.
#[test]
fn agent_close_during_in_flight_work_keeps_shared_state_consistent() {
    let _guard = workspace_lock();
    let workspace = Workspace::new(BROKEN_MAIN);
    let mut h = spawn_with_heavy_context(&workspace, &store_root("seven-agents-close"));

    let tags: Vec<String> = (1..=7).map(agent).collect();
    let sets: Vec<Value> = (1..=7)
        .map(|i| {
            overlay_set(
                10 + i64::try_from(i).unwrap_or(i64::MAX),
                &agent(i),
                &overlay_buffer(&agent(i)),
            )
        })
        .collect();
    h.burst(&sets, 7);

    // One concurrent batch: seven in-flight views, agent 4's teardown
    // landing mid-batch, and one more agent-4 request staged after it. The
    // teardown acks at intake; the drain answers everything staged before.
    let mut batch: Vec<Value> = (1..=7)
        .map(|i| {
            request(
                20 + i64::try_from(i).unwrap_or(i64::MAX),
                "dartq/analyze",
                json!({"agentId": agent(i)}),
            )
        })
        .collect();
    batch.push(request(
        30,
        "dartq/overlay.clear",
        json!({"agentId": agent(4)}),
    ));
    batch.push(request(31, "dartq/analyze", json!({"agentId": agent(4)})));
    h.burst(&batch, 9);

    // The teardown acked...
    assert!(
        answer_error(&h, 30).is_null(),
        "the teardown acks: {:?}",
        answer_result(&h, 30)
    );
    // ...the in-flight analyze staged BEFORE it fails loud — a cleared view
    // is an unknown one, never a silent base answer...
    let error = answer_error(&h, 24);
    assert_eq!(
        error["code"], -32602,
        "the in-flight view fails loud: {error}"
    );
    assert!(
        error["message"].as_str().unwrap_or("").contains(&agent(4)),
        "the error names the agent: {error}"
    );
    // ...and the request staged after it fails at intake the same way.
    assert_eq!(answer_error(&h, 31)["code"], -32602);

    // Every other concurrent view came back intact.
    for i in [1, 2, 3, 5, 6, 7] {
        assert_isolated_view(
            &h,
            20 + i64::try_from(i).unwrap_or(i64::MAX),
            &agent(i),
            &tags,
        );
    }

    // Recovery: agent 4 rejoins with a fresh view; the survivors and the
    // base are untouched — the close left no corruption behind.
    h.burst(
        &[
            overlay_set(40, &agent(4), &overlay_buffer("agent4b")),
            request(41, "dartq/analyze", json!({"agentId": agent(4)})),
            request(42, "dartq/analyze", json!({"agentId": agent(1)})),
            request(43, "dartq/analyze", json!({})),
        ],
        4,
    );
    assert_isolated_view(&h, 41, "agent4b", &tags);
    assert_isolated_view(&h, 42, &agent(1), &tags);
    assert_base_view(&h, 43);

    let (code, _) = h.join_ended();
    assert_eq!(code, 0);

    fs::remove_dir_all(&workspace.root).ok();
}

/// The >7 policy: the agent set is open-ended (`Agent A/B/C …`) —
/// seven is the M3 acceptance pin, not a capacity — and the daemon's
/// documented bound (256 live agent overlays, the session contract) is
/// loud at its boundary and recovers by eviction.
#[test]
fn agent_population_exceeds_seven_up_to_the_documented_bound() {
    let _guard = workspace_lock();
    let workspace = Workspace::new(BROKEN_MAIN);
    let mut h = spawn_with_heavy_context(&workspace, &store_root("seven-agents-bound"));

    // Eight agents stage and isolate over the one workspace.
    let eight: Vec<String> = (1..=8).map(agent).collect();
    let sets: Vec<Value> = (1..=8)
        .map(|i| {
            overlay_set(
                10 + i64::try_from(i).unwrap_or(i64::MAX),
                &agent(i),
                &overlay_buffer(&agent(i)),
            )
        })
        .collect();
    h.burst(&sets, 8);
    h.burst(
        &[
            request(30, "dartq/analyze", json!({"agentId": agent(8)})),
            request(31, "dartq/analyze", json!({"agentId": agent(1)})),
        ],
        2,
    );
    assert_isolated_view(&h, 30, &agent(8), &eight);
    assert_isolated_view(&h, 31, &agent(1), &eight);

    // Scale to the documented bound: 256 live overlays. Staging is lazy —
    // the bulk ride as intake acks, no analysis.
    let bulk: Vec<Value> = (9..=256)
        .map(|i| {
            let name = format!("agent{i}");
            overlay_set(100 + i64::from(i), &name, &overlay_buffer(&name))
        })
        .collect();
    h.burst(&bulk, bulk.len());

    // The 257th distinct agent crosses the bound: loud INVALID_PARAMS
    // naming the policy and the number.
    h.step(
        &overlay_set(400, "overflow", &overlay_buffer("overflow")),
        1,
    );
    let error = answer_error(&h, 400);
    assert_eq!(
        error["code"], -32602,
        "the overflow agent fails loud: {error}"
    );
    let text = error["message"].as_str().unwrap_or("");
    assert!(text.contains("256"), "the error names the bound: {error}");
    assert!(
        text.contains("too many live agent overlays"),
        "the error states the policy: {error}"
    );

    // Eviction frees the slot: the same agent now stages, answers, and
    // isolates like any of the seven.
    h.burst(
        &[
            request(401, "dartq/overlay.clear", json!({"agentId": "agent9"})),
            overlay_set(402, "overflow", &overlay_buffer("overflow")),
            request(403, "dartq/analyze", json!({"agentId": "overflow"})),
        ],
        3,
    );
    let mut neighbors: Vec<String> = (1..=9).map(agent).collect();
    neighbors.push("overflow".to_owned());
    assert_isolated_view(&h, 403, "overflow", &neighbors);

    // The bulk staging never disturbed the earliest views.
    h.step(
        &request(410, "dartq/analyze", json!({"agentId": agent(1)})),
        1,
    );
    assert_isolated_view(&h, 410, &agent(1), &neighbors);

    let (code, _) = h.join_ended();
    assert_eq!(code, 0);

    fs::remove_dir_all(&workspace.root).ok();
}
