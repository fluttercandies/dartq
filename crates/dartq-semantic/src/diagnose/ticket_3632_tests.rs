//! The #3632 pins: the statement-form operand faces ground the OPERAND
//! EXPRESSION's own static type, never a single-read window attribution —
//! the compound iterable (a member chain the receiver's row cannot
//! answer), the pattern-header foreach (the same verifier ask
//! `for_resolver.dart:71-74` fires for `ForEachPartsWithPattern`), the
//! `void` iterable/operand (`isPotentiallyNullable`'s `VoidType` arm —
//! the read rows the shared join drops join the operand-face side
//! table), the prefix `-`/`~` void operand
//! (`uncheckedMethodInvocationOfNullableValue` with the `unary-`/`~`
//! member spelling, `prefix_expression_resolver.dart:152-227` through
//! `type_property_resolver.dart:100-181`'s parent-node fallback), and
//! the const-initializer condition operand (`bool_expression_verifier.dart:51-77`
//! — the operand's flow-aware type decides, whatever the parse-time
//! face spelled).
//!
//! Oracle provenance: `why_not_promoted/for_in_loop_type_not_iterable_nullability_error_test.dart`
//! (the `c.bad` iterable row at the whole member-access span),
//! `patterns/for_in_nullable_error_test.dart` (`for (var (y) in x)`),
//! `void/void_type_usage_test.dart` (`param_for_in`/`param_negation`/`async_use_in_yield_star`,
//! the UNCHECKED twin beside every `USE_OF_VOID_RESULT`), and
//! `string/multiline_newline_test.dart` (`const c4 = c1 ? 1 : 2`).

use super::test_rig::{CORE_PRELUDE, report_ops, rows};
use super::*;

/// The absolute fixture offset of `needle` (the prelude the rig prepends
/// included).
fn at(fixture: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + fixture.find(needle).unwrap()
}

/// The `unchecked_use_of_nullable_value` rows as `offset:len message`.
fn unchecked(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unchecked_use_of_nullable_value")
        .map(|d| {
            format!(
                "{} @{}:{} {}",
                d.code.name, d.range.offset, d.range.length, d.message
            )
        })
        .collect()
}

/// Exactly one unchecked row at `offset:len` carrying `message`.
#[track_caller]
fn assert_unchecked(report: &FileReport, offset: usize, len: u32, message: &str) {
    assert_eq!(
        unchecked(report),
        vec![format!(
            "unchecked_use_of_nullable_value @{}:{} {}",
            offset, len, message
        )],
    );
}

/// The compound member-chain iterable reports at the whole member-access
/// span (the official `.at(iterable)` anchor — `forStatement`'s `c.bad`
/// row).
#[test]
fn for_in_compound_member_iterable_reports_at_the_chain() {
    let fixture = "class C1 {\n  List? bad;\n}\nf(C1 c) {\n  for (var x in c.bad) {}\n}\n";
    let report = report_ops(fixture);
    let chain = at(fixture, "c.bad");
    assert_unchecked(
        &report,
        chain,
        5,
        "A nullable expression can't be used as an iterator in a for-in loop.",
    );
}

/// The pattern-header foreach asks the same verifier face
/// (`_analyzePatternForIn`'s `.at(iterable)` — `for (var (y) in x)`).
#[test]
fn for_in_pattern_header_reports_at_the_iterable() {
    let fixture = "f(Iterable? x) {\n  for (var (y) in x) {}\n}\n";
    let report = report_ops(fixture);
    let x = at(fixture, "x) {}");
    assert_unchecked(
        &report,
        x,
        1,
        "A nullable expression can't be used as an iterator in a for-in loop.",
    );
}

/// The `void` iterable (`param_for_in`): the read the shared join drops
/// grounds through the operand-face side table and reports
/// `isPotentiallyNullable(void)`.
#[test]
fn for_in_void_iterable_reports_at_the_iterable() {
    let fixture = "f(void x) {\n  for (var v in x) {}\n}\n";
    let report = report_ops(fixture);
    let x = at(fixture, "x) {}");
    assert_unchecked(
        &report,
        x,
        1,
        "A nullable expression can't be used as an iterator in a for-in loop.",
    );
}

