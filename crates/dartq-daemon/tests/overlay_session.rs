//! The M4-T6 acceptance scenario over the adopted Cockpit contract (m4-recon
//! Q1/Q2: the daemon RPC itself + LSP Content-Length framing, no bespoke
//! protocol): connect → initialize → didOpen → `dartq/check` →
//! `overlay.set` → `dartq/check` per agent (view isolation) → `didChange` →
//! `dartq/check` delta `since` (changed-row subset + cursor echo) →
//! `overlay.clear` → shutdown/exit. Also pinned here: publishes never leak
//! an agent view, the visibility contract's disk-vs-engine split, and the
//! short-id symbol economics over `dartq/query` (validated against the
//! published schema, with the byte savings asserted numerically). No
//! network, no dart SDK.

mod common;

use std::fs;
use std::sync::Mutex;

use common::{BROKEN_MAIN, CLEAN_MAIN, Workspace, notification, request, spawn, store_root};
use serde_json::{Value, json};

/// `Workspace::new` pins one pid-derived temp root; the tests in this
/// binary serialize on this lock so a parallel test's workspace re-create
/// can never yank another test's workspace mid-session.
static WORKSPACE: Mutex<()> = Mutex::new(());

/// A second syntactic error, distinct from anything the base fixture
/// produces — agent B's overlay signature.
const AGENT_B_MAIN: &str = "void b() { z }\n";

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
    result["diagnostics"]
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
fn two_agents_edit_and_delta_over_one_base() {
    let _guard = WORKSPACE.lock().expect("workspace lock");
    let workspace = Workspace::new(BROKEN_MAIN);
    let main_uri = dartq_uri(&workspace, "lib/main.dart");
    let mut h = spawn(&workspace, &store_root("overlay-scenario"));

    handshake(&mut h, 1);

    // didOpen with the on-disk text: a write applies (the cursor starts
    // moving), the publish diff sees no change.
    h.step(
        &notification(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": main_uri, "languageId": "dart", "version": 1,
                   "text": BROKEN_MAIN}}),
        ),
        0,
    );

    // Base check = the M0 disk pipeline (module-doc visibility table): a
    // missing import is a semantic error, invisible to scan+parse — empty.
    h.step(&request(2, "dartq/check", json!({"paths": ["lib"]})), 1);
    assert!(
        codes(&answer_result(&h, 2)).is_empty(),
        "base check is the disk pipeline: {:?}",
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
            json!({"agentId": "A", "files": {"lib/main.dart": CLEAN_MAIN}}),
        ),
        1,
    );
    h.step(
        &request(
            5,
            "dartq/overlay.set",
            json!({"agentId": "B", "files": {"lib/main.dart": AGENT_B_MAIN}}),
        ),
        1,
    );

    // The acceptance: two concurrent agent overlays over one base produce
    // isolated diagnostics — and the base view is untouched.
    h.burst(
        &[
            request(6, "dartq/check", json!({"paths": ["lib"], "agentId": "A"})),
            request(7, "dartq/check", json!({"paths": ["lib"], "agentId": "B"})),
            request(8, "dartq/analyze", json!({"agentId": "A"})),
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
        ["undefined_identifier", "expected_token"],
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
    let publishes_so_far = h.transcript().len();

    // The base edit: the client fixes the file; the clear publish drains.
    h.step(
        &notification(
            "textDocument/didChange",
            json!({"textDocument": {"uri": main_uri, "version": 2},
                   "contentChanges": [{"text": CLEAN_MAIN}]}),
        ),
        1,
    );
    let transcript = h.transcript();
    let pushed: Vec<&Value> = transcript[publishes_so_far..]
        .iter()
        .filter(|frame| frame["method"] == "textDocument/publishDiagnostics")
        .collect();
    assert_eq!(
        pushed.len(),
        1,
        "one publish for the base edit: {transcript:?}"
    );
    assert!(
        pushed[0]["params"]["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .is_empty(),
        "the base clear push carries the base view, never an agent view"
    );

    // Delta mode (PRD §15.8): agent B's overlay masks the base edit, so
    // B's view changed at its overlay.set — `since` cuts by that cursor.
    // Cursors: didOpen drain = 1, overlay.set A = 2, overlay.set B = 3,
    // didChange drain = 4.
    h.burst(
        &[
            request(
                10,
                "dartq/check",
                json!({"paths": ["lib"], "agentId": "B", "since": 3}),
            ),
            request(
                11,
                "dartq/check",
                json!({"paths": ["lib"], "agentId": "B", "since": 2}),
            ),
            request(12, "dartq/check", json!({"paths": ["lib"], "since": 3})),
        ],
        3,
    );
    // B overlaid main at cursor 3: since=3 excludes it — the error row
    // vanishes; since=2 keeps it. The answer echoes the current cursor (4).
    assert!(
        codes(&answer_result(&h, 10)).is_empty(),
        "delta since the overlay's own cursor: {:?}",
        answer_result(&h, 10)
    );
    assert_eq!(answer_result(&h, 10)["since"], 4, "the cursor echoes");
    assert_eq!(
        codes(&answer_result(&h, 11)),
        ["undefined_identifier", "expected_token"],
        "delta before the overlay's cursor keeps the rows: {:?}",
        answer_result(&h, 11)
    );
    // The base view changed at the didChange drain (4 > 3): rows included —
    // and the disk pipeline now scans clean text, so the subset is empty.
    assert!(
        codes(&answer_result(&h, 12)).is_empty(),
        "base delta on a now-clean file: {:?}",
        answer_result(&h, 12)
    );
    assert_eq!(answer_result(&h, 12)["since"], 4);

    // Teardown evicts: a cleared agent answers INVALID_PARAMS, fail loud.
    h.step(
        &request(13, "dartq/overlay.clear", json!({"agentId": "A"})),
        1,
    );
    h.step(
        &request(14, "dartq/check", json!({"paths": ["lib"], "agentId": "A"})),
        1,
    );
    let error = answer_error(&h, 14);
    assert_eq!(error["code"], -32602, "unknown agentId: {error}");
    assert!(
        error["message"].as_str().unwrap_or("").contains("A"),
        "the error names the agent: {error}"
    );

    // Overlay-only state never leaks across teardown: B clears silently.
    h.step(
        &request(15, "dartq/overlay.clear", json!({"agentId": "B"})),
        1,
    );
    h.step(
        &request(16, "dartq/check", json!({"paths": ["lib"], "agentId": "B"})),
        1,
    );
    assert_eq!(answer_error(&h, 16)["code"], -32602);

    h.step(&request(17, "shutdown", json!(null)), 1);
    let (code, transcript) = h.exit();
    assert_eq!(code, 0);
    // Across the whole session, agent views never published: exactly two
    // pushes — the sweep's base diagnostics and the didChange clear.
    let publishes: Vec<&Value> = transcript
        .iter()
        .filter(|frame| frame["method"] == "textDocument/publishDiagnostics")
        .collect();
    assert_eq!(
        publishes.len(),
        2,
        "no agent-view publish ever: {publishes:?}"
    );

    fs::remove_dir_all(&workspace.root).ok();
}

/// The session economics (PRD §15.8): the short-id symbol table over the
/// `dartq/check` full tier (the daemon-native envelope; the query/impact
/// `symbols` field stays unemitted — its published values are pinned to
/// `label|name`, a different value domain than these rows' file paths, see
/// the commands module docs). First full answer: a `symbols` preamble plus
/// id-substituted rows, strictly smaller than the path-spelled shape; second
/// answer: back-references with no re-announcement, smaller still; compact
/// stays byte-identical to the pre-economics shape. The query answer's
/// delta echo validates against the published query schema.
#[test]
fn session_economics_save_bytes_and_stay_schema_valid() {
    let _guard = WORKSPACE.lock().expect("workspace lock");
    let workspace = Workspace::new("void main() {}\n");
    // One file, several rows: the repeated long path is what the
    // economics collapse.
    fs::write(
        workspace.path("lib/rows.dart"),
        "void a() { b }\nvoid c() { d }\nvoid e() { f }\n",
    )
    .expect("write");
    let mut h = spawn(&workspace, &store_root("overlay-economics"));

    handshake(&mut h, 1);
    let params = json!({"paths": ["lib"], "verbosity": "full"});

    h.step(&request(2, "dartq/check", params.clone()), 1);
    let first = answer_result(&h, 2);
    let preamble = first["symbols"].as_object().expect("symbols preamble");
    assert_eq!(preamble.len(), 1, "one path announced: {first}");
    let (id, path) = preamble.iter().next().expect("entry");
    assert!(path.as_str().unwrap_or("").ends_with("lib/rows.dart"));
    let rows = first["diagnostics"].as_array().expect("rows");
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
        "schema_version": first["schema_version"],
        "diagnostics": first["diagnostics"]
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
            .map_or(true, |table| table.is_empty()),
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
    for row in compact["diagnostics"].as_array().expect("rows") {
        assert_eq!(row["file"], *path, "compact spells the path");
    }

    // The published query schema tolerates the session fields it declares:
    // a delta query answer (with the cursor echo, no symbols) validates.
    h.step(&request(5, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, 0);

    // A fresh session for the schema pin: query with `since` only.
    let workspace = Workspace::new("void main() {}\n");
    let mut h = spawn(&workspace, &store_root("overlay-economics-query"));
    handshake(&mut h, 0);
    h.step(
        &request(2, "dartq/query", json!({"name": "main", "since": 0})),
        1,
    );
    let query = answer_result(&h, 2);
    assert_eq!(query["since"], 0, "the delta echo (no content change yet)");
    dartq_schema::validate(&query.to_string(), dartq_schema::Domain::Query)
        .expect("the delta query answer matches the published schema");
    h.step(&request(3, "shutdown", json!(null)), 1);
    let (code, _) = h.exit();
    assert_eq!(code, 0);

    fs::remove_dir_all(&workspace.root).ok();
}

/// The workspace-rooted `file://` URI for a workspace-relative path.
fn dartq_uri(workspace: &Workspace, relative: &str) -> String {
    format!("file://{}", workspace.path(relative).display())
}
