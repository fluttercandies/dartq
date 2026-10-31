//! #3134 lane B pins: the expression/literal recovery faces —
//! `expected_token`/`missing_identifier`-family rows on the record-literal
//! named-field name slot and the constant-pattern prefix/postfix
//! `++`/`--` arms.
//!
//! Root (upstream Dart SDK source):
//!
//! * `NamedRecordFieldReferenceIdentifierContext.ensureIdentifier`
//!   (upstream Dart SDK source,
//!   pkg/_fe_analyzer_shared/lib/src/parser/identifier_context_impl.dart
//!   :1258-1284): `:`/EOF splice the synthetic name in front (branch 2
//!   keep), keywords report `expectedIdentifierButGotKeyword` and are
//!   consumed, and every OTHER junk token is REPLACED by the synthetic —
//!   branch 3's `insertSyntheticIdentifier(identifier)` (:1283) consumes
//!   the junk, so the record's expect-`:` and the group close stay silent.
//! * Prefix `++`/`--` in a constant pattern (`parseUnaryExpression`
//!   :7892-7958 + `handleUnaryPrefixAssignmentExpression`,
//!   `pkg/analyzer/lib/src/fasta/ast_builder.dart`:5647-5656) reports
//!   `missingAssignableSelector` at the operand's endToken when the
//!   operand is not assignable; an assignable operand is silent.
//! * Postfix `++`/`--` (`handleUnaryPostfixAssignmentExpression`,
//!   ast_builder.dart:5620-5642) reports `illegalAssignmentToNonAssignable`
//!   anchored on the operator when the operand is not assignable;
//!   assignable operands are parser-silent (const_patterns_binary 86/87).
//!
//! Every expected row below is transcribed from the 3.13.1 oracle machine
//! output (`dart analyze --format=machine`) for the exact source text.

use super::collector::{Collector, latest_features, scan_ok};

fn rows_with(source: &'static str) -> Vec<(String, u32, u32)> {
    let (table, text) = scan_ok(source);
    let mut collector = Collector::default();
    let mut parser = crate::Parser::new(text, table, latest_features(), &mut collector);
    parser.parse_unit();
    parser.listener.errors.clone()
}

/// `var r = (5: 1);` (oracle: single `missing_identifier` 1:21 on the `5`):
/// the junk named-field name is consumed by the synthetic, so the
/// expect-`:` and the record close never cascade.
#[test]
fn record_int_junk_name_consumes_silently() {
    let rows = rows_with("void f() { var r = (5: 1); }\n");
    assert_eq!(rows, vec![("missing_identifier".to_owned(), 20, 21)]);
}

/// `var x = const (1: 2);` (oracle: single `missing_identifier` 1:16): the
/// const-record named-field face shares the consume posture.
#[test]
fn const_record_int_junk_name_consumes_silently() {
    let rows = rows_with("var x = const (1: 2);\n");
    assert_eq!(rows, vec![("missing_identifier".to_owned(), 15, 16)]);
}

/// `var x = const (1:);` (oracle: `missing_identifier` 1:16 on the `1` +
/// 1:18 on the `)` — the missing VALUE after the colon): the junk name
/// consumes AND the missing-value face still fires.
#[test]
fn const_record_junk_name_and_missing_value() {
    let rows = rows_with("var x = const (1:);\n");
    assert_eq!(
        rows,
        vec![
            ("missing_identifier".to_owned(), 15, 16),
            ("missing_identifier".to_owned(), 17, 18),
        ]
    );
}

/// `var r = ('a': 1);` (oracle: single `missing_identifier` 1:21 spanning
/// the string): string junk anchors on its own span.
#[test]
fn record_string_junk_name_anchors_on_its_span() {
    let rows = rows_with("void f() { var r = ('a': 1); }\n");
    assert_eq!(rows, vec![("missing_identifier".to_owned(), 20, 23)]);
}

/// `var r = (this: 1);` (oracle: `expectedIdentifierButGotKeyword` 1:21 on
/// the keyword): keywords keep the tolerant-consume face, no cascade.
#[test]
fn record_keyword_name_stays_tolerant_consume() {
    let rows = rows_with("void f() { var r = (this: 1); }\n");
    assert_eq!(
        rows,
        vec![(
            "expected_identifier_but_got_keyword".to_owned(),
            20,
            24
        )]
    );
}

