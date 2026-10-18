//! End-to-end `dartq/*` RPC tests over the in-memory-pipe harness: each
//! face answers on a fixture workspace and its result is pinned against the
//! CLI face it mirrors (equivalence rule — the two share one
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

/// The `result` of the answer frame for `id`. Waiting, not one-shot: the
/// plugin-dispatch faces (`dartq/fix`, `dartq/assists`) stage at intake,
/// emit their progress notice at the drain, and answer one worker hop
/// later, so the answer may land after the step's frame count settled.
fn answer_result(h: &common::Harness, id: i64) -> Value {
    h.wait_answer(id)["result"].clone()
}

/// The `error` of the answer frame for `id` (same wait contract).
fn answer_error(h: &common::Harness, id: i64) -> Value {
    h.wait_answer(id)["error"].clone()
}

/// Renders `diagnostics` through the CLI's machine writer and parses the
/// lines back into the RPC check row shape (code lowercased to dartq's
/// stable `snake_case` name; severity/type stay the machine spellings).
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

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// dartq/analyze vs the CLI machine face: one RPC entry per machine line,
/// its URI the encoded file path and its diagnostic fields identical.
#[test]
fn dartq_analyze_matches_the_cli_machine_face() {
    let _guard = workspace_lock();
    let workspace = Workspace::new(BROKEN_MAIN);
    let mut h = spawn(&workspace, &store_root("rpc-analyze"));

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
            "wire positions are zero-based"
        );
        assert_eq!(
            entry["range"]["start"]["character"],
            row["column"].as_u64().expect("column") - 1
        );
        assert_eq!(entry["severity"], 1, "the fixture row is an error");
    }

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// dartq/query vs the CLI JSON face: the compact answer IS the shared
/// shaper's object — pinned here field for field with the same numbers the
/// CLI's own end-to-end test pins.
#[test]
fn dartq_query_equals_the_cli_json_face() {
    let _guard = workspace_lock();
    let workspace = seeded_query_workspace();
    let mut h = spawn(&workspace, &store_root("rpc-query"));

    h.step(&request(2, "dartq/query", json!({ "name": "Foo" })), 1);
    let result = answer_result(&h, 2);
    assert_eq!(result["schema_version"], 1);
    assert_eq!(result["name"], "Foo");

    let declarations = result["declarations"].as_array().expect("declarations");
    assert_eq!(declarations.len(), 1);
    assert_eq!(
        declarations[0]["file"], "package:fixture/a.dart",
        "the stable package-form label (the shared shaper's projection)"
    );
    assert_eq!(declarations[0]["kind"], "class");
    assert_eq!(declarations[0]["name"], "Foo");
    assert!(
        declarations[0].get("container").is_none(),
        "a top-level row omits the container key (the omission rule): {row}",
        row = &declarations[0]
    );
    assert_eq!(declarations[0]["line"], 1);
    assert_eq!(declarations[0]["column"], 7);
    assert_eq!(declarations[0]["length"], 3);

    let references = result["references"].as_array().expect("references");
    assert_eq!(references.len(), 2, "the type use plus the invocation");
    assert_eq!(references[0]["target_file"], "package:fixture/a.dart");
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

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// dartq/resolveAt over the seeded workspace: the declaration arm, the
/// reference arm (target identity plus the target's own name span), the
/// null hit on an uncovered position, the `since` rejection (a point
/// answer has no rows for a cursor), and the fail-loud unknown file — the
/// wire contract over the engine substrate (`Engine::resolve_at` carries
/// the semantics; offsets are the engine's UTF-16 convention, the same
/// units the query rows' `offset` fields pin: 6 = `Foo`'s declaration, 33
/// its type use in b.dart).
#[test]
fn dartq_resolve_at_answers_the_position_shapes() {
    let _guard = workspace_lock();
    let workspace = seeded_query_workspace();
    let a = workspace.path("lib/a.dart");
    let b = workspace.path("lib/b.dart");
    let mut h = spawn(&workspace, &store_root("rpc-resolve-at"));

    // A declaration name token: the declaration itself is the answer.
    h.step(
        &request(2, "dartq/resolveAt", json!({ "file": a, "offset": 6 })),
        1,
    );
    let result = answer_result(&h, 2);
    assert_eq!(result["schema_version"], 1);
    assert_eq!(result["file"], a.display().to_string());
    assert_eq!(result["offset"], 6);
    let hit = &result["hit"];
    assert_eq!(hit["kind"], "declaration");
    assert_eq!(hit["name"], "Foo");
    assert_eq!(hit["decl_kind"], "class");
    assert!(
        hit.get("container").is_none(),
        "a top-level declaration omits the container key: {hit}",
        hit = hit,
    );
    assert_eq!(hit["file"], a.display().to_string());
    assert_eq!(hit["range"]["offset"], 6);
    assert_eq!(hit["range"]["length"], 3);

    // A resolved use: the target identity plus its name span.
    h.step(
        &request(3, "dartq/resolveAt", json!({ "file": b, "offset": 33 })),
        1,
    );
    let hit = &answer_result(&h, 3)["hit"];
    assert_eq!(hit["kind"], "reference");
    assert_eq!(hit["name"], "Foo");
    assert_eq!(hit["decl_kind"], "class");
    assert_eq!(hit["file"], a.display().to_string(), "the target's file");
    assert_eq!(hit["range"]["offset"], 33);
    assert_eq!(hit["target_range"]["offset"], 6);
    assert_eq!(hit["target_range"]["length"], 3);
    assert_eq!(hit["ref_kind"], "reference");
    assert_eq!(hit["qualified"], false);
    assert!(
        hit.get("prefix").is_none(),
        "a bare use omits the prefix key: {hit}",
        hit = hit,
    );

    // An uncovered position (the newline after `class Foo {`): a null hit,
    // not an error.
    h.step(
        &request(4, "dartq/resolveAt", json!({ "file": a, "offset": 11 })),
        1,
    );
    let result = answer_result(&h, 4);
    assert_eq!(
        result.get("hit"),
        Some(&Value::Null),
        "the verdict slot stays an explicit null, never an omitted key: {result}"
    );

    // A point answer rejects the delta cursor...
    h.step(
        &request(
            5,
            "dartq/resolveAt",
            json!({ "file": a, "offset": 6, "since": 0 }),
        ),
        1,
    );
    let error = answer_error(&h, 5);
    assert_eq!(error["code"], -32602, "INVALID_PARAMS: {error}");
    assert!(
        error["message"]
            .as_str()
            .expect("message")
            .contains("since"),
        "the rejection names the rule: {error}"
    );

    // ...and a file the analyzed view never interned fails loud (the
    // typo'd-path rule); the session is unharmed either way.
    h.step(
        &request(
            6,
            "dartq/resolveAt",
            json!({ "file": workspace.path("lib/absent.dart"), "offset": 0 }),
        ),
        1,
    );
    let error = answer_error(&h, 6);
    assert_eq!(error["code"], -32602, "INVALID_PARAMS: {error}");
    h.step(&request(7, "dartq/analyze", json!({})), 1);
    assert_eq!(answer_result(&h, 7)["schema_version"], 1);

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// dartq/completion over the seeded workspace: the visible-name set at a
/// cursor — the fragment filter over the text left of the offset (the
/// engine extracts it from the analyzed snapshot), the import-proven
/// reference rows carrying their target identity and span, the whole set
/// at a fresh-name cursor, the empty set for a fragment nothing matches
/// and for a past-end offset, the `since` rejection (a point answer has no
/// rows for a cursor), and the fail-loud unknown file — the wire contract
/// over the engine substrate (`Engine::complete_at` carries the
/// semantics; offsets are the engine's UTF-16 convention: in b.dart 53 is
/// after the `fo` of `foo`, 17 a fresh-name line start, 19 inside the
/// `void` keyword).
#[test]
fn dartq_completion_answers_the_visible_names() {
    let _guard = workspace_lock();
    let workspace = seeded_query_workspace();
    // A prefixed import (`a.Foo` / `a.foo`): the qualifier arms a
    // continuation candidate — the one origin that carries no declaration
    // identity.
    fs::write(
        workspace.path("lib/p.dart"),
        "import 'a.dart' as a;\nvoid main() {\n  a.Foo? f = a.foo();\n}\n",
    )
    .expect("write p.dart");
    let a = workspace.path("lib/a.dart");
    let b = workspace.path("lib/b.dart");
    let p = workspace.path("lib/p.dart");
    let mut h = spawn(&workspace, &store_root("rpc-completion"));

    // The fragment filter: `fo|` answers the import-proven `foo` only —
    // case-sensitive, so `Foo` stays out — with the target's declaring
    // file and name span (the completed pick can jump to it).
    h.step(
        &request(2, "dartq/completion", json!({ "file": b, "offset": 53 })),
        1,
    );
    let result = answer_result(&h, 2);
    assert_eq!(result["schema_version"], 1);
    assert_eq!(result["file"], b.display().to_string());
    assert_eq!(result["offset"], 53);
    assert_eq!(result["prefix"], "fo");
    let items = result["items"].as_array().expect("items");
    assert_eq!(items.len(), 1, "only `foo` extends `fo`: {items:?}");
    assert_eq!(items[0]["name"], "foo");
    assert_eq!(items[0]["origin"], "reference");
    assert_eq!(items[0]["decl_kind"], "function");
    assert!(
        items[0].get("container").is_none(),
        "a top-level target omits the container key (the omission rule): {item}",
        item = &items[0]
    );
    assert_eq!(
        items[0]["file"],
        a.display().to_string(),
        "the target's file"
    );
    assert_eq!(items[0]["range"]["offset"], 36);
    assert_eq!(items[0]["range"]["length"], 3);

    // A fresh-name cursor (no fragment): the whole visible set — the
    // import-proven `Foo` and `foo`, the file-local `main` — ascending by
    // name, each origin and identity honest.
    h.step(
        &request(3, "dartq/completion", json!({ "file": b, "offset": 17 })),
        1,
    );
    let result = answer_result(&h, 3);
    let items = result["items"].as_array().expect("items");
    let names: Vec<&str> = items
        .iter()
        .map(|item| item["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names, vec!["Foo", "foo", "main"], "{items:?}");
    let origins: Vec<&str> = items
        .iter()
        .map(|item| item["origin"].as_str().expect("origin"))
        .collect();
    assert_eq!(origins, vec!["reference", "reference", "declaration"]);
    assert_eq!(items[0]["decl_kind"], "class");
    assert_eq!(items[0]["file"], a.display().to_string());
    assert_eq!(items[0]["range"]["offset"], 6, "Foo's own name span");
    assert_eq!(items[2]["file"], b.display().to_string());
    assert_eq!(items[2]["range"]["offset"], 22, "main's own name span");

    // The armed qualifier in p.dart: `a` answers as a Prefix-origin item —
    // a wire word, not a declaration — beside the import-proven names.
    h.step(
        &request(4, "dartq/completion", json!({ "file": p, "offset": 22 })),
        1,
    );
    let result = answer_result(&h, 4);
    assert_eq!(result["prefix"], "", "a fresh-name cursor: the whole set");
    let items = result["items"].as_array().expect("items");
    let named: Vec<(&str, &str)> = items
        .iter()
        .map(|item| {
            (
                item["name"].as_str().expect("name"),
                item["origin"].as_str().expect("origin"),
            )
        })
        .collect();
    assert_eq!(
        named,
        vec![
            ("Foo", "reference"),
            ("a", "prefix"),
            ("foo", "reference"),
            ("main", "declaration"),
        ],
        "ascending by name, the qualifier arms a continuation: {items:?}"
    );
    let prefix_item = &items[1];
    assert!(
        prefix_item.get("decl_kind").is_none()
            && prefix_item.get("container").is_none()
            && prefix_item.get("file").is_none()
            && prefix_item.get("range").is_none(),
        "a prefix item carries no identity — every column omits, never a null: {item}",
        item = prefix_item
    );

    // A fragment nothing visible extends (`vo|` inside the keyword), and a
    // past-end offset: the empty set, never an error.
    h.step(
        &request(5, "dartq/completion", json!({ "file": b, "offset": 19 })),
        1,
    );
    let result = answer_result(&h, 5);
    assert_eq!(result["prefix"], "vo");
    assert!(result["items"].as_array().expect("items").is_empty());
    h.step(
        &request(6, "dartq/completion", json!({ "file": b, "offset": 9_999 })),
        1,
    );
    let result = answer_result(&h, 6);
    assert!(result["items"].as_array().expect("items").is_empty());

    // A point answer rejects the delta cursor...
    h.step(
        &request(
            7,
            "dartq/completion",
            json!({ "file": b, "offset": 53, "since": 0 }),
        ),
        1,
    );
    let error = answer_error(&h, 7);
    assert_eq!(error["code"], -32602, "INVALID_PARAMS: {error}");
    assert!(
        error["message"]
            .as_str()
            .expect("message")
            .contains("since"),
        "the rejection names the rule: {error}"
    );

    // ...and a file the analyzed view never interned fails loud (the
    // typo'd-path rule); the session is unharmed either way.
    h.step(
        &request(
            8,
            "dartq/completion",
            json!({ "file": workspace.path("lib/absent.dart"), "offset": 0 }),
        ),
        1,
    );
    let error = answer_error(&h, 8);
    assert_eq!(error["code"], -32602, "INVALID_PARAMS: {error}");
    h.step(&request(9, "dartq/analyze", json!({})), 1);
    assert_eq!(answer_result(&h, 9)["schema_version"], 1);

    let (code, _) = h.join_ended();
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

    h.step(
        &request(
            2,
            "dartq/impact",
            json!({ "path": a.display().to_string() }),
        ),
        1,
    );
    let result = answer_result(&h, 2);
    assert_eq!(result["schema_version"], 2);

    let symbols: Vec<String> = result["changed"]
        .as_array()
        .expect("changed")
        .iter()
        .map(|row| row["symbol"].as_str().expect("symbol").to_string())
        .collect();
    assert_eq!(
        symbols,
        vec![
            "package:fixture/a.dart|Foo".to_string(),
            "package:fixture/a.dart|foo".to_string()
        ],
        "every face name, ASCII-sorted (the shared pkg_label projection)"
    );

    let test_path = workspace
        .path("test/a_test.dart")
        .canonicalize()
        .expect("canonical test path");
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
            (test_path.display().to_string(), "semantic".to_string()),
            ("package:fixture/b.dart".to_string(), "semantic".to_string()),
        ],
        "the test tree keeps its canonical path and sorts first: `/` before `p`"
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

    let (code, _) = h.join_ended();
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

/// dartq/explain: the report over the fixture symbol, section for
/// section (the same content the CLI's report pins — the faces share one
/// implementation), the `since` delta filtering the use/import/test rows
/// while the declaration-site sections stay, and the minimal tier's
/// `counts` swap. An `agentId` is rejected: the face answers the disk view
/// (the `dartq/compat` posture — the report's harvest rides the semantic
/// fan-out, which the session engine runs over recomputed libraries only).
#[test]
#[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
fn dartq_explain_answers_the_report() {
    let _guard = workspace_lock();
    let workspace = seeded_explain_workspace();
    let dog_test = workspace
        .path("test/dog_test.dart")
        .canonicalize()
        .expect("canonical test path");
    let dog_source = "import 'animal.dart';\nclass Dog extends Animal {\n  void bark() {}\n}\n";
    let dog_at = dog_source.find("Dog").expect("Dog in source") as u64;
    let bark_at = dog_source.find("bark").expect("bark in source") as u64;
    let mut h = spawn(&workspace, &store_root("rpc-explain"));

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
            "file": "package:fixture/dog.dart",
            "name": "Dog",
            "kind": "class",
            "range": [dog_at, dog_at + 3],
        })
    );
    assert_eq!(
        result["inheritance"],
        json!([{
            "name": "Animal",
            "file": "package:fixture/animal.dart",
            "clause": "extends",
        }])
    );
    assert_eq!(
        result["methods"],
        json!([{ "name": "bark", "kind": "method", "range": [bark_at, bark_at + 4] }])
    );
    let references = result["references"].as_array().expect("references");
    assert_eq!(references.len(), 2, "the main use plus the test use");
    // Row order keys on the projected label (`package:fixture/main.dart`
    // vs the test tree's canonical path): `/` sorts before `p`.
    assert_eq!(references[0]["file"], dog_test.display().to_string());
    assert_eq!(references[0]["kind"], "invoked");
    assert_eq!(references[1]["file"], "package:fixture/main.dart");
    assert_eq!(
        result["dependencies"],
        json!([
            { "path": dog_test.display().to_string() },
            { "path": "package:fixture/main.dart" },
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

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// dartq/explain intake validation: a missing or empty `symbol` answers
/// `INVALID_PARAMS` and the session stays healthy.
#[test]
fn dartq_explain_rejects_a_bad_symbol() {
    let _guard = workspace_lock();
    let workspace = seeded_explain_workspace();
    let mut h = spawn(&workspace, &store_root("rpc-explain-usage"));

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

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// dartq/compat vs the CLI matrix: per-SDK verdict rows in axis order,
/// incompatible axes as data under a clean exit — the CLI compat test's
/// fixture shape driven over RPC.
#[test]
fn dartq_compat_matches_the_cli_matrix() {
    let _guard = workspace_lock();
    let workspace = Workspace::new(CLEAN_MAIN);
    // The per-axis lever is the SDK whitelist, not the `dart:` library
    // table: dafa65b made the `dart:` URI face the generated oracle-pinned
    // flat table (axis-independent), so the same re-differentiation the
    // CLI compat fixture took applies here — `class Foo<in T>` codes
    // `experiment_not_enabled_off_by_default` only under the SDK whose
    // `allowed_experiments.json` denies package `fixture` the `variance`
    // grant (oracle-cited `experiment_not_enabled_off_by_default@1:11`).
    fs::write(workspace.path("lib/a.dart"), "class Foo<in T> {}\n").expect("write a.dart");
    // `variance` has no experimental release version, so a grant applies
    // only at the current language version (the exact-equality gate in
    // `dartq-language`): the package pins 3.13. The compat face re-reads
    // the config from disk per query (the disk-view rule), so the
    // overwrite after `Workspace::new` is what the axes see.
    fs::write(
        workspace.path(".dart_tool/package_config.json"),
        r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "packageUri": "lib/", "languageVersion": "3.13"}], "generator": "pub", "generatorVersion": "3.13.1"}"#,
    )
    .expect("write package config");
    // The SDK axes live under `sdks/`, which the base fixture doesn't
    // create: each write establishes its parent directories.
    let write = |relative: &str, content: &str| {
        let path = workspace.path(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        fs::write(path, content).expect("file");
    };
    // The grant-carrying axis (the compatible one).
    write("sdks/new/version", "3.13.1\n");
    write(
        "sdks/new/lib/libraries.json",
        r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}, "async": {"uri": "async/async.dart"}}}}"#,
    );
    write("sdks/new/lib/async/async.dart", "library dart.async;\n");
    write(
        "sdks/new/lib/_internal/allowed_experiments.json",
        r#"{"version": 1, "experimentSets": {"grants": ["variance"]}, "sdk": {"default": {"experimentSet": "grants"}}, "packages": {"fixture": {"experimentSet": "grants"}}}"#,
    );
    // The grant-less axis (the incompatible one).
    write("sdks/old/version", "3.5.0\n");
    write(
        "sdks/old/lib/libraries.json",
        r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#,
    );
    write(
        "sdks/old/lib/_internal/allowed_experiments.json",
        r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
    );

    let a = workspace.path("lib/a.dart");
    let mut h = spawn(&workspace, &store_root("rpc-compat"));
    // `lib/a.dart` carries the variance modifier the session's SDK never
    // grants: the sweep publishes one push for it.
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
    assert_eq!(
        matrix[0]["first_failure"]["code"],
        "experiment_not_enabled_off_by_default"
    );
    assert_eq!(
        matrix[0]["first_failure"]["file"],
        "package:fixture/a.dart",
        "the failure file is the package-form label, matching the CLI matrix"
    );
    assert_eq!(matrix[0]["first_failure"]["line"], 1);
    assert_eq!(matrix[0]["first_failure"]["col"], 11, "the `in` modifier span");
    assert_eq!(matrix[1]["sdk"], "3.13.1");
    assert_eq!(matrix[1]["compatible"], true);
    assert!(matrix[1].get("first_failure").is_none());

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// dartq/fix answers with real patch documents (the #355 bridge feeds the
/// built-in lint rows; the provenance tag rides every edit). The staged
/// protocol's first frame is the `dartq/fixProgress` notice (written at the
/// dispatch drain); the answer lands one worker hop later, which
/// `answer_result` waits out. A missing target answers the usage tier over
/// RPC.
#[test]
fn dartq_fix_reports_the_schema_shaped_empty_document() {
    let _guard = workspace_lock();
    let workspace = Workspace::new(CLEAN_MAIN);
    let mut h = spawn(&workspace, &store_root("rpc-fix"));

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

    let (code, _) = h.join_ended();
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

    h.burst(
        &[
            request(
                2,
                "dartq/check",
                json!({ "paths": [workspace.path("lib")] }),
            ),
            notification("dartq/cancel", json!({ "id": 2 })),
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

    let (code, _) = h.join_ended();
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

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// An unknown verbosity spelling is an `INVALID_PARAMS` answer, and the
/// session stays healthy afterwards.
#[test]
fn dartq_invalid_verbosity_answers_invalid_params() {
    let _guard = workspace_lock();
    let workspace = Workspace::new(CLEAN_MAIN);
    let mut h = spawn(&workspace, &store_root("rpc-invalid-verbosity"));

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

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// Multi-root context routing over the wire: a `context` tag
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

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);

    fs::remove_dir_all(&workspace.root).ok();
}

/// dartq/fix emits real patch rows through the staged dispatch (progress
/// notice at the drain, answer one worker hop later): a file carrying an
/// `unnecessary_new` hit materializes an anchored edit through the #355
/// bridge, provenance-tagged `builtin-lint`, and an `agentId` selector is
/// rejected (the disk-view rule). The #2070/#2120-window fix face widens
/// the answer legitimately: the unused local rides its `builtin-semantic`
/// rename producer, every diagnostic rides the line/file ignore offers,
/// and the governing `analysis_options.yaml` enters the document set with
/// its own per-code ignore rows.
#[test]
fn dartq_fix_emits_real_builtin_rows_with_provenance() {
    let _guard = workspace_lock();
    let workspace = Workspace::new(CLEAN_MAIN);
    let fixable = workspace.root.join("fixable.dart");
    fs::write(&fixable, "void f() { var a = new B(); }\n").expect("write fixable");
    // The options file is the fixture's governing posture: options-less
    // enables nothing (#936), so the rule rides an explicit enable.
    fs::write(
        workspace.root.join("analysis_options.yaml"),
        "linter:\n  rules:\n    - unnecessary_new\n",
    )
    .expect("write options");
    let mut h = spawn(&workspace, &store_root("rpc-fix-rows"));

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
    assert_eq!(
        files.len(),
        2,
        "the hit file plus the governing options file (the cross-file document set)"
    );
    assert_eq!(files[0]["file"], fixable.display().to_string());
    let edits = files[0]["edits"].as_array().expect("edits array");
    assert_eq!(
        edits.len(),
        6,
        "the rewrite, the unused-local rename, and the two codes' ignore offers"
    );
    assert_eq!(edits[0]["source"], "builtin-lint");
    assert_eq!(edits[0]["offset"], 19);
    assert_eq!(edits[0]["length"], 4, "keyword plus trailing space");
    assert_eq!(edits[0]["anchor"]["line"], 1);
    // The `unused_local_variable` semantic row rides its rename producer
    // (the #2070 producer wave): `a` never reads, so it becomes `_a`.
    assert_eq!(edits[1]["source"], "builtin-semantic");
    assert_eq!(edits[1]["offset"], 15);
    assert_eq!(edits[1]["length"], 1);
    assert_eq!(edits[1]["replacement"], "_a");
    assert_eq!(edits[1]["anchor"]["line"], 1);
    // Every diagnostic also rides the line and file ignore offers (the
    // builtin-ignore provenance), both anchored at the file head — the
    // lint code first, then the semantic one.
    assert_eq!(edits[2]["source"], "builtin-ignore");
    assert_eq!(edits[2]["offset"], 0);
    assert_eq!(edits[2]["length"], 0);
    assert_eq!(edits[2]["replacement"], "// ignore: unnecessary_new\n");
    assert_eq!(edits[2]["anchor"]["line"], 1);
    assert_eq!(edits[3]["source"], "builtin-ignore");
    assert_eq!(edits[3]["offset"], 0);
    assert_eq!(edits[3]["length"], 0);
    assert_eq!(
        edits[3]["replacement"],
        "// ignore_for_file: unnecessary_new\n\n"
    );
    assert_eq!(edits[3]["anchor"]["line"], 1);
    assert_eq!(edits[4]["source"], "builtin-ignore");
    assert_eq!(edits[4]["offset"], 0);
    assert_eq!(edits[4]["length"], 0);
    assert_eq!(edits[4]["replacement"], "// ignore: unused_local_variable\n");
    assert_eq!(edits[4]["anchor"]["line"], 1);
    assert_eq!(edits[5]["source"], "builtin-ignore");
    assert_eq!(edits[5]["offset"], 0);
    assert_eq!(edits[5]["length"], 0);
    assert_eq!(
        edits[5]["replacement"],
        "// ignore_for_file: unused_local_variable\n\n"
    );
    assert_eq!(edits[5]["anchor"]["line"], 1);
    // The options-file set: one row per code in sorted order, each
    // appending the `analyzer: errors:` ignore at the options file's end
    // (offset = its UTF-16 length), anchored against the options text.
    assert_eq!(
        files[1]["file"],
        workspace.path("analysis_options.yaml").display().to_string()
    );
    let options = files[1]["edits"].as_array().expect("options edits array");
    assert_eq!(options.len(), 2, "one ignore row per code");
    assert_eq!(options[0]["source"], "builtin-ignore");
    assert_eq!(options[0]["offset"], 39);
    assert_eq!(options[0]["length"], 0);
    assert_eq!(
        options[0]["replacement"],
        "analyzer:\n  errors:\n    unnecessary_new: ignore\n"
    );
    assert_eq!(options[0]["anchor"]["line"], 4);
    assert_eq!(options[1]["source"], "builtin-ignore");
    assert_eq!(options[1]["offset"], 39);
    assert_eq!(
        options[1]["replacement"],
        "analyzer:\n  errors:\n    unused_local_variable: ignore\n"
    );

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

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);

    fs::remove_dir_all(&workspace.root).ok();
}

/// dartq/assists answers through the staged dispatch (progress notice at
/// the drain, answer one worker hop later): with no plugins discovered the
/// answer carries the built-in rows alone, schema-stamped; a missing target
/// is the usage tier; an `agentId` selector is rejected.
#[test]
fn dartq_assists_answers_schema_stamped_without_plugins() {
    let _guard = workspace_lock();
    let workspace = Workspace::new(CLEAN_MAIN);
    let mut h = spawn(&workspace, &store_root("rpc-assists"));

    h.step(
        &request(
            2,
            "dartq/assists",
            json!({ "path": workspace.path("lib/main.dart"), "offset": 0, "length": 0 }),
        ),
        1,
    );
    let result = answer_result(&h, 2);
    // Offset 0 rests on the `void` return type, before the body's first
    // token end, so the built-in async-body assist proposes (the official
    // guard declines only past `body.block.beginToken.end`): one change,
    // two edits — the `async ` insertion at the body's `{` and the
    // `Future<void>` wrap over the written return type. Empty anchor
    // flanks (the line start) are omitted.
    assert_eq!(
        result,
        json!({
            "file": workspace.path("lib/main.dart"),
            "schema_version": 1,
            "assists": [{
                "message": "Convert to async function body",
                "source": "builtin-assist",
                "priority": 31,
                "edits": [
                    {
                        "offset": 12,
                        "length": 0,
                        "replacement": "async ",
                        "source": "builtin-assist",
                        "anchor": {
                            "line": 1,
                            "column": 13,
                            "context_before": " main() ",
                            "context_after": "{}",
                        },
                    },
                    {
                        "offset": 0,
                        "length": 4,
                        "replacement": "Future<void>",
                        "source": "builtin-assist",
                        "anchor": { "line": 1, "column": 1, "context_after": " main() " },
                    },
                ],
            }],
        }),
        "no plugins: the built-in async-body row alone, schema-stamped"
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

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);

    fs::remove_dir_all(&workspace.root).ok();
}

/// dartq/assists threads the session's SDK root into the resolved-light
/// assist mount: `Add return type` is a resolved-only producer (silent
/// without a mounted engine), so the row answers only when the commands
/// layer receives the session's SDK — the synthetic fixture SDK, which no
/// `DART_SDK` environment names. The pre-fix hard-coded `None` mounted
/// nothing here and the row was silently absent.
#[test]
fn dartq_assists_mounts_the_session_sdk_for_resolved_rows() {
    let _guard = workspace_lock();
    let workspace = Workspace::new("f() { return 42; }\n");
    let mut h = spawn(&workspace, &store_root("rpc-assists-sdk"));

    h.step(
        &request(
            2,
            "dartq/assists",
            json!({ "path": workspace.path("lib/main.dart"), "offset": 0, "length": 0 }),
        ),
        1,
    );
    let result = answer_result(&h, 2);
    let assists = result["assists"].as_array().expect("assists array");
    let row = assists
        .iter()
        .find(|row| row["message"] == "Add return type")
        .expect("the resolved-only row mounts through the session SDK");
    assert_eq!(row["source"], "builtin-assist");
    assert_eq!(row["priority"], 30);
    // The inferred return type inserts before the name (the resolved mount's
    // type, not the engine-free silence).
    assert_eq!(row["edits"][0]["offset"], 0);
    assert_eq!(row["edits"][0]["replacement"], "int ");

    let (code, _) = h.join_ended();
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
    // The fix dispatch is hit-directed: a plugin is consulted per lint hit,
    // so the hit the dispatch keys on rides an explicit enable — options-less
    // enables nothing (#936), the same fixture shape the rpc_schema fix pins
    // use.
    fs::write(
        workspace.root.join("analysis_options.yaml"),
        "linter:\n  rules:\n    - unnecessary_new\n",
    )
    .expect("write options");

    let mut h = spawn(&workspace, &store_root("rpc-assists-live"));

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
    assert_eq!(
        files.len(),
        2,
        "the hit file plus its governing options file (the cross-file set): {result}"
    );
    let edits = files[0]["edits"].as_array().expect("edits array");
    let sources: Vec<&str> = edits
        .iter()
        .map(|edit| edit["source"].as_str().expect("source"))
        .collect();
    assert_eq!(
        sources.first(),
        Some(&"builtin-lint"),
        "the built-in lint rows lead: {edits:?}"
    );
    assert!(
        sources[..sources.len() - 1]
            .iter()
            .all(|source| matches!(
                *source,
                "builtin-lint" | "builtin-semantic" | "builtin-ignore"
            )),
        "the built-in provenance spellings (the SDK face rides the daemon's fix pass): {sources:?}"
    );
    assert_eq!(
        sources.last(),
        Some(&"fixer"),
        "built-in rows first, then the plugin change flattened: {edits:?}"
    );
    // The dispatch pin: the plugin was consulted at the lint hit's range and
    // echoed it back — the discovery -> spawn -> push -> fixes path over a
    // real child process.
    let fixer = edits.last().expect("the plugin row").clone();
    assert_eq!(fixer["offset"], 19);
    assert_eq!(fixer["replacement"], "const ");

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);

    fs::remove_dir_all(&workspace.root).ok();
}

// ---------------------------------------------------------------------------
// dartq/index and the non-.dart watch
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

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);

    fs::remove_dir_all(&workspace.root).ok();
}

