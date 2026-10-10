//! Directive grammar tests (#300): library/library-augmentation, import
//! (conditional URIs, prefix, combinators), export, part, part-of, the
//! `DirectiveContext` ordering checks, and the out-of-order-clause import
//! recovery (official `parseImportRecovery`, `parser_impl.dart:995`).

use super::collector::{Collector, latest_features, scan_ok, setup};
use crate::listener::{ParserListener, Token};
use crate::message::Message;
use crate::parser::Parser;
use crate::token_id::TokenSpan;

fn parse(source: &'static str) -> Collector {
    let mut p = setup(source);
    p.parse_unit();
    p.listener
}

fn error_codes(source: &'static str) -> Vec<String> {
    parse(source).error_codes()
}

// —— valid directive forms ———————————————————————————————————————————————

#[test]
fn import_plain() {
    let c = parse("import 'a.dart';");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("begin", "import"), ("end", "import")]);
}

#[test]
fn import_full_clause_set() {
    let c = parse("import 'a.dart' deferred as p show a, b hide c;");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "import"),
        ("handle", "import_prefix"),
        ("begin", "combinators"),
        ("begin", "show"),
        ("handle", "identifier_list"),
        ("end", "show"),
        ("begin", "hide"),
        ("handle", "identifier_list"),
        ("end", "hide"),
        ("end", "combinators"),
        ("end", "import"),
    ]);
}

#[test]
fn import_conditional_uris() {
    let c = parse("import 'a.dart' if (dart.library.io) 'io.dart' if (x == 'y') 'z.dart';");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "import"),
        ("begin", "conditional_uris"),
        ("begin", "conditional_uri"),
        ("handle", "dotted_name"),
        ("end", "conditional_uri"),
        ("begin", "conditional_uri"),
        ("handle", "dotted_name"),
        ("end", "conditional_uri"),
        ("end", "conditional_uris"),
        ("end", "import"),
    ]);
}

/// Captures the conditional-clause payloads in source order (#512): every
/// clause's dotted condition name and its string literals — the candidate
/// URI last, an earlier entry the `==` value. ASCII sources only (UTF-16
/// offsets slice as bytes).
#[derive(Default)]
struct ConditionalPayloads {
    text: &'static str,
    errors: Vec<String>,
    /// `(dotted name, string bodies)` per `if` clause, in source order.
    clauses: Vec<(String, Vec<String>)>,
    /// Begin offset of the literal string currently open.
    pending_string: Option<usize>,
}

impl ConditionalPayloads {
    /// Parses `source` and drains the capture.
    fn of(source: &'static str) -> Self {
        let (table, text) = scan_ok(source);
        let mut parser = Parser::new(
            text,
            table,
            latest_features(),
            Self {
                text,
                ..Default::default()
            },
        );
        parser.parse_unit();
        parser.listener
    }
}

impl ParserListener for ConditionalPayloads {
    fn handle_recoverable_error(&mut self, message: Message, _: TokenSpan, _: TokenSpan) {
        self.errors.push(message.code.name.to_owned());
    }

    fn begin_conditional_uri(&mut self, _: Token) {
        self.clauses.push((String::new(), Vec::new()));
    }