/// `var r = (a: (5: 1));` (oracle: single `missing_identifier` 1:25): the
/// consume posture holds in the nested record.
#[test]
fn nested_record_junk_name_consumes_silently() {
    let rows = rows_with("void f() { var r = (a: (5: 1)); }\n");
    assert_eq!(rows, vec![("missing_identifier".to_owned(), 24, 25)]);
}

/// `var r = (5: 1, 6: 2);` (oracle: `missing_identifier` 1:21 + 1:27): each
/// junk name consumes independently, the comma list stays intact.
#[test]
fn two_junk_names_each_consume_independently() {
    let rows = rows_with("void f() { var r = (5: 1, 6: 2); }\n");
    assert_eq!(
        rows,
        vec![
            ("missing_identifier".to_owned(), 20, 21),
            ("missing_identifier".to_owned(), 26, 27),
        ]
    );
}

/// `case const (5: 1):` (oracle: single `missing_identifier` 1:39): the
/// const-PATTERN record parse rides the same consume posture.
#[test]
fn const_pattern_record_junk_name_consumes_silently() {
    let rows = rows_with("void f(x) { switch (x) { case const (5: 1): break; } }\n");
    assert_eq!(rows, vec![("missing_identifier".to_owned(), 37, 38)]);
}

/// `case ++1:` (oracle: `missingAssignableSelector` 1:33 on the `1`): the
/// pattern-unary prefix arm carries the operand check.
#[test]
fn pattern_prefix_increment_non_assignable_reports_selector() {
    let rows = rows_with("void f(x) { switch (x) { case ++1: break; } }\n");
    assert_eq!(
        rows,
        vec![("missing_assignable_selector".to_owned(), 32, 33)]
    );
}

/// `case --1:` (oracle: `missingAssignableSelector` 1:33): the minus-minus
/// prefix face mirrors the increment.
#[test]
fn pattern_prefix_decrement_non_assignable_reports_selector() {
    let rows = rows_with("void f(x) { switch (x) { case --1: break; } }\n");
    assert_eq!(
        rows,
        vec![("missing_assignable_selector".to_owned(), 32, 33)]
    );
}

/// `case ++super:` (oracle: `missingAssignableSelector` 1:33 spanning the
/// `super`): the operand's endToken anchors the row.
#[test]
fn pattern_prefix_increment_super_reports_selector_on_super() {
    let rows = rows_with("void f(x) { switch (x) { case ++super: break; } }\n");
    assert_eq!(
        rows,
        vec![("missing_assignable_selector".to_owned(), 32, 37)]
    );
}

/// `case 1++:` (oracle: `illegalAssignmentToNonAssignable` 1:32 on the
/// `++`): the pattern postfix arm anchors on the operator.
#[test]
fn pattern_postfix_increment_non_assignable_reports_illegal() {
    let rows = rows_with("void f(x) { switch (x) { case 1++: break; } }\n");
    assert_eq!(
        rows,
        vec![(
            "illegal_assignment_to_non_assignable".to_owned(),
            31,
            33
        )]
    );
}

/// `case (++1):` (oracle: `missingAssignableSelector` 1:34): the prefix
/// face holds through a parenthesized pattern operand.
#[test]
fn pattern_prefix_increment_in_parens_reports_selector() {
    let rows = rows_with("void f(x) { switch (x) { case (++1): break; } }\n");
    assert_eq!(
        rows,
        vec![("missing_assignable_selector".to_owned(), 33, 34)]
    );
}

/// `case ++x:` (oracle: silent) — an assignable operand stays parser-silent;
/// the fam11 silent pins' counterpart for the now-gated arm.
#[test]
fn pattern_prefix_increment_assignable_is_silent() {
    let rows = rows_with("void f(x) { switch (x) { case ++x: break; } }\n");
    assert!(rows.is_empty(), "assignable operand is silent, got {rows:?}");
}

/// `[if (x) o++]` (oracle: silent) — the postfix arm keeps the assignable
/// silence inside collection elements.
#[test]
fn collection_element_postfix_assignable_is_silent() {
    let rows = rows_with("void f() { var l = [if (x) o++]; }\n");
    assert!(rows.is_empty(), "assignable operand is silent, got {rows:?}");
}
