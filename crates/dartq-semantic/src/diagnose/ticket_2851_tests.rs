//! The #2851 const-condition dead-arm pins: the S2 census faces of the
//! dead_code burn-down that already hold end-to-end, the negatives that
//! keep them honest, and the adjacent-face no-cross-talk contract.
//!
//! Ownership verdict (pinned before coding, per the dispatch): all three
//! S2 sub-faces are flow-verdict faces upstream and here —
//! `booleanLiteral` (flow_analysis.dart:5364-5375) folds literal
//! conditions, the `conditionVariableState` write/restore pair
//! (:7565-7577 / :6890-6908) replays an assigned-bool condition, and the
//! collection-element conditions fold through the same branch machinery;
//! the spans are shaped by `NullSafetyDeadCodeVerifier.flowEnd`
//! (dead_code_verifier.dart:223-330, `handle_ifElement_thenEnd` /
//! `elseEnd` → `flowEnd(ifTrue/ifFalse)` at resolver.dart:1070-1102).
//! The const evaluator owns nothing here: `ConstantVerifier` reports
//! only const codes — the dead rows riding the ConstantVisitorTest cases
//! are flow null-aware rows (the S5 face). Ours mirrors the split:
//! `FlowFact::DeadCode` emits in dartq-flow's interp and translates at
//! diagnose.rs's fact join; this module pins the translated rows.
//!
//! Seams still owned flow-side (reported for the flow lane, current-tree
//! verified against the census and a HEAD probe):
//! - for-element under a constant-false condition — the element row is
//!   missing or mis-shaped (`features/control-flow-collections`, 14
//!   census rows; `for (; false;) ...dyn` reps). The verdict folds in
//!   `replay_c_for`, but the element-form body close is skipped
//!   (`collection = true`) and no arm-row face covers the element.
//! - assigned-bool condition variable — `b = false; if (b) {…}` stays
//!   silent: `cond_vars` (interp.rs) records only test-derived entries;
//!   the write-side entry mirroring `conditionVariableState` is absent
//!   (nnbd `local_variable_read_A02/A05`, ~8-11 census rows).
//! - empty then/else block in a LIVE if — the oracle reports the bare
//!   `{}` (`DeadCodeTestCases_Language212/test_deadBlock_if#0` @3:13|2,
//!   `test_deadBlock_else#0` @3:20|2, both failing); a branch-empty
//!   block nested in a dead region stays oracle-silent
//!   (`test_deadBlock_else_nested#0` passes — the existing
//!   branch-empty suppression in the flow's `Ev::Block` arm).

use super::test_rig::{CORE_PRELUDE, report_ops, rows};
use super::*;

/// The absolute fixture offset of `needle` (the prelude the rig prepends
/// included).
fn at(fixture: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + fixture.find(needle).unwrap()
}

/// The `dead_code` rows as `offset:length`, the span convention under pin.
fn dead_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .map(|d| format!("{}:{}", d.range.offset, d.range.length))
        .collect()
}

/// Exactly one `dead_code` row at `offset` spanning `length`.
#[track_caller]
fn assert_dead_row(report: &FileReport, offset: usize, length: u32) {
    let hits = dead_rows(report);
    assert_eq!(
        hits,
        vec![format!("{offset}:{length}")],
        "{:#?}",
        rows(report)
    );
}

/// A literal `false` condition deads the whole then-block: the row spans
/// both braces (official `flowEnd(ifTrue)` with the block as its own
/// first dead node — `node == firstDeadNode` reports the node range,
/// dead_code_verifier.dart:253-279; the co19 nnbd census rows are the
/// whole-block spans `30:10|18`).
#[test]
fn literal_false_deads_the_whole_then_statement() {
    let fixture = "f() {\n  if (false) {\n    1;\n  }\n}\n";
    let report = report_ops(fixture);
    let start = at(fixture, "if (false) {") + "if (false) ".len();
    let end = at(fixture, "1;\n  }") + "1;\n  ".len();
    assert_dead_row(&report, start, (end - start + 1) as u32);
}

