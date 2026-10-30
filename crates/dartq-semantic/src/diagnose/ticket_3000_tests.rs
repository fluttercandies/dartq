//! The #3000 pins (UNDEF): the `undefined_operator` receiver-face verdicts
//! the non-`Named` arm of `check_operator_expr` and the compound/prefix/
//! postfix twin `adjacent_operator_defined_in` own — the dominant
//! over-emission family in the parity sweep (the previous else-arm fired
//! `undefined_operator` on EVERY non-`Ty::Named` face, including
//! `dynamic`, `Null`, `void`, `Never`, and shape-extension receivers).
//!
//! - `dynamic`/`InvalidType`/`Never` operands dispatch dynamically and
//!   stay silent (upstream `type_property_resolver.dart:88-94`'s
//!   `isDynamicBounded`/`isInvalidBounded` short-circuit and the
//!   `NeverType` arm :211-216).
//! - `Null` dereferences unconditionally — the verifier's `NullType` arm
//!   swaps the code to `invalid_use_of_null_value` at the operand
//!   (nullable_dereference_verifier.dart:47-49).
//! - `void` is potentially nullable upstream: a binary/unary/increment
//!   application reports the shared `unchecked_use_of_nullable_value`
//!   code at the operator; `[]`/`[]=` and `+=` report
//!   `use_of_void_result` — that family owns the row
//!   (property_element_resolver.dart:183-186).
//! - Record and function faces report `undefined_operator` per spelling
//!   — unless `==` or a shape extension resolves the spelling
//!   (`_lookupExtension` runs on shapes, :227).
//! - `T extends num` resolves `+` through the bound
//!   (`resolveToBound`, binary_expression_resolver.dart:461) — a
//!   member-result `T` consults its DECLARING container's captured
//!   bounds, a bare `T a` field the `refs` publication's container.
//! - An ambiguous extension application replaces the miss
//!   (`ambiguous_extension_member_access` at the operator).
//!
//! Oracle provenance: `dart analyze` 3.13.1 — every expectation below
//! was probed against the real binary before the fix.

use super::test_rig::{code_names, report_ops, report_ops_bare, rows};

/// Operator rows only — the report's `undefined_operator` diagnostics in
/// emission order.
fn operator_names(report: &super::FileReport) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_operator")
        .map(|d| d.code.name)
        .collect()
}

/// `int + int` resolves the declared `operator +` — silent.
#[test]
fn declared_operator_is_silent() {
    let report = report_ops("void f(int a, int b) { a + b; }\n");
    assert_silent(&report);
}

/// `a - b` on a class declaring no `-` reports `undefined_operator`.
#[test]
fn missing_operator_reports() {
    let report = report_ops("class C {} void f(C a, C b) { a - b; }\n");
    assert_eq!(
        operator_names(&report),
        vec!["undefined_operator"],
        "{:?}",
        rows(&report)
    );
}

/// `a + s` with `operator +(int)` fed a `String` reports
/// `argument_type_not_assignable`, never `undefined_operator`.
#[test]
fn wrong_argument_reports_assignable_not_undefined() {
    let report = report_ops(
        "class C { C operator +(int o) => this; } \
         void f(C a, String s) { a + s; }\n",
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"undefined_operator"),
        "unexpected undefined_operator: {:?}",
        rows(&report)
    );
    assert_eq!(
        names,
        vec!["argument_type_not_assignable"],
        "{:?}",
        rows(&report)
    );
}

/// `x[0] = v` resolves `[]=` on the declared indexer — silent.
#[test]
fn index_set_is_silent() {
    let report =
        report_ops("class C { void operator []=(int i, int v) {} } void f(C c) { c[0] = 1; }\n");
    assert_silent(&report);
}

/// `List`'s declared `[]` resolves `xs[0]` — silent.
#[test]
fn prelude_index_read_is_silent() {
    let report = report_ops("void f(List<int> xs) { xs[0]; }\n");
    assert_silent(&report);
}

/// `d + x` on `dynamic` dispatches dynamically — silent
/// (`isDynamicBounded`, type_property_resolver.dart:88-94).
#[test]
fn dynamic_receiver_is_silent() {
    let report = report_ops("void f(dynamic d, int x) { d + x; }\n");
    assert_silent(&report);
}

/// `d++` on `dynamic` — the compound/increment twin is silent too.
#[test]
fn dynamic_increment_is_silent() {
    let report = report_ops("void f(dynamic d) { d++; }\n");
    assert_silent(&report);
}

/// `n + 1` on `Null` swaps to `invalid_use_of_null_value` at the operand
/// — never `undefined_operator` (nullable_dereference_verifier's
/// `NullType` arm).
#[test]
fn null_receiver_reports_invalid_use_of_null() {
    let report = report_ops("void f(Null n) { n + 1; }\n");
    let names = code_names(&report);
    assert!(
        !names.contains(&"undefined_operator"),
        "unexpected undefined_operator: {:?}",
        rows(&report)
    );
    assert_eq!(names, vec!["invalid_use_of_null_value"], "{:?}", rows(&report));
}

/// `n++` on `Null` — the increment twin swaps to `invalid_use_of_null_value`.
#[test]
fn null_increment_reports_invalid_use_of_null() {
    let report = report_ops("void f(Null n) { n++; }\n");
    assert_eq!(
        code_names(&report),
        vec!["invalid_use_of_null_value"],
        "{:?}",
        rows(&report)
    );
}

