//! The #3779 pins: `use_of_void_result` on `??`'s LEFT operand anchors the
//! operand's use-site span, never the raw row (upstream
//! `visitBinaryExpression` → `checkForUseOfVoidResult(node.leftOperand)`,
//! error_verifier.dart:482 → error_detection_helpers.dart:245-258). The
//! `end_if_null` seam emitted `left.range` verbatim, so every shape whose
//! row is narrower than the operand's expression face under-spanned:
//!
//! - `value() ?? d` anchored the callee token `value` — correct by
//!   coincidence (a declared-method send anchors `methodName`), but only
//!   because the row happens to hold exactly that span;
//! - `a.x ?? d` anchored the selector `x` — the oracle spans the whole
//!   member access `a.x`;
//! - `(x) ?? d` anchored the group row's `)` token — the oracle covers
//!   the parens `(x)`;
//! - `(true ? x : x) ?? d` anchored the else-arm leaf `x` — the oracle
//!   covers the whole parenthesized conditional `(true ? x : x)`.
//!
//! The fix routes the emit through `emit_use_of_void_result`, whose
//! `use_site_range` already answers all four faces (`methodName` for a
//! declared send, receiver-through-selector for a member read, the
//! enclosing-paren walk for a `)`-anchored group, and the group-row
//! expansion for a leaf nested inside one) — plus the span-identity dedup
//! the `fold_binary` seam already shares.
//!
//! # The contract
//!
//! - `value() ?? d` reports `value` (len 5 — the `methodName` anchor, NOT
//!   `value()`).
//! - `a.xmemb ?? d` reports `a.xmemb` (receiver start through selector
//!   end).
//! - `(x) ?? d` / `(true ? x : x) ?? d` report the whole parenthesized
//!   group, parens included.
//! - A bare void leaf (`x ?? d`) still reports at the leaf's own span.

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

// —— Declared-function send LHS: `value() ?? 499` anchors `value` — the
// —— `methodName` face (probed oracle 3.13.7: `3|3|5`), never `value()`.
#[test]
fn ticket_3779_if_null_left_call_anchors_method_name() {
    let fixture = "void value() {}\nvoid main() {\n  value() ?? 499;\n}";
    let report = report_ops(fixture);
    let at = fixture.rfind("value").unwrap();
    assert_void_rows(&report, fixture, &[(at, 5)]);
}

// —— Member-read LHS: `a.xmemb ?? 499` anchors `a.xmemb` — receiver start
// —— through selector end (probed oracle: `5|3|7`), never the bare `xmemb`
// —— selector.
#[test]
fn ticket_3779_if_null_left_member_anchors_access() {
    let fixture = "void v() {}\nclass A { void xmemb = v(); }\n\
                   void main() {\n  A a = A();\n  a.xmemb ?? 499;\n}";
    let report = report_ops(fixture);
    let at = fixture.find("a.xmemb").unwrap();
    assert_void_rows(&report, fixture, &[(at, "a.xmemb".len() as u32)]);
}

// —— Parenthesized LHS: `(longername) ?? 499` anchors `(longername)` —
// —— the group's parens included (probed oracle: `4|3|12`), never the
// —— group row's `)` token.
#[test]
fn ticket_3779_if_null_left_paren_anchors_group() {
    let fixture = "void v() {}\nvoid main() {\n  void longername = v();\n  (longername) ?? 499;\n}";
    let report = report_ops(fixture);
    let at = fixture.find("(longername)").unwrap();
    assert_void_rows(&report, fixture, &[(at, "(longername)".len() as u32)]);
}

// —— Parenthesized-conditional LHS: `(true ? x : x) ?? null` anchors the
// —— whole group (probed oracle: `4|3|14`), never the else-arm leaf `x`.
#[test]
fn ticket_3779_if_null_left_paren_conditional_anchors_group() {
    let fixture = "void v() {}\nvoid main() {\n  void x = v();\n  (true ? x : x) ?? null;\n}";
    let report = report_ops(fixture);
    let at = fixture.find("(true ? x : x)").unwrap();
    assert_void_rows(&report, fixture, &[(at, "(true ? x : x)".len() as u32)]);
}

// —— Negative direction: a bare void leaf `x ?? 499` still reports — at
// —— the leaf's own span (the use-site answer for a plain read is the
// —— read itself).
#[test]
fn ticket_3779_if_null_left_bare_void_still_flagged() {
    let fixture = "void v() {}\nvoid main() {\n  void x = v();\n  x ?? 499;\n}";
    let report = report_ops(fixture);
    let at = fixture.rfind("x").unwrap();
    assert_void_rows(&report, fixture, &[(at, 1)]);
}
