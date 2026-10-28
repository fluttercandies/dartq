//! Stage-B schema validation over the live `dartq/*` RPC answers (#623):
//! every face is driven over the fixture workspace through the wire harness
//! and its answer is run through `dartq_schema::validate` for its domain and
//! `validate_at_tier` at the tier the request asked for — the daemon-boundary
//! capture the CI consumption names. Also pinned: the single-stamp
//! rule (exactly one version stamp per response level — the compact
//! vocabulary's `v`, or the compat face's frozen `schema_version`; the
//! producer's own, the daemon assembly adds none and re-adds no dropped
//! field) and the validator bite (a corrupted live answer is named at its
//! field path).
//!
//! Face → domain map (dartq-schema `Domain`): `dartq/query` → `Query`,
//! `dartq/impact` → `Impact` (tier-invariant), `dartq/compat` → `Compat`,
//! `dartq/explain` → `Explain`, `dartq/fix` → `FixPatch` per per-file patch
//! document (the daemon envelope is the JSONL batch convention),
//! `dartq/check` → `Check`, `dartq/analyze` → `Analyze`,
//! `dartq/index` → `Index` (tier-invariant), and `dartq/assists` →
//! `Assists`. Every stamped face carries a published domain — the #632
//! gap (a stamp with no validating contract) is closed and pinned below.

mod common;

use std::fs;

use common::{BROKEN_MAIN, CLEAN_MAIN, Workspace, request, spawn, store_root};
use dartq_schema::{Domain, Verbosity, validate, validate_at_tier};
use serde_json::{Value, json};

/// The `result` of the answer frame for `id`. Waits the answer out: the
/// staged dispatch faces (`dartq/fix`, `dartq/assists`) write their progress
/// notice at the drain and answer one worker hop later, so a scan of the
/// settled transcript can race the completion (the window's heavier fix
/// dispatch loses that race consistently).
fn answer_result(h: &common::Harness, id: i64) -> Value {
    h.wait_answer(id)["result"].clone()
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

/// `v`-stamp occurrences in the serialized response: the wire-level
/// single-stamp pin over the compact vocabulary's spelling. Parsed maps
/// dedupe keys, so only the text count can observe a stamp landing at two
/// levels of one payload. (The compat face's frozen `schema_version`
/// stamp is counted inline where it applies.)
fn stamp_count(result: &Value) -> usize {
    serde_json::to_string(result)
        .expect("response serializes")
        .matches("\"v\"")
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
            payload.matches("\"v\"").count(),
            1,
            "the writer's stamp is the only one at {tier}: {payload}"
        );
    }

    // The #627 omission stays omitted through the daemon assembly: the
    // frozen contract types `qualified` as the qualified-name string, the
    // row model carries only the bit, so the writer omits the column —
    // and the daemon adds no field on top of the writer's payload.
    let compact = answer_result(&h, 3);
    for row in compact["refs"].as_array().expect("refs") {
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

    let (code, _) = h.join_ended();
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
    // workspace keeps the sweep publish-free.
    fs::write(
        workspace.path("test/a_test.dart"),
        "import 'package:fixture/a.dart';\nvoid main() {\n  foo();\n}\n",
    )
    .expect("write the test");
    let a = workspace.path("lib/a.dart");
    let mut h = spawn(&workspace, &store_root("schema-impact"));

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

    let (code, _) = h.join_ended();
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
    // The per-axis lever is the SDK whitelist, not the `dart:` library
    // table (dafa65b made the `dart:` face the oracle-pinned flat table,
    // axis-independent): the same re-differentiation the CLI and
    // rpc_commands compat fixtures took — `class Foo<in T>` codes
    // `experiment_not_enabled_off_by_default` only under the axis whose
    // `allowed_experiments.json` denies `fixture` the `variance` grant,
    // which applies only at the package's pinned 3.13 language version
    // (the exact-equality gate; the compat face re-reads the config from
    // disk per query).
    fs::write(workspace.path("lib/a.dart"), "class Foo<in T> {}\n").expect("write a.dart");
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
    let mut h = spawn(&workspace, &store_root("schema-compat"));
    // `lib/a.dart` carries the variance modifier the session's SDK never
    // grants: the sweep's startup drain pushes it before the first
    // request — already in the transcript when these steps run.

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
        compact["matrix"][0]["first_failure"]["code"], "experiment_not_enabled_off_by_default",
        "the compact tier carries the evidence"
    );

    let (code, _) = h.join_ended();
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
            payload.matches("\"v\"").count(),
            1,
            "one stamp at {tier}: {payload}"
        );
        if tier == Verbosity::Minimal {
            assert!(
                result.get("refs").is_none() && result.get("counts").is_some(),
                "the minimal tier swaps the arrays for counts: {result}"
            );
        }
    }

    // The arbitration pin (#623B): the full-tier table maps short ids to
    // the substituted rows' file paths, assigned in the economics'
    // deterministic first-seen order. Under the package-form label wave the
    // lib rows carry `lib/main.dart` and the test tree `test/dog_test.dart`
    // — the locator's root-relative tails. Only the test file's repeats
    // (refs/deps/tests) pay a preamble id; the twice-seen `lib/main.dart`
    // tail never beats the entry cost, so the table is one entry and the
    // lib rows ride spelled out.
    let full = answer_result(&h, 4);
    let symbols = full["symbols"]
        .as_object()
        .expect("the short-id table pays on the test tree's repeats");
    assert_eq!(
        symbols["s1"], "test/dog_test.dart",
        "the table maps ids to located file spellings: {symbols:?}"
    );
    assert_eq!(
        symbols.len(),
        1,
        "the twice-seen `lib/main.dart` tail never pays an id: {symbols:?}"
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

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
    fs::remove_dir_all(&workspace.root).ok();
}

