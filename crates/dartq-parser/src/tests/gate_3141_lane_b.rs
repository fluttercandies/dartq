//! #3141 lane B recovery faces: the expression/literal/pattern
//! `expected_token`/`missing_identifier` surfaces. Every pinned row is the
//! frozen oracle row (3.13.7 `dart analyze --format=machine`, syntactic
//! face; semantic-tier rows are out of the parser's window). The list/map
//! pattern closer adoption pins the `moveSynthetic` relocation: an
//! unclosed pattern bracket's scanner-synthetic closer re-anchors the
//! unmatched-begin row to the token the pattern resumes behind
//! (official `parseListPatternSuffix`, pkg/_fe_analyzer_shared/lib/src/
//! parser/parser_impl.dart:12051-12053, with
//! token_stream_rewriter.dart:105-134 `_setOffset(endGroup, next.offset)`).

use std::collections::BTreeSet;

use super::collector::{latest_features, Collector};
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
fn list_pattern_unclosed_adopts_synthetic_closer_at_resume() {
    // The element junk `;` takes the missing-identifier synthetic face; the
    // unclosed `[`'s scanner-synthetic `]` is adopted at the resume token —
    // the unmatched row re-anchors onto the `:` (oracle: exactly these two
    // rows, never a duplicate at the parked position, and the case body
    // resumes on `: break;`).
    assert_rows(
        "void f() { switch (y) { case [;: break; } }",
        &[
            ("missing_identifier", 30, 31),
            ("expected_token", 31, 32),
        ],
    );
}

#[test]
fn missing_expression_and_operand_faces() {
    // `parsePrimary`'s identifier-recovery face (`var x = ;`), the prefix
    // `+` unsupported-operand face (official `unsupportedPrefixPlus`,
    // parser_impl.dart:7904-7912 — pseudo-shared missing_identifier), the
    // prefix `!`/`-` operand faces, and the `??` RHS face all anchor on the
    // blocker token, length 1.
    assert_rows("var x = ;", &[("missing_identifier", 8, 9)]);
    assert_rows("var x = !;", &[("missing_identifier", 9, 10)]);
    assert_rows("var x = -;", &[("missing_identifier", 9, 10)]);
    assert_rows("var x = (a +);", &[("missing_identifier", 12, 13)]);
    assert_rows("var x = a ??;", &[("missing_identifier", 12, 13)]);
}

#[test]
fn selector_and_index_missing_faces() {
    // The dot-continuation operand face (`parsePrecedence`'s
    // ExpressionContinuation — one row on the literal operand, never a
    // cascade), the empty-index face (`rewriteSquareBrackets`-adjacent
    // `a[]` shape: one row on the `]`), the missing-index and missing-closer
    // index faces (official :8017-8032), and the missing-argument faces.
    assert_rows("var x = a.;", &[("missing_identifier", 10, 11)]);
    assert_rows("var x = a.();", &[("missing_identifier", 10, 11)]);
    assert_rows("var x = a[;", &[
        ("missing_identifier", 10, 11),
        ("expected_token", 10, 11),
    ]);
    assert_rows("var x = a[1;", &[("expected_token", 11, 12)]);
    assert_rows("var x = f(;", &[
        ("missing_identifier", 10, 11),
        ("expected_token", 10, 11),
    ]);
    assert_rows("var x = f(a,;", &[
        ("missing_identifier", 12, 13),
        ("expected_token", 12, 13),
    ]);
}

#[test]
fn collection_literal_element_faces() {
    // List/set/map element junk: the missing-comma split, the element
    // identifier recovery, and the closer cascade (official
    // `parseListLiteralAndRest`/`parseSetOrMapLiteral` recovery paths).
    assert_rows("var x = [1, ;]", &[
        ("missing_identifier", 12, 13),
        ("expected_token", 12, 13),
        ("expected_token", 13, 14),
    ]);
    assert_rows("var x = {1, ;}", &[
        ("missing_identifier", 12, 13),
        ("expected_token", 12, 13),
        ("expected_token", 13, 14),
    ]);
    assert_rows("var x = {1: ;}", &[
        ("missing_identifier", 12, 13),
        ("expected_token", 12, 13),
        ("expected_token", 13, 14),
    ]);
    assert_rows("var x = {:1, ;}", &[
        ("missing_identifier", 9, 10),
        ("missing_identifier", 13, 14),
        ("expected_token", 13, 14),
        ("expected_token", 14, 15),
    ]);
}

#[test]
fn record_literal_named_field_faces() {
    // The named-field junk name is REPLACED by the synthetic (branch 3
    // `insertSyntheticIdentifier`, identifier_context_impl.dart:1283), so
    // the expect-`:` and the record close stay silent — one row per junk
    // name (oracle `var r = (5: 1);` posture).
    assert_rows("var x = (a:1, :2);", &[("missing_identifier", 14, 15)]);
    assert_rows("var x = (5: 1);", &[("missing_identifier", 9, 10)]);
    assert_rows("var x = (:);", &[
        ("missing_identifier", 9, 10),
        ("missing_identifier", 10, 11),
    ]);
}

#[test]
fn conditional_cascade_and_type_faces() {
    // The conditional then/else operand faces, the null-aware cascade's
    // empty-section face, and the `is`/`as` type faces (official
    // `computeTypeAfterIsOrAs` recovery — `expected_type_name` on the
    // blocker, length 1).
    assert_rows("var x = a ? b : ;", &[("missing_identifier", 16, 17)]);
    assert_rows("var x = a ? : b;", &[("missing_identifier", 12, 13)]);
    assert_rows("var x = a ?.. ;", &[("missing_identifier", 14, 15)]);
    assert_rows("var x = a is ;", &[("expected_type_name", 13, 14)]);
    assert_rows("var x = a as ;", &[("expected_type_name", 13, 14)]);
}

#[test]
fn switch_expression_arm_faces() {
    // The missing switch-expression pattern face (`=>` preceded by junk)
    // and the arm-comma face (official `parseSwitchExpression` recovery).
    assert_rows(
        "var x = switch(y) { 1 => a, => b };",
        &[
            ("missing_identifier", 28, 30),
            ("expected_token", 31, 32),
        ],
    );
}

#[test]
fn record_pattern_colon_prefix_splices_synthetic_name() {
    // The `(: )` record-pattern field: the pattern position after the
    // colon takes the identifier-recovery synthetic face on the `)` (the
    // semantic-tier `missing_named_pattern_field_name` companion row is the
    // resolution window's, outside the parser tier).
    assert_rows(
        "void f() { switch (y) { case (: ): break; } }",
        &[("missing_identifier", 32, 33)],
    );
}
