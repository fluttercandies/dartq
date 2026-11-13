//! Ticket #3332: the directive walker's name gate — a top-level-declaration
//! head in a name position is a name only when its follower keeps it there.
//!
//! Upstream `ensureIdentifier` splices a synthetic name in front of a
//! declaration head whose follower leaves the slot's keep-set
//! (`identifier_context_impl.dart:130-155`, `:948-1000`, `:698-720`) and
//! leaves the head for the next top-level parse. The diet walk used to take
//! any built-in identifier as a name unconditionally, so `import`/`export`/
//! `part`/`library` heads were eaten and the following directive never
//! became a fact — no `uri_does_not_exist` row for its literal, and a
//! `multiple_combinators` span running into it. These pins hold each of the
//! four gated sites on the oracle-3.13.7 shape plus the silent negatives
//! (a head the keep-set DOES hold stays a name).

mod common;

use common::Fixture;
use dartq_engine::{CancellationToken, DirectiveKind};
use dartq_source::FileId;

/// One analyzed single-file unit: the fixture (its path universe outlives
/// the engine's reads), the engine, the unit's id, and the run's rows.
struct Run {
    _fixture: Fixture,
    engine: dartq_engine::Engine,
    file: FileId,
    diagnostics: Vec<dartq_diagnostics::Diagnostic>,
}

fn run(tag: &str, source: &str) -> Run {
    let relative = format!("lib/{tag}.dart");
    let fixture = Fixture::new(&[(relative.as_str(), source)]);
    let mut engine = fixture.engine();
    let file = engine.set_content(&fixture.path(&relative), source.as_bytes());
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold");
    Run {
        _fixture: fixture,
        engine,
        file,
        diagnostics,
    }
}

/// A directive's observable face: kind, URI, dotted name, and the written
/// combinator names (clause order, `hidden` flag second).
#[derive(Debug, PartialEq, Eq)]
struct Face {
    kind: DirectiveKind,
    uri: Option<String>,
    name: Option<String>,
    written: Vec<(String, bool)>,
}

impl Run {
    /// `uri_does_not_exist` rows as `(offset, length)`, source order.
    fn uri_rows(&self) -> Vec<(u32, u32)> {
        let mut rows: Vec<(u32, u32)> = self
            .diagnostics
            .iter()
            .filter(|row| row.code.name == "uri_does_not_exist")
            .map(|row| (row.range.offset, row.range.length))
            .collect();
        rows.sort_unstable();
        rows
    }

    /// Every row of `code` as `(offset, length)`, source order.
    fn rows_of(&self, code: &str) -> Vec<(u32, u32)> {
        let mut rows: Vec<(u32, u32)> = self
            .diagnostics
            .iter()
            .filter(|row| row.code.name == code)
            .map(|row| (row.range.offset, row.range.length))
            .collect();
        rows.sort_unstable();
        rows
    }

    /// The unit's directive faces, source order.
    fn faces(&self) -> Vec<Face> {
        let facts = self.engine.file_facts(self.file).expect("unit facts");
        let strings = self.engine.strings();
        facts
            .directives
            .iter()
            .map(|directive| Face {
                kind: directive.kind,
                uri: directive.uri.as_deref().map(str::to_owned),
                name: directive.name.as_deref().map(str::to_owned),
                written: directive
                    .combinators
                    .written()
                    .map(|(name, hidden, _)| (strings.resolve(name).to_owned(), hidden))
                    .collect(),
            })
            .collect()
    }
}

/// `library import 'a.dart';` — the `library` name slot refuses the `import`
/// head (follower is a string literal), so the import keeps its own face and
/// its literal reports. Offset 15 = line 1 column 16, the oracle's.
#[test]
fn a_library_name_slot_refuses_an_import_head() {
    const SOURCE: &str = "library import 'a.dart';\n";
    let run = run("library-head", SOURCE);
    assert_eq!(
        run.uri_rows(),
        vec![(15, 8)],
        "the import's literal reports"
    );
    assert_eq!(
        run.faces(),
        vec![
            Face {
                kind: DirectiveKind::Library,
                uri: None,
                name: Some(String::new()),
                written: Vec::new(),
            },
            Face {
                kind: DirectiveKind::Import,
                uri: Some("a.dart".to_owned()),
                name: None,
                written: Vec::new(),
            },
        ],
        "a synthetic (empty) library name, then the import's own face"
    );
}

