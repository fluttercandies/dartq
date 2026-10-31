//! The #2345 pins — the flow-narrowed scrutinee drives EVERY
//! exhaustiveness face, not the declared type the push-time latch
//! carries. Upstream `ExhaustivenessValidator` reads
//! `scrutinee.typeOrThrow` (the flow-narrowed `staticType` at the switch
//! site) for the whole pass — per-case reachability, the covering gap,
//! and the default's reachability — and the statement's
//! `mustBeExhaustive` gate reads `isAlwaysExhaustive` over that same
//! narrowed type (constant_verifier.dart:1001-1002, :498).
//!
//! Oracle provenance (`dart analyze` 3.13.7):
//!
//! - `if (x is int)` narrowing + int-complete cases: silent — the
//!   narrowed `int` is exhausted by `case int i`, where the declared
//!   `dynamic`/`Object` was not.
//! - Without the promotion the same case set over the declared type
//!   reports `non_exhaustive_switch_expression`.
//! - `!= null` narrowing + the null case omitted over an enum: the
//!   non-null remainder is still exhaustive — silent.
//! - A sealed-class scrutinee narrowed by `is` missing one variant
//!   still reports `non_exhaustive_switch_statement` — the narrowed
//!   type is always-exhaustive where the declared `Object` was not.

use crate::diagnose::test_rig::{code_names, report_ops};

/// `is int` on a `dynamic` parameter narrows the scrutinee to `int`: the
/// `case int i` declaration covers the whole narrowed space, so the
/// switch expression — always must-be-exhaustive — stays silent. The
/// declared `dynamic` would never decide the face (its latch is also no
/// `Nullable`/`Named` deferral shape, so the parse-time row landed
/// verbatim before this pass).
#[test]
fn ticket_2345_is_narrowed_scrutinee_expression_silent() {
    let report = report_ops(
        "void f(dynamic x) { if (x is int) { var y = switch (x) { int i => 1 }; } }",
    );
    assert!(
        !code_names(&report)
            .iter()
            .any(|name| *name == "non_exhaustive_switch_expression"),
        "{:?}",
        code_names(&report)
    );
}

/// The control: the same `case int i` over the UNPROMOTED `dynamic`
/// scrutinee is the covering gap — `non_exhaustive_switch_expression`
/// reports (the `dynamic` latch is no deferral shape, so the row is the
/// parse-time emission verbatim).
#[test]
fn ticket_2345_unpromoted_scrutinee_expression_reports() {
    let report =
        report_ops("void f(dynamic x) { var y = switch (x) { int i => 1 }; }");
    let names = code_names(&report);
    assert!(
        names
            .iter()
            .any(|name| *name == "non_exhaustive_switch_expression"),
        "{names:?}"
    );
}

/// `!= null` narrowing over a nullable enum: the non-null remainder is
/// exhausted by the three constants — the null witness the declared
/// `Color?` reported drops out under the narrowed `Color`, and the
/// statement stays silent.
#[test]
fn ticket_2345_not_null_narrowed_statement_silent() {
    let report = report_ops(
        "enum Color { red, green, blue } \
         void f(Color? c) { if (c != null) { switch (c) { \
           case Color.red: break; case Color.green: break; case Color.blue: break; } } }",
    );
    assert!(
        !code_names(&report)
            .iter()
            .any(|name| *name == "non_exhaustive_switch_statement"),
        "{:?}",
        code_names(&report)
    );
}

/// The negative pin — narrowing must not suppress a real gap: `x`
/// promoted from `Object` to the sealed `S` makes the switch
/// must-be-exhaustive, and the missing `B` arm still reports
/// `non_exhaustive_switch_statement` rendered over the narrowed `S`.
#[test]
fn ticket_2345_narrowed_sealed_missing_variant_still_reports() {
    let report = report_ops(
        "sealed class S {} class A extends S {} class B extends S {} \
         void f(Object x) { if (x is S) { switch (x) { case A a: break; } } }",
    );
    let names = code_names(&report);
    assert!(
        names
            .iter()
            .any(|name| *name == "non_exhaustive_switch_statement"),
        "{names:?}"
    );
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "non_exhaustive_switch_statement")
        .expect("the narrowed-sealed covering row");
    assert_eq!(
        row.message,
        "The type 'S' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'B()'."
    );
}

/// Reachability re-decides too: under the narrowed `Color` the earlier
/// `case Color c` covers the whole space, so the trailing `Color.red`
/// head is `unreachable_switch_case` — a row the declared-`Object`
/// verdict never produced.
#[test]
fn ticket_2345_narrowed_scrutinee_unreachable_case_reports() {
    let report = report_ops(
        "enum Color { red, green, blue } \
         void f(Object x) { if (x is Color) { switch (x) { \
           case Color c: break; case Color.red: break; } } }",
    );
    let names = code_names(&report);
    assert!(
        names
            .iter()
            .any(|name| *name == "unreachable_switch_case"),
        "{names:?}"
    );
}