/// A literal `true` condition deads the whole else-block, same span
/// convention (`handle_ifStatement_elseEnd` → `flowEnd(ifFalse)`).
#[test]
fn literal_true_deads_the_whole_else_statement() {
    let fixture = "f() {\n  if (true) {} else {\n    2;\n  }\n}\n";
    let report = report_ops(fixture);
    let start = at(fixture, "else {") + "else ".len();
    let end = at(fixture, "2;\n  }") + "2;\n  ".len();
    assert_dead_row(&report, start, (end - start + 1) as u32);
}

/// A const VARIABLE condition does not fold (`const k = false; if (k)`):
/// the oracle's booleanLiteral face folds literal expressions only — a
/// const local's read carries no branch info (the
/// `deadBlock_if_debugConst_*` census cases are silent on both sides).
/// This is the over-fold guard.
#[test]
fn const_variable_condition_stays_live() {
    let fixture = "f() {\n  const k = false;\n  if (k) {\n    1;\n  }\n}\n";
    let report = report_ops(fixture);
    assert!(dead_rows(&report).is_empty(), "{:#?}", rows(&report));
}

/// An unchosen collection-if element reports at the ELEMENT's own extent,
/// not the literal's: `handle_ifElement_thenEnd` → `flowEnd(ifTrue)`
/// reports the arm node (the map entry `1: 2`), the one-row-per-arm
/// convention the `deadBlock_ifElement` census cases pin.
#[test]
fn unchosen_if_element_arms_row_at_the_element_extent() {
    let fixture = "f() {\n  var m = {if (false) 1: 2};\n  m;\n}\n";
    let report = report_ops(fixture);
    let start = at(fixture, "if (false) 1: 2") + "if (false) ".len();
    assert_dead_row(&report, start, 4);
}

/// The else element of a collection-if reports at its own extent too
/// (`flowEnd(ifFalse)`): `if (true) … else 3: 4` deads `3: 4`, brace
/// excluded.
#[test]
fn unchosen_else_element_arms_row_at_the_element_extent() {
    let fixture = "f() {\n  var s = {if (true) 1: 2 else 3: 4};\n  s;\n}\n";
    let report = report_ops(fixture);
    let start = at(fixture, "else 3: 4") + "else ".len();
    assert_dead_row(&report, start, 4);
}

/// A non-const condition keeps every arm live — the statement form and
/// the element form alike (`cond_split` splits, neither branch folds).
#[test]
fn non_const_condition_keeps_statement_and_element_arms_live() {
    let fixture = "f(bool c) {\n  if (c) {\n    1;\n  }\n  var m = {if (c) 1: 2};\n  m;\n}\n";
    let report = report_ops(fixture);
    assert!(dead_rows(&report).is_empty(), "{:#?}", rows(&report));
}

/// The adjacent faces are untouched by the dead-arm work: the #2814
/// for-in iterable operand still reports its single
/// `unchecked_use_of_nullable_value` row, the #2805 close-frame verdict
/// still reports the unread loop variable, and neither rides a dead_code
/// row.
#[test]
fn adjacent_operand_and_usage_faces_unchanged() {
    let fixture = "f() {\n  List? x;\n  for (var y in x) {}\n}\n";
    let report = report_ops(fixture);
    let x = at(fixture, "x) {}");
    let unchecked: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unchecked_use_of_nullable_value")
        .collect();
    assert_eq!(unchecked.len(), 1, "{:#?}", rows(&report));
    assert_eq!(unchecked[0].range.offset as usize, x);
    assert_eq!(unchecked[0].range.length, 1);
    let unused: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_local_variable")
        .collect();
    assert_eq!(unused.len(), 1, "{:#?}", rows(&report));
    assert!(dead_rows(&report).is_empty(), "{:#?}", rows(&report));
}
