//! Ticket #3674 — `missing_function_body` differential closure. The
//! deferred singleton-completeness verdict (`_checkForFunctionBodyCompleteness`,
//! error_verifier.dart:5256-5323) keys its augmentation chain on
//! (enclosing element, name, executable kind, staticness). The parser's
//! `BodilessKey` mirrors that grouping: three pin groups cover the census
//! clusters.
//!
//! * staticness — an `augment` whose `static` differs from the intro's
//!   never joins the intro's element chain (`augmentationWithoutDeclaration`
//!   is the semantic walk's row), so the intro stays a singleton and the
//!   `;` keeps its `missing_function_body` (co19 `augmenting_functions_A08`
//!   family). A matching-`static` `augment` suppresses the same row.
//! * extension/extension-type instance members — the verifier's member
//!   gate (:5273-5280) exempts instance members outright; an extension
//!   instance `;` member never reaches the `missingFunctionBody` arm (it
//!   carries `extensionDeclaresAbstractMember` /
//!   `extensionTypeWithAbstractMember`, reported by the semantic tier).
//!   Only `static` extension/extension-type members route through the
//!   deferred path.
//! * `augment` variable induced accessors — a variable declaration's
//!   implicit getter is a fragment of the getter element, so `get x;`
//!   stops being a singleton the moment `augment var x = …` (or
//!   `augment final x = …`) lands; the setter fragment exists only when
//!   the variable is assignable (non-`final`, non-`const`).

use std::collections::BTreeSet;

use crate::parser::Parser;
use crate::tests::collector::Collector;
use dartq_language::{Feature, FeatureSet, LanguageVersion};
use dartq_scanner::ScanConfig;

/// Parses `source` at `set` and returns the scanner+parser rows as
/// `(code, start, end)`.
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

fn augment_set() -> FeatureSet {
    FeatureSet::latest().with_experiment(Feature::Augmentations)
}

// ------------------------------------------------------------------
// staticness key
// ------------------------------------------------------------------

#[test]
fn augment_without_static_does_not_claim_static_intro() {
    // co19 augmenting_functions_A08 shape: `augment String X()` declares
    // an instance getter chain member, not a static-method fragment, so
    // `static void X();` stays a singleton incomplete intro and reports
    // `missing_function_body` at its `;` (1:14 len 1).
    assert_rows_at(
        "class A {\n  static void X();\n  augment String X() => \"\";\n}\n",
        augment_set(),
        &[("missing_function_body", 27, 28)],
    );
}

#[test]
fn augment_with_static_claims_static_intro() {
    // Control: `augment static void X()` joins the static intro's chain,
    // flipping the verdict to the multi-fragment face — no
    // `missing_function_body` at the intro `;`.
    assert_rows_at(
        "class A {\n  static void X();\n  augment static void X() {}\n}\n",
        augment_set(),
        &[],
    );
}

// ------------------------------------------------------------------
// extension / extension-type instance members
// ------------------------------------------------------------------

#[test]
fn extension_instance_member_is_exempt() {
    // Upstream exempts instance members of every container kind
    // (:5273-5280); an extension instance `;` member never reports
    // `missing_function_body` — its semantic row is
    // `extension_declares_abstract_member` instead.
    assert_rows_at("extension E on int {\n  void m();\n}\n", augment_set(), &[]);
}

#[test]
fn extension_static_member_reports() {
    // Positive pin: the static extension member is not instance-exempt,
    // so a singleton `;` body still reports at the `;` (1:30 len 1).
    assert_rows_at(
        "extension E on int {\n  static void m();\n}\n",
        augment_set(),
        &[("missing_function_body", 38, 39)],
    );
}

#[test]
fn extension_type_instance_member_is_exempt() {
    // Extension-type instance `;` members route to
    // `extension_type_with_abstract_member` upstream, never
    // `missing_function_body`.
    assert_rows_at(
        "extension type ET(int i) {\n  void m();\n}\n",
        augment_set(),
        &[],
    );
}

#[test]
fn extension_type_static_member_reports() {
    assert_rows_at(
        "extension type ET(int i) {\n  static void m();\n}\n",
        augment_set(),
        &[("missing_function_body", 44, 45)],
    );
}

// ------------------------------------------------------------------
// augment variable induced accessors
// ------------------------------------------------------------------

#[test]
fn augment_variable_claims_getter_intro() {
    // `augment var x = 0;` induces a getter fragment, so `int get x;`
    // stops being a singleton — the multi-fragment arm owns the verdict
    // and the `;` stays silent.
    assert_rows_at("int get x;\naugment var x = 0;\n", augment_set(), &[]);
}

#[test]
fn augment_final_variable_claims_getter_intro() {
    // `augment final x = 0;` still induces the getter (final suppresses
    // only the setter).
    assert_rows_at("int get x;\naugment final x = 0;\n", augment_set(), &[]);
}

#[test]
fn augment_final_variable_does_not_claim_setter() {
    // A `final` variable induces no setter fragment: `set x(v);` remains
    // a singleton incomplete intro and keeps `missing_function_body` at
    // its `;` (1:10 len 1).
    assert_rows_at(
        "void set x(int v);\naugment final x = 0;\n",
        augment_set(),
        &[("missing_function_body", 17, 18)],
    );
}

#[test]
fn augment_variable_claims_setter_intro() {
    // `augment var x = 0;` also induces the setter fragment, so the
    // `set x(v);` intro flips to the multi-fragment face — silent.
    assert_rows_at("void set x(int v);\naugment var x = 0;\n", augment_set(), &[]);
}

#[test]
fn augment_class_member_variable_claims_instance_getter() {
    // Member-arm parity: `augment class C { augment var x = 0; }`
    // induces the instance getter fragment of `C.x`, so `int get x;`
    // inside `class C` stops being a singleton.
    assert_rows_at(
        "class C {\n  int get x;\n}\naugment class C {\n  augment var x = 0;\n}\n",
        augment_set(),
        &[],
    );
}

#[test]
fn augment_class_member_variable_staticness_must_match() {
    // `augment var x` (no `static`) claims the *instance* accessor chain
    // only — the `static int get x;` intro stays a singleton and reports
    // `missing_function_body` at its `;` (offset 28).
    assert_rows_at(
        "class C {\n  static int get x;\n}\naugment class C {\n  augment var x = 0;\n}\n",
        augment_set(),
        &[("missing_function_body", 28, 29)],
    );
}
