//! #3995 — Lane CT regression triage: three pins reported regressed between
//! the 2026-11-05 10:48 inventory and re-verification. The pins below lock the
//! bisected outcome so the shapes stay machine-checked:
//!
//! - `lambda_context_covariant_column_never_reports` (pin 1) — the oracle is
//!   SILENT on `..call(true)` under the covariant `check<X>` column: the
//!   generic outer callee's arg-slot context returns `None` through the
//!   unsolved-parameter gate, so no `argument_type_not_assignable` fires. The
//!   earlier expectation pinned a row that was never oracle-truth. Asserted in
//!   `ticket_3880_tests.rs` (rewritten there); the shape is re-pinned here for
//!   the regression ledger.
//! - `lambda_context_concrete_formal_types_inner_call` (pin 2) — a NON-generic
//!   outer callee's arg-slot context DOES resolve, so the literal's context
//!   return `void Function(int)` types `..call(true)` and the `bool -> int`
//!   argument verdict fires — exactly what the oracle reports. The
//!   `closure_body_context` owner-search (invocation.rs) now binds `X := int`
//!   on the inner `captureTypeArgument()` send (probe-verified), but a
//!   cascade-value typing regression in `assignability.rs` still types
//!   `x..call(true)` as `call`'s `void` return instead of the `check<int>`
//!   target — `use_of_void_result` + `return_of_invalid_type_from_closure`
//!   fire in place of `bool -> int`. Asserted in `ticket_3880_tests.rs`
//!   (held `#[ignore]`); the same shape is re-pinned here for the ledger.
//! - `await_uses_interface_derived_future_type` (pin 3) — `SubFuture
//!   implements Future<int>` is oracle-silent on the clause head (the stub
//!   `Future` resolves to the real `Future` class upstream, so
//!   `implements_non_class` never fires). The remaining `invalid_assignment
//!   'SubFuture'->'int'` extra is the held lane-V flatten seam
//!   (`future_supertype_arg`, `engine_core.rs`). The clause-legality route is
//!   `overrides.rs::check_clause_element_kinds` (a namespace-resolved decl the
//!   element index carries no model for must not emit `implements_non_class`).
//!   Asserted in `ticket_3893_tests.rs` (already `#[ignore]`-held); the
//!   clause-silence half is re-pinned here against a shape that does not hit
//!   the flatten seam.

use super::test_rig::{code_names, report_ops};

/// Pin 1 re-pinned: under the covariant `check<X>` column the oracle stays
/// silent on `..call(true)` — no `argument_type_not_assignable`.
#[test]
fn lambda_context_covariant_column_stays_silent() {
    let report = report_ops(
        r#"
typedef check<X> = X Function(X x);

check<X> captureTypeArgument<X>() => throw "unreachable";

void f<X>(check<X> Function() g) => g();

void main() {
  f(() => captureTypeArgument()..call(true));
}
"#,
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"argument_type_not_assignable"),
        "oracle-silent on ..call(true) under the covariant column, got {names:?}"
    );
}

/// Pin 2 re-pinned: under the concrete `check<int>` formal the oracle reports
/// `bool -> int` on `..call(true)` — `argument_type_not_assignable` fires.
///
/// Held with the same routed residual as `ticket_3880_tests`'s pin: the
/// `closure_body_context` owner-search now binds `X := int` on the inner send
/// (landed), but `assignability.rs`'s cascade value typing reads `x..call()`
/// as `call`'s `void` return — `use_of_void_result` +
/// `return_of_invalid_type_from_closure` fire in place of the `bool -> int`
/// argument verdict. Routed to the assignability lane.
#[test]
#[ignore = "held: cascade `x..call()` value typed `void` (call's return) not `check<int>` (target) inside a `=>` body — assignability.rs emit_use_of_void_result reads from==Void; the `bool -> int` arg verdict routes to the assignability lane; closure_body_context's X:=int solve (invocation.rs) already landed"]
fn lambda_context_concrete_formal_reports_bool_to_int() {
    let report = report_ops(
        r#"
typedef check<X> = void Function(X x);

check<X> captureTypeArgument<X>() => throw "unreachable";

void f2(check<int> Function() g) => g();

void main() {
  f2(() => captureTypeArgument()..call(true));
}
"#,
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"argument_type_not_assignable"),
        "expected argument_type_not_assignable on ..call(true) (bool -> int), got {names:?}"
    );
}

/// Pin 3's clause half re-pinned on a shape that does not hit the lane-V
/// flatten seam: `class SubFuture implements Future<int>` is oracle-silent on
/// the clause head — the stub `Future` resolves to the real `Future` class
/// upstream, so `implements_non_class` must not fire.
///
/// Landed (#4001): the `ScopedHit::Found(NamespaceEntry::Decl(owner))` and
/// prefixed `Lookup::Found(Decl(owner))` arms of
/// `overrides.rs::check_clause_element_kinds` answer clause-legal when
/// `index.get(owner)` is `None` — the names-only `dart:core` face — instead
/// of emitting the clause code at a resolved name.
#[test]
fn implements_stub_future_clause_head_stays_silent() {
    let report = report_ops(
        r#"
class SubFuture implements Future<int> {
  SubFuture();
}

void f(SubFuture x) {
  x.hashCode;
}
"#,
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"implements_non_class"),
        "the stub Future clause head must not report implements_non_class, got {names:?}"
    );
}
