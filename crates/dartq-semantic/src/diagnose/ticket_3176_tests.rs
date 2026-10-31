//! The #3176 `use_of_void_result` pins: the operand-consumption seams
//! folded onto the #3164 in-scope set — the faces the resolver drives at
//! its own operand-type seams (upstream `checkForUseOfVoidResult`,
//! upstream Dart SDK source
//! pkg/analyzer/lib/src/generated/error_detection_helpers.dart:245-258 —
//! fires iff the operand's `staticType` is `VoidTypeImpl`, anchors
//! `.at(methodName)` for a `MethodInvocation` and `.at(expression)` for
//! every other operand shape). The seams wired here are the assignability
//! shapes (#3164's declared out-of-scope set): assignment right operand
//! (`assignment_expression_resolver.dart:136`), declaration initializer,
//! generic argument (`g(v())`), for-in iterable (error_verifier.dart:
//! 4442), switch scrutinee (:2024/:2038), `??` left operand, `is`
//! expression (:1436), interpolation (:1428), `await` (:459), `throw`
//! (:2052), `yield`/`yield*` (yield_statement_resolver.dart:181/:198),
//! non-`!` prefix/postfix unary (:1727), collection/list/set elements and
//! map-entry key/value (literal_element_verifier.dart:95/:151/:166), and
//! the anonymous-invocation callee
//! (function_expression_invocation_resolver.dart:56).

use crate::diagnose::test_rig::{code_names, report_ops, rows};

/// The message the oracle spells for every face — parameterless
/// (messages.yaml:19638 `useOfVoidResult`).
const VOID_MSG: &str =
    "This expression has a type of 'void' so its value can't be used.";

/// Asserts a `use_of_void_result` diagnostic sits at `offset`, tolerating
/// the co-diagnostics a declared-but-unread local (`unused_local_variable`)
/// or a dead-`??` arm (`dead_code`) adds around it — every fixture here
/// declares the consuming slot without reading it, so the family rows sit
/// beside the void use, not instead of it.
fn assert_void_at(report: &super::FileReport, offset: usize) {
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.message == VOID_MSG && d.range.offset as usize == offset),
        "no use_of_void_result at {offset}: {:?}",
        rows(report)
    );
}

/// A `void`-returning call on the assignment right operand reports
/// `use_of_void_result` at the callee's name token — the resolver's
/// `_checkForUseOfVoidResult(right)` gate runs only when the target's
/// `writeType is! VoidType` (assignment_expression_resolver.dart:136):
/// `x = v()` into a non-`void` `x` reports, a `void`-typed target absorbs.
#[test]
fn ticket_3176_assignment_rhs_void_call() {
    let fixture = "void v() {}\nvoid main() {\n  Object x = Object();\n  x = v();\n}";
    let report = report_ops(fixture);
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.rfind("v()").unwrap();
    assert_void_at(&report, offset);
    let diagnostic = report
        .diagnostics
        .iter()
        .find(|d| d.message == VOID_MSG)
        .expect("use_of_void_result row");
    assert_eq!(diagnostic.range.length, 1, "{:?}", rows(&report));
}

/// A `void`-typed read on the assignment right operand anchors at the
/// read's own expression span (the non-`MethodInvocation` arm,
/// error_detection_helpers.dart:254 `.at(expression)`).
#[test]
fn ticket_3176_assignment_rhs_void_read() {
    let fixture =
        "void main(void Function() f) {\n  var v = f();\n  Object x = Object();\n  x = v;\n}";
    let report = report_ops(fixture);
    let names = code_names(&report);
    // `var v = f()` itself reports (decl-init seam) and `x = v` reports the
    // read — two void uses beside `v`'s own `unused_local_variable` if any.
    let void_count = names
        .iter()
        .filter(|name| **name == "use_of_void_result")
        .count();
    assert_eq!(void_count, 2, "{:?}", rows(&report));
}

