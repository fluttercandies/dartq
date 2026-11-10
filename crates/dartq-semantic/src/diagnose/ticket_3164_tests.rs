//! The #3164 `use_of_void_result` pins: the operand-consumption faces the
//! engine decides at its own operand-type seams (upstream
//! `checkForUseOfVoidResult`, upstream Dart SDK source
//! pkg/analyzer/lib/src/generated/error_detection_helpers.dart:245-258 —
//! fires iff the operand's `staticType` is `VoidTypeImpl`, anchors
//! `.at(methodName)` for a `MethodInvocation` and `.at(expression)` for
//! every other operand shape). The in-scope seams are the binary-operand
//! fold (`visitBinaryExpression`, error_verifier.dart:467-482 — LEFT
//! unconditionally, RIGHT only for `&&`/`||`), the `!`-postfix operand
//! (`visitPostfixExpression` :1688-1699 — the whole `x!` node), the
//! `!`-prefix operand (`checkForNonBoolNegationExpression`,
//! bool_expression_verifier.dart:50-57/76-88), and every condition
//! operand (`if`/`while`/`assert`/ternary/junction-operand —
//! `checkForNonBoolExpression`'s early-return, :50-57). The
//! assignability-driven shapes (`var x = v()`, `g(v())`, `return v()`,
//! for-in iterables, spreads, interpolation, `is`/`as`, literals, `await`,
//! `throw`, switch expressions/statements, non-`!` prefix ops) resolve in
//! co-tenant seams and stay out of scope here.

use crate::diagnose::test_rig::{code_names, report_ops, rows};
use crate::diagnose::types::FileReport;

/// The message the oracle spells for every face — parameterless
/// (messages.yaml:19638 `useOfVoidResult`).
const VOID_MSG: &str = "This expression has a type of 'void' so its value can't be used.";

/// Asserts `report`'s `use_of_void_result` rows are exactly `expected` —
/// `(offset, length)` in fixture coordinates, each spelling [`VOID_MSG`] —
/// and that every other row carries one of `others`, the remaining codes
/// the 3.13.7 oracle reports on the fixture. The rig may lack one of
/// those rows, never add a code the oracle does not report.
fn assert_void_rows(report: &FileReport, expected: &[(usize, u32)], others: &[&str]) {
    let prelude = crate::diagnose::test_rig::CORE_PRELUDE.len();
    let mut voids = Vec::new();
    for d in &report.diagnostics {
        if d.code.name == "use_of_void_result" {
            assert_eq!(d.message, VOID_MSG, "{:?}", rows(report));
            voids.push((d.range.offset as usize - prelude, d.range.length));
        } else {
            assert!(others.contains(&d.code.name), "{:?}", rows(report));
        }
    }
    assert_eq!(voids, expected, "{:?}", rows(report));
}

/// A `void f()`-returning callee and a `void v`-typed read both fold the
/// `Void` static type; the binary LEFT operand is checked for every
/// operator (error_verifier.dart:482). A `MethodInvocation` operand
/// anchors at the callee's name token — `v` inside `v() + 1`, length 1
/// (error_detection_helpers.dart:250-252 `.at(methodName)`). The `+`
/// operator's own nullable-operand face reports beside it — the oracle
/// emits both rows (`dart analyze` 3.13.7, `~/.dartq-oracle/3.13.7`,
/// the fixture verbatim: `error - main.dart:3:3 - use_of_void_result`
/// plus `error - main.dart:3:7 - unchecked_use_of_nullable_value`).
#[test]
fn ticket_3164_binary_left_invocation_anchors_method_name() {
    let fixture = "void v() {}\nvoid main() {\n  v() + 1;\n}";
    let report = report_ops(fixture);
    let names = code_names(&report);
    assert_eq!(
        names,
        ["use_of_void_result", "unchecked_use_of_nullable_value"],
        "{:?}",
        rows(&report)
    );
    let diagnostic = &report.diagnostics[0];
    assert_eq!(diagnostic.message, VOID_MSG);
    let offset = crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.find("v() + 1").unwrap();
    assert_eq!(
        diagnostic.range.offset as usize,
        offset,
        "{:?}",
        rows(&report)
    );
    assert_eq!(diagnostic.range.length, 1, "{:?}", rows(&report));
}