    fn handle_dotted_name(&mut self, _: usize, first: Token) {
        // The dotted name is the maximal identifier/period run at the first
        // identifier's offset — the parser consumed it contiguously.
        let start = first.range.offset as usize;
        let end = self.text[start..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$' || c == '.'))
            .map_or(self.text.len(), |relative| start + relative);
        if let Some(clause) = self.clauses.last_mut() {
            clause.0 = self.text[start..end].to_owned();
        }
    }

    fn begin_literal_string(&mut self, begin: Token) {
        self.pending_string = Some(begin.range.offset as usize);
    }

    fn end_literal_string(&mut self, _: usize, end: Token) {
        let start = self.pending_string.take().expect("string begin seen");
        let stop = end.range.offset as usize + end.range.length as usize;
        // Plain single-segment literals in these sources: strip the quotes.
        let body = self.text[start..stop].trim_matches(|c| c == '\'' || c == '"');
        if let Some(clause) = self.clauses.last_mut() {
            clause.1.push(body.to_owned());
        }
    }
}

/// The clause payloads ride the events in source order: the dotted variable
/// verbatim (bare and `==` forms), the candidate URI last per clause — the
/// shape the resolution fork consumes (#512). The directive's own primary
/// URI fires before any clause opens and is not captured.
#[test]
fn conditional_uri_payloads_in_source_order() {
    let listener = ConditionalPayloads::of(
        "import 'a.dart' if (dart.library.io) 'io.dart' \
         if (mode == 'release') 'release.dart' if (p.q) 'pq.dart';",
    );
    assert!(listener.errors.is_empty(), "{:?}", listener.errors);
    assert_eq!(
        listener.clauses,
        vec![
            ("dart.library.io".to_owned(), vec!["io.dart".to_owned()]),
            (
                "mode".to_owned(),
                vec!["release".to_owned(), "release.dart".to_owned()]
            ),
            ("p.q".to_owned(), vec!["pq.dart".to_owned()]),
        ]
    );
}

/// Missing candidate URI (official `ensureLiteralString` :5075): `x` is a
/// legal dotted-name condition, so the only failure is the missing
/// candidate — expectedString on the `;`, which survives (the synthetic
/// string is spliced before it) and terminates the import.
#[test]
fn conditional_uri_payloads_missing_candidate_uri() {
    let listener = ConditionalPayloads::of("import 'a.dart' if (x);");
    assert_eq!(listener.errors, ["expected_string_literal"]);
    assert_eq!(listener.clauses, vec![("x".to_owned(), vec![String::new()])]);
}

/// Garbage between the condition and the closer (official :1124-1136): the
/// intervening token is reported, the real `)` is jumped to, and the
/// candidate URI still binds to the clause.
#[test]
fn conditional_uri_payloads_garbage_before_close() {
    let listener = ConditionalPayloads::of("import 'a.dart' if (x y) 'b.dart';");
    assert_eq!(listener.errors, ["unexpected_token"]);
    assert_eq!(
        listener.clauses,
        vec![("x".to_owned(), vec!["b.dart".to_owned()])]
    );
}

#[test]
fn export_with_combinators() {
    let c = parse("export 'a.dart' show a hide b;");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "export"),
        ("begin", "combinators"),
        ("begin", "show"),
        ("end", "show"),
        ("begin", "hide"),
        ("end", "hide"),
        ("end", "combinators"),
        ("end", "export"),
    ]);
}

#[test]
fn part_uri() {
    let c = parse("part 'a.dart';");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("begin", "part"), ("end", "part")]);
}

#[test]
fn part_of_uri_form() {
    let c = parse("part of 'a.dart';");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("begin", "part_of"), ("end", "part_of")]);
}

#[test]
fn part_of_name_form() {
    let c = parse("part of foo.bar;");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("begin", "part_of"), ("end", "part_of")]);
}

#[test]
fn library_named_and_unnamed() {
    let c = parse("library foo.bar;");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("begin", "library_name"), ("end", "library_name")]);

    let c = parse("library;");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("begin", "library_name"), ("end", "library_name")]);
}

#[test]
fn library_augmentation() {
    let c = parse("augment library 'a.dart';");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "library_augmentation"),
        ("end", "library_augmentation"),
    ]);
}

#[test]
fn full_directive_prelude_before_declarations() {
    let c = parse("library l; import 'a.dart'; export 'b.dart'; part 'c.dart'; class C {}");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "library_name"),
        ("end", "library_name"),
        ("begin", "import"),
        ("end", "import"),
        ("begin", "export"),
        ("end", "export"),
        ("begin", "part"),
        ("end", "part"),
        ("begin", "class_declaration"),
        ("end", "class_declaration"),
    ]);
}

#[test]
fn metadata_on_directive() {
    let c = parse("@foo import 'a.dart';");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("begin", "import"), ("end", "import")]);
}

#[test]
fn script_tag_then_library() {
    let c = parse("#!/usr/bin/env dart\nlibrary l;");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("begin", "library_name"), ("end", "library_name")]);
}

// —— DirectiveContext ordering checks —————————————————————————————————————

#[test]
fn import_after_part() {
    assert_eq!(
        error_codes("part 'a.dart'; import 'b.dart';"),
        ["import_directive_after_part_directive"]
    );
}

