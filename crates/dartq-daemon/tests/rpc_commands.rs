//! End-to-end `dartq/*` RPC tests over the in-memory-pipe harness: each
//! face answers on a fixture workspace and its result is pinned against the
//! CLI face it mirrors (equivalence rule — the two share one
//! implementation in `dartq-commands`, so the comparison runs the command
//! core directly and renders it through the CLI's oracle-exact writers).
//! Also covers cancellation through the epoch machinery, the verbosity red
//! line, and the version stamp. No network, no dart SDK.

mod common;

use std::fs;
use std::sync::Mutex;

use common::{
    BROKEN_MAIN, CLEAN_MAIN, Workspace, notification, request, spawn, spawn_with_heavy_context,
    store_root,
};
use dartq_commands::discover::discover;
use dartq_schema::SCHEMA_VERSION_IMPACT;
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
                "code": fields[2].to_lowercase(),
                "sev": fields[0].to_lowercase(),
                "file": fields[3],
                "line": fields[4].parse::<u32>().expect("line"),
                "col": fields[5].parse::<u32>().expect("column"),
                "len": fields[6].parse::<u32>().expect("length"),
                "msg": fields[7],
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
/// under the `v` stamp.
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
    assert_eq!(result["v"], 1);

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
        result["diags"],
        json!(expected),
        "the RPC rows equal the CLI machine rows"
    );

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// dartq/analyze vs the CLI lon face (#2657): the RPC answer IS the CLI's
/// own analyze document — the shared `diags_document` builder over the
/// same engine pass, root key and located files included — and a `target`
/// narrows the answer to the covered files exactly like the CLI's
/// positional; a missing target is the CLI's own usage wording.
#[test]
fn dartq_analyze_answers_the_cli_document() {
    let _guard = workspace_lock();
    let workspace = Workspace::new(BROKEN_MAIN);
    let mut h = spawn(&workspace, &store_root("rpc-analyze"));

    h.step(&request(2, "dartq/analyze", json!({})), 1);
    let result = answer_result(&h, 2);
    assert_eq!(result["v"], 1);

    // The CLI face: a fresh engine over the same workspace, the same lint
    // roots the session uses, merged exactly like the CLI's runner — then
    // the one shared document builder both faces render through.
    let mut engine = dartq_engine::Engine::new(workspace.engine_config()).expect("engine");
    let run = dartq_commands::analyze::run(&mut engine, std::slice::from_ref(&workspace.root))
        .expect("analyze core");
    let merged = dartq_commands::analyze::merge_runs(vec![run]);
    let expected = dartq_commands::analyze::diags_document(&dartq_diagnostics::AnalysisOutput {
        diagnostics: &merged.diagnostics,
        files: &merged,
    });
    assert!(
        !expected["diags"].as_array().expect("rows").is_empty(),
        "the broken fixture has diagnostics"
    );
    assert_eq!(
        result, expected,
        "the RPC answer is the CLI's own document, field for field"
    );
    assert_eq!(
        result["root"],
        workspace.root.display().to_string(),
        "the envelope carries the analysis root once"
    );
    assert_eq!(
        result["diags"][0]["file"], "lib/main.dart",
        "inside the root: the located spelling"
    );

    // A target narrows the answer: the file target keeps exactly that
    // file's rows, the same coverage the CLI positional applies.
    h.step(
        &request(
            3,
            "dartq/analyze",
            json!({ "target": workspace.path("lib/main.dart").display().to_string() }),
        ),
        1,
    );
    let narrowed = answer_result(&h, 3);
    assert_eq!(
        narrowed["diags"].as_array().expect("rows").len(),
        expected["diags"].as_array().expect("rows").len(),
        "the fixture's rows all live in main.dart"
    );

    // A relative target rides the read faces' resolution rule: the
    // cwd-relative miss falls back to the workspace root.
    h.step(
        &request(4, "dartq/analyze", json!({ "target": "lib/main.dart" })),
        1,
    );
    assert_eq!(answer_result(&h, 4), narrowed, "the relative target resolves");

    // A missing target is the CLI's own usage wording.
    h.step(
        &request(
            5,
            "dartq/analyze",
            json!({ "target": workspace.path("lib/nope.dart").display().to_string() }),
        ),
        1,
    );
    let error = answer_error(&h, 5);
    assert_eq!(error["code"], -32602, "INVALID_PARAMS: {error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Directory or file doesn't exist"),
        "the CLI face's wording: {error}"
    );

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// The package-inside-a-workspace shape the MCP server serves (#2657): the
/// session's engine analyzes the whole two-package workspace, and a
/// `target` naming one package's directory answers exactly that package's
/// rows — the CLI run in the package directory's own answer — with every
/// file located against the workspace root.
#[test]
fn dartq_analyze_target_narrows_to_the_client_package() {
    let _guard = workspace_lock();
    let workspace = two_package_workspace();
    let mut h = spawn(&workspace, &store_root("rpc-analyze-package"));

    // The whole-workspace answer: both packages' rows.
    h.step(&request(2, "dartq/analyze", json!({})), 1);
    let whole = answer_result(&h, 2);
    let files: Vec<&str> = whole["diags"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["file"].as_str().expect("file"))
        .collect();
    assert_eq!(
        files.len(),
        2,
        "one broken row per package: {files:?} in {whole}"
    );
    assert!(
        files.contains(&"packages/a/lib/main.dart")
            && files.contains(&"packages/b/lib/main.dart"),
        "both packages' rows, located against the workspace root: {files:?}"
    );
    assert_eq!(whole["root"], workspace.root.display().to_string());

    // The package target: the client package's rows alone — the CLI's
    // answer for `dartq analyze` run inside that directory.
    h.step(
        &request(
            3,
            "dartq/analyze",
            json!({ "target": workspace.path("packages/a").display().to_string() }),
        ),
        1,
    );
    let package = answer_result(&h, 3);
    let files: Vec<&str> = package["diags"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["file"].as_str().expect("file"))
        .collect();
    assert_eq!(files, vec!["packages/a/lib/main.dart"], "{package}");

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// A pub-workspace fixture: one root declaring two member packages, each
/// with one broken lib file — assembled through the same pub/package/sdk
/// faces the single-package fixture uses, at the workspace root.
fn two_package_workspace() -> Workspace {
    let workspace = Workspace::new("void main() {}\n");
    let root = &workspace.root;
    fs::write(
        root.join("pubspec.yaml"),
        "name: fixture\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - packages/a\n  - packages/b\n",
    )
    .expect("root pubspec");
    for member in ["a", "b"] {
        let dir = root.join("packages").join(member);
        fs::create_dir_all(dir.join("lib")).expect("member dir");
        fs::write(
            dir.join("pubspec.yaml"),
            format!(
                "name: {member}\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n"
            ),
        )
        .expect("member pubspec");
        fs::write(dir.join("lib/main.dart"), BROKEN_MAIN).expect("member lib");
    }
    // The root's config lists both members plus the root package; the
    // engine assembles the whole workspace through it.
    let config = serde_json::json!({
        "configVersion": 2,
        "packages": [
            {"name": "fixture", "rootUri": "../", "packageUri": "lib/", "languageVersion": "3.5"},
            {"name": "a", "rootUri": "../packages/a", "packageUri": "lib/", "languageVersion": "3.5"},
            {"name": "b", "rootUri": "../packages/b", "packageUri": "lib/", "languageVersion": "3.5"},
        ],
        "generator": "pub",
    });
    fs::write(
        root.join(".dart_tool/package_config.json"),
        config.to_string(),
    )
    .expect("config");
    // Rebuild the session-facing faces over the workspace-wide config.
    let config = std::sync::Arc::new(
        dartq_pub::PackageConfig::parse(&config.to_string()).expect("config parses"),
    );
    let lock =
        dartq_pub::LockFile::parse("packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n").expect("lock");
    let pubspec = dartq_pub::Pubspec::parse(
        &fs::read_to_string(root.join("pubspec.yaml")).expect("pubspec"),
    )
    .expect("pubspec parses");
    let graph = dartq_pub::build_graph(
        &config,
        &lock,
        &pubspec,
        &std::collections::BTreeMap::new(),
        None,
        &root.join(".dart_tool"),
    )
    .expect("graph");
    let packages = dartq_package::PackageTable::build(&config, &graph, &root.join(".dart_tool"))
        .expect("table");
    Workspace {
        root: root.clone(),
        config,
        config_dir: root.join(".dart_tool"),
        packages,
    }
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
    assert_eq!(result["v"], 1);
    assert_eq!(result["sym"], "Foo");
    assert_eq!(
        result["root"],
        workspace.root.display().to_string(),
        "the locator envelope rides once, the analysis root itself"
    );

    let decls = result["decls"].as_array().expect("decls");
    assert_eq!(decls.len(), 1);
    assert_eq!(
        decls[0]["file"], "lib/a.dart",
        "inside the analysis root: the locator's root-relative spelling"
    );
    assert_eq!(decls[0]["kind"], "class");
    assert_eq!(decls[0]["name"], "Foo");
    assert!(
        decls[0].get("container").is_none(),
        "a top-level row omits the container key (the omission rule): {row}",
        row = &decls[0]
    );
    assert_eq!(decls[0]["line"], 1);
    assert_eq!(decls[0]["col"], 7);
    assert_eq!(decls[0]["len"], 3);

    let refs = result["refs"].as_array().expect("refs");
    assert_eq!(refs.len(), 2, "the type use plus the invocation");
    assert_eq!(refs[0]["target"], "lib/a.dart");
    assert_eq!(refs[0]["kind"], "reference");
    // The frozen contract types `qualified` as the qualified-name string and
    // the row model carries only the bit, so the shared writer omits the
    // column (contract-legal, #627) and the daemon adds nothing back.
    assert!(
        refs[0].get("qualified").is_none(),
        "the unproducible qualified column stays omitted: {row}",
        row = &refs[0]
    );
    assert_eq!(refs[0]["off"], 33);
    assert_eq!(refs[1]["kind"], "invoked");
    assert_eq!(refs[1]["off"], 42);

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// dartq/resolveAt over the seeded workspace: the declaration arm, the
/// reference arm (target identity plus the target's own name span), the
/// null hit on an uncovered position, the `since` rejection (a point
/// answer has no rows for a cursor), and the fail-loud unknown file — the
/// wire contract over the engine substrate (`Engine::resolve_at` carries
/// the semantics; offsets are the engine's UTF-16 convention, the same
/// units the query rows' `off` fields pin: 6 = `Foo`'s declaration, 33
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
    assert_eq!(result["v"], 1);
    assert_eq!(result["file"], a.display().to_string());
    assert_eq!(result["off"], 6);
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
    assert_eq!(hit["range"]["off"], 6);
    assert_eq!(hit["range"]["len"], 3);

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
    assert_eq!(hit["range"]["off"], 33);
    assert_eq!(hit["target_range"]["off"], 6);
    assert_eq!(hit["target_range"]["len"], 3);
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
    assert_eq!(answer_result(&h, 7)["v"], 1);

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
    assert_eq!(result["v"], 1);
    assert_eq!(result["file"], b.display().to_string());
    assert_eq!(result["off"], 53);
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
    assert_eq!(items[0]["range"]["off"], 36);
    assert_eq!(items[0]["range"]["len"], 3);

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
    assert_eq!(items[0]["range"]["off"], 6, "Foo's own name span");
    assert_eq!(items[2]["file"], b.display().to_string());
    assert_eq!(items[2]["range"]["off"], 22, "main's own name span");

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
    assert_eq!(answer_result(&h, 9)["v"], 1);

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
    assert_eq!(result["v"], SCHEMA_VERSION_IMPACT);
    assert_eq!(
        result["root"],
        workspace.root.display().to_string(),
        "the locator envelope rides once, the analysis root itself"
    );

    let symbols: Vec<String> = result["sym"]
        .as_array()
        .expect("sym")
        .iter()
        .map(|row| row["sym"].as_str().expect("sym").to_string())
        .collect();
    assert_eq!(
        symbols,
        vec![
            "lib/a.dart|Foo".to_string(),
            "lib/a.dart|foo".to_string()
        ],
        "every face name, ASCII-sorted (the locator's located labels)"
    );

    let affected: Vec<(String, String)> = result["files"]
        .as_array()
        .expect("files")
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
            ("lib/b.dart".to_string(), "semantic".to_string()),
            ("test/a_test.dart".to_string(), "semantic".to_string()),
        ],
        "both files sit inside the root: located tails, `l` sorts before `t`"
    );
    let tests: Vec<String> = result["tests"]
        .as_array()
        .expect("tests")
        .iter()
        .map(|row| row["path"].as_str().expect("path").to_string())
        .collect();
    assert_eq!(tests, vec!["test/a_test.dart".to_string()]);
    assert_eq!(
        result["envs"],
        json!([{ "label": "default", "compatible": true }]),
    );

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// The environment axis parity (#2657): the daemon's `dartq/impact`
/// computes its `envs` verdict through the same shared function the CLI
/// face does, over the analyzed view's diagnostics — on a workspace whose
/// affected set carries an ERROR (the broken fixture's unresolved import),
/// both faces answer the honest `compatible: false` where the daemon once
/// answered a vacuous affirmative. Byte-identical rows, both faces.
#[test]
fn dartq_impact_environments_match_the_cli_face() {
    let _guard = workspace_lock();
    let workspace = Workspace::new(BROKEN_MAIN);
    let main = workspace.path("lib/main.dart");
    let mut h = spawn(&workspace, &store_root("rpc-impact-envs"));

    h.step(
        &request(2, "dartq/impact", json!({ "path": main.display().to_string() })),
        1,
    );
    let result = answer_result(&h, 2);
    assert_eq!(
        result["envs"],
        json!([{ "label": "default", "compatible": false }]),
        "the broken affected set is incompatible on the daemon face"
    );

    // The CLI face over its own fresh engine: the same rows, the same
    // verdict, through the one shared environments function.
    let mut engine = dartq_engine::Engine::new(workspace.engine_config()).expect("engine");
    let report =
        dartq_commands::impact::impact(&mut engine, &main, &[]).expect("the CLI impact face");
    assert_eq!(
        serde_json::to_value(&report.affected_environments).expect("serializes"),
        result["envs"],
        "both faces answer the same environment rows"
    );

    // The relative path rides the read faces' resolution rule.
    h.step(&request(3, "dartq/impact", json!({ "path": "lib/main.dart" })), 1);
    let relative = answer_result(&h, 3);
    assert_eq!(relative["envs"], result["envs"], "the relative path resolves");

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
/// `counts` swap. An `agent_id` is rejected: the face answers the disk view
/// (the `dartq/compat` posture — the report's harvest rides the semantic
/// fan-out, which the session engine runs over recomputed libraries only).
#[test]
#[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
fn dartq_explain_answers_the_report() {
    let _guard = workspace_lock();
    let workspace = seeded_explain_workspace();
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
    assert_eq!(result["v"], 1);
    assert_eq!(result["sym"], "Dog");
    assert_eq!(
        result["root"],
        workspace.root.display().to_string(),
        "the declaration rides, so the locator envelope does too"
    );
    assert_eq!(
        result["declaration"],
        json!({
            "file": "lib/dog.dart",
            "name": "Dog",
            "kind": "class",
            "off": dog_at,
            "len": 3,
        })
    );
    assert_eq!(
        result["bases"],
        json!([{
            "name": "Animal",
            "file": "lib/animal.dart",
            "clause": "extends",
        }])
    );
    assert_eq!(
        result["methods"],
        json!([{ "name": "bark", "kind": "method", "off": bark_at, "len": 4 }])
    );
    let refs = result["refs"].as_array().expect("refs");
    assert_eq!(refs.len(), 2, "the main use plus the test use");
    // Row order keys on the located label: both uses sit inside the
    // analysis root, so the tails sort `lib/` before `test/`.
    assert_eq!(refs[0]["file"], "lib/main.dart");
    assert_eq!(refs[0]["kind"], "invoked");
    assert_eq!(refs[1]["file"], "test/dog_test.dart");
    assert_eq!(
        result["deps"],
        json!([
            { "path": "lib/main.dart" },
            { "path": "test/dog_test.dart" },
        ])
    );
    assert_eq!(
        result["tests"],
        json!([{ "path": "test/dog_test.dart" }])
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
    assert_eq!(result["bases"].as_array().expect("array").len(), 1);
    assert!(
        result["refs"].as_array().expect("array").is_empty()
            && result["deps"].as_array().expect("array").is_empty()
            && result["tests"].as_array().expect("array").is_empty(),
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
    assert!(result.get("refs").is_none(), "arrays drop");
    assert_eq!(result["counts"]["refs"], 2);
    assert_eq!(result["counts"]["tests"], 1);

    // The view selector is rejected loudly, never answered as the disk view.
    h.step(
        &request(
            5,
            "dartq/explain",
            json!({ "symbol": "Dog", "agent_id": "A" }),
        ),
        0,
    );
    let error = answer_error(&h, 5);
    assert_eq!(error["code"], -32602, "INVALID_PARAMS: {error}");
    assert!(
        error["message"]
            .as_str()
            .expect("message")
            .contains("agent_id"),
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
    assert_eq!(answer_result(&h, 5)["sym"], "Dog");

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
    assert_eq!(result["file"], "lib/a.dart");
    assert_eq!(
        result["root"],
        workspace.root.display().to_string(),
        "the subject rides located, so the root envelope does too"
    );

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
        "lib/a.dart",
        "the failure file is the locator's root-relative spelling, matching the CLI matrix"
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
        json!({ "v": 1, "files": [] }),
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
    assert_eq!(result["v"], 1);
    let entries = result["diags"].as_array().expect("entries");
    assert!(!entries.is_empty());
    let entry = &entries[0];
    assert!(entry.get("file").is_some(), "location stays");
    assert!(entry.get("code").is_some(), "code stays");
    assert!(entry.get("line").is_some(), "location stays");
    assert!(entry.get("col").is_some(), "location stays");
    assert!(entry.get("len").is_some(), "location stays");
    assert!(entry.get("sev").is_none(), "prose drops");
    assert!(entry.get("msg").is_none(), "prose drops");
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
    let rows = result["diags"].as_array().expect("rows");
    assert!(!rows.is_empty());
    let row = &rows[0];
    assert!(row.get("file").is_some(), "location stays");
    assert!(row.get("code").is_some(), "code stays");
    assert!(row.get("line").is_some(), "location stays");
    assert!(row.get("col").is_some(), "location stays");
    assert!(row.get("len").is_some(), "location stays");
    assert!(row.get("msg").is_none(), "prose drops");
    assert!(row.get("sev").is_none(), "prose drops");

    h.step(&request(4, "dartq/analyze", json!({})), 1);
    let result = answer_result(&h, 4);
    let entry = &result["diags"].as_array().expect("entries")[0];
    assert!(entry.get("sev").is_some(), "compact keeps severity");
    assert!(entry.get("msg").is_some(), "compact keeps the message");

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
    assert_eq!(answer["v"], 1);

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
        answer_result(&h, id)["diags"]
            .as_array()
            .expect("diagnostics array")
            .iter()
            .map(|row| row["file"].as_str().expect("file").to_owned())
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

    // The disk-view faces reject the selector (the agent_id rule).
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
            .contains("agent_id/context/since"),
        "the rejection names the selector set: {error}"
    );

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);

    fs::remove_dir_all(&workspace.root).ok();
}

/// dartq/fix emits real patch rows through the staged dispatch (progress
/// notice at the drain, answer one worker hop later): a file carrying an
/// `unnecessary_new` hit materializes an anchored edit through the #355
/// bridge, provenance-tagged `builtin-lint`, and an `agent_id` selector is
/// rejected (the disk-view rule). The answer is role-filtered (#2617): the
/// default `primary` role serves one code change per diagnostic — the lint
/// rewrite plus the `builtin-semantic` unused-local rename, each row
/// carrying its `role` key — while the ignore-comment candidates answer
/// only under their own role spelling (`ignore_line` pinned here: the two
/// line comments alone, still provenance-tagged `builtin-ignore`).
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
    assert_eq!(result["v"], 1);
    let files = result["files"].as_array().expect("files array");
    assert_eq!(
        files.len(),
        1,
        "the default role answers the hit file's code changes alone (the \
         ignore trio and the options suppression ride their own roles)"
    );
    assert_eq!(files[0]["file"], fixable.display().to_string());
    let edits = files[0]["edits"].as_array().expect("edits array");
    assert_eq!(
        edits.len(),
        2,
        "the rewrite plus the unused-local rename, one primary candidate \
         per diagnostic"
    );
    assert_eq!(edits[0]["src"], "builtin-lint");
    assert_eq!(edits[0]["role"], "primary");
    assert_eq!(edits[0]["off"], 19);
    assert_eq!(edits[0]["len"], 4, "keyword plus trailing space");
    assert_eq!(edits[0]["anchor"]["line"], 1);
    // The `unused_local_variable` semantic row rides its rename producer
    // (the #2070 producer wave): `a` never reads, so it becomes `_a`.
    assert_eq!(edits[1]["src"], "builtin-semantic");
    assert_eq!(edits[1]["role"], "primary");
    assert_eq!(edits[1]["off"], 15);
    assert_eq!(edits[1]["len"], 1);
    assert_eq!(edits[1]["text"], "_a");
    assert_eq!(edits[1]["anchor"]["line"], 1);

    // The suppression spelling opts into its own candidates alone: the
    // line-comment offers for both codes, anchored at the file head — the
    // lint code first, then the semantic one.
    h.step(
        &request(
            3,
            "dartq/fix",
            json!({
                "paths": [fixable.display().to_string()],
                "role": "ignore_line",
            }),
        ),
        1,
    );
    let result = answer_result(&h, 3);
    let files = result["files"].as_array().expect("files array");
    assert_eq!(files.len(), 1, "the line comments address the hit file: {result}");
    assert_eq!(files[0]["file"], fixable.display().to_string());
    let edits = files[0]["edits"].as_array().expect("edits array");
    assert_eq!(edits.len(), 2, "one line-comment offer per code");
    assert_eq!(edits[0]["src"], "builtin-ignore");
    assert_eq!(edits[0]["role"], "ignore_line");
    assert_eq!(edits[0]["off"], 0);
    assert_eq!(edits[0]["len"], 0);
    assert_eq!(edits[0]["text"], "// ignore: unnecessary_new\n");
    assert_eq!(edits[0]["anchor"]["line"], 1);
    assert_eq!(edits[1]["src"], "builtin-ignore");
    assert_eq!(edits[1]["role"], "ignore_line");
    assert_eq!(edits[1]["off"], 0);
    assert_eq!(edits[1]["len"], 0);
    assert_eq!(edits[1]["text"], "// ignore: unused_local_variable\n");
    assert_eq!(edits[1]["anchor"]["line"], 1);

    h.step(
        &request(
            4,
            "dartq/fix",
            json!({ "paths": [fixable.display().to_string()], "agent_id": "a" }),
        ),
        1,
    );
    let error = answer_error(&h, 4);
    assert_eq!(error["code"], -32602, "INVALID_PARAMS: {error}");
    assert!(
        error["message"].as_str().unwrap_or("").contains("agent_id"),
        "the disk-view rejection names the selector: {error}"
    );

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);

    fs::remove_dir_all(&workspace.root).ok();
}

