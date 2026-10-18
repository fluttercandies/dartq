//! End-to-end RPC session tests over an in-memory pipe: a real runtime
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

/// The resident loop's whole shape: the startup sweep pushes the seeded
/// diagnostics before the client speaks, a `dartq/sync` change-report
/// re-analyzes the touched path and pushes the fresh set, a pull request
/// answers the same view, and EOF ends the session cleanly.
#[test]
fn full_session_sync_and_diagnostics() {
    let workspace = Workspace::new(BROKEN_MAIN);
    let uri = dartq_daemon::uri::from_path(&workspace.path("lib/main.dart"));
    let main_dart = workspace.path("lib/main.dart");
    let mut h = spawn(&workspace, &store_root("full-session"));

    // The sweep's push already landed (the harness settled the startup
    // drain); it reports the seeded error, unversioned — pushes carry no
    // version on this face.
    fs::write(&main_dart, CLEAN_MAIN).expect("rewrite");
    h.step(
        &notification("dartq/sync", json!({"paths": ["lib/main.dart"]})),
        1,
    );
    h.step(&request(2, "dartq/analyze", json!({})), 1);
    let (code, transcript) = h.join_ended();

    assert_eq!(code, dartq_daemon::EXIT_CLEAN, "EOF exits 0");

    // Two pushes: the sweep's seeded set, then the sync's clear. A pure
    // diff against the baseline — the clean set pushes once and stays
    // suppressed afterward.
    let pushes = publishes(&transcript);
    assert_eq!(pushes.len(), 2, "sweep + sync clear: {pushes:?}");
    assert_eq!(pushes[0]["params"]["uri"], uri);
    let initial = &pushes[0]["params"]["diagnostics"];
    assert!(
        initial
            .as_array()
            .expect("array")
            .iter()
            .any(|d| d["code"] == "uri_does_not_exist"),
        "sweep reports the seeded error: {initial}"
    );
    assert_eq!(pushes[1]["params"]["uri"], uri);
    assert_eq!(pushes[1]["params"]["diagnostics"], json!([]));

    // The pull face drains after the write and answers the same clean view.
    let analyze = answered(&transcript)
        .into_iter()
        .find(|frame| frame["id"] == 2)
        .expect("analyze answer");
    assert_eq!(analyze["result"]["diagnostics"], json!([]));
}

/// A `dartq/sync` report reflects in the subsequent diagnostics while the
/// base view on disk is what the file actually holds — the workspace file
/// is the only truth, and the accelerator's snapshot store absorbs the
/// re-analysis.
#[test]
fn sync_change_reflects_in_diagnostics_and_updates_the_snapshot() {
    let workspace = Workspace::new(BROKEN_MAIN);
    let main_dart = workspace.path("lib/main.dart");
    let root = store_root("sync-snapshot");
    let mut h = spawn(&workspace, &root);

    fs::write(&main_dart, CLEAN_MAIN).expect("rewrite");
    h.step(
        &notification("dartq/sync", json!({"paths": ["lib/main.dart"]})),
        1,
    );
    h.step(&request(2, "dartq/analyze", json!({})), 1);
    let (code, transcript) = h.join_ended();

    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
    let pushes = publishes(&transcript);
    assert_eq!(pushes.len(), 2, "sweep push + the sync's clear push");
    assert_eq!(pushes[1]["params"]["diagnostics"], json!([]));
    let analyze = answered(&transcript)
        .into_iter()
        .find(|frame| frame["id"] == 2)
        .expect("analyze answer");
    assert_eq!(analyze["result"]["diagnostics"], json!([]));

    // The workspace file holds exactly what the test wrote — the sync
    // read it, never rewrote it.
    assert_eq!(
        fs::read_to_string(&main_dart).expect("disk read"),
        CLEAN_MAIN
    );
    // The write task marked the snapshot due: the session-end flush wrote
    // one generation under the store root.
    let generations = fs::read_dir(root.join("snapshots"))
        .expect("snapshots dir")
        .count();
    assert!(generations >= 1, "a generation is written");
}

