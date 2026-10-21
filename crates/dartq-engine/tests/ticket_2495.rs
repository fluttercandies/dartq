//! The #2495 pins: a persisted facts generation carrying pre-change
//! layout (the extension-face bounds columns absent) or bounds-empty
//! faces must be re-collected, never served.
//!
//! #2470's forensics surfaced two staleness defects on the snapshot
//! path. First, #2431 added `PersistedExtensionFace::type_param_bounds`
//! without bumping the engine's `FACTS_SHAPE_VERSION` — both shape
//! probes rode `extension_faces` empty, so the structural guard saw zero
//! bytes move — and a pre-column payload restores bounds-empty, which
//! the bound gate reads past-the-end as `Dynamic` (unbounded): every
//! bound-excluded face over-admits. Second, the `api_signature`
//! composition omitted the bounds column entirely, so a stale
//! bounds-empty model carried a signature indistinguishable from the
//! bounds-full truth and nothing on the serve path could tell them
//! apart. The fix folds the bounds into the hash (dartq-semantic
//! `SIGNATURE_VERSION` 9 — its composition-level pin lives next to the
//! fold) and retroactively bumps `FACTS_SHAPE_VERSION` to 15. These pins
//! exercise the engine serve path end to end: a generation forged to the
//! pre-bump key semantics must drain as stale and recompute, while the
//! same stale payload under the current generation's keys still serves —
//! proving the rejection is the version fork, not a broken staging path.

mod common;

use std::path::Path;

use common::Fixture;
use dartq_diagnostics::Diagnostic;
use dartq_engine::{CancellationToken, Engine, PersistedFacts};
use dartq_semantic::model::Ty;
use dartq_snapshot::{Snapshot, SnapshotConfig, SnapshotStore};

/// The generation key semantics BEFORE this ticket's bumps: the
/// `(FACTS_SHAPE_VERSION << 16) | SIGNATURE_VERSION` fold with the
/// pre-#2495 constants (14, 8). A generation keyed with it must never
/// serve against the current engine.
const PRE_BUMP_SEMANTIC_VERSION: u32 = (14 << 16) | 8;

/// A bound-excluded receiver: the real bound (`T extends Num`) rejects
/// `List<Str>`, so the fresh capture reports the access's own
/// `undefined_getter`; a bounds-empty (stale) model reads the bound as
/// `Dynamic`, the face over-admits, and the access goes silent — the
/// exact wrong-output shape the staleness defects produce.
const SOURCE: &str = r#"class Num {}
class Str {}
extension E<T extends Num> on List<T> {
  int get probe => 1;
}
void use(List<Str> xs) {
  xs.probe;
}
"#;

/// The code names reported for the file whose path ends with `tail`,
/// sorted and deduped.
fn codes_for(diagnostics: &[Diagnostic], engine: &Engine, tail: &str) -> Vec<&'static str> {
    let mut codes: Vec<&'static str> = diagnostics
        .iter()
        .filter(|diagnostic| {
            engine
                .file_table()
                .path(diagnostic.file)
                .is_some_and(|path: &Path| path.ends_with(tail))
        })
        .map(|diagnostic| diagnostic.code.name)
        .collect();
    codes.sort_unstable();
    codes.dedup();
    codes
}

/// Feeds [`SOURCE`] through the real engine path — `set_content` +
/// `analyze`, the same multiplexed parse the CLI runs — and flushes the
/// resident facts as one generation (the daemon's save face), returning
/// the engine and the loaded snapshot.
fn flushed_snapshot(
    fixture: &Fixture,
    store: &SnapshotStore,
) -> (Engine, Snapshot<PersistedFacts>) {
    let mut engine = fixture.engine();
    let main = fixture.path("lib/main.dart");
    engine.set_content(&main, SOURCE.as_bytes());
    engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("analyze completes");
    engine.save_facts_snapshot(store).expect("flush");
    let snapshot = store
        .load::<PersistedFacts>(&engine.snapshot_header())
        .expect("the flushed generation loads under its own header");
    (engine, snapshot)
}

/// Erases the bounds columns — the bytes a pre-column payload decodes
/// without — from every persisted extension face.
fn erase_bounds(snapshot: &mut Snapshot<PersistedFacts>) {
    for entry in &mut snapshot.entries {
        for face in &mut entry.payload.elements.extension_faces {
            face.type_param_bounds = Vec::new();
        }
    }
}

