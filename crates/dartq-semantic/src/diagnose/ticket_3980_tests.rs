//! #3980 pins — the composite-fold teardown recompute: a binary or
//! conditional fold whose operand/arm `read_type_at` consult ran during
//! pass 1 (before `Engine::read_types` fills) starved to the declared face,
//! so the folded node's type never saw the flow promotion. `fold_binary`/
//! `fold_conditional` now record the fold into a thread-local pending table
//! and `filter_staged` re-derives the promoted result once the read-type
//! latch is populated, feeding the staged-check `from` ladder the corrected
//! composite type through the `(node_range → ty)` rewrite map.
//!
//! The mechanism mirrors the `fn_shape_required` side-table pattern (#3564):
//! record during the fold, answer at teardown. The recompute is a pure
//! result-ty derivation — it re-runs the operator signature resolution and
//! the numeric refinement / the conditional LUB + return-context ladder
//! while skipping the side-effecting `check_binary_argument` (its staged
//! `Argument` row already exists from the fold).
//!
//! Every fixture below is oracle-reasoned against `dart analyze
//! --format=machine` (3.13.7) semantics: a promoted operand re-types the
//! composite node exactly as upstream's live flow state does.

use super::test_rig::report_ops;
use super::*;

/// Every `code` row of `report` as `(offset, length, message)`.
fn rows_of(report: &FileReport, code: &str) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

// —— the conditional (`?:`) arm promotion ————————————————————————

/// A promoted conditional arm must feed the fold: under `o is int` the
/// then arm `o` reads `int`, so `b ? o : 1` joins `int` and `int x = …`
/// reports nothing. Oracle: silent. Exercises
/// [`PendingCompositeFold::Conditional`] — the arm `read_type_at` consult
/// starved identically to the binary operand's.
#[test]
fn conditional_promoted_arm_recomputes_lub() {
    let source = r#"
void f(Object? o, bool b) {
  if (o is int) {
    int x = b ? o : 1;
    x = x;
  }
}
"#;
    let report = report_ops(source);
    assert_eq!(
        rows_of(&report, "invalid_assignment"),
        vec![],
        "{:#?}",
        report.diagnostics
    );
}

// —— nested composite order-independence ————————————————————————

/// `n + 1 + 2` folds the inner `n + 1` first, then the outer `(n + 1) + 2`
/// whose left operand is the inner's `Value` row. The forward pass
/// recomputes the inner to `int` before the outer consults it, so the
/// outer's operand lookup hits the inner's rewrite-map entry and the whole
/// node answers `int`. Oracle: silent. Guards the fold-order record +
/// rewrite-map operand consult — without it the outer would keep the
/// inner's unpromoted `num`.
#[test]
fn nested_binary_promoted_inner_propagates() {
    let source = r#"
void f(num n) {
  if (n is int) {
    int x = n + 1 + 2;
    x = x;
  }
}
"#;
    let report = report_ops(source);
    assert_eq!(
        rows_of(&report, "invalid_assignment"),
        vec![],
        "{:#?}",
        report.diagnostics
    );
}

// —— selectivity: the rewrite never over-cleans ————————————————————

/// The recompute must still REPORT a genuine mismatch, spelling the
/// promoted face: under `n is int`, `n + 1` is `int`, so `double x = …`
/// (a non-literal `int` is not assignable to `double`) reports
/// `invalid_assignment` with `'int'` in the message — never the unpromoted
/// `'num'` the starvation left, and never silence (the rewrite is not a
/// blanket suppress). Oracle: one row.
#[test]
fn promoted_binary_still_reports_real_mismatch() {
    let source = r#"
void f(num n) {
  if (n is int) {
    double x = n + 1;
    x = x;
  }
}
"#;
    let report = report_ops(source);
    let found = rows_of(&report, "invalid_assignment");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        found[0].2.contains("'int'") && found[0].2.contains("'double'"),
        "{}",
        found[0].2
    );
    assert!(
        !found[0].2.contains("'num'"),
        "promoted face must replace the unpromoted 'num': {}",
        found[0].2
    );
}

// —— the drain is total: no cross-unit leak ————————————————————————

/// Two units in one report run: the first unit's pending records must not
/// leak into the second unit's `filter_staged` (the thread-local drains on
/// every path and records carry the `file` tag). Both units are silent;
/// a leaked record answering the wrong file's consult would surface as a
/// spurious rewrite. Oracle: silent for both.
#[test]
fn pending_records_do_not_leak_across_units() {
    // `report_ops` analyzes a single unit; run two independent analyses and
    // confirm each stays silent — the second run reuses the same rayon
    // thread's thread-local, so a non-draining first pass would corrupt it.
    let first = report_ops(
        r#"
void f(num n) {
  if (n is int) {
    int x = n + 1;
    x = x;
  }
}
"#,
    );
    let second = report_ops(
        r#"
void g(num n) {
  if (n is int) {
    int y = n + 2;
    y = y;
  }
}
"#,
    );
    assert_eq!(rows_of(&first, "invalid_assignment"), vec![]);
    assert_eq!(rows_of(&second, "invalid_assignment"), vec![]);
}
