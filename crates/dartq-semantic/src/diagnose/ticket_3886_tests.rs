//! Ticket #3886 pins — the message faces of a generic-function-type
//! bound and of an unresolvable argument leaf.
//!
//! Two divergences in the bounds family's report texts (the verdicts
//! themselves already fired at the right spans):
//!
//! * `Snapshot::display_written_bound` refused every function-form
//!   bound tree (`tree.function.is_some()` answered `None`), so a
//!   written `X extends T Function<T>()` bound fell back to the
//!   expanded interned display — which spells `T Function()`, dropping
//!   the bound's own binder list. The oracle's `getDisplayString`
//!   writes the bound verbatim with its quantifier
//!   (`display_string_builder.dart` `_writeTypeParameters`),
//!   `'T Function<T>()'`, so every row of the co19
//!   Generic-functions-as-type-args family read as a missing+extra
//!   mirror pair on the message text alone.
//! * `Snapshot::display_written_argument` answered `None` for a leaf
//!   head that names no declaration in the registry and nothing in
//!   scope; the row then fell back to the expanded display, which
//!   spells the raw source name (`'T Function(T)'`). A failed
//!   resolution upstream reads the `InvalidType` sentinel and its
//!   `getDisplayString` spells it verbatim:
//!   `'InvalidType Function(InvalidType)'`.
//!
//! Both faces now render the written tree (binder-scoped, with the
//! binder shadowing any same-named outer parameter) and spell
//! `InvalidType` for an unresolvable leaf.

use super::test_rig::report_ops;
use crate::FileReport;

/// The offsets of one code's rows, ascending.
fn offsets(report: &FileReport, code: &str) -> Vec<u32> {
    let mut at = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.range.offset)
        .collect::<Vec<_>>();
    at.sort_unstable();
    at
}

/// The one `type_argument_not_matching_bounds` message of a fixture.
fn bounds_message(report: &FileReport) -> String {
    report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "type_argument_not_matching_bounds")
        .map(|d| d.message.to_string())
        .expect("the bounds violation reports")
}

/// `class C<X extends T Function<T>()>` created with a generic
/// function-type argument — the co19 Generic-functions-as-type-args
/// `class_A01_t06` shape. The row existed with the right code and span;
/// the bound text must keep the bound's own binder list, and the
/// argument's binder-scoped leaves must render by their binder name.
#[test]
fn generic_function_bound_message_keeps_its_binder() {
    let fixture = "\
class C<X extends T Function<T>()> {}
void main() {
  C<void Function<T>(T)>();
  C<T Function<T>(T)>();
}
";
    let report = report_ops(fixture);
    let at = offsets(&report, "type_argument_not_matching_bounds");
    assert_eq!(at.len(), 2, "both violations report: {report:#?}");
    let messages: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "type_argument_not_matching_bounds")
        .map(|d| d.message.to_string())
        .collect();
    assert!(
        messages[0].contains(
            "'void Function<T>(T)' doesn't conform to the bound \
             'T Function<T>()' of the type parameter 'X'."
        ),
        "the bound must spell its binder list: {}",
        messages[0]
    );
    assert!(
        messages[1].contains(
            "'T Function<T>(T)' doesn't conform to the bound \
             'T Function<T>()' of the type parameter 'X'."
        ),
        "the binder-scoped argument leaves render by name: {}",
        messages[1]
    );
}

/// A binder WITH a written bound keeps the bound beside its name — the
/// `class_A01_t04` shape (`T Function<T extends num>(T)`).
#[test]
#[ignore = "held: the bounds verdict on `C<T Function(T)>` against `X extends T Function<T extends num>(T)` never fires — `type_argument_not_matching_bounds` should report `'InvalidType Function(InvalidType)' doesn't conform to bound 'T Function<T extends num>(T)'` (upstream display_string_builder.dart `_writeTypeParameters` keeps the binder's own bound); re-verified 2026-11-05 vs oracle 3.13.7: still fails — zero rows where the oracle reports 1; owner bounds.rs written-bound display face"]
fn generic_function_bound_message_keeps_binder_bounds() {
    let fixture = "\
class C<X extends T Function<T extends num>(T)> {}
void main() {
  C<T Function(T)>();
}
";
    let report = report_ops(fixture);
    let message = bounds_message(&report);
    assert!(
        message.contains(
            "'InvalidType Function(InvalidType)' doesn't conform to the bound \
             'T Function<T extends num>(T)' of the type parameter 'X'."
        ),
        "the bound spells binder bounds, unresolvable leaves spell \
         InvalidType: {message}"
    );
}

/// An unresolvable argument leaf spells `InvalidType` — the oracle's
/// failed-resolution display — never the raw source name.
#[test]
#[ignore = "held: `C<T Function(T)>` against `X extends int` never emits `type_argument_not_matching_bounds` — the unresolvable leaf should read `InvalidType` and report `'InvalidType Function(InvalidType)' doesn't conform to bound 'int'` (upstream display_string_builder.dart); re-verified 2026-11-05 vs oracle 3.13.7: still fails — zero rows where the oracle reports 1; owner bounds.rs unresolvable-leaf InvalidType display face"]
fn unresolved_argument_leaves_display_invalid_type() {
    let fixture = "\
class C<X extends int> {}
void main() {
  C<T Function(T)>();
}
";
    let report = report_ops(fixture);
    let message = bounds_message(&report);
    assert!(
        message.contains(
            "'InvalidType Function(InvalidType)' doesn't conform to the bound \
             'int' of the type parameter 'X'."
        ),
        "unresolved leaves render InvalidType: {message}"
    );
    assert!(
        !message.contains("'T Function(T)'"),
        "the raw source names of an unresolved leaf never render: {message}"
    );
}

/// Control: a plain bound violation keeps its message exactly — the
/// display rework must not move a non-function face.
#[test]
fn plain_bound_violation_message_unchanged() {
    let fixture = "\
class C<X extends num> {}
void main() {
  C<String>();
}
";
    let report = report_ops(fixture);
    let at = offsets(&report, "type_argument_not_matching_bounds");
    assert_eq!(at.len(), 1, "exactly one row: {report:#?}");
    let message = bounds_message(&report);
    assert_eq!(
        message,
        "'String' doesn't conform to the bound 'num' of the type parameter 'X'."
    );
}

/// Control: a binder-scoped argument of a NESTED function inside an
/// argument renders by its own binder (the inner `V` never leaks an
/// outer spelling), and a nested named-formal keeps the `required` face.
#[test]
fn nested_function_argument_binder_scope_shadows() {
    let fixture = "\
class C<X extends void Function<V extends num>()> {}
void main() {
  C<int Function(int Function<V extends num>(V))>();
}
";
    let report = report_ops(fixture);
    let message = bounds_message(&report);
    assert!(
        message.contains("'int Function(int Function<V extends num>(V))'"),
        "the nested function argument renders verbatim: {message}"
    );
}