/// dartq/fix's per-file documents are the `Domain::FixPatch` payloads (the
/// daemon envelope is the batch convention; it stamps the compact
/// vocabulary's `v`, as does every patch document). Since #631 declared
/// the provenance surface — `src` required at every tier, `msg`/
/// `priority`/`rule` full-only — the live documents validate outright at
/// every tier; `stale` remains the only absence-tolerant edit field. The
/// builtin row carries the `builtin-lint` tag, and the full tier adds its
/// rule id and human message (a plugin row's `msg` + `priority` ride the
/// same full-only rule; the live-plugin dispatch is the ignored serial
/// test's scene). The answer is role-filtered (#2617): the default
/// `primary` role answers the hit file's code changes alone, and the
/// governing options file's suppression document rides the `suppress`
/// role spelling, pinned after the tier loop.
#[test]
fn dartq_fix_documents_validate_with_their_provenance() {
    let workspace = Workspace::new(CLEAN_MAIN);
    let fixable = workspace.root.join("fixable.dart");
    fs::write(&fixable, "void f() { var a = new B(); }\n").expect("write fixable");
    // The options file is the fixture's governing posture: options-less
    // enables nothing (#936), so the pinned `builtin-lint` row rides an
    // explicit enable — the same fixture shape the rpc_commands fix test
    // pins.
    fs::write(
        workspace.root.join("analysis_options.yaml"),
        "linter:\n  rules:\n    - unnecessary_new\n",
    )
    .expect("write options");
    let mut h = spawn(&workspace, &store_root("schema-fix"));

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
        assert_eq!(
            files.len(),
            1,
            "the default role answers the hit file alone (the suppression \
             candidates ride their own role spellings): {result}"
        );
        assert_eq!(
            files[0]["file"],
            fixable.display().to_string(),
            "the fixable file's document: {result}"
        );
        assert_valid_at_tier(&files[0], Domain::FixPatch, tier);
        assert_eq!(
            stamp_count(&result),
            2,
            "one `v` stamp per level (the envelope plus the document): {result}"
        );

        let edit = &files[0]["edits"][0];
        assert_eq!(
            edit["src"], "builtin-lint",
            "the provenance tag rides every tier: {edit}"
        );
        if tier == Verbosity::Full {
            assert_eq!(edit["rule"], "unnecessary_new", "the rule id: {edit}");
            assert!(
                !edit["msg"].as_str().expect("msg").is_empty(),
                "the human message: {edit}"
            );
        } else {
            assert!(
                edit.get("rule").is_none()
                    && edit.get("msg").is_none()
                    && edit.get("priority").is_none(),
                "msg/priority/rule are full-only: {edit}"
            );
        }
    }

    // The suppression role keeps the options file's document alone — the
    // hit file's rows are primary/ignore-roled, so its set drops — keeping
    // the cross-file document schema-pinned through its own role: one row
    // per code, each appending the `analyzer: errors:` ignore at the
    // options file's end, provenance-tagged `builtin-ignore`.
    h.step(
        &request(
            5,
            "dartq/fix",
            json!({
                "paths": [fixable.display().to_string()],
                "role": "suppress",
            }),
        ),
        1,
    );
    let result = answer_result(&h, 5);
    let files = result["files"].as_array().expect("files array");
    assert_eq!(
        files.len(),
        1,
        "the governing options file alone under `suppress`: {result}"
    );
    assert_eq!(
        files[0]["file"],
        workspace
            .path("analysis_options.yaml")
            .display()
            .to_string()
    );
    let edits = files[0]["edits"].as_array().expect("edits array");
    assert_eq!(edits.len(), 2, "one suppression row per code");
    assert_eq!(edits[0]["src"], "builtin-ignore");
    assert_eq!(edits[0]["role"], "suppress");
    assert_valid_at_tier(&files[0], Domain::FixPatch, Verbosity::Compact);

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
    fs::remove_dir_all(&workspace.root).ok();
}

