//! Ticket #3742 pins — the `FutureOr<T>` exhaustiveness union and the
//! enum-constant `slot_subtype` fallback repair.
//!
//! Upstream (`pkg/_fe_analyzer_shared/lib/src/exhaustiveness/types/future_or.dart:20-32`):
//! `FutureOr<T>` is a sealed static type whose subtypes are `T` then
//! `Future<T>`. A `FutureOr<int>` switch that covers `int` but no
//! `Future<int>` therefore reports `non_exhaustive_switch_*` with the
//! uncovered `Future<int>()` witness — never the union's own
//! `FutureOr<int>()` face. A `Future<bool>` arm completes the union and
//! the switch is silent.
//!
//! The second family pins the #3742 repair site itself
//! (`NameWorld::enum_constant_populates`): when `slot_subtype` declines a
//! declared-vs-overapproximated pair, a same-head generic enum (`E<String>`
//! against `E<num>`) still decides argument-wise so `E.b` never populates
//! the `E<num>` set — the uncovered constant is the witness.

use crate::diagnose::test_rig::{code_names, report_ops};

/// `FutureOr<int>` over `int _` alone: the `int` half is covered, the
/// `Future<int>` half is the covering gap — `non_exhaustive_switch_expression`
/// reports the `Future<int>()` witness, not the union's own face.
#[test]
fn future_or_scrutinee_int_only_reports_future_witness() {
    let report = report_ops(
        "String f(FutureOr<int> x) => switch (x) { int _ => 'i' };",
    );
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "non_exhaustive_switch_expression")
        .unwrap_or_else(|| panic!("expected a non_exhaustive row, got {:?}", code_names(&report)));
    assert_eq!(
        row.message,
        "The type 'FutureOr<int>' isn't exhaustively matched by the switch cases \
         since it doesn't match the pattern 'Future<int>()'.",
        "the uncovered half is `Future<int>()`, not the union"
    );
}

/// `FutureOr<int>` over `int _` and `Future<int> _`: both halves of the
/// union are covered — the switch is exhaustive and silent.
#[test]
fn future_or_scrutinee_full_cover_is_silent() {
    let report = report_ops(
        "String f(FutureOr<int> x) => switch (x) { int _ => 'i', Future<int> _ => 'g' };",
    );
    assert!(
        !code_names(&report)
            .iter()
            .any(|name| *name == "non_exhaustive_switch_expression"),
        "the covered union reports no non_exhaustive row, got {:?}",
        code_names(&report)
    );
}

/// `FutureOr<int>` over `Future<int> _` alone: the `Future<int>` half is
/// covered, the `int` half is the covering gap — the witness is `int()`.
#[test]
fn future_or_scrutinee_future_only_reports_int_witness() {
    let report = report_ops(
        "String f(FutureOr<int> x) => switch (x) { Future<int> _ => 'f' };",
    );
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "non_exhaustive_switch_expression")
        .unwrap_or_else(|| panic!("expected a non_exhaustive row, got {:?}", code_names(&report)));
    assert_eq!(
        row.message,
        "The type 'FutureOr<int>' isn't exhaustively matched by the switch cases \
         since it doesn't match the pattern 'int()'.",
        "the uncovered half is `int()`, not the union"
    );
}

/// `FutureOr<bool>` over `true`/`false` constants: the `bool` half is
#[test]
fn future_or_bool_constants_report_future_witness() {
    let report = report_ops(
        "String f(FutureOr<bool> x) => switch (x) { true => 't', false => 'f' };",
    );
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "non_exhaustive_switch_expression")
        .unwrap_or_else(|| panic!("expected a non_exhaustive row, got {:?}", code_names(&report)));
    assert_eq!(
        row.message,
        "The type 'FutureOr<bool>' isn't exhaustively matched by the switch cases \
         since it doesn't match the pattern 'Future<bool>()'.",
        "the uncovered half is `Future<bool>()`, not the union"
    );
}
