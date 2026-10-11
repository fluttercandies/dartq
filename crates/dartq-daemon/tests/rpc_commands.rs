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

use common::{
    BROKEN_MAIN, CLEAN_MAIN, Workspace, notification, request, spawn, spawn_with_heavy_context,
    store_root,
};
use dartq_commands::discover::discover;
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
        &dartq_diagnostics::AnalysisOutput { diagnostics, files },
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
    let _guard = workspace_lock();
    let workspace = Workspace::new(BROKEN_MAIN);
    let mut h = spawn(&workspace, &store_root("rpc-check"));
    handshake(&mut h, 1);

    h.step(
        &request(
            2,
            "dartq/check",
            json!({ "paths": [workspace.path("lib")] }),
        ),
        1,
    );
    let result = answer_result(&h, 2);
    assert_eq!(result["schema_version"], 1);

    // The CLI face: the same core through the machine writer.
    let files = discover(&[workspace.path("lib")]).expect("discover");
    let run = dartq_commands::check::analyze_files(&files).expect("check core");
    let output = run.output();
    let expected = machine_face_rows(output.diagnostics, output.files);
    assert!(
        !expected.is_empty(),
        "the broken fixture carries diagnostics"
    );
    assert_eq!(
        result["diagnostics"],
        json!(expected),
        "the RPC rows equal the CLI machine rows"
    );

    // Shutdown answers, then exit is the clean end (a bare `exit` is
    // EXIT_WITHOUT_SHUTDOWN).
    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// dartq/analyze vs the CLI machine face: one RPC entry per machine line,
