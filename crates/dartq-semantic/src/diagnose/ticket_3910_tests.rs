//! Ticket #3910 pins: the `diagnose.rs` pass-phase packet — the E13-4
//! promoted-operand latch starvation, the E13-6 conditional return context
//! and the `??` argument join.
//!
//! The pipeline in [`fold_unit_with_ops`] has two phases: the listener fold
//! runs INSIDE `Parser::parse_unit` (pass 1), while [`Engine::read_types`] —
//! the flow read latch every in-fold promotion consult answers from
//! ([`Engine::read_type_at`]) — is filled from the flow facts only after the
//! parse (the fill site in `fold_unit_with_ops`). An in-fold consult
//! therefore always sees an empty latch. Upstream keeps the promotion state
//! live at typing time: a
//! promoted read's static type is answered by the local-variable type
//! provider during resolution (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/dart/resolver/flow_analysis_visitor.dart`:1394-1417
//! `_LocalVariableTypeProvider.getType` → `flow.promotedType`), so
//! `visitBinaryExpression` types its operands on the promoted face.
//!
//! Every fixture below is oracle-verified against `dart analyze
//! --format=machine` (3.13.7) at the fixture's own language version.

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

// —— E13-4: the promoted binary operand ——————————————————————

/// A promoted left operand must feed the binary fold: under `n is int` the
/// node `n + 1` is `int`, so `int x = n + 1` reports nothing. Oracle: silent.
///
/// Fixed by #3980's teardown recompute: the in-fold `read_type_at` consult
/// starved (the latch fills after `parse_unit`), so `fold_binary` records
/// the fold and `filter_staged` re-derives the promoted result once
/// `read_types` is populated — the staged assignment's `from` ladder reads
/// the recomputed `int`, never the unpromoted `num`.
#[test]
fn promoted_binary_operand_reads_flow_promotion() {
    let source = r#"
void f(num n) {
  if (n is int) {
    int x = n + 1;
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

// —— E13-6: the conditional return context ————————————————————

/// `x ? 42 : Future.value(42)` returned from an `async` `Future<int>`
/// function types `FutureOr<int>` (the return context; `inference-update-3`,
/// enabled from 3.4.0 — upstream Dart SDK source,
/// `pkg/analyzer/lib/src/generated/static_type_analyzer.dart`:74-112), so
/// the return is clean. Oracle: silent. This pin is the negative evidence
/// for the E13-6 half of the starvation claim: the face is already correct
/// on the current tree.
#[test]
fn conditional_async_return_types_context_not_lub() {
    let source = r#"
Future<int> f(bool x) async {
  return x ? 42 : Future.value(42);
}
"#;
    let report = report_ops(source);
    assert_eq!(
        rows_of(&report, "return_of_invalid_type"),
        vec![],
        "{:#?}",
        report.diagnostics
    );
}

// —— the `??` argument face ——————————————————————————————————

/// An `int? ?? int?` argument keeps the nullable join (`int?`), so passing
/// it to an `int` parameter reports `argument_type_not_assignable`. Oracle:
/// one row. `fold_binary` still answers `UNKNOWN` for `??` on purpose
/// (`engine_core.rs:11127-11135`), but the argument face now reads the
/// join `stage_if_null_result` staged: `end_binary_expression` re-keys the
/// entry onto the folded NODE row (the row the argument's expression
/// presents), so the nullable join reaches the argument check.
#[test]
fn if_null_argument_joins_operand_types() {
    let source = r#"
void take(int x) {}

void f(int? a, int? b) {
  take(a ?? b);
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "argument_type_not_assignable");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        rows[0].2.contains("'int?'") && rows[0].2.contains("'int'"),
        "{}",
        rows[0].2
    );
}

// —— the control: post-pass-1 consumers still see the fill ————

/// The pass-2 consumers read the latch only after it is filled: the
/// assignment right-hand side re-decides against the promoted read
/// (`assignability.rs:4988`'s `read_type_at`), so `int x = n` under
/// `n is int` is clean. Oracle: silent. This pin guards the reorder: any
/// fix for the in-fold consult must keep the post-parse fill in place.
#[test]
fn post_pass1_latch_fill_reaches_assignment_rhs() {
    let source = r#"
void f(num n) {
  if (n is int) {
    int x = n;
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
