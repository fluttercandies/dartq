//! #3284 pins — the explicit-receiver member CALLEE face of
//! `unchecked_use_of_nullable_value`.
//!
//! The family's function text ("The function can't be unconditionally
//! invoked because it can be 'null'.") asserts a nullable callee
//! (`function_expression_invocation_resolver.dart`'s
//! `isNullableFunctionType` read of the callee's static type). An
//! explicit-receiver member read invoked as a call — `e.f()`, `this.g()`,
//! `(e).f()` — where the member is a field or getter of function type
//! reaches that arm at the selector, and the arm's callee snapshot can keep
//! a nullable function shape the member table has already narrowed, so the
//! row lands on a callee that can never be `null` (census source:
//! `tests/language/extension_type/field_promotion_representation_*_test.dart`
//! and `tests/language/inference_update_2/field_invocation_promotion_test.dart`
//! — the oracle reports nothing there).
//!
//! The teardown gate re-decides against the settled member tables: the row
//! drops only when the receiver is a non-nullable face AND the member's
//! resolved result is a non-nullable function type. The pins below lock the
//! firing half plus the three shapes that must keep their verdict (a
//! nullable function member, the raw `Function?` head, and the ordinary
//! non-function member), and the always-green implicit-`this`/raw-`Function`
//! halves that guard the gate's blast radius.

use crate::diagnose::FileReport;
use crate::diagnose::test_rig::report_ops;

/// Every row of one code as `(offset, message)`, in report order.
fn rows(report: &FileReport, code: &str) -> Vec<(u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.message.as_str().to_owned()))
        .collect()
}

/// The family's rows only.
fn unchecked(report: &FileReport) -> Vec<(u32, String)> {
    rows(report, "unchecked_use_of_nullable_value")
}

/// The function variant's text — the only problem message an invoked
/// non-callable-shaped callee row can carry.
const FUNCTION_TEXT: &str =
    "The function can't be unconditionally invoked because it can be 'null'.";

/// The dominant cluster: a field or getter of NON-nullable function type
/// invoked through an explicit receiver (a parameter, a local, `this`, and
/// the parenthesized receiver form alike) reports nothing. Before the gate
/// every one of these rows fired.
#[test]
fn member_function_field_and_getter_callees_are_silent() {
    let report = report_ops(
        "class A { \
           final int Function() f = () => 0; \
           int Function() get g => () => 0; \
           void inner() { this.f(); this.g(); f(); g(); } \
         } \
         void useParam(A a) { a.f(); a.g(); (a).f(); } \
         void useLocal() { A a = A(); a.f(); } \
         void useField(B b) { b.a.f(); } \
         class B { final A a = A(); }\n",
    );
    assert!(
        unchecked(&report).is_empty(),
        "non-nullable function member callees never report: {:?}",
        unchecked(&report)
    );
}

/// The extension-type representation field (one more member-face receiver
/// spelling, the census's `field_promotion_representation_*` shape) stays
/// silent through both the implicit-`this` and the explicit-`this` forms.
#[test]
fn extension_type_representation_field_callee_is_silent() {
    let report = report_ops(
        "extension type E(Object Function() f) { \
           void implicit() { f(); } \
           void explicit() { this.f(); } \
         }\n",
    );
    assert!(
        unchecked(&report).is_empty(),
        "extension-type representation callee never reports: {:?}",
        unchecked(&report)
    );
}

/// The raw `Function` head is non-nullable, so invoking it through a member
/// is legal — the gate must not fire on a face that never reported.
#[test]
fn raw_function_member_callee_stays_silent() {
    let report = report_ops("class C { Function raw = () {}; }\nvoid h(C c) { c.raw(); }\n");
    assert!(
        unchecked(&report).is_empty(),
        "a non-nullable raw `Function` callee never reports: {:?}",
        unchecked(&report)
    );
}

/// The silent negative: a nullable function member keeps its row — a written
/// `int Function()?` field and the nullable raw `Function?` head both invoke
/// a callee that genuinely can be `null`.
#[test]
fn nullable_function_member_callees_still_report() {
    let report = report_ops(
        "class D { int Function()? q = null; Function? r = null; }\
         \nvoid h(D d) { d.q(); d.r(); }\n",
    );
    let rows = unchecked(&report);
    assert_eq!(
        rows.len(),
        2,
        "both nullable function callees report: {rows:?}"
    );
    for (_, message) in &rows {
        assert_eq!(message, FUNCTION_TEXT);
    }
}

/// A member that is not function-typed invokes as a method: no unchecked row
/// anywhere in the file (the gate must leave the ordinary invocation face
/// alone). The receiver container is spelled `Box` because the rig's
/// core prelude declares `E` as a type-parameter tag in `List<E>`/`Set<E>`,
/// and a fixture class named `E` would resolve through that stale tag.
#[test]
fn non_function_member_invocation_stays_clean() {
    let report = report_ops(
        "class Box { int m() => 0; int Function() n() => () => 0; }\
         \nvoid h(Box b) { b.m(); b.n()(); }\n",
    );
    assert!(
        unchecked(&report).is_empty(),
        "ordinary member invocations never carry the family's rows: {:?}",
        unchecked(&report)
    );
}
