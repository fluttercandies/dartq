//! The #3240 pins: `flag && v()` reports `use_of_void_result` ONCE. The
//! lazy boolean operators check BOTH operands (upstream Dart SDK source,
//! pkg/analyzer/lib/src/generated/error_verifier.dart
//! `visitBinaryExpression` → `checkForUseOfVoidResult`) and the oracle
//! answers a single row per consumed operand, while the engine folded the
//! same right-operand row twice — `end_binary_expression`'s junction arm
//! classifies both operands (`classify_condition`'s void face) and
//! `fold_binary` then reports the pair again.

use crate::diagnose::test_rig::{code_names, report_ops, rows};

/// The message the oracle spells for every face — parameterless
/// (messages.yaml:19638 `useOfVoidResult`).
const VOID_MSG: &str = "This expression has a type of 'void' so its value can't be used.";

/// The fixture's `use_of_void_result` spans, in report order.
fn void_spans(report: &super::FileReport) -> Vec<(usize, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.message == VOID_MSG)
        .map(|d| (d.range.offset as usize, d.range.length))
        .collect()
}

/// `flag && v()` — the right operand's void use reports exactly once, at
/// the callee's name token (the `MethodInvocation` anchor).
#[test]
fn ticket_3240_logical_right_operand_single_report() {
    let fixture = "void v() {}\nvoid main() {\n  bool flag = true;\n  flag && v();\n}";
    let report = report_ops(fixture);
    let offset = crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.find("v();").unwrap();
    assert_eq!(void_spans(&report), [(offset, 1)], "{:?}", rows(&report));
}

/// `||` takes the same path — one row, the right operand's own span.
#[test]
fn ticket_3240_logical_or_right_operand_single_report() {
    let fixture = "void v() {}\nvoid main() {\n  bool flag = true;\n  flag || v();\n}";
    let report = report_ops(fixture);
    let offset = crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.find("v();").unwrap();
    assert_eq!(void_spans(&report), [(offset, 1)], "{:?}", rows(&report));
}

/// The left operand keeps its single report — the junction's adjacency gate
/// never classified it, so the fold's row is the only one.
#[test]
fn ticket_3240_logical_left_operand_single_report() {
    let fixture = "void v() {}\nvoid main() {\n  bool flag = true;\n  v() && flag;\n}";
    let report = report_ops(fixture);
    let offset = crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.rfind("v()").unwrap();
    assert_eq!(void_spans(&report), [(offset, 1)], "{:?}", rows(&report));
}

/// A junction inside a statement condition keeps one row as well: the `&&`
/// arm reports the right operand and the enclosing condition checkpoint
/// reads the folded `bool`.
#[test]
fn ticket_3240_logical_in_condition_single_report() {
    let fixture = "void v() {}\nvoid main() {\n  bool flag = true;\n  if (flag && v()) {}\n}";
    let report = report_ops(fixture);
    let offset = crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.find("v())").unwrap();
    assert_eq!(void_spans(&report), [(offset, 1)], "{:?}", rows(&report));
}

/// A non-`void` junction stays silent — the dedup must not suppress the
/// ordinary `bool` operands.
#[test]
fn ticket_3240_logical_bool_operands_stay_silent() {
    let fixture =
        "void main() {\n  bool flag = true;\n  bool other = false;\n  flag && other;\n}";
    let report = report_ops(fixture);
    assert!(
        !code_names(&report)
            .iter()
            .any(|name| *name == "use_of_void_result"),
        "{:?}",
        rows(&report)
    );
}
