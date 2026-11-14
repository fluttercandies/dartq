//! Ticket #2998: the emit wiring for the landed file-role and export-graph
//! faces — the parser's whole-`partOfName` span, the `ResolvedTarget::
//! External` arms of the part family and the non-library import/export rows
//! (SDK `dart:` parts probed through `SdkFileKinds`), the direct-includer
//! claim gate for nested `part` directives, and the export-graph verifier
//! rows (`ambiguous_export`, `export_internal_library`,
//! `deferred_import_of_extension`).
//!
//! Upstream mirrors: `diag.partOfName.at(name)` spans the whole dotted name
//! (ast_builder.dart:2819-2826); the part/include matrix is
//! `library_analyzer.dart` `_resolvePartDirective` (:1015-1113) over
//! `FileState._updateKind` (file_state.dart:928-949), applied to SDK file
//! kinds exactly like workspace ones; the export rows are
//! `error_verifier.dart` `_checkForAmbiguousExport` (:2593-2621),
//! `exportInternalLibrary` (:4645-4674), and
//! `_checkForDeferredImportOfExtensions` (:1339-1356/:4393-4405).

mod common;

use common::Fixture;
use dartq_engine::CancellationToken;

/// The rows of one code over the whole analyze run, as (file tail, offset,
/// length, message) — the same carrier `ticket_2971.rs` uses: code, anchor,
/// and text without pinning faces this ticket does not own.
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
            (
                tail,
                row.range.offset,
                row.range.length,
                row.message.to_string(),
            )
        })
        .collect()
}

/// `part of foo.bar;` under enhanced-parts reports `part_of_name` over the
/// WHOLE dotted name (`foo.bar` = offset 8, length 7), not just the head
/// identifier — the ~39-case conformance face this span fix lands.
#[test]
fn part_of_name_spans_the_whole_dotted_name() {
    const P: &str = "part of foo.bar;\nint g() => 1;\n";

    let fixture = Fixture::with_enhanced_parts(&[("lib/p.dart", P)]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/p.dart"), P.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert_eq!(
        rows_of(&engine, &diagnostics, "part_of_name"),
        vec![(
            "p.dart".to_owned(),
            8,
            7,
            "The 'part of' directive can't use a name with the enhanced-parts \
             feature."
                .to_owned()
        )],
        "the whole `foo.bar` region: {diagnostics:?}"
    );
}

/// `part 'dart:math'` — the SDK entry is a library (`SdkFileKind::Library`),
/// so the include reports `part_of_non_part` at the URI literal carrying
/// the canonical `dart:` spelling (`includedFile.uriStr`).
#[test]
fn part_of_an_sdk_library_reports_non_part() {
    const A: &str = "part 'dart:math';\nint f() => 1;\n";

    let fixture = Fixture::new(&[
        ("lib/a.dart", A),
        ("sdk/lib/math/math.dart", "library dart.math;\n"),
    ]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/a.dart"), A.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert_eq!(
        rows_of(&engine, &diagnostics, "part_of_non_part"),
        vec![(
            "a.dart".to_owned(),
            5,
            11,
            "The included part 'dart:math' must have a part-of directive.".to_owned()
        )],
        "the SDK library include: {diagnostics:?}"
    );
}

