//! #3728 pins — a switch expression over the names-only `Object` stub head
//! produces the covering gap the oracle reports.
//!
//! The `Object?` scrutinee's head resolves to dart:core's `Object` — in
//! these rigs a names-only stub (an unindexed declaration, the same
//! posture #3722's gate fix kept decidable). `NameWorld::name_resolvable`
//! already treats dart:core's `Enum` root as decided without a container;
//! `Object` is the same shape: not sealed, no coverable subtypes, and its
//! nullable split `[Object, Null]` is spelled by name alone
//! (`NameWorld::subtypes`'s `Named(_, true)` arm). An unindexed `Object`
//! is therefore the world's answer, not a blind spot — the expression
//! face's covering emit must not suppress over it.
//!
//! Oracle rows: dart 3.13.7 (`dart analyze --format=machine`) over the
//! fixtures below — `switch (o) { Object _ => 1 }` reports
//! `non_exhaustive_switch_expression` "doesn't match the pattern 'null'";
//! the same switch with a `_`/`Object?` catch-all stays silent; a sealed
//! root missing a case reports; a sealed root fully covered stays silent.

use super::test_rig::{code_names, report_ops};

/// The `non_exhaustive_switch_expression` messages of `report`, in source
/// order.
fn covering_rows(source: &str) -> Vec<String> {
    report_ops(source)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "non_exhaustive_switch_expression")
        .map(|d| d.message.to_string())
        .collect()
}

// —— the stub gap: `Object _` over `Object?` leaves `null` uncovered ————

/// The ticket's measured repro: `Object _` covers the non-null half of
/// `Object?`, so the `{null}` gap reports the covering row at `switch`.
/// The pre-fix world read the names-only `Object` head as unresolvable
/// and suppressed the emit outright — zero rows, not a gate drop.
#[test]
fn object_stub_scrutinee_reports_the_null_gap() {
    let rows = covering_rows("int f(Object? o) => switch (o) { Object _ => 1 };\n");
    assert_eq!(
        rows,
        [
            "The type 'Object?' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'null'."
        ]
    );
}

/// The same stub head with a narrower case still reports — the witness is
/// the non-null remainder `Object()` (oracle s1: `int _` over `Object?`).
#[test]
fn object_stub_scrutinee_subtype_case_reports_the_object_witness() {
    let rows = covering_rows("int f(Object? o) => switch (o) { int _ => 1 };\n");
    assert_eq!(
        rows,
        [
            "The type 'Object?' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'Object()'."
        ]
    );
}

// —— the exhaustive controls stay silent ————————————————————————————————

/// A catch-all arm after `Object _` covers the `null` half too — the
/// switch is exhaustive and the stub head reports nothing.
#[test]
fn object_stub_scrutinee_with_default_stays_silent() {
    let rows = covering_rows("int f(Object? o) => switch (o) { Object _ => 1, _ => 2 };\n");
    assert!(rows.is_empty(), "{rows:?}");
}

/// `Object? _` covers the whole space in one arm — silent.
#[test]
fn object_stub_scrutinee_nullable_case_stays_silent() {
    let rows = covering_rows("int f(Object? o) => switch (o) { Object? _ => 1 };\n");
    assert!(rows.is_empty(), "{rows:?}");
}

// —— the non-stub controls: a declared sealed root decides the same way ——

/// A declared sealed root missing a case still reports — the `Object`
/// arm never touches the declared-container path.
#[test]
fn sealed_scrutinee_missing_a_case_reports() {
    let rows = covering_rows(
        "sealed class S {} final class A extends S {} final class B extends S {} \
         int g(S s) => switch (s) { A _ => 1 };\n",
    );
    assert_eq!(
        rows,
        [
            "The type 'S' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'B()'."
        ]
    );
}

/// A declared sealed root with every case stays silent.
#[test]
fn sealed_scrutinee_fully_covered_stays_silent() {
    let rows = covering_rows(
        "sealed class S {} final class A extends S {} final class B extends S {} \
         int g(S s) => switch (s) { A _ => 1, B _ => 2 };\n",
    );
    assert!(rows.is_empty(), "{rows:?}");
}

// —— the suppression the fix must NOT undo ———————————————————————————————

/// A scrutinee head that resolves NOWHERE stays the oracle's
/// `InvalidType` — the gate drops the whole family (the #2336/#3722
/// preserved suppression; `Missing` is no declaration anywhere, not the
/// `Object` stub's unindexed-declaration shape).
#[test]
fn unresolvable_scrutinee_still_suppresses() {
    let report = report_ops("int f(Missing m) => switch (m) { Object _ => 1, _ => 2 };\n");
    assert!(
        !code_names(&report)
            .iter()
            .any(|name| *name == "non_exhaustive_switch_expression"),
        "{:?}",
        code_names(&report)
    );
}
