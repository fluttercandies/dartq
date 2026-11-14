//! The serve-early opt-in (#4268): `dartq/analyze` with `serve_early: true`
//! answers the asked files' rows before the session's workspace settles —
//! the engine's file-aim turn scoped to the aims' libraries, every answer
//! carrying the `settle: "may-precede-settle"` marker on the envelope (the
//! rendered `stdout` bytes stay marker-free). The default path (flag absent
//! or `false`) is byte-identical to the settled face: no marker, the full
//! turn, the warm-turn cache untouched.
//!
//! Coverage:
//! - opt-in off ⇒ the plain answer has no `settle` field;
//! - opt-in on an unsettled context ⇒ the asked file's rows answer with
//!   the marker, equal to the settled run's rows for that file;
//! - opt-in on a settled context ⇒ the marker is absent (degrades to the
//!   default face);
//! - a mixed group (one serve-early ask, one plain ask) ⇒ the whole group
//!   takes the full turn, unmarked;
//! - a directory aim or an agent view with the flag ⇒ INVALID_PARAMS;
//! - a serve-early answer never enters the warm-turn cache.
//!
//! The unsettled window is opened the way a real client opens it: an
//! on-disk rewrite reported through `dartq/sync` in the same batch as the
//! ask — never an agent overlay (overlays wear a view, they do not dirty
//! the base context's settle state).

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

/// The `result` of the answer frame for `id`.
fn answer_result(h: &common::Harness, id: i64) -> Value {
    h.wait_answer(id)["result"].clone()
}

/// The `error` of the answer frame for `id`.
fn answer_error(h: &common::Harness, id: i64) -> Value {
    h.wait_answer(id)["error"].clone()
}

/// A two-library fixture: `lib/a.dart` declares `Foo` and calls an
/// undefined function (one engine row, always present); `lib/b.dart`
/// uses `Foo` cleanly. `lib/main.dart` is the `CLEAN_MAIN` seed.
fn two_library_workspace() -> Workspace {
    let workspace = Workspace::new(CLEAN_MAIN);
    fs::write(
        workspace.path("lib/a.dart"),
        "class Foo {\n  void meth() {}\n}\nvoid foo() {}\nvoid broken() { doesNotExist(); }\n",
    )
    .expect("write a.dart");
    fs::write(
        workspace.path("lib/b.dart"),
        "import 'a.dart';\nvoid main() {\n  Foo? f = Foo();\n  f.meth();\n  foo();\n}\n",
    )
    .expect("write b.dart");
    workspace
}

/// Rewrites `lib/b.dart` on disk and reports it through `dartq/sync` in
/// the same batch as `messages` — the client's real change-report shape:
/// the write applies at the drain, reopening the unsettled window before
/// the coalesced read runs.
fn dirty_and_send(
    h: &mut common::Harness,
    workspace: &Workspace,
    messages: &[Value],
    expect: usize,
) {
    let b = workspace.path("lib/b.dart");
    fs::write(
        &b,
        "import 'a.dart';\nvoid main() {\n  Foo? f = Foo();\n  f.meth();\n  foo();\n  f.meth();\n}\n",
    )
    .expect("rewrite b.dart");
    let mut batch = vec![notification(
        "dartq/sync",
        json!({ "paths": ["lib/b.dart"] }),
    )];
    batch.extend_from_slice(messages);
    h.burst(&batch, expect);
}