/// `library lib. import 'a.dart';` — the `lib.` continuation slot applies the
/// same keep-set, so the import after the dot survives (offset 20 = column
/// 21, the oracle's).
#[test]
fn a_library_name_continuation_refuses_an_import_head() {
    const SOURCE: &str = "library lib. import 'a.dart';\n";
    let run = run("library-continuation", SOURCE);
    assert_eq!(
        run.uri_rows(),
        vec![(20, 8)],
        "the import's literal reports"
    );
    assert_eq!(
        run.faces().iter().map(|face| face.kind).collect::<Vec<_>>(),
        vec![DirectiveKind::Library, DirectiveKind::Import],
        "the qualified name ends at the dot; the import is its own directive"
    );
}

/// `import 'a.dart' show import 'b.dart';` — the show list refuses the
/// `import` head (follower is a string literal), so the second import keeps
/// its face and both literals report.
#[test]
fn a_show_list_refuses_an_import_head() {
    const SOURCE: &str = "import 'a.dart' show import 'b.dart';\n";
    let run = run("show-head", SOURCE);
    assert_eq!(
        run.uri_rows(),
        vec![(7, 8), (28, 8)],
        "both literals report, not just the first"
    );
    let faces = run.faces();
    assert_eq!(faces.len(), 2, "two directives: {faces:?}");
    assert_eq!(
        faces[0].written,
        Vec::new(),
        "the show clause keeps no name"
    );
    assert_eq!(faces[1].uri.as_deref(), Some("b.dart"));
}

/// `import 'a.dart' as import 'b.dart';` — the `as` prefix slot refuses the
/// head (follower is a string literal), the directive ends prefix-less at
/// `as`, and the second import parses.
#[test]
fn an_as_prefix_slot_refuses_an_import_head() {
    const SOURCE: &str = "import 'a.dart' as import 'b.dart';\n";
    let run = run("as-head", SOURCE);
    assert_eq!(
        run.uri_rows(),
        vec![(7, 8), (26, 8)],
        "both literals report"
    );
    let faces = run.faces();
    assert_eq!(faces.len(), 2, "two directives: {faces:?}");
    assert_eq!(faces[1].kind, DirectiveKind::Import);
    assert_eq!(faces[1].uri.as_deref(), Some("b.dart"));
}

/// `import 'a.dart' if ( import 'b.dart';` — the dotted-name slot refuses the
/// head (follower is a string literal), the malformed clause contributes no
/// configuration, and the second import parses.
#[test]
fn a_conditional_uri_name_slot_refuses_an_import_head() {
    const SOURCE: &str = "import 'a.dart' if ( import 'b.dart';\n";
    let run = run("if-head", SOURCE);
    assert_eq!(
        run.uri_rows(),
        vec![(7, 8), (28, 8)],
        "both literals report"
    );
    let faces = run.faces();
    assert_eq!(faces.len(), 2, "two directives: {faces:?}");
    assert_eq!(faces[1].kind, DirectiveKind::Import);
    assert_eq!(faces[1].uri.as_deref(), Some("b.dart"));
}

/// `export 'a.dart' hide A show export 'a.dart';` — the trailing `export`
/// head ends the first directive after `show`, so `a.dart` reports twice and
/// `multiple_combinators` spans the whole combinator list: the `show` clause
/// reads no name (the trailing head is refused in the name slot), so its
/// `endToken` is the synthetic identifier spliced at the follower — the span
/// runs through the follower's offset and never covers the trailing literal.
#[test]
fn an_export_combinator_list_hands_the_trailing_head_back() {
    const SOURCE: &str = "export 'a.dart' hide A show export 'a.dart';\n";
    let run = run("export-head", SOURCE);
    assert_eq!(
        run.uri_rows(),
        vec![(7, 8), (35, 8)],
        "the first and the trailing literal both report"
    );
    let faces = run.faces();
    assert_eq!(faces.len(), 2, "two exports: {faces:?}");
    assert_eq!(faces[0].written, vec![("A".to_owned(), true)]);
    assert_eq!(faces[1].kind, DirectiveKind::Export);
    assert_eq!(faces[1].written, Vec::new());
    assert_eq!(
        run.rows_of("multiple_combinators"),
        vec![(16, 12)],
        "the warning spans `hide A show ` through the synthetic name at the \
         trailing head, never the literal itself"
    );
    assert_eq!(&SOURCE[16..28], "hide A show ");
}

