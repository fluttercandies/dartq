//! Incremental gate pins (goal #88): the `ContentId` no-op gate, the
//! single-file recompute after one edit, the cascade reach, and the
//! apply+analyze vs analyze-alone byte-equality.

mod common;

use common::{EventLog, Fixture, GREETER_0, GREETER_1, MAIN_0, STANDALONE_0};
use dartq_engine::{CancellationToken, RevalidateGate};

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
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert!(clean.is_empty());

    // Rewrite with identical bytes: legal changeFile, zero cascade.
    let greeter = fixture.path("lib/greeter.dart");
    engine.set_content(&greeter, GREETER_0.as_bytes());

    let mut log = EventLog::default();
    let diagnostics = engine
        .analyze(&[], &mut log.sink(), &CancellationToken::default())
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
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");

    // Change one file: only it revalidates; the L2 signature gate (the
    // cascade rule) stops the cascade there — the edit keeps greet's element face,
    // so the importer keeps its result.
    engine.set_content(&fixture.path("lib/greeter.dart"), GREETER_1.as_bytes());

    // The untouched libraries ride along as the AnalyzeFile targets: targets
    // drive only the amplification events, and a clean targeted library
    // reports a cache hit.
    let target = engine
        .file_table()
        .try_file_id(&fixture.path("lib/standalone.dart"))
        .expect("interned");

    let mut log = EventLog::default();
    let diagnostics = engine
        .analyze(&[target], &mut log.sink(), &CancellationToken::default())
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
    // Cascade: the signature gate keeps it at the edited library.
    let cascade = log.cascades.into_iter().next().expect("one cascade event");
    assert_eq!(cascade.len(), 1, "the edit keeps greet's face: no cascade");
    assert_eq!(
        log.analyzed
            .iter()
            .filter(|(_, from_cache)| !*from_cache)
            .count(),
        1,
        "exactly the edited library recomputed"
    );
    assert!(
        log.analyzed.iter().any(|(_, from_cache)| *from_cache),
        "the clean target library reports a cache hit"
    );

    // The unchanged library kept its cached result: analyze again with the
    // same content and observe zero work.
    let mut again = EventLog::default();
    engine
        .analyze(&[], &mut again.sink(), &CancellationToken::default())
        .expect("settle");
    assert!(again.revalidated.is_empty() && again.analyzed.iter().all(|(_, hit)| *hit));
}

/// The directive face rides the API signature: flipping
/// an export combinator changes the file's contribution to its importers'
/// namespaces while its element face stands, so the signature gate must
/// still seed the cascade — without the directive face in the hash, b would
/// keep a stale namespace and silently miss the `undefined_function`.
#[test]
fn reexport_combinator_change_reaches_importers() {
    const X: &str = "int x() => 1;\n";
    const A_SHOW: &str = "export 'x.dart' show x;\n";
    const A_HIDE: &str = "export 'x.dart' hide x;\n";
    const B: &str = "import 'a.dart';\nint g() => x();\n";

    let fixture = Fixture::new(&[("lib/x.dart", X), ("lib/a.dart", A_SHOW), ("lib/b.dart", B)]);
    let a_path = fixture.path("lib/a.dart");
    let b_path = fixture.path("lib/b.dart");
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/x.dart"), X.as_bytes());
    engine.set_content(&a_path, A_SHOW.as_bytes());
    engine.set_content(&b_path, B.as_bytes());
    let cold = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert!(cold.is_empty(), "{cold:?}");

    // `show x` → `hide x`: a's element face is unchanged; only its
    // directive face moved — exactly the face the signature folds.
    engine.set_content(&a_path, A_HIDE.as_bytes());
    let mut log = EventLog::default();
    let diagnostics = engine
        .analyze(&[], &mut log.sink(), &CancellationToken::default())
        .expect("flip");
    let b = engine.file_table().try_file_id(&b_path).expect("interned");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.file == b && d.code.name == "undefined_function"),
        "b must see x() vanish: {diagnostics:?}"
    );
    assert_eq!(log.cascades.len(), 1, "one cascade");
    assert_eq!(log.cascades[0].len(), 2, "a and its importer b recompute");

    // The revert heals b the same way.
    engine.set_content(&a_path, A_SHOW.as_bytes());
    let mut log = EventLog::default();
    let healed = engine
        .analyze(&[], &mut log.sink(), &CancellationToken::default())
        .expect("revert");
    assert!(healed.is_empty(), "{healed:?}");
    assert_eq!(log.cascades[0].len(), 2, "a and b recompute again");
}

