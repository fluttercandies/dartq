//! Ticket #3010: the `uri_with_interpolation` faces over configurable
//! directive URIs — a primary literal carrying `${…}`/`$id` reports at its
//! whole-literal region on `import`/`export`/`part`, an interpolated
//! `part of` URI stays silent (the oracle only classifies with it), and an
//! interpolated conditional-clause URI still occupies its selection slot:
//! first-match-wins picks it like any other clause
//! (`file_state.dart:738-764`, the clause recorded as
//! `DirectiveUriWithoutString` :118), so selecting it surfaces
//! `uri_with_interpolation` at the branch literal — never a resolution.
//!
//! Upstream mirrors: `_buildConfigurableDirectiveUris`
//! (file_state.dart:738-764) records every clause's URI node; the
//! interpolated literal's `stringValue` is null (:759-764), selection is
//! the same first-match pass (`library_analyzer.dart:698-717` reports
//! `state.selectedUri`), and `uriWithInterpolation.at(directive.uri)`
//! anchors the URI node's beginToken..endToken (:732/:993/:1031).

mod common;

use common::Fixture;
use dartq_engine::CancellationToken;

/// The rows of one code over the whole analyze run, as (file tail, offset,
/// length, message) — the same carrier `ticket_2998.rs` uses.
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

/// `import 'a${b}.dart';` — the `${…}` form on the primary URI reports
/// `uri_with_interpolation` at the whole literal (`'a${b}.dart'` = offset
/// 7, length 12), and the directive never resolves (no target, no other
/// URI code).
#[test]
fn import_with_an_interpolated_primary_uri_reports_interpolation() {
    const A: &str = "import 'a${b}.dart';\nint f() => 1;\n";

    let fixture = Fixture::new(&[("lib/a.dart", A)]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/a.dart"), A.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert_eq!(
        rows_of(&engine, &diagnostics, "uri_with_interpolation"),
        vec![(
            "a.dart".to_owned(),
            7,
            12,
            "URIs can't use string interpolation.".to_owned()
        )],
        "the interpolated primary import: {diagnostics:?}"
    );
    for code in ["uri_does_not_exist", "invalid_uri", "import_of_non_library"] {
        assert!(
            rows_of(&engine, &diagnostics, code).is_empty(),
            "{code} must stay silent — an interpolated URI never resolves: {diagnostics:?}"
        );
    }
}

/// `export "a$b.dart";` — the identifier form on an export reports at the
/// whole literal the same way (`"a$b.dart"` = offset 7, length 10).
#[test]
fn export_with_an_interpolated_primary_uri_reports_interpolation() {
    const A: &str = r#"export "a$b.dart";"#;

    let fixture = Fixture::new(&[("lib/a.dart", A)]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/a.dart"), A.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert_eq!(
        rows_of(&engine, &diagnostics, "uri_with_interpolation"),
        vec![(
            "a.dart".to_owned(),
            7,
            10,
            "URIs can't use string interpolation.".to_owned()
        )],
        "the interpolated primary export: {diagnostics:?}"
    );
}

/// `part 'p${x}.dart';` — an interpolated include URI still reports
/// `uri_with_interpolation` (the `part` directive resolves through the
/// `ImportExport` face like its siblings), at the whole literal.
#[test]
fn part_with_an_interpolated_uri_reports_interpolation() {
    const A: &str = "part 'p${x}.dart';\nint f() => 1;\n";

    let fixture = Fixture::new(&[("lib/a.dart", A)]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/a.dart"), A.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert_eq!(
        rows_of(&engine, &diagnostics, "uri_with_interpolation"),
        vec![(
            "a.dart".to_owned(),
            5,
            12,
            "URIs can't use string interpolation.".to_owned()
        )],
        "the interpolated part include: {diagnostics:?}"
    );
}

/// `part of 'u${x}.dart';` — an interpolated part-of URI reports NOTHING:
/// the oracle only uses the part-of URI to classify the file
/// (`PartOfUriUnknownFileKind`, file_state.dart:938-953 — a stringless URI
/// resolves to nothing and never attaches), and its diagnostic faces force
/// `code: None`. The file simply stays unclaimed.
#[test]
fn part_of_with_an_interpolated_uri_stays_silent() {
    const P: &str = "part of 'u${x}.dart';\nint g() => 1;\n";

    let fixture = Fixture::new(&[("lib/p.dart", P)]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/p.dart"), P.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    for code in [
        "uri_with_interpolation",
        "uri_does_not_exist",
        "invalid_uri",
        "part_of_non_part",
        "part_of_different_library",
        "part_of_unnamed_library",
    ] {
        assert!(
            rows_of(&engine, &diagnostics, code).is_empty(),
            "{code} must stay silent — the PartOf face forces no code: {diagnostics:?}"
        );
    }
}

