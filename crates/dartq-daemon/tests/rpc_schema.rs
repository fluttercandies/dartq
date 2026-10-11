//! Stage-B schema validation over the live `dartq/*` RPC answers (#623):
//! every face is driven over the fixture workspace through the wire harness
//! and its answer is run through `dartq_schema::validate` for its domain and
//! `validate_at_tier` at the tier the request asked for — the daemon-boundary
//! capture PRD §15.7's CI consumption names. Also pinned: the single-stamp
//! rule (exactly one `schema_version` per response level — the producer's
//! own; the daemon assembly adds none and re-adds no dropped field) and the
//! validator bite (a corrupted live answer is named at its field path).
//!
//! Face → domain map (dartq-schema `Domain`): `dartq/query` → `Query`,
//! `dartq/impact` → `Impact` (tier-invariant), `dartq/compat` → `Compat`,
//! `dartq/explain` → `Explain`, `dartq/fix` → `FixPatch` per per-file patch
//! document (the daemon envelope is the JSONL batch convention).
//! `dartq/check`, `dartq/analyze`, `dartq/index`, and `dartq/assists` carry
//! the `schema_version` stamp with no published domain yet — the stamp-only
//! pin below documents that gap (#632).

mod common;

use std::fs;

use common::{BROKEN_MAIN, CLEAN_MAIN, Workspace, notification, request, spawn, store_root};
use dartq_schema::{Domain, Verbosity, validate, validate_at_tier};
use serde_json::{Value, json};

