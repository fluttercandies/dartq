//! End-to-end LSP session tests over an in-memory pipe: a real runtime
//! context on a throwaway workspace, a scripted client pacing the server
//! one turn at a time (or bursting a batch), the transcript asserted frame
//! by frame. No real process spawns.

mod common;

use std::fs;

use common::{
    BROKEN_MAIN, CLEAN_MAIN, Workspace, answered, framed, notification, publishes, request, spawn,
    store_root,
};
use serde_json::{Value, json};

#[test]
fn full_session_overlay_and_diagnostics() {
    let workspace = Workspace::new(BROKEN_MAIN);
    let uri = dartq_daemon::uri::from_path(&workspace.path("lib/main.dart"));
    let mut h = spawn(&workspace, &store_root("full-session"));

    h.step(
        &request(1, "initialize", json!({"processId": null, "rootUri": null})),
        1,
    );
    h.step(&notification("initialized", json!({})), 1);
    // didOpen with the on-disk text: applies, but the diff baseline holds —
    // no push (the step settles through a probe).
    h.step(
        &notification(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": uri, "languageId": "dart", "version": 1,
                   "text": BROKEN_MAIN}}),
        ),
        0,
    );
    h.step(
        &notification(
            "textDocument/didChange",
            json!({"textDocument": {"uri": uri, "version": 2},
                   "contentChanges": [{"text": CLEAN_MAIN}]}),
        ),
        1,
    );
    h.step(&request(2, "dartq/analyze", json!({})), 1);
    h.step(
        &notification(
            "textDocument/didClose",
            json!({"textDocument": {"uri": uri}}),
        ),
        1,
    );
    h.step(&request(3, "shutdown", json!(null)), 1);
    let (code, transcript) = h.exit();

    assert_eq!(code, dartq_daemon::EXIT_CLEAN, "shutdown+exit exits 0");

    // initialize answered with the negotiated capabilities.
    let answered_frames = answered(&transcript);
    assert_eq!(answered_frames[0]["id"], 1);
    assert_eq!(
        answered_frames[0]["result"]["capabilities"]["textDocumentSync"],
        1
    );

    // The `initialized` sweep publishes the seeded disk error; didOpen with
    // identical text must NOT re-publish (the diff baseline holds).
    let pushes = publishes(&transcript);
    assert_eq!(
        pushes.len(),
        3,
        "exactly: sweep push, didChange clear push, didClose restore push"
    );
    assert_eq!(pushes[0]["params"]["uri"], uri);
    assert_eq!(pushes[0]["params"]["version"], Value::Null);
    let initial = &pushes[0]["params"]["diagnostics"];
    assert!(
        initial
            .as_array()
            .expect("array")
            .iter()
            .any(|d| d["code"] == "uri_does_not_exist"),
        "sweep reports the seeded error: {initial}"
    );

    // The didChange publish cleared the diagnostics under the client's
    // version, and the dartq/analyze answer (which drains after the write)
    // agrees.
    assert_eq!(pushes[1]["params"]["uri"], uri);
    assert_eq!(pushes[1]["params"]["version"], 2);
    assert_eq!(pushes[1]["params"]["diagnostics"], json!([]));
    let analyze = answered_frames
        .iter()
        .find(|frame| frame["id"] == 2)
        .expect("analyze answer");
    assert_eq!(analyze["result"]["diagnostics"], json!([]));

    // didClose restored the disk content: the error is back, unversioned.
    assert_eq!(pushes[2]["params"]["uri"], uri);
    assert_eq!(pushes[2]["params"]["version"], Value::Null);
    let restored = &pushes[2]["params"]["diagnostics"];
    assert!(
        restored
            .as_array()
            .expect("array")
            .iter()
            .any(|d| d["code"] == "uri_does_not_exist"),
        "didClose falls back to disk state: {restored}"
    );

    // shutdown answers null.
    let shutdown = answered_frames
        .iter()
        .find(|frame| frame["id"] == 3)
        .expect("shutdown answer");
    assert_eq!(shutdown["result"], Value::Null);
    assert_eq!(shutdown.get("error"), None);
}