/// There is no lifecycle: a request ahead of anything else answers
/// normally, unknown notifications drop silently, an unknown method
/// answers `METHOD_NOT_FOUND`, and EOF on a quiet session is a clean end.
#[test]
fn no_lifecycle_and_eof_is_clean() {
    let workspace = Workspace::new(CLEAN_MAIN);
    let mut h = spawn(&workspace, &store_root("no-lifecycle"));

    // Unknown notifications settle through probes (no frame).
    h.step(&notification("some/unknown", json!({})), 0);
    h.step(&request(1, "no/such/method", json!({})), 1);
    let (code, transcript) = h.join_ended();

    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
    let answered_frames = answered(&transcript);
    let unknown = answered_frames
        .iter()
        .find(|frame| frame["id"] == 1)
        .expect("the unknown-method answer");
    assert_eq!(unknown["error"]["code"], -32601);
    // A clean workspace sweeps to an empty baseline: the unknown
    // notification produced no push.
    assert!(
        publishes(&transcript).is_empty(),
        "nothing publishes on a clean workspace: {transcript:?}"
    );
}

#[test]
fn unparsable_body_answers_and_session_continues() {
    let workspace = Workspace::new(CLEAN_MAIN);
    let mut h = spawn(&workspace, &store_root("unparsable"));

    h.send_raw(b"Content-Length: 8\r\n\r\nnot json");
    h.step(&request(1, "dartq/analyze", json!({})), 1);
    let (code, transcript) = h.join_ended();

    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
    let parse_error = transcript
        .iter()
        .find(|frame| frame["error"]["code"] == -32700)
        .expect("parse error answered");
    assert_eq!(parse_error["id"], Value::Null);
    let answered_frames = answered(&transcript);
    assert_eq!(answered_frames.len(), 1, "the analyze answer");
    assert!(
        answered_frames[0]["result"]["diagnostics"].is_array(),
        "the session continued past the bad frame: {:?}",
        answered_frames[0]
    );
}

#[test]
fn broken_framing_ends_the_session() {
    let workspace = Workspace::new(CLEAN_MAIN);
    let mut h = spawn(&workspace, &store_root("broken-framing"));

    // No Content-Length: the stream cannot be resynchronized. The server
    // answers ParseError and stops reading — the next frame is never
    // processed, and the exit code says "protocol violation" (#947 F10),
    // never a clean close.
    h.send_raw(b"X-Drop: it\r\n\r\norphan body");
    h.send_raw(&framed(&request(1, "dartq/analyze", json!({}))));
    let (code, transcript) = h.join_ended();

    assert_eq!(code, dartq_daemon::EXIT_PROTOCOL);
    // The spawn baseline's settle probe (an id >= 1000 harness artifact)
    // may drain ahead of the broken frame; the contract pins the
    // client-driven faces: the broken frame's best-effort ParseError is
    // the final word and the piped `dartq/analyze` (id 1) was never
    // processed (`answered` counts client ids only, below the probe
    // range).
    assert_eq!(
        transcript.last().expect("the ParseError answered")["error"]["code"],
        -32700,
        "the best-effort ParseError answer: {transcript:?}"
    );
    assert!(
        answered(&transcript).is_empty(),
        "the next frame is never processed: {transcript:?}"
    );
}

/// A frame truncated mid-body (headers delivered, body short) ends the
/// session on the protocol exit, not a clean close (#947 F10): supervision
/// must distinguish a broken client from an orderly one.
#[test]
fn truncated_frame_body_ends_with_protocol_exit() {
    let workspace = Workspace::new(CLEAN_MAIN);
    let mut h = spawn(&workspace, &store_root("truncated-body"));

    h.send_raw(b"Content-Length: 10\r\n\r\nshort");
    let (code, transcript) = h.join_ended();

    assert_eq!(code, dartq_daemon::EXIT_PROTOCOL);
    // The spawn baseline's settle probe (an id >= 1000 harness artifact)
    // may drain ahead of the truncated frame; the contract pins the
    // client-driven face: the best-effort ParseError is the final word
    // and nothing else was ever answered.
    assert_eq!(
        transcript.last().expect("the ParseError answered")["error"]["code"],
        -32700,
        "the best-effort ParseError answer: {transcript:?}"
    );
    assert!(
        answered(&transcript).is_empty(),
        "the truncated frame answers nothing else: {transcript:?}"
    );
}