#[test]
fn export_after_part() {
    assert_eq!(
        error_codes("part 'a.dart'; export 'b.dart';"),
        ["export_directive_after_part_directive"]
    );
}

#[test]
fn directive_after_declaration() {
    assert_eq!(
        error_codes("class C {} import 'a.dart';"),
        ["directive_after_declaration"]
    );
    assert_eq!(
        error_codes("int x = 1; part 'a.dart';"),
        ["directive_after_declaration"]
    );
}

#[test]
fn multiple_library_directives() {
    assert_eq!(
        error_codes("library a; library b;"),
        ["multiple_library_directives"]
    );
}

#[test]
fn library_directive_not_first() {
    assert_eq!(
        error_codes("import 'a.dart'; library b;"),
        ["library_directive_not_first"]
    );
}

#[test]
fn part_of_twice() {
    assert_eq!(
        error_codes("part of 'a.dart'; part of 'b.dart';"),
        ["multiple_part_of_directives"]
    );
}

#[test]
fn non_part_of_directive_in_part() {
    // enhanced-parts is off by default: a directive after `part of` is the
    // official `nonPartOfDirectiveInPart` report.
    assert_eq!(
        error_codes("part of 'a.dart'; import 'b.dart';"),
        ["non_part_of_directive_in_part"]
    );
}

#[test]
fn directive_after_part_of_with_enhanced_parts() {
    // With the experiment on, the directive is admitted
    // (directive_context.dart:63-67).
    use crate::ParserFeatures;
    let (table, text) = scan_ok("part of 'a.dart'; import 'b.dart';");
    let features = ParserFeatures::from_feature_set(
        dartq_language::FeatureSet::for_language_version(dartq_language::LanguageVersion::new(
            3, 15,
        ))
        .with_experiment(dartq_language::Feature::EnhancedParts),
    );
    let mut p = Parser::new(text, table, features, Collector::default());
    p.parse_unit();
    assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
}

// —— import clause recovery (parseImportRecovery) —————————————————————————

#[test]
fn import_out_of_order_clause_recovery() {
    // The second `import` is not a clause: the primary parse closes with
    // `endImport(_, null)`, one recovery pass inserts the `;`, and the
    // second import parses as its own directive.
    let c = parse("import 'a.dart' import 'b.dart';");
    assert_eq!(c.error_codes(), ["expected_token"]);
    c.assert_sequence(&[
        ("begin", "import"),
        ("end", "import"),
        ("handle", "recover_import"),
        ("begin", "import"),
        ("end", "import"),
    ]);
}

#[test]
fn duplicate_prefix() {
    assert_eq!(
        error_codes("import 'a.dart' as p as q;"),
        ["duplicate_prefix"]
    );
}

#[test]
fn prefix_after_combinator() {
    assert_eq!(
        error_codes("import 'a.dart' show a as p;"),
        ["prefix_after_combinator"]
    );
}

#[test]
fn duplicate_deferred_and_prefix() {
    assert_eq!(
        error_codes("import 'a.dart' deferred as p deferred as q;"),
        ["duplicate_deferred", "duplicate_prefix"]
    );
}

#[test]
fn deferred_without_as_is_silent() {
    // The loop's `deferred`-without-`as` special case emits the prefix
    // event and records nothing (official :1046-1051): no diagnostic.
    let c = parse("import 'a.dart' deferred;");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("handle", "import_prefix"),
        ("handle", "import_prefix"),
        ("handle", "recover_import"),
    ]);
}

#[test]
fn unexpected_token_before_clause_is_skipped() {
    // `skipUnexpectedTokenOpt` (:5148): a stray identifier followed by a
    // clause keyword is reported and skipped.
    assert_eq!(
        error_codes("import 'a.dart' foo as p;"),
        ["unexpected_token"]
    );
}

#[test]
fn import_at_eof_recovers_semicolon() {
    assert_eq!(error_codes("import 'a.dart'"), ["expected_token"]);
}

// —— clause-level recovery ————————————————————————————————————————————————

