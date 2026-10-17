//! Run-to-run session-transcript determinism: one multi-edit session
//! driven twice from an identical initial state — the same workspace
//! bytes, a fresh session, a fresh snapshot store — must answer a
//! byte-identical transcript: every response and every
//! `dartq/diagnostics` push, in arrival order. Internal ids and
//! scheduling-dependent orderings may vary between runs; the serialized
//! transcript bytes may not. No network, no dart SDK.

mod common;

use std::fs;
use std::sync::Mutex;

use common::{BROKEN_MAIN, Workspace, notification, publishes, request, spawn, store_root};
use serde_json::{Value, json};

/// The tests in this binary serialize on the workspace: both runs share
/// one workspace tree (byte-identical output requires byte-identical
/// paths), so a parallel test's workspace re-create must never yank it
/// mid-session.
static WORKSPACE: Mutex<()> = Mutex::new(());

fn workspace_lock() -> std::sync::MutexGuard<'static, ()> {
    WORKSPACE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The edit sequence: three disk edits, each reported through
/// `dartq/sync` (the re-analysis face) and answered by one explicit
/// `dartq/analyze`. The pushed rows change at every step (a different
/// missing URI, the cleared file, the error's return), so the publish
/// face rides the same sequence.
const EDITS: [&str; 3] = [
    "import 'gone.dart';\nvoid main() {}\n",
    "void main() {}\n",
    "import 'back.dart';\nvoid main() {}\n",
];

/// One session's full transcript over the script: the startup drain's
/// baseline, then (disk edit → `dartq/sync` → `dartq/analyze`) per edit,
/// then EOF. The runs differ only in the snapshot-store root's name.
fn drive_session(workspace: &Workspace, main_dart: &std::path::Path, run: usize) -> Vec<Value> {
    let mut h = spawn(workspace, &store_root(&format!("transcript-pin-{run}")));
    for (index, edit) in EDITS.iter().enumerate() {
        fs::write(main_dart, edit).expect("edit lands");
        // The sync's push (when the rows changed) lands inside this turn;
        // a frame-less settle goes through the probe answer.
        h.step(
            &notification("dartq/sync", json!({"paths": ["lib/main.dart"]})),
            0,
        );
        h.step(&request(100 + index as i64, "dartq/analyze", json!({})), 1);
    }
    let (code, transcript) = h.join_ended();
    assert_eq!(code, 0, "the session ends cleanly");
    transcript
}

/// The first frame index where the two transcripts diverge, for the pin's
/// failure message.
fn first_divergence(a: &[Value], b: &[Value]) -> Option<usize> {
    a.iter()
        .zip(b.iter())
        .position(|(x, y)| x != y)
        .or_else(|| {
            if a.len() == b.len() {
                None
            } else {
                Some(a.len().min(b.len()))
            }
        })
}

#[test]
fn multi_edit_session_transcript_is_byte_identical_across_runs() {
    let _guard = workspace_lock();
    let workspace = Workspace::new(BROKEN_MAIN);
    let main_dart = workspace.path("lib/main.dart");

    let first = drive_session(&workspace, &main_dart, 0);
    // Identical initial state for the second run: the same workspace
    // bytes, a fresh session, a fresh snapshot store.
    fs::write(&main_dart, BROKEN_MAIN).expect("reset to the seeded bytes");
    let second = drive_session(&workspace, &main_dart, 1);

    // The pin's substance: the full transcripts — responses and
    // `dartq/diagnostics` pushes in arrival order — agree byte for byte.
    let first_bytes = serde_json::to_vec(&first).expect("serializes");
    let second_bytes = serde_json::to_vec(&second).expect("serializes");
    assert_eq!(
        first_bytes,
        second_bytes,
        "run-to-run transcript divergence at frame {:?}:\
         \nfirst: {first:#?}\nsecond: {second:#?}",
        first_divergence(&first, &second)
    );

    // The sequence genuinely covers the faces the audit named: every edit
    // answered, and the publish face pushed (the sweep's baseline plus
    // one per row-changing edit).
    for id in 100..100 + EDITS.len() as i64 {
        assert!(
            first
                .iter()
                .any(|frame| frame.get("id") == Some(&json!(id))),
            "analyze {id} answered: {first:?}"
        );
    }
    assert!(
        publishes(&first).len() >= EDITS.len(),
        "each row-changing edit published: {:?}",
        publishes(&first)
    );

    fs::remove_dir_all(&workspace.root).ok();
}
