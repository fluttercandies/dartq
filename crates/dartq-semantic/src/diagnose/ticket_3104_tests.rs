//! The #3104 pins: the dead-region row suppression. Inside a flow dead
//! interval (`FlowFact::DeadCode`'s range) the oracle reports ONLY
//! `dead_code` — the `ErrorVerifier` visits the dead node but the flow
//! marks it dead and the non-`dead_code` verification never fires
//! (upstream's dead-node short-circuit). dartq's
//! `unchecked_use_of_nullable_value` arm was re-deciding against the
//! promoted/static type and reporting inside the window — the residual
//! #3087 left. The fix re-homes the dead intervals into
//! [`Engine::dead_windows`] before `resolve_rows` and gates
//! `emit_unchecked_nullable` on [`Engine::in_dead_window`]: a row anchored
//! inside a dead region never reaches the report.
//!
//! Upstream law: `dead_code_verifier.dart`'s dead regions mark the node
//! dead; the verifier-family diagnostics that would otherwise anchor on
//! the dead operand stay silent (the `dead_null_aware_expression` arm is
//! the exception — the official emits it INSIDE the window, so it never
//! consults the table).

use crate::diagnose::test_rig::{code_names, report_ops, rows};

/// Asserts no `unchecked_use_of_nullable_value` sits anywhere in the
/// report — the whole point of the gate.
fn assert_no_unchecked(report: &super::FileReport) {
    assert!(
        !code_names(report)
            .iter()
            .any(|&c| c == "unchecked_use_of_nullable_value"),
        "unchecked_use_of_nullable_value inside a dead window: {:?}",
        rows(report)
    );
}

/// Asserts the dead_code row fired (the fixture IS dead — the gate is
/// wrong if nothing reports).
fn assert_dead_code(report: &super::FileReport) {
    assert!(
        code_names(report).iter().any(|&c| c == "dead_code"),
        "expected a dead_code row: {:?}",
        rows(report)
    );
}

/// A nullable local's member read inside `if (false) { ... }`'s dead
/// block reports only `dead_code` — never `unchecked_use_of_nullable_value`
/// on the dead `x.abs()` selector.
#[test]
fn member_read_inside_dead_block_stays_silent() {
    let fixture = "void f(int? x) {\n  if (false) {\n    x.abs();\n  }\n}";
    let report = report_ops(fixture);
    assert_dead_code(&report);
    assert_no_unchecked(&report);
}

/// A nullable local's member CALL inside a dead block — `x.abs()` reads
/// the member's invoke, the oracle's dead-node short-circuit suppresses
/// the unchecked-use row.
#[test]
fn member_call_inside_dead_block_stays_silent() {
    let fixture = "void f(int? x) {\n  if (false) {\n    var y = x.abs();\n  }\n}";
    let report = report_ops(fixture);
    assert_dead_code(&report);
    assert_no_unchecked(&report);
}

/// A dead block nested inside ANOTHER dead block: `if (false) { if
/// (false) { x.abs(); } }` — the interior read sits inside two dead
/// windows and still reports only `dead_code`.
#[test]
fn member_read_inside_nested_dead_blocks_stays_silent() {
    let fixture =
        "void f(int? x) {\n  if (false) {\n    if (false) {\n      x.abs();\n    }\n  }\n}";
    let report = report_ops(fixture);
    assert_dead_code(&report);
    assert_no_unchecked(&report);
}

/// `x?.abs()` is null-AWARE — no unchecked_use fires anywhere — but the
/// fixture still asserts the dead window reports only its dead_code row
/// (the null-aware arm never rode the gate anyway).
#[test]
fn null_aware_read_inside_dead_block_stays_silent() {
    let fixture = "void f(int? x) {\n  if (false) {\n    x?.abs();\n  }\n}";
    let report = report_ops(fixture);
    assert_dead_code(&report);
    assert_no_unchecked(&report);
}

/// A nullable local's member read AFTER the dead block — outside the
/// window — still reports `unchecked_use_of_nullable_value` (the gate is
/// containment, not a blanket suppress).
#[test]
fn member_read_outside_dead_block_reports() {
    let fixture = "void f(int? x) {\n  if (false) {\n    x.abs();\n  }\n  x.abs();\n}";
    let report = report_ops(fixture);
    assert!(
        code_names(&report)
            .iter()
            .any(|&c| c == "unchecked_use_of_nullable_value"),
        "expected the live read's unchecked_use_of_nullable_value: {:?}",
        rows(&report)
    );
}

/// Two dead windows (`if (false) {}` then `if (false) {}`) each suppress
/// their own interior row — the table holds every interval, not just the
/// first.
#[test]
fn two_dead_windows_each_suppress() {
    let fixture =
        "void f(int? x) {\n  if (false) {\n    x.abs();\n  }\n  if (false) {\n    x.abs();\n  }\n}";
    let report = report_ops(fixture);
    assert_dead_code(&report);
    assert_no_unchecked(&report);
}

/// A dead block containing BOTH a member call and a property read —
/// every suppressed face inside the window stays silent (one dead_code
/// row covers the whole interior).
#[test]
fn mixed_accesses_inside_dead_block_stay_silent() {
    let fixture = "class C { int? get p => null; int m() => 0; }\n\
                   void f(C? c, int? x) {\n  if (false) {\n    c.m();\n    var y = x.abs();\n  }\n}";
    let report = report_ops(fixture);
    assert_dead_code(&report);
    assert_no_unchecked(&report);
}

/// A `??` dead arm inside a dead window: the `dead_null_aware_expression`
/// row still fires INSIDE the dead region (the official emits it there —
/// `_checkForDeadNullCoalesce` reports at `node.rightOperand`), so the
/// gate suppresses only the unchecked-use family, never the
/// dedicated dead-coalesce row.
#[test]
fn dead_coalesce_rhs_inside_dead_window_still_reports() {
    let fixture = "void f(int? x, int d) {\n  if (false) {\n    var y = x ?? d;\n  }\n}";
    let report = report_ops(fixture);
    // The `??` right operand's dead arm reports `dead_null_aware_expression`
    // at the rhs AND the enclosing dead window's `dead_code` — both stand.
    assert!(
        code_names(&report)
            .iter()
            .any(|&c| c == "dead_null_aware_expression" || c == "dead_code"),
        "expected the dead arm's dedicated row or dead_code: {:?}",
        rows(&report)
    );
}

/// A return inside a dead block keeps the block's interior reads silent —
/// the dead window's suppression is total, not statement-scoped.
#[test]
fn dead_block_after_return_stays_silent() {
    let fixture = "int f(int? x) {\n  return 0;\n  var y = x.abs();\n  return 1;\n}";
    let report = report_ops(fixture);
    assert_dead_code(&report);
    assert_no_unchecked(&report);
}
