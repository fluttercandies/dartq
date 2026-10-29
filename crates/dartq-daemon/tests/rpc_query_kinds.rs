//! `dartq/query`'s `kind` parameter end to end: the default
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

use common::{CLEAN_MAIN, Workspace, request, spawn, store_root};
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
    for field in ["target", "kind", "off", "len"] {
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
    let mut h = spawn(&workspace, &store_root("rpc-query-kind-default"));

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

    // The symbol face's compact shape (the CLI JSON face verbatim, the
    // shared shaper's stable package-form labels included).
    assert_eq!(absent["v"], 1);
    assert_eq!(absent["sym"], "Foo");
    let decls = absent["decls"].as_array().expect("decls");
    assert_eq!(decls.len(), 1);
    assert_eq!(
        decls[0]["file"], "lib/a.dart",
        "the unified locator's root-relative spelling"
    );
    assert_eq!(decls[0]["kind"], "class");
    assert_eq!(decls[0]["name"], "Foo");
    assert_eq!(decls[0]["line"], 1);
    assert_eq!(decls[0]["col"], 7);
    assert_eq!(decls[0]["len"], 3);
    let refs = absent["refs"].as_array().expect("refs");
    assert_eq!(refs.len(), 2, "the annotation use plus the invocation");
    assert_eq!(refs[0]["kind"], "reference");
    assert_eq!(refs[0]["off"], 33);
    assert_eq!(refs[1]["kind"], "invoked");
    assert_eq!(refs[1]["off"], 42);
    refs.iter().for_each(assert_reference_row_shape);

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// The minimal tier shapes through the shared shaper's own tier cut (#1556):
/// the red-line rows keep the stable package-form labels — the projection
/// is tier-agnostic, so the daemon's minimal answer cannot drift from the
/// CLI's.
#[test]
fn minimal_tier_keeps_the_package_form_labels() {
    let _guard = workspace_lock();
    let workspace = seeded_query_workspace();
    let mut h = spawn(&workspace, &store_root("rpc-query-minimal-labels"));

    h.step(
        &request(
            2,
            "dartq/query",
            json!({ "name": "Foo", "verbosity": "minimal" }),
        ),
        1,
    );
    let result = answer_result(&h, 2);
    let decls = result["decls"].as_array().expect("decls");
    assert_eq!(decls.len(), 1);
    assert_eq!(
        decls[0]["file"], "lib/a.dart",
        "the unified locator's root-relative spelling"
    );
    let refs = result["refs"].as_array().expect("refs");
    assert_eq!(refs.len(), 2);
    assert_eq!(
        refs[0]["target"], "lib/a.dart",
        "the unified locator's root-relative spelling"
    );
    assert!(
        refs[0].get("file").is_none(),
        "the minimal red line keeps the target locator only: {row}",
        row = &refs[0]
    );

    let (code, _) = h.join_ended();
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
    assert_eq!(references["v"], 1);

    // The declaration rows' schema-domain shape (compact carries the
    // convenience `container` when a row has one — the omission rule drops
    // it on top-level rows rather than spelling a null).
    let decls = references["decls"].as_array().expect("decls");
    assert_eq!(decls.len(), 1);
    for field in ["file", "kind", "name", "line", "col", "len"] {
        assert!(
            decls[0].get(field).is_some(),
            "declaration row carries {field}"
        );
    }
    // `Foo` is a top-level class, so `container` is omitted per the
    // null/empty omission — not spelled as a null.
    assert!(decls[0].get("container").is_none());

    let text = serde_json::to_string(&references).expect("serialize");
    dartq_schema::validate(&text, dartq_schema::Domain::Query).expect("schema-valid");

    let (code, _) = h.join_ended();
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
    assert_eq!(result["v"], 1);
    let decls = result["decls"].as_array().expect("decls");
    assert_eq!(decls.len(), 1, "the declaration rows stay");
    let refs = result["refs"].as_array().expect("refs");
    assert_eq!(refs.len(), 1, "only the invocation survives: {refs:?}");
    assert_eq!(refs[0]["kind"], "invoked");
    assert_eq!(refs[0]["off"], 42);
    refs.iter().for_each(assert_reference_row_shape);

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
    let refs = result["refs"].as_array().expect("refs");
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0]["kind"], "invoked");
    assert_eq!(refs[0]["off"], 51);
    let decls = result["decls"].as_array().expect("decls");
    assert_eq!(decls.len(), 1);
    assert_eq!(decls[0]["kind"], "function");

    let text = serde_json::to_string(&result).expect("serialize");
    dartq_schema::validate(&text, dartq_schema::Domain::Query).expect("schema-valid");

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// An unknown kind answers `INVALID_PARAMS` at intake with the accepted set
/// named, and the face stays usable for the requests after it.
#[test]
fn unknown_kind_answers_invalid_params() {
    let _guard = workspace_lock();
    let workspace = seeded_query_workspace();
    let mut h = spawn(&workspace, &store_root("rpc-query-kind-unknown"));

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
    assert_eq!(result["sym"], "Foo");

    let (code, _) = h.join_ended();
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
/// face serves the subtype/override rows on the frozen reserved arrays,
/// and the `dependencies` forward answer for a library with no imports is
/// the omitted additive array; the batch face answers both spellings
/// through the same expansion, and every answer validates against the
/// published query contract.
#[test]
fn implementation_and_dependency_kinds_end_to_end() {
    let _guard = workspace_lock();
    let workspace = seeded_inheritance_workspace();
    let mut h = spawn(&workspace, &store_root("rpc-query-kind-impl-deps"));

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
    assert_eq!(result["sym"], "Animal");
    let implementations = result["implementations"].as_array().expect("array");
    assert_eq!(
        implementations.len(),
        2,
        "Dog and Cat, the extends subtypes: {implementations:?}"
    );
    assert_eq!(implementations[0]["name"], "Dog");
    assert_eq!(implementations[0]["kind"], "class");
    assert_eq!(implementations[1]["name"], "Cat");
    let decls = result["decls"].as_array().expect("decls");
    assert_eq!(decls.len(), 1);
    assert!(result["refs"].as_array().expect("array").is_empty());
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
    assert_eq!(
        implementations[0]["file"], "lib/b.dart",
        "the unified locator's root-relative spelling (the shared shaper's projection)"
    );

    // `dependencies` is the forward face: the declaring library's own
    // imports. `Animal` lives in a.dart, which imports nothing, so the
    // additive array is omitted (the schema's empty-row omission) — the
    // importer answer (b.dart) is the impact face's, not this kind's.
    h.step(
        &request(
            4,
            "dartq/query",
            json!({ "name": "Animal", "kind": "dependencies" }),
        ),
        1,
    );
    let result = answer_result(&h, 4);
    assert!(
        result.get("deps").is_none(),
        "a.dart imports nothing — the additive array is omitted: {result:?}"
    );
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
    // The forward `dependencies` answer for `Animal` (a.dart imports
    // nothing) is empty, so the additive array is omitted here too.
    assert!(
        answers[1].get("deps").is_none(),
        "a.dart imports nothing — the additive array is omitted: {answers:?}"
    );

    let (code, _) = h.join_ended();
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
    let caller_rows = callers["refs"].as_array().expect("refs");
    let symbol_rows = symbol["refs"].as_array().expect("refs");
    assert_eq!(
        caller_rows.len() + 1,
        symbol_rows.len(),
        "callers drops exactly the non-invocation use"
    );

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}