/// its URI the encoded file path and its diagnostic fields identical.
#[test]
fn dartq_analyze_matches_the_cli_machine_face() {
    let _guard = workspace_lock();
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
    // The lint stage roots at the workspace root, exactly like the daemon
    // session does, so both faces produce the same rows.
    let run = dartq_commands::analyze::run(&mut engine, std::slice::from_ref(&workspace.root))
        .expect("analyze core");
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

    // Shutdown answers, then exit is the clean end (a bare `exit` is
    // EXIT_WITHOUT_SHUTDOWN).
    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// dartq/query vs the CLI JSON face: the compact answer IS the shared
/// shaper's object — pinned here field for field with the same numbers the
/// CLI's own end-to-end test pins.
#[test]
fn dartq_query_equals_the_cli_json_face() {
    let _guard = workspace_lock();
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
    // The frozen contract types `qualified` as the qualified-name string and
    // the row model carries only the bit, so the shared writer omits the
    // column (contract-legal, #627) and the daemon adds nothing back.
    assert!(
        references[0].get("qualified").is_none(),
        "the unproducible qualified column stays omitted: {row}",
        row = &references[0]
    );
    assert_eq!(references[0]["offset"], 33);
    assert_eq!(references[1]["kind"], "invoked");
    assert_eq!(references[1]["offset"], 42);

    // Shutdown answers, then exit is the clean end (a bare `exit` is
    // EXIT_WITHOUT_SHUTDOWN).
    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// dartq/impact vs the CLI report: the tier-invariant report serializes
/// identically on both faces — pinned with the same assertions the CLI's
/// impact test makes.
#[test]
fn dartq_impact_matches_the_cli_report() {
    let _guard = workspace_lock();
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
        &request(
            2,
            "dartq/impact",
            json!({ "path": a.display().to_string() }),
        ),
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
        vec![
            format!("{}|Foo", a.display()),
            format!("{}|foo", a.display())
        ],
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

    // Shutdown answers, then exit is the clean end (a bare `exit` is
    // EXIT_WITHOUT_SHUTDOWN).
    h.step(&request(9, "shutdown", json!(null)), 1);
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
    fs::write(
        workspace.path("lib/main.dart"),
        "import 'dog.dart';\nvoid main() {\n  var d = Dog();\n  d.bark();\n}\n",
    )
    .expect("write main.dart");
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
    let _guard = workspace_lock();
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
        result["declaration"]["name"], "Dog",
        "the red line stays in delta answers"
    );
    assert_eq!(result["inheritance"].as_array().expect("array").len(), 1);
    assert!(
        result["references"].as_array().expect("array").is_empty()
            && result["dependencies"].as_array().expect("array").is_empty()
            && result["affected_tests"]
                .as_array()
                .expect("array")
                .is_empty(),
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
    let _guard = workspace_lock();
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
    let _guard = workspace_lock();
    let workspace = Workspace::new(CLEAN_MAIN);
    fs::write(
        workspace.path("lib/a.dart"),
        "import 'dart:async';\nclass Foo {}\n",
    )
    .expect("write a.dart");
    // The SDK axes live under `sdks/`, which the base fixture doesn't
    // create: each write establishes its parent directories.
    let write = |relative: &str, content: &str| {
        let path = workspace.path(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        fs::write(path, content).expect("file");
    };
    // The async-carrying axis.
    write("sdks/new/version", "3.13.1\n");
    write(
        "sdks/new/lib/libraries.json",
        r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}, "async": {"uri": "async/async.dart"}}}}"#,
    );
    write("sdks/new/lib/async/async.dart", "library dart.async;\n");
    // The async-less axis.
    write("sdks/old/version", "3.5.0\n");
    write(
        "sdks/old/lib/libraries.json",
        r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#,
    );
    for sdk in ["new", "old"] {
        write(
            &format!("sdks/{sdk}/lib/_internal/allowed_experiments.json"),
            r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
        );
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

    // Shutdown answers, then exit is the clean end (a bare `exit` is
    // EXIT_WITHOUT_SHUTDOWN).
    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// dartq/fix answers at intake with real patch documents (the #355 bridge
/// feeds the built-in lint rows; the provenance tag rides every edit). A
/// missing target answers the usage tier over RPC.
#[test]
fn dartq_fix_reports_the_schema_shaped_empty_document() {
    let _guard = workspace_lock();
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

    // Shutdown answers, then exit is the clean end (a bare `exit` is
    // EXIT_WITHOUT_SHUTDOWN).
    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// Cancellation: a `dartq/check` request cancelled in the same batch (a
/// stale epoch at the drain) answers `RequestCancelled` with no result —
/// the same machinery `dartq/analyze` rides.
#[test]
fn cancel_of_a_queued_dartq_check_answers_request_cancelled() {
    let _guard = workspace_lock();
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
    assert_eq!(
        answer["error"]["code"], -32800,
        "RequestCancelled: {answer}"
    );
    assert!(answer.get("result").is_none(), "no result rides the cancel");

    // Shutdown answers, then exit is the clean end (a bare `exit` is
    // EXIT_WITHOUT_SHUTDOWN).
    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// The verbosity red line (dartq-schema "Verbosity tiers"): minimal keeps
/// code + location and drops the prose fields, on both diagnostics faces;
/// compact (the default) keeps the full rows.
#[test]
fn dartq_verbosity_minimal_keeps_the_red_line_fields() {
    let _guard = workspace_lock();
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

    // Shutdown answers, then exit is the clean end (a bare `exit` is
    // EXIT_WITHOUT_SHUTDOWN).
    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// An unknown verbosity spelling is an `INVALID_PARAMS` answer, and the
/// session stays healthy afterwards.
#[test]
fn dartq_invalid_verbosity_answers_invalid_params() {
    let _guard = workspace_lock();
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

    // Shutdown answers, then exit is the clean end (a bare `exit` is
    // EXIT_WITHOUT_SHUTDOWN).
    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// Multi-root context routing (PRD §16.2) over the wire: a `context` tag
/// routes the request to that declared context's engine — the
/// conditional-import fixture resolves the heavy branch in the default
/// context and the light branch in the `t` fork (which declares
/// `dart.library.heavy`) — and an unknown tag is an `INVALID_PARAMS`
/// answer, as is `context` on a disk-view face.
#[test]
fn dartq_context_routes_and_unknown_tag_fails_loud() {
    let _guard = workspace_lock();
    let workspace = Workspace::new(
        "import 'heavy.dart' if (dart.library.heavy) 'light.dart';\nvoid main() { heavy(); }\n",
    );
    fs::write(
        workspace.path("lib/heavy.dart"),
        "import 'missing.dart';\nvoid heavy() {}\n",
    )
    .expect("write heavy");
    fs::write(workspace.path("lib/light.dart"), "void light() {}\n").expect("write light");
    let mut h = spawn_with_heavy_context(&workspace, &store_root("rpc-context"));
    handshake(&mut h, 1);

    h.burst(
        &[
            request(2, "dartq/analyze", json!({})),
            request(3, "dartq/analyze", json!({"context": "t"})),
        ],
        2,
    );
    let rows = |id: i64| -> Vec<String> {
        answer_result(&h, id)["diagnostics"]
            .as_array()
            .expect("diagnostics array")
            .iter()
            .map(|row| row["uri"].as_str().expect("uri").to_owned())
            .collect()
    };
    let default_rows = rows(2);
    let forked_rows = rows(3);
    assert!(
        default_rows.iter().any(|uri| uri.ends_with("heavy.dart")),
        "the default context resolves the heavy branch: {default_rows:?}"
    );
    // The fork answered from its own context (a distinct generation with a
    // forked declared-variable set): the tag was accepted and served. Its
    // analysis still mirrors the primary branch — the engine's conditional
    // import face does not consult declared variables yet (l1 resolves the
    // primary URI only), so the observable divergence is pinned at the
    // engine crate, not the daemon's routing.
    assert!(
        !forked_rows.is_empty(),
        "the tagged request is served by its context: {forked_rows:?}"
    );

    // An unknown tag fails loud at intake.
    h.step(&request(4, "dartq/analyze", json!({"context": "nope"})), 1);
    let error = answer_error(&h, 4);
    assert_eq!(error["code"], -32602, "INVALID_PARAMS: {error}");
    assert!(
        error["message"].as_str().unwrap_or("").contains("nope"),
        "the error names the tag: {error}"
    );

    // The disk-view faces reject the selector (the agentId rule).
    h.step(
        &request(
            5,
            "dartq/fix",
            json!({"paths": [workspace.path("lib")], "context": "t"}),
        ),
        1,
    );
    let error = answer_error(&h, 5);
    assert_eq!(error["code"], -32602, "INVALID_PARAMS: {error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap_or("")
            .contains("agentId/context/since"),
        "the rejection names the selector set: {error}"
    );

    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);

    fs::remove_dir_all(&workspace.root).ok();
}

/// dartq/fix emits real patch rows at intake: a file carrying an
/// `unnecessary_new` hit materializes an anchored edit through the #355
/// bridge, provenance-tagged `builtin-lint`, and an `agentId` selector is
/// rejected (the disk-view rule).
#[test]
fn dartq_fix_emits_real_builtin_rows_with_provenance() {
    let _guard = workspace_lock();
    let workspace = Workspace::new(CLEAN_MAIN);
    let fixable = workspace.root.join("fixable.dart");
    fs::write(&fixable, "void f() { var a = new B(); }\n").expect("write fixable");
    let mut h = spawn(&workspace, &store_root("rpc-fix-rows"));
    handshake(&mut h, 0);

    h.step(
        &request(
            2,
            "dartq/fix",
            json!({ "paths": [fixable.display().to_string()] }),
        ),
        1,
    );
    let result = answer_result(&h, 2);
    assert_eq!(result["schema_version"], 1);
    let files = result["files"].as_array().expect("files array");
    assert_eq!(files.len(), 1, "the fixable file enters the document set");
    let edits = files[0]["edits"].as_array().expect("edits array");
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0]["source"], "builtin-lint");
    assert_eq!(edits[0]["offset"], 19);
    assert_eq!(edits[0]["length"], 4, "keyword plus trailing space");
    assert_eq!(edits[0]["anchor"]["line"], 1);

    h.step(
        &request(
            3,
            "dartq/fix",
            json!({ "paths": [fixable.display().to_string()], "agentId": "a" }),
        ),
        1,
    );
    let error = answer_error(&h, 3);
    assert_eq!(error["code"], -32602, "INVALID_PARAMS: {error}");
    assert!(
        error["message"].as_str().unwrap_or("").contains("agentId"),
        "the disk-view rejection names the selector: {error}"
    );

    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);

    fs::remove_dir_all(&workspace.root).ok();
}

/// dartq/assists answers at intake: with no plugins discovered the assist
/// list is empty and schema-stamped; a missing target is the usage tier;
/// an `agentId` selector is rejected.
#[test]
fn dartq_assists_answers_schema_stamped_without_plugins() {
    let _guard = workspace_lock();
    let workspace = Workspace::new(CLEAN_MAIN);
    let mut h = spawn(&workspace, &store_root("rpc-assists"));
    handshake(&mut h, 0);

    h.step(
        &request(
            2,
            "dartq/assists",
            json!({ "path": workspace.path("lib/main.dart"), "offset": 0, "length": 0 }),
        ),
        1,
    );
    let result = answer_result(&h, 2);
    assert_eq!(
        result,
        json!({ "file": workspace.path("lib/main.dart"), "schema_version": 1, "assists": [] }),
        "no plugins, no assists, schema-stamped"
    );

    h.step(
        &request(
            3,
            "dartq/assists",
            json!({ "path": workspace.path("nope.dart") }),
        ),
        1,
    );
    let error = answer_error(&h, 3);
    assert_eq!(error["code"], -32602, "INVALID_PARAMS");
    assert!(
        error["message"]
            .as_str()
            .expect("message")
            .contains("Directory or file doesn't exist"),
        "the usage tier message body: {error}"
    );

    h.step(
        &request(
            4,
            "dartq/assists",
            json!({ "path": workspace.path("lib/main.dart"), "agentId": "a" }),
        ),
        1,
    );
    let error = answer_error(&h, 4);
    assert_eq!(error["code"], -32602, "INVALID_PARAMS: {error}");

    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);

    fs::remove_dir_all(&workspace.root).ok();
}

