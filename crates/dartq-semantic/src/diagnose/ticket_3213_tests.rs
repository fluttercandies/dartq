//! The #3213 pins: `dead_code`'s span-shape law — the oracle's
//! `NullSafetyDeadCodeVerifier` emits ONE merged tail per dead region and
//! small marker rows on the dead trigger/head nodes, with exact
//! `(offset, length)` spans.
//!
//! Upstream law (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/error/dead_code_verifier.dart:223-330`):
//! `_firstDeadNode` anchors the row at the FIRST node visited
//! unreachable; `flowEnd(node)` reports `[firstDeadNode.offset, node.end)`
//! trimmed to the covering node's last statement. The dedicated faces:
//!
//! * a constant-folded conditional's dead arm is its own first dead node —
//!   the row IS the arm expression (`x`, or the arm's whole extent), never
//!   spilling into the statement's `;` (`flowEnd(elseExpression)` —
//!   `node == firstDeadNode`, so no trim and no `;`);
//! * `for (; false; x) {}`: the body's dead block reports `{}` and the
//!   unreachable updater gets its own `[updaters.begin, updaters.end)` row
//!   (`for_updaterBegin`, dead_code_verifier.dart:350-371); the loop's
//!   cond-false exit stays LIVE, so the statement after the `for` is
//!   reachable;
//! * a `throw`'s tail reports `[next statement, last statement of block)`
//!   as one merged span — consecutive dead statements never split.
//!
//! Anchors are asserted as `(offset, length)` triples: `dead_code` carries
//! a fixed message ("Dead code."), so the span IS the observable.

use crate::diagnose::test_rig::report_ops_bare;

/// The `dead_code` rows of `report` as `(offset, length)`, source order.
fn dead_rows(report: &super::FileReport) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == "dead_code")
        .map(|row| (row.range.offset, row.range.length))
        .collect()
}

/// The `void`-parameter corpus shape (`language/void/void_type_usage_test`
/// `testVoidParam`): a constant `?:` arm marker on the arm expression, the
/// `for`'s updater + empty-body marker pair, a live statement after the
/// loop, and ONE merged tail after `throw`.
///
/// Fixture offsets (whitespace-sensitive — an edit to the literal moves
/// them):
/// * `true ? x : x;` at line 11: the dead else-arm `x` at `134`, len `1`
///   (the arm expression alone — never `x;`);
/// * `for (x; false; x) {}` at line 13: the updater `x` at `155`, len `1`;
///   the dead body `{}` at `158`, len `2`;
/// * `useAsVoid(x);` stays LIVE (the cond-false exit is reachable);
/// * `throw x;` kills `use(x);` — one tail `[193, 200)`, `use(x);` whole.
#[test]
fn void_param_dead_shape_matrix() {
    let fixture = "void useAsVoid(void x) {}\n\
                   void use(dynamic x) {}\n\
                   \n\
                   Object? testVoidParam(void x) {\n\
                   \x20 var _ = () {\n\
                   \x20   x = 15;\n\
                   \x20 };\n\
                   \n\
                   \x20 x;\n\
                   \n\
                   \x20 true ? x : x;\n\
                   \n\
                   \x20 for (x; false; x) {}\n\
                   \n\
                   \x20 useAsVoid(x);\n\
                   \n\
                   \x20 throw x;\n\
                   \n\
                   \x20 use(x);\n\
                   }";
    assert_eq!(
        dead_rows(&report_ops_bare(fixture)),
        [(134, 1), (155, 1), (158, 2), (193, 7)]
    );
}

/// A constant-`true` conditional marks ONLY the dead arm expression — the
/// `;` terminator is the statement's, not the arm's. Oracle triples:
/// `true ? x : x` → the else `x` (`30`, len `1`); `false ? 1 : x` → the
/// then `1`... wait — upstream walks the THEN arm dead on `false` too
/// (`flowEnd(thenExpression)`), so the then operand reports.
#[test]
fn conditional_dead_arm_excludes_semicolon() {
    let fixture = "void f(int x) {\n  true ? x : x;\n}";
    // `true ? x : x`: the `:`-side arm `x` at offset 29, len 1 — not `x;`.
    assert_eq!(dead_rows(&report_ops_bare(fixture)), [(29, 1)]);
}

/// A multi-token dead else-arm reports the arm's whole extent and stops at
/// the arm end — `x + 1` keeps the `;` out.
#[test]
fn dead_arm_stops_at_arm_end() {
    let fixture = "void f(int x) {\n  true ? x : x + 1;\n}";
    // The else arm `x + 1` reports its own extent (offsets 29..34), never
    // reaching the `;`.
    assert_eq!(dead_rows(&report_ops_bare(fixture)), [(29, 5)]);
}