/// An overlay RPC change reflects in the subsequent diagnostics while the
/// workspace file on disk — and the snapshot cache — stay untouched (PRD
/// §16.3: writes never leave the overlay).
#[test]
fn overlay_change_reflects_in_diagnostics_and_never_touches_disk() {
    let workspace = Workspace::new(BROKEN_MAIN);
    let uri = dartq_daemon::uri::from_path(&workspace.path("lib/main.dart"));
    let main_dart = workspace.path("lib/main.dart");
    let root = store_root("overlay-no-disk");
    let mut h = spawn(&workspace, &root);

    h.step(&request(1, "initialize", json!({})), 1);
    h.step(&notification("initialized", json!({})), 1);
    h.step(
        &notification(
            "textDocument/didChange",
            json!({"textDocument": {"uri": uri, "version": 2},
                   "contentChanges": [{"text": CLEAN_MAIN}]}),
        ),
        1,
    );
    h.step(&request(2, "dartq/analyze", json!({})), 1);
    h.step(&request(9, "shutdown", json!(null)), 1);
    let (code, transcript) = h.exit();

    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
    let answered_frames = answered(&transcript);
    let pushes = publishes(&transcript);
    assert_eq!(pushes.len(), 2, "sweep push + change clear push");
    assert_eq!(pushes[1]["params"]["version"], 2);
    assert_eq!(pushes[1]["params"]["diagnostics"], json!([]));
    let analyze = answered_frames
        .iter()
        .find(|frame| frame["id"] == 2)
        .expect("analyze answer");
    assert_eq!(analyze["result"]["diagnostics"], json!([]));

    // The workspace file kept its on-disk content: the overlay never wrote.
    assert_eq!(
        fs::read_to_string(&main_dart).expect("disk read"),
        BROKEN_MAIN
    );
    // The accelerator wrote nothing: no snapshot generation exists (the
    // debounced save is blocked on engine faces; see Session docs).
    assert!(!root.join("snapshots").exists(), "no generation is written");
}

#[test]
fn lifecycle_gates_and_exit_without_shutdown() {
    let workspace = Workspace::new(CLEAN_MAIN);
    let mut h = spawn(&workspace, &store_root("lifecycle"));

    // Any request before initialize is ServerNotInitialized.
    h.step(&request(1, "shutdown", json!(null)), 1);
    // Still uninitialized: the dropped didOpen and unknown notification
    // settle through probes.
    h.step(
        &notification(
            "textDocument/didOpen",
            json!({"textDocument": {
                "uri": "file:///x.dart", "languageId": "dart", "version": 1, "text": ""
            }}),
        ),
        0,
    );
    h.step(&notification("some/unknown", json!({})), 0);
    h.step(&request(2, "initialize", json!({})), 1);
    h.step(&request(3, "no/such/method", json!({})), 1);
    // exit without shutdown: the process exits 1 (LSP contract).
    let (code, transcript) = h.exit();

    assert_eq!(code, dartq_daemon::EXIT_WITHOUT_SHUTDOWN);
    let answered_frames = answered(&transcript);
    assert_eq!(
        answered_frames.len(),
        3,
        "three requests, three answers: {answered_frames:?}"
    );
    assert_eq!(answered_frames[0]["error"]["code"], -32002);
    assert_eq!(answered_frames[1]["id"], 2);
    assert_eq!(
        answered_frames[1]["result"]["capabilities"]["textDocumentSync"],
        1
    );
    assert_eq!(answered_frames[2]["error"]["code"], -32601);
    // The dropped didOpen/unknown notifications produced no pushes.
    assert!(publishes(&transcript).is_empty());
}

#[test]
fn unparsable_body_answers_and_session_continues() {
    let workspace = Workspace::new(CLEAN_MAIN);
    let mut h = spawn(&workspace, &store_root("unparsable"));

    h.send_raw(b"Content-Length: 8\r\n\r\nnot json");
    h.step(&request(1, "initialize", json!({})), 1);
    h.step(&request(2, "shutdown", json!(null)), 1);
    let (code, transcript) = h.exit();

    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
    let parse_error = transcript
        .iter()
        .find(|frame| frame["error"]["code"] == -32700)
        .expect("parse error answered");
    assert_eq!(parse_error["id"], Value::Null);
    let answered_frames = answered(&transcript);
    assert_eq!(answered_frames.len(), 2, "initialize + shutdown");
    assert_eq!(
        answered_frames[0]["result"]["capabilities"]["textDocumentSync"],
        1
    );
    assert_eq!(
        answered_frames[1]["result"],
        Value::Null,
        "shutdown answers null"
    );
}

#[test]
fn broken_framing_ends_the_session() {
    let workspace = Workspace::new(CLEAN_MAIN);
    let mut h = spawn(&workspace, &store_root("broken-framing"));

    // No Content-Length: the stream cannot be resynchronized. The server
    // answers ParseError and stops reading — the next frame is never
    // processed.
    h.send_raw(b"X-Drop: it\r\n\r\norphan body");
    h.send_raw(&framed(&request(1, "initialize", json!({}))));
    let (code, transcript) = h.join_ended();

    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
    assert_eq!(transcript.len(), 1, "one answer, then the session ends");
    assert_eq!(transcript[0]["error"]["code"], -32700);
}
