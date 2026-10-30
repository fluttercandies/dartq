//! Ticket #3039: the engine's URI-resolution residual — the ignore-comment
//! suppression over `facts.diagnostics`, and the `file:`/`package:` URI
//! identity that decides whether a `part` include attaches.
//!
//! Upstream mirrors:
//!
//! * `_filterIgnoredDiagnostics` (library_analyzer.dart:539-563) applies the
//!   unit's `IgnoreInfo` to EVERY diagnostic the file reports — URI-layer
//!   ERROR rows included — gated only by the governing options'
//!   `analyzer.cannot-ignore` set (`is_ignorable`, :552-555). A
//!   `// ignore_for_file: uri_does_not_exist` therefore drops the
//!   `uri_does_not_exist` rows the URI layer baked into `facts.diagnostics`;
//!   dartq's `diagnose_library` pushed them verbatim and over-fired.
//! * `isPartOf` is a FileState identity test (`uriFile == container.file`,
//!   file_state.dart:2248-2250) keyed on the URI a file was DISCOVERED
//!   under, not its path. A `part 'file:///p2.dart'` literal registers `p2`
//!   under a `file:`-uri FileState, so its `part of 'p1.dart'` resolves a
//!   `file:`-uri FileState for `p1` that is NOT the `package:`-canonical
//!   FileState `p1` was discovered as — `isPartOf` fails, the include does
//!   not attach, and the directive reports `part_of_different_library` with
//!   `enclosingFile.file.uriStr` (canonical) / `includedFile.uriStr`
//!   (file:-spelled). dartq interns targets by path, so the FileIds
//!   coincided and the include attached silently.

mod common;

use common::Fixture;
use dartq_engine::CancellationToken;

/// The rows of one code over the whole analyze run, as (file tail, offset,
/// length, message).
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

/// `// ignore_for_file: uri_does_not_exist` suppresses the baked URI-layer
/// row for a `part` directive whose target does not resolve (the oracle
/// drops it through `_filterIgnoredDiagnostics`; the case's own source
/// carries the citation at file scope, so position does not matter).
#[test]
fn ignore_for_file_suppresses_uri_does_not_exist() {
    const TEST: &str = "library lib;\n\npart 'missing.dart';\n\n// ignore_for_file: uri_does_not_exist\n";

    let fixture = Fixture::new(&[("lib/test.dart", TEST)]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/test.dart"), TEST.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert!(
        rows_of(&engine, &diagnostics, "uri_does_not_exist").is_empty(),
        "the for-file citation drops the baked row: {diagnostics:?}"
    );
}

/// A `// ignore:` on the line before an unresolvable `part` target drops
/// the row, exactly as the line-form `IgnoreInfo` citation does.
#[test]
fn ignore_line_suppresses_uri_does_not_exist() {
    const TEST: &str = "library lib;\n\n// ignore: uri_does_not_exist\npart 'missing.dart';\n";

    let fixture = Fixture::new(&[("lib/test.dart", TEST)]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/test.dart"), TEST.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert!(
        rows_of(&engine, &diagnostics, "uri_does_not_exist").is_empty(),
        "the line citation drops the baked row: {diagnostics:?}"
    );
}

/// Without any citation the same `part 'missing.dart'` still reports —
/// the suppression is gated on the source actually carrying `ignore:` /
/// `ignore_for_file:`, so an unrelated file keeps its row.
#[test]
fn an_unignored_uri_does_not_exist_still_reports() {
    const TEST: &str = "library lib;\n\npart 'missing.dart';\n";

    let fixture = Fixture::new(&[("lib/test.dart", TEST)]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/test.dart"), TEST.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert_eq!(
        rows_of(&engine, &diagnostics, "uri_does_not_exist").len(),
        1,
        "no citation => the baked row survives: {diagnostics:?}"
    );
}

/// `part 'file:///…/p2.dart'` includes a part discovered under a `file:`
/// URI while the includer resolves `package:`-canonically — the FileState
/// identities differ, so `isPartOf` fails and the directive reports
/// `part_of_different_library` with the canonical `enclosingFile.uriStr`
/// and the file:-spelled `includedFile.uriStr`. dartq must not attach it.
#[test]
fn a_file_uri_part_include_reports_part_of_different_library() {
    // `lib.dart` is the includer (canonical `package:`); `p2.dart` is its
    // part, claiming `part of 'lib.dart'` — a path-equal claim on the
    // includer. The include literal is `file:`-schemed, so upstream's
    // FileState URI identity fails and the directive reports the mismatch.
    let fixture = Fixture::new(&[("lib/p2.dart", "part of 'lib.dart';")]);
    let p2 = fixture.path("lib/p2.dart");
    let lib = format!("part 'file://{}';", p2.to_string_lossy());

    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/lib.dart"), lib.as_bytes());
    engine.set_content(&p2, "part of 'lib.dart';".as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    let rows = rows_of(&engine, &diagnostics, "part_of_different_library");
    assert_eq!(rows.len(), 1, "the file:-include reports one mismatch row: {diagnostics:?}");
    // `{1}` is the included file's discovery URI — `file:`-spelled, not the
    // `package:` canonical spelling `canonical_uri_spelling` would produce.
    assert!(
        rows[0].3.contains("file://"),
        "the included URI keeps the file: discovery spelling: {rows:?}"
    );
    assert!(
        rows[0].3.contains(&p2.to_string_lossy().to_string()),
        "the included URI names p2's file: path: {rows:?}"
    );
}

/// The matching positive: a `part 'p2.dart'` relative include under a
/// `package:`-canonical includer resolves the include under the SAME
/// canonical scheme as the part-of claim — `isPartOf` holds, the part
/// joins silently, and no `part_of_different_library` emits.
#[test]
fn a_relative_part_include_attaches_silently() {
    let fixture = Fixture::new(&[
        ("lib/lib.dart", "part 'p2.dart';"),
        ("lib/p2.dart", "part of 'lib.dart';"),
    ]);

    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/lib.dart"), "part 'p2.dart';".as_bytes());
    engine.set_content(&fixture.path("lib/p2.dart"), "part of 'lib.dart';".as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert!(
        rows_of(&engine, &diagnostics, "part_of_different_library").is_empty(),
        "a same-scheme include attaches, no mismatch row: {diagnostics:?}"
    );
}