/// dartq/assists answers through the staged dispatch (progress notice at
/// the drain, answer one worker hop later): with no plugins discovered the
/// answer carries the built-in rows alone, schema-stamped; a missing target
/// is the usage tier; an `agent_id` selector is rejected.
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
            "v": 1,
            "assists": [{
                "msg": "Convert to async function body",
                "src": "builtin-assist",
                "priority": 31,
                "edits": [
                    {
                        "off": 12,
                        "len": 0,
                        "text": "async ",
                        "src": "builtin-assist",
                        "anchor": {
                            "line": 1,
                            "col": 13,
                            "before": " main() ",
                            "after": "{}",
                        },
                    },
                    {
                        "off": 0,
                        "len": 4,
                        "text": "Future<void>",
                        "src": "builtin-assist",
                        "anchor": { "line": 1, "col": 1, "after": " main() " },
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
            json!({ "path": workspace.path("lib/main.dart"), "agent_id": "a" }),
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
        .find(|row| row["msg"] == "Add return type")
        .expect("the resolved-only row mounts through the session SDK");
    assert_eq!(row["src"], "builtin-assist");
    assert_eq!(row["priority"], 30);
    // The inferred return type inserts before the name (the resolved mount's
    // type, not the engine-free silence).
    assert_eq!(row["edits"][0]["off"], 0);
    assert_eq!(row["edits"][0]["text"], "int ");

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
        assists[0]["src"], "fixer",
        "the manifest name is the provenance"
    );
    assert_eq!(assists[0]["msg"], "insert const");
    assert_eq!(assists[0]["priority"], 1);
    assert_eq!(
        assists[0]["edits"][0]["off"], 19,
        "the requested range echoes back"
    );
    assert_eq!(assists[0]["edits"][0]["text"], "const ");
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
        .map(|edit| edit["src"].as_str().expect("src"))
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
    assert_eq!(fixer["off"], 19);
    assert_eq!(fixer["text"], "const ");

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
/// the engine-view intake rule (an unknown `agent_id` fails loud).
#[test]
fn dartq_index_matches_the_cli_totals() {
    let _guard = workspace_lock();
    let workspace = seeded_query_workspace();
    let mut h = spawn(&workspace, &store_root("rpc-index"));

    h.step(&request(2, "dartq/index", json!({})), 1);
    let result = answer_result(&h, 2);
    assert_eq!(result["v"], 1);
    let run = dartq_commands::index::run(&workspace.root, Some(&workspace.path("sdk")), false)
        .expect("index core");
    assert!(run.files >= 3, "the fixture's file set: {result}");
    assert!(run.references >= 2, "the cross-file uses: {result}");
    assert_eq!(result["files"], run.files, "{result} vs {run:?}");
    assert_eq!(result["libraries"], run.libraries);
    assert_eq!(result["symbols"], run.symbols);
    assert_eq!(result["refs"], run.references);

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

    // An unknown agent_id fails loud at intake (the engine-view rule).
    h.step(&request(5, "dartq/index", json!({ "agent_id": "a" })), 1);
    let error = answer_error(&h, 5);
    assert_eq!(error["code"], -32602, "INVALID_PARAMS: {error}");
    assert!(
        error["message"]
            .as_str()
            .expect("message")
            .contains("unknown agent_id"),
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
            i64::from(SCHEMA_VERSION_IMPACT), // the report's own generation
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
        // Every face stamps the compact vocabulary's `v`, except the
        // compat face's frozen `schema_version`.
        let stamp = if result.get("v").is_some() { &result["v"] } else { &result["schema_version"] };
        assert_eq!(
            stamp, schema,
            "{method} is schema-stamped: {result}"
        );
    }
    // Each face answers its own shape, not a shared envelope.
    assert!(answer_result(&h, 2).get("files").is_some(), "index tallies");
    assert!(
        answer_result(&h, 3).get("diags").is_some(),
        "check rows"
    );
    assert!(
        answer_result(&h, 5).get("decls").is_some(),
        "query rows"
    );
    assert!(
        answer_result(&h, 6).get("sym").is_some(),
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
    let mut session =
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
            &mut session,
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
                frame["result"]["diags"]
                    .as_array()
                    .expect("entries")
                    .clone()
            })
    });
    for entry in &entries {
        let uri = entry["file"].as_str().expect("file");
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
