//! The #3762 pins: the binary-operator right-operand `void` face of
//! `use_of_void_result`. Upstream, `checkForArgumentTypeNotAssignable`
//! (error_detection_helpers.dart:42-60) asks `checkForUseOfVoidResult`
//! FIRST whenever the expected parameter type is non-`void` — a `void`
//! actual into a non-`void` formal reports `use_of_void_result` at the
//! operand's use-site span and answers no `argument_type_not_assignable`.
//! The `==`/`!=` invocation's declared formal is `Object` upstream
//! (`Object.==(Object other)`), and every user operator's declared
//! parameter lands the same ask, so `3 == v()`, `x != v()`, `3 + v()` and
//! `a - v()` all fire the void-operand face.
//!
//! The suppression this cluster replaces — `check_binary_argument` and
//! `check_equals_argument` both early-returned on `right_ty == Void`
//! without reporting — silently dropped every binary right operand whose
//! type was `void`. The oracle's emit order is the inverse: void
//! short-circuits the assignability ask, never falls through it.
//!
//! # The contract
//!
//! - `K op v()` where `op` resolves to a declared binary (`==`, `!=`, the
//!   user-declared `+`/`-`/`*`/comparison operators) — the right operand
//!   reports `use_of_void_result` at the operand's use-site span
//!   (`.at(methodName)` for an invoked operand — the `v` of `v()`).
//! - `K op x` where `x` is a `void`-typed read (parameter, local, getter)
//!   — the same face, same anchor.
//! - `v() op K` (void LEFT operand) still fires through the separate
//!   `visitBinaryExpression` `checkForUseOfVoidResult(node.leftOperand)`
//!   face — unchanged.
//! - A `void` parameter into a `void` formal (`useAsVoid(x)` for
//!   `void useAsVoid(void x)`) stays silent: `checkForArgumentTypeNot
//!   Assignable` guards `expectedStaticType is! VoidType` before asking.
//! - The `dynamic` operand still suppresses — `isAssignableTo(dynamic, X)`
//!   holds upstream and the const-eval verdict owns the row.

use crate::diagnose::test_rig::{code_names, report_ops, rows};

/// The oracle's `useOfVoidResult` message spelling (messages.yaml:19638).
const VOID_MSG: &str = "This expression has a type of 'void' so its value can't be used.";

/// Asserts the fixture's `use_of_void_result` rows sit at exactly
/// `expected` `(offset, length)` pairs (offsets relative to the fixture's
/// own text, `CORE_PRELUDE` shifted). Other diagnostics a fixture's shape
/// invites (`unused_element`, `unused_local_variable`) are tolerated.
fn assert_void_rows(
    report: &super::FileReport,
    fixture: &str,
    expected: &[(usize, u32)],
) {
    let mut got: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.message == VOID_MSG)
        .map(|d| (d.range.offset, d.range.length))
        .collect();
    got.sort_unstable();
    let prelude = crate::diagnose::test_rig::CORE_PRELUDE.len() as u32;
    let mut want: Vec<(u32, u32)> = expected
        .iter()
        .map(|(offset, length)| (prelude + *offset as u32, *length))
        .collect();
    want.sort_unstable();
    assert_eq!(got, want, "fixture={fixture:?}\nrows={:?}", rows(report));
}

// —— `==` right operand: an invoked `void` callee anchors at the
// —— methodName `v` (upstream `.at(methodName)` on `MethodInvocation`).
#[test]
fn ticket_3762_equals_right_invoked_void() {
    let fixture = "void v() {}\nvoid main() {\n  1 == v();\n}";
    let report = report_ops(fixture);
    let at = fixture.rfind("v()").unwrap();
    assert_void_rows(&report, fixture, &[(at, 1)]);
}

// —— `==` right operand: a `void`-typed local read anchors at `x`.
// —— The `void x = v()` init is silent upstream (`void` to `void` is no
// —— misuse); the operand read alone reports.
#[test]
fn ticket_3762_equals_right_bare_void_local() {
    let fixture = "void v() {}\nvoid main() {\n  void x = v();\n  1 == x;\n}";
    let report = report_ops(fixture);
    let use_at = fixture.rfind("x").unwrap();
    assert_void_rows(&report, fixture, &[(use_at, 1)]);
}

