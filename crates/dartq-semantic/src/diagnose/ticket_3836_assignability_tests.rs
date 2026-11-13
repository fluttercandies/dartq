//! #3836 pins — Left Bottom on the assignability face: a non-nullable
//! `Never` source subtypes every target.
//!
//! Upstream `subtype.dart:69-71` answers the bottom source unconditionally
//! (`if (T0 is NeverTypeImpl && T0.nullabilitySuffix == none) return true;`),
//! ahead of every target decomposition. [`Engine::slot_subtype`] has carried
//! that arm since #2967; the assignability face — which that function's own
//! delegation tail re-enters — did not, so a bottom spelled through the
//! `Ty::Named` face answered `None` instead of the rule.
//!
//! Oracle (3.13.7, `dart analyze --format=machine` over a single-file
//! package) for the fixtures below, verbatim rows including the trailing
//! space `argument_type_not_assignable` carries:
//!
//! ```text
//! // Never boom() => throw 0;
//! // int i = boom(); double d = boom(); String s = boom();
//! (no rows)
//! // void takesNever(Never n) { if (n == null) throw 0; }
//! // void applyTakesNull(void Function(Null) fn) { fn(null); }
//! // main() { applyTakesNull(takesNever); }
//! ERROR|COMPILE_TIME_ERROR|ARGUMENT_TYPE_NOT_ASSIGNABLE|...|The argument
//!   type 'void Function(Never)' can't be assigned to the parameter type
//!   'void Function(Null)'.
//! ```
//!
//! The residual this arm does NOT reach — `void Function(Null)` against
//! `void Function(Never)`, an oracle-silent pair that still reports — is
//! decided by the argument face's own verdict staging (a face outside this
//! module), not by the pair walk these pins cover.

use super::FileReport;
use super::test_rig::report_ops;

/// Every `argument_type_not_assignable` message in report order, raw:
/// the message ends at `'. ` even with the appendix empty (the oracle's
/// machine-format face — messages.yaml:1363's `#additionalInfo`
/// substitution keeps the separating space).
fn argument_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == "argument_type_not_assignable")
        .map(|row| row.message.to_string())
        .collect()
}

/// Every `invalid_assignment` message, raw.
fn assignment_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == "invalid_assignment")
        .map(|row| row.message.to_string())
        .collect()
}

/// The arm's own direction at the assignment face: a `Never`-typed read
/// assigns into every declared target — the final primitives included.
/// Upstream answers no row here, and neither does the arm.
#[test]
fn never_read_assigns_into_every_declared_target() {
    let report = report_ops(
        "Never boom() => throw 0;\n\
         void main() {\n  int i = boom();\n  double d = boom();\n  String s = boom();\n  print([i, d, s]);\n}\n",
    );
    assert_eq!(assignment_rows(&report), Vec::<String>::new());
    assert_eq!(argument_rows(&report), Vec::<String>::new());
}

/// The arm is the BOTTOM's alone: the same contravariant formal leg still
/// refuses `void Function(Never)` against `void Function(Null)`, because
/// that pair asks `Null <: Never`, which the Left Null arm decides
/// negatively — a bottom is not a top, so no acceptance leaks across.
#[test]
fn nullable_formal_still_refuses_the_never_function() {
    let report = report_ops(
        "void takesNever(Never n) {\n  if (n == null) throw 0;\n}\n\
         void applyTakesNull(void Function(Null) fn) {\n  fn(null);\n}\n\
         void main() {\n  applyTakesNull(takesNever);\n}\n",
    );
    assert_eq!(
        argument_rows(&report),
        [
            "The argument type 'void Function(Never)' can't be assigned to the parameter type 'void Function(Null)'. "
        ],
    );
}
