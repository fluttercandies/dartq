use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

/// The #2927 census pins: the three under-report faces' oracle
/// shapes through the production bridge with the semantic face
/// mounted (the census-case sources read verbatim; every pin cites
/// its `conformance/expected.tsv` row so the conformance diff is the
/// same verdict the pin asserts).
///
/// `no_dynamic_casts` — the implicit-`dynamic` reads through
/// `static_type_faces::expr_type` (identifier and bare-send runs).

/// `lints/no_dynamic_casts/NoDynamicCastsTest/test_return#0` —
/// `INFO|no_dynamic_casts|lib/test.dart|3:10|1` — a `dynamic` return
/// value against the written `int` return reports at the operand.
#[test]
fn ndc_return_reports_the_dynamic_operand() {
    let f = Fixture::parse("int f(dynamic a) {\n  return a;\n}\n");
    let lints = f.run_semantic(&one_rule("no_dynamic_casts"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Implicit cast from 'dynamic'.");
    let a = tok(&f.source, &f.tokens, "a", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, a));
    assert_eq!(lints[0].range.end - lints[0].range.start, 1);
}

/// `lints/no_dynamic_casts/NoDynamicCastsTest/test_argument#0` —
/// `INFO|no_dynamic_casts|lib/test.dart|4:5|1` — a `dynamic` positional
/// against the `int` formal reports.
#[test]
fn ndc_positional_argument_reports() {
    let f = Fixture::parse("void f(int x) {}\nvoid g(dynamic a) {\n  f(a);\n}\n");
    let lints = f.run_semantic(&one_rule("no_dynamic_casts"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Implicit cast from 'dynamic'.");
    let a = tok(&f.source, &f.tokens, "a", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, a));
}

/// `lints/no_dynamic_casts/NoDynamicCastsTest/test_namedArgument#0` —
/// `INFO|no_dynamic_casts|lib/test.dart|4:8|1` — a `dynamic` named
/// argument against the named `int` formal reports.
#[test]
fn ndc_named_argument_reports() {
    let f = Fixture::parse("void f({required int x}) {}\nvoid g(dynamic a) {\n  f(x: a);\n}\n");
    let lints = f.run_semantic(&one_rule("no_dynamic_casts"));
    assert_eq!(lints.len(), 1, "{lints:?}");
}

/// `lints/no_dynamic_casts/NoDynamicCastsTest/test_expressionFunctionBody#0`
/// — `INFO|no_dynamic_casts|lib/test.dart|2:21|1` — the arrow body
/// face reports at the operand.
#[test]
fn ndc_expression_body_reports() {
    let f = Fixture::parse("int f(dynamic a) => a;\n");
    let lints = f.run_semantic(&one_rule("no_dynamic_casts"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let a = tok(&f.source, &f.tokens, "a", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, a));
}

/// `lints/no_dynamic_casts/NoDynamicCastsTest/test_condition_ifStatement#0`
/// — `INFO|no_dynamic_casts|lib/test.dart|3:7|1` — the `bool`-target
/// condition face reports.
#[test]
fn ndc_if_condition_reports() {
    let f = Fixture::parse("void f(dynamic a) {\n  if (a) {}\n}\n");
    let lints = f.run_semantic(&one_rule("no_dynamic_casts"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let a = tok(&f.source, &f.tokens, "a", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, a));
}

/// `lints/no_dynamic_casts/NoDynamicCastsTest/test_explicitCast_ok#0` —
/// the official bail: `x = a as int` never reports (the explicit-cast
/// arm).
#[test]
fn ndc_explicit_cast_passes() {
    let f = Fixture::parse("void f(dynamic a) {\n  var x = a as int;\n  print(x);\n}\n");
    assert!(
        f.run_semantic(&one_rule("no_dynamic_casts")).is_empty(),
        "{:?}",
        f.run_semantic(&one_rule("no_dynamic_casts"))
    );
}

/// `avoid_dynamic_calls` — the report-span math the census diff
/// flagged: the operand-run anchors, not the container row's own
/// tiling.

/// `lints/avoid_dynamic_calls/AvoidDynamicCallsTest/test_dynamicBinaryExpression_plus#0`
/// — `INFO|avoid_dynamic_calls|lib/test.dart|3:3|1` — `a + 1` reports
/// at `a` alone (the operator's left-operand run, not the whole
/// binary).
#[test]
fn adc_binary_reports_at_the_left_operand() {
    let f = Fixture::parse("void f(dynamic a) {\n  a + 1;\n}\n");
    let lints = f.run_semantic(&one_rule("avoid_dynamic_calls"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Method invocation or property access on a 'dynamic' target."
    );
    let a = tok(&f.source, &f.tokens, "a", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, a));
    assert_eq!(lints[0].range.end - lints[0].range.start, 1);
}

/// `lints/avoid_dynamic_calls/AvoidDynamicCallsTest/test_dynamicUnaryMinusOperator#0`
/// — `INFO|avoid_dynamic_calls|lib/test.dart|3:4|1` — `-a` reports at
/// `a` (the operand, not the operator).
#[test]
fn adc_unary_minus_reports_at_the_operand() {
    let f = Fixture::parse("void f(dynamic a) {\n  -a;\n}\n");
    let lints = f.run_semantic(&one_rule("avoid_dynamic_calls"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let a = tok(&f.source, &f.tokens, "a", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, a));
    assert_eq!(lints[0].range.end - lints[0].range.start, 1);
}

/// `lints/avoid_dynamic_calls/AvoidDynamicCallsTest/test_dynamicIncrementPostfixOperator#0`
/// — `INFO|avoid_dynamic_calls|lib/test.dart|3:3|3` — `a++` reports
/// over the whole compound (the official `compoundAssignment` shape).
#[test]
fn adc_postfix_increment_reports_the_compound() {
    let f = Fixture::parse("void f(dynamic a) {\n  a++;\n}\n");
    let lints = f.run_semantic(&one_rule("avoid_dynamic_calls"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let a = tok(&f.source, &f.tokens, "a", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, a));
    assert_eq!(lints[0].range.end - lints[0].range.start, 3);
}

/// `lints/avoid_dynamic_calls/AvoidDynamicCallsTest/test_dynamicImplicitCall#0`
/// — `INFO|avoid_dynamic_calls|lib/test.dart|3:3|1` — `a()` reports at
/// `a` (the `.call`-implicit send's target run).
#[test]
fn adc_implicit_call_reports_at_the_target() {
    let f = Fixture::parse("void f(dynamic a) {\n  a();\n}\n");
    let lints = f.run_semantic(&one_rule("avoid_dynamic_calls"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let a = tok(&f.source, &f.tokens, "a", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, a));
    assert_eq!(lints[0].range.end - lints[0].range.start, 1);
}

/// `lints/avoid_dynamic_calls/AvoidDynamicCallsTest/test_prefixedIdentifier_dynamicMethodCall#0`'s
/// negative twin — `INFO` rows only for a `dynamic` target: a typed
/// receiver's member read stays silent (`doc_good_0`'s
/// `void f(String s) { s.length; }`, no row).
#[test]
fn adc_typed_receiver_stays_silent() {
    let f = Fixture::parse("void f(String s) {\n  s.length;\n}\n");
    assert!(
        f.run_semantic(&one_rule("avoid_dynamic_calls")).is_empty(),
        "{:?}",
        f.run_semantic(&one_rule("avoid_dynamic_calls"))
    );
}

/// `lints/avoid_dynamic_calls/AvoidDynamicCallsTest/test_prefixedIdentifier_dynamicMethodCall_asDynamic#0`
/// — no row — the explicit-cast bail (:115): `(a as dynamic).foo()`
/// resolves through the cast, so neither the member face nor the
/// bare-invocation face reports.
#[test]
fn adc_explicit_cast_receiver_stays_silent() {
    let f = Fixture::parse("void f(Object? a) {\n  (a as dynamic).foo();\n}\n");
    assert!(
        f.run_semantic(&one_rule("avoid_dynamic_calls")).is_empty(),
        "{:?}",
        f.run_semantic(&one_rule("avoid_dynamic_calls"))
    );
}

/// `unnecessary_parenthesis` — the repaired faces' oracle rows (the
/// helper set `inner_has_kind`/`binary_child_operator`/`spread_token_-
/// before` drives the record-literal, return, and assignment parents).

/// `lints/unnecessary_parenthesis/UnnecessaryParenthesisTest/test_intLiteralInside_recordLiteral#0`
/// — `INFO|unnecessary_parenthesis|lib/test.dart|3:15|3` — `(2)` inside
/// a record literal reports at the parens.
#[test]
fn up_record_literal_inner_parens_report() {
    let f = Fixture::parse("void f() {\n  var x = (1, (2));\n  print(x);\n}\n");
    let lints = f.run(&one_rule("unnecessary_parenthesis"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Unnecessary use of parentheses.");
    let open = tok(&f.source, &f.tokens, "(", 2);
    assert_eq!(lints[0].range.start, utf16_at(&f, open));
    assert_eq!(lints[0].range.end - lints[0].range.start, 3);
}

/// `lints/unnecessary_parenthesis/UnnecessaryParenthesisTest/test_binaryExpressionInside_returnExpression#0`
/// — `INFO|unnecessary_parenthesis|lib/test.dart|3:10|7` — `(1 > 1)`
/// under a return reports.
#[test]
fn up_binary_inside_return_reports() {
    let f = Fixture::parse("bool f() {\n  return (1 > 1);\n}\n");
    let lints = f.run(&one_rule("unnecessary_parenthesis"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let open = tok(&f.source, &f.tokens, "(", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, open));
    assert_eq!(lints[0].range.end - lints[0].range.start, 7);
}

/// `lints/unnecessary_parenthesis/UnnecessaryParenthesisTest/test_functionExpressionInside_assignment#0`
/// — `INFO|unnecessary_parenthesis|lib/test.dart|2:9|12` — a
/// function-expression parenthetical under an assignment reports.
#[test]
fn up_function_expression_inside_assignment_reports() {
    let f = Fixture::parse("void g() {\n  var f = (() => null);\n  print(f);\n}\n");
    let lints = f.run(&one_rule("unnecessary_parenthesis"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let open = tok(&f.source, &f.tokens, "(", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, open));
    assert_eq!(lints[0].range.end - lints[0].range.start, 12);
}