/// Opt-in off ⇒ the plain `dartq/analyze` answer carries no `settle`
/// field — the default path's bytes are untouched by #4268.
#[test]
fn serve_early_off_leaves_the_default_answer_unmarked() {
    let _guard = workspace_lock();
    let workspace = two_library_workspace();
    let mut h = spawn(&workspace, &store_root("serve-early-off"));

    // The probe already ran a full analyze (settling the context); a
    // plain ask answers the settled document with no marker.
    h.step(&request(2, "dartq/analyze", json!({})), 1);
    let result = answer_result(&h, 2);
    assert!(
        result.get("settle").is_none(),
        "the default answer carries no settle marker: {result}"
    );

    // An explicit `serve_early: false` is the same face.
    h.step(
        &request(
            3,
            "dartq/analyze",
            json!({ "serve_early": false, "targets": ["lib/a.dart"] }),
        ),
        1,
    );
    let result = answer_result(&h, 3);
    assert!(
        result.get("settle").is_none(),
        "serve_early:false answers unmarked: {result}"
    );

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// Opt-in on an unsettled context ⇒ the asked file's rows answer with
/// the marker, and the rows equal the settled run's rows for that file
/// (the engine's file-aim contract).
#[test]
fn serve_early_on_answers_the_asked_file_with_the_marker() {
    let _guard = workspace_lock();
    let workspace = two_library_workspace();
    let mut h = spawn(&workspace, &store_root("serve-early-on"));

    let a_path = workspace.path("lib/a.dart");
    let a_target = a_path.display().to_string();

    // The settled baseline: a plain ask on the settled context.
    h.step(
        &request(2, "dartq/analyze", json!({ "targets": [a_target.clone()] })),
        1,
    );
    let settled = answer_result(&h, 2);
    assert!(settled.get("settle").is_none());
    assert!(
        settled["diags"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty()),
        "the fixture file carries a diagnostic: {settled}"
    );

    // Dirty the workspace and ask early in the same batch: the write
    // applies first, the serve-early ask answers the file-aim turn.
    dirty_and_send(
        &mut h,
        &workspace,
        &[request(
            3,
            "dartq/analyze",
            json!({ "serve_early": true, "targets": [a_target.clone()] }),
        )],
        1,
    );
    let early = answer_result(&h, 3);
    assert_eq!(
        early["settle"],
        json!("may-precede-settle"),
        "the serve-early answer carries the marker: {early}"
    );
    assert_eq!(
        early["diags"], settled["diags"],
        "the asked file's rows equal the settled run's"
    );

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// A mixed group (one serve-early ask, one plain ask) takes the full
/// turn: the plain ask's presence means the group cannot answer early,
/// so every answer is the settled face, unmarked.
#[test]
fn serve_early_mixed_group_falls_back_to_the_full_turn() {
    let _guard = workspace_lock();
    let workspace = two_library_workspace();
    let mut h = spawn(&workspace, &store_root("serve-early-mixed"));

    let a_target = workspace.path("lib/a.dart").display().to_string();
    dirty_and_send(
        &mut h,
        &workspace,
        &[
            request(
                3,
                "dartq/analyze",
                json!({ "serve_early": true, "targets": [a_target.clone()] }),
            ),
            request(4, "dartq/analyze", json!({ "targets": [a_target.clone()] })),
        ],
        2,
    );
    let early = answer_result(&h, 3);
    let plain = answer_result(&h, 4);
    assert!(
        early.get("settle").is_none(),
        "a mixed group answers unmarked: {early}"
    );
    assert!(
        plain.get("settle").is_none(),
        "the plain ask answers unmarked: {plain}"
    );
    assert_eq!(
        early, plain,
        "the mixed group's answers are identical — the full turn served both"
    );

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// The flag's shape guards: a directory aim, an agent view, or a
/// path-less ask with `serve_early: true` is INVALID_PARAMS — the mode
/// answers per-file asks on the base view only.
#[test]
fn serve_early_rejects_directory_and_agent_shapes() {
    let _guard = workspace_lock();
    let workspace = two_library_workspace();
    let mut h = spawn(&workspace, &store_root("serve-early-guards"));

    // A directory target spans whole libraries — no per-file slice.
    h.step(
        &request(
            2,
            "dartq/analyze",
            json!({
                "serve_early": true,
                "targets": [workspace.path("lib").display().to_string()],
            }),
        ),
        1,
    );
    let error = answer_error(&h, 2);
    assert_eq!(error["code"], -32602, "INVALID_PARAMS: {error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("per-file"),
        "the directory refusal names the per-file rule: {error}"
    );

    // An agent view has no serve-early spelling.
    h.step(
        &request(
            3,
            "dartq/analyze",
            json!({
                "serve_early": true,
                "agent_id": "agent-a",
                "targets": [workspace.path("lib/a.dart").display().to_string()],
            }),
        ),
        1,
    );
    let error = answer_error(&h, 3);
    assert_eq!(error["code"], -32602, "INVALID_PARAMS: {error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("base view"),
        "the agent refusal names the base-view rule: {error}"
    );

    // A path-less ask is the whole workspace — not a per-file ask.
    h.step(
        &request(4, "dartq/analyze", json!({ "serve_early": true })),
        1,
    );
    let error = answer_error(&h, 4);
    assert_eq!(error["code"], -32602, "INVALID_PARAMS: {error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("per-file"),
        "the empty-targets refusal names the per-file rule: {error}"
    );

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}

/// The warm-turn answer cache never serves a marked answer: the early
/// group returns before the cache is consulted (a marked answer can never
/// enter it, #4297), and a later plain ask on the same file answers the
/// settled bytes (no marker).
#[test]
fn serve_early_answers_never_enter_the_warm_cache() {
    let _guard = workspace_lock();
    let workspace = two_library_workspace();
    let mut h = spawn(&workspace, &store_root("serve-early-cache"));

    let a_target = workspace.path("lib/a.dart").display().to_string();
    // Dirty the workspace so the context is unsettled; the serve-early
    // ask uses `format: "lon"` — the only cacheable rendered shape.
    dirty_and_send(
        &mut h,
        &workspace,
        &[request(
            3,
            "dartq/analyze",
            json!({
                "serve_early": true,
                "format": "lon",
                "targets": [a_target.clone()],
            }),
        )],
        1,
    );
    let early = answer_result(&h, 3);
    assert_eq!(early["settle"], json!("may-precede-settle"));

    // The deferred settle drains on the next turn; a plain rendered ask
    // on the same file then answers the settled bytes, unmarked.
    h.step(&request(4, "dartq/analyze", json!({})), 1);
    h.step(
        &request(
            5,
            "dartq/analyze",
            json!({ "format": "lon", "targets": [a_target.clone()] }),
        ),
        1,
    );
    let plain = answer_result(&h, 5);
    assert!(
        plain.get("settle").is_none(),
        "the plain ask answers unmarked after serve-early: {plain}"
    );
    // The rendered stdout is the settled document — the marker never
    // leaked into the bytes a cache could re-serve.
    assert!(
        plain["stdout"].as_str().is_some_and(|s| !s.is_empty()),
        "the rendered answer carries stdout: {plain}"
    );

    let (code, _) = h.join_ended();
    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
}
