//! The `unused_local_variable` write-vs-read gate (#3057): the official
//! verifier marks a local used only by a READ — a write-only variable still
//! reports `unused_local_variable` unless wildcard-exempt
//! (`GatherUsedLocalElementsVisitor.visitSimpleIdentifier` gates the
//! `LocalVariableElement` credit on `_isReadIdentifier`,
//! unused_local_elements_verifier.dart:300-303 + :429-453; the shared
//! case-scope join's guard-vs-body split comes from `_resolveGuardedPattern`,
//! resolution_visitor.dart:926-929).

use super::test_rig::{assert_silent, code_names, report_ops, rows};

/// An existing-variable for-in target is a WRITE per iteration, never a
/// read: `int i; for (i in xs)` still reports `unused_local_variable` on
/// `i`. The bare `i` row's `resolve_local` read credit is retracted at the
/// close's write-position gate.
#[test]
fn for_in_existing_variable_write_target_reports() {
    let report = report_ops("void f() { int i; for (i in <int>[]) {} }");
    assert_eq!(
        code_names(&report)
            .into_iter()
            .filter(|c| *c == "unused_local_variable")
            .count(),
        1,
        "rows: {:#?}",
        rows(&report)
    );
}

/// The same write target stays a use when the body reads it — only the
/// head's write-position hit is retracted.
#[test]
fn for_in_existing_variable_read_in_body_silent() {
    assert_silent(&report_ops(
        "void f() { int i = 0; for (i in <int>[]) { print(i); } }",
    ));
}

/// A shared-case-scope `when` guard resolves its OWN case's binder, never
/// the joined element — a guard read in one arm never spares the
/// same-named sibling's binder in another arm of the shared body.
#[test]
fn shared_scope_guard_read_spares_own_arm_only() {
    let report = report_ops(
        "void f(Object? x) { switch (x) { case (int a,) when a > 0: case [int a,]: break; } }",
    );
    let ulv = code_names(&report)
        .into_iter()
        .filter(|c| *c == "unused_local_variable")
        .count();
    // Only the second case's `a` (never read anywhere) reports.
    assert_eq!(ulv, 1, "rows: {:#?}", rows(&report));
}

/// A body-statement read of the shared spelling resolves the joined
/// element and spares every same-named binder in the group.
#[test]
fn shared_scope_body_read_spares_all() {
    assert_silent(&report_ops(
        "void f(Object? x) { switch (x) { case (var a,): case [var a,]: a; } }",
    ));
}
