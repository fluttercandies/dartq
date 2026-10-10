//! End-to-end `dartq/*` RPC tests over the in-memory-pipe harness: each
//! face answers on a fixture workspace and its result is pinned against the
//! CLI face it mirrors (PRD §15.1's equivalence rule — the two share one
//! implementation in `dartq-commands`, so the comparison runs the command
//! core directly and renders it through the CLI's oracle-exact writers).
//! Also covers cancellation through the epoch machinery, the verbosity red
//! line, and the `schema_version` stamp. No network, no dart SDK.

mod common;

use std::fs;
use std::sync::Mutex;

use common::{BROKEN_MAIN, CLEAN_MAIN, Workspace, notification, request, spawn, store_root};
use dartq_commands::discover::discover;
use serde_json::{Value, json};

/// `Workspace::new` pins one pid-derived temp root; the tests in this
/// binary serialize on this lock so a parallel test's workspace re-create
/// can never yank another test's workspace mid-session.
static WORKSPACE: Mutex<()> = Mutex::new(());

/// `initialize` + `initialized`: the sweep publishes `publishes` frames
/// (one on a fixture with diagnostics, zero on a clean one).
fn handshake(h: &mut common::Harness, publishes: usize) {
    h.step(&request(1, "initialize", json!({})), 1);
    h.step(&notification("initialized", json!({})), publishes);
}

/// The `result` of the answer frame for `id` (owned: the transcript clone
/// dies here on purpose).
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

/// Renders `diagnostics` through the CLI's machine writer and parses the
/// lines back into the RPC check row shape (code lowercased to dartq's
/// stable snake_case name; severity/type stay the machine spellings).
fn machine_face_rows(
    diagnostics: &[dartq_diagnostics::Diagnostic],
    files: &dyn dartq_diagnostics::FileLayoutProvider,
) -> Vec<Value> {
    let mut machine = Vec::new();
    dartq_diagnostics::write_machine(
        &mut machine,
        &dartq_diagnostics::AnalysisOutput {
            diagnostics,
            files,
        },
    )
    .expect("machine write");
    String::from_utf8(machine)
        .expect("machine output is UTF-8")
        .lines()
        .map(|line| {
            let fields: Vec<&str> = line.splitn(8, '|').collect();
            assert_eq!(fields.len(), 8, "machine line: {line}");
            json!({
                "severity": fields[0],
                "type": fields[1],
                "code": fields[2].to_lowercase(),
                "file": fields[3],
                "line": fields[4].parse::<u32>().expect("line"),
                "column": fields[5].parse::<u32>().expect("column"),
                "length": fields[6].parse::<u32>().expect("length"),
                "message": fields[7],
            })
        })
        .collect()
}

/// The seeded workspace of the query face: a lib pair with a class and a
/// cross-file use pair (clean under the stub SDK, so the sweep publishes
/// nothing).
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

