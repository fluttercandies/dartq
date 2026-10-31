//! Ticket #2978 — token-family residual pins. One positive + one negative
//! per residual code, each frozen to the oracle 3.13.7 row
//! (`dart analyze --format=json`, registry corpus `expected.tsv`):
//!
//! * `unexpected_token` — junk-primary face, anchored on the offending
//!   token (`parser_impl.dart:1143` family), message
//!   "Unexpected text '{lexeme}'."
//! * `expected_identifier_but_got_keyword` — the keyword-named face:
//!   a reserved word in a name slot that the context's recovery template
//!   consumes as the name (`identifier_context_impl.dart`
//!   `ensureIdentifier` recovery branches), message
//!   "'{lexeme}' can't be used as an identifier because it's a keyword."
//! * `expected_body` — `ensureBlock`'s declaration kinds
//!   (`block_kind.dart`, `parser_impl.dart:4939`), re-anchored through
//!   `findNonZeroLengthToken`, message
//!   "A {kind} declaration must have a body, even if it is empty."
//! * `expected_class_member` — the member loop's no-progress arm
//!   (`token == beforeStart`, `parser_impl.dart:11322`), message
//!   "Expected a class member."
//! * `missing_function_body` — the function-body `;` arm
//!   (`parseFunctionBody`/`skipFunctionBody`, `diag.expectedBody`
//!   pseudo-shared `missingFunctionBody`, re-rendered by the analyzer
//!   converter as fixed text "A function body must be provided.").
//!
//! Negative siblings freeze the shapes that must NOT report: the legal
//! expression sibling, a keyword consumed in a legal position, a class
//! with a body, the same class body without the stray `;`, and the
//! abstract-legal (non-static) `;` member.

use std::collections::BTreeSet;

use crate::parser::Parser;
use crate::tests::collector::{latest_features, Collector};
use dartq_scanner::ScanConfig;

/// Parses `source` and returns the scanner+parser rows as a set of
/// `(code, start, end)` (the oracle rows compare position-sorted).
fn rows(source: &'static str) -> BTreeSet<(String, u32, u32)> {
    let config = ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut files = dartq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("test.dart"));
    let result = dartq_scanner::scan(source, config, file);
    assert!(
        result.diagnostics.is_empty(),
        "scanner diagnostics in {source:?}: {:?}",
        result.diagnostics
    );
    let mut rows: BTreeSet<(String, u32, u32)> = result
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.name.to_owned(),
                d.range.offset,
                d.range.offset + d.range.length,
            )
        })
        .collect();
    let mut parser = Parser::new(
        source,
        Box::leak(Box::new(result.tokens)),
        latest_features(),
        Box::leak(Box::new(Collector::default())),
    );
    parser.parse_unit();
    rows.extend(parser.listener.errors.iter().cloned());
    rows
}

fn assert_rows(source: &'static str, expected: &[(&str, u32, u32)]) {
    let expected: BTreeSet<(String, u32, u32)> = expected
        .iter()
        .map(|(c, s, e)| ((*c).to_owned(), *s, *e))
        .collect();
    assert_eq!(rows(source), expected, "rows for {source:?}");
}

#[test]
fn unexpected_token_at_offending_token() {
    // Oracle: RecoveryParserTest/test_incomplete_functionExpression —
    // `var v = () a => null;` reports exactly once, at the stray `a`
    // (2:12 len 1, "Unexpected text 'a'."). The junk token itself is the
    // anchor; nothing else fires.
    // Negative: the legal function-expression sibling stays silent.
    assert_rows("\nvar v = () a => null;\n", &[("unexpected_token", 12, 13)]);
    assert_rows("\nvar v = () => null;\n", &[]);
}

#[test]
fn keyword_named_in_name_slot() {
    // Oracle: parser-fe/error_recovery/issue_48371_prime1 — `library enum;`
    // reports exactly `expected_identifier_but_got_keyword` at the reserved
    // word (1:9 len 4, "'enum' can't be used as an identifier because it's
    // a keyword."), consumed as the library name by the context's recovery
    // template.
    // Negative: `library;` is a legal empty library directive.
    assert_rows(
        "library enum;\n\nmain() {}\n",
        &[("expected_identifier_but_got_keyword", 8, 12)],
    );
    assert_rows("library;\n\nmain() {}\n", &[]);
}

#[test]
fn expected_body_class_without_body() {
    // Oracle: ErrorParserTest/test_expectedBody_class — `class A class B {}`
    // reports exactly once at the recovered name `A` (2:7 len 1, "A class
    // declaration must have a body, even if it is empty."), the
    // `ensureBlock(BlockKind.classDeclaration)` row re-anchored on the real
    // follower token per `findNonZeroLengthToken`.
    // Negative: a class with a body never reports the code.
    assert_rows("\nclass A class B {}\n", &[("expected_body", 7, 8)]);
    assert_rows("\nclass A {}\n", &[]);
}

#[test]
fn expected_class_member_at_stray_semicolon() {
    // Oracle: PunctuationTest/test_extraSemicolon_betweenClassMembers —
    // `class C { foo() {}; bar() {} }` reports exactly once at the stray
    // `;` (3:11 len 1, "Expected a class member."), the member loop's
    // `token == beforeStart` arm (parser_impl.dart:11322) with
    // `handleInvalidMember`.
    // Negative: the same members without the stray `;` stay silent.
    assert_rows(
        "class C {\n  foo() {};\n  bar() {}\n}\n",
        &[("expected_class_member", 20, 21)],
    );
    assert_rows("class C {\n  foo() {}\n  bar() {}\n}\n", &[]);
}

#[test]
fn missing_function_body_on_semicolon_member() {
    // Oracle: ExecutableBodyTest/test_class_getter_static_noBody —
    // `class A { static int get foo; }` reports exactly once at the `;`
    // (3:21 len 1, "A function body must be provided."), the
    // `parseFunctionBody` `;` arm's `diag.expectedBody` pseudo-shared
    // `missingFunctionBody` row (static members can never be abstract, so
    // `allowAbstract` is false).
    // Negative: a non-static `;` member is abstract-legal at the parse
    // tier and must not report the code.
    assert_rows(
        "\n// @dart = 3.5\nclass A {\n  static int get foo;\n}\n",
        &[("missing_function_body", 46, 47)],
    );
    assert_rows("\nclass A {\n  int get foo;\n}\n", &[]);
}
