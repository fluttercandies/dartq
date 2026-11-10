//! Ticket #3463 — the `missing_function_body` carve-outs. Each positive
//! freeze is a shape the code 1:1-locked oracle (3.13.1
//! `dart analyze --format=json`) leaves silent; the negative pins the
//! genuinely body-less row:
//!
//! * augmentations (parser_impl.dart:4606-4610 `parseTopLevelMethod`:
//!   `allowAbstract = isExternal || isAugmentationsFeatureEnabled`;
//!   :6029-6035 `parseMethod` adds `&& inPlainSync`) — with the
//!   augmentations feature on, a `;` declaration body is legal and its
//!   augmenting counterpart never reports.
//! * external (`isExternal` in both formulas) — `external void g();` is
//!   silent at the parse tier.
//! * abstract-legal members (`parseMethod`'s non-static + plain-sync
//!   arm) — a non-static `;` getter is silent.
//! * genuinely body-less — the `;` arm reports
//!   `missing_function_body` ("A function body must be provided.") at
//!   the `;`.

use std::collections::BTreeSet;

use crate::parser::Parser;
use crate::tests::collector::Collector;
use dartq_language::{Feature, FeatureSet, LanguageVersion};
use dartq_scanner::ScanConfig;

/// Parses `source` at `set` (both the scanner fold and the parser fold
/// derive from the same [`FeatureSet`]) and returns the scanner+parser
/// rows as `(code, start, end)`.
fn rows_at(source: &'static str, set: FeatureSet) -> BTreeSet<(String, u32, u32)> {
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

fn assert_rows_at(source: &'static str, set: FeatureSet, expected: &[(&str, u32, u32)]) {
    let expected: BTreeSet<(String, u32, u32)> = expected
        .iter()
        .map(|(c, s, e)| ((*c).to_owned(), *s, *e))
        .collect();
    assert_eq!(rows_at(source, set), expected, "rows for {source:?}");
}

#[test]
fn augmentations_body_is_silent_with_the_feature() {
    // Oracle 3.13.1 with augmentations enabled: the `;` declaration body
    // plus its augmenting counterpart (co19 language/augmentation library
    // shapes) produce zero diagnostics — `isAugmentationsFeatureEnabled`
    // arms the `allowAbstract` formula (parser_impl.dart:4606-4610) and
    // the static+sync member arm (:6029-6035).
    assert_rows_at(
        "class A {\n  static int get foo;\n  augment static int get foo;\n}\n",
        FeatureSet::latest().with_experiment(Feature::Augmentations),
        &[],
    );
}

#[test]
fn external_function_is_silent() {
    // Oracle 3.13.1: `external void g();` produces zero diagnostics —
    // `isExternal` bypasses the body requirement outright
    // (parser_impl.dart:4606-4610).
    assert_rows_at("external void g();\n", latest_set(), &[]);
}

#[test]
fn abstract_legal_member_is_silent() {
    // Oracle 3.13.1: a non-static plain-sync `;` member is abstract-legal
    // at the parse tier (parseMethod's allowAbstract arm,
    // parser_impl.dart:6029-6035) and never reports the code.
    assert_rows_at("class A {\n  int get ok;\n}\n", latest_set(), &[]);
}

#[test]
fn body_less_function_reports() {
    // Oracle 3.13.1: `void f();` reports exactly once at the `;` (1:9
    // len 1, "A function body must be provided."), the parseFunctionBody
    // `;` arm's `missingFunctionBody` row.
    assert_rows_at(
        "void f();\n",
        latest_set(),
        &[("missing_function_body", 8, 9)],
    );
}

/// The latest-version set (the `latest_features` fold's source set).
fn latest_set() -> FeatureSet {
    FeatureSet::for_language_version(LanguageVersion::new(3, 15))
}
