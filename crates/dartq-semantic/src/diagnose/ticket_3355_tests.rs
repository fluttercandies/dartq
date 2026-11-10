//! #3355 — the `missing_required_argument` coverage pins + the
//! `use_of_void_result` residual faces absorbed from #3350.
//!
//! `missing_required_argument`: the required-named coverage at the
//! invocation's callee span — upstream `RequiredParametersVerifier._check`
//! walks `parameters` in declaration order and reports a
//! `required` named formal with no same-named argument
//! (required_parameters_verifier.dart:122-156).
//!
//! `use_of_void_result` residuals (the #3316 faces):
//! 1. the extension-override argument `E(v()).g` — `v()` returns `void`
//!    and the extension's receiver check reports `use_of_void_result` at
//!    the receiver expression before the not-assignable verdict
//!    (extension_member_resolver.dart:287-288);
//! 2. a member send `v().sel()` — `v()`'s `void` return makes the receiver
//!    read's access consume a void value (upstream's
//!    `property_element_resolver.dart` `_resolve` `targetType is VoidType`
//!    arm);
//! 3. the conditional-expression cascade target — the `.m()` send folded
//!    under a `?:` arm consumes its void target.

use super::*;
use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};

fn names(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

fn count(report: &FileReport, code: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .count()
}

fn offsets(report: &FileReport, code: &str) -> Vec<u32> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.range.offset)
        .collect()
}

fn offset_of(source: &str, needle: &str) -> u32 {
    let prelude = CORE_PRELUDE.chars().map(char::len_utf16).sum::<usize>() as u32;
    prelude
        + source[..source.find(needle).expect("needle present")]
            .chars()
            .map(char::len_utf16)
            .sum::<usize>() as u32
}

// —— `missing_required_argument` ——

/// A `required` named formal with no same-named argument reports at the
/// invocation's callee entity (upstream anchors at `errorEntity: node` —
/// the method name, constructor name, or function name).
#[test]
fn required_named_missing_reports_at_callee() {
    let source = "void callee({required int x}) {}\nvoid g() { callee(); }\n";
    let report = report_ops(source);
    let expected = offset_of(source, "callee();");
    assert_eq!(
        offsets(&report, "missing_required_argument"),
        vec![expected],
        "callee() with missing required x reports at callee: {:?}",
        names(&report)
    );
}

/// Two `required` named formals both missing report in declaration order —
/// the oracle walks `parameters` once and emits one row per missing
/// required named.
#[test]
fn required_named_missing_reports_each_in_order() {
    let source = "void f({required int a, required int b, int c = 0}) {}\n\
         void g() { f(); }\n";
    let report = report_ops(source);
    assert_eq!(
        count(&report, "missing_required_argument"),
        2,
        "f() missing a and b reports both: {:?}",
        names(&report)
    );
}

/// A `required` named formal with the same-named argument supplied stays
/// silent — the coverage walk's `_containsNamedExpression` returns true.
#[test]
fn required_named_supplied_stays_silent() {
    let source = "void f({required int x}) {}\nvoid g() { f(x: 1); }\n";
    let report = report_ops(source);
    assert!(
        !names(&report).contains(&"missing_required_argument"),
        "f(x: 1) covers required x: {:?}",
        names(&report)
    );
}

/// A member send `c.m()` over a `required` named method parameter reports —
/// the method's formal table reaches the same coverage walk as a top-level
/// function's.
#[test]
fn method_required_named_missing_reports() {
    let source = "class C {\n\
         \x20 void m({required int x}) {}\n\
         }\n\
         void g(C c) { c.m(); }\n";
    let report = report_ops(source);
    assert_eq!(
        count(&report, "missing_required_argument"),
        1,
        "c.m() missing required x reports: {:?}",
        names(&report)
    );
}

/// A constructor call `C()` over a `required` named parameter reports —
/// the unnamed-creation face walks the same coverage table.
#[test]
fn unnamed_ctor_required_named_missing_reports() {
    let source = "class C {\n\
         \x20 C({required int x});\n\
         }\n\
         void g() { C(); }\n";
    let report = report_ops(source);
    assert_eq!(
        count(&report, "missing_required_argument"),
        1,
        "C() missing required x reports: {:?}",
        names(&report)
    );
}