/// dartq/check vs the CLI machine face: the RPC rows equal the core
/// diagnostics rendered through the oracle-exact writer, field for field,
/// under the frozen `schema_version` stamp.
#[test]
fn dartq_check_matches_the_cli_machine_face() {
    let _guard = WORKSPACE.lock().expect("workspace lock");
    let workspace = Workspace::new(BROKEN_MAIN);
    let mut h = spawn(&workspace, &store_root("rpc-check"));
    handshake(&mut h, 1);

    h.step(
        &request(2, "dartq/check", json!({ "paths": [workspace.path("lib")] })),
        1,
    );
    let result = answer_result(&h, 2);
    assert_eq!(result["schema_version"], 1);

    // The CLI face: the same core through the machine writer.
    let files = discover(&[workspace.path("lib")]).expect("discover");
    let run = dartq_commands::check::analyze_files(&files).expect("check core");
    let output = run.output();
    let expected = machine_face_rows(output.diagnostics, output.files);
    assert!(!expected.is_empty(), "the broken fixture carries diagnostics");
    assert_eq!(
        result["diagnostics"],
        json!(expected),
        "the RPC rows equal the CLI machine rows"
    );

    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// dartq/analyze vs the CLI machine face: one RPC entry per machine line,
/// its URI the encoded file path and its diagnostic fields identical.
#[test]
fn dartq_analyze_matches_the_cli_machine_face() {
    let _guard = WORKSPACE.lock().expect("workspace lock");
    let workspace = Workspace::new(BROKEN_MAIN);
    let mut h = spawn(&workspace, &store_root("rpc-analyze"));
    handshake(&mut h, 1);

    h.step(&request(2, "dartq/analyze", json!({})), 1);
    let result = answer_result(&h, 2);
    assert_eq!(result["schema_version"], 1);
    let entries = result["diagnostics"].as_array().expect("entries");
    assert!(!entries.is_empty(), "the broken fixture has diagnostics");

    // The CLI face: a fresh engine over the same workspace.
    let mut engine = dartq_engine::Engine::new(workspace.engine_config()).expect("engine");
    let run = dartq_commands::analyze::run(&mut engine).expect("analyze core");
    let expected = machine_face_rows(&run.diagnostics, &run.layout);
    assert_eq!(entries.len(), expected.len(), "one entry per machine row");
    for (entry, row) in entries.iter().zip(&expected) {
        let path = std::path::Path::new(row["file"].as_str().expect("file"));
        assert_eq!(entry["uri"], dartq_daemon::uri::from_path(path));
        assert_eq!(entry["code"], row["code"]);
        assert_eq!(entry["message"], row["message"]);
        assert_eq!(
            entry["range"]["start"]["line"],
            row["line"].as_u64().expect("line") - 1,
            "LSP positions are zero-based"
        );
        assert_eq!(
            entry["range"]["start"]["character"],
            row["column"].as_u64().expect("column") - 1
        );
        assert_eq!(entry["severity"], 1, "the fixture row is an error");
    }

    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// dartq/query vs the CLI JSON face: the compact answer IS the shared
/// shaper's object — pinned here field for field with the same numbers the
/// CLI's own end-to-end test pins.
#[test]
fn dartq_query_equals_the_cli_json_face() {
    let _guard = WORKSPACE.lock().expect("workspace lock");
    let workspace = seeded_query_workspace();
    let a = workspace.path("lib/a.dart");
    let mut h = spawn(&workspace, &store_root("rpc-query"));
    handshake(&mut h, 0);

    h.step(&request(2, "dartq/query", json!({ "name": "Foo" })), 1);
    let result = answer_result(&h, 2);
    assert_eq!(result["schema_version"], 1);
    assert_eq!(result["name"], "Foo");

    let declarations = result["declarations"].as_array().expect("declarations");
    assert_eq!(declarations.len(), 1);
    assert_eq!(declarations[0]["file"], a.display().to_string());
    assert_eq!(declarations[0]["kind"], "class");
    assert_eq!(declarations[0]["name"], "Foo");
    assert!(declarations[0]["container"].is_null());
    assert_eq!(declarations[0]["line"], 1);
    assert_eq!(declarations[0]["column"], 7);
    assert_eq!(declarations[0]["length"], 3);

    let references = result["references"].as_array().expect("references");
    assert_eq!(references.len(), 2, "the type use plus the invocation");
    assert_eq!(references[0]["target_file"], a.display().to_string());
    assert_eq!(references[0]["kind"], "reference");
    assert_eq!(references[0]["qualified"], false);
    assert_eq!(references[0]["offset"], 33);
    assert_eq!(references[1]["kind"], "invoked");
    assert_eq!(references[1]["offset"], 42);

    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// dartq/impact vs the CLI report: the tier-invariant report serializes
/// identically on both faces — pinned with the same assertions the CLI's
/// impact test makes.
#[test]
fn dartq_impact_matches_the_cli_report() {
    let _guard = WORKSPACE.lock().expect("workspace lock");
    let workspace = seeded_query_workspace();
    // The test imports the lib through its package URI only: a clean
    // workspace keeps the sweep publish-free (a deterministic handshake).
    fs::write(
        workspace.path("test/a_test.dart"),
        "import 'package:fixture/a.dart';\nvoid main() {\n  foo();\n}\n",
    )
    .expect("write the test");
    let a = workspace.path("lib/a.dart");
    let mut h = spawn(&workspace, &store_root("rpc-impact"));
    handshake(&mut h, 0);

    h.step(
        &request(2, "dartq/impact", json!({ "path": a.display().to_string() })),
        1,
    );
    let result = answer_result(&h, 2);
    assert_eq!(result["schema_version"], 1);

    let symbols: Vec<String> = result["changed"]
        .as_array()
        .expect("changed")
        .iter()
        .map(|row| row["symbol"].as_str().expect("symbol").to_string())
        .collect();
    assert_eq!(
        symbols,
        vec![format!("{}|Foo", a.display()), format!("{}|foo", a.display())],
        "every face name, ASCII-sorted"
    );

    let b = workspace.path("lib/b.dart");
    let test_path = workspace.path("test/a_test.dart");
    let affected: Vec<(String, String)> = result["affected"]
        .as_array()
        .expect("affected")
        .iter()
        .map(|row| {
            (
                row["path"].as_str().expect("path").to_string(),
                row["impact"].as_str().expect("impact").to_string(),
            )
        })
        .collect();
    assert_eq!(
        affected,
        vec![
            (b.display().to_string(), "semantic".to_string()),
            (test_path.display().to_string(), "semantic".to_string()),
        ]
    );
    let tests: Vec<String> = result["affected_tests"]
        .as_array()
        .expect("tests")
        .iter()
        .map(|row| row["path"].as_str().expect("path").to_string())
        .collect();
    assert_eq!(tests, vec![test_path.display().to_string()]);
    assert_eq!(
        result["affected_environments"],
        json!([{ "label": "default", "compatible": true }]),
    );

    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// The seeded workspace of the explain face: the dog import chain plus its
/// package-URI test — clean under the stub SDK, so the sweep publishes
/// nothing.
fn seeded_explain_workspace() -> Workspace {
    let workspace = Workspace::new(CLEAN_MAIN);
    fs::write(
        workspace.path("lib/animal.dart"),
        "class Animal {\n  void breathe() {}\n}\n",
    )
    .expect("write animal.dart");
    fs::write(
        workspace.path("lib/dog.dart"),
        "import 'animal.dart';\nclass Dog extends Animal {\n  void bark() {}\n}\n",
    )
    .expect("write dog.dart");
    fs::write(workspace.path("lib/main.dart"), "import 'dog.dart';\nvoid main() {\n  var d = Dog();\n  d.bark();\n}\n")
        .expect("write main.dart");
    fs::create_dir_all(workspace.path("test")).expect("test dir");
    fs::write(
        workspace.path("test/dog_test.dart"),
        "import 'package:fixture/dog.dart';\nvoid main() {\n  var d = Dog();\n}\n",
    )
    .expect("write dog_test.dart");
    workspace
}

/// dartq/explain: the §15.3 report over the fixture symbol, section for
/// section (the same content the CLI's report pins — the faces share one
/// implementation), the `since` delta filtering the use/import/test rows
/// while the declaration-site sections stay, and the minimal tier's
/// `counts` swap. An `agentId` is rejected: the face answers the disk view
/// (the `dartq/compat` posture — the report's harvest rides the semantic
/// fan-out, which the session engine runs over recomputed libraries only).
#[test]
fn dartq_explain_answers_the_report() {
    let _guard = WORKSPACE.lock().expect("workspace lock");
    let workspace = seeded_explain_workspace();
    let animal = workspace.path("lib/animal.dart");
    let dog = workspace.path("lib/dog.dart");
    let main = workspace.path("lib/main.dart");
    let dog_test = workspace.path("test/dog_test.dart");
    let dog_source = "import 'animal.dart';\nclass Dog extends Animal {\n  void bark() {}\n}\n";
    let dog_at = dog_source.find("Dog").expect("Dog in source") as u64;
    let bark_at = dog_source.find("bark").expect("bark in source") as u64;
    let mut h = spawn(&workspace, &store_root("rpc-explain"));
    handshake(&mut h, 0);

    h.step(
        &request(
            2,
            "dartq/explain",
            json!({ "symbol": "Dog", "sdk": workspace.path("sdk") }),
        ),
        0,
    );
    let result = answer_result(&h, 2);
    assert_eq!(result["schema_version"], 1);
    assert_eq!(result["symbol"], "Dog");
    assert_eq!(
        result["declaration"],
        json!({
            "file": dog.display().to_string(),
            "name": "Dog",
            "kind": "class",
            "range": [dog_at, dog_at + 3],
        })
    );
    assert_eq!(
        result["inheritance"],
        json!([{ "name": "Animal", "file": animal.display().to_string() }])
    );
    assert_eq!(
        result["methods"],
        json!([{ "name": "bark", "kind": "method", "range": [bark_at, bark_at + 4] }])
    );
    let references = result["references"].as_array().expect("references");
    assert_eq!(references.len(), 2, "the main use plus the test use");
    assert_eq!(references[0]["file"], main.display().to_string());
    assert_eq!(references[0]["kind"], "invoked");
    assert_eq!(references[1]["file"], dog_test.display().to_string());
    assert_eq!(
        result["dependencies"],
        json!([
            { "path": main.display().to_string() },
            { "path": dog_test.display().to_string() },
        ])
    );
    assert_eq!(
        result["affected_tests"],
        json!([{ "path": dog_test.display().to_string() }])
    );

    // Delta mode: nothing was written through this session, so the
    // use/import/test rows all filter out while the declaration-site
    // sections stay, and the answer echoes the current cursor.
    h.step(
        &request(
            3,
            "dartq/explain",
            json!({ "symbol": "Dog", "sdk": workspace.path("sdk"), "since": 0 }),
        ),
        0,
    );
    let result = answer_result(&h, 3);
    assert_eq!(result["since"], 0, "the cursor echoes");
    assert_eq!(
        result["declaration"]["name"],
        "Dog",
        "the red line stays in delta answers"
    );
    assert_eq!(result["inheritance"].as_array().expect("array").len(), 1);
    assert!(
        result["references"].as_array().expect("array").is_empty()
            && result["dependencies"].as_array().expect("array").is_empty()
            && result["affected_tests"].as_array().expect("array").is_empty(),
        "no file changed since the cursor: {result}"
    );

    // The minimal tier: declaration locators plus the counts object.
    h.step(
        &request(
            4,
            "dartq/explain",
            json!({
                "symbol": "Dog",
                "sdk": workspace.path("sdk"),
                "verbosity": "minimal",
            }),
        ),
        0,
    );
    let result = answer_result(&h, 4);
    assert_eq!(result["declaration"]["name"], "Dog", "the red line stays");
    assert!(result.get("references").is_none(), "arrays drop");
    assert_eq!(result["counts"]["references"], 2);
    assert_eq!(result["counts"]["affected_tests"], 1);

    // The view selector is rejected loudly, never answered as the disk view.
    h.step(
        &request(
            5,
            "dartq/explain",
            json!({ "symbol": "Dog", "agentId": "A" }),
        ),
        0,
    );
    let error = answer_error(&h, 5);
    assert_eq!(error["code"], -32602, "INVALID_PARAMS: {error}");
    assert!(
        error["message"]
            .as_str()
            .expect("message")
            .contains("agentId"),
        "the rejection names the field: {error}"
    );

    // The overlay-session suite's green lifecycle: shutdown answers, then
    // exit is the clean end (a bare `exit` is EXIT_WITHOUT_SHUTDOWN).
    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// dartq/explain intake validation: a missing or empty `symbol` answers
/// `INVALID_PARAMS` and the session stays healthy.
#[test]
fn dartq_explain_rejects_a_bad_symbol() {
    let _guard = WORKSPACE.lock().expect("workspace lock");
    let workspace = seeded_explain_workspace();
    let mut h = spawn(&workspace, &store_root("rpc-explain-usage"));
    handshake(&mut h, 0);

    for (id, params) in [
        (2, json!({ "symbol": "" })),
        (3, json!({})),
        (4, json!({ "symbol": 7 })),
    ] {
        h.step(&request(id, "dartq/explain", params), 0);
        let error = answer_error(&h, id);
        assert_eq!(error["code"], -32602, "INVALID_PARAMS for id {id}: {error}");
    }

    // The session is unharmed: the next request answers normally.
    h.step(
        &request(
            5,
            "dartq/explain",
            json!({ "symbol": "Dog", "sdk": workspace.path("sdk") }),
        ),
        0,
    );
    assert_eq!(answer_result(&h, 5)["symbol"], "Dog");

    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// dartq/compat vs the CLI matrix: per-SDK verdict rows in axis order,
/// incompatible axes as data under a clean exit — the CLI compat test's
/// fixture shape driven over RPC.
#[test]
fn dartq_compat_matches_the_cli_matrix() {
    let _guard = WORKSPACE.lock().expect("workspace lock");
    let workspace = Workspace::new(CLEAN_MAIN);
    fs::write(
        workspace.path("lib/a.dart"),
        "import 'dart:async';\nclass Foo {}\n",
    )
    .expect("write a.dart");
    // The async-carrying axis.
    fs::write(workspace.path("sdks/new/version"), "3.13.1\n").expect("version");
    fs::write(
        workspace.path("sdks/new/lib/libraries.json"),
        r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}, "async": {"uri": "async/async.dart"}}}}"#,
    )
    .expect("libraries.json");
    fs::write(
        workspace.path("sdks/new/lib/async/async.dart"),
        "library dart.async;\n",
    )
    .expect("async.dart");
    // The async-less axis.
    fs::write(workspace.path("sdks/old/version"), "3.5.0\n").expect("version");
    fs::write(
        workspace.path("sdks/old/lib/libraries.json"),
        r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#,
    )
    .expect("libraries.json");
    for sdk in ["new", "old"] {
        fs::write(
            workspace.path(&format!("sdks/{sdk}/lib/_internal/allowed_experiments.json")),
            r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
        )
        .expect("allowed_experiments.json");
    }

    let a = workspace.path("lib/a.dart");
    let mut h = spawn(&workspace, &store_root("rpc-compat"));
    // `lib/a.dart` imports `dart:async`, unresolvable under the session's
    // core-only SDK: the sweep publishes one push for it.
    handshake(&mut h, 1);
    h.step(
        &request(
            2,
            "dartq/compat",
            json!({
                "path": a.display().to_string(),
                "sdk_roots": [workspace.path("sdks/old"), workspace.path("sdks/new")],
            }),
        ),
        1,
    );
    let result = answer_result(&h, 2);
    assert_eq!(result["schema_version"], 1);
    assert_eq!(result["file"], a.display().to_string());

    let matrix = result["matrix"].as_array().expect("matrix");
    assert_eq!(matrix.len(), 2, "one row per axis, in request order");
    assert_eq!(matrix[0]["sdk"], "3.5.0");
    assert_eq!(matrix[0]["compatible"], false);
    assert_eq!(matrix[0]["first_failure"]["code"], "uri_does_not_exist");
    assert_eq!(matrix[0]["first_failure"]["file"], a.display().to_string());
    assert_eq!(matrix[0]["first_failure"]["line"], 1);
    assert_eq!(matrix[0]["first_failure"]["col"], 8, "the URI literal span");
    assert_eq!(matrix[1]["sdk"], "3.13.1");
    assert_eq!(matrix[1]["compatible"], true);
    assert!(matrix[1].get("first_failure").is_none());

    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// dartq/fix: the row source is not wired yet (the parser→AST bridge is
/// another ticket), so the schema-shaped document reports zero patches; a
/// missing target answers the usage tier over RPC.
#[test]
fn dartq_fix_reports_the_schema_shaped_empty_document() {
    let _guard = WORKSPACE.lock().expect("workspace lock");
    let workspace = Workspace::new(CLEAN_MAIN);
    let mut h = spawn(&workspace, &store_root("rpc-fix"));
    handshake(&mut h, 0);

    h.step(
        &request(2, "dartq/fix", json!({ "paths": [workspace.path("lib")] })),
        1,
    );
    let result = answer_result(&h, 2);
    assert_eq!(
        result,
        json!({ "schema_version": 1, "files": [] }),
        "zero patches, schema-stamped"
    );

    h.step(
        &request(3, "dartq/fix", json!({ "paths": [workspace.path("nope")] })),
        1,
    );
    let error = answer_error(&h, 3);
    assert_eq!(error["code"], -32602, "INVALID_PARAMS");
    assert!(
        error["message"]
            .as_str()
            .expect("message")
            .contains("Directory or file doesn't exist"),
        "the CLI's usage message body: {error}"
    );

    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// Cancellation: a `dartq/check` request cancelled in the same batch (a
/// stale epoch at the drain) answers `RequestCancelled` with no result —
/// the same machinery `dartq/analyze` rides.
#[test]
fn cancel_of_a_queued_dartq_check_answers_request_cancelled() {
    let _guard = WORKSPACE.lock().expect("workspace lock");
    let workspace = Workspace::new(BROKEN_MAIN);
    let mut h = spawn(&workspace, &store_root("rpc-cancel"));
    handshake(&mut h, 1);

    h.burst(
        &[
            request(
                2,
                "dartq/check",
                json!({ "paths": [workspace.path("lib")] }),
            ),
            notification("$/cancelRequest", json!({ "id": 2 })),
        ],
        1,
    );
    let transcript = h.transcript();
    let answer = transcript
        .iter()
        .find(|frame| frame.get("id") == Some(&json!(2)))
        .expect("the cancelled request answers");
    assert_eq!(answer["error"]["code"], -32800, "RequestCancelled: {answer}");
    assert!(answer.get("result").is_none(), "no result rides the cancel");

    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// The verbosity red line (dartq-schema "Verbosity tiers"): minimal keeps
/// code + location and drops the prose fields, on both diagnostics faces;
/// compact (the default) keeps the full rows.
#[test]
fn dartq_verbosity_minimal_keeps_the_red_line_fields() {
    let _guard = WORKSPACE.lock().expect("workspace lock");
    let workspace = Workspace::new(BROKEN_MAIN);
    let mut h = spawn(&workspace, &store_root("rpc-verbosity"));
    handshake(&mut h, 1);

    h.step(
        &request(2, "dartq/analyze", json!({ "verbosity": "minimal" })),
        1,
    );
    let result = answer_result(&h, 2);
    assert_eq!(result["schema_version"], 1);
    let entries = result["diagnostics"].as_array().expect("entries");
    assert!(!entries.is_empty());
    let entry = &entries[0];
    assert!(entry.get("uri").is_some(), "location stays");
    assert!(entry.get("code").is_some(), "code stays");
    assert!(entry.get("range").is_some(), "range stays");
    assert!(entry.get("severity").is_none(), "prose drops");
    assert!(entry.get("message").is_none(), "prose drops");
    assert!(entry.get("source").is_none(), "prose drops");
    assert_eq!(entry["code"], "uri_does_not_exist");

    h.step(
        &request(
            3,
            "dartq/check",
            json!({ "paths": [workspace.path("lib")], "verbosity": "minimal" }),
        ),
        1,
    );
    let result = answer_result(&h, 3);
    let rows = result["diagnostics"].as_array().expect("rows");
    assert!(!rows.is_empty());
    let row = &rows[0];
    assert!(row.get("file").is_some(), "location stays");
    assert!(row.get("code").is_some(), "code stays");
    assert!(row.get("line").is_some(), "location stays");
    assert!(row.get("column").is_some(), "location stays");
    assert!(row.get("length").is_some(), "location stays");
    assert!(row.get("message").is_none(), "prose drops");
    assert!(row.get("severity").is_none(), "prose drops");

    h.step(&request(4, "dartq/analyze", json!({})), 1);
    let result = answer_result(&h, 4);
    let entry = &result["diagnostics"].as_array().expect("entries")[0];
    assert!(entry.get("severity").is_some(), "compact keeps severity");
    assert!(entry.get("message").is_some(), "compact keeps the message");

    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// An unknown verbosity spelling is an `INVALID_PARAMS` answer, and the
/// session stays healthy afterwards.
#[test]
fn dartq_invalid_verbosity_answers_invalid_params() {
    let _guard = WORKSPACE.lock().expect("workspace lock");
    let workspace = Workspace::new(CLEAN_MAIN);
    let mut h = spawn(&workspace, &store_root("rpc-invalid-verbosity"));
    handshake(&mut h, 0);

    h.step(
        &request(
            2,
            "dartq/check",
            json!({ "paths": [workspace.path("lib")], "verbosity": "loud" }),
        ),
        1,
    );
    let error = answer_error(&h, 2);
    assert_eq!(error["code"], -32602, "INVALID_PARAMS: {error}");

    // The session is unharmed: the next request answers normally.
    h.step(&request(3, "dartq/analyze", json!({})), 1);
    let answer = answer_result(&h, 3);
    assert_eq!(answer["schema_version"], 1);

    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}
