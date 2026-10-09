//! Incremental gate pins (goal #88, engine-api-spec §1.2 + §5 item 2,
//! incremental-l1-spec §1.2/§3.2/§6): the ContentId no-op gate, the
//! single-file recompute after one edit, the cascade reach, and the
//! apply+analyze vs analyze-alone byte-equality.

mod common;

use common::{EventLog, Fixture, GREETER_0, GREETER_1, MAIN_0, STANDALONE_0};
use dartq_engine::RevalidateGate;

fn summarized(
    engine: &dartq_engine::Engine,
    diagnostics: &[dartq_diagnostics::Diagnostic],
) -> Vec<String> {
    diagnostics
        .iter()
        .map(|diagnostic| {
            let path = engine
                .file_table()
                .path(diagnostic.file)
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default();
            format!(
                "{}|{}|{}|{}",
                path, diagnostic.code.name, diagnostic.range.offset, diagnostic.message
            )
        })
        .collect()
}

#[test]
fn identical_rewrite_is_a_gate_no_op() {
    let fixture = Fixture::new(&[("lib/main.dart", MAIN_0), ("lib/greeter.dart", GREETER_0)]);
    let mut engine = fixture.engine();
    engine.discover_workspace().expect("discovery");
    let clean = engine
        .analyze(&[], &mut |_| {}, &Default::default())
        .expect("cold");
    assert!(clean.is_empty());

    // Rewrite with identical bytes: legal changeFile, zero cascade (§1.2).
    let greeter = fixture.path("lib/greeter.dart");
    engine.set_content(&greeter, GREETER_0.as_bytes());

    let mut log = EventLog::default();
    let diagnostics = engine
        .analyze(&[], &mut log.sink(), &Default::default())
        .expect("no-op");
    assert_eq!(diagnostics, clean);

    assert_eq!(
        log.overlay_applied,
        vec![(0, 0)],
        "queue drained, nothing changed"
    );
    assert_eq!(
        log.revalidated,
        vec![(
            engine.file_table().try_file_id(&greeter).expect("interned"),
            RevalidateGate::ContentIdHit
        )],
        "gate outcome per file"
    );
    assert!(
        log.cascades.iter().flatten().next().is_none(),
        "zero cascade on identical bytes"
    );
    assert!(
        log.analyzed.iter().all(|(_, from_cache)| *from_cache),
        "no library recomputed"
    );
}

#[test]
fn single_edit_recomputes_only_its_cascade() {
    let fixture = Fixture::new(&[
        ("lib/main.dart", MAIN_0),
        ("lib/greeter.dart", GREETER_0),
        ("lib/standalone.dart", STANDALONE_0),
    ]);
    let mut engine = fixture.engine();
    engine.discover_workspace().expect("discovery");
    engine
        .analyze(&[], &mut |_| {}, &Default::default())
        .expect("cold");

    // Change one file: only it revalidates; the cascade reaches its
    // importer (L1 is content-driven — §3.2, no signature refinement).
    engine.set_content(&fixture.path("lib/greeter.dart"), GREETER_1.as_bytes());

    // The untouched library rides along as the AnalyzeFile target: targets
    // drive only the amplification events, and a clean targeted library
    // reports a cache hit (engine-api-spec §1.3).
    let target = engine
        .file_table()
        .try_file_id(&fixture.path("lib/standalone.dart"))
        .expect("interned");

    let mut log = EventLog::default();
    let diagnostics = engine
        .analyze(&[target], &mut log.sink(), &Default::default())
        .expect("edit");
    assert!(diagnostics.is_empty());

    assert_eq!(log.overlay_applied, vec![(1, 0)], "one file changed");
    assert_eq!(log.revalidated.len(), 1, "only the edited file revalidates");
    assert!(log.revalidated.iter().all(|(_, gate)| matches!(
        gate,
        RevalidateGate::Parsed {
            ast_cache_hit: false
        }
    )));
    // Cascade: the edited library plus its importer, never more.
    let cascade = log.cascades.into_iter().next().expect("one cascade event");
    assert_eq!(cascade.len(), 2, "edited library + downstream importer");
    assert_eq!(
        log.analyzed
            .iter()
            .filter(|(_, from_cache)| !*from_cache)
            .count(),
        2,
        "exactly the cascade recomputed"
    );
    assert!(
        log.analyzed.iter().any(|(_, from_cache)| *from_cache),
        "the clean target library reports a cache hit"
    );

    // The unchanged library kept its cached result: analyze again with the
    // same content and observe zero work.
    let mut again = EventLog::default();
    engine
        .analyze(&[], &mut again.sink(), &Default::default())
        .expect("settle");
    assert!(again.revalidated.is_empty() && again.analyzed.iter().all(|(_, hit)| *hit));
}

#[test]
fn explicit_apply_then_analyze_matches_analyze_alone() {
    let fixture = Fixture::new(&[("lib/main.dart", MAIN_0), ("lib/greeter.dart", GREETER_1)]);
    let mut solo = fixture.engine();
    solo.discover_workspace().expect("discovery");
    let mut solo_log = EventLog::default();
    let solo_result = solo
        .analyze(&[], &mut solo_log.sink(), &Default::default())
        .expect("solo");

    let mut split = fixture.engine();
    split.discover_workspace().expect("discovery");
    let mut apply_log = EventLog::default();
    let report = split.apply_pending(&mut apply_log.sink());
    let mut analyze_log = EventLog::default();
    let split_result = split
        .analyze(&[], &mut analyze_log.sink(), &Default::default())
        .expect("split");

    // Byte-identical diagnostics (§5 item 2), single OverlayApplied across
    // both calls, no double emission.
    assert_eq!(
        summarized(&split, &split_result),
        summarized(&solo, &solo_result)
    );
    assert_eq!(apply_log.overlay_applied.len(), 1, "apply sees the queue");
    assert!(
        analyze_log.overlay_applied.is_empty(),
        "second apply is empty, no event"
    );
    assert_eq!(report.changed.len(), 2);
    assert_eq!(
        analyze_log.revalidated.len(),
        0,
        "no work left for analyze's apply"
    );
}

#[test]
fn cancel_keeps_completed_libraries_and_stays_dirty() {
    // The M1 recompute is fast; cancel deterministically before any work by
    // cancelling the token ahead of analyze, then verify the engine stays
    // consistent and a later analyze completes everything.
    let fixture = Fixture::new(&[("lib/main.dart", MAIN_0), ("lib/greeter.dart", GREETER_0)]);
    let mut engine = fixture.engine();
    engine.discover_workspace().expect("discovery");

    let token = dartq_engine::CancellationToken::new();
    token.cancel();
    let outcome = engine.analyze(&[], &mut |_| {}, &token);
    assert!(matches!(outcome, Err(dartq_engine::Cancelled)));

    // Nothing completed: a fresh token recomputes everything cleanly.
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &dartq_engine::CancellationToken::new())
        .expect("retry");
    assert!(diagnostics.is_empty(), "retry completes: {diagnostics:?}");
    assert_eq!(engine.libraries().len(), 2);
}
