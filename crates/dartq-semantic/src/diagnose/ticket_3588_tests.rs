//! #3588 pins — a completed send's function-typed RESULT is never
//! unwrapped a second time.
//!
//! # The defect
//!
//! Upstream types an invocation node by the invoked signature's return —
//! ONE unwrap (`method_invocation_resolver.dart` reads the receiver
//! node's `staticType.returnType`; `function_expression_invocation_
//! resolver.dart:55-78` records it as the node's static type). dartq's
//! send close already performs that flatten: the callee row's own
//! function face folds onto its return at the argument group's close,
//! and the resolved result lands on the row the same way. But
//! [`Engine::operand_static_ty`] then read the row's `ty` as if it were
//! still the CALLEE's declared face and applied
//! [`Engine::invoked_callee_result`] again — so any send whose RESULT is
//! itself a function type lost one unwrap level:
//!
//! ```text
//! int Function(int) top() => (int x) => x;
//! var r = top();          // typed `int` by dartq, `int Function(int)`
//!                          // by the oracle — 3.13.7 probed
//! String s = r;           // the miss names 'int', or vanishes entirely
//!                          // when the unwrapped type happens to assign
//! ```
//!
//! ~180 of the failing `invalid_assignment` census rows ride this one
//! lever, in both directions: the double-unwrapped face invents rows the
//! oracle never reports (`int Function(int) f = top()` "fails" against
//! the phantom `int`), and swallows rows it does (an `int` target
//! accepts the phantom `int` silently).
//!
//! # The wiring
//!
//! [`Engine::operand_static_ty`]'s Invoke arm now decides what the row's
//! `ty` IS before unwrapping it. A send-close solve's instantiated
//! callee keeps its face (the solve skips the fold), so it stays first
//! with its single unwrap. A BARE callee's function-typed `ty` is the
//! send's already-flattened result — the send-close fold consumed every
//! non-null bare function face ahead of any verdict read — so it returns
//! as-is; a nullable function type survives the fold only as the
//! callee's own standing face (the unchecked-use verdict owns that
//! value), except when no local binds the name and the nullable RESULT
//! of a top-level send stands. The bare `Function` interface spelling
//! whose declaration names a return (`Function top()`) keeps that
//! return instead of the interface's `dynamic` dispatch face. Member and
//! prefixed rows keep the unwrap arms: their faces arm at the dot merge,
//! which the deferred pass can run after the send close, so their
//! function-typed `ty` may still be the callee's own face owed exactly
//! one unwrap (`comparison(a, b)` on `int Function(E, E) comparison`).
//! All oracle verdicts below were probed on 3.13.7.

use crate::diagnose::test_rig::{assert_silent, code_names, report_ops, rows};

/// The exact repro: the send's result is the function type itself, and
/// the miss names it — never the unwrapped `int`.
#[test]
fn t3588_function_typed_call_result_not_unwrapped() {
    let report = report_ops(
        "int Function(int) top() => (int x) => x; \
         var r = top(); \
         String s = r;",
    );
    let misses: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .collect();
    assert_eq!(misses.len(), 1, "expected one row, got {:?}", rows(&report));
    assert!(
        misses[0].message.contains("'int Function(int)'"),
        "the miss must name the function-typed result, got {:?}",
        misses[0].message
    );
}

/// The silent direction: a function-typed result assigns to a
/// function-typed slot without inventing a row the oracle never reports
/// (`int Function(int) f = top()` silently binds — the phantom `int`
/// used to fail against the slot).
#[test]
fn t3588_function_typed_result_assigns_silently() {
    let report = report_ops(
        "int Function(int) top() => (int x) => x; \
         void h() { int Function(int) f = top(); f(1); }",
    );
    assert_silent(&report);
}

/// A genuinely wrong assignment with a NON-function result still reports,
/// with the result type in the message (the fix must not suppress the
/// plain arm).
#[test]
fn t3588_plain_result_assignment_reports_with_result_type() {
    let report = report_ops(
        "int count() => 1; \
         void h() { String s = count(); s.length; }",
    );
    let codes = code_names(&report);
    assert!(
        codes.iter().any(|code| *code == "invalid_assignment"),
        "expected the assignment miss, got {:?}",
        rows(&report)
    );
    let miss = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "invalid_assignment")
        .unwrap();
    assert!(
        miss.message.contains("'int'"),
        "the miss must name the plain result type, got {:?}",
        miss.message
    );
}

/// A nullable function-typed RESULT keeps its nullable face (the fold's
/// nullable refusal must not be read as a standing callee face when no
/// local binds the name).
#[test]
fn t3588_nullable_function_result_keeps_nullable_face() {
    let report = report_ops(
        "int Function(int)? ntop() => (int x) => x; \
         var r = ntop(); \
         String s = r;",
    );
    let misses: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .collect();
    assert_eq!(misses.len(), 1, "expected one row, got {:?}", rows(&report));
    assert!(
        misses[0].message.contains("'int Function(int)?'"),
        "the miss must name the nullable result, got {:?}",
        misses[0].message
    );
}

