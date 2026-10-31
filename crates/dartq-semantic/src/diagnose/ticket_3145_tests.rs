//! The #3145 pins: the multi-row right-hand-side window and the
//! assignability verdict's callable-class/generic-signature faces —
//! `invalid_assignment` and `argument_type_not_assignable` read the
//! expression's RESULT row (the fold discipline pushes the outermost
//! expression's row last), never require a single-row window.
//!
//! Upstream premises: `checkForInvalidAssignment` reads
//! `AssignmentExpression.rightHandSide.staticType`
//! (error_verifier.dart's `InvalidAssignment` arm —
//! assignment_expression_resolver.dart:694-703 feeds the flow-promoted
//! `valueType` the staged re-decision mirrors); `isAssignableTo`'s
//! callable-class acceptance (type_system.dart:906-916 —
//! `getCallMethodType` null on a resolvable no-`call` container rejects);
//! `relateTypeParameters`' shared-parameter instantiation
//! (type_system.dart:1581-1635 — `T Function<T>(T)` against
//! `U Function<U>(U)` compares unified positions, never the two unrelated
//! heads); `handleValuedFormalParameter`'s default-value ask
//! (`checkForInvalidAssignment(defaultValue, paramType)`).

use super::super::types::FileReport;
use super::async_return_tests::{code_names, report};

/// The two codes this ticket owns; the pins assert over these alone so
/// incidental warnings (`unused_local_variable` on pin locals,
/// `dead_code` on unreachable tails) never blur the verdict.
fn assign_codes(report: &FileReport) -> Vec<&'static str> {
    code_names(report)
        .into_iter()
        .filter(|code| matches!(*code, "invalid_assignment" | "argument_type_not_assignable"))
        .collect()
}

// —— the multi-row result window ——————————————————————————————————

/// A member read on a creation receiver asks the member's declared type:
/// `String y = B().x` resolves `x` against `B` even though the merge
/// keeps no typed receiver face (oracle: 'int' into 'String' reports).
#[test]
fn initializer_member_on_creation_reports() {
    let report = report("class B { int x = 2; }\nvoid main() { String y = B().x; }");
    assert_eq!(assign_codes(&report), ["invalid_assignment"]);
}

/// The same member read on the plain `=` face (`y = B().x`): the
/// right-hand-side window resolves to its tail row exactly like the
/// single-row window resolved to its only entry.
#[test]
fn assignment_member_on_creation_reports() {
    let report = report("class B { int x = 2; }\nvoid main() { String y; y = B().x; }");
    assert_eq!(assign_codes(&report), ["invalid_assignment"]);
}

/// A getter on a creation receiver (`B().x` where `x` is a getter) reads
/// the accessor's declared return — the same leaf face as the field.
#[test]
fn assignment_getter_on_creation_reports() {
    let report = report("class B { int get x => 3; }\nvoid main() { String y = B().x; }");
    assert_eq!(assign_codes(&report), ["invalid_assignment"]);
}

/// A bare creation head reads its instantiated container: `String y =
/// C()` asks `C` against `String` (the fold's sentinel row carries the
/// creation's static type even when no written type arguments resolved).
#[test]
fn initializer_bare_creation_reports() {
    let report = report("class C {}\nvoid main() { String y = C(); }");
    assert_eq!(assign_codes(&report), ["invalid_assignment"]);
}

/// A cascade's result row is the target-typed value stand-in: `v =
/// (a)..toString()` asks the target's `int`, not the last section's.
#[test]
fn assignment_cascade_target_reports() {
    let report = report("void main() { String v = \"a\"; int a = 0; v = (a)..toString(); }");
    assert_eq!(assign_codes(&report), ["invalid_assignment"]);
}

/// A conditional tail joins its two operand rows: `y = c ? 1 : 2` asks
/// `int` against `String` (the join's same-head arm).
#[test]
fn assignment_conditional_reports() {
    let report = report("void main() { bool c = true; String y; y = c ? 1 : 2; }");
    assert_eq!(assign_codes(&report), ["invalid_assignment"]);
}

/// The conditional's clean twin: matching operand types stay silent.
#[test]
fn assignment_conditional_matching_stays_clean() {
    let report = report("void main() { bool c = true; String y; y = c ? \"a\" : \"b\"; }");
    assert!(
        assign_codes(&report).is_empty(),
        "{:?}",
        code_names(&report)
    );
}

/// An if-null tail joins `nonNull(left)` with the right operand:
/// `y = a ?? 0` over `int? a` asks `int` against `String`.
#[test]
fn assignment_if_null_reports() {
    let report = report("void main() { int? a; String y; y = a ?? 0; }");
    assert_eq!(assign_codes(&report), ["invalid_assignment"]);
}

/// The if-null's clean twin: `String? a; y = a ?? "b"` joins `String`
/// with `String` — no row.
#[test]
fn assignment_if_null_matching_stays_clean() {
    let report = report("void main() { String? a; String y; y = a ?? \"b\"; }");
    assert!(
        assign_codes(&report).is_empty(),
        "{:?}",
        code_names(&report)
    );
}