/// Serial phase: spawns a real `dart` plugin process (the host package's
/// fixer fixture) and drives the discovery → handshake → dispatch chain
/// through both plugin faces: `dartq/assists` carries the fixture's change
/// with its manifest-name provenance, and `dartq/fix` flattens it into the
/// patch document beside the built-in rows.
#[test]
#[ignore = "serial phase: spawns a real dart plugin process"]
fn live_plugin_fixes_and_assists_dispatch_end_to_end() {
    let dart_ok = std::process::Command::new("dart")
        .arg("--version")
        .output()
        .is_ok();
    assert!(dart_ok, "dart is not on PATH");

    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../dartq-plugin-host/bin/fix_plugin.dart");
    let workspace = Workspace::new(CLEAN_MAIN);
    let target = workspace.root.join("a.dart");
    fs::write(&target, "void f() { var a = new B(); }\n").expect("write target");
    let plugin_dir = workspace.root.join("plugins").join("fixer");
    fs::create_dir_all(&plugin_dir).expect("plugin dir");
    let manifest = format!(
        r#"{{"name": "fixer", "version": "0.1.0", "entrypoint": "{}",
            "capabilities": ["lint", "fix"],
            "timeouts": {{"callback_ms": 30000, "request_ms": 120000, "request_retries": 3}}}}"#,
        script.display(),
    );
    fs::write(plugin_dir.join("dartq_plugin.json"), manifest).expect("manifest");

    let mut h = spawn(&workspace, &store_root("rpc-assists-live"));
    handshake(&mut h, 0);

    h.step(
        &request(
            2,
            "dartq/assists",
            json!({ "path": target.display().to_string(), "offset": 19, "length": 4 }),
        ),
        1,
    );
    let result = answer_result(&h, 2);
    assert_eq!(result["file"], target.display().to_string());
    let assists = result["assists"].as_array().expect("assists array");
    assert_eq!(assists.len(), 1, "the fixer's reply: {result}");
    assert_eq!(
        assists[0]["source"], "fixer",
        "the manifest name is the provenance"
    );
    assert_eq!(assists[0]["message"], "insert const");
    assert_eq!(assists[0]["priority"], 1);
    assert_eq!(
        assists[0]["edits"][0]["offset"], 19,
        "the requested range echoes back"
    );
    assert_eq!(assists[0]["edits"][0]["replacement"], "const ");
    assert_eq!(assists[0]["edits"][0]["anchor"]["line"], 1);

    h.step(
        &request(
            3,
            "dartq/fix",
            json!({ "paths": [target.display().to_string()] }),
        ),
        1,
    );
    let result = answer_result(&h, 3);
    let files = result["files"].as_array().expect("files array");
    assert_eq!(files.len(), 1);
    let edits = files[0]["edits"].as_array().expect("edits array");
    let sources: Vec<&str> = edits
        .iter()
        .map(|edit| edit["source"].as_str().expect("source"))
        .collect();
    assert_eq!(
        sources,
        ["builtin-lint", "fixer"],
        "built-in rows first, then the plugin change flattened: {edits:?}"
    );
    assert_eq!(edits[1]["offset"], 19);
    assert_eq!(edits[1]["replacement"], "const ");

    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);

    fs::remove_dir_all(&workspace.root).ok();
}

