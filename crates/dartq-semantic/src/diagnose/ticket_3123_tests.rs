//! The #3123 pins: an UNTYPED formal parameter is `dynamic`, not a
//! poisoned function-type capture.
//!
//! `fn_capture_formal` armed its slot from `pending_formal`, which only a
//! written type's `fn_capture_type` sets. An untyped formal (`void f(x)`,
//! `f([x])`, `f({x})`, a nested `f(void g(x))` inner formal) armed
//! nothing → `pending_formal.take()` returned `None` →
//! `capture.poisoned = true` → the WHOLE function-type capture was
//! discarded. The slot silently vanished from the shape: `void g(x)`
//! rendered `void Function()` (zero formals) instead of
//! `void Function(dynamic)`, so assignability/arity verdicts against it
//! read a phantom-arity or empty-parameter signature.
//!
//! Upstream, an untyped `NormalFormalParameter` resolves `dynamic` —
//! `visitRegularFormalParameter` assigns
//! `element.type = _typeProvider.dynamicType` whenever `node.type == null`
//! (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/resolver/resolution_visitor.dart:808-813).
//! The capture now pushes `Ty::Dynamic` for the unarmed slot instead of
//! poisoning.
//!
//! The UNNAMED-formal face (`fn_capture_formal_unnamed`,
//! `fn_capture_formal_end`) is correct as-is and deliberately NOT
//! changed: an unnamed formal column (`void Function(Object)`) always
//! carries a written type — Dart's grammar has no unnamed-UNtyped
//! formal — so `pending_formal` is `Some` there and its `None`
//! early-return is the no-op arm, never a real untyped slot.
//!
//! Oracle provenance: `dart analyze` 3.13.7 over the /tmp/at3123
//! battery — `v_arg.dart` (`f(void g(x))` + `f((int y)=>0)`) reports
//! `argument_type_not_assignable` with param face `void Function(dynamic)`;
//! `v_ret.dart` (`return g`) reports `return_of_invalid_type` naming
//! `void Function(dynamic)`; `v_typedformal_arg2.dart`
//! (`f(void g(int x))` + `f((int y)=>0)`) is silent.

use super::test_rig::{assert_silent, report_ops};

fn rows_named<'d>(
    report: &'d crate::diagnose::FileReport,
    name: &str,
) -> Vec<&'d crate::diagnose::Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .collect()
}

// —— the untyped slot renders `dynamic` in the signature face ——

