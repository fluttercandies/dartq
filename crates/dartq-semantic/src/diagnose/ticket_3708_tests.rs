//! #3708 pins — `assignment_to_final_local` on pattern-assignment targets.
//!
//! `handle_assigned_variable_pattern` resolves its target at parse time —
//! no row reaches the pass-2 write face, so the
//! `checkFinalAlreadyAssigned` mirror (`pa_final_local_face` +
//! `pa_prior_write` + `pa_captured_write`, the `pa_`-prefixed port of
//! `row_resolution.rs`'s `final_local_face` cluster) runs inside the hook
//! before the `assignedVariables` ledger note. Upstream that is
//! `resolveAssignedVariablePattern` (Dart SDK source,
//! `pkg/analyzer/lib/src/dart/resolver/resolver.dart:1429-1455`): a
//! resolved LOCAL target that is `final` and already assigned reports at
//! the name.
//!
//! Oracle-verbatim message: `The final variable 'af' can only be set once.`

use super::test_rig::{report_ops, rows};
use crate::diagnose::FileReport;

/// The `assignment_to_final_local` rows as `(offset, message)` pairs,
/// ascending.
fn atfl_rows(report: &FileReport) -> Vec<(u32, String)> {
    let mut rows: Vec<(u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "assignment_to_final_local")
        .map(|d| (d.range.offset, d.message.to_string()))
        .collect();
    rows.sort_unstable();
    rows
}

/// `needle`'s absolute offset inside `fixture` as the rig analyzes it (the
/// prelude shifts every fixture-local offset).
fn at(fixture: &str, needle: &str) -> u32 {
    let hit = fixture
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in fixture:\n{fixture}"));
    u32::try_from(super::test_rig::CORE_PRELUDE.len() + hit).expect("fixture offsets fit u32")
}

/// `final af = 0; (af, _) = (1, 2);` — an initialized `final` local as a
/// pattern-assignment target: reports at the target's own name.
#[test]
fn pattern_assignment_to_initialized_final_reports() {
    let fixture = "void f() {\n  final af = 0;\n  (af, _) = (1, 2);\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        atfl_rows(&report),
        vec![
            (at(fixture, "af, _"), "The final variable 'af' can only be set once.".to_string()),
        ],
        "{:#?}",
        rows(&report)
    );
}

/// The non-final control: `var af = 0; (af, _) = (1, 2);` stays silent —
/// the verdict must not fire on a writable target.
#[test]
fn pattern_assignment_to_var_is_silent() {
    let fixture = "void f() {\n  var af = 0;\n  (af, _) = (1, 2);\n  af;\n}\n";
    let report = report_ops(fixture);
    assert!(
        atfl_rows(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// `final af; (af, _) = (1, 2);` — the FIRST write to a declared-but-
/// unassigned `final` is legal: silent.
#[test]
fn pattern_assignment_first_write_to_unassigned_final_is_silent() {
    let fixture = "void f() {\n  final af;\n  (af, _) = (1, 2);\n  af;\n}\n";
    let report = report_ops(fixture);
    assert!(
        atfl_rows(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// `final af; af = 0; (af, _) = (1, 2);` — the prior plain assignment
/// makes the pattern write the second: reports.
#[test]
fn pattern_assignment_after_prior_write_reports() {
    let fixture = "void f() {\n  final af;\n  af = 0;\n  (af, _) = (1, 2);\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        atfl_rows(&report),
        vec![
            (at(fixture, "af, _"), "The final variable 'af' can only be set once.".to_string()),
        ],
        "{:#?}",
        rows(&report)
    );
}

/// A `final` FORMAL parameter as a pattern-assignment target reports —
/// a formal is bound on entry.
#[test]
fn pattern_assignment_to_final_formal_reports() {
    let fixture = "void f(final af) {\n  (af, _) = (1, 2);\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        atfl_rows(&report),
        vec![
            (at(fixture, "af, _"), "The final variable 'af' can only be set once.".to_string()),
        ],
        "{:#?}",
        rows(&report)
    );
}

/// A mutable formal stays silent — the keyword decides, not the site.
#[test]
fn pattern_assignment_to_mutable_formal_is_silent() {
    let fixture = "void f(af) {\n  (af, _) = (1, 2);\n  af;\n}\n";
    let report = report_ops(fixture);
    assert!(
        atfl_rows(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// A write inside a nested closure to an enclosing `final` reports even
/// when it is the variable's only write site — the captured variable is
/// `potentiallyMutatedInScope` upstream.
#[test]
fn pattern_assignment_inside_closure_to_enclosing_final_reports() {
    let fixture = "void f() {\n  final af;\n  g() { (af, _) = (1, 2); }\n  g;\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        atfl_rows(&report),
        vec![
            (at(fixture, "af, _"), "The final variable 'af' can only be set once.".to_string()),
        ],
        "{:#?}",
        rows(&report)
    );
}

/// A prior write on ONE branch of an `if` still assigns on that path —
/// the pattern write after it reports.
#[test]
fn pattern_assignment_after_one_sided_prior_write_reports() {
    let fixture = "void f(bool b) {\n  final af;\n  if (b) { af = 1; }\n  (af, _) = (1, 2);\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        atfl_rows(&report),
        vec![
            (at(fixture, "af, _"), "The final variable 'af' can only be set once.".to_string()),
        ],
        "{:#?}",
        rows(&report)
    );
}

/// A second pattern assignment to the same `final` reports — the first
/// pattern write was already its (legal) assignment.
#[test]
fn second_pattern_assignment_to_final_reports() {
    let fixture = "void f() {\n  final af;\n  (af, _) = (1, 2);\n  (af, _) = (3, 4);\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        atfl_rows(&report),
        vec![
            (at(fixture, "af, _) = (3, 4)"), "The final variable 'af' can only be set once.".to_string()),
        ],
        "{:#?}",
        rows(&report)
    );
}

/// Two targets in one pattern: only the `final` one reports; the `var`
/// sibling stays silent.
#[test]
fn pattern_assignment_only_final_target_reports() {
    let fixture = "void f() {\n  final af = 0;\n  var bg = 0;\n  (af, bg) = (1, 2);\n  bg;\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        atfl_rows(&report),
        vec![
            (at(fixture, "af, bg"), "The final variable 'af' can only be set once.".to_string()),
        ],
        "{:#?}",
        rows(&report)
    );
}
