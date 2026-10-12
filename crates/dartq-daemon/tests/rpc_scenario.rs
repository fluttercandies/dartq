//! The scripted end-to-end scenario a Cockpit-style client drives over the
//! daemon RPC (the m4-recon Q1/Q2 contract: the daemon RPC faces themselves
//! over LSP Content-Length framing — no bespoke protocol). One fixture
//! workspace, one session, the full face sequence as a data-driven steps
//! table; the table doubles as the replayable conformance script (each entry
//! is the wire message verbatim plus the transcript growth it settles on and
//! the response contract it asserts — schema-domain field names where the
//! frozen schemas pin them):
//!
//! initialize → didOpen (the analysis face) → query symbol → `overlay.set`
//! (agent A edits a file) → analyze (the agent view) → query references
//! under the overlay → `overlay.commit` → delta query (cursor echo) →
//! didChange (full sync) → the publish diff arrives → full query vs the
//! second `since:` query returning only the delta rows → `dartq/impact` on
//! the changed symbol → `dartq/explain` with the session short-ids (the
//! repeat path back-references `s1`, mapped by the `symbols` table) →
//! shutdown/exit. No network, no dart SDK.

mod common;

use std::fs;
use std::sync::Mutex;

use common::{
    BROKEN_MAIN, CLEAN_MAIN, Harness, Workspace, notification, request, spawn, store_root,
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

/// The scenario fixture: `Foo`/`foo` declared in lib/a.dart, used from
/// lib/b.dart and test/a_test.dart — clean under the synthetic SDK, so the
/// sweep publishes nothing and every later frame is scenario-caused.
fn scenario_workspace() -> Workspace {
    let workspace = Workspace::new(CLEAN_MAIN);
    fs::write(workspace.path("lib/a.dart"), A_SOURCE).expect("write a.dart");
    fs::write(workspace.path("lib/b.dart"), B_SOURCE).expect("write b.dart");
    fs::write(workspace.path("test/a_test.dart"), TEST_SOURCE).expect("write a_test.dart");
    workspace
}

const A_SOURCE: &str = "class Foo {\n  void meth() {}\n}\nvoid foo() {}\n";
/// Every local is read (`f?.meth()`), so no `unused_local_variable` row ever
/// fires and the clean-open baseline holds. The `Foo` rows stay at
/// `reference@33` / `invoked@42` — the fix appends after line 3.
const B_SOURCE: &str =
    "import 'a.dart';\nvoid main() {\n  Foo? f = Foo();\n  f?.meth();\n  foo();\n}\n";
const TEST_SOURCE: &str =
    "import 'package:fixture/a.dart';\nvoid main() {\n  Foo? f = Foo();\n  f?.meth();\n}\n";
/// Agent A's staged rewrite of lib/b.dart: one more `Foo` use pair than the
/// base — the delta the overlay-view query must surface. Lint-clean like
/// the base; its second pair sits at `reference@64` / `invoked@73`
/// (ASCII-counted: line 3 ends at 48, the `f?.meth();` line runs 49–61,
/// line 5 starts at 62 with `Foo` at 62+2 and 62+11).
const AGENT_B_SOURCE: &str = "import 'a.dart';\nvoid main() {\n  Foo? f = Foo();\n  f?.meth();\n  Foo? g = Foo();\n  g?.meth();\n  foo();\n}\n";

/// One scripted step: the wire message, the transcript growth to settle on
/// (0 = settle through a probe answer, for frame-less steps), and the
/// response-contract check over the transcript so far.
struct Step<'check> {
    message: Value,
    settle: usize,
    check: Box<dyn Fn(&Harness) + 'check>,
}

fn step<'check>(message: Value, settle: usize, check: impl Fn(&Harness) + 'check) -> Step<'check> {
    Step {
        message,
        settle,
        check: Box::new(check),
    }
}