/// `f(void g(x))` then `f((int y) => 0)`: the oracle's parameter face is
/// `void Function(dynamic)` — `dynamic` accepts nothing narrower under
/// parameter contravariance, so `void Function(int)` can't be assigned.
/// The pre-fix capture dropped `x`, rendering `void Function()` and a
/// wrong message face.
#[test]
fn untyped_inner_formal_renders_dynamic_param_face() {
    let report = report_ops(
        "void f(void g(x)) {}\n\
         void main() { f((int y) => 0); }\n",
    );
    let rows = rows_named(&report, "argument_type_not_assignable");
    assert_eq!(
        rows.len(),
        1,
        "void Function(dynamic) param rejects a void Function(int) arg: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
    assert!(
        rows[0].message.contains("void Function(dynamic)"),
        "param face renders dynamic, not an empty list: {}",
        rows[0].message
    );
}

/// Same shape with a `String` arg — the param face stays
/// `void Function(dynamic)` regardless of the argument.
#[test]
fn untyped_inner_formal_dynamic_face_string_arg() {
    let report = report_ops(
        "void f(void g(x)) {}\n\
         void main() { f((String y) => 0); }\n",
    );
    let rows = rows_named(&report, "argument_type_not_assignable");
    assert_eq!(rows.len(), 1, "one ATNA row: {rows:?}");
    assert!(
        rows[0].message.contains("void Function(dynamic)"),
        "param face is dynamic: {}",
        rows[0].message
    );
}

// —— the capture survives: a returned untyped-typed value checks ——

/// `return g` where `g: void Function(dynamic)` into an
/// `int Function(int)` return slot reports `return_of_invalid_type`.
/// The discarded capture used to give `g` no type → the row went MISSING.
#[test]
fn untyped_inner_formal_return_of_invalid_type() {
    let report = report_ops(
        "int Function(int) f(void g(x)) { return g; }\n\
         void main() {}\n",
    );
    let rows = rows_named(&report, "return_of_invalid_type");
    assert_eq!(
        rows.len(),
        1,
        "void Function(dynamic) cannot return as int Function(int): {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

// —— the silent controls ——

/// `void topx(x)` and `void argfx(void f(x))` then `argfx(topx)`: BOTH
/// sides are `void Function(dynamic)` — an untyped-formal pair assigns
/// cleanly (the #3110 AT1 arm). No `argument_type_not_assignable`.
#[test]
fn untyped_formal_pair_assigns_cleanly() {
    let report = report_ops(
        "void topx(x) {}\n\
         void argfx(void f(x)) {}\n\
         void main() { argfx(topx); }\n",
    );
    assert!(
        rows_named(&report, "argument_type_not_assignable").is_empty(),
        "untyped-formal pair must not report ATNA: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

/// The optional-positional twin — `void Function([dynamic])` both sides.
#[test]
fn untyped_optional_formal_pair_assigns_cleanly() {
    let report = report_ops(
        "void topy([y]) {}\n\
         void argfy(void f([y])) {}\n\
         void main() { argfy(topy); }\n",
    );
    assert!(
        rows_named(&report, "argument_type_not_assignable").is_empty(),
        "untyped-optional pair must not report ATNA: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

/// A matching typed inner formal stays silent — no regression on the
/// typed path.
#[test]
fn typed_inner_formal_match_silent() {
    let report = report_ops(
        "void f(void g(int x)) {}\n\
         void main() { f((int y) => 0); }\n",
    );
    assert!(
        rows_named(&report, "argument_type_not_assignable").is_empty(),
        "int-to-int inner formal is clean: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

/// A typed-typed MISMATCH still reports — the dynamic fill doesn't widen
/// the typed face.
#[test]
fn typed_inner_formal_mismatch_reports() {
    let report = report_ops(
        "void f(void g(int x)) {}\n\
         void main() { f((String y) => 0); }\n",
    );
    let rows = rows_named(&report, "argument_type_not_assignable");
    assert_eq!(
        rows.len(),
        1,
        "String-vs-int inner formal reports: {rows:?}"
    );
    assert!(
        rows[0].message.contains("void Function(int)"),
        "typed face stays int: {}",
        rows[0].message
    );
}

// —— every section gets the dynamic fill ——

/// An untyped REQUIRED positional (`f(x)`) fills `dynamic`.
#[test]
fn untyped_required_positional_is_dynamic() {
    let report = report_ops(
        "void f(x) {}\n\
         void main() { f(1); f(\"s\"); }\n",
    );
    assert!(
        report.diagnostics.is_empty(),
        "untyped formal accepts anything: {:?}",
        report.diagnostics
    );
}

/// An untyped OPTIONAL positional (`f([x])`) fills `dynamic`.
#[test]
fn untyped_optional_positional_is_dynamic() {
    let report = report_ops(
        "void f([x]) {}\n\
         void main() { f(1); f(\"s\"); }\n",
    );
    assert!(
        report.diagnostics.is_empty(),
        "untyped optional formal accepts anything: {:?}",
        report.diagnostics
    );
}

/// An untyped NAMED formal (`f({x})`) fills `dynamic` — the named slot
/// keeps its name and gets `Ty::Dynamic`.
#[test]
fn untyped_named_formal_is_dynamic() {
    let report = report_ops(
        "void f({x}) {}\n\
         void main() { f(x: 1); f(x: \"s\"); }\n",
    );
    assert!(
        report.diagnostics.is_empty(),
        "untyped named formal accepts anything: {:?}",
        report.diagnostics
    );
}

/// A MIXED list (`f(int a, b)`): the typed formal keeps `int`, the
/// untyped one fills `dynamic` — no cross-contamination, no phantom arity.
#[test]
fn mixed_typed_and_untyped_formals() {
    let report = report_ops(
        "void f(int a, b) {}\n\
         void main() { f(1, \"s\"); f(\"s\", 1); }\n",
    );
    // `a` is int → `f("s", 1)` should report ATNA on `a`; `b` is dynamic → fine.
    let rows = rows_named(&report, "argument_type_not_assignable");
    assert_eq!(
        rows.len(),
        1,
        "only the int 'a' slot rejects a String: {rows:?}"
    );
}
