//! #3385 pins — the recovered formal slot still counts.
//!
//! The `arg_param_trailing_comma` family's `topBad*` variants open or split
//! a formal-parameter list with a stray separator (`void f(, a)`,
//! `void f(a,, b)`). The parser already counts the empty slot as a real
//! formal — `parse_formal_parameters_rest` increments `parameter_count` and
//! `ensureIdentifier` splices the same zero-width identifier upstream does —
//! and the oracle's `not_enough_positional_arguments` text agrees
//! ("2 positional arguments expected by 'topBadStart'").
//!
//! `end_formal_parameter` dropped that slot when `capture_name` answered
//! `None` for the parser-inserted synthetic name, so the signature's arity
//! came out one short and the oracle's row went missing. The fix keeps the
//! slot under the element model's empty spelling for a null identifier; the
//! slot still never joins the list's name scope, so the duplicate checks are
//! untouched.

use super::test_rig::report_ops;
use crate::diagnose::FileReport;

/// The `not_enough_positional_arguments` rows' rendered messages, in report
/// order.
fn arg_count_messages(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "not_enough_positional_arguments")
        .map(|d| d.message.to_string())
        .collect()
}

/// `void topBadStart(, a)` — the leading empty slot is a required positional,
/// so the one-argument call is short by two (oracle: "2 positional arguments
/// expected by 'topBadStart'").
#[test]
fn t3385_leading_empty_slot_counts_two_positionals() {
    let report = report_ops(
        "void topBadStart(, a) {} \
         void use() { topBadStart(1); }",
    );
    let messages = arg_count_messages(&report);
    assert_eq!(messages.len(), 1, "{:?}", report.diagnostics);
    assert!(
        messages[0].starts_with("2 positional arguments expected"),
        "{messages:?}"
    );
}

/// `void topBadMiddle(a,, b)` — the empty middle slot counts too (oracle:
/// "3 positional arguments expected").
#[test]
fn t3385_middle_empty_slot_counts_three_positionals() {
    let report = report_ops(
        "void topBadMiddle(a,, b) {} \
         void use() { topBadMiddle(1); }",
    );
    let messages = arg_count_messages(&report);
    assert_eq!(messages.len(), 1, "{:?}", report.diagnostics);
    assert!(
        messages[0].starts_with("3 positional arguments expected"),
        "{messages:?}"
    );
}

/// `void topBadEmpty(,)` — the single recovered slot is the whole arity, so a
/// zero-argument call is short by one.
#[test]
fn t3385_only_empty_slot_counts_one_positional() {
    let report = report_ops(
        "void topBadEmpty(,) {} \
         void use() { topBadEmpty(); }",
    );
    let messages = arg_count_messages(&report);
    assert_eq!(messages.len(), 1, "{:?}", report.diagnostics);
    assert!(
        messages[0].starts_with("1 positional argument expected"),
        "{messages:?}"
    );
}

/// Regression control: a well-formed signature keeps its arity — the
/// recovered-slot arm changes nothing for real names.
#[test]
fn t3385_well_formed_signature_keeps_arity() {
    let report = report_ops(
        "void f(int a, int b) {} \
         void use() { f(); }",
    );
    let messages = arg_count_messages(&report);
    assert_eq!(messages.len(), 1, "{:?}", report.diagnostics);
    assert!(
        messages[0].starts_with("2 positional arguments expected"),
        "{messages:?}"
    );
}

/// A named formal is unaffected: `{int x}` stays optional, so both a bare
/// call and a named one stay silent.
#[test]
fn t3385_named_formal_slot_unaffected() {
    let report = report_ops(
        "void g({int x}) {} \
         void use() { g(); g(x: 1); }",
    );
    let messages = arg_count_messages(&report);
    assert!(
        messages.is_empty(),
        "{messages:?} / {:?}",
        report.diagnostics
    );
}
