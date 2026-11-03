//! The #3767 pins: the `use_of_void_result` anchor contract on the operand
//! row's use-site span (upstream `checkForUseOfVoidResult`,
//! error_detection_helpers.dart:245-258 — `.at(expression)` on the operand
//! node, `.at(methodName)` only for a declared-method send). Three folding
//! defects broke that contract and each minted rows the oracle never
//! emits:
//!
//! - a ParenthesizedExpression operand folded to a `)`-anchored row — or a
//!   leaf row nested inside one — so `!(x)`, `dynamic z = (x)`, `while
//!   ((x))`, `<Object?>[(x)]`, `(x) && true` and `(true ? x : x) is T`
//!   reads reported the inner leaf's span instead of the group's (`(x)`
//!   reports `(x)`, `((x))` reports `((x))`, `(true ? x : x)` reports the
//!   whole parenthesized conditional);
//! - `fold_conditional` measured the `?:` node's tail from the token the
//!   listener handed in — for a bare-read or member-read else arm that
//!   token is the send-leaf FOLLOWER (`;`/`,`/`)`/`]`/`}`), so the folded
//!   `Value` row swallowed the following delimiter and reported one column
//!   past the arm (`true ? x : x` flagged `x;`);
//! - `callable_call_ret` re-dispatched a declared-method/member send whose
//!   declared return type happened to name a callable class: `f()` on a
//!   `C`-typed send where `C` declares `void call()` read the SEND's
//!   static type as `void` and flagged the operand — upstream only a
//!   function-expression invocation (a value callee) runs `.call`. The
//!   same misfire at the send-close fold can store `void` on the row; a
//!   declared callee's send re-reads its declared return instead.
//!
//! # The contract
//!
//! - A void operand's row reports at its expression's own span — parens
//!   included when the operand is parenthesized, never one byte past a
//!   leaf arm's last token.
//! - A send whose callee resolves to a declared method or function
//!   reports that method's declared return type — a callable-class-typed
//!   return never re-dispatches through `call`.
//! - A send through a `void Function()`-typed VALUE (a field, getter, or
//!   function-typed formal/local) still fires — the function-expression
//!   invocation face is unchanged.

use crate::diagnose::test_rig::{report_ops, rows};

/// The oracle's `useOfVoidResult` message spelling (messages.yaml:19638).
const VOID_MSG: &str = "This expression has a type of 'void' so its value can't be used.";

