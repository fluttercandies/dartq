//! #3394 pins — the `invocation.rs` half of recon bucket C (instance
//! creation downward inference) and the T2 cascade-receiver residual.
//!
//! # T2 — a creation cascade target is an INSTANCE, never the `Type`
//! literal
//!
//! A dot-send on a class head (`C.named(...)`) whose declared return the
//! name-level read cannot decide leaves the merged row `Unknown`
//! ([`Engine::callee_return_ty`]'s generic-constructor arm), and the
//! cascade section over it used to inherit the unknown dispatch — whose
//! pass-2 re-read of the target span answers the bare head's `Type`
//! instance and reports every section against `Type`. Upstream types the
//! `InstanceCreationExpression` and resolves the next member on the
//! created class (probe 3.13.7: `G<T> { G.value(T t); void n() { } }` —
//! `G.value(1)..n()` reports nothing, `G.value(1)..gone()` reports "The
//! method 'gone' isn't defined for the type 'G'."), so
//! [`Engine::merge_member`] now classifies that row as the created
//! instance.
//!
//! # Bucket C — the created type is pinned before the constraints
//!
//! `pkg/_fe_analyzer_shared/lib/src/type_inference/
//! type_analyzer_operations.dart`'s `inferTypeParameterFromAll` returns the
//! parameter's previous (downward) inference for a covariant parameter
//! instead of merging the argument constraint, so a context-seeded
//! creation keeps its context instantiation and the conflicting argument
//! reports through the ordinary assignability walk. Two sites carry that
//! here: [`Engine::creation_call_site_ty`]'s seed is no longer poisoned by
//! a disagreeing argument, and the explicit-keyword spelling
//! ([`Engine::check_explicit_creation_arguments`]) now runs the
//! per-argument verdict the callee-linked spellings already ran.

use super::test_rig::{assert_silent, report_ops};
use super::*;

/// Every `code` row of `report` as `(offset, length, message)`.
fn rows_of(report: &FileReport, code: &str) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

fn count(report: &FileReport, code: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .count()
}

fn names(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// A GENERIC class's named-constructor creation as a cascade target: the
/// section resolves on the created instance, so a member the class
/// declares reports nothing.
#[test]
fn generic_named_creation_cascade_member_resolves() {
    let report = report_ops(
        "class G<T> { G.value(T t); void n() {} } \
         void f() { G.value(1)..n(); }",
    );
    assert_silent(&report);
}

/// The miss side: the section's verdict names the CREATED class, never the
/// `Type` literal the bare head would answer.
#[test]
fn generic_named_creation_cascade_miss_names_the_class() {
    let report = report_ops(
        "class G<T> { G.value(T t); } \
         void f() { G.value(1)..gone(); }",
    );
    assert_eq!(
        count(&report, "undefined_method"),
        1,
        "rows: {:?}",
        names(&report)
    );
    let rows = rows_of(&report, "undefined_method");
    assert!(
        rows[0].2.contains("isn't defined for the type 'G'."),
        "the section must name the created class, got {:?}",
        rows[0].2
    );
}

/// The plain-chain twin (`G.value(1).gone()`): the same created-instance
/// receiver, so the chained access names the class too.
#[test]
fn plain_chain_over_generic_named_creation_names_the_class() {
    let report = report_ops(
        "class G<T> { G.value(T t); } \
         void f() { G.value(1).gone(); }",
    );
    assert_eq!(
        count(&report, "undefined_method"),
        1,
        "rows: {:?}",
        names(&report)
    );
    let rows = rows_of(&report, "undefined_method");
    assert!(
        rows[0].2.contains("isn't defined for the type 'G'."),
        "the chained access must name the created class, got {:?}",
        rows[0].2
    );
}

/// A non-generic named-creation cascade target keeps its own face (the
/// pre-existing behaviour the generic arm must not disturb).
#[test]
fn non_generic_named_creation_cascade_miss_names_the_class() {
    let report = report_ops(
        "class D { D.named(int n); } \
         void f() { D.named(1)..gone(); }",
    );
    let rows = rows_of(&report, "undefined_method");
    assert_eq!(rows.len(), 1, "rows: {:?}", names(&report));
    assert!(
        rows[0].2.contains("isn't defined for the type 'D'."),
        "got {:?}",
        rows[0].2
    );
}

/// The context-seeded creation keeps its DOWNWARD instantiation when an
/// argument disagrees: `A<int, String> a = new A('s', 1)` types the
/// creation `A<int, String>` and reports each conflict at its own
/// argument — the oracle's two rows, never a to-bounds
/// `A<dynamic, dynamic>`.
#[test]
fn context_seeded_creation_reports_each_conflicting_argument() {
    let report = report_ops(
        "class A<T, U> { A(T t, U u); } \
         void f() { A<int, String> a = new A('s', 1); }",
    );
    let rows = rows_of(&report, "argument_type_not_assignable");
    assert_eq!(rows.len(), 2, "rows: {:?}", names(&report));
    assert!(
        rows[0]
            .2
            .contains("The argument type 'String' can't be assigned to the parameter type 'int'."),
        "got {:?}",
        rows[0].2
    );
    assert!(
        rows[1]
            .2
            .contains("The argument type 'int' can't be assigned to the parameter type 'String'."),
        "got {:?}",
        rows[1].2
    );
}

/// The agreeing twin: a context-seeded creation whose arguments satisfy
/// the pinned instantiation reports nothing.
#[test]
fn context_seeded_creation_agrees_silently() {
    let report = report_ops(
        "class A<T, U> { A(T t, U u); } \
         void f() { A<int, String> a = new A(1, 'x'); }",
    );
    assert_eq!(
        count(&report, "argument_type_not_assignable"),
        0,
        "rows: {:?}",
        names(&report)
    );
}

/// The explicit-keyword spelling outside any context: the argument verdict
/// runs on the declared formal, so the mismatch reports where the
/// callee-linked spelling already reported.
#[test]
fn keyword_creation_reports_unseeded_argument_mismatch() {
    let report = report_ops(
        "class A<T> { A(T t, int n); } \
         void f() { new A(1, 'x'); }",
    );
    let rows = rows_of(&report, "argument_type_not_assignable");
    assert_eq!(rows.len(), 1, "rows: {:?}", names(&report));
    assert!(
        rows[0]
            .2
            .contains("The argument type 'String' can't be assigned to the parameter type 'int'."),
        "got {:?}",
        rows[0].2
    );
}
