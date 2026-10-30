//! The #2894 pins: the `a()` invocation-form `call` credit. A send whose
//! callee resolves through a `call` method — a class member or an applied
//! extension's member — must credit that member WHILE THE FRAME IS ALIVE,
//! at the resolver's invoke arm: the send spells no `.call` selector, so no
//! resolution-side capture records the member, and a finalize-time
//! re-derivation cannot see the frame. The un-credited shape surfaced as a
//! spurious `unused_element` on the extension's `call` (the conformance
//! row `FunctionExpressionInvocationTest/
//! test_expression_interfaceType_nullable_hasCall#0`, whose oracle
//! expectation is empty and whose only divergence was that extra row).

use super::test_rig::{assert_silent, code_names, report_ops};

/// The corpus pin (unnamed extension — the whole face checks like a
/// private declaration, so the un-credited `call` reported): the applied
/// extension's `call` method is credited at the invoke, and the face stays
/// silent on every row.
#[test]
fn invoked_unnamed_extension_call_is_not_unused() {
    let report = report_ops(
        r#"
void f(int? a) {
  a();
}
extension on int? {
  int call() => 0;
}
"#,
    );
    assert_silent(&report);
}

/// The named-private extension variant: its members are declaration sites
/// (the `DeclKind::Member` face-join arm), spared only by the same
/// element-attributed credit set. The invoke credits the applied face and
/// the member stays silent.
#[test]
fn invoked_named_private_extension_call_is_not_unused() {
    let report = report_ops(
        r#"
void f(int? a) {
  a();
}
extension _N on int? {
  int call() => 0;
}
"#,
    );
    assert_silent(&report);
}

/// The credit rides the INVOKE event only: the same extension `call` with
/// no invocation still reports `unused_element` — the invoke arm's credit
/// must not over-fire into a declaration-free fixture.
#[test]
fn uninvoked_extension_call_still_reports_unused() {
    let report = report_ops(
        r#"
void f(int? a) {}
extension on int? {
  int call() => 0;
}
"#,
    );
    assert_eq!(
        code_names(&report),
        ["unused_element"],
        "{:?}",
        report.diagnostics
    );
}

/// The class arm: a resolved `call` method joins the name-level use pool
/// at the invoke, and the send stays callable — the two-stage arm's
/// callable verdict for the class-member shape is unchanged.
#[test]
fn invoked_class_call_method_stays_silent() {
    let report = report_ops(
        r#"
class C {
  int call() => 0;
}
void f(C c) {
  c();
}
"#,
    );
    assert_silent(&report);
}