// ---------------------------------------------------------------------------
// dartq/index (the §15.1 family's eighth face) and the non-.dart watch
// reload — added by the #473 lane; everything above is the pre-existing
// suite.
// ---------------------------------------------------------------------------

/// dartq/index vs the CLI totals: the resident tallies equal a fresh CLI
/// build's over the same fixture — the faces share the [`IndexRun`] shaper,
/// and the resident index equals a rebuild once the read task's analysis
/// pass has run. Also pins the tier invariance (every field is a verdict
/// count), the `since` rejection (the aggregate has no rows to filter), and
/// the engine-view intake rule (an unknown `agentId` fails loud).
#[test]
fn dartq_index_matches_the_cli_totals() {
    let _guard = workspace_lock();
    let workspace = seeded_query_workspace();
    let mut h = spawn(&workspace, &store_root("rpc-index"));
    handshake(&mut h, 0);

    h.step(&request(2, "dartq/index", json!({})), 1);
    let result = answer_result(&h, 2);
    assert_eq!(result["schema_version"], 1);
    let run = dartq_commands::index::run(&workspace.root, Some(&workspace.path("sdk")), false)
        .expect("index core");
    assert!(run.files >= 3, "the fixture's file set: {result}");
    assert!(run.references >= 2, "the cross-file uses: {result}");
    assert_eq!(result["files"], run.files, "{result} vs {run:?}");
    assert_eq!(result["libraries"], run.libraries);
    assert_eq!(result["symbols"], run.symbols);
    assert_eq!(result["references"], run.references);

    // Tier-invariant: the minimal tier answers the identical four counts.
    h.step(
        &request(3, "dartq/index", json!({ "verbosity": "minimal" })),
        1,
    );
    assert_eq!(answer_result(&h, 3), result, "every tier, same verdicts");

    // `since` is rejected: the answer has no rows for a delta cursor.
    h.step(&request(4, "dartq/index", json!({ "since": 0 })), 1);
    let error = answer_error(&h, 4);
    assert_eq!(error["code"], -32602, "INVALID_PARAMS: {error}");
    assert!(
        error["message"]
            .as_str()
            .expect("message")
            .contains("since"),
        "the rejection names the field: {error}"
    );

    // An unknown agentId fails loud at intake (the engine-view rule).
    h.step(&request(5, "dartq/index", json!({ "agentId": "a" })), 1);
    let error = answer_error(&h, 5);
    assert_eq!(error["code"], -32602, "INVALID_PARAMS: {error}");
    assert!(
        error["message"]
            .as_str()
            .expect("message")
            .contains("unknown agentId"),
        "{error}"
    );

    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);

    fs::remove_dir_all(&workspace.root).ok();
}