/// `initialize` + `initialized`: the sweep publishes `publishes` frames.
fn handshake(h: &mut common::Harness, publishes: usize) {
    h.step(&request(1, "initialize", json!({})), 1);
    h.step(&notification("initialized", json!({})), publishes);
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

/// Serializes `result` and validates it against `domain` — the union check
/// plus the exactness of `tier` — returning the payload text.
fn assert_valid_at_tier(result: &Value, domain: Domain, tier: Verbosity) -> String {
    let payload = serde_json::to_string(result).expect("response serializes");
    validate(&payload, domain)
        .unwrap_or_else(|v| panic!("{domain:?} union violation: {v}; payload: {payload}"));
    validate_at_tier(&payload, domain, tier)
        .unwrap_or_else(|v| panic!("{domain:?} violates the {tier} tier: {v}; payload: {payload}"));
    payload
}

/// `schema_version` occurrences in the serialized response: the wire-level
/// single-stamp pin. Parsed maps dedupe keys, so only the text count can
/// observe a stamp landing at two levels of one payload.
fn stamp_count(result: &Value) -> usize {
    serde_json::to_string(result)
        .expect("response serializes")
        .matches("\"schema_version\"")
        .count()
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

/// The seeded workspace of the explain face: the dog import chain plus its
/// package-URI test.
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

/// dartq/query answers `Domain::Query` at every tier it serves: the minimal
/// answer carries only the row locators, the compact answer is the shared
/// writer's object verbatim (one stamp — the daemon assembly adds none and
/// re-adds no dropped field), and the full tier adds nothing the union
/// rejects. A `since` delta answer validates with its cursor echo.
#[test]
fn dartq_query_answers_validate_at_every_tier() {
    let workspace = seeded_query_workspace();
    let mut h = spawn(&workspace, &store_root("schema-query"));
    handshake(&mut h, 0);

    for (id, tier) in [
        (2, Verbosity::Minimal),
        (3, Verbosity::Compact),
        (4, Verbosity::Full),
    ] {
        h.step(
            &request(
                id,
                "dartq/query",
                json!({ "name": "Foo", "verbosity": tier.to_string() }),
            ),
            1,
        );
        let result = answer_result(&h, id);
        let payload = assert_valid_at_tier(&result, Domain::Query, tier);
        assert_eq!(
            payload.matches("\"schema_version\"").count(),
            1,
            "the writer's stamp is the only one at {tier}: {payload}"
        );
    }

    // The #627 omission stays omitted through the daemon assembly: the
    // frozen contract types `qualified` as the qualified-name string, the
    // row model carries only the bit, so the writer omits the column —
    // and the daemon adds no field on top of the writer's payload.
    let compact = answer_result(&h, 3);
    for row in compact["references"].as_array().expect("references") {
        assert!(
            row.get("qualified").is_none(),
            "the unproducible qualified column stays omitted: {row}"
        );
    }

    // A delta answer validates too: the cursor echo is a declared session
    // field and the filtered rows keep their envelope shape.
    h.step(
        &request(5, "dartq/query", json!({ "name": "Foo", "since": 0 })),
        1,
    );
    let delta = answer_result(&h, 5);
    assert_eq!(delta["since"], 0, "the cursor echoes");
    assert_valid_at_tier(&delta, Domain::Query, Verbosity::Compact);

    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
    fs::remove_dir_all(&workspace.root).ok();
}

/// dartq/impact answers `Domain::Impact` and is tier-invariant: the same
/// report answers at minimal, compact, and full byte for byte (every field
/// is a locator or a verdict — the canonical table trims nothing), so the
/// tier-exact check is the union check itself, asserted here by equality
/// across the three tiers.
#[test]
fn dartq_impact_validates_and_is_tier_invariant() {
    let workspace = seeded_query_workspace();
    // The test imports the lib through its package URI only: a clean
    // workspace keeps the sweep publish-free (a deterministic handshake).
    fs::write(
        workspace.path("test/a_test.dart"),
        "import 'package:fixture/a.dart';\nvoid main() {\n  foo();\n}\n",
    )
    .expect("write the test");
    let a = workspace.path("lib/a.dart");
    let mut h = spawn(&workspace, &store_root("schema-impact"));
    handshake(&mut h, 0);

    let mut baseline: Option<Value> = None;
    for (id, tier) in [
        (2, Verbosity::Minimal),
        (3, Verbosity::Compact),
        (4, Verbosity::Full),
    ] {
        h.step(
            &request(
                id,
                "dartq/impact",
                json!({ "path": a.display().to_string(), "verbosity": tier.to_string() }),
            ),
            1,
        );
        let result = answer_result(&h, id);
        assert_valid_at_tier(&result, Domain::Impact, tier);
        match &baseline {
            Some(first) => assert_eq!(&result, first, "tier-invariant report at {tier}"),
            None => baseline = Some(result),
        }
    }
    assert_eq!(
        stamp_count(baseline.as_ref().expect("answered")),
        1,
        "the report carries its own single stamp"
    );

    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
    fs::remove_dir_all(&workspace.root).ok();
}

/// dartq/compat answers `Domain::Compat` at every tier: compact/full carry
/// the evidence columns exactly when an axis's content demands them; the
/// minimal answer keeps the verdict + axis identity and drops the evidence
/// (the tier-exact check enforces the drop on the live answer).
#[test]
fn dartq_compat_answers_validate_at_every_tier() {
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
    write("sdks/new/version", "3.13.1\n");
    write(
        "sdks/new/lib/libraries.json",
        r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}, "async": {"uri": "async/async.dart"}}}}"#,
    );
    write("sdks/new/lib/async/async.dart", "library dart.async;\n");
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
    let mut h = spawn(&workspace, &store_root("schema-compat"));
    // `lib/a.dart` imports `dart:async`, unresolvable under the session's
    // core-only SDK: the sweep publishes one push for it.
    handshake(&mut h, 1);

    for (id, tier) in [
        (2, Verbosity::Minimal),
        (3, Verbosity::Compact),
        (4, Verbosity::Full),
    ] {
        h.step(
            &request(
                id,
                "dartq/compat",
                json!({
                    "path": a.display().to_string(),
                    "sdk_roots": [workspace.path("sdks/old"), workspace.path("sdks/new")],
                    "verbosity": tier.to_string(),
                }),
            ),
            1,
        );
        let result = answer_result(&h, id);
        let payload = assert_valid_at_tier(&result, Domain::Compat, tier);
        assert_eq!(
            payload.matches("\"schema_version\"").count(),
            1,
            "one stamp at {tier}: {payload}"
        );
    }

    // Minimal drops the evidence columns; compact keeps them.
    let minimal = answer_result(&h, 2);
    for axis in minimal["matrix"].as_array().expect("matrix") {
        assert!(
            axis.get("first_failure").is_none() && axis.get("error").is_none(),
            "the minimal axis keeps only the verdict + identity: {axis}"
        );
    }
    let compact = answer_result(&h, 3);
    assert_eq!(
        compact["matrix"][0]["first_failure"]["code"], "uri_does_not_exist",
        "the compact tier carries the evidence"
    );

    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
    fs::remove_dir_all(&workspace.root).ok();
}

