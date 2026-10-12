//! `dartq/queryBatch` end to end (PRD §15.8.4): one round trip answers N
//! symbols × M kinds — one frozen v1 query envelope per `(item, kind)`
//! pair, request order, no aggregate envelope (dartq-schema's frozen batch
//! convention). The empty batch answers an empty array; an unknown symbol
//! answers the single face's own not-found shape; the single `dartq/query`
//! request keeps parsing and answering byte-identically. No network, no
//! dart SDK, no binary: the server runs in-process over the pipe harness.

mod common;

use std::fs;
use std::sync::Mutex;

use common::{CLEAN_MAIN, Workspace, notification, request, spawn, store_root};
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

/// `initialize` + `initialized`: the fixture is clean, so the sweep
/// publishes nothing.
fn handshake(h: &mut common::Harness) {
    h.step(&request(1, "initialize", json!({})), 1);
    h.step(&notification("initialized", json!({})), 0);
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

/// The query fixture: a lib pair with a class, a method, a function, and
/// cross-file uses — `Foo` type-annotated (`reference` @33) and constructed
/// (`invoked` @42) in b.dart, `foo()` called there (`invoked` @51).
fn seeded_query_workspace() -> Workspace {
    let workspace = Workspace::new(CLEAN_MAIN);
    fs::write(
        workspace.path("lib/a.dart"),
        "class Foo {\n  void meth() {}\n}\nvoid foo() {}\n",
    )
    .expect("write a.dart");
    fs::write(
        workspace.path("lib/b.dart"),
        "import 'a.dart';\nvoid main() {\n  Foo? f = Foo();\n  foo();\n}\n",
    )
    .expect("write b.dart");
    workspace
}

/// 3 symbols × 2 kinds answer all 6 envelopes in request order (item order,
/// then each item's kind order), every envelope the frozen v1 query
/// contract, each pair walking the one session graph.
#[test]
fn batch_answers_every_pair_in_request_order() {
    let _guard = workspace_lock();
    let workspace = seeded_query_workspace();
    let mut h = spawn(&workspace, &store_root("rpc-query-batch-order"));
    handshake(&mut h);

    h.step(
        &request(
            2,
            "dartq/queryBatch",
            json!({
                "queries": [
                    { "name": "Foo", "kinds": ["symbol", "callers"] },
                    { "name": "foo", "kinds": ["symbol", "callers"] },
                    { "name": "meth", "kinds": ["symbol", "callers"] },
                ]
            }),
        ),
        1,
    );
    let results = answer_result(&h, 2)
        .as_array()
        .expect("the batch answers an array")
        .clone();
    assert_eq!(results.len(), 6, "3 symbols x 2 kinds: {results:?}");
    let names: Vec<&str> = results
        .iter()
        .map(|envelope| envelope["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names, ["Foo", "Foo", "foo", "foo", "meth", "meth"]);

    // Foo: the symbol face keeps the annotation use plus the invocation;
    // the callers face keeps the invocation only.
    assert_eq!(results[0]["schema_version"], 1);
    assert_eq!(
        results[0]["references"].as_array().expect("refs").len(),
        2,
        "reference @33 plus invoked @42"
    );
    let caller_rows = results[1]["references"].as_array().expect("refs");
    assert_eq!(caller_rows.len(), 1);
    assert_eq!(caller_rows[0]["kind"], "invoked");
    assert_eq!(caller_rows[0]["offset"], 42);

    // foo: one call site under both kinds; meth: declared, never used.
    assert_eq!(results[2]["references"].as_array().expect("refs").len(), 1);
    assert_eq!(results[3]["references"].as_array().expect("refs").len(), 1);
    assert_eq!(results[4]["references"].as_array().expect("refs").len(), 0);
    assert_eq!(
        results[4]["declarations"].as_array().expect("decls").len(),
        1
    );

    for envelope in &results {
        let text = serde_json::to_string(envelope).expect("serialize");
        dartq_schema::validate(&text, dartq_schema::Domain::Query).expect("schema-valid");
    }

    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// An empty batch answers an empty array — no items, no results, no error.
#[test]
fn empty_batch_answers_an_empty_array() {
    let _guard = workspace_lock();
    let workspace = seeded_query_workspace();
    let mut h = spawn(&workspace, &store_root("rpc-query-batch-empty"));
    handshake(&mut h);

    h.step(&request(2, "dartq/queryBatch", json!({ "queries": [] })), 1);
    assert_eq!(answer_result(&h, 2), json!([]));

    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// An unknown symbol answers the single face's own not-found shape —
/// envelope for envelope — and absent item kinds default to the symbol
/// face, the single request's own default.
#[test]
fn unknown_symbol_answers_the_single_faces_not_found_shape() {
    let _guard = workspace_lock();
    let workspace = seeded_query_workspace();
    let mut h = spawn(&workspace, &store_root("rpc-query-batch-unknown"));
    handshake(&mut h);

    h.step(&request(2, "dartq/query", json!({ "name": "Nope" })), 1);
    h.step(
        &request(
            3,
            "dartq/queryBatch",
            json!({ "queries": [{ "name": "Nope" }] }),
        ),
        1,
    );
    let single = answer_result(&h, 2);
    assert_eq!(answer_result(&h, 3), json!([single]));

    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// The legacy single-symbol request keeps parsing and answering
/// byte-identically — a one-item batch with default kinds is exactly the
/// single face's envelope, the frozen face unperturbed by the additive
/// batch face.
#[test]
fn single_symbol_request_still_parses_and_answers() {
    let _guard = workspace_lock();
    let workspace = seeded_query_workspace();
    let mut h = spawn(&workspace, &store_root("rpc-query-batch-legacy"));
    handshake(&mut h);

    h.step(&request(2, "dartq/query", json!({ "name": "Foo" })), 1);
    h.step(
        &request(
            3,
            "dartq/queryBatch",
            json!({ "queries": [{ "name": "Foo" }] }),
        ),
        1,
    );
    let single = answer_result(&h, 2);
    assert_eq!(single["schema_version"], 1);
    assert_eq!(single["name"], "Foo");
    assert_eq!(
        answer_result(&h, 3),
        json!([single]),
        "one default-kind batch item is the single face's own answer"
    );

    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// An unknown kind spelling inside a batch item answers `INVALID_PARAMS`
/// naming the accepted set — the single face's own intake behavior, one
/// spelling surface for both faces.
#[test]
fn unknown_item_kind_answers_invalid_params() {
    let _guard = workspace_lock();
    let workspace = seeded_query_workspace();
    let mut h = spawn(&workspace, &store_root("rpc-query-batch-kind"));
    handshake(&mut h);

    h.step(
        &request(
            2,
            "dartq/queryBatch",
            json!({ "queries": [{ "name": "Foo", "kinds": ["subclasses"] }] }),
        ),
        1,
    );
    let error = answer_error(&h, 2);
    assert_eq!(error["code"], -32602, "INVALID_PARAMS: {error}");
    let message = error["message"].as_str().expect("message");
    for accepted in ["symbol", "references", "callers"] {
        assert!(
            message.contains(accepted),
            "the accepted set is named ({accepted}): {message}"
        );
    }

    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}
