//! #3246 pins: the `unnecessary_final` rule's INFO faces — local variable
//! declarations, the for-each pattern form, and pattern-declared variables —
//! over the pinned `dart analyze` 3.13.7 corpus shapes
//! (`pkg/linter/test/rules/unnecessary_final_test.dart`).
//!
//! The variant follows the written type
//! (`unnecessary_final.dart:119` `getDiagnosticCode`): a type between the
//! keyword and the name pins "Remove the 'final'.", its absence pins
//! "Replace 'final' with 'var'.". Two pattern forms fix the variant
//! explicitly — `ForEachPartsWithPattern` reports the without-type variant
//! unconditionally (:103-106) and a declared variable pattern reports the
//! with-type variant (`matchedValueType` is non-null on every resolved run).
//! The parameter face is dead under the shipped `primary_constructors`
//! feature (:73-75) and re-arms pre-3.13, matching the upstream
//! `// @dart = 3.10` tests.

use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

const PROBLEM: &str = "Local variables should not be marked as 'final'.";
const WITHOUT_TYPE: &str = "Replace 'final' with 'var'.";
const WITH_TYPE: &str = "Remove the 'final'.";

/// The UTF-16 offset of the `nth` occurrence of `needle` in `f`.
fn at(f: &Fixture, needle: &str, nth: usize) -> u32 {
    utf16_at(f, tok(&f.source, &f.tokens, needle, nth))
}

/// One row for `source`, anchored at the `nth` `final` keyword, carrying the
/// variant's correction message.
fn assert_row(source: &str, nth: usize, correction: &str) {
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("unnecessary_final"));
    assert_eq!(lints.len(), 1, "{source:?}: {lints:?}");
    assert_eq!(lints[0].message, PROBLEM, "{source:?}");
    assert_eq!(
        lints[0].correction.as_deref(),
        Some(correction),
        "{source:?}"
    );
    assert_eq!(lints[0].range.start, at(&f, "final", nth), "{source:?}");
}

/// No rows for `source`.
fn assert_silent(source: &str) {
    let f = Fixture::parse(source);
    assert!(
        f.run(&one_rule("unnecessary_final")).is_empty(),
        "{source:?}"
    );
}

/// The pre-3.13 posture that arms the parameter face (the
/// `primary_constructors` gate, `unnecessary_final.dart:73-75`).
fn assert_armed_row(source: &str, correction: &str) {
    let mut f = Fixture::parse(source);
    f.features.primary_constructors = false;
    let lints = f.run(&one_rule("unnecessary_final"));
    assert_eq!(lints.len(), 1, "{source:?}: {lints:?}");
    assert_eq!(
        lints[0].correction.as_deref(),
        Some(correction),
        "{source:?}"
    );
    assert_eq!(lints[0].range.start, at(&f, "final", 0), "{source:?}");
}

// —— local declarations (`test_localVariable_*`) —————————————————————

#[test]
fn uf3246_untyped_local_reports_without_type() {
    assert_row("void f() { final x = 1; }", 0, WITHOUT_TYPE);
}

#[test]
fn uf3246_typed_local_reports_with_type() {
    assert_row("void f() { final int x = 1; }", 0, WITH_TYPE);
}

/// `test_localVariable_final_wildcard`: the wildcard binder reports like a
/// named one (the rule has no wildcard gate; the upstream pre-wildcards
/// variant `test_localVariable_final_wildcard_preWildcards` fires the same).
#[test]
fn uf3246_wildcard_local_reports() {
    assert_row("void f() { final _ = ''; }", 0, WITHOUT_TYPE);
}

// —— for-each loop variable (`test_forEachLoopVariable_*`) ———————————

/// The typed declaration form keys the with-type variant
/// (`ForEachPartsWithDeclaration` -> `getDiagnosticCode(loopVariable.type)`).
#[test]
fn uf3246_typed_for_each_variable_reports_with_type() {
    assert_row("void f() { for (final int x in [1]) {} }", 0, WITH_TYPE);
}

/// The pattern form takes the without-type variant unconditionally
/// (`unnecessary_final.dart:103-106`)
/// (`test_recordPattern_destructured_forEach`).
#[test]
fn uf3246_for_each_pattern_form_reports_without_type() {
    assert_row("void f() { for (final (a, b) in [(1, 2)]) {} }", 0, WITHOUT_TYPE);
}