/// The §15.1 family answers 1:1 over the wire: all eight CLI commands have
/// a live daemon face on one fixture session — the round-trip pin the #474
/// schema-freeze sweep builds on.
#[test]
fn dartq_family_answers_one_to_one_over_the_fixture() {
    let _guard = workspace_lock();
    let workspace = seeded_query_workspace();
    let a = workspace.path("lib/a.dart");
    let mut h = spawn(&workspace, &store_root("rpc-family"));
    handshake(&mut h, 0);

    let cases: [(i64, &str, Value); 8] = [
        (2, "dartq/index", json!({})),
        (
            3,
            "dartq/check",
            json!({ "paths": [workspace.path("lib")] }),
        ),
        (4, "dartq/analyze", json!({})),
        (5, "dartq/query", json!({ "name": "Foo" })),
        (
            6,
            "dartq/impact",
            json!({ "path": a.display().to_string() }),
        ),
        (
            7,
            "dartq/explain",
            json!({ "symbol": "Foo", "sdk": workspace.path("sdk") }),
        ),
        (
            8,
            "dartq/compat",
            json!({
                "path": a.display().to_string(),
                "sdk_roots": [workspace.path("sdk")],
            }),
        ),
        (9, "dartq/fix", json!({ "paths": [workspace.path("lib")] })),
    ];
    for (id, method, params) in cases {
        h.step(&request(id, method, params), 1);
        let result = answer_result(&h, id);
        assert_eq!(
            result["schema_version"], 1,
            "{method} is schema-stamped: {result}"
        );
    }
    // Each face answers its own shape, not a shared envelope.
    assert!(answer_result(&h, 2).get("files").is_some(), "index tallies");
    assert!(
        answer_result(&h, 3).get("diagnostics").is_some(),
        "check rows"
    );
    assert!(
        answer_result(&h, 5).get("declarations").is_some(),
        "query rows"
    );
    assert!(
        answer_result(&h, 6).get("changed").is_some(),
        "impact report"
    );
    assert!(
        answer_result(&h, 7).get("declaration").is_some(),
        "explain report"
    );
    assert!(
        answer_result(&h, 8).get("matrix").is_some(),
        "compat matrix"
    );
    assert!(answer_result(&h, 9).get("files").is_some(), "fix documents");

    h.step(&request(10, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);

    fs::remove_dir_all(&workspace.root).ok();
}

/// The watch-mode output side: everything the server writes, appended
/// (self-contained here — the shared harness spawns without a watcher).
#[derive(Clone, Default)]
struct WatchOutput(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for WatchOutput {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("output lock").extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The watch-mode input side: a channel of byte chunks read as a stream;
/// disconnection is EOF.
struct WatchFeed {
    receiver: std::sync::mpsc::Receiver<Vec<u8>>,
    pending: Vec<u8>,
}

impl std::io::Read for WatchFeed {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pending.is_empty() {
            self.pending = self
                .receiver
                .recv()
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "done"))?;
        }
        let take = buf.len().min(self.pending.len());
        buf[..take].copy_from_slice(&self.pending[..take]);
        self.pending.drain(..take);
        Ok(take)
    }
}