/// The family answers 1:1 over the wire: all eight CLI commands have
/// a live daemon face on one fixture session — the round-trip pin the #474
/// schema-freeze sweep builds on.
#[test]
fn dartq_family_answers_one_to_one_over_the_fixture() {
    let _guard = workspace_lock();
    let workspace = seeded_query_workspace();
    let a = workspace.path("lib/a.dart");
    let mut h = spawn(&workspace, &store_root("rpc-family"));

    let cases: [(i64, &str, Value, i64); 8] = [
        (2, "dartq/index", json!({}), 1),
        (
            3,
            "dartq/check",
            json!({ "paths": [workspace.path("lib")] }),
            1,
        ),
        (4, "dartq/analyze", json!({}), 1),
        (5, "dartq/query", json!({ "name": "Foo" }), 1),
        (
            6,
            "dartq/impact",
            json!({ "path": a.display().to_string() }),
            2, // the impact report stamps its own schema generation
        ),
        (
            7,
            "dartq/explain",
            json!({ "symbol": "Foo", "sdk": workspace.path("sdk") }),
            1,
        ),
        (
            8,
            "dartq/compat",
            json!({
                "path": a.display().to_string(),
                "sdk_roots": [workspace.path("sdk")],
            }),
            1,
        ),
        (
            9,
            "dartq/fix",
            json!({ "paths": [workspace.path("lib")] }),
            1,
        ),
    ];
    for (id, method, params, schema) in cases {
        h.step(&request(id, method, params), 1);
        let result = answer_result(&h, id);
        assert_eq!(
            result["schema_version"], schema,
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

    let (code, _) = h.join_ended();
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
        .filter(|frame| frame["method"] == "dartq/diagnostics")
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
/// file, and the on-disk fix the daemon was never told about through
/// `dartq/sync` converges into the pushed diagnostics — while the config
/// bytes themselves never surface as analysis content.
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
        dartq_daemon::Session::with_store(workspace.engine_config(), store, None).expect("session");
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

    // The startup drain publishes the broken fixture's diagnostic set —
    // no handshake, the sweep lands ahead of the first client frame.
    let first = wait_for(|| main_pushes(&transcript()).first().cloned());
    assert!(
        !first.is_empty(),
        "the broken fixture publishes errors: {first:?}"
    );

    // The on-disk fix plus the manifest touch: neither is reported through
    // `dartq/sync`. The watch batch carries both; the split routes the
    // manifest to the context refresh, whose re-discovery re-reads
    // main.dart. The manifest touch retriggers until the refresh lands:
    // the OS substrate arms asynchronously and a pre-arm change is
    // absorbed by design (the intake contract watch.rs pins), so a single
    // write can race the arm — the same retrigger tolerance the
    // substrate's spawned-thread pin uses. Same-byte rewrites dedup in
    // the debounce (bounded further by the max-hold flush), and the
    // config lever heals main.dart regardless of whether main's own event
    // survived the arm.
    fs::write(workspace.path("lib/main.dart"), CLEAN_MAIN).expect("fix main on disk");
    let manifest = workspace.path("pubspec.yaml");
    let touched_manifest =
        "name: fixture\nversion: 0.1.0\n# watched touch\nenvironment:\n  sdk: ^3.5.0\n";
    wait_for(|| {
        fs::write(&manifest, touched_manifest).expect("touch the manifest");
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

    // Clean end: EOF — a client close is the session's only end signal.
    drop(client);
    let code = server
        .join()
        .expect("server thread does not panic")
        .expect("transport does not fail on memory pipes");
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);

    fs::remove_dir_all(&workspace.root).ok();
}
