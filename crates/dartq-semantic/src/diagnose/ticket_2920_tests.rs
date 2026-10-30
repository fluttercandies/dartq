//! The #2920 pins: `return_of_invalid_type_from_closure` under
//! the inferred-context return the closure captures latch — the
//! residual census family #2912's ledger left behind.
//!
//! Four faces pinned, each keyed to the upstream mechanism it
//! mirrors:
//!
//! * **Old-style typedef** (`typedef T F<S, T>(S x)`): the
//!   declaration-initializer channel folds `F<int, String>`
//!   through the element model's `var_ty` — a `Ty::Function`
//!   whose `type_params_named` pairs the written arguments
//!   positionally. The `instantiate_alias` gate misses this
//!   spelling entirely (the old typedef carries a count, never
//!   names; resolution.rs:234).
//!
//! * **Constructor callee** (`Future<Null>(() => 1)`): the
//!   argument-slot channel reads the constructor's formal —
//!   `FutureOr<T> Function()` — and `closure_callee_params`
//!   now recovers the class's own `type_param_names` so the
//!   explicit `<Null>` fixes `T` (the `ordered` arm of
//!   `closure_schema_instantiate`). The oracle's context
//!   renders `FutureOr<Null>`.
//!
//! * **Async literal** (`void Function() f = () async {
//!   return v; }`): the oracle's `element.returnType` is the
//!   imposed `R` wrapped through `Future<flatten(R)>`
//!   (body_inference_context.dart:132-153). `note_async_modifier`
//!   rewrites the latched context's return so the verdict's
//!   destination renders `Future<void>`, not the raw `void`.
//!
//! * **Untyped formal under a typed context** (`(x) => 3`
//!   under `String Function(int)`): the literal's own formal
//!   binds `int` from the context (`_inferFormalParameters`),
//!   but the return verdict still reads `String` — the
//!   mismatch is `int`-returned vs `String`-required.
//!
//! Census rows:
//! `return_of_invalid_type_from_closure` 79 cases, 19 pass,
//! 60 fail (status.tsv:812). The 44 fail cases decompose
//! across these four faces plus a handful of already-passing
//! non-closure returns.

use super::test_rig::{report_ops, rows};

/// `return_of_invalid_type_from_closure` rows for `report`.
fn closure_rows(
    report: &crate::diagnose::types::FileReport,
) -> Vec<&dartq_diagnostics::Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "return_of_invalid_type_from_closure")
        .collect()
}

// ---------- Family A: old-style (function) typedef declaration
// `typedef T Function2<S, T>(S x);` — the legacy alias face.
// `Function2<int, String>` folds to `String Function(int)` and
// the body's returns answer it. Census case:
// `StrongModeLocalInferenceTest/test_functionLiteral_assignment_
// typedArguments` — expected `'Null' isn't returnable from a
// 'String' function` / `'int' isn't returnable from a 'String'
// function`.

/// Typed-formal arrow returning `null` — `(int x) => null`
/// under `Function2<int, String>`: the null body's `Null`
/// fails the `String` return.
#[test]
fn typedef_typed_formal_arrow_null() {
    let report = report_ops(
        "typedef T Function2<S, T>(S x);\n\
         \x20 void f() {\n\
         \x20   Function2<int, String> l0 = (int x) => null;\n\
         \x20   l0(0);\n\
         \x20 }",
    );
    let got = closure_rows(&report);
    assert_eq!(got.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        got[0].message,
        "The returned type 'Null' isn't returnable from a 'String' \
         function, as required by the closure's context."
    );
}

/// Typed-formal arrow returning `int` — `(int x) => 3` under
/// `Function2<int, String>`: the `int` body fails `String`.
#[test]
fn typedef_typed_formal_arrow_int() {
    let report = report_ops(
        "typedef T Function2<S, T>(S x);\n\
         \x20 void f() {\n\
         \x20   Function2<int, String> l3 = (int x) => 3;\n\
         \x20   l3(0);\n\
         \x20 }",
    );
    let got = closure_rows(&report);
    assert_eq!(got.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        got[0].message,
        "The returned type 'int' isn't returnable from a 'String' \
         function, as required by the closure's context."
    );
}

/// Typed-formal block body — `(int x) { return 3; }` under
/// `Function2<int, String>`: the `return 3` statement reports
/// the same `int` vs `String` mismatch the arrow body reads.
#[test]
fn typedef_typed_formal_block_return() {
    let report = report_ops(
        "typedef T Function2<S, T>(S x);\n\
         \x20 void f() {\n\
         \x20   Function2<int, String> l4 = (int x) { return 3; };\n\
         \x20   l4(0);\n\
         \x20 }",
    );
    let got = closure_rows(&report);
    assert_eq!(got.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        got[0].message,
        "The returned type 'int' isn't returnable from a 'String' \
         function, as required by the closure's context."
    );
}

/// Untyped-formal arrow — `(x) => null` under
/// `Function2<int, String>`: the literal's own `x` binds `int`
/// from the context (the `_inferFormalParameters` face), and
/// the body's `Null` still fails `String`.
#[test]
fn typedef_untyped_formal_arrow_null() {
    let report = report_ops(
        "typedef T Function2<S, T>(S x);\n\
         \x20 void f() {\n\
         \x20   Function2<int, String> l0 = (x) => null;\n\
         \x20   l0(0);\n\
         \x20 }",
    );
    let got = closure_rows(&report);
    assert_eq!(got.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        got[0].message,
        "The returned type 'Null' isn't returnable from a 'String' \
         function, as required by the closure's context."
    );
}