#[test]
fn explicit_apply_then_analyze_matches_analyze_alone() {
    let fixture = Fixture::new(&[("lib/main.dart", MAIN_0), ("lib/greeter.dart", GREETER_1)]);
    let mut solo = fixture.engine();
    solo.discover_workspace().expect("discovery");
    let mut solo_log = EventLog::default();
    let solo_result = solo
        .analyze(&[], &mut solo_log.sink(), &CancellationToken::default())
        .expect("solo");

    let mut split = fixture.engine();
    split.discover_workspace().expect("discovery");
    let mut apply_log = EventLog::default();
    let report = split.apply_pending(&mut apply_log.sink());
    let mut analyze_log = EventLog::default();
    let split_result = split
        .analyze(&[], &mut analyze_log.sink(), &CancellationToken::default())
        .expect("split");

    // Byte-identical diagnostics, single OverlayApplied across
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

#[test]
fn cancel_then_remove_then_analyze_does_not_panic() {
    // Audit #230 regression: a cancelled analyze leaves its unfinished dirty
    // set queued; retiring one of those libraries (removing its defining
    // file) must drop the leftover, not resurrect it — analyze unwraps every
    // queued id's info, and a retired id has none.
    let fixture = Fixture::new(&[
        ("lib/a.dart", "import 'b.dart';\nint f() => g();\n"),
        ("lib/b.dart", "int g() => 1;\n"),
    ]);
    let mut engine = fixture.engine();
    engine.discover_workspace().expect("discovery");
    engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");

    // Edit the importer: only its library goes stale, and a pre-cancelled
    // analyze queues it as the unfinished leftover.
    engine.set_content(
        &fixture.path("lib/a.dart"),
        b"import 'b.dart';\nint f() => g() + 0;\n",
    );
    let token = dartq_engine::CancellationToken::new();
    token.cancel();
    assert!(matches!(
        engine.analyze(&[], &mut |_| {}, &token),
        Err(dartq_engine::Cancelled)
    ));

    // Removing the importer retires its library id; the next apply's dirty
    // set must not keep the dead id.
    engine
        .remove_file(&fixture.path("lib/a.dart"))
        .expect("removal enqueued");
    let report = engine.apply_pending(&mut |_| {});
    assert!(
        report.invalidated.is_empty(),
        "the retired id left the dirty set: {:?}",
        report.invalidated
    );

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &dartq_engine::CancellationToken::new())
        .expect("analyze after remove");
    assert!(diagnostics.is_empty());
    assert_eq!(engine.libraries().len(), 1, "only b's library remains");
}