/// Silent negative: `show export;` — the follower `;` is in the combinator
/// keep-set, so `export` IS the shown name and no second directive appears.
#[test]
fn a_shown_export_before_a_semicolon_stays_a_name() {
    const SOURCE: &str = "import 'a.dart' show export;\n";
    let run = run("show-export", SOURCE);
    assert_eq!(run.uri_rows(), vec![(7, 8)], "one literal, one directive");
    let faces = run.faces();
    assert_eq!(
        faces.len(),
        1,
        "the `export` is a name, not a head: {faces:?}"
    );
    assert_eq!(faces[0].written, vec![("export".to_owned(), false)]);
}

/// Silent negative: `library library;` — the follower `;` is in the qualified
/// name keep-set, so the library is named `library`.
#[test]
fn a_library_named_library_before_a_semicolon_stays_a_name() {
    const SOURCE: &str = "library library;\n";
    let run = run("library-library", SOURCE);
    assert_eq!(run.uri_rows(), Vec::new(), "no URI rows at all");
    let faces = run.faces();
    assert_eq!(faces.len(), 1, "one library directive: {faces:?}");
    assert_eq!(faces[0].kind, DirectiveKind::Library);
    assert_eq!(faces[0].name.as_deref(), Some("library"));
}

/// The dup-export report order: two libraries exporting the same name report
/// `ambiguous_export` at the repeating export's URI with `{1}` = the
/// **earlier** export's declaring library and `{2}` = the repeat's — the
/// oracle's `firstUri: prevElement.library!.uri` / `secondUri:
/// element.library!.uri` argument order (`error_verifier.dart:2593-2621`
/// `_checkForAmbiguousExport`). The pair is no `duplicate_export`: that
/// family groups same-target directives only.
#[test]
fn an_ambiguous_export_names_the_earlier_library_first() {
    const SOURCE: &str = "export 'a.dart' show X;\nexport 'b.dart' show X;\n";
    let fixture = Fixture::new(&[
        ("lib/main.dart", SOURCE),
        ("lib/a.dart", "class X {}\n"),
        ("lib/b.dart", "class X {}\n"),
    ]);
    let mut engine = fixture.engine();
    engine.discover_workspace().expect("discover");
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("analyze");
    assert!(
        diagnostics
            .iter()
            .all(|row| row.code.name != "duplicate_export"),
        "distinct targets are no duplicate: {diagnostics:?}"
    );
    let rows: Vec<_> = diagnostics
        .iter()
        .filter(|row| row.code.name == "ambiguous_export")
        .collect();
    assert_eq!(rows.len(), 1, "one ambiguity at the repeat: {diagnostics:?}");
    let row = rows[0];
    assert_eq!(
        (row.range.offset, row.range.length),
        (31, 8),
        "the repeat's own literal anchors the row: {diagnostics:?}"
    );
    // `{1}` leads with the earlier export's library, `{2}` trails with the
    // repeat's — the byte-level order the oracle prints.
    let body = row
        .message
        .strip_prefix("The name 'X' is defined in the libraries '")
        .unwrap_or_else(|| panic!("oracle message shape: {:?}", row.message));
    let (first, second) = body
        .split_once("' and '")
        .unwrap_or_else(|| panic!("two libraries in the message: {:?}", row.message));
    let second = second.strip_suffix("'.").unwrap_or(second);
    assert!(
        first.ends_with("lib/a.dart"),
        "first is the earlier export's library: {:?}", row.message
    );
    assert!(
        second.ends_with("lib/b.dart"),
        "second is the repeat's library: {:?}", row.message
    );
}
