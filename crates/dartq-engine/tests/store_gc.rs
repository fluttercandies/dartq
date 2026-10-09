//! Content-store eviction pins (goal #271): steady editing leaves the store
//! bounded — residency + facts-cache cap + sweep threshold — and no
//! referenced content is ever evicted, whether the reference holder is
//! residency or the facts cache (the mark-sweep liveness contract, the
//! rust-analyzer vfs GC posture).

mod common;

use common::{EventLog, Fixture};
use dartq_engine::{EngineEvent, RevalidateGate};

/// The steady-state entry bound after any apply: one resident content, the
/// facts-cache cap, and fewer than `GC_DEAD_THRESHOLD` (state.rs) unswept
/// dead versions between amortized sweeps.
const ENTRY_BOUND: usize = 1 + 1024 + 63;

#[test]
fn ten_thousand_edits_keep_the_store_bounded() {
    let fixture = Fixture::new(&[("lib/main.dart", "int a() => 0;\n")]);
    let mut engine = fixture.engine();
    let path = fixture.path("lib/main.dart");

    let mut sink = |_: EngineEvent| {};
    for step in 0..10_000 {
        let text = format!("int a() => {step};\n");
        engine.set_content(&path, text.as_bytes());
        engine.apply_pending(&mut sink);
    }
    let main = engine.file_table().try_file_id(&path).expect("interned");

    let entries = engine.stored_contents();
    assert!(
        entries <= ENTRY_BOUND,
        "unbounded growth: {entries} entries after 10k edits (bound {ENTRY_BOUND})"
    );
    // The final content's bytes survive the churn (resident reference).
    let expected = "int a() => 9999;\n";
    assert_eq!(engine.content(main), Some(expected.as_bytes()));
}

#[test]
fn facts_cache_referenced_content_survives_sweeps() {
    let fixture = Fixture::new(&[("lib/main.dart", "int a() => 0;\n")]);
    let mut engine = fixture.engine();
    let path = fixture.path("lib/main.dart");
    let original = std::fs::read(&path).expect("fixture bytes");

    let mut sink = |_: EngineEvent| {};
    engine.set_content(&path, &original);
    engine.apply_pending(&mut sink);
    let main = engine.file_table().try_file_id(&path).expect("interned");

    // 300 edits of distinct content cross the sweep threshold four times
    // while staying under the facts-cache cap, so the original's facts
    // entry — and with it its content id — stays referenced throughout.
    for step in 1..=300 {
        let text = format!("int a() => {step};\n");
        engine.set_content(&path, text.as_bytes());
        engine.apply_pending(&mut sink);
    }

    // Change back to the original bytes: the facts cache must still serve
    // the parse, and the referenced bytes must still be resident.
    engine.set_content(&path, &original);
    let mut log = EventLog::default();
    engine.apply_pending(&mut log.sink());
    assert_eq!(log.revalidated.len(), 1, "single file revalidated");
    assert_eq!(
        log.revalidated[0],
        (
            main,
            RevalidateGate::Parsed {
                ast_cache_hit: true
            }
        ),
        "change-back must hit the facts cache"
    );
    assert_eq!(engine.content(main), Some(original.as_slice()));
}

#[test]
fn resident_content_survives_another_files_churn() {
    let fixture = Fixture::new(&[
        ("lib/main.dart", "int keep() => 1;\n"),
        ("lib/churn.dart", "int churn() => 0;\n"),
    ]);
    let mut engine = fixture.engine();
    engine.discover_workspace().expect("discovery");

    let mut sink = |_: EngineEvent| {};
    engine.apply_pending(&mut sink);

    let keep_path = fixture.path("lib/main.dart");
    let keep = engine
        .file_table()
        .try_file_id(&keep_path)
        .expect("interned");
    let keep_bytes = engine.content(keep).expect("resident").to_vec();

    // The churn target drives sweeps; the untouched bystander's content
    // must ride every one of them untouched.
    let churn_path = fixture.path("lib/churn.dart");
    for step in 1..=300 {
        let text = format!("int churn() => {step};\n");
        engine.set_content(&churn_path, text.as_bytes());
        engine.apply_pending(&mut sink);
        assert_eq!(
            engine.content(keep),
            Some(keep_bytes.as_slice()),
            "bystander content lost at step {step}"
        );
    }
}
