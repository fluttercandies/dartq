//! #3135 lane C recovery faces: the for-header synthetic-group stream (the
//! no-`(` recovery runs entirely on queued synthetics), the block-loop
//! no-progress guard quoting the semicolon-recovery synthetic, the
//! metadata/local-variable name consume-in-doubt posture, and the field
//! name == enclosing declaration row. Every pinned span is the frozen
//! oracle row (3.13.1 `dart analyze --format=json` syntactic face).

use std::collections::BTreeSet;

use super::collector::{Collector, latest_features};
use crate::ParserFeatures;
use crate::parser::Parser;
use dartq_scanner::ScanConfig;

/// Parses `source` and returns the scanner+parser rows as a set of
/// `(code, start, end)` (the oracle rows compare position-sorted).
fn rows(features: ParserFeatures, source: &'static str) -> BTreeSet<(String, u32, u32)> {
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
        features,
        Box::leak(Box::new(Collector::default())),
    );
    parser.parse_unit();
    rows.extend(parser.listener.errors.iter().cloned());
    rows
}

fn rows_latest(source: &'static str) -> BTreeSet<(String, u32, u32)> {
    rows(latest_features(), source)
}

fn assert_rows(source: &'static str, expected: &[(&str, u32, u32)]) {
    let expected: BTreeSet<(String, u32, u32)> = expected
        .iter()
        .map(|(c, s, e)| ((*c).to_owned(), *s, *e))
        .collect();
    assert_eq!(rows_latest(source), expected, "rows for {source:?}");
}

#[test]
fn for_no_paren_single_row() {
    // The synthetic `(...)` group absorbs the parts parse; the real `x`
    // becomes the body statement (oracle: only the `(` report).
    assert_rows("void f() { for x; }", &[("expected_token", 15, 16)]);
    assert_rows("void f() { for ; }", &[("expected_token", 15, 16)]);
    assert_rows("void f() { for x }", &[("expected_token", 15, 16)]);
}

#[test]
fn await_for_no_paren_single_row() {
    // No `(`: the single `(` report; the real head stays for the body.
    assert_rows(
        "void f() async { await for x; }",
        &[("expected_token", 27, 28)],
    );
    // `await for x in y;`: the body statement `x in y;` reads as a broken
    // declaration (type `x`, name slot at the reserved `in`). The oracle
    // carries five diagnostics here — `(` and `;` both anchor at the `x`
    // span, so the (code, start, end) tuple set folds them into one entry.
    assert_rows(
        "void f() async { await for x in y; }",
        &[
            ("expected_token", 27, 28),
            ("expected_identifier_but_got_keyword", 29, 31),
            ("expected_token", 29, 31),
            ("missing_statement", 29, 31),
        ],
    );
}

#[test]
fn var_junk_name_consumed_in_doubt() {
    // `5` is absorbed behind the parse pointer; the initializer reads
    // cleanly (oracle: one missing_identifier, no semicolon cascade).
    assert_rows("void f() { var 5 = 1; }", &[("missing_identifier", 15, 16)]);
}

#[test]
fn block_guard_quotes_semicolon_synthetic() {
    // `Unexpected text ';'.` — the stalled statement's end token is the
    // semicolon-recovery synthetic, never the junk it precedes.
    assert_rows(
        "void f() { : }",
        &[
            ("expected_token", 9, 10),
            ("missing_identifier", 11, 12),
            ("unexpected_token", 11, 12),
        ],
    );
    assert_rows(
        "void f() { : ; }",
        &[
            ("expected_token", 9, 10),
            ("missing_identifier", 11, 12),
            ("unexpected_token", 11, 12),
        ],
    );
    assert_rows(
        "void f() { , ; }",
        &[
            ("expected_token", 9, 10),
            ("missing_identifier", 11, 12),
            ("unexpected_token", 11, 12),
        ],
    );
    assert_rows(
        "void f() { % : }",
        &[
            ("missing_identifier", 11, 12),
            ("expected_token", 11, 12),
            ("missing_identifier", 13, 14),
            ("unexpected_token", 13, 14),
        ],
    );
}

