//! The daemon face's dartq.yaml context assembly (#1086) at the session
//! boundary: a temp workspace whose config declares two contexts — one
//! forked by a declared variable, one equal to the default's shape —
//! assembles a multi-context session whose contents land once in the
//! session's shared mapped pool (#483's once-per-content semantic under
//! #1842's default posture), whose equal-shape tag dedups onto the
//! default context (equal resolved shapes share one context), and whose
//! `context:` tags pass the wire's queue-time validation (known
//! tags route, unknown tags answer `INVALID_PARAMS`). No network, no dart
//! SDK.

mod common;

use std::fs;

use common::{Workspace, request, spawn_session, store_root};
use dartq_commands::config::{load, resolve_locator};
use dartq_daemon::Session;
use dartq_snapshot::{DEFAULT_CAPACITY_BYTES, SnapshotConfig, SnapshotStore};
use serde_json::json;

/// The conditional-import fixture: the default context (and its equal-
/// shape `twin`) resolves `heavy.dart`, the declared-variable context
/// resolves `light.dart` — one workspace, two observable views.
const COND_MAIN: &str =
    "import 'heavy.dart' if (dart.library.heavy) 'light.dart';\nvoid main() { heavy(); }\n";
const HEAVY_DART: &str = "import 'missing.dart';\nvoid heavy() {}\n";
const LIGHT_DART: &str = "void light() {}\n";

/// The declared pair: `heavy` forks the declared variable, `twin` carries
/// no fork dimensions (its shape equals the default context's).
const YAML: &str = "version: 1\ncontexts:\n  heavy:\n    sdk: path:sdk\n    define: {dart.library.heavy: \"true\"}\n  twin:\n    sdk: path:sdk\n";

/// The conditional fixture's workspace: `main` plus the two branch files
/// on disk.
fn cond_workspace() -> Workspace {
    let workspace = Workspace::new(COND_MAIN);
    fs::write(workspace.path("lib/heavy.dart"), HEAVY_DART).expect("write heavy");
    fs::write(workspace.path("lib/light.dart"), LIGHT_DART).expect("write light");
    workspace
}

/// The CLI daemon face's shell-edge assembly, re-driven over the same
/// pieces (the config module's parser + locator resolution, one SDK load
/// per declared context). The binary crate's `assemble_contexts` is not
/// importable from an integration test, so this mirror pins the contract
/// at the session boundary it feeds. Sharing runs through the session's
/// own mapped pool — the default `SessionRuntime` posture rebinds every
/// context's content store to one `MappedContentPool`, so each distinct
/// content lands once across the contexts (#483's once-per-content
/// semantic; the configs' own stores are placeholders the posture
/// replaces). Returns the session and the store root (to count
/// generations).
fn assemble(workspace: &Workspace) -> (Session, std::path::PathBuf) {
    fs::write(workspace.path("dartq.yaml"), YAML).expect("write config");
    let declared = load(&workspace.root)
        .expect("the config parses")
        .expect("contexts declared");
    let base = workspace.engine_config();
    let extras = declared
        .contexts
        .iter()
        .map(|context| {
            let root = resolve_locator(&context.label, &context.sdk, &workspace.root)
                .expect("the locator resolves");
            let mut engine = workspace.engine_config();
            engine.sdk = dartq_sdk::SdkSnapshot::load(&root, None).expect("context sdk loads");
            engine.declared_variables = context
                .define
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect();
            (context.label.clone(), engine)
        })
        .collect::<Vec<_>>();
    let store_root = store_root("dartq-yaml-contexts");
    let store = SnapshotStore::open(SnapshotConfig {
        root: Some(store_root.clone()),
        capacity_bytes: DEFAULT_CAPACITY_BYTES,
    })
    .expect("store opens on a temp root");
    let session =
        Session::with_contexts(base, extras, Some(store), None).expect("multi-context session");
    (session, store_root)
}

/// The assembly contract: the two contexts' distinct engines hold the
/// workspace's three contents exactly once — through the session's shared
/// mapped pool (#483's once-per-content semantic, #1842's default
/// posture), not once per context — and the flush writes ONE generation
/// per distinct configuration — two here, the `heavy` fork's salt
/// separating its directory while the equal-shape `twin` shares the
/// default's (equal resolved shapes dedup to one context; a third engine
/// would write three).
#[test]
fn yaml_contexts_share_one_pool_and_dedup_equal_shapes() {
    let workspace = cond_workspace();
    let (mut session, store_path) = assemble(&workspace);

    session.sweep().expect("sweep");
    let _ = session.service();
    assert_eq!(
        session.shared_store_contents(),
        Some(3),
        "three files shared across both engines, not once per context"
    );

    // A base write marks the flush due (the sweep's discovery alone never
    // saves); one generation per distinct configuration lands on disk. The
    // rewrite is the same content — the write task's mtime bump is what
    // marks the sink due.
    let main = workspace.path("lib/main.dart");
    fs::write(&main, COND_MAIN).expect("rewrite");
    session
        .queue_disk_changes(&[main])
        .expect("a small sync admits");
    let _ = session.service();
    session.flush_snapshot();
    let generations = fs::read_dir(store_path.join("snapshots"))
        .expect("the flush wrote the generations dir")
        .count();
    assert_eq!(
        generations, 2,
        "one generation per distinct configuration: the fork's salt separates the directories, the equal shape shares the default's"
    );

    fs::remove_dir_all(&workspace.root).ok();
}

/// The wire contract: both declared tags pass the queue-time validation
/// and answer from the session (the equal-shape `twin` included — it
/// routes through the default's engine), and an unknown tag answers
/// `INVALID_PARAMS` naming it instead of silently answering the default
/// view.
#[test]
fn yaml_context_tags_validate_at_intake() {
    let workspace = cond_workspace();
    let (session, _store) = assemble(&workspace);
    let mut h = spawn_session(session);

    h.step(
        &request(
            2,
            "dartq/check",
            json!({"paths": ["lib"], "context": "heavy"}),
        ),
        1,
    );
    h.step(
        &request(
            3,
            "dartq/check",
            json!({"paths": ["lib"], "context": "twin"}),
        ),
        1,
    );
    let transcript = h.transcript();
    for id in [2, 3] {
        let frame = transcript
            .iter()
            .find(|frame| frame.get("id") == Some(&json!(id)))
            .unwrap_or_else(|| panic!("request {id} answers: {transcript:?}"));
        assert!(
            frame.get("result").is_some(),
            "the declared tag {id} routes: {frame}"
        );
    }

    h.step(
        &request(
            4,
            "dartq/check",
            json!({"paths": ["lib"], "context": "ghost"}),
        ),
        1,
    );
    let transcript = h.transcript();
    let frame = transcript
        .iter()
        .find(|frame| frame.get("id") == Some(&json!(4)))
        .expect("request 4 answers");
    let error = &frame["error"];
    assert_eq!(error["code"], -32602, "INVALID_PARAMS: {frame}");
    assert_eq!(
        error["message"], "unknown context: ghost",
        "the error names the tag: {frame}"
    );

    let (code, _) = h.join_ended();
    assert_eq!(code, 0);

    fs::remove_dir_all(&workspace.root).ok();
}
