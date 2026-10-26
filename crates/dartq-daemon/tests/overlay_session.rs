//! The M4-T6 acceptance scenario over the daemon RPC face: `dartq/check`
//! → `overlay.set` → `dartq/check` per agent (view isolation) →
//! `dartq/sync` base edit → `dartq/check` delta `since` (changed-row
//! subset + cursor echo) → `overlay.clear` → EOF. Also pinned here:
//! publishes never leak an agent view, the visibility contract's
//! disk-vs-engine split, and the short-id symbol economics over
//! `dartq/query` (validated against the published schema, with the byte
//! savings asserted numerically). No network, no dart SDK.

mod common;

use std::fs;
use std::sync::Mutex;

use common::{
    BROKEN_MAIN, CLEAN_MAIN, Workspace, notification, publishes, request, spawn, store_root,
};
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

/// A second syntactic error, distinct from anything the base fixture
/// produces — agent B's overlay signature.
const AGENT_B_MAIN: &str = "void b() { z }\n";

/// The `result` of the answer frame for `id`.
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

/// The `code` spellings of a check/analyze answer's diagnostic rows.
fn codes(result: &Value) -> Vec<&str> {
    result["diags"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .map(|row| row["code"].as_str().expect("code"))
        .collect()
}

/// The scenario, end to end (the ticket's scriptable E2E, over RPC):
/// isolation between two concurrent agent overlays over one base, base
/// immunity to agent views, and the delta subset with its cursor echo.
#[test]
#[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
fn two_agents_edit_and_delta_over_one_base() {
    let _guard = workspace_lock();
    let workspace = Workspace::new(BROKEN_MAIN);
    let main_dart = workspace.path("lib/main.dart");
    let mut h = spawn(&workspace, &store_root("overlay-scenario"));

    // The startup drain already pushed the seeded error (the harness
    // settled it); the base view starts from there.

    // Base check = the disk pipeline: it reports missing relative-URI
    // directive targets, and the fixture's import is one.
    h.step(&request(2, "dartq/check", json!({"paths": ["lib"]})), 1);
    assert_eq!(
        codes(&answer_result(&h, 2)),
        ["uri_does_not_exist"],
        "base check reports the missing import: {:?}",
        answer_result(&h, 2)
    );
    // Base analyze = the engine view: the URI error shows.
    h.step(&request(3, "dartq/analyze", json!({})), 1);
    assert_eq!(codes(&answer_result(&h, 3)), ["uri_does_not_exist"]);

    // Two agents stage conflicting views of the same file.
    h.step(
        &request(
            4,
            "dartq/overlay.set",
            json!({"agent_id": "A", "files": {"lib/main.dart": CLEAN_MAIN}}),
        ),
        1,
    );
    h.step(
        &request(
            5,
            "dartq/overlay.set",
            json!({"agent_id": "B", "files": {"lib/main.dart": AGENT_B_MAIN}}),
        ),
        1,
    );

    // The acceptance: two concurrent agent overlays over one base produce
    // isolated diagnostics — and the base view is untouched.
    h.burst(
        &[
            request(6, "dartq/check", json!({"paths": ["lib"], "agent_id": "A"})),
            request(7, "dartq/check", json!({"paths": ["lib"], "agent_id": "B"})),
            request(8, "dartq/analyze", json!({"agent_id": "A"})),
            request(9, "dartq/analyze", json!({})),
        ],
        4,
    );
    assert!(
        codes(&answer_result(&h, 6)).is_empty(),
        "agent A's view is clean: {:?}",
        answer_result(&h, 6)
    );
    assert_eq!(
        codes(&answer_result(&h, 7)),
        ["expected_token", "undefined_identifier"],
        "agent B's view carries only its own errors: {:?}",
        answer_result(&h, 7)
    );
    assert!(
        codes(&answer_result(&h, 8)).is_empty(),
        "agent A sees its own view, not B's or the base: {:?}",
        answer_result(&h, 8)
    );
    assert_eq!(
        codes(&answer_result(&h, 9)),
        ["uri_does_not_exist"],
        "the base view is immune to both overlays: {:?}",
        answer_result(&h, 9)
    );

    // The base edit: the client fixes the file on disk and reports it;
    // the clear publish drains.
    fs::write(&main_dart, CLEAN_MAIN).expect("rewrite");
    h.step(
        &notification("dartq/sync", json!({"paths": ["lib/main.dart"]})),
        1,
    );
    let transcript = h.transcript();
    let pushed = publishes(&transcript);
    assert_eq!(
        pushed.len(),
        2,
        "the sweep push + one publish for the base edit"
    );
    assert!(
        pushed[1]["params"]["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .is_empty(),
        "the base clear push carries the base view, never an agent view"
    );

    // Delta mode: agent B's overlay masks the base edit, so
    // B's view changed at its overlay.set — `since` cuts by that cursor.
    // The cursor counts session-observed writes only — the startup sweep
    // is the cursor-0 baseline, not a change: overlay.set A = 1,
    // overlay.set B = 2, the sync's drain = 3.
    h.burst(
        &[
            request(
                10,
                "dartq/check",
                json!({"paths": ["lib"], "agent_id": "B", "since": 2}),
            ),
            request(
                11,
                "dartq/check",
                json!({"paths": ["lib"], "agent_id": "B", "since": 1}),
            ),
            request(12, "dartq/check", json!({"paths": ["lib"], "since": 2})),
        ],
        3,
    );
    // B overlaid main at cursor 2: since=2 excludes it — the error row
    // vanishes; since=1 keeps it. The answer echoes the current cursor (3).
    assert!(
        codes(&answer_result(&h, 10)).is_empty(),
        "delta since the overlay's own cursor: {:?}",
        answer_result(&h, 10)
    );
    assert_eq!(answer_result(&h, 10)["since"], 3, "the cursor echoes");
    assert_eq!(
        codes(&answer_result(&h, 11)),
        ["expected_token", "undefined_identifier"],
        "delta before the overlay's cursor keeps the rows: {:?}",
        answer_result(&h, 11)
    );
    // The base view changed at the sync drain (3 > 2): the engine saw the
    // fixed text, so the URI error is gone — but the rows' change stamp is
    // what the delta includes, and the fresh clean set has nothing to list.
    assert!(
        codes(&answer_result(&h, 12)).is_empty(),
        "base delta since 2 carries no rows (the fix cleared them): {:?}",
        answer_result(&h, 12)
    );
    assert_eq!(answer_result(&h, 12)["since"], 3);

    // Teardown evicts: a cleared agent answers INVALID_PARAMS, fail loud.
    h.step(
        &request(13, "dartq/overlay.clear", json!({"agent_id": "A"})),
        1,
    );
    h.step(
        &request(14, "dartq/check", json!({"paths": ["lib"], "agent_id": "A"})),
        1,
    );
    let error = answer_error(&h, 14);
    assert_eq!(error["code"], -32602, "unknown agent_id: {error}");
    assert!(
        error["message"].as_str().unwrap_or("").contains('A'),
        "the error names the agent: {error}"
    );

    // Overlay-only state never leaks across teardown: B clears silently.
    h.step(
        &request(15, "dartq/overlay.clear", json!({"agent_id": "B"})),
        1,
    );
    h.step(
        &request(16, "dartq/check", json!({"paths": ["lib"], "agent_id": "B"})),
        1,
    );
    assert_eq!(answer_error(&h, 16)["code"], -32602);

    let (code, transcript) = h.join_ended();
    assert_eq!(code, 0);
    // Across the whole session, agent views never published: the two
    // pushes are the base view — the sweep's rows and the sync's clear.
    let pushes: Vec<&Value> = publishes(&transcript);
    assert_eq!(pushes.len(), 2, "no agent-view publish ever: {pushes:?}");

    fs::remove_dir_all(&workspace.root).ok();
}

/// The session economics: the short-id symbol table over the
/// `dartq/check` full tier (the daemon-native envelope; the query/impact
/// `symbols` field stays unemitted — its published values are pinned to
/// `label|name`, a different value domain than these rows' file paths, see
/// the commands module docs). First full answer: a `symbols` preamble plus
/// id-substituted rows, strictly smaller than the path-spelled shape; second
/// answer: back-references with no re-announcement, smaller still; compact
/// stays byte-identical to the pre-economics shape. The query answer's
/// delta echo validates against the published query schema.
#[test]
#[expect(clippy::cast_precision_loss)] // byte counts cast to f64 for the saved-percentage assert
fn session_economics_save_bytes_and_stay_schema_valid() {
    let _guard = workspace_lock();
    let workspace = Workspace::new("void main() {}\n");
    // One file, several rows: the repeated long path is what the
    // economics collapse.
    fs::write(
        workspace.path("lib/rows.dart"),
        "void a() { b }\nvoid c() { d }\nvoid e() { f }\n",
    )
    .expect("write");
    let mut h = spawn(&workspace, &store_root("overlay-economics"));

    let params = json!({"paths": ["lib"], "verbosity": "full"});

    h.step(&request(2, "dartq/check", params.clone()), 1);
    let first = answer_result(&h, 2);
    let preamble = first["symbols"].as_object().expect("symbols preamble");
    assert_eq!(preamble.len(), 1, "one path announced: {first}");
    let (id, path) = preamble.iter().next().expect("entry");
    assert!(path.as_str().unwrap_or("").ends_with("lib/rows.dart"));
    let rows = first["diags"].as_array().expect("rows");
    assert!(
        rows.len() >= 3,
        "the fixture carries repeated-path rows: {first}"
    );
    for row in rows {
        assert_eq!(row["file"], *id, "rows back-reference the id");
    }
    let first_bytes = first.to_string().len();

    // The counterfactual: the same answer with the path spelled out.
    let spelled = json!({
        "v": first["v"],
        "diags": first["diags"]
            .as_array()
            .expect("rows")
            .iter()
            .map(|row| {
                let mut row = row.clone();
                row["file"] = json!(path.as_str().unwrap_or(""));
                row
            })
            .collect::<Vec<_>>(),
    });

    // Second answer: the id is already announced — no preamble entry again,
    // and the substitution now pays without any table cost.
    h.step(&request(3, "dartq/check", params.clone()), 1);
    let second = answer_result(&h, 3);
    assert!(
        second["symbols"]
            .as_object()
            .is_none_or(serde_json::Map::is_empty),
        "an announced id is never re-announced: {second}"
    );
    let second_bytes = second.to_string().len();

    // The savings, numerically (the substitution guard exists to guarantee
    // the direction): preamble answer < path-spelled, back-reference
    // answer < preamble answer.
    assert!(
        first_bytes < spelled.to_string().len(),
        "preamble answer {first_bytes} bytes must beat the path-spelled shape {}",
        spelled.to_string().len()
    );
    assert!(
        second_bytes < first_bytes,
        "back-reference answer {second_bytes} bytes must beat the preamble answer {first_bytes}"
    );
    println!(
        "economics: path-spelled {} B, first answer (preamble) {first_bytes} B, \
         back-reference answer {second_bytes} B; per-answer savings vs spelled: preamble {:.0}%, \
         back-reference {:.0}%",
        spelled.to_string().len(),
        100.0 * (spelled.to_string().len() - first_bytes) as f64 / spelled.to_string().len() as f64,
        100.0 * (spelled.to_string().len() - second_bytes) as f64
            / spelled.to_string().len() as f64,
    );

    // Compact byte-identity: no session field may appear unprompted, and
    // the path is spelled out.
    h.step(&request(4, "dartq/check", json!({"paths": ["lib"]})), 1);
    let compact = answer_result(&h, 4);
    assert!(
        compact.get("symbols").is_none(),
        "compact stays bare: {compact}"
    );
    assert!(compact.get("since").is_none());
    for row in compact["diags"].as_array().expect("rows") {
        assert_eq!(row["file"], *path, "compact spells the path");
    }

    let (code, _) = h.join_ended();
    assert_eq!(code, 0);

    // A fresh session for the schema pin: query with `since` only.
    let workspace = Workspace::new("void main() {}\n");
    let mut h = spawn(&workspace, &store_root("overlay-economics-query"));
    h.step(
        &request(2, "dartq/query", json!({"name": "main", "since": 0})),
        1,
    );
    let query = answer_result(&h, 2);
    assert_eq!(query["since"], 0, "the delta echo (no content change yet)");
    dartq_schema::validate(&query.to_string(), dartq_schema::Domain::Query)
        .expect("the delta query answer matches the published schema");
    let (code, _) = h.join_ended();
    assert_eq!(code, 0);

    fs::remove_dir_all(&workspace.root).ok();
}

/// `dartq/overlay.commit` over the wire: the agent's
/// staged whole-file buffer writes back to the worktree, the answer names
/// the committed count, the merge drains with the base write (the analyze
/// view turns clean), and an unknown agent fails loud.
#[test]
fn overlay_commit_writes_back_over_rpc() {
    let _guard = workspace_lock();
    let workspace = Workspace::new(BROKEN_MAIN);
    let main = workspace.path("lib/main.dart");
    let mut h = spawn(&workspace, &store_root("overlay-commit-wire"));

    h.step(
        &request(
            2,
            "dartq/overlay.set",
            json!({"agent_id": "A", "files": {"lib/main.dart": CLEAN_MAIN}}),
        ),
        1,
    );

    // Commit + the merged base's analyze ride one batch: the commit
    // response, the merge's clear publish, and the analyze answer.
    h.burst(
        &[
            request(3, "dartq/overlay.commit", json!({"agent_id": "A"})),
            request(4, "dartq/analyze", json!({})),
        ],
        3,
    );
    assert_eq!(
        answer_result(&h, 3)["committed"],
        1,
        "the answer names the committed count"
    );
    assert_eq!(
        fs::read(&main).expect("read back"),
        CLEAN_MAIN.as_bytes(),
        "the buffer reached the worktree"
    );
    assert!(
        codes(&answer_result(&h, 4)).is_empty(),
        "the merged base analyzes clean: {:?}",
        answer_result(&h, 4)
    );

    // An unknown agent fails loud at intake.
    h.step(
        &request(5, "dartq/overlay.commit", json!({"agent_id": "ghost"})),
        1,
    );
    let error = answer_error(&h, 5);
    assert_eq!(error["code"], -32602, "INVALID_PARAMS: {error}");
    assert!(
        error["message"].as_str().unwrap_or("").contains("ghost"),
        "the error names the agent: {error}"
    );

    let (code, _) = h.join_ended();
    assert_eq!(code, 0);

    fs::remove_dir_all(&workspace.root).ok();
}

/// The tombstone face of `overlay.set`: a `null` content entry excludes
/// the path from the agent's view (a base-resident file leaves the view),
/// the base view stays immune, a follow-up content staging resurrects the
/// path in the agent's view (tombstone → content merges both directions),
/// and a commit makes the declared end state real on disk — the tombstoned
/// file is deleted. Also pinned: the parse boundary — a content value that
/// is neither a string nor `null` answers INVALID_PARAMS.
#[test]
fn tombstone_excludes_view_and_commit_deletes() {
    let _guard = workspace_lock();
    let workspace = Workspace::new(BROKEN_MAIN);
    let main_dart = workspace.path("lib/main.dart");
    let mut h = spawn(&workspace, &store_root("overlay-tombstone"));

    // The parse boundary: a number is neither content nor a tombstone.
    h.step(
        &request(
            2,
            "dartq/overlay.set",
            json!({"agent_id": "T", "files": {"lib/main.dart": 42}}),
        ),
        1,
    );
    let error = answer_error(&h, 2);
    assert_eq!(error["code"], -32602, "INVALID_PARAMS: {error}");
    assert!(
        error["message"].as_str().unwrap_or("").contains("null"),
        "the error names the accepted forms (string or null): {error}"
    );

    // The tombstone: T's view excludes main.dart, so its uri error is
    // gone; the base view still carries it.
    h.step(
        &request(
            3,
            "dartq/overlay.set",
            json!({"agent_id": "T", "files": {"lib/main.dart": null}}),
        ),
        1,
    );
    h.burst(
        &[
            request(4, "dartq/analyze", json!({"agent_id": "T"})),
            request(5, "dartq/analyze", json!({})),
        ],
        2,
    );
    assert!(
        codes(&answer_result(&h, 4)).is_empty(),
        "the tombstoned file's rows leave the agent view: {:?}",
        answer_result(&h, 4)
    );
    assert_eq!(
        codes(&answer_result(&h, 5)),
        ["uri_does_not_exist"],
        "the base view is immune to the tombstone: {:?}",
        answer_result(&h, 5)
    );

    // Tombstone → content re-stages: the merge works both directions, and
    // the agent view serves the staged text (clean, not the disk bytes).
    h.step(
        &request(
            6,
            "dartq/overlay.set",
            json!({"agent_id": "T", "files": {"lib/main.dart": CLEAN_MAIN}}),
        ),
        1,
    );
    h.step(&request(7, "dartq/analyze", json!({"agent_id": "T"})), 1);
    assert!(
        codes(&answer_result(&h, 7)).is_empty(),
        "a content staging replaces the tombstone: {:?}",
        answer_result(&h, 7)
    );

    // Back to the tombstone, then commit: the declared end state excludes
    // the file, so the commit deletes it from the worktree and the base
    // view drops its rows.
    h.step(
        &request(
            8,
            "dartq/overlay.set",
            json!({"agent_id": "T", "files": {"lib/main.dart": null}}),
        ),
        1,
    );
    h.step(&request(9, "dartq/overlay.commit", json!({"agent_id": "T"})), 1);
    assert!(
        !main_dart.exists(),
        "the committed tombstone deletes the worktree file"
    );
    h.step(&request(10, "dartq/analyze", json!({})), 1);
    assert!(
        codes(&answer_result(&h, 10)).is_empty(),
        "the base view drops the deleted file's rows: {:?}",
        answer_result(&h, 10)
    );

    let (code, _) = h.join_ended();
    assert_eq!(code, 0);

    fs::remove_dir_all(&workspace.root).ok();
}
