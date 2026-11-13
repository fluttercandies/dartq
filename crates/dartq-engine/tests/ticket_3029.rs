//! Ticket #3029: file-role verdict precision — the part-of URI face and the
//! claim gate's `_updateKind` precedence.
//!
//! Upstream mirrors:
//!
//! * A `part of '<uri>'` never carries a URI diagnostic — the analyzer only
//!   *classifies* the file with it (`FileState._updateKind`,
//!   file_state.dart:928-953: `PartOfUriKnownFileKind` /
//!   `PartOfUriUnknownFileKind`); `_resolvePartDirective`
//!   (library_analyzer.dart:975-1080) covers `part` directives only. A
//!   missing target therefore reports NOTHING — no `uri_does_not_exist`,
//!   no stale-overlay row either.
//! * A `library` directive classifies the file as `LibraryFileKind`
//!   outright (file_state.dart:937-939): a `part of` in the same file is
//!   dead weight, `isPartOf` does not exist on the library kinds, and the
//!   file claims into nothing — it forms its own defining library while
//!   the includer reports `part_of_non_part`. Claiming it anyway emits the
//!   file's diagnostics under two libraries' runs (the duplicate-row bug).

mod common;

use common::Fixture;
use dartq_engine::CancellationToken;

/// The rows of one code over the whole analyze run, as (file tail, offset,
/// length, message) — the carrier `ticket_3010.rs` established.
fn rows_of(
    engine: &dartq_engine::Engine,
    diagnostics: &[dartq_diagnostics::Diagnostic],
    code: &str,
) -> Vec<(String, u32, u32, String)> {
    diagnostics
        .iter()
        .filter(|row| row.code.name == code)
        .map(|row| {
            let tail = engine
                .file_table()
                .path(row.file)
                .map(|path| {
                    path.to_string_lossy()
                        .rsplit('/')
                        .next()
                        .unwrap_or_default()
                        .to_owned()
                })
                .unwrap_or_default();
            (tail, row.range.offset, row.range.length, row.message.to_string())
        })
        .collect()
}

/// `part of 'missing.dart';` — the URI resolves to a file that does not
/// exist; the oracle reports nothing (the part-of URI only classifies).
/// The staleness arm must not translate the resolved-but-nonresident
/// target into `uri_does_not_exist`.
#[test]
fn part_of_a_missing_uri_reports_nothing() {
    const P: &str = "part of 'missing.dart';\nint g() => 1;\n";

    let fixture = Fixture::new(&[("lib/p.dart", P)]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/p.dart"), P.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    for code in [
        "uri_does_not_exist",
        "uri_has_not_been_generated",
        "uri_with_interpolation",
        "invalid_uri",
    ] {
        assert!(
            rows_of(&engine, &diagnostics, code).is_empty(),
            "{code} must stay silent — a part-of URI only classifies: {diagnostics:?}"
        );
    }
}

/// `library; part of 'test.dart';` inside an included file — `_updateKind`
/// makes it a LIBRARY, so the includer's `part 'b.dart'` reports
/// `part_of_non_part` and the file claims into nothing: its own
/// `non_part_of_directive_in_part` parser row emits exactly once (under
/// its own library's run), never duplicated through the includer's
/// member list.
#[test]
fn a_library_file_with_a_part_of_claims_nothing() {
    const LIB: &str = "library test;\npart 'b.dart';\n";
    const B: &str = "library;\npart of 'test.dart';\n";

    let fixture = Fixture::new(&[("lib/test.dart", LIB), ("lib/b.dart", B)]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/test.dart"), LIB.as_bytes());
    engine.set_content(&fixture.path("lib/b.dart"), B.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert_eq!(
        rows_of(&engine, &diagnostics, "non_part_of_directive_in_part"),
        vec![(
            "b.dart".to_owned(),
            9,
            4,
            "The part-of directive must be the only directive in a part.".to_owned()
        )],
        "the parser row emits once — the file is a library, not a member: {diagnostics:?}"
    );
    assert_eq!(
        rows_of(&engine, &diagnostics, "part_of_non_part"),
        vec![(
            "test.dart".to_owned(),
            19,
            8,
            "The included part 'package:fixture/lib/b.dart' must have a part-of directive."
                .to_owned()
        )],
        "the includer reports the non-part face: {diagnostics:?}"
    );
    assert!(
        rows_of(&engine, &diagnostics, "uri_does_not_exist").is_empty(),
        "the part-of URI resolved — nothing stale: {diagnostics:?}"
    );
}

/// The name-form twin: `library other; part of test;` still classifies as
/// a library (the directive kinds are order-blind), so the includer
/// reports `part_of_non_part` and the file's parser row emits once.
#[test]
fn a_library_file_with_a_name_part_of_claims_nothing() {
    const LIB: &str = "library test;\npart 'b.dart';\n";
    const B: &str = "library other;\npart of test;\n";

    let fixture = Fixture::new(&[("lib/test.dart", LIB), ("lib/b.dart", B)]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/test.dart"), LIB.as_bytes());
    engine.set_content(&fixture.path("lib/b.dart"), B.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert_eq!(
        rows_of(&engine, &diagnostics, "non_part_of_directive_in_part")
            .iter()
            .map(|(file, ..)| file.clone())
            .collect::<Vec<_>>(),
        vec!["b.dart".to_owned()],
        "the parser row emits once: {diagnostics:?}"
    );
    assert_eq!(
        rows_of(&engine, &diagnostics, "part_of_non_part").len(),
        1,
        "the includer reports the non-part face: {diagnostics:?}"
    );
}

/// A clean `part of 'test.dart';` attach: the claim joins test's library,
/// no part-family rows fire, and the part's own diagnostics emit once
/// (inside test's run — the one owning library).
#[test]
fn a_clean_part_attach_emits_its_rows_once() {
    const LIB: &str = "library test;\npart 'b.dart';\n";
    const B: &str = "part of 'test.dart';\ndeprecated_member_use_trigger_undefined x y z;\n";

    let fixture = Fixture::new(&[("lib/test.dart", LIB), ("lib/b.dart", B)]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/test.dart"), LIB.as_bytes());
    engine.set_content(&fixture.path("lib/b.dart"), B.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    for code in [
        "part_of_non_part",
        "part_of_different_library",
        "duplicate_part",
        "uri_does_not_exist",
    ] {
        assert!(
            rows_of(&engine, &diagnostics, code).is_empty(),
            "{code} must stay silent — the part attached cleanly: {diagnostics:?}"
        );
    }
    // One row per parser diagnostic: the member is emitted through the
    // owning library's run only — no orphan shadow.
    let parser_rows: Vec<_> = diagnostics
        .iter()
        .filter(|row| {
            engine
                .file_table()
                .path(row.file)
                .is_some_and(|path| path.ends_with("b.dart"))
        })
        .collect();
    let mut seen = std::collections::HashSet::new();
    for row in &parser_rows {
        assert!(
            seen.insert((row.code.name, row.range.offset)),
            "duplicated row under two library runs: {diagnostics:?}"
        );
    }
}
