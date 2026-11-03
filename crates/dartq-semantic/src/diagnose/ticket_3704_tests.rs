//! #3704 pins — a named function expression never reports `unused_element`.
//!
//! The oracle's `UnusedLocalElementsVerifier.visitFunctionExpression` adds
//! the element to `usedElements` unconditionally (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/error/unused_local_elements_verifier.dart`
//! `visitFunctionExpression` — `addElement` under the
//! `!_enclosingExec`/`_enclosingClass` gates): a named function expression
//! is used by construction and has no `unused_element` face at all. The
//! listener staged its name through the local-function enqueue anyway
//! (`C::LocalFunctionDeclaration` → `note_local_function` →
//! `finish_local_fn_site`), so `var f = () { void g() {} }`-shape heads
//! fabricated an `unused_element 'g'`.
//!
//! The control matters as much as the positive pins: a NAMED local
//! function DECLARATION (`void f() { void inner() {} }`) is still a
//! `visitFunctionDeclaration` face upstream and must keep reporting —
//! dropping the pending slot only removes the expression flavor's
//! enqueue.

use super::test_rig::{report_ops, rows};
use crate::diagnose::FileReport;

/// The `unused_element` rows' messages, in source order.
fn unused_element_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_element")
        .map(|d| d.message.to_string())
        .collect()
}

/// `void f() {}` written in EXPRESSION position (`assert (a, void f() {})`
/// — the named-function-expression recovery shape) is `visitFunctionExpression`'s
/// `addElement` face upstream: silent.
///
/// Oracle: no `unused_element` row; the parser's `named_function_expression`
/// row is the only face the shape carries.
#[test]
fn named_function_expression_in_argument_is_silent() {
    let report = report_ops("f() { assert (a, void f() {} }\n");
    assert!(
        unused_element_rows(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// A named function expression assigned to a variable is silent on the
/// function's own name — `var g = void h() {};` reports nothing on `h`.
#[test]
fn named_function_expression_initializer_is_silent() {
    let report = report_ops("main() { var g = void h() {}; g; }\n");
    assert!(
        unused_element_rows(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// The control: a NAMED local function declaration still reports when
/// never called — the declaration flavor keeps its `visitFunctionDeclaration`
/// face.
#[test]
fn unused_named_local_function_declaration_still_reports() {
    let report = report_ops("void outer() { void inner() {} }\nmain() { outer(); }\n");
    assert_eq!(
        unused_element_rows(&report),
        vec!["The declaration 'inner' isn't referenced.".to_string()],
        "{:#?}",
        rows(&report)
    );
}

/// A named function expression nested inside a used local function's body
/// is silent on the expression's name — while the outer declaration's own
/// verdict is untouched.
#[test]
fn nested_named_function_expression_is_silent() {
    let report = report_ops(
        "void outer() { inner(); void inner() { var g = void leaf() {}; g; } }\nmain() { outer(); }\n",
    );
    assert!(
        unused_element_rows(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}