/// `v() + 1` on a `void` result: the operand's `void` face never reaches
/// `undefined_operator` (the void-result read reports its own family —
/// `use_of_void_result` upstream, `isPotentiallyNullable`'s VoidType arm
/// keeping the operand out of the operator-miss consult).
#[test]
fn void_result_binary_reports_no_undefined_operator() {
    let report = report_ops("void v() {} void f() { v() + 1; }\n");
    assert!(
        !code_names(&report).contains(&"undefined_operator"),
        "unexpected undefined_operator: {:?}",
        rows(&report)
    );
}

/// `x < y` on a `Function`-typed operand reports `undefined_operator`
/// naming the receiver's signature display — a shape face declares no
/// user operator (upstream `instanceReceiverType` answers the composite
/// `int Function()`).
#[test]
fn function_relational_reports_undefined() {
    let report = report_ops("void f(int Function() x, int Function() y) { x < y; }\n");
    assert_eq!(
        operator_names(&report),
        vec!["undefined_operator"],
        "{:?}",
        rows(&report)
    );
}

/// `r + x` on a record operand reports `undefined_operator` naming the
/// record's render — `_lookupExtension` finds no extension here.
#[test]
fn record_binary_reports_undefined() {
    let report = report_ops("void f((int, String) r, int x) { r + x; }\n");
    assert_eq!(
        operator_names(&report),
        vec!["undefined_operator"],
        "{:?}",
        rows(&report)
    );
}

/// `(1, 'x') == r` resolves through `Object`'s `==` face — silent.
#[test]
fn record_equality_is_silent() {
    let report = report_ops("void f((int, String) r) { (1, 'x') == r; }\n");
    assert!(
        !code_names(&report).contains(&"undefined_operator"),
        "unexpected undefined_operator: {:?}",
        rows(&report)
    );
}

/// A shape extension declaring `+` resolves the record application —
/// `_lookupExtension` runs on shape receivers too
/// (type_property_resolver.dart:227).
#[test]
fn record_extension_operator_is_silent() {
    let report = report_ops(
        "extension E on (int, String) { (int, String) operator +((int, String) o) => this; } \
         void f((int, String) r) { r + r; }\n",
    );
    assert!(
        !code_names(&report).contains(&"undefined_operator"),
        "unexpected undefined_operator: {:?}",
        rows(&report)
    );
}

/// `a += 1` on `T extends num` resolves `+` through `num`'s bound —
/// `resolveToBound` precedes the member consult (the prelude declares
/// `int`'s `+` on `int`, so the bound spells `int`).
#[test]
fn type_parameter_compound_is_silent() {
    let report = report_ops("void f<T extends int>(T a) { a += 1; }\n");
    assert!(
        !code_names(&report).contains(&"undefined_operator"),
        "unexpected undefined_operator: {:?}",
        rows(&report)
    );
}

/// `c.a + 1` on a `T a` member result consults its declaring
/// container's captured bound for `T` — `C<T extends int>` resolves `+`.
#[test]
fn member_result_type_parameter_is_silent() {
    let report = report_ops(
        "class C<T extends int> { T? a; } \
         void f(C<int> c) { c.a! + 1; }\n",
    );
    assert!(
        !code_names(&report).contains(&"undefined_operator"),
        "unexpected undefined_operator: {:?}",
        rows(&report)
    );
}

/// `a += 1` on a `T a` field resolves `T`'s bound through the `refs`
/// publication's declaring container.
#[test]
fn bare_field_type_parameter_compound_is_silent() {
    let report = report_ops(
        "class C<T extends int> { T? a; void m() { a! += 1; } }\n",
    );
    assert!(
        !code_names(&report).contains(&"undefined_operator"),
        "unexpected undefined_operator: {:?}",
        rows(&report)
    );
}

/// Two applicable `operator +` extensions report
/// `ambiguous_extension_member_access` at the operator, never
/// `undefined_operator`.
#[test]
fn ambiguous_extension_operator_reports_ambiguity() {
    let report = report_ops(
        "class A {} \
         extension E1 on A { A operator +(A o) => this; } \
         extension E2 on A { A operator +(A o) => this; } \
         void f(A a, A b) { a += b; }\n",
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"undefined_operator"),
        "unexpected undefined_operator: {:?}",
        rows(&report)
    );
    assert!(
        names.contains(&"ambiguous_extension_member_access"),
        "expected ambiguous_extension_member_access: {:?}",
        rows(&report)
    );
}

/// `a + b` over an extension declaring `operator +` resolves the
/// implicit application — silent.
#[test]
fn extension_operator_binary_is_silent() {
    let report = report_ops(
        "class A {} \
         extension E on A { A operator +(A o) => this; } \
         void f(A a, A b) { a + b; }\n",
    );
    assert!(
        !code_names(&report).contains(&"undefined_operator"),
        "unexpected undefined_operator: {:?}",
        rows(&report)
    );
}

/// A bare `report_ops_bare` fixture declares no `operator -`: a
/// genuinely missing operator on a declared class still reports.
#[test]
fn bare_missing_operator_reports() {
    let report = report_ops_bare("class C {} void f(C a, C b) { a - b; }\n");
    assert_eq!(
        operator_names(&report),
        vec!["undefined_operator"],
        "{:?}",
        rows(&report)
    );
}

fn assert_silent(report: &super::FileReport) {
    assert!(
        report.diagnostics.is_empty(),
        "expected no rows, got {:#?}",
        rows(report)
    );
}