/// The bare `Function` interface RESULT reads `Function`, never the
/// interface's `dynamic` dispatch face (`Function top()` probed 3.13.7:
/// the assignment reports naming 'Function'; the `dynamic` answer was
/// silently assignable).
#[test]
fn t3588_function_interface_result_not_dynamic() {
    let report = report_ops(
        "Function ftop() => (int x) => x; \
         var r = ftop(); \
         String s = r;",
    );
    let misses: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .collect();
    assert_eq!(misses.len(), 1, "expected one row, got {:?}", rows(&report));
    assert!(
        misses[0].message.contains("'Function'"),
        "the miss must name the Function interface result, got {:?}",
        misses[0].message
    );
}

/// The member faces keep their single unwrap: a function-typed FIELD or
/// GETTER callee answers its signature's return, a METHOD its declared
/// return — one level, never two, never zero (all three probed 3.13.7).
#[test]
fn t3588_member_callee_faces_unwrap_once() {
    let report = report_ops(
        "class A { \
         int Function(int) Function(int) f = (int x) => (int y) => y; \
         int Function(int) get g => (int x) => x; \
         int Function(int) m() => (int x) => x; \
         } \
         void h(A a) { \
         var rf = a.f(1); String sf = rf; \
         var rg = a.g(1); String sg = rg; \
         var rm = a.m(); String sm = rm; \
         }",
    );
    let misses: Vec<&str> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(misses.len(), 3, "expected three rows, got {:?}", rows(&report));
    assert!(
        misses[0].contains("'int Function(int)'") && !misses[0].contains("Function(int) Function"),
        "the field callee's miss must name the signature return, got {:?}",
        misses[0]
    );
    assert!(
        misses[1].contains("'int'"),
        "the getter callee's miss must name the signature return, got {:?}",
        misses[1]
    );
    assert!(
        misses[2].contains("'int Function(int)'"),
        "the method's miss must name its declared return, got {:?}",
        misses[2]
    );
}

/// A local function-typed VARIABLE callee answers its signature's return
/// through the same bare-result gate (the fold put the result on the row;
/// the second unwrap used to take it down to `int`).
#[test]
fn t3588_local_var_callee_result() {
    let report = report_ops(
        "void h() { \
         int Function(int) Function(int) g = (int x) => (int y) => y; \
         var r = g(1); \
         String s = r; \
         }",
    );
    let misses: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .collect();
    assert_eq!(misses.len(), 1, "expected one row, got {:?}", rows(&report));
    assert!(
        misses[0].message.contains("'int Function(int)'"),
        "the miss must name the signature return, got {:?}",
        misses[0].message
    );
}

/// A NULLABLE local callee keeps the unwrap arms (probed 3.13.7: the
/// invocation reports the unchecked use and still types the send `int`)
/// — the nullable face is the callee's own, not a flattened result.
#[test]
fn t3588_nullable_local_callee_still_unwraps() {
    let report = report_ops(
        "void h() { \
         int Function(int)? f; \
         var r = f(1); \
         String s = r; \
         }",
    );
    let codes = code_names(&report);
    assert!(
        codes.contains(&"unchecked_use_of_nullable_value"),
        "expected the unchecked-use row, got {:?}",
        rows(&report)
    );
    let miss = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "invalid_assignment")
        .expect("the assignment miss over the unwrapped return");
    assert!(
        miss.message.contains("'int'"),
        "the miss must name the unwrapped return, got {:?}",
        miss.message
    );
}

/// A callable-class callee keeps the implicit `call` dispatch (the
/// invocation types the `call` method's return — the gate must not
/// mistake the class face for a send result).
#[test]
fn t3588_callable_class_callee_still_dispatches_call() {
    let report = report_ops(
        "class C { int call() => 1; } \
         void h(C c) { var r = c(); int i = r; String s = r; }",
    );
    let miss = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "invalid_assignment")
        .expect("the String assignment over the call return reports");
    assert!(
        miss.message.contains("'int'"),
        "the miss must name the call method's return, got {:?}",
        miss.message
    );
}

/// A send-close solve's instantiated callee keeps its unwrap arm: the
/// solved face unwraps exactly as before the gate (the regression
/// sentinel for [`Engine::solved_call_params`]'s precedence — a generic
/// formal solved from its argument still answers through the row's
/// face, never through the declared-return walk's free parameter).
/// (Known residual, out of this ticket's scope: a solve whose
/// INSTANTIATION is itself a function type still loses a level —
/// `typedef F<T> = T Function(T); F<T> toF<T>(T x)` invoked `toF(1)`
/// answers `int` where the oracle answers `int Function(int)`; the
/// second unwrap there rides the solve's own write-back path, not
/// `operand_static_ty`.)
#[test]
fn t3588_solved_generic_callee_keeps_unwrap_arm() {
    let report = report_ops(
        "void apply(int Function(int) f) { f(1); } \
         void h() { apply((int x) => x); }",
    );
    assert_silent(&report);
}