#[test]
fn uf3246_var_for_each_variable_stays_silent() {
    assert_silent("void f() { for (var x in [1]) {} }");
}

// —— pattern-declared variables (`test_*Pattern_*`) ——————————————————

/// `test_listPattern_destructured`.
#[test]
fn uf3246_list_pattern_destructuring_reports() {
    assert_row("void f() { final [a] = [1]; }", 0, WITH_TYPE);
}

/// `test_mapPattern_destructured`.
#[test]
fn uf3246_map_pattern_destructuring_reports() {
    assert_row("void f() { final {'a': a} = {'a': 1}; }", 0, WITH_TYPE);
}

/// `test_objectPattern_destructured`.
#[test]
fn uf3246_object_pattern_destructuring_reports() {
    assert_row(
        "class C { int c; C(this.c); } void f() { final C(:c) = C(1); }",
        0,
        WITH_TYPE,
    );
}

#[test]
fn uf3246_var_pattern_destructuring_stays_silent() {
    assert_silent("void f() { var (a, b) = (1, 2); }");
}

#[test]
fn uf3246_var_object_pattern_destructuring_stays_silent() {
    assert_silent("class C { int c; C(this.c); } void f() { var C(:c) = C(1); }");
}

/// `test_recordPattern_ifCase`: the sibling `int y` (no keyword) must not
/// report — the official `thisOrAncestorOfType` walk finds no
/// `PatternVariableDeclaration` inside an if-case.
#[test]
fn uf3246_record_pattern_if_case_reports_one_row() {
    assert_row("void f(Object o) { if (o case (final int x, int y)) {} }", 0, WITH_TYPE);
}

#[test]
fn uf3246_record_pattern_if_case_without_keywords_stays_silent() {
    assert_silent("void f(Object o) { if (o case (int x, int y)) {} }");
}

/// `test_mapPattern_ifCase`: the `var y` sibling stays silent.
#[test]
fn uf3246_map_pattern_if_case_reports_one_row() {
    assert_row(
        "void f(Object o) { if (o case {'x': final x, 'y': var y}) {} }",
        0,
        WITH_TYPE,
    );
}

/// `test_objectPattern_ifCase`: the `var y` sibling stays silent.
#[test]
fn uf3246_object_pattern_if_case_reports_one_row() {
    assert_row(
        "class C { int c; int d; C(this.c, this.d); } void f(Object o) { if (o case C(c: final x, d: var y)) {} }",
        0,
        WITH_TYPE,
    );
}

/// `test_objectPattern_switch`: a declared variable behind a relational
/// and-bound reports at its own keyword.
#[test]
fn uf3246_object_pattern_switch_and_bound_reports() {
    assert_row(
        "class A { int a; A(this.a); } void f() { switch (A(1)) { case A(a: >0 && final b): } }",
        0,
        WITH_TYPE,
    );
}

/// `test_recordPattern_switch`: two declared variables each anchor at their
/// own keyword — distinct rows, no dedup.
#[test]
fn uf3246_record_pattern_switch_reports_two_rows() {
    let source = "void f() { switch ((1, 2)) { case (final a, final b): } }";
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("unnecessary_final"));
    assert_eq!(lints.len(), 2, "{source:?}: {lints:?}");
    assert_eq!(lints[0].range.start, at(&f, "final", 0), "{source:?}");
    assert_eq!(lints[1].range.start, at(&f, "final", 1), "{source:?}");
    for lint in &lints {
        assert_eq!(lint.correction.as_deref(), Some(WITH_TYPE), "{source:?}");
    }
}

// —— parameters (`test_parameter_*`) ——————————————————————————————————

/// `test_parameter_functionExpression_final_prePrimaryConstructors`: the
/// function-expression parameter arms only pre-3.13.
#[test]
fn uf3246_function_expression_parameter_reports_pre_3_13() {
    assert_armed_row("void f() { (final c) => c.length; }", WITHOUT_TYPE);
}

/// `test_parameter_declaring`: under the shipped 3.13 feature the declaring
/// parameter is skipped (the parser rejects the modifier; the oracle's row
/// for this shape is the analyzer's static warning, not this rule).
#[test]
fn uf3246_declaring_parameter_stays_silent_at_3_13() {
    assert_silent("class C(final int x);");
}