/// Name-form `part of` belonging (the enhanced-parts world, where the
/// include-face mismatch rows are off): a name-form
/// part file joins the same-package library whose declared library name
/// matches, and stays an orphan singleton when none does — the URI include
/// (`part 'pn.dart';`) alone must not claim it. The one diagnostic row is
/// the name-form directive itself: with the feature granted, upstream
/// reports `part_of_name` on the dotted name (upstream Dart SDK source,
/// ast_builder.dart `endPartOf`, the `Feature.enhanced_parts` branch),
/// while the include's name-form mismatch row is above the reporting
/// horizon (library_analyzer.dart `_resolvePartDirective`, the
/// `Feature.enhanced_parts` branch); the clean contents bake nothing else.
#[test]
fn name_form_part_bounded_name_scan() {
    const N_P8: &str = "library p8;\npart 'pn.dart';\nint f() => 1;\n";
    const N_P9: &str = "library p9;\npart 'pn.dart';\nint f() => 1;\n";
    const PN_P9: &str = "part of p9;\nint g() => 1;\n";
    const PN_P8: &str = "part of p8;\nint g() => 1;\n";

    // The name-form directive's own row — identical for `part of p9;` and
    // `part of p8;` (the dotted name starts after the 8-code-unit
    // `part of ` prefix).
    let expect_part_of_name_row = |diagnostics: &[dartq_diagnostics::Diagnostic], pn| {
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let row = &diagnostics[0];
        assert_eq!(row.file, pn, "the row anchors on the dotted name");
        assert_eq!(row.code.name, "part_of_name");
        assert_eq!(row.range.offset, 8);
        assert_eq!(row.range.length, 2);
    };

    let fixture = Fixture::with_enhanced_parts(&[("lib/n.dart", N_P8), ("lib/pn.dart", PN_P9)]);
    let n_path = fixture.path("lib/n.dart");
    let pn_path = fixture.path("lib/pn.dart");
    let mut engine = fixture.engine();
    engine.set_content(&n_path, N_P8.as_bytes());
    engine.set_content(&pn_path, PN_P9.as_bytes());
    let n = engine.file_table().try_file_id(&n_path).expect("interned");
    let pn = engine.file_table().try_file_id(&pn_path).expect("interned");

    // Cold: p9 names no library — pn stays its own singleton even though
    // n's URI-resolved include points at it (no force-claim).
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    expect_part_of_name_row(&diagnostics, pn);
    assert_eq!(
        engine.libraries().len(),
        2,
        "n plus the pn orphan singleton"
    );
    assert_ne!(
        engine.library_of(n),
        engine.library_of(pn),
        "the unmatched name-form part is no member of n"
    );

    // The part-of name now matches n's declared name: pn joins via the
    // bounded name scan, and only n's library dirties (the retired singleton
    // id must not ride the cascade).
    engine.set_content(&pn_path, PN_P8.as_bytes());
    let mut log = EventLog::default();
    let diagnostics = engine
        .analyze(&[], &mut log.sink(), &CancellationToken::default())
        .expect("join");
    expect_part_of_name_row(&diagnostics, pn);
    assert_eq!(engine.library_of(n), engine.library_of(pn), "pn joined n");
    let joined = engine.library_of(n).expect("live");
    assert_eq!(
        engine
            .libraries()
            .into_iter()
            .find(|(id, _)| *id == joined)
            .map(|(_, members)| members)
            .expect("live"),
        vec![n, pn],
        "pn is a member, not an unclaimed part target"
    );
    let joined = engine.library_of(n).expect("live");
    assert_eq!(
        log.cascades,
        vec![vec![joined]],
        "the join dirties exactly n"
    );

    // n renames its library away from the part-of name: pn leaves to a fresh
    // singleton (the C8 step-2 shape), and both libraries dirty.
    engine.set_content(&n_path, N_P9.as_bytes());
    let mut log = EventLog::default();
    let diagnostics = engine
        .analyze(&[], &mut log.sink(), &CancellationToken::default())
        .expect("leave");
    expect_part_of_name_row(&diagnostics, pn);
    assert_eq!(engine.libraries().len(), 2, "n plus the fresh pn singleton");
    assert_ne!(
        engine.library_of(n),
        engine.library_of(pn),
        "the stale name-form part is no member of n"
    );
    let orphan = engine.library_of(pn).expect("live");
    assert_eq!(log.cascades.len(), 1, "one cascade");
    assert_eq!(log.cascades[0].len(), 2, "n and the fresh singleton dirty");
    assert!(
        log.cascades[0].contains(&orphan),
        "the new singleton dirties"
    );
}