/// `import 'dart:core' if (dart.library.io) 'b${y}.dart';` with
/// `dart.library.io` declared: the interpolated clause occupies its
/// selection slot (a `DirectiveUriWithoutString` still participates —
/// file_state.dart:738-764), first-match-wins picks it, and the selected
/// branch reports `uri_with_interpolation` anchored on the PRIMARY
/// literal (`'dart:core'` = offset 7, length 11): upstream reports every
/// import URI row `.at(directive.uri)` — the directive's declared primary
/// node (library_analyzer.dart:736-745); `_resolveUriConfigurations`
/// (:1086-1095) never reassigns it — so the selected string governs the
/// message, the primary literal the span. No resolution ever runs.
#[test]
fn a_selected_interpolated_configuration_reports_at_the_branch_literal() {
    const A: &str = "import 'dart:core' if (dart.library.io) 'b${y}.dart';\nint f() => 1;\n";

    let fixture = Fixture::new(&[("lib/a.dart", A)]);
    let mut declared = rustc_hash::FxHashMap::default();
    declared.insert("dart.library.io".to_owned(), "true".to_owned());
    let mut engine = dartq_engine::Engine::new(dartq_engine::EngineConfig {
        store: dartq_engine::memory_store(),
        sdk: dartq_sdk::SdkSnapshot::load(&fixture.root.join("sdk"), None)
            .expect("synthetic SDK loads"),
        packages: fixture.packages.clone(),
        pub_config: std::sync::Arc::clone(&fixture.config),
        config_dir: fixture.config_dir.clone(),
        context_roots: Vec::new(),
        semdiag: Some(dartq_engine::semdiag::diagnoser()),
        target: dartq_engine::DEFAULT_TARGET.to_owned(),
        declared_variables: declared,
        experiments: Vec::new(),
    })
    .expect("engine");
    engine.set_content(&fixture.path("lib/a.dart"), A.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert_eq!(
        rows_of(&engine, &diagnostics, "uri_with_interpolation"),
        vec![(
            "a.dart".to_owned(),
            7,
            11,
            "URIs can't use string interpolation.".to_owned()
        )],
        "the selected interpolated branch anchors on the primary literal: {diagnostics:?}"
    );
    for code in ["uri_does_not_exist", "import_of_non_library"] {
        assert!(
            rows_of(&engine, &diagnostics, code).is_empty(),
            "{code} must stay silent — the selected branch never resolves: {diagnostics:?}"
        );
    }
}

/// The A/B twin: the same conditional URI with the declared map EMPTY
/// falls back to the primary `dart:core` — the interpolated branch is
/// never read, so nothing reports (unmatched clauses are never resolved,
/// library_analyzer.dart:698-717).
#[test]
fn an_unselected_interpolated_configuration_stays_silent() {
    const A: &str = "import 'dart:core' if (dart.library.io) 'b${y}.dart';\nint f() => 1;\n";

    let fixture = Fixture::new(&[("lib/a.dart", A)]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/a.dart"), A.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    for code in [
        "uri_with_interpolation",
        "uri_does_not_exist",
        "import_of_non_library",
    ] {
        assert!(
            rows_of(&engine, &diagnostics, code).is_empty(),
            "{code} must stay silent — the unmatched clause is never read: {diagnostics:?}"
        );
    }
}

/// A/B for the conditional primary arm: `import 'a${b}.dart' if
/// (dart.library.io) 'real.dart';` — the interpolated PRIMARY still
/// reports when no clause matches (the `primary_uri: Option<&str>` arm of
/// `resolve_conditional`), at the primary literal.
#[test]
fn an_interpolated_primary_still_reports_when_no_clause_matches() {
    const A: &str = "import 'a${b}.dart' if (dart.library.io) 'real.dart';\nint f() => 1;\n";
    const REAL: &str = "int real() => 0;\n";

    let fixture = Fixture::new(&[("lib/a.dart", A), ("lib/real.dart", REAL)]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/a.dart"), A.as_bytes());
    engine.set_content(&fixture.path("lib/real.dart"), REAL.as_bytes());

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    assert_eq!(
        rows_of(&engine, &diagnostics, "uri_with_interpolation"),
        vec![(
            "a.dart".to_owned(),
            7,
            12,
            "URIs can't use string interpolation.".to_owned()
        )],
        "the unmatched selection falls back to the interpolated primary: {diagnostics:?}"
    );
}

/// `import 'p.dart'` of a workspace part file reports
/// `import_of_non_library` at the URI literal with the written spelling —
/// the `UnitFacts::is_part` role (`_updateKind`,
/// file_state.dart:928-949) decides the non-library face, and the part's
/// own `part of` attach is irrelevant (the emitter reads residency, not
/// membership).
#[test]
fn import_of_a_workspace_part_reports_non_library() {
    const A: &str = "import 'p.dart';\nint f() => 1;\n";
    const P: &str = "part of 'lib.dart';\nint g() => 1;\n";

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
            7,
            8,
            "The imported library 'p.dart' can't have a part-of directive.".to_owned()
        )],
        "the workspace part import: {diagnostics:?}"
    );
}

/// The export twin on the same workspace part target.
#[test]
fn export_of_a_workspace_part_reports_non_library() {
    const A: &str = "export 'p.dart';\n";
    const P: &str = "part of 'lib.dart';\nint g() => 1;\n";

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
        "the workspace part export: {diagnostics:?}"
    );
}