/// A `void`-typed local read as the binary LEFT operand anchors at its own
/// expression span (error_detection_helpers.dart:254 `.at(expression)`) —
/// `v` inside `v + 1`, length 1. The declaration `var v = f()` reports
/// nothing: the local's type is inferred from the initializer, so the
/// write type is `void` and `checkForAssignableExpressionAtType`'s
/// `expectedStaticType is! VoidType` gate
/// (variable_declaration_resolver.dart:104 →
/// error_detection_helpers.dart:107) skips the void use. The 3.13.7
/// oracle answers that single row beside
/// `main_first_positional_parameter_type` (the `void Function()` formal)
/// and `unchecked_use_of_nullable_value` (the `+` on a `void` receiver).
#[test]
fn ticket_3164_binary_left_identifier_anchors_expression() {
    let fixture = "void main(void Function() f) {\n  var v = f();\n  v + 1;\n}";
    let report = report_ops(fixture);
    assert_void_rows(
        &report,
        &[(fixture.find("v + 1").unwrap(), 1)],
        &[
            "main_first_positional_parameter_type",
            "unchecked_use_of_nullable_value",
        ],
    );
}

/// `&&`/`||` check the RIGHT operand too (error_verifier.dart:469-470 —
/// the junction consumes both operands' values); `v() && flag` reports
/// only the left `v`, while `flag && v()` reports only the right one.
#[test]
fn ticket_3164_logical_operators_check_both_operands() {
    let left = report_ops("void v() {}\nvoid main() {\n  bool flag = true;\n  v() && flag;\n}");
    let names = code_names(&left);
    assert_eq!(names, ["use_of_void_result"], "{:?}", rows(&left));
    let right = report_ops("void v() {}\nvoid main() {\n  bool flag = true;\n  flag && v();\n}");
    let names = code_names(&right);
    assert_eq!(names, ["use_of_void_result"], "{:?}", rows(&right));
    // The right operand's anchor is the `v` inside `flag && v()`.
    let fixture = "void v() {}\nvoid main() {\n  bool flag = true;\n  flag && v();\n}";
    let offset = crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.find("v();\n}").unwrap();
    assert_eq!(
        right.diagnostics[0].range.offset as usize,
        offset,
        "{:?}",
        rows(&right)
    );
}

/// Non-logical binaries DO check the RIGHT operand — `1 + v()` reports
/// `use_of_void_result` on the void-valued operand, exactly like the
/// logical `&&`/`||` forms and the `==`/`!=` operator invocation.
///
/// Oracle 3.13.7 (`dart analyze --format machine`), one file containing
/// `flag && v();`, `1 + v();` and `3 == v();`:
/// ```
/// ERROR|COMPILE_TIME_ERROR|USE_OF_VOID_RESULT|...|4|11|1|This expression has a type of 'void' so its value can't be used.
/// ERROR|COMPILE_TIME_ERROR|USE_OF_VOID_RESULT|...|5|7|1|...
/// ERROR|COMPILE_TIME_ERROR|USE_OF_VOID_RESULT|...|6|8|1|...
/// ```
/// Three rows, one per statement — the non-logical `+` fires at col 7 on
/// the `v` of `1 + v()`, i.e. the operand's own use-site span.
///
/// This pin previously asserted silence, on the theory that the resolver
/// owns the assignability question for non-logical binaries. The oracle
/// refutes that for a `void` operand: the void-use check runs first and
/// answers, so the row is owed. Renamed from `..._stays_silent` to match
/// what it now asserts.
#[test]
fn ticket_3164_binary_right_non_logical_reports_void_use() {
    let report = report_ops("void v() {}\nvoid main() {\n  1 + v();\n}");
    let codes = code_names(&report);
    assert!(
        codes.iter().any(|name| *name == "use_of_void_result"),
        "the void operand of `1 + v()` owes a use_of_void_result row; got {:?}",
        rows(&report)
    );
}

/// `!`-prefix operands run `checkForNonBoolNegationExpression` → the
/// early-return void check (bool_expression_verifier.dart:50-57/76-88):
/// `!v()` reports `use_of_void_result` at the `v` name token, never
/// `non_bool_negation_expression`.
#[test]
fn ticket_3164_negation_operand_fires_void_not_non_bool() {
    let fixture = "void v() {}\nvoid main() {\n  !v();\n}";
    let report = report_ops(fixture);
    let names = code_names(&report);
    assert_eq!(names, ["use_of_void_result"], "{:?}", rows(&report));
    let offset = crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.find("v();").unwrap();
    assert_eq!(
        report.diagnostics[0].range.offset as usize,
        offset,
        "{:?}",
        rows(&report)
    );
}