/// Downgrades the generation keys — the header and every entry — to the
/// pre-bump semantics.
fn downgrade_generation(snapshot: &mut Snapshot<PersistedFacts>) {
    snapshot.header.semantic_version = PRE_BUMP_SEMANTIC_VERSION;
    for entry in &mut snapshot.entries {
        entry.key.semantic_version = PRE_BUMP_SEMANTIC_VERSION;
    }
}

/// Warm-starts a fresh engine from `snapshot` the way the daemon
/// assembles a session: ingest before the first apply, then feed the
/// same content and analyze. Returns the diagnostics and the engine.
fn warm_start(fixture: &Fixture, snapshot: Snapshot<PersistedFacts>) -> (Vec<Diagnostic>, Engine) {
    let mut engine = fixture.engine();
    let staged = engine.ingest_facts_snapshot(snapshot);
    assert_eq!(staged, 1, "the forged generation stages its one entry");
    let main = fixture.path("lib/main.dart");
    engine.set_content(&main, SOURCE.as_bytes());
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("analyze completes");
    (diagnostics, engine)
}

/// The served bounds of main.dart's single extension face as bound-head
/// texts — `None` for the empty column a pre-column payload restores.
fn served_bounds(engine: &Engine, fixture: &Fixture) -> Vec<Option<String>> {
    let main = fixture.path("lib/main.dart");
    let file = engine.file_table().try_file_id(&main).expect("file id");
    let facts = engine.file_facts(file).expect("facts");
    let strings = engine.strings();
    facts
        .elements
        .extension_faces()
        .iter()
        .map(|face| {
            face.type_param_bounds.first().map(|bound| match bound {
                Ty::Named { name, .. } => strings.resolve(*name).to_owned(),
                other => format!("{other:?}"),
            })
        })
        .collect()
}

/// The headline pin: a snapshot forged to the pre-bump generation
/// (bounds columns erased, keys downgraded to
/// [`PRE_BUMP_SEMANTIC_VERSION`]) must drain as stale and recompute —
/// the warm-started engine serves the bounds-full truth, and the
/// bound-excluded access keeps its `undefined_getter`. Without the
/// version fork the forged keys would match the live lookups and the
/// bounds-empty payload would serve (the control pin below).
#[test]
fn stale_generation_snapshot_is_recollected_not_served() {
    let fixture = Fixture::new(&[("lib/main.dart", SOURCE)]);
    let store = SnapshotStore::open(SnapshotConfig {
        root: Some(fixture.root.join("snapshot-store")),
        capacity_bytes: 64 * 1024 * 1024,
    })
    .expect("store opens");
    let (_, mut snapshot) = flushed_snapshot(&fixture, &store);
    assert_ne!(
        snapshot.header.semantic_version, PRE_BUMP_SEMANTIC_VERSION,
        "the bumps must have moved the generation key, or this pin guards nothing"
    );
    erase_bounds(&mut snapshot);
    downgrade_generation(&mut snapshot);
    let (diagnostics, engine) = warm_start(&fixture, snapshot);
    assert_eq!(
        served_bounds(&engine, &fixture),
        vec![Some("Num".to_owned())],
        "the stale generation drained: the bounds-full truth recomputed"
    );
    assert_eq!(
        codes_for(&diagnostics, &engine, "main.dart"),
        vec!["undefined_getter"],
        "the bound-excluded access keeps its own miss: {diagnostics:?}"
    );
}

/// The control: the same bounds-erased payload under the CURRENT
/// generation's keys still serves — the facts come back bounds-empty and
/// the over-admitted face silences the access. This pins the staging
/// path as live (the rejection above is the version fork, not a broken
/// warm start) and quantifies the damage the fork prevents.
#[test]
fn same_generation_stale_payload_still_serves() {
    let fixture = Fixture::new(&[("lib/main.dart", SOURCE)]);
    let store = SnapshotStore::open(SnapshotConfig {
        root: Some(fixture.root.join("snapshot-store")),
        capacity_bytes: 64 * 1024 * 1024,
    })
    .expect("store opens");
    let (_, mut snapshot) = flushed_snapshot(&fixture, &store);
    erase_bounds(&mut snapshot);
    let (diagnostics, engine) = warm_start(&fixture, snapshot);
    assert_eq!(
        served_bounds(&engine, &fixture),
        vec![None],
        "the stale payload served: the bounds column came back empty"
    );
    assert!(
        codes_for(&diagnostics, &engine, "main.dart").is_empty(),
        "the over-admitted face silences the bound-excluded access — \
         the wrong output the version fork prevents: {diagnostics:?}"
    );
}