// ---------- Family B: FutureOr<F> context return via Future
// constructor — `Future<Null>(() => 1)`. The constructor
// formal is `FutureOr<T> Function()`; the written `Null`
// fixes `T`, and the body's `1` fails `FutureOr<Null>`.
// Census case: `future_flattening_A01_t78` — expected
// `'int' isn't returnable from a 'FutureOr<Null>' function`.

/// `Future<Null>(() => 1)` — `int` vs `FutureOr<Null>`.
#[test]
fn future_null_closure_returns_int() {
    let report = report_ops(
        "void f() {\n\
         \x20   Future<Null>(() => 1);\n\
         \x20 }",
    );
    let got = closure_rows(&report);
    assert_eq!(got.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        got[0].message,
        "The returned type 'int' isn't returnable from a \
         'FutureOr<Null>' function, as required by the closure's \
         context."
    );
}

/// `Future<Future<Null>>(() => 1)` — the nested `Future`
// formal reads `FutureOr<Future<Null>>`; `int` fails it.
// Census case: `future_flattening_A01_t78` second row —
// `'int' isn't returnable from a 'FutureOr<Future<Null>>'
// function`.
#[test]
fn future_future_null_closure_returns_int() {
    let report = report_ops(
        "void f() {\n\
         \x20   Future<Future<Null>>(() => 1);\n\
         \x20 }",
    );
    let got = closure_rows(&report);
    assert_eq!(got.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        got[0].message,
        "The returned type 'int' isn't returnable from a \
         'FutureOr<Future<Null>>' function, as required by the \
         closure's context."
    );
}

/// `Future<Never>(() => 1)` — `int` vs `FutureOr<Never>`.
/// Census case: `future_flattening_A01_t82` — `'int' isn't
/// returnable from a 'FutureOr<Never>' function`.
#[test]
fn future_never_closure_returns_int() {
    let report = report_ops(
        "void f() {\n\
         \x20   Future<Never>(() => 1);\n\
         \x20 }",
    );
    let got = closure_rows(&report);
    assert_eq!(got.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        got[0].message,
        "The returned type 'int' isn't returnable from a \
         'FutureOr<Never>' function, as required by the closure's \
         context."
    );
}

/// `Future<Null>(() => null)` — `Null` vs `FutureOr<Null>`:
/// `Null` is returnable from `FutureOr<Null>` (the `Null` arm
/// admits), so this case is clean.
#[test]
fn future_null_closure_returns_null_clean() {
    let report = report_ops(
        "void f() {\n\
         \x20   Future<Null>(() => null);\n\
         \x20 }",
    );
    let got = closure_rows(&report);
    assert_eq!(got.len(), 0, "{:#?}", rows(&report));
}

// ---------- Family C: async closure body returns a value
// `void Function() f = () async { return v; }` — the context
// return is `void`; the async literal's context reads
// `Future<void>` (the oracle's `Future<flatten(void)>`) and a
// value return reports under the closure message. Census case:
// `invalid_returns/async_invalid_return_05_test` — expected
// `'int' isn't returnable from a 'Future<void>' function`.

/// `void Function() test2 = () async { return v; }` — `int`
/// vs `Future<void>`.
#[test]
fn async_void_closure_returns_int() {
    let report = report_ops(
        "int v = 0;\n\
         \x20 void Function() test2 = () async {\n\
         \x20   return v;\n\
         \x20 };\n\
         \x20 void f() { test2(); }",
    );
    let got = closure_rows(&report);
    assert_eq!(got.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        got[0].message,
        "The returned type 'int' isn't returnable from a \
         'Future<void>' function, as required by the closure's \
         context."
    );
}

/// `Future<void> Function() test2 = () async { return v; }`
/// — same shape under an explicit `Future<void>` context:
/// `flatten(Future<void>) = void` → `Future<void>` — the
/// same `Future<void>` destination, and `int` still fails.
#[test]
fn async_future_void_closure_returns_int() {
    let report = report_ops(
        "int v = 0;\n\
         \x20 Future<void> Function() test2 = () async {\n\
         \x20   return v;\n\
         \x20 };\n\
         \x20 void f() { test2(); }",
    );
    let got = closure_rows(&report);
    assert_eq!(got.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        got[0].message,
        "The returned type 'int' isn't returnable from a \
         'Future<void>' function, as required by the closure's \
         context."
    );
}

/// `Future<void> Function() test2 = () async { return null;
/// }` — `Null` IS returnable from `Future<void>` (the `Null`
/// arm admits), so this case is clean.
#[test]
fn async_future_void_closure_returns_null_clean() {
    let report = report_ops(
        "void f() {\n\
         \x20   Future<void> Function() test2 = () async {\n\
         \x20     return null;\n\
         \x20   };\n\
         \x20   test2();\n\
         \x20 }",
    );
    let got = closure_rows(&report);
    assert_eq!(got.len(), 0, "{:#?}", rows(&report));
}

// ---------- Family D: untyped formal under a typed context —
// `(x) => 3` under `B Function(A)` reads `int` vs `B` (the
// body's return type vs the context's declared return). The
// formal's `A` binds through `_inferFormalParameters`, but the
// return still answers `B`. Census case:
// `implicit_downcast_during/function_literal_arrow_test` —
// `'A' isn't returnable from a 'B' function`.

/// `(A a) => a` under `B Function(A)` — the body's `a` is
/// `A`, which misses `B`.
#[test]
fn supertype_formal_returns_wrong_type() {
    let report = report_ops(
        "class A {}\n\
         \x20 class B extends A {}\n\
         \x20 void f() {\n\
         \x20   B Function(A) ff = (A a) => a;\n\
         \x20   ff(A());\n\
         \x20 }",
    );
    let got = closure_rows(&report);
    assert_eq!(got.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        got[0].message,
        "The returned type 'A' isn't returnable from a 'B' \
         function, as required by the closure's context."
    );
}
