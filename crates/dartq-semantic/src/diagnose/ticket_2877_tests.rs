//! The #2877 pins: the `unchecked_use_of_nullable_value` message-variant
//! dispatch — messages.yaml's four shared-name rows
//! (`uncheckedInvocationOfNullableValue`,
//! `uncheckedMethodInvocationOfNullableValue`,
//! `uncheckedOperatorInvocationOfNullableValue`,
//! `uncheckedPropertyAccessOfNullableValue`) picked by the access node's
//! own shape (type_property_resolver.dart:128-148): a bare nullable
//! function expression invoked reports the function text, a member
//! invocation the method text, a binary-operator application the operator
//! text, every other access the property text — each with its matching
//! correction.
//!
//! Oracle rows: dart 3.13.7 (the pinned conformance oracle) — the
//! unchecked-nullable census cluster 5 (431 fail rows; the operator and
//! function texts verbatim from the oracle side of the conformance diff).

use super::test_rig::{CORE_PRELUDE, report_ops};

/// The single unchecked row of `source` as `(offset, length, message,
/// correction)` — the offset translated back past [`CORE_PRELUDE`] so it
/// addresses the fixture.
fn one_unchecked(source: &str) -> (u32, u32, String, Option<String>) {
    let shift = CORE_PRELUDE.len() as u32;
    let report = report_ops(source);
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unchecked_use_of_nullable_value")
        .collect();
    assert_eq!(rows.len(), 1, "{source:?} -> {:?}", report.diagnostics);
    let row = rows[0];
    (
        row.range.offset.saturating_sub(shift),
        row.range.length,
        row.message.to_string(),
        row.correction.as_deref().map(str::to_string),
    )
}

/// The byte offset of `needle`'s first occurrence (the anchor locator —
/// fixtures keep every spelling unique).
fn at(source: &str, needle: &str) -> u32 {
    source.find(needle).expect(needle) as u32
}

// —— the property arm (regression) ———————————————————————————————————

/// A getter read through a nullable receiver keeps the property text with
/// the access correction (messages.yaml
/// `uncheckedPropertyAccessOfNullableValue`).
#[test]
fn property_access_reports_the_property_variant() {
    let src = "void f(int? x) {\n  x.isEven;\n}";
    let (offset, length, message, correction) = one_unchecked(src);
    assert_eq!(offset, at(src, "isEven"));
    assert_eq!(length, 6);
    assert_eq!(
        message,
        "The property 'isEven' can't be unconditionally accessed because the receiver can be 'null'."
    );
    assert_eq!(
        correction,
        Some("Try making the access conditional (using '?.') or adding a null check to the target ('!').".to_string())
    );
}

// —— the function arm —————————————————————————————————————————————————

/// A nullable function-typed expression invoked bare reports the function
/// text with the null-check correction (messages.yaml
/// `uncheckedInvocationOfNullableValue` — the FunctionExpressionInvocation
/// arm; the census's `Never?` tear-off rows).
#[test]
fn function_invocation_reports_the_function_variant() {
    let src = "void f(void Function()? cb) {\n  cb();\n}";
    let (offset, length, message, correction) = one_unchecked(src);
    assert_eq!(offset, at(src, "cb();"));
    assert_eq!(length, 2);
    assert_eq!(
        message,
        "The function can't be unconditionally invoked because it can be 'null'."
    );
    assert_eq!(
        correction,
        Some("Try adding a null check ('!').".to_string())
    );
}

// —— the method arm ———————————————————————————————————————————————————

/// A method invoked through a nullable receiver reports the method text
/// with the call correction (messages.yaml
/// `uncheckedMethodInvocationOfNullableValue`).
#[test]
fn method_invocation_reports_the_method_variant() {
    let src = "class C {\n  int foo() => 0;\n}\nvoid f(C? c) {\n  c.foo();\n}";
    let (offset, length, message, correction) = one_unchecked(src);
    assert_eq!(offset, at(src, "c.foo") + 2);
    assert_eq!(length, 3);
    assert_eq!(
        message,
        "The method 'foo' can't be unconditionally invoked because the receiver can be 'null'."
    );
    assert_eq!(
        correction,
        Some("Try making the call conditional (using '?.') or adding a null check to the target ('!').".to_string())
    );
}

// —— the operator arm —————————————————————————————————————————————————

/// A binary operator applied to a nullable operand reports the operator
/// text with the null-check correction (messages.yaml
/// `uncheckedOperatorInvocationOfNullableValue` — the BinaryExpression
/// arm; the census's `nullable_operator_call_error_test` rows).
#[test]
fn binary_operator_reports_the_operator_variant() {
    let src = "void f(int? x) {\n  x + 1;\n}";
    let (offset, length, message, correction) = one_unchecked(src);
    assert_eq!(offset, at(src, "+ 1"));
    assert_eq!(length, 1);
    assert_eq!(
        message,
        "The operator '+' can't be unconditionally invoked because the receiver can be 'null'."
    );
    assert_eq!(
        correction,
        Some("Try adding a null check to the target ('!').".to_string())
    );
}
