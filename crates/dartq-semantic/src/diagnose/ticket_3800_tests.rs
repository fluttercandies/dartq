//! Ticket #3800 pins: `const_initialized_with_non_constant_value` anchor
//! and classification faces the corpus census still misses.
//!
//! Two roots, both oracle-verified against Dart SDK 3.13.7:
//!
//! * A declared LOCAL FUNCTION is an `ExecutableElement` callee, so the
//!   constant evaluator answers `constEvalMethodInvocation` at the call
//!   (`const i = x();` over `int x() => 1;`) — never the initializer's own
//!   site default. A local that merely HOLDS a function (a variable, a
//!   function-typed formal) stays a resolver-rewritten
//!   `FunctionExpressionInvocation`, whose failure is the site default.
//! * A function literal is rejected at the whole `FunctionExpression`
//!   (`InvalidConstant.genericError`), so `const a = () {};` anchors at
//!   `() {}` and `const b = () => 1;` at `() => 1`, not at the body token
//!   the const capture folds the literal to.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// The exact anchored source text of every row carrying `code`, in
/// diagnostic order.
fn spans_of(report: &FileReport, full: &str, code: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| {
            full[d.range.offset as usize..(d.range.offset + d.range.length) as usize].to_string()
        })
        .collect()
}

/// `const i = x();` over a local arrow-bodied function: the oracle's row
/// is `const_eval_method_invocation` at the call, with no site-default
/// twin (corpus `const_functions_block_test` shape).
#[test]
fn local_function_call_is_a_method_invocation() {
    let fixture = "void m() {\n\
                   \x20 int x() => 1;\n\
                   \x20 const i = x();\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "const_eval_method_invocation"),
        ["x()"],
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
    assert!(
        spans_of(&report, &full, "const_initialized_with_non_constant_value").is_empty(),
        "const_init twin must not fire: {:?}",
        spans_of(&report, &full, "const_initialized_with_non_constant_value")
    );
}

/// The block-bodied local function answers the same way (corpus
/// `const_functions_recursion_test` / `const_functions_block_test`).
#[test]
fn block_body_local_function_call_is_a_method_invocation() {
    let fixture = "void m() {\n\
                   \x20 int b() { return 1; }\n\
                   \x20 const i = b();\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "const_eval_method_invocation"),
        ["b()"],
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
    assert!(spans_of(&report, &full, "const_initialized_with_non_constant_value").is_empty());
}

/// The counter direction: a local VARIABLE holding a closure is a value
/// call, so the initializer's own code still fires at the call — the local
/// function pin above must not swallow it.
#[test]
fn local_closure_variable_call_keeps_the_site_default() {
    let fixture = "void m() {\n\
                   \x20 var f = () => 1;\n\
                   \x20 const i = f();\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "const_initialized_with_non_constant_value"),
        ["f()"],
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
    assert!(spans_of(&report, &full, "const_eval_method_invocation").is_empty());
}

/// `const a = () {};` — the row spans the whole block-bodied literal.
#[test]
fn block_body_function_literal_anchors_whole() {
    let fixture = "const a = () {};\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "const_initialized_with_non_constant_value"),
        ["() {}"],
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

/// `const b = () => 1;` — the row spans the literal through its body
/// expression, the `;` excluded.
#[test]
fn arrow_function_literal_anchors_whole() {
    let fixture = "const b = () => 1;\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "const_initialized_with_non_constant_value"),
        ["() => 1"],
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

/// A literal whose head carries a type-parameter list and an `async`
/// keyword still anchors at the formal list (`<T>() async => x`), and a
/// plain non-constant initializer keeps its own anchor — the widening is
/// shape-gated, not status-wide.
#[test]
fn function_literal_extent_is_shape_gated() {
    let fixture = "const c = <int>() async => 1;\n\
                   var v = 1;\n\
                   const d = v;\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "const_initialized_with_non_constant_value"),
        ["<int>() async => 1", "v"],
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

/// `x?.length` over a constant String/dynamic receiver is a legal constant
/// read — the oracle stays silent on the declaration's own face (corpus
/// `test_propertyAccess_nullAware_{string,dynamic}_length_*`).
#[test]
fn null_aware_length_on_string_and_dynamic_stays_silent() {
    for fixture in [
        "const String? s = 'foo';\nconst int? c = s?.length;\n",
        "const dynamic d = 'foo';\nconst int? c = d?.length;\n",
        "const dynamic d = null;\nconst int? c = d?.length;\n",
        "const String? s = null;\nconst int? c = s?.length;\n",
    ] {
        let full = format!("{CORE_PRELUDE}{fixture}");
        let report = report_ops(fixture);
        assert!(
            spans_of(&report, &full, "const_initialized_with_non_constant_value").is_empty(),
            "{fixture:?} must stay silent, got {:?}",
            report
                .diagnostics
                .iter()
                .map(|d| d.code.name)
                .collect::<Vec<_>>()
        );
    }
}

/// `l?.length` over `const List? = null` short-circuits to null — the
/// oracle's `isDartCoreNull && isNullAware` fold (corpus
/// `test_propertyAccess_nullAware_list_length_null`); the declaration face
/// stays silent.
#[test]
fn null_aware_length_on_null_list_folds_to_null() {
    let fixture = "const List? l = null;\nconst int? c = l?.length;\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert!(
        spans_of(&report, &full, "const_initialized_with_non_constant_value").is_empty(),
        "the null short-circuit must not report, got {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

/// `l?.length` over `const List? = []` and `s?.isEven` over
/// `const int? = null` fault upstream — but as `const_eval_property_access`
/// on the property-access face, never as the declaration's own
/// `const_initialized_with_non_constant_value` row. The member fold must
/// not surface the site default here (corpus `test_nullAware_length_list_notNull`,
/// `test_nullAware_isEven_null`).
#[test]
fn null_aware_access_fault_belongs_to_the_property_access_face() {
    for fixture in [
        "const List? l = [];\nconst int? c = l?.length;\n",
        "const int? s = null;\nconst bool? c = s?.isEven;\n",
    ] {
        let full = format!("{CORE_PRELUDE}{fixture}");
        let report = report_ops(fixture);
        assert!(
            spans_of(&report, &full, "const_initialized_with_non_constant_value").is_empty(),
            "{fixture:?} must not carry the site default, got {:?}",
            report
                .diagnostics
                .iter()
                .map(|d| d.code.name)
                .collect::<Vec<_>>()
        );
    }
}

/// The `?.` fold keeps its receiver subtree: a provably non-constant
/// receiver under `?.` still reports at the receiver — the null-aware flag
/// does not swallow the operand's own verdict.
#[test]
fn null_aware_access_propagates_a_non_constant_receiver() {
    let fixture = "void m() {\n\
                   \x20 var v = 'x';\n\
                   \x20 const c = v?.length;\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "const_initialized_with_non_constant_value"),
        ["v"],
        "the non-constant receiver must still report, got {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}