/// `for (; false; u) {}`: the dead empty body reports `{}` and the
/// unreachable updater reports `u` — while the statement after the loop
/// stays LIVE (the cond-false exit is reachable). A `throw` tail merges
/// every later statement into ONE span ending at the block's last
/// statement.
#[test]
fn for_false_body_updater_markers_and_merged_tail() {
    let fixture = "void f(Object u) {\n  for (; false; u) {}\n  u;\n  throw 0;\n  u;\n  u;\n}";
    // `for (; false; u) {}` at line 2: updater `u` at `35` len `1`, dead
    // body `{}` at `38` len `2`. `u;` at line 3 stays live. `throw 0;`
    // merges the last two `u;` into ONE tail — `u;\n  u;` at `[59, 66)`.
    assert_eq!(dead_rows(&report_ops_bare(fixture)), [(35, 1), (38, 2), (59, 7)]);
}

/// A null-shorted WRITE's dead tail stops at the assignment's own end —
/// `n?.p = i` reports `p = i`, never `p = i;` (`flowEnd` reports the
/// assignment node's `node.end`; the `;` belongs to the statement).
#[test]
fn null_aware_write_tail_excludes_semicolon() {
    let fixture = "void f(Null n, int i) {\n  n?.p = i;\n  print(0);\n}";
    // `n?.p = i;`: the dead chain anchors at the member `p` (offset 29)
    // and reports `p = i` — offsets 29..34, the `;` excluded.
    assert_eq!(dead_rows(&report_ops_bare(fixture)), [(29, 5)]);
}

/// A constant conditional's dead MEMBER-ACCESS arm reports the arm's own
/// extent — `p.y`, never `p.y;` (the `;` is the statement's terminator,
/// not the arm's).
#[test]
fn conditional_member_arm_excludes_semicolon() {
    let fixture = "void f(({int x, int y}) p) {\n  true ? p.x : p.y;\n}";
    // The dead else arm `p.y` at offset 44, len 3 — not `p.y;`.
    assert_eq!(dead_rows(&report_ops_bare(fixture)), [(44, 3)]);
}

/// A block ENTERED dead reports the block node whole — `{`..`}` —
/// (`checkUnreachableNode(block)` latches the block itself as the first
/// dead node). A dead statement inside a live-entered block still rows
/// only its own extent.
#[test]
fn dead_entered_block_reports_whole_braces() {
    let fixture = "void f(Object x) {\n  return;\n  {\n    x;\n  }\n}";
    // The inner `{ x; }` block is entered dead: the row is `{`..`}` —
    // offsets 31..43 — not the inner statement `x;` alone.
    assert_eq!(dead_rows(&report_ops_bare(fixture)), [(31, 12)]);
}

/// A dead run reaching the END of a list literal reports the literal's
/// `]` too — `f()` diverges (Never), so `4]` is dead (`flowEnd` covers
/// the whole literal node).
#[test]
fn dead_list_tail_extends_through_closer() {
    let fixture = "Never g() => throw 0;\nvoid f() {\n  var x = [1, 2, g(), 4];\n}";
    // `4]` at offsets 55..57 — the last dead element plus the list's
    // closing `]` — not `4` alone.
    assert_eq!(dead_rows(&report_ops_bare(fixture)), [(55, 2)]);
}

/// A dead case-label tail keeps its statement `;` — the `;`-clamp only
/// trims mid-expression markers (`p = i`), never a statement anchored
/// after a case colon or a `;`.
#[test]
fn case_label_dead_statement_keeps_semicolon() {
    let fixture = "void f(int x) {\n  switch (x) {\n    case 1:\n      return;\n      x;\n  }\n}";
    // `x;` after `return;` is the merged tail's single statement — the
    // row is `x;` whole (offset 63, len 2), `;` included.
    assert_eq!(dead_rows(&report_ops_bare(fixture)), [(63, 2)]);
}

/// `for (x; false; x) {}` — the void_param corpus shape with an
/// expression INIT before the false condition: the init runs once and
/// stays live, the cond-false exit keeps the statement after the loop
/// reachable, and a `throw` tail still merges downstream statements into
/// ONE span.
#[test]
fn for_init_false_body_and_merged_tail() {
    let fixture = "void f(Object x) {\n  for (x; false; x) {}\n  x;\n  throw 0;\n  x;\n}";
    // Updater `x` at `36` len `1`, dead body `{}` at `39` len `2`; `x;`
    // at line 3 stays live (cond-false exit); `throw 0;` merges `x;` into
    // ONE tail `[60, 62)`.
    assert_eq!(
        dead_rows(&report_ops_bare(fixture)),
        [(36, 1), (39, 2), (60, 2)]
    );
}

/// Negative: an all-live sequence reports nothing — a live conditional's
/// used arm, a live `for`, and no diverging statement.
#[test]
fn live_code_emits_no_dead_code() {
    let fixture =
        "void f(int x) {\n  x == 0 ? x : x;\n  for (x; x < 3; x) {}\n  x;\n}";
    assert_eq!(dead_rows(&report_ops_bare(fixture)), []);
}