/// The depth-2 chain joins through the attached middle: `root ← mid ← leaf`
/// where leaf's `part of 'mid.dart'` binds to its direct includer — the
/// URI-form claim is against the including file, not the defining library
/// (upstream file_state.dart's `LibraryFileKind` part check; the lints-new
/// `deprecated_member_use_from_same_package/part_mid.dart` shape carries
/// no `part_of_different_library`). The middle's own `part` directive
/// keeps its `non_part_of_directive_in_part` row — the oracle emits it on
/// this exact shape — and the leaf still joins the library through it.
#[test]
fn depth2_part_chain_joins_through_the_attached_middle() {
    const ROOT: &str = "library root;\npart 'mid.dart';\nint f() => 1;\n";
    const MID: &str = "part of 'root.dart';\npart 'leaf.dart';\n";
    const LEAF: &str = "part of 'mid.dart';\nint g() => 1;\n";

    let fixture = Fixture::new(&[
        ("lib/root.dart", ROOT),
        ("lib/mid.dart", MID),
        ("lib/leaf.dart", LEAF),
    ]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/root.dart"), ROOT.as_bytes());
    engine.set_content(&fixture.path("lib/mid.dart"), MID.as_bytes());
    engine.set_content(&fixture.path("lib/leaf.dart"), LEAF.as_bytes());
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    // The plate row stays gone: the include's own `non_part_of_directive_
    // in_part` fires (the oracle emits it on this exact shape) and NO
    // `part_of_different_library` — the URI-form claim binds to the direct
    // includer, so the leaf never reports the defining-library mismatch.
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(
        diagnostics[0].code.name,
        "non_part_of_directive_in_part",
        "{diagnostics:?}"
    );
}

/// The same include face below the enhanced-parts horizon (package language
/// 3.5, no grants): the URI include of a name-form part whose part-of name
/// does not match the includer's declared library name reports
/// `part_of_different_library` on the include's URI literal — the oracle's
/// ungated-at-3.5 row (upstream Dart SDK source, library_analyzer.dart
/// `_resolvePartDirective`, the `!featureSet.isEnabled(Feature.enhanced_parts)`
/// branch) — while the include still claims nothing: the unmatched part
/// stays an orphan singleton, and matching the name heals the row and joins
/// it through the bounded name scan.
#[test]
fn name_form_include_mismatch_reports_below_enhanced_parts() {
    const N_P8: &str = "library p8;\npart 'pn.dart';\nint f() => 1;\n";
    const PN_P9: &str = "part of p9;\nint g() => 1;\n";
    const PN_P8: &str = "part of p8;\nint g() => 1;\n";

    let fixture = Fixture::new(&[("lib/n.dart", N_P8), ("lib/pn.dart", PN_P9)]);
    let pn_path = fixture.path("lib/pn.dart");
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/n.dart"), N_P8.as_bytes());
    engine.set_content(&pn_path, PN_P9.as_bytes());
    let n = engine
        .file_table()
        .try_file_id(&fixture.path("lib/n.dart"))
        .expect("interned");
    let pn = engine.file_table().try_file_id(&pn_path).expect("interned");

    // Cold: the mismatch row fires on n's include URI; pn stays an orphan.
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    let row = &diagnostics[0];
    assert_eq!(row.file, n, "the row anchors on the include directive");
    assert_eq!(row.code.name, "part_of_different_library");
    assert_eq!(
        row.message.as_str(),
        "Expected this library to be part of 'p8', not 'p9'."
    );
    assert_eq!(row.range.offset, 17);
    assert_eq!(row.range.length, 9);
    assert_eq!(
        engine.libraries().len(),
        2,
        "n plus the pn orphan singleton"
    );
    assert_ne!(engine.library_of(n), engine.library_of(pn));

    // The part-of name matching the declared library heals the row and
    // joins pn through the name scan.
    engine.set_content(&pn_path, PN_P8.as_bytes());
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("join");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(engine.library_of(n), engine.library_of(pn), "pn joined n");
    let joined = engine.library_of(n).expect("live");
    assert_eq!(
        engine
            .libraries()
            .into_iter()
            .find(|(id, _)| *id == joined)
            .map(|(_, members)| members)
            .expect("live"),
        vec![n, pn]
    );
}

/// Interpolated `part` include (#251 pin): an includer whose `part`
/// directive carries no resolvable string (`'${x}.dart'` — the
/// `uri_with_interpolation` face) claims nothing; the part file it names
/// stays an orphan singleton even though its own `part of` URI resolves
/// back to the includer. The claim needs both halves — the resolved include
/// and the resolved back-reference (`PartOfUriKnownFileKind::isPartOf`,
/// file_state.dart:2262-2265).
#[test]
fn interpolated_part_include_claims_nothing() {
    const A: &str = "part '${x}.dart';\nint f() => 1;\n";
    const P: &str = "part of 'a.dart';\nint g() => 1;\n";

    let fixture = Fixture::new(&[("lib/a.dart", A), ("lib/p.dart", P)]);
    let a_path = fixture.path("lib/a.dart");
    let p_path = fixture.path("lib/p.dart");
    let mut engine = fixture.engine();
    engine.set_content(&a_path, A.as_bytes());
    engine.set_content(&p_path, P.as_bytes());
    let a = engine.file_table().try_file_id(&a_path).expect("interned");
    let p = engine.file_table().try_file_id(&p_path).expect("interned");

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.file == a
                && diagnostic.code.name == "uri_with_interpolation"),
        "the interpolated include bakes the interpolation face: {diagnostics:?}"
    );
    assert_eq!(engine.libraries().len(), 2, "a plus the p orphan singleton");
    assert_ne!(
        engine.library_of(a),
        engine.library_of(p),
        "the unresolved include claims no member"
    );
    let lib_a = engine.library_of(a).expect("live");
    assert_eq!(
        engine
            .libraries()
            .into_iter()
            .find(|(id, _)| *id == lib_a)
            .map(|(_, members)| members)
            .expect("live"),
        vec![a],
        "a's library has no parts"
    );
    assert_eq!(
        engine.library_defining_file(engine.library_of(p).expect("live")),
        Some(p),
        "p is its own orphan singleton"
    );
}

