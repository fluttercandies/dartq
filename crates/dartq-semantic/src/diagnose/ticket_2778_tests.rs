//! The #2778 pins: the `unnecessary_type_check` emission face matches the
//! oracle's rows exactly — the 4,003-case family's three shapes.
//!
//! - The whole-expression span (`_checkAllTypeChecks` reports
//!   `.at(node)`, best_practices_verifier.dart:993-999): the verdict row
//!   anchors the operand's start and ends at the written type's end —
//!   `null is! Null` spans 13 columns (the oracle's report row for
//!   UnnecessaryTypeCheckFalseTest/test_null_isNot_Null is `2:9|13`),
//!   `a is int?` spans 9 (test_typeNullable_is_same `3:3|9`), a generic
//!   target ends at the group's `>`.
//! - The strict subtype gate (the dominant failing shape — dartq
//!   over-emitted 2,935 rows where the oracle is silent): the verdict's
//!   terminal ask is the declared-edge lattice, never the assignability
//!   face whose nullable-target Right Top answers `num? <: int?` true —
//!   `num? a; a is int?` is informative upstream
//!   (test_typeNullable_is_subtype's oracle row set is empty) and stays
//!   silent here, as do the unrelated-head and `dynamic`-operand pairs.
//! - The keyword operand (`this is C`, this_not_promoted_test's oracle
//!   row at the `isSameType` guard): the keyword pushes no row, so the
//!   verdict reads the enclosing container's `this` face and anchors at
//!   the keyword.

use super::test_rig::{report_ops, CORE_PRELUDE};
use super::*;