/// Every stamped face carries a published domain (#632 closed): the four
/// session faces the original gap named validate live over the broken
/// fixture — `dartq/check` → `Check` and `dartq/analyze` → `Analyze` at
/// every tier (the tier-exact check enforces the minimal red line on the
/// wire, and the broken fixture keeps the row shapes honest),
/// `dartq/assists` → `Assists` at every tier (the fixture carries no
/// plugin manifests, so the document validates at its envelope — the row
/// scene is the plugin dispatch's), and `dartq/index` → `Index`,
/// tier-invariant. Each answer carries exactly one stamp — its producer's
/// own; the daemon assembly adds none.
#[test]
fn every_stamped_face_validates_against_its_published_domain() {
    let workspace = Workspace::new(BROKEN_MAIN);
    let mut h = spawn(&workspace, &store_root("schema-stamped-faces"));

    for (id, tier) in [
        (2, Verbosity::Minimal),
        (3, Verbosity::Compact),
        (4, Verbosity::Full),
    ] {
        h.step(
            &request(
                id,
                "dartq/check",
                json!({ "paths": [workspace.path("lib")], "verbosity": tier.to_string() }),
            ),
            1,
        );
        let result = answer_result(&h, id);
        assert!(
            !result["diags"].as_array().expect("check rows").is_empty(),
            "the broken fixture keeps the row shape honest: {result}"
        );
        let payload = assert_valid_at_tier(&result, Domain::Check, tier);
        // The daemon envelope stamps the compact vocabulary's `v` (the
        // `schema_version` text count is 0); the validator + this count
        // flip with the schema lane's own vocabulary pass.
        assert_eq!(
            payload.matches("\"schema_version\"").count(),
            0,
            "check: {payload}"
        );
    }

    for (id, tier) in [
        (5, Verbosity::Minimal),
        (6, Verbosity::Compact),
        (7, Verbosity::Full),
    ] {
        h.step(
            &request(
                id,
                "dartq/analyze",
                json!({ "verbosity": tier.to_string() }),
            ),
            1,
        );
        let result = answer_result(&h, id);
        assert!(
            !result["diags"]
                .as_array()
                .expect("analyze entries")
                .is_empty(),
            "the analyzed view keeps the entry shape honest: {result}"
        );
        let payload = assert_valid_at_tier(&result, Domain::Analyze, tier);
        assert_eq!(
            payload.matches("\"schema_version\"").count(),
            0,
            "analyze: {payload}"
        );
    }

    h.step(&request(8, "dartq/index", json!({})), 1);
    let result = answer_result(&h, 8);
    let payload = assert_valid_at_tier(&result, Domain::Index, Verbosity::Compact);
    assert_eq!(payload.matches("\"v\"").count(), 1, "index: {payload}");

    for (id, tier) in [
        (9, Verbosity::Minimal),
        (10, Verbosity::Compact),
        (11, Verbosity::Full),
    ] {
        h.step(
            &request(
                id,
                "dartq/assists",
                json!({ "path": workspace.path("lib/main.dart"), "verbosity": tier.to_string() }),
            ),
            1,
        );
        let result = answer_result(&h, id);
        let payload = assert_valid_at_tier(&result, Domain::Assists, tier);
        assert_eq!(payload.matches("\"v\"").count(), 1, "assists: {payload}");
    }

    let (code, _) = h.join_ended();
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
    corrupt["decls"][0]["len"] = json!("three");
    let payload = serde_json::to_string(&corrupt).expect("serializes");
    let violation = validate(&payload, Domain::Query).expect_err("type break must violate");
    assert_eq!(violation.path, "decls[0].len");
    assert_eq!(violation.expected, "an integer");

    // A required array dropped.
    let mut corrupt = compact;
    corrupt
        .as_object_mut()
        .expect("response object")
        .remove("refs");
    let payload = serde_json::to_string(&corrupt).expect("serializes");
    let violation = validate(&payload, Domain::Query).expect_err("dropped array must violate");
    assert_eq!(violation.path, "refs");
    assert_eq!(violation.found, "missing");

    // A minimal answer carrying the compact tier's convenience field.
    let mut corrupt = minimal;
    corrupt["decls"][0]["container"] = json!("Foo");
    let payload = serde_json::to_string(&corrupt).expect("serializes");
    let violation = validate_at_tier(&payload, Domain::Query, Verbosity::Minimal)
        .expect_err("tier leak must violate");
    assert_eq!(violation.path, "decls[0].container");
    assert_eq!(violation.expected, "omitted at the minimal tier");

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
    fs::remove_dir_all(&workspace.root).ok();
}

