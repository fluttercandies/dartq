//! Ticket #3352: the directive walk's recovery spans — a name-less combinator
//! clause ends at its synthetic identifier, and an unterminated conditional
//! group ends where the official parser relocates its synthetic `)`.
//!
//! `_reportForMultipleCombinators` reports `combinators.beginToken` through
//! `combinators.endToken`, and a `Combinator.endToken` is its last name's end
//! or, with an empty list, the synthetic identifier `ensureIdentifier`
//! splices at the offending follower (`ast.dart` `HideCombinatorImpl:17348` /
//! `ShowCombinatorImpl:29973`; `identifier_context_impl.dart:948-1000`). The
//! walk stopped at the follower without extending the span, so every
//! `export 'a.dart' hide A show <declaration>` recovery case reported the
//! first clause's names only.
//!
//! `parseConditionalUri` relocates an unterminated group's synthetic `)` to
//! right after the last consumed clause token (`rewriter.moveSynthetic`,
//! `parser_impl.dart:1130-1140`) and reads the URI there, so the directive
//! ends in front of a following declaration head and that head keeps its own
//! face. The walk jumped past the appended synthetic closer instead —
//! swallowing the second directive and its `uri_does_not_exist` row.
//!
//! Spans pinned against the oracle-3.13.7 shapes (`conformance/expected.tsv`,
//! `parser/src/fasta/recovery/partial_code/{export,import}_directive`).

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

/// A directive's observable face: kind, URI, clause count, and the written
/// combinator names (clause order, `hidden` flag second).
#[derive(Debug, PartialEq, Eq)]
struct Face {
    kind: DirectiveKind,
    uri: Option<String>,
    configurations: usize,
    written: Vec<(String, bool)>,
}

impl Run {
    /// `uri_does_not_exist` rows as `(offset, length)`, source order.
    fn uri_rows(&self) -> Vec<(u32, u32)> {
        self.rows_of("uri_does_not_exist")
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
                configurations: directive.configurations.len(),
                written: directive
                    .combinators
                    .written()
                    .map(|(name, hidden, _)| (strings.resolve(name).to_owned(), hidden))
                    .collect(),
            })
            .collect()
    }
}

/// `export 'a.dart' hide A show class A {}` — the second clause reads no name
/// (the `class` head starts the next construct), so its node is the synthetic
/// identifier at `class`; the warning runs `hide A show `, exactly the
/// oracle's `2:17|12`.
#[test]
fn a_name_less_combinator_clause_ends_at_its_follower() {
    const SOURCE: &str = "export 'a.dart' hide A show class A {}\n";
    let run = run("combinator-class", SOURCE);
    assert_eq!(run.uri_rows(), vec![(7, 8)], "the literal reports");
    assert_eq!(
        run.rows_of("multiple_combinators"),
        vec![(16, 12)],
        "the warning spans through the `show` clause's synthetic name"
    );
    assert_eq!(&SOURCE[16..28], "hide A show ");
    let faces = run.faces();
    assert_eq!(faces.len(), 1, "one export directive: {faces:?}");
    assert_eq!(faces[0].written, vec![("A".to_owned(), true)]);
}

/// `export 'a.dart' hide A show` — the empty second clause's follower is EOF;
/// the synthetic identifier sits at the end of the source, so the span still
/// runs to `2:17|12`.
#[test]
fn a_name_less_clause_before_eof_spans_to_the_end_of_source() {
    const SOURCE: &str = "export 'a.dart' hide A show\n";
    let run = run("combinator-eof", SOURCE);
    assert_eq!(run.uri_rows(), vec![(7, 8)], "the literal reports");
    assert_eq!(
        run.rows_of("multiple_combinators"),
        vec![(16, 12)],
        "the span ends at the EOF token the synthetic name anchors on"
    );
}

/// Negative: two named clauses keep the last name's end — `hide A show B`
/// spans 13 (`2:17|13` in the parser corpus), not the keyword or a follower.
#[test]
fn named_combinator_clauses_span_their_last_name() {
    const SOURCE: &str = "export 'a.dart' hide A show B;\n";
    let run = run("combinator-names", SOURCE);
    assert_eq!(run.uri_rows(), vec![(7, 8)], "the literal reports");
    assert_eq!(
        run.rows_of("multiple_combinators"),
        vec![(16, 13)],
        "the warning ends at the last written name"
    );
    assert_eq!(&SOURCE[16..29], "hide A show B");
}

/// `import 'a.dart' if (b import 'b.dart';` — the unterminated group's
/// synthetic `)` is relocated after the condition name, so the clause ends at
/// the second `import` and that directive keeps its own face (the oracle's
/// `2:30|8` row).
#[test]
fn an_unterminated_conditional_group_hands_a_trailing_head_back() {
    const SOURCE: &str = "import 'a.dart' if (b import 'b.dart';\n";
    let run = run("if-id-head", SOURCE);
    assert_eq!(
        run.uri_rows(),
        vec![(7, 8), (29, 8)],
        "both literals report, not just the first"
    );
    let faces = run.faces();
    assert_eq!(faces.len(), 2, "two imports: {faces:?}");
    assert_eq!(
        faces[0].configurations, 0,
        "the broken clause contributes no configuration"
    );
    assert_eq!(faces[1].kind, DirectiveKind::Import);
    assert_eq!(faces[1].uri.as_deref(), Some("b.dart"));
}

/// `import 'a.dart' if (b == import 'b.dart';` — the same relocation after a
/// failed `==` value read: the directive ends at the second `import`
/// (the oracle's `2:33|8` row).
#[test]
fn an_unterminated_conditional_value_clause_hands_a_trailing_head_back() {
    const SOURCE: &str = "import 'a.dart' if (b == import 'b.dart';\n";
    let run = run("if-eq-head", SOURCE);
    assert_eq!(
        run.uri_rows(),
        vec![(7, 8), (32, 8)],
        "both literals report"
    );
    let faces = run.faces();
    assert_eq!(faces.len(), 2, "two imports: {faces:?}");
    assert_eq!(faces[0].configurations, 0);
    assert_eq!(faces[1].uri.as_deref(), Some("b.dart"));
}

/// Silent negative: a well-formed clause still records its selection slot and
/// the directive does not fork — the primary URI is the selected one under
/// the empty declared-variable map, so one literal reports.
#[test]
fn a_closed_conditional_clause_records_its_configuration() {
    const SOURCE: &str = "import 'a.dart' if (b) 'c.dart';\n";
    let run = run("if-clean", SOURCE);
    assert_eq!(run.uri_rows(), vec![(7, 8)], "the selected primary reports");
    let faces = run.faces();
    assert_eq!(faces.len(), 1, "one import directive: {faces:?}");
    assert_eq!(faces[0].configurations, 1);
    assert_eq!(faces[0].uri.as_deref(), Some("a.dart"));
}
