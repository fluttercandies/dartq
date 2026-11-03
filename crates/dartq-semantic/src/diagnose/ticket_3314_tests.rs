//! The #3314 pins (the pattern over-bind residual #3297 measured): an
//! if-case / switch-case binder is bound to its case scope alone, so a read
//! in the `else` branch, in a later sibling arm, or after the statement
//! falls through to the outer declaration it shadowed — or reports
//! `undefined_identifier` when it shadowed none.
//!
//! # The mechanism these pins lock
//!
//! `pattern_scope::retire_binders` stamps `LocalDecl::retired` on the rows a
//! closing case scope declared. `Frame::locals` is a name set, so the
//! membership outlives the scope and a shadowing binder shares its slot with
//! the declaration it hid; the row flag is what `Engine::resolve_bare` reads
//! to walk past a frame whose every same-named row is retired.
//!
//! Both directions are pinned: the out-of-scope reads (the fix) and the
//! reads that must keep resolving — the binder inside its own arm, a
//! shadowed outer declaration, and an outer pattern declaration a same-named
//! binder must not retire with it.
//!
//! Oracle provenance: `co19 case `LanguageFeatures/Patterns/scope_A04_t01`
//! /`t02`, `A06_t01`/`t02` (pinned 3.13.7 corpus), the marked sites being
//! the reported rows.

use super::test_rig::{code_names, report_ops};

/// Every `undefined_identifier` message of `report`, in emission order.
fn identifier_rows(report: &super::FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_identifier")
        .map(|d| d.message.to_string())
        .collect()
}

/// Assert the report carries exactly `expected` undefined-name rows.
#[track_caller]
fn assert_identifier_rows(report: &super::FileReport, expected: &[&str]) {
    assert_eq!(
        identifier_rows(report),
        expected
            .iter()
            .map(|row| (*row).to_owned())
            .collect::<Vec<_>>(),
        "{:?}",
        code_names(report)
    );
}

// — if-case ————————————————————————————————————————————————————————————

/// An if-case binder reads in the then branch (`scope_A04_t01` test3).
#[test]
fn if_case_binder_resolves_in_the_then_branch() {
    let report = report_ops("void test() {\n  if (42 case var v) {\n    v;\n  }\n}\n");
    assert_identifier_rows(&report, &[]);
}

/// ... and falls through in the `else` branch (`scope_A06_t01`): the case
/// scope closed, so the read is undefined.
#[test]
fn if_case_binder_is_out_of_scope_in_the_else_branch() {
    let report = report_ops(
        "void test() {\n  if (42 case var v) {\n    v;\n  } else {\n    v;\n  }\n}\n",
    );
    assert_identifier_rows(&report, &["Undefined name 'v'."]);
}

/// ... and after the statement (`scope_A04_t01` test3's second read).
#[test]
fn if_case_binder_is_out_of_scope_after_the_statement() {
    let report = report_ops("void test() {\n  if (42 case var v) {\n    v;\n  }\n  v;\n}\n");
    assert_identifier_rows(&report, &["Undefined name 'v'."]);
}

/// The `else` of a condition-only `if` is the statement end: the binder
/// retires there too.
#[test]
fn if_case_binder_is_out_of_scope_after_a_braceless_statement() {
    let report = report_ops("void test() {\n  if (42 case var v) v;\n  v;\n}\n");
    assert_identifier_rows(&report, &["Undefined name 'v'."]);
}

/// The shadowed declaration answers again in the `else` branch: the binder
/// retired, the outer local is what the spelling names.
#[test]
fn if_case_binder_shadowing_an_outer_local_reads_the_outer_in_else() {
    let report = report_ops(
        "void test(int e) {\n  if (42 case int e) {\n    e;\n  } else {\n    e;\n  }\n}\n",
    );
    assert_identifier_rows(&report, &[]);
}

/// A nested case scope closes into its own then-statement: the outer binder
/// stays readable after the inner statement.
#[test]
fn nested_if_case_binder_closes_into_its_own_then_statement() {
    let report = report_ops(
        "void test() {\n  if (42 case int x) {\n    if (42 case int y) {\n      x;\n      y;\n    }\n    x;\n  }\n}\n",
    );
    assert_identifier_rows(&report, &[]);
}

/// An outer pattern declaration is not the case scope's to retire: the
/// `var (a, b)` row keeps its spelling live after the statement.
#[test]
fn outer_pattern_declaration_survives_a_shadowing_if_case_binder() {
    let report = report_ops(
        "void test() {\n  var (a, b) = (1, 2);\n  if (42 case var a) {\n    a;\n  }\n  a;\n  b;\n}\n",
    );
    assert_identifier_rows(&report, &[]);
}

// — switch-case ———————————————————————————————————————————————————————

/// A switch-case binder reads in its own arm.
#[test]
fn switch_case_binder_resolves_in_its_own_arm() {
    let report = report_ops(
        "void test() {\n  switch (42) {\n    case var v:\n      v;\n      break;\n    default:\n      break;\n  }\n}\n",
    );
    assert_identifier_rows(&report, &[]);
}

/// ... and falls through in a later sibling arm (`scope_A04_t01` test1's
/// `case 0` / `default` reads): each guarded pattern introduces its own
/// case scope.
#[test]
fn switch_case_binder_is_out_of_scope_in_a_later_arm() {
    let report = report_ops(
        "void test() {\n  switch (42) {\n    case var v:\n      v;\n      break;\n    default:\n      v;\n  }\n}\n",
    );
    assert_identifier_rows(&report, &["Undefined name 'v'."]);
}

/// ... and after the switch statement, whose last member closed it.
#[test]
fn switch_case_binder_is_out_of_scope_after_the_switch() {
    let report = report_ops(
        "void test() {\n  switch (42) {\n    case var v:\n      break;\n    default:\n      break;\n  }\n  v;\n}\n",
    );
    assert_identifier_rows(&report, &["Undefined name 'v'."]);
}

/// The arm's binder retires at its own close, never an outer declaration of
/// the same spelling.
#[test]
fn switch_case_binder_shadowing_an_outer_local_reads_the_outer_in_a_later_arm() {
    let report = report_ops(
        "void test() {\n  int v = 0;\n  switch (42) {\n    case int v:\n      v;\n      break;\n    default:\n      v;\n  }\n}\n",
    );
    assert_identifier_rows(&report, &[]);
}

/// A nested switch's arm scope is its own: the outer arm's binder is not
/// retired by the inner member's close.
#[test]
fn nested_switch_arm_close_retires_only_its_own_binders() {
    let report = report_ops(
        "void test() {\n  switch (42) {\n    case int a:\n      switch (a) {\n        case int b:\n          b;\n          break;\n      }\n      a;\n      break;\n    default:\n      break;\n  }\n}\n",
    );
    assert_identifier_rows(&report, &[]);
}
