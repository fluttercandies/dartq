//! #3231 (wave-B cluster C) — the compound receiver's static type.
//!
//! A conditional (`c ? a : b`) and a null-aware binary (`a ?? b`) are
//! resolver nodes with a `staticType` of their own: the resolver stamps
//! `leastUpperBound(thenType, elseType)` (`conditional_expression_
//! resolver`, `resolver.dart:2426`) and `LUB(NonNull(lhs), rhs)` for
//! `??` (`binary_expression_resolver.dart:69-109`). A member or operator
//! applied to such a composite therefore resolves against the JOIN, not
//! against nothing: `(c ? a : b).gone` reports `undefined_getter` naming
//! the joined type, and so does the `??` twin.
//!
//! The engine's row model keys member/operator faces by receiver SPAN and
//! stages no fact for a composite expression. A conditional's folded row
//! carries the join, but the `??` fold leaves the enclosing group row's
//! range as its closing paren alone, so the receiver resolved `Unknown`
//! and every member/operator check on a `(a ?? b)` receiver was skipped —
//! `Engine::group_expression_face` (row_resolution.rs) closes that: the
//! group's static type is its inner expression's own, re-derived through
//! the same compound join.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block (`diagnose.rs`
//! is a live-lane file out of this ticket's write scope; `tests.rs` keeps
//! the module inside the `diagnose` subtree so
//! `crate::diagnose::test_rig` resolves).

use crate::diagnose::test_rig::{code_names, report_ops, rows};

/// The `class C` fixture head both receiver shapes read.
const C_FIXTURE: &str = "class C { int x = 0; }\n";

/// `(c ? a : b).gone` — the join of two `C` arms is `C`.
#[test]
fn ticket_3231_conditional_receiver_absent_getter_reports() {
    let fixture = format!("{C_FIXTURE}void g(bool c, C a, C b) {{\n  (c ? a : b).gone;\n}}");
    let report = report_ops(&fixture);
    let shown = rows(&report);
    assert_eq!(code_names(&report), ["undefined_getter"], "rows: {shown:#?}");
    assert_eq!(
        report
            .diagnostics
            .iter()
            .find(|d| d.code.name == "undefined_getter")
            .expect("getter row")
            .message,
        "The getter 'gone' isn't defined for the type 'C'."
    );
}

/// `(a ?? b).gone` — the `??` join of `C?` and `C` is `C`.
#[test]
fn ticket_3231_if_null_receiver_absent_getter_reports() {
    let fixture = format!("{C_FIXTURE}void g(C? a, C b) {{\n  (a ?? b).gone;\n}}");
    let report = report_ops(&fixture);
    let shown = rows(&report);
    assert_eq!(code_names(&report), ["undefined_getter"], "rows: {shown:#?}");
    assert_eq!(
        report
            .diagnostics
            .iter()
            .find(|d| d.code.name == "undefined_getter")
            .expect("getter row")
            .message,
        "The getter 'gone' isn't defined for the type 'C'."
    );
}

/// The nested group reaches the same face: `((a ?? b)).m` is the group of
/// a group, and each level is its inner expression's type.
#[test]
fn ticket_3231_nested_group_receiver_reports() {
    let fixture = format!("{C_FIXTURE}void g(C? a, C b) {{\n  ((a ?? b)).gone;\n}}");
    let report = report_ops(&fixture);
    assert_eq!(
        code_names(&report),
        ["undefined_getter"],
        "rows: {:#?}",
        rows(&report)
    );
}

/// The operator face of the same composites: `C` declares no `+`.
#[test]
fn ticket_3231_compound_receiver_operator_reports() {
    for fixture in [
        format!("{C_FIXTURE}void g(bool c, C a, C b) {{\n  (c ? a : b) + 1;\n}}"),
        format!("{C_FIXTURE}void g(C? a, C b) {{\n  (a ?? b) + 1;\n}}"),
    ] {
        let report = report_ops(&fixture);
        let shown = rows(&report);
        assert_eq!(
            code_names(&report),
            ["undefined_operator"],
            "fixture: {fixture:#?}\nrows: {shown:#?}"
        );
        assert_eq!(
            report
                .diagnostics
                .iter()
                .find(|d| d.code.name == "undefined_operator")
                .expect("operator row")
                .message,
            "The operator '+' isn't defined for the type 'C'."
        );
    }
}

/// A member the joined type DOES declare stays silent — the face must not
/// invent rows on a resolvable composite, and an `Object` member
/// (`toString`) resolves on every class.
#[test]
fn ticket_3231_compound_receiver_declared_member_silent() {
    for fixture in [
        format!("{C_FIXTURE}void g(bool c, C a, C b) {{\n  (c ? a : b).x;\n}}"),
        format!("{C_FIXTURE}void g(C? a, C b) {{\n  (a ?? b).x;\n}}"),
        format!("{C_FIXTURE}void g(C? a, C b) {{\n  (a ?? b).toString();\n}}"),
    ] {
        let report = report_ops(&fixture);
        assert!(
            report.diagnostics.is_empty(),
            "fixture: {fixture:#?}\nrows: {:#?}",
            rows(&report)
        );
    }
}