/// The frozen `dartq/*` method family: the daemon's 17-method request
/// table, pinned here as the independent copy the live face must serve.
/// dartq-schema validates answer shapes per domain and has no method-name
/// registry, so the frozen wire shape is the pin — an added, renamed, or
/// removed method drifts the table and fails the gate below.
const FROZEN_METHODS: [&str; 17] = [
    "dartq/index",
    "dartq/memory",
    "dartq/analyze",
    "dartq/check",
    "dartq/query",
    "dartq/queryBatch",
    "dartq/resolveAt",
    "dartq/completion",
    "dartq/impact",
    "dartq/fix",
    "dartq/assists",
    "dartq/compat",
    "dartq/explain",
    "dartq/rename",
    "dartq/overlay.set",
    "dartq/overlay.commit",
    "dartq/overlay.clear",
];

/// The method-table drift gate: every frozen `dartq/*` name is a live face
/// — the server answers each one (a real result or its own INVALID_PARAMS
/// intake verdict), never `METHOD_NOT_FOUND`. A renamed or dropped method
/// fails on its own line; a stray `dartq/`-spelled unknown still answers
/// `METHOD_NOT_FOUND`, so the face surface is exactly the frozen family.
#[test]
fn every_frozen_method_is_a_live_face() {
    let workspace = seeded_query_workspace();
    let a = workspace.path("lib/a.dart");
    let mut h = spawn(&workspace, &store_root("schema-methods"));

    // Params chosen to reach each face's own intake (a valid selector where
    // the face validates one) — the pin is "answered, never METHOD_NOT_FOUND".
    let paths = json!({ "paths": [a.display().to_string()] });
    let cases: [(i64, &str, Value); 17] = [
        (1, "dartq/index", json!({})),
        (18, "dartq/memory", json!({})),
        (2, "dartq/analyze", json!({})),
        (3, "dartq/check", paths.clone()),
        (4, "dartq/query", json!({ "name": "Foo" })),
        (5, "dartq/queryBatch", json!({ "queries": [] })),
        (
            6,
            "dartq/resolveAt",
            json!({ "file": a.display().to_string(), "offset": 0 }),
        ),
        (
            7,
            "dartq/completion",
            json!({ "file": a.display().to_string(), "offset": 0 }),
        ),
        (
            8,
            "dartq/impact",
            json!({ "path": a.display().to_string() }),
        ),
        (9, "dartq/fix", paths.clone()),
        (
            10,
            "dartq/assists",
            json!({ "path": a.display().to_string(), "offset": 0, "length": 0 }),
        ),
        (
            11,
            "dartq/compat",
            json!({ "path": a.display().to_string(), "sdk_roots": [workspace.path("sdk")] }),
        ),
        (
            12,
            "dartq/explain",
            json!({ "symbol": "Foo", "sdk": workspace.path("sdk") }),
        ),
        (
            13,
            "dartq/rename",
            json!({ "path": a.display().to_string(), "line": 1, "col": 1, "new_name": "gb" }),
        ),
        (
            14,
            "dartq/overlay.set",
            json!({ "agent_id": "A", "files": {} }),
        ),
        (15, "dartq/overlay.commit", json!({ "agent_id": "A" })),
        (16, "dartq/overlay.clear", json!({ "agent_id": "A" })),
    ];
    for (id, method, params) in cases {
        h.step(&request(id, method, params), 1);
        let frame = h.wait_answer(id);
        let code = frame["error"]["code"].as_i64();
        assert_ne!(
            code,
            Some(-32601),
            "{method} must be a live face, not METHOD_NOT_FOUND: {frame}"
        );
        // The all-optional session faces with empty params answer the
        // session-domain miss (NO_SESSIONS), their own verdict — not
        // INVALID_PARAMS.
        assert!(
            frame.get("error").is_none() || code == Some(-32602) || code == Some(-32001),
            "{method} answers a result or its own intake verdict: {frame}"
        );
    }

    // The family is closed: a `dartq/`-spelled unknown answers
    // METHOD_NOT_FOUND, so nothing outside the frozen table answers.
    h.step(&request(17, "dartq/nope", json!({})), 1);
    let frame = h.wait_answer(17);
    assert_eq!(frame["error"]["code"], -32601, "the face set is closed");

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
    fs::remove_dir_all(&workspace.root).ok();
}