/// Asserts the fixture's `use_of_void_result` rows sit at exactly
/// `expected` `(offset, length)` pairs (offsets relative to the fixture's
/// own text, `CORE_PRELUDE` shifted). Other diagnostics a fixture's shape
/// invites (`unused_local_variable`, `dead_code`, type-check faces) are
/// tolerated.
fn assert_void_rows(
    report: &crate::diagnose::FileReport,
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

// —— `is` operand: `(true ? x : x) is Object?` anchors the whole
// —— parenthesized group (probed oracle 3.13.7: `3|14`), never the inner
// —— arm leaf.
#[test]
fn ticket_3767_is_operand_paren_conditional_anchors_group() {
    let fixture = "void v() {}\nvoid main() {\n  void x = v();\n  (true ? x : x) is Object?;\n}";
    let report = report_ops(fixture);
    let at = fixture.find("(true ? x : x)").unwrap();
    assert_void_rows(&report, fixture, &[(at, "(true ? x : x)".len() as u32)]);
}

// —— `dynamic` init RHS: `true ? x : x` spans through the second `x`,
// —— never the following `;` — the listener's `end` token is the
// —— send-leaf follower (probed oracle: `15|12`).
#[test]
fn ticket_3767_conditional_tail_stops_at_else_arm() {
    let fixture = "void v() {}\nvoid main() {\n  void x = v();\n  dynamic z = true ? x : x;\n}";
    let report = report_ops(fixture);
    let at = fixture.find("true ? x : x").unwrap();
    assert_void_rows(&report, fixture, &[(at, "true ? x : x".len() as u32)]);
}

// —— `while` condition: `while (true ? 499 : x)` anchors the conditional
// —— (the statement's own parens are syntax, not a group — probed oracle:
// —— `10|14`).
#[test]
fn ticket_3767_while_condition_anchors_conditional() {
    let fixture = "void v() {}\nvoid main() {\n  void x = v();\n  while (true ? 499 : x) {}\n}";
    let report = report_ops(fixture);
    let at = fixture.find("true ? 499 : x").unwrap();
    assert_void_rows(&report, fixture, &[(at, "true ? 499 : x".len() as u32)]);
}

// —— Map-literal value: `{4: true ? x : x}` anchors the conditional, not
// —— the `x` leaf nor the following `}` (probed oracle: `33|12`).
#[test]
fn ticket_3767_map_value_anchors_conditional() {
    let fixture = "void v() {}\nvoid main() {\n  void x = v();\n  var m = <dynamic, dynamic>{4: true ? x : x};\n}";
    let report = report_ops(fixture);
    let at = fixture.find("true ? x : x").unwrap();
    assert_void_rows(&report, fixture, &[(at, "true ? x : x".len() as u32)]);
}

// —— Prefix `!` on a parenthesized void read: `!(x)` anchors `(x)` —
// —— parens included (probed oracle: the group span).
#[test]
fn ticket_3767_bang_paren_operand_anchors_group() {
    let fixture = "void v() {}\nvoid main() {\n  void x = v();\n  bool b = !(x);\n}";
    let report = report_ops(fixture);
    let at = fixture.find("(x)").unwrap();
    assert_void_rows(&report, fixture, &[(at, 3)]);
}

// —— `dynamic` init on a parenthesized void read: `dynamic z = (x)`
// —— anchors `(x)` (probed oracle: the group span).
#[test]
fn ticket_3767_dynamic_init_paren_anchors_group() {
    let fixture = "void v() {}\nvoid main() {\n  void x = v();\n  dynamic z = (x);\n}";
    let report = report_ops(fixture);
    let at = fixture.find("(x)").unwrap();
    assert_void_rows(&report, fixture, &[(at, 3)]);
}

// —— `while ((x))` anchors the INNER group `(x)` — the condition parens
// —— are the statement's own (probed oracle: `(x)` span).
#[test]
fn ticket_3767_while_paren_condition_anchors_inner_group() {
    let fixture = "void v() {}\nvoid main() {\n  void x = v();\n  while ((x)) {}\n}";
    let report = report_ops(fixture);
    let at = fixture.find("(x)").unwrap();
    assert_void_rows(&report, fixture, &[(at, 3)]);
}

// —— List element: `<Object?>[(x)]` anchors `(x)` (probed oracle: the
// —— group span).
#[test]
fn ticket_3767_list_element_paren_anchors_group() {
    let fixture = "void v() {}\nvoid main() {\n  void x = v();\n  var l = <Object?>[(x)];\n}";
    let report = report_ops(fixture);
    let at = fixture.find("(x)").unwrap();
    assert_void_rows(&report, fixture, &[(at, 3)]);
}

// —— `&&` left operand: `(x) && true` anchors `(x)` (probed oracle: the
// —— group span).
#[test]
fn ticket_3767_and_left_paren_anchors_group() {
    let fixture = "void v() {}\nvoid main() {\n  void x = v();\n  bool c = (x) && true;\n}";
    let report = report_ops(fixture);
    let at = fixture.find("(x)").unwrap();
    assert_void_rows(&report, fixture, &[(at, 3)]);
}

// —— `==` right operand: `3 == (x)` anchors `(x)` (probed oracle: the
// —— group span).
#[test]
fn ticket_3767_equals_right_paren_anchors_group() {
    let fixture = "void v() {}\nvoid main() {\n  void x = v();\n  bool d = 3 == (x);\n}";
    let report = report_ops(fixture);
    let at = fixture.find("(x)").unwrap();
    assert_void_rows(&report, fixture, &[(at, 3)]);
}

// —— Callable-class return is NOT re-dispatched for a declared-method
// —— send: `C m()` declared on `D`, invoked `d.m()` where `C` has
// —— `void call()` — the send's static type is `C` (the declared
// —— return), never `void`. Upstream fires no `use_of_void_result`.
#[test]
fn ticket_3767_declared_method_callable_return_not_flagged() {
    let fixture = "class C { void call() {} }\nclass D { C m() => C(); }\n\
                   void use(Object o) {}\nvoid main() {\n  D d = D();\n  use(d.m());\n}";
    let report = report_ops(fixture);
    let got: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.message == VOID_MSG)
        .collect();
    assert!(
        got.is_empty(),
        "declared-method send on a callable-class return must not flag: {:?}",
        rows(&report)
    );
}