/// A parenthesized right side asks the group's own type: `y = (a)` where
/// `a` is `int` reports against `String`.
#[test]
fn assignment_parenthesized_reports() {
    let report = report("void main() { int a = 0; String y; y = (a); }");
    assert_eq!(assign_codes(&report), ["invalid_assignment"]);
}

/// An invoked member on a creation receiver asks the callee's declared
/// return: `y = B().m()` reads `m`'s `int` (the send-result face
/// [`expression_row_ty`] resolves), never the unknown row.
#[test]
fn assignment_send_on_creation_reports() {
    let report = report("class B { int m() => 0; }\nvoid main() { String y = B().m(); }");
    assert_eq!(assign_codes(&report), ["invalid_assignment"]);
}

/// The return window's result row is its tail too: `return B().x` asks
/// the member's `int` against the declared `String` return
/// (`return_of_invalid_type` is the return face's own code — the ask is
/// the same tail-row read, asserted empty here only on the owned codes).
#[test]
fn return_member_on_creation_checks() {
    let report = report("class B { int x = 2; }\nString f() { return B().x; }");
    assert!(code_names(&report).contains(&"return_of_invalid_type"));
}

/// The single-row regression guard: a plain mismatched initializer still
/// reports exactly once.
#[test]
fn initializer_plain_mismatch_reports() {
    let report = report("void main() { int x = \"s\"; }");
    assert_eq!(assign_codes(&report), ["invalid_assignment"]);
}

// —— the callable-class faces (type_system.dart:906-916) ————————————

/// A resolvable container with NO `call` member rejects against the bare
/// `Function` head: upstream's `getCallMethodType` is null there, so the
/// arm falls through to `false` — `Function f = C()` reports 'C'.
#[test]
fn callable_missing_call_reports() {
    let report = report("class C {}\nFunction f = C();");
    assert_eq!(assign_codes(&report), ["invalid_assignment"]);
}

/// A `call`-carrying container accepts the bare `Function` head
/// (`isAssignableTo(callMethodType, Function)` is unconditional).
#[test]
fn callable_with_call_accepts_function_head() {
    let report = report("class C { void call(int a) {} }\nvoid main() { Function f = C(); }");
    assert!(
        assign_codes(&report).is_empty(),
        "{:?}",
        code_names(&report)
    );
}

/// A `call` whose signature mismatches the function-shaped target
/// rejects: `int call(String)` against `String Function(String)` fails
/// the return-covariance ask.
#[test]
fn callable_signature_mismatch_reports() {
    let report = report(
        "class C { int call(String s) => 0; }\nvoid main() { String Function(String) f = C(); }",
    );
    assert_eq!(assign_codes(&report), ["invalid_assignment"]);
}

/// The matching twin stays clean (`int call(String)` against
/// `int Function(String)`).
#[test]
fn callable_signature_match_stays_clean() {
    let report = report(
        "class C { int call(String s) => 0; }\nvoid main() { int Function(String) f = C(); }",
    );
    assert!(
        assign_codes(&report).is_empty(),
        "{:?}",
        code_names(&report)
    );
}

/// A generic `call`'s parameters ride the signature — the count arm
/// rejects a nullary target outright (`void call<U>(U)` against
/// `void Function()` cannot spell its one required formal).
#[test]
fn callable_generic_call_counts_against_target() {
    let report = report(
        "class C { void call<U>(U u) {} }\nvoid main() { void Function(bool, String) f = C(); }",
    );
    assert_eq!(assign_codes(&report), ["invalid_assignment"]);
}

// —— `relateTypeParameters`' shared-parameter instantiation ————————

/// Two generic function types with matching arity and unwritten bounds
/// are assignable — `U Function<U>(U) foo = f` over `T f<T>(T)` stays
/// silent (upstream instantiates both sides with one shared parameter
/// set; the unified positions compare reflexive, never as two heads).
#[test]
fn generic_tearoff_unified_positions_stay_clean() {
    let report = report("T f<T>(T a) => a;\nU Function<U>(U) foo = f;");
    assert!(
        assign_codes(&report).is_empty(),
        "{:?}",
        code_names(&report)
    );
}

/// The arity-mismatched twin reports: `T Function<T>(T)` cannot spell the
/// two-parameter target `U Function<U>(U, int)`.
#[test]
fn generic_tearoff_arity_mismatch_reports() {
    let report = report("T f<T>(T a) => a;\nU Function<U>(U, int) foo = f;");
    assert_eq!(assign_codes(&report), ["invalid_assignment"]);
}

// —— the type-parameter target arm (subtype.dart's Left Variable) ———

/// An interface source never subtypes a bare type-parameter target:
/// `U f = C()` inside `class D<U extends Function>` reports 'C'.
#[test]
fn type_param_target_reports_interface_source() {
    let report = report("class C {}\nclass D<U extends Function> { void m() { U f = C(); } }");
    assert_eq!(assign_codes(&report), ["invalid_assignment"]);
}