/// The scenario runner: sends each step in order, settles on its transcript
/// growth, and asserts its contract.
fn run_script(h: &mut Harness, script: &[Step<'_>]) {
    for step in script {
        h.step(&step.message, step.settle);
        (step.check)(h);
    }
}

/// The `result` of the answer frame for `id`.
fn answer_result(h: &Harness, id: i64) -> Value {
    let transcript = h.transcript();
    transcript
        .iter()
        .find(|frame| frame.get("id") == Some(&json!(id)))
        .unwrap_or_else(|| panic!("request {id} answers: {transcript:?}"))["result"]
        .clone()
}

/// The session's `textDocument/publishDiagnostics` pushes so far (owned
/// copies — the assertions run after the transcript borrow would die).
fn publishes(h: &Harness) -> Vec<Value> {
    h.transcript()
        .into_iter()
        .filter(|frame| frame["method"] == "textDocument/publishDiagnostics")
        .collect()
}

/// A query answer's declaration files in canonical (path) order.
fn declaration_files(query: &Value) -> Vec<String> {
    query["declarations"]
        .as_array()
        .expect("declarations")
        .iter()
        .map(|row| row["file"].as_str().expect("file").to_owned())
        .collect()
}

/// The published schema contract for one response's domain.
fn assert_schema_valid(result: &Value, domain: dartq_schema::Domain) {
    dartq_schema::validate(&result.to_string(), domain).expect("schema-valid");
}

/// The scenario, end to end. The cursor arithmetic every delta below chains
/// from: the didOpen drain = 1, the overlay.set = 2, the commit merge = 3,
/// the didChange drain = 4.
#[test]
fn cockpit_script_over_the_daemon_rpc_surface() {
    let _guard = workspace_lock();
    let workspace = scenario_workspace();
    let a = workspace.path("lib/a.dart");
    let b = workspace.path("lib/b.dart");
    let main = workspace.path("lib/main.dart");
    let a_test = workspace.path("test/a_test.dart");
    let main_uri = format!("file://{}", main.display());
    let mut h = spawn(&workspace, &store_root("rpc-scenario"));

    let script = vec![
        // -- lifecycle: the negotiated capability set pins the LSP surface
        // plus the experimental dartq/* family, each entry carrying the
        // response contract version --
        step(request(1, "initialize", json!({})), 1, |h| {
            let result = answer_result(h, 1);
            assert_eq!(result["capabilities"]["textDocumentSync"], 1);
            assert_eq!(result["capabilities"]["positionEncoding"], "utf-16");
            assert_eq!(result["serverInfo"]["name"], "dartq");
            let experimental = result["capabilities"]["experimental"]
                .as_object()
                .expect("experimental capability map");
            assert_eq!(
                experimental.len(),
                13,
                "the full dartq/* family: {experimental:?}"
            );
            for method in [
                "dartq/index",
                "dartq/analyze",
                "dartq/check",
                "dartq/query",
                "dartq/queryBatch",
                "dartq/impact",
                "dartq/fix",
                "dartq/assists",
                "dartq/compat",
                "dartq/explain",
                "dartq/overlay.set",
                "dartq/overlay.commit",
                "dartq/overlay.clear",
            ] {
                assert_eq!(
                    experimental[method]["schemaVersion"], 1,
                    "{method} announced with the contract version"
                );
            }
        }),
        // -- the sweep finds the fixture clean: no publish --
        step(notification("initialized", json!({})), 0, |_| {}),
        // -- the analysis face: the client opens main.dart with its on-disk
        // text; a clean view diffs quiet, so nothing publishes --
        step(
            notification(
                "textDocument/didOpen",
                json!({"textDocument": {"uri": main_uri, "languageId": "dart", "version": 1,
                       "text": CLEAN_MAIN}}),
            ),
            0,
            |h| {
                assert!(
                    publishes(h).is_empty(),
                    "a clean open publishes nothing: {:?}",
                    publishes(h)
                );
            },
        ),
        // -- the base symbol query: the declaration plus each file's
        // annotation-plus-invocation pair (both b.dart and the test file
        // use Foo) --
        step(
            request(2, "dartq/query", json!({ "name": "Foo" })),
            1,
            |h| {
                let result = answer_result(h, 2);
                assert_eq!(result["schema_version"], 1);
                let declarations = result["declarations"].as_array().expect("declarations");
                assert_eq!(declarations.len(), 1);
                assert_eq!(declarations[0]["file"], a.display().to_string());
                assert_eq!(declarations[0]["kind"], "class");
                assert_eq!(declarations[0]["name"], "Foo");
                assert_eq!(declarations[0]["line"], 1);
                assert_eq!(declarations[0]["column"], 7);
                let references = result["references"].as_array().expect("references");
                // Row contract (crates/dartq-schema/src/query.rs:76-92, frozen
                // v1 #361): `target_file` is the referenced declaration's file;
                // the additive `file` is the row's use-site file, emitted by
                // the query producers since the freeze.
                let mut uses: Vec<(String, &str, i64)> = references
                    .iter()
                    .map(|row| {
                        assert_eq!(
                            row["target_file"],
                            a.display().to_string(),
                            "target_file names the declaring file: {row}"
                        );
                        (
                            row["file"].as_str().expect("use-site file").to_owned(),
                            row["kind"].as_str().expect("kind"),
                            row["offset"].as_i64().expect("offset"),
                        )
                    })
                    .collect();
                uses.sort();
                let mut expected: Vec<(String, &str, i64)> = vec![
                    (b.display().to_string(), "reference", 33),
                    (b.display().to_string(), "invoked", 42),
                    (a_test.display().to_string(), "reference", 49),
                    (a_test.display().to_string(), "invoked", 58),
                ];
                expected.sort();
                assert_eq!(
                    uses, expected,
                    "one annotation-plus-invocation pair per using file"
                );
                assert_schema_valid(&result, dartq_schema::Domain::Query);
            },
        ),
        // -- agent A stages its rewrite of lib/b.dart --
        step(
            request(
                3,
                "dartq/overlay.set",
                json!({"agentId": "A", "files": {"lib/b.dart": AGENT_B_SOURCE}}),
            ),
            1,
            |h| {
                assert_eq!(
                    answer_result(h, 3),
                    json!({"schema_version": 1}),
                    "the staging ack carries the contract version"
                );
            },
        ),
        // -- the agent view analyzes its own overlay, not the base --
        step(
            request(4, "dartq/analyze", json!({"agentId": "A"})),
            1,
            |h| {
                let result = answer_result(h, 4);
                assert_eq!(result["schema_version"], 1);
                assert!(
                    result["diagnostics"]
                        .as_array()
                        .expect("diagnostics")
                        .is_empty(),
                    "agent A's view is clean: {result}"
                );
            },
        ),
        // -- references under the overlay: A's extra use pair shows --
        step(
            request(
                5,
                "dartq/query",
                json!({"name": "Foo", "kind": "references", "agentId": "A"}),
            ),
            1,
            |h| {
                let result = answer_result(h, 5);
                let references = result["references"].as_array().expect("references");
                assert_eq!(
                    references.len(),
                    6,
                    "the overlay's extra use pair shows: {references:?}"
                );
                let mut uses: Vec<(String, &str, i64)> = references
                    .iter()
                    .map(|row| {
                        assert_eq!(
                            row["target_file"],
                            a.display().to_string(),
                            "target_file names the declaring file: {row}"
                        );
                        (
                            row["file"].as_str().expect("use-site file").to_owned(),
                            row["kind"].as_str().expect("kind"),
                            row["offset"].as_i64().expect("offset"),
                        )
                    })
                    .collect();
                uses.sort();
                let mut expected: Vec<(String, &str, i64)> = vec![
                    (b.display().to_string(), "reference", 33),
                    (b.display().to_string(), "invoked", 42),
                    (b.display().to_string(), "reference", 64),
                    (b.display().to_string(), "invoked", 73),
                    (a_test.display().to_string(), "reference", 49),
                    (a_test.display().to_string(), "invoked", 58),
                ];
                expected.sort();
                assert_eq!(uses, expected, "the overlay pair lands at 64/73");
                assert_schema_valid(&result, dartq_schema::Domain::Query);
                assert_schema_valid(&result, dartq_schema::Domain::Query);
            },
        ),
        // -- the merge: the staged buffer reaches the worktree and the base
        // view; both sides stay clean, so the publish diff pushes nothing --
        step(
            request(6, "dartq/overlay.commit", json!({"agentId": "A"})),
            1,
            |h| {
                let result = answer_result(h, 6);
                assert_eq!(result["schema_version"], 1);
                assert_eq!(result["committed"], 1, "one file merged");
                assert_eq!(
                    fs::read(&b).expect("read back"),
                    AGENT_B_SOURCE.as_bytes(),
                    "the staged buffer reached the worktree"
                );
                assert!(
                    publishes(h).is_empty(),
                    "a no-diff merge pushes nothing: {:?}",
                    publishes(h)
                );
            },
        ),
        // -- delta mode: only the session-written files' rows survive
        // (b.dart at cursor 3, main.dart opened at 1; the test file was
        // never written through this session) --
        step(
            request(7, "dartq/query", json!({"name": "main", "since": 0})),
            1,
            |h| {
                let result = answer_result(h, 7);
                assert_eq!(
                    result["since"], 3,
                    "the cursor echoes (didOpen 1 + overlay.set 2 + commit merge 3)"
                );
                assert_eq!(
                    declaration_files(&result),
                    [b.display().to_string(), main.display().to_string()],
                    "only files written through this session"
                );
            },
        ),
        // -- the client's full-sync edit: clean → the URI error, and the
        // publish diff arrives for the open document --
        step(
            notification(
                "textDocument/didChange",
                json!({"textDocument": {"uri": main_uri, "version": 2},
                       "contentChanges": [{"text": BROKEN_MAIN}]}),
            ),
            1,
            |h| {
                let pushed = publishes(h);
                assert_eq!(pushed.len(), 1, "the diff arrives: {pushed:?}");
                let params = &pushed[0]["params"];
                assert_eq!(params["uri"], main_uri);
                assert_eq!(params["version"], 2, "the open document's version rides");
                let diagnostics = params["diagnostics"].as_array().expect("diagnostics");
                assert_eq!(diagnostics.len(), 1);
                assert_eq!(diagnostics[0]["code"], "uri_does_not_exist");
                assert_eq!(diagnostics[0]["severity"], 1);
            },
        ),
        // -- the full picture first: every `main` declaration --
        step(
            request(8, "dartq/query", json!({ "name": "main" })),
            1,
            |h| {
                let result = answer_result(h, 8);
                assert_eq!(
                    declaration_files(&result),
                    [
                        b.display().to_string(),
                        main.display().to_string(),
                        a_test.display().to_string(),
                    ],
                    "every declaration, canonical path order"
                );
                assert!(
                    result.get("since").is_none(),
                    "no delta echo without since: {result}"
                );
            },
        ),
        // -- the second query, since the commit cursor: only the file that
        // changed after it (the didChange drain) keeps its rows --
        step(
            request(9, "dartq/query", json!({"name": "main", "since": 3})),
            1,
            |h| {
                let result = answer_result(h, 9);
                assert_eq!(
                    declaration_files(&result),
                    [main.display().to_string()],
                    "only the delta rows survive"
                );
                assert!(
                    result["references"]
                        .as_array()
                        .expect("references")
                        .is_empty(),
                    "no references to main anywhere"
                );
                assert_eq!(
                    result["since"], 4,
                    "the cursor echoes (the didChange drain)"
                );
                assert_schema_valid(&result, dartq_schema::Domain::Query);
            },
        ),
        // -- impact on the changed symbol: agent A rewrote b.dart's main --
        step(
            request(10, "dartq/impact", json!({ "path": "lib/b.dart" })),
            1,
            |h| {
                let result = answer_result(h, 10);
                assert_eq!(result["schema_version"], 1);
                let changed: Vec<String> = result["changed"]
                    .as_array()
                    .expect("changed")
                    .iter()
                    .map(|row| row["symbol"].as_str().expect("symbol").to_owned())
                    .collect();
                assert_eq!(
                    changed,
                    [format!("{}|main", b.display())],
                    "the target library's face names"
                );
                assert!(
                    result["affected"].as_array().expect("affected").is_empty(),
                    "nothing imports b.dart"
                );
                assert!(
                    result["affected_tests"]
                        .as_array()
                        .expect("affected_tests")
                        .is_empty()
                );
                assert_eq!(
                    result["affected_environments"],
                    json!([{ "label": "default", "compatible": true }]),
                );
            },
        ),
        // -- explain at the full tier: the session short-ids are assigned on
        // first payable use — the repeat paths (b.dart, then the test file)
        // become s1/s2, announced by the symbols table, and every
        // path-valued row back-references them --
        step(
            request(
                11,
                "dartq/explain",
                json!({"symbol": "Foo", "verbosity": "full"}),
            ),
            1,
            |h| {
                let result = answer_result(h, 11);
                assert_eq!(result["schema_version"], 1);
                assert_eq!(result["symbol"], "Foo");
                assert_eq!(
                    result["declaration"]["file"],
                    a.display().to_string(),
                    "the single-occurrence declaration stays spelled out"
                );
                let symbols = result["symbols"].as_object().expect("symbols preamble");
                assert_eq!(symbols.len(), 2, "both repeat paths announced: {symbols:?}");
                assert_eq!(
                    symbols["s1"],
                    b.display().to_string(),
                    "first-seen path (b.dart) maps to s1"
                );
                assert_eq!(symbols["s2"], a_test.display().to_string());
                let references = result["references"].as_array().expect("references");
                assert_eq!(
                    references.len(),
                    6,
                    "the merged library's two pairs plus the test file's pair"
                );
                for row in references {
                    let file = row["file"].as_str().expect("file");
                    assert!(
                        file == "s1" || file == "s2",
                        "rows back-reference the ids: {row}"
                    );
                }
                let dependencies = result["dependencies"].as_array().expect("dependencies");
                assert_eq!(
                    dependencies
                        .iter()
                        .map(|row| row["path"].as_str().expect("path"))
                        .collect::<Vec<_>>(),
                    ["s1", "s2"],
                    "dependency rows carry the ids too"
                );
                assert_schema_valid(&result, dartq_schema::Domain::Explain);
            },
        ),
        // -- the repeat path arrives as the s1 back-reference: announced ids
        // are never re-announced, and the rows keep referencing them --
        step(
            request(
                12,
                "dartq/explain",
                json!({"symbol": "Foo", "verbosity": "full"}),
            ),
            1,
            |h| {
                let result = answer_result(h, 12);
                assert!(
                    result.get("symbols").is_none(),
                    "an announced id is never re-announced: {result}"
                );
                let references = result["references"].as_array().expect("references");
                assert_eq!(references.len(), 6);
                assert!(
                    references.iter().any(|row| row["file"] == "s1"),
                    "the repeat path arrives as the s1 back-reference: {references:?}"
                );
                assert_eq!(
                    result["dependencies"][0]["path"], "s1",
                    "the id mapping is session-stable"
                );
                assert_eq!(
                    result["declaration"]["file"],
                    a.display().to_string(),
                    "the declaration stays spelled out"
                );
                assert_schema_valid(&result, dartq_schema::Domain::Explain);
            },
        ),
        // -- shutdown answers, then exit is the clean end --
        step(request(13, "shutdown", json!(null)), 1, |h| {
            assert!(
                answer_result(h, 13).is_null(),
                "shutdown answers a null result"
            );
        }),
    ];
    run_script(&mut h, &script);

    let (code, transcript) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
    let pushed: Vec<&Value> = transcript
        .iter()
        .filter(|frame| frame["method"] == "textDocument/publishDiagnostics")
        .collect();
    assert_eq!(
        pushed.len(),
        1,
        "exactly the didChange diff ever published: {pushed:?}"
    );

    fs::remove_dir_all(&workspace.root).ok();
}