/// `part 'dart:async/future.dart'` — the SDK file is a URI-form part
/// (`part of "dart:async";`), which no workspace container can claim:
/// `part_of_different_library` with the INCLUDING file's canonical URI and
/// the included SDK file's own `dart:` spelling.
#[test]
fn part_of_an_sdk_uri_part_reports_different_library() {
    const A: &str = "part 'dart:async/future.dart';\nint f() => 1;\n";

    let fixture = Fixture::new(&[
        ("lib/a.dart", A),
        ("sdk/lib/async/future.dart", "part of \"dart:async\";\n"),
    ]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/a.dart"), A.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    let rows = rows_of(&engine, &diagnostics, "part_of_different_library");
    assert_eq!(rows.len(), 1, "one URI-mismatch row: {diagnostics:?}");
    let (file, offset, length, message) = &rows[0];
    assert_eq!(file, "a.dart");
    assert_eq!((*offset, *length), (5, 24));
    assert!(
        message.ends_with("'dart:async/future.dart'."),
        "the included file's own dart: spelling is `{{1}}`: {message}"
    );
    assert!(
        message.contains("part of 'package:fixture/lib/a.dart'"),
        "the including file's canonical URI is `{{0}}`: {message}"
    );
}

/// `import 'dart:async/future.dart'` — a `dart:` part file is a non-library
/// import target the same way a workspace part is: `import_of_non_library`
/// with the written URI string.
#[test]
fn import_of_an_sdk_part_reports_non_library() {
    const A: &str = "import 'dart:async/future.dart';\nint f() => 1;\n";

    let fixture = Fixture::new(&[
        ("lib/a.dart", A),
        ("sdk/lib/async/future.dart", "part of \"dart:async\";\n"),
    ]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/a.dart"), A.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert_eq!(
        rows_of(&engine, &diagnostics, "import_of_non_library"),
        vec![(
            "a.dart".to_owned(),
            7,
            24,
            "The imported library 'dart:async/future.dart' can't have a \
             part-of directive."
                .to_owned()
        )],
        "the SDK part import: {diagnostics:?}"
    );
}

/// The export twin on the same SDK part target.
#[test]
fn export_of_an_sdk_part_reports_non_library() {
    const A: &str = "export 'dart:async/future.dart';\n";

    let fixture = Fixture::new(&[
        ("lib/a.dart", A),
        ("sdk/lib/async/future.dart", "part of \"dart:async\";\n"),
    ]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/a.dart"), A.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert_eq!(
        rows_of(&engine, &diagnostics, "export_of_non_library"),
        vec![(
            "a.dart".to_owned(),
            7,
            24,
            "The exported library 'dart:async/future.dart' can't have a \
             part-of directive."
                .to_owned()
        )],
        "the SDK part export: {diagnostics:?}"
    );
}

/// An SDK name-form part (`part of dart.developer;`) attaches to a library
/// named the same — silent — and reports `part_of_different_library` under
/// any other name (and `part_of_unnamed_library` under no name).
#[test]
fn name_form_sdk_part_attaches_by_library_name() {
    const ATTACHED: &str =
        "library dart.developer;\npart 'dart:developer/creation_tracking.dart';\n";
    const MISMATCHED: &str = "library other.lib;\npart 'dart:developer/creation_tracking.dart';\n";
    const UNNAMED: &str = "part 'dart:developer/creation_tracking.dart';\n";

    for (label, content, expected) in [
        ("attached", ATTACHED, None),
        ("mismatched", MISMATCHED, Some("part_of_different_library")),
        ("unnamed", UNNAMED, Some("part_of_unnamed_library")),
    ] {
        let fixture = Fixture::new(&[
            ("lib/a.dart", content),
            (
                "sdk/lib/developer/creation_tracking.dart",
                "part of dart.developer;\n",
            ),
        ]);
        let mut engine = fixture.engine();
        engine.set_content(&fixture.path("lib/a.dart"), content.as_bytes());

        let diagnostics = engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("cold");
        let family: Vec<_> = diagnostics
            .iter()
            .filter(|row| {
                matches!(
                    row.code.name,
                    "part_of_different_library" | "part_of_unnamed_library" | "part_of_non_part"
                )
            })
            .collect();
        match expected {
            None => assert!(
                family.is_empty(),
                "{label}: a matching library name attaches silently: {diagnostics:?}"
            ),
            Some(code) => {
                assert_eq!(
                    family.len(),
                    1,
                    "{label}: exactly one {code} row: {diagnostics:?}"
                );
                assert_eq!(family[0].code.name, code, "{label}: {diagnostics:?}");
            }
        }
    }
}

/// Depth-2 include + container-is-library: `mid` is a URI-form part of the
/// root and carries `part 'leaf.dart'` of its own. `leaf` declares
/// `part of my.lib;` — a NAME-form part can only be claimed by a library
/// container (`isPartOf` is `container is LibraryFileKind`), so the part
/// includer can never claim it even though the names match: the row is
/// `part_of_different_library` at mid's directive, carrying the defining
/// library's name on both sides (upstream reports
/// `{expectedName: 'my.lib', actualName: 'my.lib'}` verbatim).
#[test]
fn nested_name_form_part_under_a_part_includer_reports() {
    const A: &str = "library my.lib;\npart 'mid.dart';\nint f() => 1;\n";
    const MID: &str = "part of 'a.dart';\npart 'leaf.dart';\n";
    const LEAF: &str = "part of my.lib;\nint g() => 1;\n";

    let fixture = Fixture::new(&[
        ("lib/a.dart", A),
        ("lib/mid.dart", MID),
        ("lib/leaf.dart", LEAF),
    ]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/a.dart"), A.as_bytes());
    engine.set_content(&fixture.path("lib/mid.dart"), MID.as_bytes());
    engine.set_content(&fixture.path("lib/leaf.dart"), LEAF.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert_eq!(
        rows_of(&engine, &diagnostics, "part_of_different_library"),
        vec![(
            "mid.dart".to_owned(),
            23,
            11,
            "Expected this library to be part of 'my.lib', not 'my.lib'.".to_owned()
        )],
        "the depth-2 include row: {diagnostics:?}"
    );
}

/// A depth-2 URI-form chain attaches cleanly — `leaf` joins through `mid`,
/// no part-family row anywhere (the walk reaches each part's own `part`
/// directives).
#[test]
fn nested_uri_form_chain_attaches_silently() {
    const A: &str = "library my.lib;\npart 'mid.dart';\nint f() => 1;\n";
    const MID: &str = "part of 'a.dart';\npart 'leaf.dart';\n";
    const LEAF: &str = "part of 'mid.dart';\nint g() => 1;\n";

    let fixture = Fixture::new(&[
        ("lib/a.dart", A),
        ("lib/mid.dart", MID),
        ("lib/leaf.dart", LEAF),
    ]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/a.dart"), A.as_bytes());
    engine.set_content(&fixture.path("lib/mid.dart"), MID.as_bytes());
    engine.set_content(&fixture.path("lib/leaf.dart"), LEAF.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    let family: Vec<_> = diagnostics
        .iter()
        .filter(|row| row.code.name.starts_with("part_of"))
        .collect();
    assert!(
        family.is_empty(),
        "a clean depth-2 chain reports nothing: {diagnostics:?}"
    );
}

/// Two exports of different elements under one name report
/// `ambiguous_export` at the second directive's URI — `{0}` the name,
/// `{1}`/`{2}` the two declaring libraries' canonical URIs.
#[test]
fn ambiguous_export_reports_the_second_directive() {
    const A: &str = "export 'b.dart';\nexport 'c.dart';\n";
    const B: &str = "int Foo = 1;\n";
    const C: &str = "int Foo = 2;\n";

    let fixture = Fixture::new(&[("lib/a.dart", A), ("lib/b.dart", B), ("lib/c.dart", C)]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/a.dart"), A.as_bytes());
    engine.set_content(&fixture.path("lib/b.dart"), B.as_bytes());
    engine.set_content(&fixture.path("lib/c.dart"), C.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert_eq!(
        rows_of(&engine, &diagnostics, "ambiguous_export"),
        vec![(
            "a.dart".to_owned(),
            24,
            8,
            "The name 'Foo' is defined in the libraries 'package:fixture/lib/b.dart' \
             and 'package:fixture/lib/c.dart'."
                .to_owned()
        )],
        "the ambiguous export row: {diagnostics:?}"
    );
}

/// An internal SDK export from a non-`dart:` library reports
/// `export_internal_library` with the written URI string.
#[test]
fn export_internal_library_reports_the_written_uri() {
    const A: &str = "export 'dart:_internal';\n";

    let fixture = Fixture::new(&[
        ("lib/a.dart", A),
        (
            "sdk/lib/internal/internal.dart",
            "library dart._internal;\n",
        ),
    ]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/a.dart"), A.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert_eq!(
        rows_of(&engine, &diagnostics, "export_internal_library"),
        vec![(
            "a.dart".to_owned(),
            7,
            16,
            "The library 'dart:_internal' is internal and can't be exported.".to_owned()
        )],
        "the internal export row: {diagnostics:?}"
    );
}

/// A deferred+prefixed import whose namespace carries an extension reports
/// `deferred_import_of_extension` at the URI literal.
#[test]
fn deferred_import_of_extension_reports() {
    const A: &str = "import 'ext.dart' deferred as p;\nint f() => 1;\n";
    const EXT: &str = "extension E on int {\n  int get doubled => this * 2;\n}\n";

    let fixture = Fixture::new(&[("lib/a.dart", A), ("lib/ext.dart", EXT)]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/a.dart"), A.as_bytes());
    engine.set_content(&fixture.path("lib/ext.dart"), EXT.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert_eq!(
        rows_of(&engine, &diagnostics, "deferred_import_of_extension"),
        vec![(
            "a.dart".to_owned(),
            7,
            10,
            "Deferred library imports must hide all extension declarations.".to_owned()
        )],
        "the deferred import row: {diagnostics:?}"
    );
}

/// A/B: the same imports and parts on clean targets stay silent — a library
/// `import`, a library `part` target, and an eager extension import.
#[test]
fn clean_directives_stay_silent() {
    const A: &str = "library my.lib;\nimport 'ext.dart' as p;\npart 'p.dart';\nint f() => 1;\n";
    const EXT: &str = "extension E on int {\n  int get doubled => this * 2;\n}\n";
    const P: &str = "part of my.lib;\nint g() => 1;\n";

    let fixture = Fixture::new(&[("lib/a.dart", A), ("lib/ext.dart", EXT), ("lib/p.dart", P)]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/a.dart"), A.as_bytes());
    engine.set_content(&fixture.path("lib/ext.dart"), EXT.as_bytes());
    engine.set_content(&fixture.path("lib/p.dart"), P.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    for code in [
        "import_of_non_library",
        "export_of_non_library",
        "part_of_non_part",
        "part_of_different_library",
        "part_of_unnamed_library",
        "ambiguous_export",
        "export_internal_library",
        "deferred_import_of_extension",
    ] {
        assert!(
            rows_of(&engine, &diagnostics, code).is_empty(),
            "{code} must stay silent: {diagnostics:?}"
        );
    }
}