/// dartq/explain answers `Domain::Explain` at every tier: the minimal tier
/// swaps the section arrays for `counts`, compact carries the arrays, full
/// rides the short-id economics — the `symbols` table maps ids to FILE
/// paths, the values the rows themselves now carry (the #623B arbitration:
/// the economics dedup this face's own path-valued rows, unlike query/
/// impact's `label|name` entity-reference table, so `Domain::Explain`
/// validates the file-path spelling). A `since` delta answer validates with
/// its cursor echo.
#[test]
fn dartq_explain_answers_validate_at_every_tier() {
    let workspace = seeded_explain_workspace();
    let mut h = spawn(&workspace, &store_root("schema-explain"));
    handshake(&mut h, 0);

    for (id, tier) in [
        (2, Verbosity::Minimal),
        (3, Verbosity::Compact),
        (4, Verbosity::Full),
    ] {
        h.step(
            &request(
                id,
                "dartq/explain",
                json!({
                    "symbol": "Dog",
                    "sdk": workspace.path("sdk"),
                    "verbosity": tier.to_string(),
                }),
            ),
            0,
        );
        let result = answer_result(&h, id);
        let payload = assert_valid_at_tier(&result, Domain::Explain, tier);
        assert_eq!(
            payload.matches("\"schema_version\"").count(),
            1,
            "one stamp at {tier}: {payload}"
        );
        if tier == Verbosity::Minimal {
            assert!(
                result.get("references").is_none() && result.get("counts").is_some(),
                "the minimal tier swaps the arrays for counts: {result}"
            );
        }
    }

    // The arbitration pin (#623B): the full-tier table maps short ids to
    // the substituted rows' file paths — `s1` is the first-seen repeated
    // path (main.dart, a reference and a dependency), assigned in the
    // economics' deterministic first-seen order.
    let full = answer_result(&h, 4);
    let symbols = full["symbols"]
        .as_object()
        .expect("the short-id table pays on the fixture's long temp paths");
    assert_eq!(
        symbols["s1"],
        workspace.path("lib/main.dart").display().to_string(),
        "the table maps ids to file paths: {symbols:?}"
    );

    // Delta economics: nothing was written through this session, so the
    // use/import/test rows filter out while the declaration-site sections
    // stay — and the answer validates with the declared `since` echo.
    h.step(
        &request(
            5,
            "dartq/explain",
            json!({ "symbol": "Dog", "sdk": workspace.path("sdk"), "since": 0 }),
        ),
        0,
    );
    let delta = answer_result(&h, 5);
    assert_eq!(delta["since"], 0, "the cursor echoes");
    assert_eq!(
        delta["declaration"]["name"], "Dog",
        "the red line stays in delta answers"
    );
    assert_valid_at_tier(&delta, Domain::Explain, Verbosity::Compact);

    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
    fs::remove_dir_all(&workspace.root).ok();
}

/// dartq/fix's per-file documents are the `Domain::FixPatch` payloads (the
/// daemon envelope is the batch convention, one stamp of its own plus one
/// per document). Since #631 declared the provenance surface — `source`
/// required at every tier, `message`/`priority`/`rule` full-only — the live
/// documents validate outright at every tier; `stale` remains the only
/// absence-tolerant edit field. The builtin row carries the `builtin-lint`
/// tag, and the full tier adds its rule id and human message (a plugin
/// row's `message` + `priority` ride the same full-only rule; the
/// live-plugin dispatch is the ignored serial test's scene).
#[test]
fn dartq_fix_documents_validate_with_their_provenance() {
    let workspace = Workspace::new(CLEAN_MAIN);
    let fixable = workspace.root.join("fixable.dart");
    fs::write(&fixable, "void f() { var a = new B(); }\n").expect("write fixable");
    let mut h = spawn(&workspace, &store_root("schema-fix"));
    handshake(&mut h, 0);

    for (id, tier) in [
        (2, Verbosity::Minimal),
        (3, Verbosity::Compact),
        (4, Verbosity::Full),
    ] {
        h.step(
            &request(
                id,
                "dartq/fix",
                json!({
                    "paths": [fixable.display().to_string()],
                    "verbosity": tier.to_string(),
                }),
            ),
            1,
        );
        let result = answer_result(&h, id);
        let files = result["files"].as_array().expect("files array");
        assert_eq!(files.len(), 1, "the fixable file's document: {result}");
        assert_valid_at_tier(&files[0], Domain::FixPatch, tier);
        assert_eq!(stamp_count(&result), 2, "envelope plus document: {result}");

        let edit = &files[0]["edits"][0];
        assert_eq!(
            edit["source"], "builtin-lint",
            "the provenance tag rides every tier: {edit}"
        );
        if tier == Verbosity::Full {
            assert_eq!(edit["rule"], "unnecessary_new", "the rule id: {edit}");
            assert!(
                !edit["message"].as_str().expect("message").is_empty(),
                "the human message: {edit}"
            );
        } else {
            assert!(
                edit.get("rule").is_none()
                    && edit.get("message").is_none()
                    && edit.get("priority").is_none(),
                "message/priority/rule are full-only: {edit}"
            );
        }
    }

    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
    fs::remove_dir_all(&workspace.root).ok();
}