/// The gate's symmetric direction: the SERVER's dispatchable set equals the
/// frozen family exactly — the live-face test above proves every frozen
/// name answers (METHODS ⊆ served), this one proves nothing outside the
/// frozen table serves (served ⊆ METHODS ⊆ frozen).
///
/// The server's request path has exactly two ways to serve a name:
/// `handle_request`'s intake-literal arms (the overlay triple and the
/// plugin-dispatch pair) and the `commands::is_command` arm — and the
/// latter is `METHODS.contains`, METHODS-driven, so it can only serve a
/// listed name. That leaves the literal arms as the only place a
/// handler could answer an unlisted name. Those arms match on the
/// `server.rs` constants that `INTAKE_REQUESTS` is built from, so the
/// served intake set IS the published set — asserting it ⊆ `METHODS`
/// closes the hole: a new literal arm serving a fresh name requires adding
/// the constant, and the constant's table entry fails this assertion
/// unless `METHODS` (and therefore the frozen copy) grows with it.
///
/// Notifications are deliberately separate: `dartq/sync`/`dartq/cancel`
/// are notification faces (no `id`, nothing to answer) and are NOT in
/// `METHODS` by design — that table is the request family only. The gate
/// pins them as the closed notification set and asserts the two families
/// stay disjoint.
#[test]
fn the_served_set_is_exactly_the_frozen_family() {
    // Direction 1: every intake-intercepted request name is in the
    // exhaustive request table — no handler serves a name `METHODS`
    // doesn't list.
    for method in dartq_daemon::INTAKE_REQUESTS {
        assert!(
            dartq_daemon::METHODS.contains(&method),
            "{method} is served at intake but missing from METHODS — \
             an unlisted live face the frozen gate cannot see"
        );
    }

    // Direction 2 (the symmetric half the existing live-face test already
    // covers behaviorally): METHODS — the dispatchable request set the
    // `is_command` arm routes — is exactly the frozen family, no extras.
    assert_eq!(
        dartq_daemon::METHODS.len(),
        FROZEN_METHODS.len(),
        "the request family grew or shrank: {METHODS:?}",
        METHODS = dartq_daemon::METHODS
    );
    for method in dartq_daemon::METHODS {
        assert!(
            FROZEN_METHODS.contains(&method),
            "{method} is a live face but not in the frozen table"
        );
    }

    // The notification faces are a parallel closed set: the served
    // notification names are exactly the frozen pair, and none is a request
    // method.
    let served = dartq_daemon::SERVED_NOTIFICATIONS;
    assert_eq!(served, ["dartq/sync", "dartq/cancel"]);
    for notification in served {
        assert!(
            !dartq_daemon::METHODS.contains(&notification),
            "{notification} is both a request and a notification"
        );
    }
}

