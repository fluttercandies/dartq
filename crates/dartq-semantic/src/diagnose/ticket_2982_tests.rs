//! The #2982 pins: the always-`Null` code substitution at the
//! `unchecked_use_of_nullable_value` operand faces — the verifier's
//! `report()` swaps the reported code to `invalid_use_of_null_value`
//! whenever `receiverType == typeProvider.nullType` (upstream Dart SDK
//! source,
//! `pkg/analyzer/lib/src/error/nullable_dereference_verifier.dart:47-49`).
//! A merely-NULLABLE operand keeps the unchecked code; only the
//! statically-`Null` face swaps.
//!
//! The consumer faces under pin are `stage_operand_faces`'s two sites —
//! the for-in iterable (`for_resolver.dart:88/:178`, the verifier's
//! `.at(iterable)` after the iterable resolves) and the non-null-aware
//! spread operand (`type_analyzer.dart`'s `visitSpreadElement`, gated on
//! `!node.isNullAware`) — both reached through the read-row's flow-aware
//! static type, the same payload the nullable arm reads.
//!
//! Oracle provenance: `InvalidUseOfNullValueTest/test_forLoop` (the
//! `Null x; for (var y in x)` row reports `invalid_use_of_null_value` at
//! the iterable) and `NotNullAwareNullSpreadTest`'s `nullTyped` rows
//! (`invalid_use_of_null_value` beside `not_null_aware_null_spread`).

use super::test_rig::{code_names, report_ops, rows};

/// `Null x; for (var y in x)` — the iterable read's `Ty::Null` face swaps
/// the row to `invalid_use_of_null_value` at the iterable's own span;
/// no `unchecked_use_of_nullable_value` twin stands beside it.
#[test]
fn null_typed_for_in_iterable_reports_invalid_use() {
    let report = report_ops("void m() { Null x; for (var y in x) { y; } }\n");
    assert_eq!(
        code_names(&report),
        vec!["invalid_use_of_null_value"],
        "{:?}",
        rows(&report)
    );
}

/// `Null x; var l = [...x]` — the non-null-aware spread's `Null` operand
/// swaps to `invalid_use_of_null_value` at the operand's own span.
#[test]
fn null_typed_spread_operand_reports_invalid_use() {
    let report = report_ops("void m() { Null x; var l = [...x]; }\n");
    assert!(
        code_names(&report).contains(&"invalid_use_of_null_value"),
        "{:?}",
        rows(&report)
    );
    assert!(
        !code_names(&report).contains(&"unchecked_use_of_nullable_value"),
        "{:?}",
        rows(&report)
    );
}

/// The merely-nullable for-in iterable keeps the unchecked code — the
/// substitution keys on `Ty::Null` alone (`int?` is not the `NullType`).
#[test]
fn nullable_for_in_iterable_keeps_unchecked_use() {
    let report = report_ops("void m(int? x) { for (var y in x) { y; } }\n");
    assert_eq!(
        code_names(&report),
        vec!["unchecked_use_of_nullable_value"],
        "{:?}",
        rows(&report)
    );
}
