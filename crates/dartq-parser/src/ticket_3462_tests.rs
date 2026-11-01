//! Ticket #3462 — the shipped features' off-version
//! `experiment_not_enabled` stances. One pre-boundary positive + one
//! post-boundary negative per feature, each frozen to the oracle 3.13.1
//! row (`dart analyze --format=json`, probed):
//!
//! * `class-modifiers` (`enabledIn: 3.0.0`) — `base class` below 3.0
//!   reports at the `base` token (`beginClassDeclaration`,
//!   ast_builder.dart:252-256).
//! * `sealed-class` (`enabledIn: 3.0.0`) — `sealed class` below 3.0
//!   reports at the `sealed` token (ast_builder.dart:242-250).
//! * `records` (`enabledIn: 3.0.0`) — a record literal below 3.0 reports
//!   at the `(` and degrades to the parenthesized shape
//!   (`endRecordLiteral`, ast_builder.dart:2786-2809).
//! * `digit-separators` (`enabledIn: 3.6.0`) — `1_000` below 3.6 reports
//!   at the literal token (`handleLiteralIntWithSeparators`,
//!   ast_builder.dart:4706-4730).
//! * `primary-constructors` (`enabledIn: 3.13.0`) — `enum E;` below 3.13
//!   reports at the `;` (parser_impl.dart:2623-2631) and a `new ()`
//!   member at the `new` token (parser_impl.dart:5380-5386).
//! * `triple-shift` (`enabledIn: 2.14.0`) — with the lexical gate off the
//!   scanner hands adjacent `>>` `>`; the binary use reports the pair
//!   plus the zero-width synthetic (parser_impl.dart:7386-7402 +
//!   ast_builder.dart:1084-1093) and `operator >>>` reports the pair
//!   once (parser_impl.dart:5786-5800).

use std::collections::BTreeSet;

use crate::parser::Parser;
use crate::tests::collector::Collector;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_scanner::ScanConfig;

/// Parses `source` at language version `major.minor` and returns the
/// scanner+parser rows as a set of `(code, start, end)`.
fn rows_at(source: &'static str, major: u32, minor: u32) -> BTreeSet<(String, u32, u32)> {
    let set = FeatureSet::for_language_version(LanguageVersion::new(major, minor));
    let config = ScanConfig::from_feature_set(set);
    let features = crate::ParserFeatures::from_feature_set(set);
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

fn assert_rows_at(
    source: &'static str,
    major: u32,
    minor: u32,
    expected: &[(&str, u32, u32)],
) {
    let expected: BTreeSet<(String, u32, u32)> = expected
        .iter()
        .map(|(c, s, e)| ((*c).to_owned(), *s, *e))
        .collect();
    assert_eq!(
        rows_at(source, major, minor),
        expected,
        "rows for {source:?} at {major}.{minor}"
    );
}

#[test]
fn class_modifiers_reports_below_three_zero() {
    // Oracle 3.13.1 at @dart=2.19: `base class CM {}` reports exactly
    // `experiment_not_enabled` at `base` (1:1 len 4); the modifier is
    // dropped and the class parses clean.
    assert_rows_at("base class CM {}\n", 2, 19, &[("experiment_not_enabled", 0, 4)]);
    // Post-boundary: silent.
    assert_rows_at("base class CM {}\n", 3, 15, &[]);
}

#[test]
fn sealed_class_reports_below_three_zero() {
    // Oracle 3.13.1 at @dart=2.19: `sealed class SC {}` reports at
    // `sealed` (1:1 len 6).
    assert_rows_at(
        "sealed class SC {}\n",
        2,
        19,
        &[("experiment_not_enabled", 0, 6)],
    );
    assert_rows_at("sealed class SC {}\n", 3, 15, &[]);
}

#[test]
fn records_report_below_three_zero() {
    // Oracle 3.13.1 at @dart=2.19: `var rec = (1, 2);` reports once at
    // the `(` (1:11 len 1); the node degrades to the parenthesized
    // shape with no further rows.
    assert_rows_at("var rec = (1, 2);\n", 2, 19, &[("experiment_not_enabled", 10, 11)]);
    assert_rows_at("var rec = (1, 2);\n", 3, 15, &[]);
}

#[test]
fn digit_separators_report_below_three_six() {
    // Oracle 3.13.1 at @dart=3.5: `var ds = 1_000;` reports at the
    // literal (1:10 len 5).
    assert_rows_at("var ds = 1_000;\n", 3, 5, &[("experiment_not_enabled", 9, 14)]);
    assert_rows_at("var ds = 1_000;\n", 3, 15, &[]);
}

#[test]
fn primary_constructors_report_below_three_thirteen() {
    // Oracle 3.13.1 at @dart=3.12: `enum E;` reports at the `;`
    // (parser_impl.dart:2623-2631, `(leftBrace, leftBrace)` — a token-
    // length row at the engine tier) and `new ();` at the `new` token
    // (parser_impl.dart:5380-5386, `(newToken, newToken)`). The test
    // Collector records the listener route begin-only (its
    // `handle_experiment_not_enabled` collapses end to begin), so the
    // pinned shapes here are zero-length at the anchor offsets.
    assert_rows_at("enum E;\n", 3, 12, &[("experiment_not_enabled", 6, 6)]);
    assert_rows_at("enum E;\n", 3, 15, &[]);
    assert_rows_at(
        "class N {\n  new ();\n}\n",
        3,
        12,
        &[("experiment_not_enabled", 12, 12)],
    );
    assert_rows_at("class N {\n  new ();\n}\n", 3, 15, &[]);
}

#[test]
fn triple_shift_reports_below_two_fourteen() {
    // Oracle 3.13.1 at @dart=2.13: `var ts = a >>> b;` reports twice at
    // the `>>>` start — the real-token pair (len 3) and the zero-width
    // synthetic (len 0) — and `operator >>>(int k)` once (the pair).
    assert_rows_at(
        "var ts = a >>> b;\n",
        2,
        13,
        &[
            ("experiment_not_enabled", 11, 14),
            ("experiment_not_enabled", 11, 11),
        ],
    );
    assert_rows_at("var ts = a >>> b;\n", 3, 15, &[]);
    assert_rows_at(
        "class Op {\n  operator >>>(int k) => 42;\n}\n",
        2,
        13,
        &[("experiment_not_enabled", 22, 25)],
    );
    assert_rows_at("class Op {\n  operator >>>(int k) => 42;\n}\n", 3, 15, &[]);
}