/// The parenthesized iterable keeps the group's inner type and the whole
/// group's span (`paren_for_in`).
#[test]
fn for_in_paren_iterable_reports_at_the_group() {
    let fixture = "f(List? x) {\n  for (var v in (x)) {}\n}\n";
    let report = report_ops(fixture);
    let group = at(fixture, "(x)");
    assert_unchecked(
        &report,
        group,
        3,
        "A nullable expression can't be used as an iterator in a for-in loop.",
    );
}

/// The prefix `-` over a `void` operand resolves as the `unary-` METHOD
/// invocation on a potentially-nullable receiver — the method face at the
/// operator token (`param_negation`; `type_property_resolver.dart`'s
/// member-call arm renders `The method 'unary-' …`).
#[test]
fn void_prefix_negation_reports_at_the_operator() {
    let fixture = "f(void x) {\n  -x;\n}\n";
    let report = report_ops(fixture);
    let op = at(fixture, "-x;");
    assert_unchecked(
        &report,
        op,
        1,
        "The method 'unary-' can't be unconditionally invoked because the receiver can be 'null'.",
    );
}

/// A binary `-` never takes the prefix face: `y - x`'s minus follows an
/// operand-ending token, and the void right operand is the operator
/// lanes' row, not the unary method face.
#[test]
fn binary_minus_takes_no_prefix_face() {
    let fixture = "f(void x, int y) {\n  y - x;\n}\n";
    let report = report_ops(fixture);
    assert!(
        !rows(&report)
            .iter()
            .any(|r| r.contains("unary-")),
        "binary minus took the prefix face: {:#?}",
        rows(&report)
    );
}

/// `yield* s` over a nullable operand reports the yield-each face at the
/// operand's own span (`async_use_in_yield_star`'s nullable twin).
#[test]
fn yield_each_nullable_operand_reports() {
    let fixture = "f(List? s) async* {\n  yield* s;\n}\n";
    let report = report_ops(fixture);
    let s = at(fixture, "s;");
    assert_unchecked(
        &report,
        s,
        1,
        "A nullable expression can't be used in a yield-each statement.",
    );
}

/// `yield* x` over a `void` operand — the dropped read grounds through
/// the side table (`async_use_in_yield_star`).
#[test]
fn yield_each_void_operand_reports() {
    let fixture = "f(void x) async* {\n  yield* x;\n}\n";
    let report = report_ops(fixture);
    let x = at(fixture, "x;");
    assert_unchecked(
        &report,
        x,
        1,
        "A nullable expression can't be used in a yield-each statement.",
    );
}

/// The const-initializer condition operand (`const c4 = c1 ? 1 : 2;`
/// over the inferred `bool?` const): the flow's condition-type row
/// re-decides the staged candidate at teardown
/// (`multiline_newline_test`'s `c4` rows). GAP: the const-eval
/// conditional fold (`0 == 0 ? true : null` const-folds to `true`)
/// assigns `c1` static type `bool`, never the ternary's static `bool?`
/// — the inferred-const staging arm (#3632, `classify_condition_marked`)
/// grounds only an `Unknown` parse face (proven by the `var c1` twin,
/// which reports), and the `bool`-carrying const row cannot re-decide
/// without the const-eval conditional's LUB keeping `?`. Owned by the
/// const-eval conditional-type face, not this walk.
#[test]
#[ignore = "const-eval conditional fold drops `?` — `c1` reads `bool`, not `bool?`; the inferred-const arm grounds `Unknown` faces only"]
fn const_initializer_condition_operand_reports() {
    let fixture = "f() {\n  const c1 = 0 == 0 ? true : null;\n  const c4 = c1 ? 1 : 2;\n}\n";
    let report = report_ops(fixture);
    let c1 = at(fixture, "c1 ? 1");
    assert_unchecked(
        &report,
        c1,
        2,
        "A nullable expression can't be used as a condition.",
    );
}

/// The `await for` form asks the same face (`async_use_in_await_for`).
#[test]
fn await_for_void_iterable_reports() {
    let fixture = "f(void x) async {\n  await for (var i in x) {}\n}\n";
    let report = report_ops(fixture);
    let x = at(fixture, "x) {}");
    assert_unchecked(
        &report,
        x,
        1,
        "A nullable expression can't be used as an iterator in a for-in loop.",
    );
}

