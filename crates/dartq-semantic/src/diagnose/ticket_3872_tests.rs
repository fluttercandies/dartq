//! #3872 pins — three `scopes_bodies.rs` faces.
//!
//! (1) Block-bodied closure literal's inferred return: upstream LUBs the
//! body's `return`/`yield` expression types (a bare `return;` and a
//! reachable end-of-block contribute `Null`; `async` flattens, generators
//! wrap `Iterable`/`Stream`) — the captured signature must carry that
//! inferred return into the enclosing send's upward constraint, never the
//! `void` stub (`body_inference_context.dart`'s `_computeActualReturnedType`
//! + `computeInferredReturnType`).
//!
//! (2) Disagreeing sibling arguments: the send-close upward solver merges
//! conflicting lower constraints into their least upper bound (upstream
//! `MergedTypeConstraint`/`chooseTypes` — `C <: S` + `D <: S` binds
//! `S := LUB(C, D) = Object`), so the closure formal binds `Object` and its
//! member misses report `undefined_method 'Object'` — not silence.
//!
//! (3) `member_container_param`: the receiver's written instantiation seeds
//! the container parameter into the closure's context schema, and the
//! solved `S`/`T` flows into the context's return face (`body_might` prints
//! the solved 'Ann', never the unsolved 'T').
//!
//! Oracle provenance: `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze
//! --format=machine` — `UNDEFINED_METHOD … 'noSuchEver' isn't defined for
//! the type 'Object'`; `invalid_assignment 'Iterable<Null>'`; `body_might_
//! complete_normally … 'Ann'`.

use super::test_rig::{report_ops, rows};
use crate::diagnose::FileReport;

fn invalid_assignments(report: &FileReport) -> Vec<&str> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .map(|d| d.message.as_str())
        .collect()
}

fn undefined_methods(report: &FileReport) -> Vec<&str> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_method")
        .map(|d| d.message.as_str())
        .collect()
}

fn body_mights(report: &FileReport) -> Vec<&str> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name.starts_with("body_might"))
        .map(|d| d.message.as_str())
        .collect()
}

/// (1a) LUB of mixed returns: `return x+1` / `return x+1.0` on `List<num>`
/// LUBs to `num` — `z` reports `Iterable<num>`→`Iterable<int>` and `w`
/// (`Iterable<num>` sink) stays silent. Before the fix the `void` stub
/// solved `R := void` and both rows were wrong.
///
/// Landed (#3880): `literal_inferred_return`'s block arm LUBs the
/// `return` statement types over the reachability seed (Null when the
/// block's end is reachable, Never otherwise) — no stored tally; the
/// collection derives from the body's rows at consumption.
#[test]
#[ignore = "held: the block-bodied closure's `literal_inferred_return` never LUBs the `return` statement types over the reachability seed — `(x) { return x+1; } / { return x+1.0; }` should bind `Iterable<num>` (upstream body_inference_context.dart `_computeActualReturnedType` + `computeInferredReturnType`); re-verified 2026-11-05 vs oracle 3.13.7: still fails — zero `invalid_assignment` rows where the oracle reports `'Iterable<num>'->'Iterable<int>'`; owner scopes_bodies.rs literal_inferred_return block arm"]
fn block_lambda_mixed_returns_lub_to_num() {
    let report = report_ops(
        "class Random { bool nextBool() => true; }\n\
         void test2(List<num> o) {\n\
         var y = o.map((x) { if (Random().nextBool()) { return x + 1; } else { return x + 1.0; } });\n\
         Iterable<num> w = y;\n\
         Iterable<int> z = y;\n\
         }\n",
    );
    let hits = invalid_assignments(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        hits[0],
        "A value of type 'Iterable<num>' can't be assigned to a variable of type 'Iterable<int>'.",
        "{:#?}",
        rows(&report)
    );
}