/// Interpolated `part of` (#251 pin): a part file whose `part of` carries no
/// resolvable string (`part of '${x}.dart';` — shapeless, no URI and no
/// name) bakes the `uri_with_interpolation` face and stays an orphan
/// singleton; the includer's resolvable `part 'p.dart';` alone claims
/// nothing, mirroring `PartOfUriUnknownFileKind`
/// (file_state.dart:2283-2291).
#[test]
fn interpolated_part_of_is_an_orphan_singleton() {
    const A: &str = "part 'p.dart';\nint f() => 1;\n";
    const P: &str = "part of '${x}.dart';\nint g() => 1;\n";

    let fixture = Fixture::new(&[("lib/a.dart", A), ("lib/p.dart", P)]);
    let a_path = fixture.path("lib/a.dart");
    let p_path = fixture.path("lib/p.dart");
    let mut engine = fixture.engine();
    engine.set_content(&a_path, A.as_bytes());
    engine.set_content(&p_path, P.as_bytes());
    let a = engine.file_table().try_file_id(&a_path).expect("interned");
    let p = engine.file_table().try_file_id(&p_path).expect("interned");

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.file == p
                && diagnostic.code.name == "uri_with_interpolation"),
        "the shapeless part of bakes the interpolation face: {diagnostics:?}"
    );
    assert_eq!(engine.libraries().len(), 2, "a plus the p orphan singleton");
    assert_ne!(
        engine.library_of(a),
        engine.library_of(p),
        "the include alone claims no shapeless part"
    );
    assert_eq!(
        engine.library_defining_file(engine.library_of(p).expect("live")),
        Some(p),
        "p is its own orphan singleton"
    );
}