// —— `!=` right operand is the same face (upstream `Object.==` handles
// —— both spellings).
#[test]
fn ticket_3762_not_equals_right_invoked_void() {
    let fixture = "void v() {}\nvoid main() {\n  1 != v();\n}";
    let report = report_ops(fixture);
    let at = fixture.rfind("v()").unwrap();
    assert_void_rows(&report, fixture, &[(at, 1)]);
}

// —— Both-`void` `==`: the left arm reports through `visitBinary
// —— Expression`'s `checkForUseOfVoidResult(leftOperand)`, the right
// —— through `checkForArgumentTypeNotAssignable` — two independent emit
// —— sites, two rows. The `void`-to-`void` inits are silent upstream.
#[test]
fn ticket_3762_equals_both_void_reports_both() {
    let fixture = "void v() {}\nvoid main() {\n  void x = v();\n  void y = v();\n  x == y;\n}";
    let report = report_ops(fixture);
    let x_use = fixture.find("x == y").unwrap();
    let y_use = fixture.find("x == y").unwrap() + 5;
    assert_void_rows(&report, fixture, &[(x_use, 1), (y_use, 1)]);
}

// —— `+` right operand: a `void` callee against `num.+(num)`.
#[test]
fn ticket_3762_plus_right_invoked_void() {
    let fixture = "void v() {}\nvoid main() {\n  1 + v();\n}";
    let report = report_ops(fixture);
    let at = fixture.rfind("v()").unwrap();
    assert_void_rows(&report, fixture, &[(at, 1)]);
}

// —— `-` right operand: a `void` local against `num.-(num)`.
#[test]
fn ticket_3762_minus_right_bare_void_local() {
    let fixture = "void v() {}\nvoid main() {\n  void x = v();\n  1 - x;\n}";
    let report = report_ops(fixture);
    let use_at = fixture.rfind("x").unwrap();
    assert_void_rows(&report, fixture, &[(use_at, 1)]);
}

// —— `*` right operand: same `num.*(num)` face.
#[test]
fn ticket_3762_times_right_invoked_void() {
    let fixture = "void v() {}\nvoid main() {\n  1 * v();\n}";
    let report = report_ops(fixture);
    let at = fixture.rfind("v()").unwrap();
    assert_void_rows(&report, fixture, &[(at, 1)]);
}

// —— Void LEFT of `==` still fires (the separate `visitBinaryExpression`
// —— left-operand face is untouched).
#[test]
fn ticket_3762_equals_left_invoked_void() {
    let fixture = "void v() {}\nvoid main() {\n  v() == 1;\n}";
    let report = report_ops(fixture);
    let at = fixture.rfind("v()").unwrap();
    assert_void_rows(&report, fixture, &[(at, 1)]);
}

// —— Negative control: `A() == x` against `bool operator ==(void o)` —
// —— the `void` formal arm stays silent (upstream guards
// —— `expectedStaticType is! VoidType` before asking). The `void x = v()`
// —— init is silent as well (`void` to `void` is no misuse), so the
// —— fixture emits zero `use_of_void_result` rows.
#[test]
fn ticket_3762_void_formal_right_operand_silent() {
    let fixture = "void v() {}\nclass A { bool operator ==(void o) => true; }\n\
                   void main() {\n  void x = v();\n  A() == x;\n}";
    let report = report_ops(fixture);
    assert_void_rows(&report, fixture, &[]);
}

// —— Negative control: `useAsVoid(v())` — same `void`-formal guard, no
// —— emit at all (no `x` declaration, nothing else to report).
#[test]
fn ticket_3762_void_formal_invoked_right_silent() {
    let fixture = "void v() {}\nclass A { bool operator ==(void o) => true; }\n\
                   void main() {\n  A() == v();\n}";
    let report = report_ops(fixture);
    let void_rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.message == VOID_MSG)
        .collect();
    assert!(
        void_rows.is_empty(),
        "void-formal right operand must stay silent: {:?}",
        rows(&report)
    );
}

// —— The `identical` helper's argument — a plain argument face, never a
// —— binary-operator's own check — still reports through the same
// —— `checkForArgumentTypeNotAssignable` short-circuit.
#[test]
fn ticket_3762_identical_argument_void() {
    let fixture = "void v() {}\nvoid identical(Object? a, Object? b) {}\n\
                   void main() {\n  identical(1, v());\n}";
    let report = report_ops(fixture);
    let at = fixture.rfind("v()").unwrap();
    assert_void_rows(&report, fixture, &[(at, 1)]);
}