/// The declaration initializer (`var`/`Object x = v()` — upstream
/// `VariableDeclaration` resolution consumes the initializer's value). A
/// `void` initializer reports at the callee name; `var a = v()` infers the
/// `void` face into a `void`-declared variable whose own read is the
/// consumed value.
#[test]
fn ticket_3176_decl_initializer_void_call() {
    let fixture = "void v() {}\nvoid main() {\n  Object a = v();\n}";
    let report = report_ops(fixture);
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.rfind("v()").unwrap();
    assert_void_at(&report, offset);
}

/// `var a = v()` infers `a: void` — the upstream
/// `writeType is! VoidType` gate (assignment_expression_resolver.dart:136)
/// swallows the use-of-void check on an inferred-`void` slot, so `var a`
/// reports only `unused_local_variable`, never `use_of_void_result`.
#[test]
fn ticket_3176_var_decl_inferred_void_stays_silent() {
    let fixture = "void v() {}\nvoid main() {\n  var a = v();\n}";
    let report = report_ops(fixture);
    assert!(
        !code_names(&report).iter().any(|name| *name == "use_of_void_result"),
        "{:?}",
        rows(&report)
    );
}

/// The generic-argument operand (`g(v())` — upstream the argument's
/// `staticType` consumption in `_checkForInvalidAssignment`/the argument
/// face): a `void` actual into a non-`void` formal reports.
#[test]
fn ticket_3176_argument_void_call() {
    let fixture =
        "void v() {}\nvoid g(Object x) {}\nvoid main() {\n  g(v());\n}";
    let report = report_ops(fixture);
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.rfind("v()").unwrap();
    assert_void_at(&report, offset);
}

/// The for-in iterable (`for (x in v())` — error_verifier.dart:4442
/// `checkForUseOfVoidResult(node.iterable)` early-returns after reporting):
/// a `void` iterable reports `use_of_void_result`, never
/// `for_in_of_invalid_type`.
#[test]
fn ticket_3176_for_in_iterable_void() {
    let fixture =
        "void v() {}\nvoid main() {\n  for (var x in v()) {}\n}";
    let report = report_ops(fixture);
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.rfind("v()").unwrap();
    assert_void_at(&report, offset);
}

/// The switch-statement scrutinee (`switch (v())` —
/// error_verifier.dart:2038 `checkForUseOfVoidResult(node.expression)`).
#[test]
fn ticket_3176_switch_statement_scrutinee_void() {
    let fixture = "void v() {}\nvoid main() {\n  switch (v()) {\n    case 1:\n      break;\n  }\n}";
    let report = report_ops(fixture);
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.rfind("v()").unwrap();
    assert_void_at(&report, offset);
}

/// The `??` left operand (`v() ?? 1` — `end_if_null`'s left read; `??`
/// carries no `Intent::Operator` mark so `fold_binary` early-returns, the
/// void use fires at the left operand's own span).
#[test]
fn ticket_3176_if_null_left_void() {
    let fixture = "void v() {}\nvoid main() {\n  var x = v() ?? 1;\n}";
    let report = report_ops(fixture);
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.rfind("v()").unwrap();
    assert_void_at(&report, offset);
}

/// The `is` operand (`v() is int` — error_verifier.dart:1436
/// `checkForUseOfVoidResult(node.expression)`): the type-test consumes the
/// operand's value.
#[test]
fn ticket_3176_is_operand_void() {
    let fixture = "void v() {}\nvoid main() {\n  var b = v() is int;\n}";
    let report = report_ops(fixture);
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.rfind("v()").unwrap();
    assert_void_at(&report, offset);
}

/// The interpolation element (`"${v()}"` — error_verifier.dart:1429
/// `checkForUseOfVoidResult(node.expression)`): the element's value is
/// consumed into the string.
#[test]
fn ticket_3176_interpolation_element_void() {
    let fixture = "void v() {}\nvoid main() {\n  var s = \"${v()}\";\n}";
    let report = report_ops(fixture);
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.rfind("v()").unwrap();
    assert_void_at(&report, offset);
}

/// `await v()` (error_verifier.dart:459
/// `checkForUseOfVoidResult(node.expression)`): the await consumes the
/// operand's value.
#[test]
fn ticket_3176_await_operand_void() {
    let fixture =
        "void v() {}\nFuture<void> main() async {\n  await v();\n}";
    let report = report_ops(fixture);
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.rfind("v()").unwrap();
    assert_void_at(&report, offset);
}

