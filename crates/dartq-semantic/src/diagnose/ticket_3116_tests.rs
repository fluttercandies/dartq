//! The #3116 pins: the `??`/`??=` dead-coalesce promotion re-decision —
//! `_checkForDeadNullCoalesce` reads the operand's FLOW-aware static type
//! (`node.leftOperand.staticType` for `??`, the target's
//! `readResolution.type` for `??=`; upstream Dart SDK source,
//! pkg/analyzer/lib/src/generated/error_verifier.dart:4733-4739), so a
//! declared-nullable local promoted non-null by the flow still anchors the
//! dead face. The flow read types materialize only at the teardown join
//! (`Engine::read_types` fills after pass 1 — the walk sees them empty),
//! so the walk-time declared-type verdict misses and `coalesce_staged`
//! carries `(operand read span, report range)` to the
//! `flush_null_aware_staged` teardown, which emits
//! `dead_null_aware_expression` when `promotions.promoted(read)` is
//! strictly non-nullable. The declared-non-null arm stays a walk-time
//! decision (the flow's answer is the same); only the walk-miss rides the
//! staged join.

use crate::diagnose::test_rig::{code_names, report_ops, rows};

/// Asserts one `dead_null_aware_expression` sits at `offset`, tolerating
/// the co-diagnostics a declared-but-unread local (`unused_local_variable`)
/// or the dead `??` arm itself (`dead_code`) adds around it.
fn assert_dead_coalesce_at(report: &super::FileReport, offset: usize) {
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "dead_null_aware_expression"
                && d.range.offset as usize == offset),
        "no dead_null_aware_expression at {offset}: {:?}",
        rows(report)
    );
}

/// The offset of `needle`'s LAST occurrence inside `fixture` shifted past
/// the prelude — `report_ops` prepends
/// [`crate::diagnose::test_rig::CORE_PRELUDE`]. The last-occurrence read
/// keys the USE site when the fixture's declaration carries the same
/// spelling (`int g() => 0;`'s `g()` head sits before the use).
fn fixture_offset(fixture: &str, needle: &str) -> usize {
    crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.rfind(needle).unwrap()
}

/// `if (x != null) x ?? d`: the declared-nullable `x` is promoted
/// non-null at the `??` read — the right operand is dead
/// (`leftOperand.staticType` reads the flow type). Before the staged
/// re-decision the walk's `int?` snapshot stayed silent.
#[test]
fn promoted_local_lhs_reports_the_dead_rhs() {
    let fixture = "void f(int? x, int d) {\n  if (x != null) {\n    var y = x ?? d;\n  }\n}";
    let report = report_ops(fixture);
    assert_dead_coalesce_at(&report, fixture_offset(fixture, "d;"));
}

/// The same promotion inside a nested block: flow promotion survives the
/// brace depth (`if (x != null) { x ?? d }`).
#[test]
fn promoted_local_lhs_inside_nested_block() {
    let fixture = "void f(int? x, int d) {\n  if (x != null) {\n    print(x ?? d);\n  }\n}";
    let report = report_ops(fixture);
    assert_dead_coalesce_at(&report, fixture_offset(fixture, "d);"));
}

/// A declared-nullable `x` promoted by an `is` test: `if (x is int) x ?? d`
/// reports — `x is int` promotes to the non-nullable `int`.
#[test]
fn is_promoted_lhs_reports_the_dead_rhs() {
    let fixture = "void f(int? x, int d) {\n  if (x is int) {\n    var y = x ?? d;\n  }\n}";
    let report = report_ops(fixture);
    assert_dead_coalesce_at(&report, fixture_offset(fixture, "d;"));
}

/// An unpromoted nullable left operand stays silent — the staged
/// re-decision joins only when the promotion table holds the read.
#[test]
fn unpromoted_nullable_lhs_stays_silent() {
    let fixture = "void f(int? x, int d) {\n  var y = x ?? d;\n}";
    let report = report_ops(fixture);
    assert!(
        !code_names(&report)
            .iter()
            .any(|&c| c == "dead_null_aware_expression"),
        "{:?}",
        rows(&report)
    );
}

/// A nullable left operand promoted to a STILL-nullable face stays
/// silent: `x` declared `int?`, the `if (x != null) { }` promotion is
/// spent by an intervening write before the `??` reads it — no dead row.
#[test]
fn invalidated_promotion_lhs_stays_silent() {
    let fixture = "void f(int? x, int d, int? source) {\n  if (x != null) {\n    x = source;\n    var y = x ?? d;\n  }\n}";
    let report = report_ops(fixture);
    assert!(
        !code_names(&report)
            .iter()
            .any(|&c| c == "dead_null_aware_expression"),
        "{:?}",
        rows(&report)
    );
}

