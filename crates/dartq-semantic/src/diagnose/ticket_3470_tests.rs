//! The #3470 `use_of_void_result` span pins: the report anchors the
//! operand's USE-SITE expression, not always the operand row's own token
//! (upstream `checkForUseOfVoidResult`, upstream Dart SDK source
//! pkg/analyzer/lib/src/generated/error_detection_helpers.dart:245-258 —
//! `MethodInvocation` operands anchor `methodName`; every other shape —
//! `FunctionExpressionInvocation`, `PropertyAccess`, `NamedExpression`,
//! `RecordLiteralNamedField` — anchors the whole expression). A send
//! through a `void Function()` formal or a function-typed member is a
//! `FunctionExpressionInvocation` upstream, so the anchor widens to the
//! receiver-start through the closing `)`; a send resolving to a
//! declared method or function keeps the callee's name token.

use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops, rows};

/// The `use_of_void_result` spans of `fixture` in fixture-relative
/// coordinates (prelude stripped), sorted.
fn void_spans(fixture: &str) -> Vec<(usize, usize)> {
    let report = report_ops(fixture);
    let shift = CORE_PRELUDE.len();
    let mut spans: Vec<(usize, usize)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "use_of_void_result")
        .map(|d| {
            (
                d.range.offset as usize - shift,
                d.range.length as usize,
            )
        })
        .collect();
    spans.sort_unstable();
    spans
}

/// The `(offset, length)` of `needle` inside `fixture`'s LAST region —
/// `find` locates the first spelling; where the declaration shares it
/// (`void f()` beside the operand `f()`), `rfind` reaches the use.
fn span_of(fixture: &str, needle: &str) -> (usize, usize) {
    (fixture.rfind(needle).unwrap(), needle.len())
}

/// `-f()` on a `void Function()` formal: the operand is a
/// `FunctionExpressionInvocation` (`f` is a formal, never a declared
/// method), so the oracle anchors the whole `f()` — not the `f` token.
#[test]
fn ticket_3470_prefix_minus_anchors_whole_invocation() {
    let fixture = "void test(void f()) {\n  -f();\n}";
    let at = fixture.rfind("f()").unwrap();
    assert_eq!(void_spans(fixture), vec![(at, 3)]);
}

/// `f() + 1` on a function-typed formal: the binary LEFT operand's send
/// anchors the whole `f()` (a declared-method send would anchor `f`).
#[test]
fn ticket_3470_binary_left_anchors_whole_invocation() {
    let fixture = "void test(void f()) {\n  f() + 1;\n}";
    let expected = span_of(fixture, "f() + 1");
    assert_eq!(void_spans(fixture), vec![(expected.0, 3)]);
}

/// `x.f()` where `f` is a function-typed FIELD: a
/// `FunctionExpressionInvocation` through a `PropertyAccess` function
/// part, so the oracle anchors receiver-start through `)` — `x.f()`
/// whole, not the `f` selector.
#[test]
fn ticket_3470_member_send_through_function_field() {
    let fixture = "class C { void Function() f = () {}; }\nvoid test(C x) {\n  -x.f();\n}";
    assert_eq!(void_spans(fixture), vec![span_of(fixture, "x.f()")]);
}

/// `x.m()` where `m` is a declared METHOD keeps the `methodName` anchor:
/// `m` alone, never `x.m()`.
#[test]
fn ticket_3470_declared_method_send_keeps_name_token() {
    let fixture = "class C { void m() {} }\nvoid test(C x) {\n  -x.m();\n}";
    let sel = fixture.rfind("x.m()").unwrap() + 2;
    assert_eq!(void_spans(fixture), vec![(sel, 1)]);
}

/// `a = f()` where `f` is a top-level `void f() {}` keeps the
/// `methodName` anchor (`f` token, length 1) — the resolver's
/// `_checkForUseOfVoidResult(right)` takes the same `MethodInvocation`
/// arm.
#[test]
fn ticket_3470_assignment_rhs_method_keeps_name_token() {
    let fixture = "void f() {}\nvoid test() {\n  dynamic a;\n  a = f();\n}";
    let name = fixture.rfind("f();").unwrap();
    assert_eq!(void_spans(fixture), vec![(name, 1)]);
}