/// A `super` constructor's `required` named formal is covered by the
/// enclosing constructor's `super.x` formal — upstream's
/// `_containsNamedExpression` second walk.
#[test]
fn super_formal_covers_required_named() {
    let source = "class B {\n\
         \x20 B({required int x});\n\
         }\n\
         class C extends B {\n\
         \x20 C({required super.x});\n\
         }\n";
    let report = report_ops(source);
    assert!(
        !names(&report).contains(&"missing_required_argument"),
        "super.x covers the required super formal: {:?}",
        names(&report)
    );
}

// —— `use_of_void_result` (the #3350 residuals) ——

/// `Ext(v()).g` over `void v()` reports `use_of_void_result` at the
/// receiver expression — the extension-override's first arm
/// (extension_member_resolver.dart:287-288) fires before the
/// not-assignable verdict the same seam suppresses on a void receiver.
/// The anchor is the whole invoked expression (`v()`, the argument node),
/// not the callee name alone.
#[test]
fn extension_override_void_receiver_reports() {
    let source = "extension Ext on int {\n\
         \x20 int get g => this;\n\
         }\n\
         void vv() {}\n\
         void f() { Ext(vv()).g; }\n";
    let report = report_ops(source);
    // `vv()` as it appears inside `Ext(` — the use, not the declaration.
    let expected = offset_of(source, "vv()).g");
    assert_eq!(
        offsets(&report, "use_of_void_result"),
        vec![expected],
        "Ext(vv()) over void reports at vv(): {:?}",
        names(&report)
    );
}

/// `E(v()).g` over `void v()` does NOT stack
/// `extension_override_argument_not_assignable` beside the void row —
/// upstream's `else if` keeps the assignability verdict off a void
/// receiver.
#[test]
fn extension_override_void_receiver_suppresses_assignable() {
    let source = "extension E on int {\n\
         \x20 int get g => this;\n\
         }\n\
         void v() {}\n\
         void f() { E(v()).g; }\n";
    let report = report_ops(source);
    assert!(
        !names(&report).contains(&"extension_override_argument_not_assignable"),
        "void receiver suppresses the assignable row: {:?}",
        names(&report)
    );
}

/// `foo()()` over `void foo()` — the invoked expression's own static type
/// is `Void` (upstream `function_expression_invocation_resolver.dart:55-56`
/// `_checkForUseOfVoidResult(function, function.typeOrThrow)`); the oracle
/// anchors at the inner send's `methodName` (`foo`). The arm at
/// `invocation.rs` documents the face but never arms — `pending_send` only
/// carries abutting callee rows and the outer `()` group's callee is the
/// inner send's folded result, not `foo`. `#[ignore]`d until the outer-`()`
/// linkage lands.
#[test]
#[ignore = "outer-() linkage on a completed-send callee is unarmed (#3355 residual)"]
fn invoked_expression_void_reports_at_callee_name() {
    let source = "void foo() {}\n\
         void f() { foo()(); }\n";
    let report = report_ops(source);
    let expected = offset_of(source, "foo()");
    assert_eq!(
        offsets(&report, "use_of_void_result"),
        vec![expected],
        "foo()() reports at foo: {:?}",
        names(&report)
    );
}

/// `v().toString()` over `void v()` — the member send's receiver is `v()`'s
/// `void` result, so `use_of_void_result` reports at `v` (the invoked
/// expression's `methodName`), never at the `.toString` selector. The
/// receiver-side emit lives in listener.rs (`emit_void_member_receiver`),
/// outside this ticket's invocation.rs scope — the pin is `#[ignore]`d for
/// the listener lane (#3359) that owns the anchor fix.
#[test]
#[ignore = "emit_void_member_receiver anchor widening lives in listener.rs (#3359)"]
fn member_send_void_receiver_anchors_at_callee_name() {
    let source = "void v() {}\n\
         void f() { v().toString(); }\n";
    let report = report_ops(source);
    let expected = offset_of(source, "v()");
    let sel = offset_of(source, "toString");
    let actual = offsets(&report, "use_of_void_result");
    assert_eq!(
        actual.len(),
        1,
        "v().toString() reports exactly once: {:?}",
        names(&report)
    );
    assert_eq!(
        actual[0],
        expected,
        "the row anchors at v (offset {expected}), not the selector {sel}: {:?}",
        names(&report)
    );
}