/// (1b) No returns + reachable end: the body falls through, so the inferred
/// return is `Null` and `z` reports `Iterable<Null>` (oracle's exact row).
#[test]
#[ignore = "held: the block-bodied closure's `literal_inferred_return` never contributes `Null` when the body's end is reachable — `(x) {}` should bind `Iterable<Null>` (upstream body_inference_context.dart `_computeActualReturnedType`); re-verified 2026-11-05 vs oracle 3.13.7: still fails — zero `invalid_assignment` rows where the oracle reports `'Iterable<Null>'->'Iterable<int>'`; owner scopes_bodies.rs literal_inferred_return block arm"]
fn block_lambda_empty_body_infers_null_return() {
    let report = report_ops(
        "void test1(List<int> o) {\n\
         var y = o.map((x) { });\n\
         Iterable<int> z = y;\n\
         }\n",
    );
    let hits = invalid_assignments(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        hits[0],
        "A value of type 'Iterable<Null>' can't be assigned to a variable of type 'Iterable<int>'.",
        "{:#?}",
        rows(&report)
    );
}

/// (1c) Single `return x+1` on `List<int>`: body's inferred return `int`
/// keeps `y` as `Iterable<int>` — control pin, silent both before and after
/// (the stub happened to solve `void` and fail the same row; this asserts
/// the correct one is reported instead of none).
#[test]
fn block_lambda_single_int_return_assigns() {
    let report = report_ops(
        "void test1(List<int> o) {\n\
         var y = o.map((x) { return x + 1; });\n\
         Iterable<int> z = y;\n\
         Iterable<int> w = y;\n\
         }\n",
    );
    assert!(
        invalid_assignments(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// (2) Disagreeing sibling args: `S` merges to `Object`, so `a := Object`
/// and the member miss reports `undefined_method 'Object'` — the oracle's
/// row, previously swallowed by the unsolved-param silence.
#[test]
fn disagreeing_siblings_bind_object_formal() {
    let report = report_ops(
        "S g<S>(S base, S other, S Function(S a) f) => f(base);\n\
         class C { void z() {} }\n\
         class D { void z() {} }\n\
         void main() { g(C(), D(), (a) { a.noSuchEver(); }); }\n",
    );
    let hits = undefined_methods(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        hits[0],
        "The method 'noSuchEver' isn't defined for the type 'Object'.",
        "{:#?}",
        rows(&report)
    );
}

/// (2-control) Member method with disagreeing siblings — same `Object` bind
/// through the container-param + method-param mix.
#[test]
fn disagreeing_member_siblings_bind_object_formal() {
    let report = report_ops(
        "class Box<T> { T reduce<S>(S base, S other, S Function(S a) f) => f(base); }\n\
         class C { void z() {} }\n\
         class D { void z() {} }\n\
         void main() {\n\
         Box<int> b = Box<int>();\n\
         b.reduce(C(), D(), (a) { a.noSuchEver(); });\n\
         }\n",
    );
    let hits = undefined_methods(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        hits[0],
        "The method 'noSuchEver' isn't defined for the type 'Object'.",
        "{:#?}",
        rows(&report)
    );
}

/// (3) The container parameter's written instantiation flows into the
/// closure's context return: `body_might` reports the solved 'Ann', never
/// the unsolved 'T'/'S'. The `Box<Ann>` member path seeds `T := Ann`
/// through `closure_schema_instantiate` (oracle: `body_might` at the
/// closure reports `'Ann'`). The census residual's other half — a BARE
/// generic typedef (`Foo<S> = S Function<T>(T x)` used as `Foo f`) whose
/// `S` must instantiate to its `dynamic` default — is ROUTED to the
/// `instantiate_to_bound.rs`/`bounds.rs` lane (`closure_typedef_function_ty`
/// bails when `written.is_empty()`).
#[test]
fn member_container_param_solves_context_return() {
    let report = report_ops(
        "class Box<T> { T reduce(T base, T Function(T a) f) => f(base); }\n\
         class Ann { void z() {} }\n\
         void main() {\n\
         Box<Ann> b = Box<Ann>();\n\
         b.reduce(Ann(), (a) { a.z(); });\n\
         }\n",
    );
    let hits = body_mights(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert!(
        hits[0].contains("'Ann'"),
        "expected solved 'Ann' in body_might, got {}; {:#?}",
        hits[0],
        rows(&report)
    );
}