/// `(one: x,)` anchors the `RecordLiteralNamedField` node — `one: x`
/// whole, never the bare `x` value.
#[test]
fn ticket_3470_record_named_field_anchors_entry() {
    let fixture = "void f(void x) {\n  (one: x,);\n}";
    assert_eq!(void_spans(fixture), vec![span_of(fixture, "one: x")]);
}

/// A `void`-typed named ARGUMENT is the same named-entry anchor:
/// `g(one: x)` reports `one: x`, not `x`.
#[test]
fn ticket_3470_named_argument_anchors_entry() {
    let fixture = "void f(void x) {\n  g(one: x);\n}\nvoid g({required dynamic one}) {}";
    assert_eq!(void_spans(fixture), vec![span_of(fixture, "one: x")]);
}

/// `await f()` on a `void Function()` formal anchors the whole send —
/// the operand expression upstream's `at(expression)` reads.
#[test]
fn ticket_3470_await_operand_anchors_whole_invocation() {
    let fixture = "void test(void f()) async {\n  await f();\n}";
    let at = fixture.rfind("f()").unwrap();
    assert_eq!(void_spans(fixture), vec![(at, 3)]);
}

/// A property access consumed as an operand anchors the whole
/// `receiver.member` chain — `ET1("42").id`-shaped operands report from
/// the receiver's start, not the selector's.
#[test]
fn ticket_3470_property_access_operand_anchors_chain() {
    let fixture = "class E { void get id => throw 0; }\nvoid test(E x) {\n  -x.id;\n}";
    let spans = void_spans(fixture);
    assert_eq!(spans, vec![span_of(fixture, "x.id")], "{:?}", rows(&report_ops(fixture)));
}

/// A `!`-prefix operand on a `void Function()` formal anchors the send —
/// the negation seam reads the same use-site span as every other operand
/// face.
#[test]
fn ticket_3470_negation_operand_anchors_whole_invocation() {
    let fixture = "void test(void f()) {\n  !f();\n}";
    let at = fixture.rfind("f()").unwrap();
    assert_eq!(void_spans(fixture), vec![(at, 3)]);
}

/// The plain-identifier control: `x` under `-` keeps `x`'s own span —
/// no widening applies to a bare operand.
#[test]
fn ticket_3470_identifier_operand_keeps_own_span() {
    let fixture = "void f(void x) {\n  -x;\n}";
    let spans = void_spans(fixture);
    assert_eq!(spans.len(), 1, "{:?}", rows(&report_ops(fixture)));
    assert_eq!(spans[0], (fixture.rfind("x;").unwrap(), 1));
}

/// `if (f())` on a `void Function()` formal: the condition operand's
/// send anchors `f()` whole — the condition seam reads the same
/// use-site span.
#[test]
fn ticket_3470_condition_operand_anchors_whole_invocation() {
    let fixture = "void test(void f()) {\n  if (f()) {}\n}";
    let at = fixture.rfind("f()").unwrap();
    assert_eq!(void_spans(fixture), vec![(at, 3)]);
}

/// `g(f())` on a `void Function()` formal: the argument's send anchors
/// `f()` whole — `operand_static_ty` reads the `void` return, the anchor
/// widens to the send expression.
#[test]
fn ticket_3470_argument_send_anchors_whole_invocation() {
    let fixture = "void test(void f()) {\n  g(f());\n}\nvoid g(dynamic x) {}";
    let spans = void_spans(fixture);
    assert_eq!(spans.len(), 1, "{:?}", rows(&report_ops(fixture)));
    // `f()` inside `g(f())` — `rfind` lands inside the argument.
    let at = fixture.rfind("f()").unwrap();
    assert_eq!(spans[0], (at, 3));
}
