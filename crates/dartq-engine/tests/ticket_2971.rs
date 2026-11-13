//! Ticket #2971: the directive-URI stringless faces — `uri_with_interpolation`
//! only for a literal whose string value is null (an interpolation), the
//! `ensureLiteralString` recovery splice for everything else (a synthetic
//! `''` that resolves to the container itself), and
//! `import_of_non_library`/`export_of_non_library` on an import/export whose
//! target file is a part.
//!
//! Upstream mirrors: the splice is `ensureLiteralString`
//! (upstream Dart SDK source, `_fe_analyzer_shared` parser_impl.dart
//! :5075-5103 — a synthetic empty string lands before the offending token,
//! so `part class A {}` resolves `''` = the container, and the oracle reports
//! `part_of_non_part` at the offending token, zero length, with the
//! container's canonical URI); the interpolation row is
//! `uriWithInterpolation.at(directive.uri)` — the whole literal region
//! (library_analyzer.dart:732/:954/:993); the non-library rows are
//! `_reportImportDirectiveErrors`/`_reportLibraryExportDirectiveErrors`
//! (library_analyzer.dart:701-744/:910-955) — `importedLibrarySource ==
//! null` exactly when the target file's kind is not `LibraryFileKind`
//! (`_updateKind`, file_state.dart:928-949), carrying the WRITTEN selected
//! URI string.

mod common;

use common::Fixture;
use dartq_engine::CancellationToken;

/// The rows of one code over the whole analyze run, as (file tail, offset,
/// length, message) — enough to pin code, anchor, and text without pinning
/// the faces this ticket does not own.
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