/// The four faces without a published domain — `dartq/check`'s machine-row
/// JSON, `dartq/analyze`'s LSP entries, `dartq/index`'s tallies,
/// `dartq/assists`'s plugin document — carry the `schema_version` stamp with
/// nothing to validate it against: the stamp asserts a contract dartq-schema
/// never froze (the #623 discovered gap; follow-up ticket = publish their
/// domains). Pinned here at the single-stamp rule; no `validate` wiring is
/// possible without inventing a contract to mask the gap.
#[test]
fn domain_less_faces_carry_exactly_one_stamp() {
    let workspace = Workspace::new(BROKEN_MAIN);
    let mut h = spawn(&workspace, &store_root("schema-domain-less"));
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
    assert_eq!(stamp_count(&result), 1, "check: {result}");

    h.step(&request(3, "dartq/analyze", json!({})), 1);
    let result = answer_result(&h, 3);
    assert_eq!(result["schema_version"], 1);
    assert_eq!(stamp_count(&result), 1, "analyze: {result}");

    h.step(&request(4, "dartq/index", json!({})), 1);
    let result = answer_result(&h, 4);
    assert_eq!(result["schema_version"], 1);
    assert_eq!(stamp_count(&result), 1, "index: {result}");

    h.step(
        &request(
            5,
            "dartq/assists",
            json!({ "path": workspace.path("lib/main.dart") }),
        ),
        1,
    );
    let result = answer_result(&h, 5);
    assert_eq!(result["schema_version"], 1);
    assert_eq!(stamp_count(&result), 1, "assists: {result}");

    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
    fs::remove_dir_all(&workspace.root).ok();
}

/// The validator wiring bites: deliberately corrupted live answers are
/// named at their field path — a locator spelled as a string, a dropped
/// required array, and a minimal-tier answer carrying a compact-only
/// convenience field each fail with the expected violation.
#[test]
fn corrupted_live_answers_are_named_at_their_path() {
    let workspace = seeded_query_workspace();
    let mut h = spawn(&workspace, &store_root("schema-corrupt"));
    handshake(&mut h, 0);

    h.step(&request(2, "dartq/query", json!({ "name": "Foo" })), 1);
    let compact = answer_result(&h, 2);
    h.step(
        &request(
            3,
            "dartq/query",
            json!({ "name": "Foo", "verbosity": "minimal" }),
        ),
        1,
    );
    let minimal = answer_result(&h, 3);

    // A locator spelled as a string.
    let mut corrupt = compact.clone();
    corrupt["declarations"][0]["length"] = json!("three");
    let payload = serde_json::to_string(&corrupt).expect("serializes");
    let violation = validate(&payload, Domain::Query).expect_err("type break must violate");
    assert_eq!(violation.path, "declarations[0].length");
    assert_eq!(violation.expected, "an integer");

    // A required array dropped.
    let mut corrupt = compact;
    corrupt
        .as_object_mut()
        .expect("response object")
        .remove("references");
    let payload = serde_json::to_string(&corrupt).expect("serializes");
    let violation = validate(&payload, Domain::Query).expect_err("dropped array must violate");
    assert_eq!(violation.path, "references");
    assert_eq!(violation.found, "missing");

    // A minimal answer carrying the compact tier's convenience field.
    let mut corrupt = minimal;
    corrupt["declarations"][0]["container"] = json!("Foo");
    let payload = serde_json::to_string(&corrupt).expect("serializes");
    let violation = validate_at_tier(&payload, Domain::Query, Verbosity::Minimal)
        .expect_err("tier leak must violate");
    assert_eq!(violation.path, "declarations[0].container");
    assert_eq!(violation.expected, "omitted at the minimal tier");

    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
    fs::remove_dir_all(&workspace.root).ok();
}