/// The frozen `dartq/debug/*` family: the daemon-native debug registry's
/// 17-method request table, pinned the same way the analysis family is —
/// an added, renamed, or removed method drifts the table and fails the
/// gate. The family is deliberately NOT part of `METHODS` (that table is
/// the CLI-mirror analysis family; the debug faces target the debug
/// registry, never the analysis engine), so it carries its own frozen
/// copy and its own disjointness pins.
const FROZEN_DEBUG_METHODS: [&str; 17] = [
    "dartq/debug/launch",
    "dartq/debug/attach",
    "dartq/debug/detach",
    "dartq/debug/stop",
    "dartq/debug/status",
    "dartq/debug/breakpointAdd",
    "dartq/debug/breakpointRemove",
    "dartq/debug/exceptionPause",
    "dartq/debug/resume",
    "dartq/debug/pause",
    "dartq/debug/step",
    "dartq/debug/snapshot",
    "dartq/debug/reload",
    "dartq/debug/restart",
    "dartq/debug/evaluate",
    "dartq/debug/events",
    "dartq/debug/output",
];

/// The debug-family drift gate: the served table equals the frozen copy
/// exactly, stays disjoint from the analysis request family and the
/// notification family, and every name is a live face — an empty-params
/// request answers the shared `INVALID_PARAMS` intake verdict (or, for
/// the all-optional `status`, a result), never `METHOD_NOT_FOUND`. The
/// empty-params probe is chosen so no face reaches a live VM: `launch`
/// and `attach` fail parameter decoding before any process or connection
/// exists.
#[test]
fn the_debug_family_matches_the_frozen_table() {
    assert_eq!(
        dartq_daemon::DEBUG_REQUESTS.len(),
        FROZEN_DEBUG_METHODS.len(),
        "the debug request family grew or shrank"
    );
    for method in dartq_daemon::DEBUG_REQUESTS {
        assert!(
            FROZEN_DEBUG_METHODS.contains(&method),
            "{method} is a live debug face but not in the frozen table"
        );
        assert!(
            !dartq_daemon::METHODS.contains(&method),
            "{method} must stay out of the analysis request family"
        );
        assert!(
            !dartq_daemon::SERVED_NOTIFICATIONS.contains(&method),
            "{method} is both a request and a notification"
        );
    }

    let workspace = seeded_query_workspace();
    let mut h = spawn(&workspace, &store_root("schema-debug-methods"));
    for (id, method) in FROZEN_DEBUG_METHODS.iter().enumerate() {
        let id = (id + 1) as i64;
        h.step(&request(id, method, json!({})), 1);
        let frame = h.wait_answer(id);
        let code = frame["error"]["code"].as_i64();
        assert_ne!(
            code,
            Some(-32601),
            "{method} must be a live face, not METHOD_NOT_FOUND: {frame}"
        );
        // The all-optional session faces with empty params answer the
        // session-domain miss (NO_SESSIONS), their own verdict — not
        // INVALID_PARAMS.
        assert!(
            frame.get("error").is_none() || code == Some(-32602) || code == Some(-32001),
            "{method} answers a result or its own intake verdict: {frame}"
        );
    }

    // The family is closed: an unknown debug-spelled name answers
    // METHOD_NOT_FOUND, and `status` over the empty registry answers
    // the always-document shape (#2619 — sessions and orphans both
    // empty is still a printed document, never blank output).
    h.step(&request(99, "dartq/debug/nope", json!({})), 1);
    let frame = h.wait_answer(99);
    assert_eq!(
        frame["error"]["code"], -32601,
        "the debug face set is closed"
    );
    h.step(&request(100, "dartq/debug/status", json!({})), 1);
    assert_eq!(
        h.wait_answer(100)["result"],
        json!({"v": 1, "sessions": [], "orphans": []})
    );

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
    fs::remove_dir_all(&workspace.root).ok();
}