// —— The bare-callee arm of the same gate: `C f()` top-level, invoked
// —— `f()` where `C` declares `void call()` — the send's static type is
// —— `C`, no void row.
#[test]
fn ticket_3767_declared_function_callable_return_not_flagged() {
    let fixture = "class C { void call() {} }\nC f() => C();\n\
                   void use(Object o) {}\nvoid main() {\n  use(f());\n}";
    let report = report_ops(fixture);
    let got: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.message == VOID_MSG)
        .collect();
    assert!(
        got.is_empty(),
        "declared-function send on a callable-class return must not flag: {:?}",
        rows(&report)
    );
}

// —— A static-method send on a callable-class return is the same face:
// —— `D.m()` reads `C`, not `void` (the send-close fold can store a
// —— misfired implicit-`call` `void` on the row — the operand read
// —— re-derives the declared return).
#[test]
fn ticket_3767_static_method_callable_return_not_flagged() {
    let fixture = "class C { void call() {} }\nclass D { static C m() => C(); }\n\
                   void use(Object o) {}\nvoid main() {\n  use(D.m());\n}";
    let report = report_ops(fixture);
    let got: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.message == VOID_MSG)
        .collect();
    assert!(
        got.is_empty(),
        "static-method send on a callable-class return must not flag: {:?}",
        rows(&report)
    );
}

// —— Positive control: a `void Function()`-typed VALUE send still fires —
// —— `f()` where `f` is a `void Function()` local is a function-
// —— expression invocation whose static type IS `void`; the `!` operand
// —— is the parenthesized group `(f())`, so the oracle anchors the group.
#[test]
fn ticket_3767_value_callee_void_function_still_flagged() {
    let fixture = "void use(Object o) {}\nvoid main() {\n  void Function() f = () {};\n  bool b = !(f());\n}";
    let report = report_ops(fixture);
    let at = fixture.find("(f())").unwrap();
    assert_void_rows(&report, fixture, &[(at, 5)]);
}

// —— Positive control: a member send through a `void Function()`-typed
// —— field is the same function-expression invocation face (the group
// —— `(d.f())` is the operand).
#[test]
fn ticket_3767_member_field_void_function_still_flagged() {
    let fixture = "class D { void Function() f = () {}; }\n\
                   void main() {\n  D d = D();\n  bool b = !(d.f());\n}";
    let report = report_ops(fixture);
    let at = fixture.find("(d.f())").unwrap();
    assert_void_rows(&report, fixture, &[(at, 7)]);
}

// —— Positive control: a plain `void`-typed local read under `!` still
// —— reports at the leaf's own span (the regression the whole cluster
// —— must not disturb).
#[test]
fn ticket_3767_bare_void_read_still_flagged() {
    let fixture = "void v() {}\nvoid main() {\n  void x = v();\n  bool b = !x;\n}";
    let report = report_ops(fixture);
    let at = fixture.rfind("x").unwrap();
    assert_void_rows(&report, fixture, &[(at, 1)]);
}