/// `throw v()` (error_verifier.dart:2052
/// `checkForUseOfVoidResult(node.expression)`): the thrown value is
/// consumed — `use_of_void_result` reports beside `throw_of_invalid_type`.
#[test]
fn ticket_3176_throw_operand_void() {
    let fixture = "void v() {}\nvoid main() {\n  throw v();\n}";
    let report = report_ops(fixture);
    let names = code_names(&report);
    assert!(
        names.iter().any(|name| *name == "use_of_void_result"),
        "{:?}",
        rows(&report)
    );
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.rfind("v()").unwrap();
    let void_row = report
        .diagnostics
        .iter()
        .find(|d| d.message == VOID_MSG)
        .expect("use_of_void_result row");
    assert_eq!(void_row.range.offset as usize, offset, "{:?}", rows(&report));
}

/// `yield v()` inside a `sync*`/`async*` generator
/// (yield_statement_resolver.dart:181/:198 — both arms run
/// `_checkForUseOfVoidResult(node.expression)`).
#[test]
fn ticket_3176_yield_operand_void() {
    let fixture =
        "void v() {}\nIterable<int> main() sync* {\n  yield v();\n}";
    let report = report_ops(fixture);
    let names = code_names(&report);
    assert!(
        names.iter().any(|name| *name == "use_of_void_result"),
        "{:?}",
        rows(&report)
    );
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.rfind("v()").unwrap();
    let void_row = report
        .diagnostics
        .iter()
        .find(|d| d.message == VOID_MSG)
        .expect("use_of_void_result row");
    assert_eq!(void_row.range.offset as usize, offset, "{:?}", rows(&report));
}

/// A non-`!` prefix unary (`-v()`/`~v()`/`++v()`/`--v()` — upstream
/// `visitPrefixExpression` :1727 `operatorType != BANG` →
/// `checkForUseOfVoidResult(operand)`): the operator consumes the
/// operand's value and reports `use_of_void_result` at the operand span.
#[test]
fn ticket_3176_prefix_minus_void() {
    let fixture = "void v() {}\nvoid main() {\n  -v();\n}";
    let report = report_ops(fixture);
    let names = code_names(&report);
    assert!(
        names.iter().any(|name| *name == "use_of_void_result"),
        "{:?}",
        rows(&report)
    );
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.rfind("v()").unwrap();
    let void_row = report
        .diagnostics
        .iter()
        .find(|d| d.message == VOID_MSG)
        .expect("use_of_void_result row");
    assert_eq!(void_row.range.offset as usize, offset, "{:?}", rows(&report));
}

/// A list element's `void` expression (`[v()]` —
/// literal_element_verifier.dart:95 `checkElement` →
/// `checkForUseOfVoidResult(element)`).
#[test]
fn ticket_3176_list_element_void() {
    let fixture = "void v() {}\nvoid main() {\n  var l = [v()];\n}";
    let report = report_ops(fixture);
    let names = code_names(&report);
    assert!(
        names.iter().any(|name| *name == "use_of_void_result"),
        "{:?}",
        rows(&report)
    );
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.rfind("v()").unwrap();
    let void_row = report
        .diagnostics
        .iter()
        .find(|d| d.message == VOID_MSG)
        .expect("use_of_void_result row");
    assert_eq!(void_row.range.offset as usize, offset, "{:?}", rows(&report));
}

/// A map-entry `void` value (`{1: v()}` — literal_element_verifier.dart:
/// 166/:171 `_verifyMapLiteralEntryValue` → `checkForUseOfVoidResult(
/// entry.value)`).
#[test]
fn ticket_3176_map_value_void() {
    let fixture = "void v() {}\nvoid main() {\n  var m = {1: v()};\n}";
    let report = report_ops(fixture);
    let names = code_names(&report);
    assert!(
        names.iter().any(|name| *name == "use_of_void_result"),
        "{:?}",
        rows(&report)
    );
}