#[test]
fn junk_keyword_statement_guard_face() {
    // `case`/`default` at statement position: the full oracle cascade, with
    // the guard row quoting `;` at the colon.
    assert_rows(
        "void f() { case 1: }",
        &[
            ("expected_identifier_but_got_keyword", 11, 15),
            ("expected_token", 11, 15),
            ("expected_token", 16, 17),
            ("missing_identifier", 17, 18),
            ("missing_statement", 11, 15),
            ("unexpected_token", 17, 18),
        ],
    );
    assert_rows(
        "void f() { default: }",
        &[
            ("expected_identifier_but_got_keyword", 11, 18),
            ("expected_token", 11, 18),
            ("missing_identifier", 18, 19),
            ("missing_statement", 11, 18),
            ("unexpected_token", 18, 19),
        ],
    );
    assert_rows(
        "void f() { 5: print(1); }",
        &[
            ("expected_token", 11, 12),
            ("missing_identifier", 12, 13),
            ("unexpected_token", 12, 13),
        ],
    );
}

#[test]
fn metadata_junk_name_consumed_in_doubt() {
    // The annotation-name junk is absorbed; the member-head no-progress
    // guard reports on the FOLLOWER (`;` or `}`).
    assert_rows(
        "class C { @ ; }",
        &[
            ("missing_identifier", 12, 13),
            ("expected_class_member", 14, 15),
        ],
    );
    assert_rows(
        "class C { @; }",
        &[
            ("missing_identifier", 11, 12),
            ("expected_class_member", 13, 14),
        ],
    );
    assert_rows(
        "class C { @ 5; }",
        &[
            ("missing_identifier", 12, 13),
            ("expected_class_member", 13, 14),
        ],
    );
    assert_rows(
        "class C { @ : }",
        &[
            ("missing_identifier", 12, 13),
            ("expected_class_member", 14, 15),
        ],
    );
}

#[test]
fn field_named_like_class_reports() {
    assert_rows(
        "class C { int C = 1; }",
        &[("member_with_class_name", 14, 15)],
    );
    assert_rows(
        "class C { var C = 1; }",
        &[("member_with_class_name", 14, 15)],
    );
    assert_rows(
        "class C { C = 1; }",
        &[
            ("member_with_class_name", 10, 11),
            ("missing_const_final_var_or_type", 10, 11),
        ],
    );
    assert_rows(
        "class C { C }",
        &[
            ("member_with_class_name", 10, 11),
            ("expected_token", 10, 11),
            ("missing_const_final_var_or_type", 10, 11),
        ],
    );
    assert_rows(
        "extension Ex on int { int Ex = 1; }",
        &[
            ("member_with_class_name", 26, 28),
            ("extension_declares_instance_field", 26, 28),
        ],
    );
}

#[test]
fn assert_end_group_quotes_junk() {
    // `parseAssert` closes on its own end-group recovery, not
    // `ensureCloseParen`: junk between the condition/message expression and
    // the group's real `)` is reported with `unexpectedToken` at its own
    // face and the `)` is consumed (parser_impl.dart:11091-11102) — one row
    // per shape.
    assert_rows("void f() { assert(x 1); }", &[("unexpected_token", 20, 21)]);
    // Junk after the message expression reads the same way.
    assert_rows(
        "void f() { assert(x, 1 2); }",
        &[("unexpected_token", 23, 24)],
    );
    // Junk before the comma: the condition parse ends at `x`, the guard
    // reports `y`, and the `, z)` tail parses as the message.
    assert_rows(
        "void f() { assert(x y, z); }",
        &[("unexpected_token", 20, 21)],
    );
    // The constructor-initializer context shares `parseAssert`.
    assert_rows(
        "class C { C() : assert(x 1); }",
        &[("unexpected_token", 25, 26)],
    );
}

#[test]
fn assert_synthetic_closer_keeps_expected_face() {
    // Unclosed `(`: the scanner's synthetic closer relocates to the `;`
    // (`moveSynthetic`, parser_impl.dart:11094-11096) and the row keeps the
    // shared `Expected to find ')'.` face.
    assert_rows("void f() { assert(x; }", &[("expected_token", 19, 20)]);
}