#[test]
fn uri_must_be_string() {
    // The string recovery reports ONLY expectedString (official
    // `ensureLiteralString` :5075; oracle: no `unexpectedToken` from the
    // read). The synthetic string is spliced BEFORE the offender
    // (`rewriteAndRecover` → `insertToken`, token_stream_rewriter
    // .dart:85), so the offender survives to the next grammar position and
    // takes its own report: the import recovery boundary
    // (`skipUnexpectedTokenOpt` :5148 via `parseImportRecovery`
    // :1022-1025) re-reports `5` before the `;`; `part`'s
    // `ensureSemicolon` reports the missing `;` anchored at `part`
    // (`findPreviousNonZeroLengthToken`), and the top-level continuation
    // then takes `5;` — `expected_declaration` (the shared
    // `EXPECTED_EXECUTABLE` code) on `5`, `unexpectedToken` on `;`.
    assert_eq!(
        error_codes("import 5;"),
        ["expected_string_literal", "unexpected_token"]
    );
    assert_eq!(
        error_codes("part 5;"),
        [
            "expected_string_literal",
            "expected_token",
            "expected_executable",
            "unexpected_token"
        ]
    );
}

#[test]
fn combinator_missing_name() {
    assert_eq!(error_codes("import 'a.dart' show;"), ["missing_identifier"]);
}

#[test]
fn conditional_uri_missing_paren() {
    // insertParens recovery (official :1136-1143): a synthetic
    // `( <empty-id> )` triple stands in for the paren pair; the offending
    // `x` then fails the URI read (expectedString only — the string
    // recovery reports no unexpectedToken). The synthetic string is spliced
    // BEFORE `x` (`insertToken`, token_stream_rewriter.dart:85), so `x`
    // survives to the import recovery boundary and is re-reported there
    // (`skipUnexpectedTokenOpt` :5148 via `parseImportRecovery`
    // :1022-1025 — `x` precedes the `;`).
    assert_eq!(
        error_codes("import 'a.dart' if x;"),
        [
            "expected_token",
            "expected_string_literal",
            "unexpected_token"
        ]
    );
}

#[test]
fn conditional_uri_garbage_before_close() {
    // A well-formed but misplaced group close: the intervening token is
    // reported and the real `)` is jumped to (official :1124-1136).
    assert_eq!(
        error_codes("import 'a.dart' if (x y) 'b.dart';"),
        ["unexpected_token"]
    );
}

#[test]
fn library_name_missing() {
    // The synthetic name lands before `5`, so the semicolon recovery
    // reports against it (official order: missing_identifier, then the
    // `expected ';'`); the leftover `5;` then member-parses with its own
    // recovery tail.
    let codes = error_codes("library 5;");
    assert_eq!(&codes[..2], ["missing_identifier", "expected_token"]);
}

#[test]
fn repeated_part_directives_are_legal() {
    // checkPart admits Part → Part (directive_context.dart:91); a
    // declaration between them is still the after-declaration error.
    assert!(error_codes("part 'a.dart'; part 'b.dart';").is_empty());
    assert_eq!(
        error_codes("part 'a.dart'; int x = 1; part 'b.dart';"),
        ["directive_after_declaration"]
    );
}

#[test]
fn directive_modifier_errors() {
    // parseTopLevelKeywordModifiers (modifier_context.dart:674-688):
    // `external` reports extraneousModifier before anything but
    // class/enum/typedef, and `const` reports constClass only before
    // `class`; every other modifier is extraneous. (var/late/final/const
    // before a directive never reach this walk — the official dispatch
    // routes them to member parsing, parser_impl.dart:613-624.)
    assert_eq!(
        error_codes("external import 'a.dart';"),
        ["extraneous_modifier"]
    );
    assert_eq!(
        error_codes("abstract part 'a.dart';"),
        ["extraneous_modifier"]
    );
    assert_eq!(
        error_codes("augment export 'a.dart';"),
        ["extraneous_modifier"]
    );
    assert_eq!(
        error_codes("static import 'a.dart';"),
        ["extraneous_modifier"]
    );
}

#[test]
fn augmentation_modifier_is_captured_for_library_only() {
    // `augment library` selects the augmentation form; `augment import`
    // reports extraneous (modifier_context.dart:188).
    let c = parse("augment library 'a.dart';");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "library_augmentation"),
        ("end", "library_augmentation"),
    ]);
    assert_eq!(
        error_codes("augment import 'a.dart';"),
        ["extraneous_modifier"]
    );
}