/// An identifier-form interpolation opening the literal (`'$x.dart'`) is the
/// shapeless face over the whole literal — the same whole-region anchor the
/// `${…}` head already pins (conformance `UriWithInterpolationTest`, oracle
/// rows `2:8|15`), never a zero-width row at offset 0.
#[test]
fn identifier_form_interpolation_spans_the_whole_literal() {
    const A: &str = "import '$x.dart';\nint f() => 1;\n";

    let fixture = Fixture::new(&[("lib/a.dart", A)]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/a.dart"), A.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    let rows = rows_of(&engine, &diagnostics, "uri_with_interpolation");
    assert_eq!(
        rows,
        vec![(
            "a.dart".to_owned(),
            7,
            9,
            "URIs can't use string interpolation.".to_owned()
        )],
        "the whole `'$x.dart'` literal, and nothing else: {diagnostics:?}"
    );
}

/// A directive with NO string token is the recovery splice, not the
/// interpolation face: `part class A {}` splices a synthetic `''`, which
/// composes to the container itself, and the container is not a part — so
/// the oracle face is `part_of_non_part` at the offending token (zero
/// length) with the container's own canonical URI
/// (conformance `PartDirectivesTest.test_part_directive_keyword_class`,
/// oracle rows `expected_string_literal 2:6|5`, `expected_token 2:1|4`,
/// `part_of_non_part 2:6|0`).
#[test]
fn missing_uri_literal_is_the_splice_not_interpolation() {
    const A: &str = "part class A {}\n";

    let fixture = Fixture::new(&[("lib/a.dart", A)]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/a.dart"), A.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert!(
        rows_of(&engine, &diagnostics, "uri_with_interpolation").is_empty(),
        "an absent literal never reports interpolation: {diagnostics:?}"
    );
    assert_eq!(
        rows_of(&engine, &diagnostics, "part_of_non_part"),
        vec![(
            "a.dart".to_owned(),
            5,
            0,
            "The included part 'package:fixture/lib/a.dart' must have a \
             part-of directive."
                .to_owned()
        )],
        "the spliced `''` resolves to the container: {diagnostics:?}"
    );
}

/// The import splice stays silent on a library container: `import class A
/// {}` splices `''`, the container imports itself, and a self-library
/// import reports nothing here (the oracle's follow-up is the
/// `unused_import` lint, not an engine row) — in particular no
/// `uri_with_interpolation` and no `import_of_non_library`.
#[test]
fn missing_import_uri_on_a_library_stays_resolution_silent() {
    const A: &str = "import class A {}\n";

    let fixture = Fixture::new(&[("lib/a.dart", A)]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/a.dart"), A.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert!(
        rows_of(&engine, &diagnostics, "uri_with_interpolation").is_empty(),
        "no interpolation face for the splice: {diagnostics:?}"
    );
    assert!(
        rows_of(&engine, &diagnostics, "import_of_non_library").is_empty(),
        "the container is a library, so the self-import is legal: {diagnostics:?}"
    );
}

/// Importing a part file reports `import_of_non_library` at the written URI
/// literal with the written spelling (conformance
/// `ImportOfNonLibraryTest.test_deferred` — `deferred as p` rides along,
/// the row is unchanged), and the orphan part's own `part of` stays silent.
#[test]
fn import_of_a_part_file_reports_on_the_written_uri() {
    const A: &str = "library lib;\nimport 'p.dart' deferred as p;\nint f() => 1;\n";
    const P: &str = "part of 'a.dart';\nint g() => 1;\n";

    let fixture = Fixture::new(&[("lib/a.dart", A), ("lib/p.dart", P)]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/a.dart"), A.as_bytes());
    engine.set_content(&fixture.path("lib/p.dart"), P.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert_eq!(
        rows_of(&engine, &diagnostics, "import_of_non_library"),
        vec![(
            "a.dart".to_owned(),
            20,
            8,
            "The imported library 'p.dart' can't have a part-of directive.".to_owned()
        )],
        "the deferred import face: {diagnostics:?}"
    );
}

/// The export twin (`ExportOfNonLibraryTest` and
/// `ExportDirectiveResolutionTest.test_inLibrary_notLibrary_partOfUri`).
#[test]
fn export_of_a_part_file_reports_on_the_written_uri() {
    const A: &str = "export 'p.dart';\n";
    const P: &str = "part of 'a.dart';\nint g() => 1;\n";

    let fixture = Fixture::new(&[("lib/a.dart", A), ("lib/p.dart", P)]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/a.dart"), A.as_bytes());
    engine.set_content(&fixture.path("lib/p.dart"), P.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert_eq!(
        rows_of(&engine, &diagnostics, "export_of_non_library"),
        vec![(
            "a.dart".to_owned(),
            7,
            8,
            "The exported library 'p.dart' can't have a part-of directive.".to_owned()
        )],
        "the export face: {diagnostics:?}"
    );
}

/// A/B: a legal import of a library stays silent — the emitter fires on the
/// target's file role, never on the directive shape.
#[test]
fn import_of_a_library_stays_silent() {
    const A: &str = "import 'b.dart';\nint f() => g();\n";
    const B: &str = "int g() => 1;\n";

    let fixture = Fixture::new(&[("lib/a.dart", A), ("lib/b.dart", B)]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/a.dart"), A.as_bytes());
    engine.set_content(&fixture.path("lib/b.dart"), B.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert!(
        rows_of(&engine, &diagnostics, "import_of_non_library").is_empty(),
        "a library target never reports: {diagnostics:?}"
    );
}

/// The in-part face under enhanced-parts (`ImportDirectiveResolutionTest.
/// test_inPart_notLibrary_partOfUri`): a part file's own import of another
/// part reports on the part file, at its import's URI literal.
#[test]
fn import_of_a_part_from_inside_a_part_reports_on_the_part() {
    const A: &str = "part 'b.dart';\nint f() => 1;\n";
    const B: &str = "part of 'a.dart';\nimport 'c.dart';\n";
    const C: &str = "part of 'a.dart';\nint h() => 1;\n";

    let fixture = Fixture::with_enhanced_parts(&[
        ("lib/a.dart", A),
        ("lib/b.dart", B),
        ("lib/c.dart", C),
    ]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/a.dart"), A.as_bytes());
    engine.set_content(&fixture.path("lib/b.dart"), B.as_bytes());
    engine.set_content(&fixture.path("lib/c.dart"), C.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert_eq!(
        rows_of(&engine, &diagnostics, "import_of_non_library"),
        vec![(
            "b.dart".to_owned(),
            25,
            8,
            "The imported library 'c.dart' can't have a part-of directive.".to_owned()
        )],
        "the in-part import face: {diagnostics:?}"
    );
}

/// A name-form `part of` carries its target in the name, never in a URI —
/// the resolution skip stays put: no `uri_with_interpolation` row, at any
/// offset (the pre-#2971 bug coded one at 0:0).
#[test]
fn name_form_part_of_never_reports_interpolation() {
    const P: &str = "part of foo.bar;\nint g() => 1;\n";

    let fixture = Fixture::new(&[("lib/p.dart", P)]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/p.dart"), P.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert!(
        rows_of(&engine, &diagnostics, "uri_with_interpolation").is_empty(),
        "the name form resolves nothing: {diagnostics:?}"
    );
}