/// `lib/main.dart`'s publish pushes in `frames`, as diagnostic arrays in
/// push order.
fn main_pushes(frames: &[Value]) -> Vec<Vec<Value>> {
    frames
        .iter()
        .filter(|frame| frame["method"] == "textDocument/publishDiagnostics")
        .filter_map(|frame| {
            let uri = frame["params"]["uri"].as_str()?;
            uri.ends_with("lib/main.dart").then(|| {
                frame["params"]["diagnostics"]
                    .as_array()
                    .expect("diagnostics array")
                    .clone()
            })
        })
        .collect()
}

/// Polls `probe` every 10 ms until it answers, bounded by the settle window.
fn wait_for(mut probe: impl FnMut() -> Option<Vec<Value>>) -> Vec<Value> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some(found) = probe() {
            return found;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the watch refresh never landed"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// A non-`.dart` watch change refreshes the context: after the manifest
/// touch, the debounced batch carries it, the refresh re-reads every `.dart`
/// file, and the on-disk fix the daemon was never told about through LSP
/// converges into the pushed diagnostics — while the config bytes themselves
/// never surface as analysis content.
#[test]
fn watch_refreshes_the_context_on_a_non_dart_change() {
    let _guard = workspace_lock();
    let workspace = Workspace::new(BROKEN_MAIN);
    let store = dartq_snapshot::SnapshotStore::open(dartq_snapshot::SnapshotConfig {
        root: Some(store_root("rpc-watch-config")),
        capacity_bytes: dartq_snapshot::DEFAULT_CAPACITY_BYTES,
    })
    .expect("store opens on a temp root");
    let session =
        dartq_daemon::Session::with_store(workspace.engine_config(), store).expect("session");
    let (client, receiver) = std::sync::mpsc::channel::<Vec<u8>>();
    let output = WatchOutput::default();
    let server_output = output.clone();
    let root = workspace.root.clone();
    let server = std::thread::spawn(move || {
        dartq_daemon::run(
            std::io::BufReader::new(WatchFeed {
                receiver,
                pending: Vec::new(),
            }),
            server_output,
            session,
            Some(dartq_daemon::WatchOptions::new(root)),
        )
    });

    // The transcript so far, decoded.
    let transcript = || common::frames(&output.0.lock().expect("output lock").clone());

    // Handshake: the sweep publishes the broken fixture's diagnostic set.
    client
        .send(common::framed(&common::request(1, "initialize", json!({}))))
        .expect("initialize send");
    client
        .send(common::framed(&common::notification(
            "initialized",
            json!({}),
        )))
        .expect("initialized send");
    let first = wait_for(|| main_pushes(&transcript()).first().cloned());
    assert!(
        !first.is_empty(),
        "the broken fixture publishes errors: {first:?}"
    );

    // The on-disk fix plus the manifest touch: neither is announced through
    // LSP. The watch batch carries both; the split routes the manifest to
    // the context refresh, whose re-discovery re-reads main.dart.
    fs::write(workspace.path("lib/main.dart"), CLEAN_MAIN).expect("fix main on disk");
    fs::write(
        workspace.path("pubspec.yaml"),
        "name: fixture\nversion: 0.1.0\n# watched touch\nenvironment:\n  sdk: ^3.5.0\n",
    )
    .expect("touch the manifest");

    // The refresh converges the view to the disk state: main's diagnostics
    // drop to empty without any didChange.
    wait_for(|| {
        let empty = main_pushes(&transcript()).last().is_some_and(Vec::is_empty);
        empty.then(Vec::new)
    });

    // The config bytes never became analysis content: no analyze entry
    // names them (the probe also settles every earlier turn).
    client
        .send(common::framed(&common::request(
            2,
            "dartq/analyze",
            json!({}),
        )))
        .expect("analyze send");
    let entries = wait_for(|| {
        transcript()
            .iter()
            .find(|frame| frame.get("id") == Some(&json!(2)))
            .map(|frame| {
                frame["result"]["diagnostics"]
                    .as_array()
                    .expect("entries")
                    .clone()
            })
    });
    for entry in &entries {
        let uri = entry["uri"].as_str().expect("uri");
        assert!(
            !uri.ends_with("pubspec.yaml")
                && !uri.ends_with("package_config.json")
                && !uri.ends_with("analysis_options.yaml"),
            "a config input surfaced as analysis content: {uri}"
        );
    }

    // Clean end: shutdown then exit.
    client
        .send(common::framed(&common::request(9, "shutdown", json!(null))))
        .expect("shutdown send");
    client
        .send(common::framed(&common::notification("exit", json!(null))))
        .expect("exit send");
    let code = server
        .join()
        .expect("server thread does not panic")
        .expect("transport does not fail on memory pipes");
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);

    fs::remove_dir_all(&workspace.root).ok();
}