/// The burst path's first use: `dartq/sync` + `dartq/analyze` +
/// `dartq/cancel` in ONE batch through the real server loop (the
/// `try_recv` batching window in server.rs).
/// The cancelled request answers -32800 precisely, and the sync's clear
/// publish still rides the re-stamped read — the client gets it in the
/// same turn.
#[test]
fn burst_sync_analyze_cancel_still_publishes() {
    let workspace = Workspace::new(BROKEN_MAIN);
    let main_dart = workspace.path("lib/main.dart");
    let mut h = spawn(&workspace, &store_root("burst-sync-cancel"));

    fs::write(&main_dart, CLEAN_MAIN).expect("rewrite");
    h.burst(
        &[
            notification("dartq/sync", json!({"paths": ["lib/main.dart"]})),
            request(2, "dartq/analyze", json!({})),
            notification("dartq/cancel", json!({"id": 2})),
        ],
        2,
    );
    let (code, transcript) = h.join_ended();

    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
    let pushes = publishes(&transcript);
    assert_eq!(pushes.len(), 2, "sweep push + the batch's clear push");
    assert_eq!(pushes[1]["params"]["diagnostics"], json!([]));
    let answered_frames = answered(&transcript);
    let analyze = answered_frames
        .iter()
        .find(|frame| frame["id"] == 2)
        .expect("analyze answer");
    assert_eq!(
        analyze["error"]["code"], -32800,
        "cancelled precisely: {analyze}"
    );
}

/// Two analyses coalesce onto one read; a cancel in the same batch
/// partitions it per id: the cancelled id answers -32800 while its co-rider
/// answers Ok with the real diagnostics (and nothing changed, so no push).
#[test]
fn burst_two_requests_one_cancel_partitions() {
    let workspace = Workspace::new(BROKEN_MAIN);
    let mut h = spawn(&workspace, &store_root("burst-partition"));

    h.burst(
        &[
            request(2, "dartq/analyze", json!({})),
            request(3, "dartq/analyze", json!({})),
            notification("dartq/cancel", json!({"id": 2})),
        ],
        2,
    );
    let (code, transcript) = h.join_ended();

    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
    assert_eq!(
        publishes(&transcript).len(),
        1,
        "only the sweep push: nothing changed in the batch"
    );
    let answered_frames = answered(&transcript);
    let cancelled = answered_frames
        .iter()
        .find(|frame| frame["id"] == 2)
        .expect("cancelled answer");
    assert_eq!(cancelled["error"]["code"], -32800);
    let survivor = answered_frames
        .iter()
        .find(|frame| frame["id"] == 3)
        .expect("survivor answer");
    assert!(
        survivor["result"]["diagnostics"]
            .as_array()
            .expect("array")
            .iter()
            .any(|d| d["code"] == "uri_does_not_exist"),
        "the survivor got the real analysis: {survivor}"
    );
}

/// Cancelling an unknown id inside a batch is ignored: the coalesced
/// request still answers normally, with the seeded diagnostics.
#[test]
fn burst_unknown_id_cancel_is_ignored() {
    let workspace = Workspace::new(BROKEN_MAIN);
    let mut h = spawn(&workspace, &store_root("burst-unknown-cancel"));

    h.burst(
        &[
            request(2, "dartq/analyze", json!({})),
            notification("dartq/cancel", json!({"id": 99})),
        ],
        1,
    );
    let (code, transcript) = h.join_ended();

    assert_eq!(code, dartq_daemon::EXIT_CLEAN);
    assert_eq!(publishes(&transcript).len(), 1, "only the sweep push");
    let analyze = answered(&transcript)
        .into_iter()
        .find(|frame| frame["id"] == 2)
        .expect("analyze answer");
    assert!(
        analyze.get("error").is_none(),
        "the cancel never landed: {analyze}"
    );
    assert!(
        analyze["result"]["diagnostics"]
            .as_array()
            .expect("array")
            .iter()
            .any(|d| d["code"] == "uri_does_not_exist"),
        "real diagnostics: {analyze}"
    );
}