/// The frozen `dartq/test/*` family: the daemon-native test orchestration
/// registry's 5-method request table, pinned the same way the debug family
/// is — an added, renamed, or removed method drifts the table and fails the
/// gate. Like the debug faces it stays out of `METHODS` (that table is the
/// analysis family; the test faces target the run registry, never the
/// analysis engine), so it carries its own frozen copy and disjointness
/// pins.
const FROZEN_TEST_METHODS: [&str; 5] = [
    "dartq/test/run",
    "dartq/test/status",
    "dartq/test/events",
    "dartq/test/cancel",
    "dartq/test/probe",
];

/// The test-family drift gate: the served table equals the frozen copy
/// exactly, stays disjoint from the analysis request family and the
/// notification family, and every name is a live face — an empty-params
/// request answers the shared `INVALID_PARAMS` intake verdict (or, for the
/// all-optional `status`/`probe`, a result), never `METHOD_NOT_FOUND`. The
/// empty-params probe is chosen so no face reaches a live toolchain: `run`
/// fails parameter decoding (or resolves an empty file set) before any
/// process spawns.
#[test]
fn the_test_family_matches_the_frozen_table() {
    assert_eq!(
        dartq_daemon::TEST_REQUESTS.len(),
        FROZEN_TEST_METHODS.len(),
        "the test request family grew or shrank"
    );
    for method in dartq_daemon::TEST_REQUESTS {
        assert!(
            FROZEN_TEST_METHODS.contains(&method),
            "{method} is a live test face but not in the frozen table"
        );
        assert!(
            !dartq_daemon::METHODS.contains(&method),
            "{method} must stay out of the analysis request family"
        );
        assert!(
            !dartq_daemon::SERVED_NOTIFICATIONS.contains(&method),
            "{method} is both a request and a notification"
        );
    }

    let workspace = seeded_query_workspace();
    let mut h = spawn(&workspace, &store_root("schema-test-methods"));
    for (id, method) in FROZEN_TEST_METHODS.iter().enumerate() {
        let id = (id + 1) as i64;
        h.step(&request(id, method, json!({})), 1);
        let frame = h.wait_answer(id);
        let code = frame["error"]["code"].as_i64();
        assert_ne!(
            code,
            Some(-32601),
            "{method} must be a live face, not METHOD_NOT_FOUND: {frame}"
        );
    }

    // The family is closed: an unknown test-spelled name answers
    // METHOD_NOT_FOUND.
    h.step(&request(99, "dartq/test/nope", json!({})), 1);
    let frame = h.wait_answer(99);
    assert_eq!(
        frame["error"]["code"], -32601,
        "the test face set is closed"
    );

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
    fs::remove_dir_all(&workspace.root).ok();
}
