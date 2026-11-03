//! #3691 pins — `unused_local_variable`: the if-case cross-head
//! same-name binder join (census-20261102-230100: ~32 miss rows).
//!
//! Upstream mints one local element per `DeclaredVariablePattern`
//! (unused_local_elements_verifier.dart:565-579 — a logical-or's
//! same-name alternatives share one element through
//! `_patternVariableElements`, but two sequential `if (… case …)` heads
//! are two elements). `join_pattern_site_binder` once scanned back to
//! the frame mark, so the second head's `x` joined the first head's site
//! group and the first body's read spared it. The window binder floor
//! (`SwitchTracker::if_case_binder_floor`) bounds the scan to the open
//! head's own binders.
//!
//! Each pin carries the oracle's verdict for the same fixture
//! (`dart analyze` 3.13.7).

use super::test_rig::{CORE_PRELUDE, report_ops, rows};

/// `report_ops` mounts [`CORE_PRELUDE`] ahead of the fixture — every
/// expected offset shifts by its length.
fn at(source: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + source.find(needle).unwrap()) as u32
}

fn unused_local_rows(report: &super::FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_local_variable")
        .map(|d| format!("@{}+{}: {}", d.range.offset, d.range.length, d.message))
        .collect()
}

/// The verified repro: two sequential if-case heads each declaring `x`.
/// The first head's body reads `x`; the second head's binder is a
/// DISTINCT element and must still report.
///
/// Oracle: `unused_local_variable` at the second `x`, none at the first.
#[test]
fn second_if_case_binder_reports_first_spared() {
    let source = "void g(Object o) {}\nvoid main() {\n  if (123 as dynamic case int x) { g(x); }\n  if ('s' as dynamic case int x) {}\n}\n";
    let report = report_ops(source);
    let first = at(source, "int x") + 4;
    let second = (CORE_PRELUDE.len() + source.rfind("int x").unwrap()) as u32 + 4;
    let hits: Vec<u32> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_local_variable" && d.message.contains("'x'"))
        .map(|d| d.range.offset)
        .collect();
    assert_eq!(
        hits,
        vec![second],
        "first 'x' @{first} must stay spared; only the second @{second} reports: {:#?}",
        unused_local_rows(&report)
    );
}

/// The switch-case analogue: sequential `case` clauses declaring the
/// same binder name are arm-scoped elements — an unread one reports.
///
/// Oracle (`case int x: break; case String x: break;` over `Object`):
/// `unused_local_variable` at BOTH `x` tokens.
#[test]
fn sequential_switch_case_same_name_binders_report() {
    let source = "void f(Object o) {\n  switch (o) {\n    case int x:\n      break;\n    case String x:\n      break;\n    default:\n      break;\n  }\n}\n";
    let report = report_ops(source);
    let first = at(source, "int x") + 4;
    let second = at(source, "String x") + 7;
    let mut hits: Vec<u32> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_local_variable" && d.message.contains("'x'"))
        .map(|d| d.range.offset)
        .collect();
    hits.sort_unstable();
    assert_eq!(
        hits,
        vec![first, second],
        "each arm-scoped 'x' is its own element: {:#?}",
        rows(&report)
    );
}

/// Negative control: two if-cases with DIFFERENT binder names, both
/// read — no row. The floor must not fabricate reports.
///
/// Oracle: silent.
#[test]
fn two_if_cases_different_names_used_stay_silent() {
    let source = "void g(Object o) {}\nvoid main() {\n  if (123 as dynamic case int a) { g(a); }\n  if ('s' as dynamic case int b) { g(b); }\n}\n";
    let report = report_ops(source);
    let hits = unused_local_rows(&report);
    assert!(
        hits.is_empty(),
        "used binders must not report: {hits:#?}"
    );
}

/// Negative control: a single if-case whose binder is never read still
/// reports — the floor bounds only the SIBLING scan, never the first
/// head's own verdict.
///
/// Oracle (`if (o case int x) {}`): `unused_local_variable` at `x`.
#[test]
fn single_if_case_unused_binder_still_reports() {
    let source = "void f(Object o) {\n  if (o case int x) {}\n}\n";
    let report = report_ops(source);
    let x_at = at(source, "int x") + 4;
    let hit = report.diagnostics.iter().any(|d| {
        d.code.name == "unused_local_variable"
            && d.range.offset == x_at
            && d.range.length == 1
            && d.message.contains("'x'")
    });
    assert!(
        hit,
        "expected unused_local_variable for 'x' @{x_at}: {:#?}",
        rows(&report)
    );
}
