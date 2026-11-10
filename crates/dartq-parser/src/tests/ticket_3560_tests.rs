//! Ticket #3560 — the nine missing shipped-feature version gates. Each pin
//! parses one construct on both sides of its `enabledIn` boundary: the
//! feature-off side must carry the analyzer's gate row, the feature-on side
//! must stay silent. Upstream gate sites (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/fasta/ast_builder.dart`):
//!
//! * `constructor-tearoffs` (2.15) — `handleNewAsIdentifier` :4984 (at the
//!   rewritten `new`), `handleTypeArgumentApplication` :5602 (over the
//!   `<...>` group).
//! * `named-arguments-anywhere` (2.17) — `endArguments` :970
//!   (`positional_after_named_argument` over the positional's expression).
//! * `super-parameters` (2.17) — `endFormalParameter` :1789 (at `super`).
//! * `enhanced-enums` (2.17) — `handleEnumElement` :4035 (at the `(` / the
//!   constructor-reference head), `handleEnumElements` :4094 (at the `;`),
//!   `handleEnumHeader` :4131 (at the first present clause).
//! * `generic-metadata` (2.14) — `endMetadata` :2423 (at the `<`).
//! * `nonfunction-type-aliases` (2.13) — `endGenericTypeAlias` :3373 (at the
//!   `=`), silent for a function-type right-hand side.
//! * `inline-class` (3.3) — `endExtensionTypeDeclaration` :1607 (at `type`).
//! * `unnamed-libraries` (2.19) — `endLibraryName` :2281 (at `library`).
//! * `null-aware-elements` (3.8) — `handleNullAwareElement` :5119 /
//!   `handleLiteralMapEntry` :4771 (at the `?`).

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

fn assert_rows_at(source: &'static str, major: u32, minor: u32, expected: &[(&str, u32, u32)]) {
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
fn constructor_tearoffs_report_below_two_fifteen() {
    // `A.new` names the unnamed constructor through the `new` rewrite.
    assert_rows_at(
        "var f = A.new;\n",
        2,
        14,
        &[("experiment_not_enabled", 10, 13)],
    );
    assert_rows_at("var f = A.new;\n", 2, 16, &[]);
    // A standalone `<...>` application spans the whole group.
    assert_rows_at(
        "void f() { g<int>; }\n",
        2,
        14,
        &[("experiment_not_enabled", 12, 17)],
    );
    assert_rows_at("void f() { g<int>; }\n", 2, 16, &[]);
}

#[test]
fn named_arguments_anywhere_reports_below_two_seventeen() {
    // The positional after the named one reports over its own expression.
    assert_rows_at(
        "void f() { g(a: 1, 2); }\n",
        2,
        16,
        &[("positional_after_named_argument", 19, 20)],
    );
    assert_rows_at("void f() { g(a: 1, 2); }\n", 2, 19, &[]);
}

#[test]
fn super_parameters_report_below_two_seventeen() {
    assert_rows_at(
        "class C {\n  C(super.y);\n}\n",
        2,
        16,
        &[("experiment_not_enabled", 14, 19)],
    );
    assert_rows_at("class C {\n  C(super.y);\n}\n", 2, 19, &[]);
}

#[test]
fn enhanced_enums_report_below_two_seventeen() {
    // A constant with arguments reports at the `(`.
    assert_rows_at(
        "enum E { a(1) }\n",
        2,
        16,
        &[("experiment_not_enabled", 10, 11)],
    );
    assert_rows_at("enum E { a(1) }\n", 2, 19, &[]);
    // A `;` closing the constant list.
    assert_rows_at(
        "enum E { a; }\n",
        2,
        16,
        &[("experiment_not_enabled", 10, 11)],
    );
    assert_rows_at("enum E { a; }\n", 2, 19, &[]);
    // A header clause reports once, at the first present construct.
    assert_rows_at(
        "enum E with M {}\n",
        2,
        16,
        &[("experiment_not_enabled", 7, 11)],
    );
    assert_rows_at("enum E with M {}\n", 2, 19, &[]);
    assert_rows_at("enum E<T> {}\n", 2, 16, &[("experiment_not_enabled", 6, 7)]);
    assert_rows_at("enum E<T> {}\n", 2, 19, &[]);
}

#[test]
fn generic_metadata_reports_below_two_fourteen() {
    assert_rows_at(
        "@A<int>()\nclass C {}\n",
        2,
        13,
        &[("experiment_not_enabled", 2, 3)],
    );
    assert_rows_at("@A<int>()\nclass C {}\n", 2, 14, &[]);
}

#[test]
fn nonfunction_type_aliases_report_below_two_thirteen() {
    assert_rows_at(
        "typedef T = int;\n",
        2,
        12,
        &[("experiment_not_enabled", 10, 11)],
    );
    assert_rows_at("typedef T = int;\n", 2, 13, &[]);
    // A function-type right-hand side is the old alias shape: never gated.
    assert_rows_at("typedef T = int Function();\n", 2, 12, &[]);
}

#[test]
fn inline_class_reports_below_three_three() {
    assert_rows_at(
        "extension type E(int i) {}\n",
        3,
        2,
        &[("experiment_not_enabled", 10, 14)],
    );
    assert_rows_at("extension type E(int i) {}\n", 3, 5, &[]);
}

#[test]
fn unnamed_libraries_report_below_two_nineteen() {
    assert_rows_at("library;\n", 2, 16, &[("experiment_not_enabled", 0, 7)]);
    assert_rows_at("library;\n", 2, 19, &[]);
    // A named library directive is never gated.
    assert_rows_at("library a.b;\n", 2, 16, &[]);
}

#[test]
fn null_aware_elements_report_below_three_eight() {
    // A `?e` list element.
    assert_rows_at(
        "var l = [?a];\n",
        3,
        7,
        &[("experiment_not_enabled", 9, 10)],
    );
    assert_rows_at("var l = [?a];\n", 3, 8, &[]);
    // A `?key` map-entry marker.
    assert_rows_at(
        "var m = {?k: v};\n",
        3,
        7,
        &[("experiment_not_enabled", 9, 10)],
    );
    assert_rows_at("var m = {?k: v};\n", 3, 8, &[]);
    // A `?value` map-entry marker.
    assert_rows_at(
        "var n = {k: ?v};\n",
        3,
        7,
        &[("experiment_not_enabled", 12, 13)],
    );
    assert_rows_at("var n = {k: ?v};\n", 3, 8, &[]);
}
