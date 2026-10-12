//! M0-gate wave-11 regression pins (the w4 trend long tail): the legacy
//! switch case expression, the `var/final/const`-vs-`late` junk-name split,
//! the primary-constructor `covariant` formal gate, and the missing-class-
//! body junk re-expansion.
//!
//! Every face was re-derived against the 3.13.1 oracle
//! (`dart analyze --format=machine` probes) before pinning; the w4 trend
//! rows for several of these files predate intermediate parser work, so the
//! pins lock the CURRENT traced behavior, not the stale captures.

use super::collector::{scan_ok, setup};
use crate::ParserFeatures;
use dartq_language::{FeatureSet, LanguageVersion};

/// Parses `source` with the latest feature set and returns the sorted
/// diagnostic code multiset.
fn sorted_codes(source: &'static str) -> Vec<String> {
    let mut parser = setup(source);
    parser.parse_unit();
    let mut codes = parser.listener.error_codes();
    codes.sort();
    codes
}

/// 2.19 feature set (patterns off — the legacy statement grammar).
fn legacy_features() -> ParserFeatures {
    ParserFeatures::from_feature_set(FeatureSet::for_language_version(LanguageVersion::new(
        2, 19,
    )))
}

fn assert_codes(source: &'static str, expected: &[&str], label: &str) {
    let got = sorted_codes(source);
    let expected: Vec<String> = expected.iter().map(ToString::to_string).collect();
    assert_eq!(got, expected, "{label}: diagnostic multiset diverged");
}

// —— legacy switch case (patterns off) ————————————————————————————————————

/// Under a 2.19 language version the case head parses as a plain
/// expression — the branch sits in `parseSwitchBlock`
/// (parser_impl.dart:10945-10949), not inside `parsePattern`, so the
/// disabled-patterns face never fires and `case 1:` closes silently.
/// The whole file is diagnostic-free (the oracle agrees, zero rows on
/// the `assigned_variables/data/switch.dart` fixture).
#[test]
fn legacy_switch_case_head_parses_as_expression() {
    let source = "f(int a) {\n  switch (a = 0) {\n    case 1:\n      break;\n    default:\n      break;\n  }\n}\n";
    let (table, text) = scan_ok(source);
    let mut parser = crate::Parser::new(
        text,
        table,
        legacy_features(),
        super::collector::Collector::default(),
    );
    parser.parse_unit();
    assert!(
        parser.listener.errors.is_empty(),
        "legacy switch case must parse clean: {:?}",
        parser.listener.errors
    );
}

// —— junk member names: the var/final/const split —————————————————————————

/// `late ;` — the junk name inserts SILENTLY when no `var`/`final`/`const`
/// modifier precedes it (probed 3.13.1: one row,
/// `missingConstFinalVarOrType` anchored on `late`), and `parseFields`'s
/// type face is the only diagnostic.
#[test]
fn late_semicolon_reports_only_missing_type() {
    assert_codes(
        "late;\n",
        &["missing_const_final_var_or_type"],
        "top-level `late ;`",
    );
    assert_codes(
        "class C {\n  late;\n}\n",
        &["missing_const_final_var_or_type"],
        "instance `late ;`",
    );
}

/// `var ;` — with a `var`/`final`/`const` modifier the inserted-name face
/// IS reported (probed 3.13.1: `missing_identifier` on the `;`), and the
/// modifier suppresses `parseFields`'s type face.
#[test]
fn var_semicolon_reports_missing_identifier() {
    assert_codes("var;\n", &["missing_identifier"], "top-level `var ;`");
    assert_codes(
        "class C {\n  var;\n}\n",
        &["missing_identifier"],
        "instance `var ;`",
    );
}

/// `int ;` — a consumed type with a junk name: silent name insert, and the
/// NoType re-derive surfaces `missingConstFinalVarOrType` on the type
/// (probed 3.13.1: one row anchored on `int`).
#[test]
fn type_semicolon_reports_only_missing_type() {
    assert_codes(
        "int;\n",
        &["missing_const_final_var_or_type"],
        "top-level `int ;`",
    );
    assert_codes(
        "class C {\n  int;\n}\n",
        &["missing_const_final_var_or_type"],
        "instance `int ;`",
    );
}

// —— primary-constructor `covariant` formal —————————————————————————————————

/// `class P(covariant int x) {}` — with the feature on, `covariant` in a
/// primary-constructor formal is only legal alongside `var`
/// (modifier_context.dart:238-246): one
/// `invalid_covariant_modifier_in_primary_constructor` row on the
/// `covariant` token.
#[test]
fn covariant_primary_constructor_formal_reports() {
    assert_codes(
        "class P(covariant int x) {}\n",
        &["invalid_covariant_modifier_in_primary_constructor"],
        "covariant primary formal",
    );
}

// —— missing class body: junk re-expansion from headerStart ————————————————

/// `class C({var int a})` at EOF — the params parse succeeds, but the
/// missing-body recovery restarts from `headerStart`
/// (parser_impl.dart:3098): the `(` ahead of the body `{` reports as
/// skipped junk (`unexpected_token`), `{var int a}` re-forms the class
/// body (the member re-reports `var_and_type` and walks the `;` back to
/// `a`), and the leftover `)` lands as top-level junk
/// (`expected_executable`). The probed oracle face is exactly these four
/// codes — `var_and_type` twice (once in the params parse, once in the
/// junk member).
#[test]
fn missing_class_body_reexpands_primary_params_as_junk() {
    assert_codes(
        "class C({var int a})\n",
        &[
            "expected_executable",
            "expected_token",
            "unexpected_token",
            "var_and_type",
            "var_and_type",
        ],
        "primary params at EOF re-expand as junk",
    );
}
