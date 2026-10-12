//! `dartq/query`'s `kind` parameter end to end (PRD §15.2): the default
//! (absent-kind) answer is byte-identical to the explicit `symbol` request,
//! `references` serves the symbol rows (the CLI face's seam), `callers`
//! keeps the invocation-shaped subset, and an unknown kind answers
//! `INVALID_PARAMS` naming the accepted set. Rows are pinned to the frozen
//! schema query domain's field names and the answers validate against the
//! published `Domain::Query` contract. No network, no dart SDK, no binary:
//! the server runs in-process over the pipe harness.

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

/// The reference rows' schema-domain shape: the required locator fields on
/// every row, the use-file column present (the engine carries it), and the
/// `qualified` spelling omitted — the contract types it as the qualified
/// name string while the row model carries only the bit.
fn assert_reference_row_shape(row: &Value) {
    for field in ["target_file", "kind", "offset", "length"] {
        assert!(
            row.get(field).is_some(),
            "reference row carries {field}: {row}"
        );
    }
    assert!(
        row.get("file").is_some(),
        "the use-file column rides the row: {row}"
    );
    assert!(
        row.get("qualified").is_none(),
        "the unproducible qualified spelling stays omitted: {row}"
    );
}

/// The default query face: absent `kind` and explicit `"symbol"` answer
/// byte-identically — the pre-kind behavior, unpinned and unchanged.
#[test]
fn default_missing_kind_is_the_symbol_answer() {
    let _guard = workspace_lock();
    let workspace = seeded_query_workspace();
    let a = workspace.path("lib/a.dart");
    let mut h = spawn(&workspace, &store_root("rpc-query-kind-default"));
    handshake(&mut h);

    h.step(&request(2, "dartq/query", json!({ "name": "Foo" })), 1);
    h.step(
        &request(3, "dartq/query", json!({ "name": "Foo", "kind": "symbol" })),
        1,
    );
    let absent = answer_result(&h, 2);
    let explicit = answer_result(&h, 3);
    assert_eq!(
        absent, explicit,
        "absent kind answers the symbol face byte-identically"
    );

    // The symbol face's compact shape (the CLI JSON face verbatim).
    assert_eq!(absent["schema_version"], 1);
    assert_eq!(absent["name"], "Foo");
    let declarations = absent["declarations"].as_array().expect("declarations");
    assert_eq!(declarations.len(), 1);
    assert_eq!(declarations[0]["file"], a.display().to_string());
    assert_eq!(declarations[0]["kind"], "class");
    assert_eq!(declarations[0]["name"], "Foo");
    assert_eq!(declarations[0]["line"], 1);
    assert_eq!(declarations[0]["column"], 7);
    assert_eq!(declarations[0]["length"], 3);
    let references = absent["references"].as_array().expect("references");
    assert_eq!(
        references.len(),
        2,
        "the annotation use plus the invocation"
    );
    assert_eq!(references[0]["kind"], "reference");
    assert_eq!(references[0]["offset"], 33);
    assert_eq!(references[1]["kind"], "invoked");
    assert_eq!(references[1]["offset"], 42);
    references.iter().for_each(assert_reference_row_shape);

    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// `references` serves the symbol rows — the index's reference surface is
/// name-keyed and already kinded, so the answer equals the default face's
/// field for field, under the frozen query contract.
#[test]
fn references_kind_serves_the_symbol_rows() {
    let _guard = workspace_lock();
    let workspace = seeded_query_workspace();
    let mut h = spawn(&workspace, &store_root("rpc-query-kind-references"));
    handshake(&mut h);

    h.step(&request(2, "dartq/query", json!({ "name": "Foo" })), 1);
    h.step(
        &request(
            3,
            "dartq/query",
            json!({ "name": "Foo", "kind": "references" }),
        ),
        1,
    );
    let symbol = answer_result(&h, 2);
    let references = answer_result(&h, 3);
    assert_eq!(
        references, symbol,
        "references serves the symbol rows verbatim"
    );
    assert_eq!(references["schema_version"], 1);

    // The declaration rows' schema-domain shape (compact carries the
    // convenience `container`, null for top-level rows).
    let declarations = references["declarations"].as_array().expect("declarations");
    assert_eq!(declarations.len(), 1);
    for field in ["file", "kind", "name", "line", "column", "length"] {
        assert!(
            declarations[0].get(field).is_some(),
            "declaration row carries {field}"
        );
    }
    assert!(declarations[0].get("container").is_some());

    let text = serde_json::to_string(&references).expect("serialize");
    dartq_schema::validate(&text, dartq_schema::Domain::Query).expect("schema-valid");

    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// `callers` keeps the invocation-shaped rows only: every reference row is
/// `invoked`, the declaration rows stay for target identity, and the shape
/// is the same reference-row contract as the other kinds.
#[test]
fn callers_kind_keeps_the_invocation_rows() {
    let _guard = workspace_lock();
    let workspace = seeded_query_workspace();
    let mut h = spawn(&workspace, &store_root("rpc-query-kind-callers"));
    handshake(&mut h);

    // `Foo`: the annotation use drops, the constructor site stays.
    h.step(
        &request(
            2,
            "dartq/query",
            json!({ "name": "Foo", "kind": "callers" }),
        ),
        1,
    );
    let result = answer_result(&h, 2);
    assert_eq!(result["schema_version"], 1);
    let declarations = result["declarations"].as_array().expect("declarations");
    assert_eq!(declarations.len(), 1, "the declaration rows stay");
    let references = result["references"].as_array().expect("references");
    assert_eq!(
        references.len(),
        1,
        "only the invocation survives: {references:?}"
    );
    assert_eq!(references[0]["kind"], "invoked");
    assert_eq!(references[0]["offset"], 42);
    references.iter().for_each(assert_reference_row_shape);

    // `foo`: the call site is the one row, already invocation-shaped.
    h.step(
        &request(
            3,
            "dartq/query",
            json!({ "name": "foo", "kind": "callers" }),
        ),
        1,
    );
    let result = answer_result(&h, 3);
    let references = result["references"].as_array().expect("references");
    assert_eq!(references.len(), 1);
    assert_eq!(references[0]["kind"], "invoked");
    assert_eq!(references[0]["offset"], 51);
    let declarations = result["declarations"].as_array().expect("declarations");
    assert_eq!(declarations.len(), 1);
    assert_eq!(declarations[0]["kind"], "function");

    let text = serde_json::to_string(&result).expect("serialize");
    dartq_schema::validate(&text, dartq_schema::Domain::Query).expect("schema-valid");

    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// An unknown kind answers `INVALID_PARAMS` at intake with the accepted set
/// named, and the face stays usable for the requests after it.
#[test]
fn unknown_kind_answers_invalid_params() {
    let _guard = workspace_lock();
    let workspace = seeded_query_workspace();
    let mut h = spawn(&workspace, &store_root("rpc-query-kind-unknown"));
    handshake(&mut h);

    h.step(
        &request(
            2,
            "dartq/query",
            json!({ "name": "Foo", "kind": "subclasses" }),
        ),
        1,
    );
    let error = answer_error(&h, 2);
    assert_eq!(error["code"], -32602, "INVALID_PARAMS: {error}");
    assert!(error.get("result").is_none(), "no result rides the error");
    let message = error["message"].as_str().expect("message");
    for accepted in [
        "symbol",
        "references",
        "callers",
        "implementations",
        "dependencies",
    ] {
        assert!(
            message.contains(accepted),
            "the accepted set is named ({accepted}): {message}"
        );
    }

    // The rejected request answers at intake without touching the session:
    // the next kinded query answers normally.
    h.step(
        &request(
            3,
            "dartq/query",
            json!({ "name": "Foo", "kind": "callers" }),
        ),
        1,
    );
    let result = answer_result(&h, 3);
    assert_eq!(result["name"], "Foo");

    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// The inheritance fixture: `Animal` declares the abstract member
/// `breathe`, `Dog extends Animal` overrides it, `Cat extends Animal` does
/// not, and b.dart's import makes it the declaring library's importer.
fn seeded_inheritance_workspace() -> Workspace {
    let workspace = Workspace::new(CLEAN_MAIN);
    fs::write(
        workspace.path("lib/a.dart"),
        "abstract class Animal {\n  void breathe();\n}\n",
    )
    .expect("write a.dart");
    fs::write(
        workspace.path("lib/b.dart"),
        "import 'a.dart';\nclass Dog extends Animal {\n  void breathe() {}\n}\nclass Cat extends Animal {}\n",
    )
    .expect("write b.dart");
    workspace
}

/// The `implementations` and `dependencies` kinds end to end: the single
/// face serves the subtype/override rows and the importer row on the
/// frozen reserved arrays, the batch face answers both spellings through
/// the same expansion, and every answer validates against the published
/// query contract.
#[test]
fn implementation_and_dependency_kinds_end_to_end() {
    let _guard = workspace_lock();
    let workspace = seeded_inheritance_workspace();
    let b = workspace.path("lib/b.dart");
    let mut h = spawn(&workspace, &store_root("rpc-query-kind-impl-deps"));
    handshake(&mut h);

    // `implementations` over the type: both extends subtypes, no reference
    // rows, the declaration row kept for identity.
    h.step(
        &request(
            2,
            "dartq/query",
            json!({ "name": "Animal", "kind": "implementations" }),
        ),
        1,
    );
    let result = answer_result(&h, 2);
    assert_eq!(result["name"], "Animal");
    let implementations = result["implementations"].as_array().expect("array");
    assert_eq!(
        implementations.len(),
        2,
        "Dog and Cat, the extends subtypes: {implementations:?}"
    );
    assert_eq!(implementations[0]["name"], "Dog");
    assert_eq!(implementations[0]["kind"], "class");
    assert_eq!(implementations[1]["name"], "Cat");
    let declarations = result["declarations"].as_array().expect("declarations");
    assert_eq!(declarations.len(), 1);
    assert!(result["references"].as_array().expect("array").is_empty());
    let text = serde_json::to_string(&result).expect("serialize");
    dartq_schema::validate(&text, dartq_schema::Domain::Query).expect("schema-valid");

    // `implementations` over the member: the overriding row only (Cat
    // overrides nothing).
    h.step(
        &request(
            3,
            "dartq/query",
            json!({ "name": "breathe", "kind": "implementations" }),
        ),
        1,
    );
    let result = answer_result(&h, 3);
    let implementations = result["implementations"].as_array().expect("array");
    assert_eq!(implementations.len(), 1, "only Dog overrides breathe");
    assert_eq!(implementations[0]["name"], "breathe");
    assert_eq!(implementations[0]["container"], "Dog");
    assert_eq!(implementations[0]["file"], b.display().to_string());

    // `dependencies`: the declaring library's importer row, `path`-shaped.
    h.step(
        &request(
            4,
            "dartq/query",
            json!({ "name": "Animal", "kind": "dependencies" }),
        ),
        1,
    );
    let result = answer_result(&h, 4);
    let dependencies = result["dependencies"].as_array().expect("array");
    assert_eq!(dependencies.len(), 1, "b.dart imports a.dart");
    assert_eq!(dependencies[0]["path"], b.display().to_string());
    let text = serde_json::to_string(&result).expect("serialize");
    dartq_schema::validate(&text, dartq_schema::Domain::Query).expect("schema-valid");

    // The batch face accepts both spellings through the same item shape.
    h.step(
        &request(
            5,
            "dartq/queryBatch",
            json!({ "queries": [ {
                "name": "Animal",
                "kinds": ["implementations", "dependencies"],
            } ] }),
        ),
        1,
    );
    let batch = answer_result(&h, 5);
    let answers = batch.as_array().expect("batch answers");
    assert_eq!(answers.len(), 2, "one answer per (item, kind) pair");
    assert_eq!(
        answers[0]["implementations"]
            .as_array()
            .expect("array")
            .len(),
        2
    );
    assert_eq!(
        answers[1]["dependencies"].as_array().expect("array").len(),
        1
    );

    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// The lifecycle smoke: three kinds batched into one turn share the single
/// coalesced read, answer in request order from the same analyzed view, and
/// the session ends clean — the kind rides the existing request lifecycle
/// (cancellation and every other scheduler property are kind-independent by
/// construction).
#[test]
fn kinded_queries_share_one_turn_of_the_request_lifecycle() {
    let _guard = workspace_lock();
    let workspace = seeded_query_workspace();
    let mut h = spawn(&workspace, &store_root("rpc-query-kind-lifecycle"));
    handshake(&mut h);

    h.burst(
        &[
            request(2, "dartq/query", json!({ "name": "Foo" })),
            request(
                3,
                "dartq/query",
                json!({ "name": "Foo", "kind": "references" }),
            ),
            request(
                4,
                "dartq/query",
                json!({ "name": "Foo", "kind": "callers" }),
            ),
        ],
        3,
    );
    let symbol = answer_result(&h, 2);
    let references = answer_result(&h, 3);
    let callers = answer_result(&h, 4);
    assert_eq!(references, symbol, "references == symbol rows");
    let caller_rows = callers["references"].as_array().expect("references");
    let symbol_rows = symbol["references"].as_array().expect("references");
    assert_eq!(
        caller_rows.len() + 1,
        symbol_rows.len(),
        "callers drops exactly the non-invocation use"
    );

    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}