/// A declared-non-null left operand still reports at walk time — the
/// staged re-decision does not double-report it.
#[test]
fn declared_non_null_lhs_reports_once() {
    let fixture = "void f(int x, int d) {\n  var y = x ?? d;\n}";
    let report = report_ops(fixture);
    assert_dead_coalesce_at(&report, fixture_offset(fixture, "d;"));
    assert_eq!(
        code_names(&report)
            .iter()
            .filter(|&&c| c == "dead_null_aware_expression")
            .count(),
        1,
        "{:?}",
        rows(&report)
    );
}

/// `x ??= 1; x ??= 2`: the first `??=` write re-promotes the nullable
/// `x` to non-null — the second `??=` reads the flow-aware
/// `readResolution.type` and reports the dead value (`visitIfNullAssignment`
/// → `_checkForDeadNullCoalesce`, error_verifier.dart:1519-1527).
#[test]
fn if_null_assignment_write_promotes_the_next_read() {
    let fixture = "void f(int? x) {\n  x ??= 1;\n  x ??= 2;\n}";
    let report = report_ops(fixture);
    assert_dead_coalesce_at(&report, fixture_offset(fixture, "2;"));
}

/// `x ??= v` over an already non-nullable `x`: the declared-type verdict
/// still anchors the dead face at walk time (no promotion needed).
#[test]
fn if_null_assignment_non_null_target_reports() {
    let fixture = "void f(int x, int v) {\n  x ??= v;\n}";
    let report = report_ops(fixture);
    assert_dead_coalesce_at(&report, fixture_offset(fixture, "v;"));
}

/// An unpromoted nullable `x ??= v` stays silent — the write's own
/// promotion is the only anchor the face reads.
#[test]
fn if_null_assignment_nullable_target_stays_silent() {
    let fixture = "void f(int? x, int v) {\n  x ??= v;\n}";
    let report = report_ops(fixture);
    assert!(
        !code_names(&report)
            .iter()
            .any(|&c| c == "dead_null_aware_expression"),
        "{:?}",
        rows(&report)
    );
}

/// `x ??= 1` inside `if (x == null)` keeps the next `x ??= 2` dead: the
/// outer promotion is absent but the `??=` write itself promotes `x` for
/// the second read — the write-promotion arm, not a test promotion.
#[test]
fn if_null_assignment_inside_null_branch_promotes() {
    let fixture = "void f(int? x) {\n  if (x == null) {\n    x ??= 1;\n  }\n  x ??= 2;\n}";
    let report = report_ops(fixture);
    assert_dead_coalesce_at(&report, fixture_offset(fixture, "2;"));
}

/// A member-read `??=` target (`o.f ??= v`) does not ride the local
/// staging — the flow property-promotion table keys elsewhere, so the
/// declared-nullable member stays silent this round (the residual face).
#[test]
fn if_null_assignment_member_target_stays_silent() {
    let fixture = "class C { int? f = 0; }\nvoid f(C c, int v) {\n  c.f ??= v;\n}";
    let report = report_ops(fixture);
    assert!(
        !code_names(&report)
            .iter()
            .any(|&c| c == "dead_null_aware_expression"),
        "{:?}",
        rows(&report)
    );
}

/// The promoted `??` inside a collection element: `if (x != null) { var l
/// = [x ?? d]; }` — the staged re-decision is span-keyed, not context
/// keyed, so it anchors inside element positions too.
#[test]
fn promoted_lhs_inside_collection_element() {
    let fixture = "void f(int? x, int d) {\n  if (x != null) {\n    var l = [x ?? d];\n  }\n}";
    let report = report_ops(fixture);
    assert_dead_coalesce_at(&report, fixture_offset(fixture, "d]"));
}

/// A promoted left operand whose right operand is a CALL still reports at
/// the call's own span — the report range is the right operand's node,
/// not the operand row's head.
#[test]
fn promoted_lhs_reports_at_call_rhs_span() {
    let fixture =
        "int g() => 0;\nvoid f(int? x) {\n  if (x != null) {\n    var y = x ?? g();\n  }\n}";
    let report = report_ops(fixture);
    assert_dead_coalesce_at(&report, fixture_offset(fixture, "g()"));
}