/// The `!`-postfix operand's void check anchors at the WHOLE `x!` node
/// (error_verifier.dart:1692 `checkForUseOfVoidResult(node)` — `node` is
/// the PostfixExpression, so the anchor spans operand + `!`,
/// `.at(expression)` on a non-MethodInvocation operand).
#[test]
fn ticket_3164_null_assert_operand_anchors_whole_bang_expression() {
    let fixture = "void v() {}\nvoid main() {\n  v()!;\n}";
    let report = report_ops(fixture);
    let names = code_names(&report);
    assert_eq!(names, ["use_of_void_result"], "{:?}", rows(&report));
    // The anchor covers `v()!` — operand start through `!` end, length 4.
    let offset = crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.find("v()!").unwrap();
    assert_eq!(
        report.diagnostics[0].range.offset as usize,
        offset,
        "{:?}",
        rows(&report)
    );
    assert_eq!(report.diagnostics[0].range.length, 4, "{:?}", rows(&report));
}

/// A `void` identifier under `!` fires the same way — the `x!` whole
/// span, not the bare `x`. `var v = f()` reports nothing (its inferred
/// `void` write type gates the void use off, as in the binary-left pin
/// above). The 3.13.7 oracle answers that single row beside
/// `main_first_positional_parameter_type` and an `unused_local_variable`
/// on `v`.
#[test]
fn ticket_3164_null_assert_identifier_anchors_bang_span() {
    let fixture = "void main(void Function() f) {\n  var v = f();\n  v!;\n}";
    let report = report_ops(fixture);
    // `v!` — operand `v` plus `!`, length 2.
    assert_void_rows(
        &report,
        &[(fixture.find("v!").unwrap(), 2)],
        &[
            "main_first_positional_parameter_type",
            "unused_local_variable",
        ],
    );
}

/// Statement conditions consume the operand's value —
/// `checkForNonBoolExpression`'s early-return void check
/// (bool_expression_verifier.dart:50-57). `if (v())`, `while (v())`, and
/// `assert(v())` each report `use_of_void_result` at the `v` token
/// instead of any `non_bool_*` face.
#[test]
fn ticket_3164_condition_operands_report_void() {
    let fixture = "void v() {}\nvoid main() {\n  if (v()) {}\n  while (v()) {}\n  assert(v());\n}";
    let report = report_ops(fixture);
    let names = code_names(&report);
    assert!(
        names.iter().all(|name| *name == "use_of_void_result"),
        "{:?}",
        rows(&report)
    );
    assert_eq!(names.len(), 3, "{:?}", rows(&report));
    for diagnostic in &report.diagnostics {
        assert_eq!(diagnostic.message, VOID_MSG);
        assert_eq!(diagnostic.range.length, 1, "{:?}", rows(&report));
    }
}

/// The ternary condition is the same `checkForNonBoolExpression` face —
/// `v() ? 1 : 2` reports at `v`, and the `non_bool_condition` arm stays
/// silent (`known_non_bool(void)` is false — the `void` operand is the
/// void check's own verdict). The `var x` declarator rides the ordinary
/// unused-local face beside it — the oracle emits both rows (`dart
/// analyze` 3.13.7, `~/.dartq-oracle/3.13.7`, the fixture verbatim:
/// `error - main.dart:3:11 - use_of_void_result` plus `warning -
/// main.dart:3:7 - unused_local_variable`; the engine's source-ordered
/// list puts the earlier `x` token first).
#[test]
fn ticket_3164_ternary_condition_reports_void() {
    let fixture = "void v() {}\nvoid main() {\n  var x = v() ? 1 : 2;\n}";
    let report = report_ops(fixture);
    let names = code_names(&report);
    assert_eq!(
        names,
        ["unused_local_variable", "use_of_void_result"],
        "{:?}",
        rows(&report)
    );
    let offset = crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.rfind("v()").unwrap();
    let void_row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "use_of_void_result")
        .expect("use_of_void_result row");
    assert_eq!(
        void_row.range.offset as usize,
        offset,
        "{:?}",
        rows(&report)
    );
}

/// A bare `v();` expression statement is the legal discard — the value
/// is never consumed (no upstream caller fires for a statement-level
/// invocation), so the report stays silent on this seam.
#[test]
fn ticket_3164_expression_statement_discard_stays_silent() {
    let report = report_ops("void v() {}\nvoid main() {\n  v();\n}");
    assert!(
        !code_names(&report)
            .iter()
            .any(|name| *name == "use_of_void_result"),
        "{:?}",
        rows(&report)
    );
}