/// The exact `(offset, length)` rows of one code, source-ordered.
fn exact_rows(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// The fixture-relative anchor shifted by the rig's prepended prelude —
/// the diagnostics' offsets span prelude + fixture.
fn anchor(fixture: &str, needle: &str) -> u32 {
    CORE_PRELUDE.len() as u32 + fixture.find(needle).map(|i| i as u32).unwrap()
}

// —— the whole-expression span ——————————————————————————————————————

/// `null is! Null` reports constant-false over the whole is-expression:
/// the oracle row is `2:9|13` (test_null_isNot_Null — the operand `null`
/// starts the span, the checked `Null` ends it), not the bare operand.
#[test]
fn null_is_not_null_spans_the_whole_expression() {
    let source = "var b = null is! Null;\n";
    let report = report_ops(&source);
    assert_eq!(
        exact_rows(&report, "unnecessary_type_check"),
        vec![(anchor(&source, "null is"), 13)],
        "{:?}",
        report.diagnostics
    );
    assert_eq!(
        report.diagnostics[0].message,
        "Unnecessary type check; the result is always 'false'."
    );
}

/// `a is int?` over `int? a` reports constant-true over the whole
/// is-expression: the oracle row is `3:3|9` (test_typeNullable_is_same),
/// and the `is!` polarity carries the complement message at `3:3|10`
/// (test_typeNullable_isNot_same, 'false').
#[test]
fn nullable_same_type_spans_whole_expression_both_polarities() {
    let fixture = "void f(int? a) {\n  a is int?;\n}\nvoid g(int? a) {\n  a is! int?;\n}\n";
    let source = fixture;
    let report = report_ops(&source);
    let first = anchor(&source, "a is int?");
    let second = anchor(&source, "a is! int?");
    assert_eq!(
        exact_rows(&report, "unnecessary_type_check"),
        vec![(first, 9), (second, 10)],
        "{:?}",
        report.diagnostics
    );
}

/// A generic checked type extends the span to the group's `>`:
/// `o1 is Iterable<int>` over the same-typed operand covers the
/// arguments (the oracle's `.at(node)` — the armed head alone stops at
/// `Iterable`).
#[test]
fn generic_checked_type_spans_through_the_arguments() {
    let fixture = "void f(Iterable<int> o1) {\n  if (o1 is Iterable<int>) {}\n}\n";
    let source = fixture;
    let report = report_ops(&source);
    assert_eq!(
        exact_rows(&report, "unnecessary_type_check"),
        vec![(anchor(&source, "o1 is"), 19)],
        "{:?}",
        report.diagnostics
    );
}

// —— the strict subtype gate —————————————————————————————————————————

/// `num? a; a is int?` is informative: `num? <: int?` needs the
/// supertype graph (subtype.dart's interface rule), and the assignment
/// face's nullable-target Right Top must not answer it — the oracle's
/// row set for test_typeNullable_is_subtype is empty.
#[test]
fn unrelated_nullable_head_pair_stays_silent() {
    let source = "void f(num? a) {\n  a is int?;\n}\n";
    let report = report_ops(&source);
    assert!(
        exact_rows(&report, "unnecessary_type_check").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// The nullable-top operand against an unrelated head stays silent:
/// `Object? o1 is Iterable<int>?` (the assignment-resolution oracle rows
/// are empty — `Object?` proves nothing toward `Iterable`).
#[test]
fn nullable_object_operand_stays_silent() {
    let fixture = "void f(Object? o1) {\n  if (o1 is Iterable<int>?) {}\n}\n";
    let source = fixture;
    let report = report_ops(&source);
    assert!(
        exact_rows(&report, "unnecessary_type_check").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// A `dynamic` operand subtypes no named target, nullable or not: the
/// oracle stays silent (`isSubtypeOf(dynamic, int?)` is false — only the
/// top targets admit it).
#[test]
fn dynamic_operand_against_named_target_stays_silent() {
    let source = "void f(dynamic x) {\n  if (x is int?) {}\n}\n";
    let report = report_ops(&source);
    assert!(
        exact_rows(&report, "unnecessary_type_check").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// The non-subtype plain pair stays informative (upstream
/// test_typeNonNullable_isNot_subtype has no marker — the oracle is
/// silent), and the true subtype direction keeps reporting
/// (test_typeNonNullable_is_supertype's marker: `int a; a is! num` is
/// constant-false over the whole node).
#[test]
fn core_numeric_edges_decide_both_directions() {
    let silent = "void f(num a) {\n  a is! int;\n}\n";
    let report = report_ops(&silent);
    assert!(
        exact_rows(&report, "unnecessary_type_check").is_empty(),
        "{:?}",
        report.diagnostics
    );
    let source = "void f(int a) {\n  a is! num;\n}\n";
    let report = report_ops(&source);
    assert_eq!(
        exact_rows(&report, "unnecessary_type_check"),
        vec![(anchor(&source, "a is"), 9)],
        "{:?}",
        report.diagnostics
    );
}

// —— `is dynamic` —————————————————————————————————————————————————————

/// `a is dynamic` is constant-true whatever the operand: the oracle row
/// is `3:3|12` (test_type_is_dynamic) with the always-true message.
#[test]
fn is_dynamic_reports_over_the_whole_expression() {
    let source = "void f(Object? a) {\n  a is dynamic;\n}\n";
    let report = report_ops(&source);
    assert_eq!(
        exact_rows(&report, "unnecessary_type_check"),
        vec![(anchor(&source, "a is"), 12)],
        "{:?}",
        report.diagnostics
    );
}

// —— the keyword operand ——————————————————————————————————————————————

/// `this is C` inside `C` is constant-true (this_not_promoted_test's
/// `isSameType` guard carries the oracle row): the keyword is the
/// operand — typed by the enclosing container — and the row anchors at
/// `this`, spanning the whole expression.
#[test]
fn this_operand_folds_and_anchors_at_the_keyword() {
    let fixture = "class C {\n  void f() {\n    if (this is C) {\n      1;\n    }\n  }\n}\n";
    let source = fixture;
    let report = report_ops(&source);
    assert_eq!(
        exact_rows(&report, "unnecessary_type_check"),
        vec![(anchor(&source, "this is"), 9)],
        "{:?}",
        report.diagnostics
    );
}

/// An extension's `this` reads its `on` type (`this is int` over
/// `extension on int` is constant-true) — the same `thisType` face the
/// keyword's flow stage adopts.
#[test]
fn extension_this_reads_the_on_type() {
    let fixture = "extension E on int {\n  void f() {\n    if (this is int) {\n      1;\n    }\n  }\n}\n";
    let source = fixture;
    let report = report_ops(&source);
    assert_eq!(
        exact_rows(&report, "unnecessary_type_check"),
        vec![(anchor(&source, "this is"), 11)],
        "{:?}",
        report.diagnostics
    );
}
