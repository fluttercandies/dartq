//! The #3596 pins — an async closure's return check reads the body's OWN
//! inferred return type, never the imposed context return verbatim.
//!
//! # The observed divergence
//!
//! Row-level census `return_of_invalid_type_from_closure`: 19 fabricated
//! triples (dartq reports, oracle 3.13.7 stays silent). Every fabricated
//! row whose destination is written `Future<…>` is an `async` literal
//! whose context return type is not a `Future`:
//!
//! - `Object Function() f = () async { return v; };` (a `void` return
//!   expression) — `async_invalid_return_23_test`,
//!   `ReturnOfInvalidTypeTest/test_functionExpression_async_futureOr_void__to_Object`,
//!   `weak_reference_target_t05`;
//! - `int Function()`, `List<int> Function()` and the `T`-headed
//!   argument formals — `future_flattening_A01_t57…t80`;
//! - `Future<Object?> Function()` / `FutureOr<Object?> Function()` with a
//!   `void` return — `async_invalid_return_32…37_test`,
//!   `test_functionExpression_async_void__to_FutureOr_ObjectQ`.
//!
//! # The contract
//!
//! Upstream `element.returnType` for a function literal is the body's
//! INFERRED return type, not the imposed context face: upstream Dart SDK
//! source, `pkg/analyzer/lib/src/dart/resolver/body_inference_context.dart`
//! — `computeInferredReturnType` wraps `futureType(flatten(clamp(T, K)))`
//! around the actual returned type `T`, `_contextTypeForImposed` supplies
//! the async body's context `K = FutureOr<futureValueType(imposed)>`
//! (:259-261), and `_clampToContextType` (:206-222) answers `void` first,
//! then `T` when `T <: K`, and only then `K` itself. The return verifier
//! then asks each `return e;` against that inferred type
//! (`return_type_verifier.dart` `_checkReturnExpression`).
//!
//! Two arms of the clamp are independent of the body and therefore
//! decidable at the literal's own `async` keyword:
//!
//! - `futureValueType(R)` is `void` → the clamp short-circuits to `void`,
//!   the destination is `Future<void>`;
//! - `futureValueType(R)` is `Object?` (R names no `Future`/`FutureOr`
//!   head, or names one whose argument is `Object?`) → every flattened
//!   return type is a subtype of `FutureOr<Object?>`, the clamp answers
//!   the body's own type, and the per-return ask degenerates to `S <: S`
//!   — silent.
//!
//! A concrete `Future<X>`/`FutureOr<X>` context keeps `Future<X>`: there
//! the clamp either repeats `X` or answers the body's own type, and the
//! `X` ask agrees on both.
//!
//! Oracle provenance: `dart analyze` 3.13.7, machine format, on the
//! fixtures below. Controls keep reporting genuine invalid returns.

use super::test_rig::{code_names, report_ops, rows};

/// The `return_of_invalid_type_from_closure` rows of `report`.
fn closure_rows(report: &crate::diagnose::types::FileReport) -> Vec<&dartq_diagnostics::Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "return_of_invalid_type_from_closure")
        .collect()
}

// ---------- Cluster A: a non-`Future` context return absorbs every
// returned type — the clamp falls through to the body's own type.

/// Pin: `Object Function()` over an `async` literal returning a `void`
/// expression is clean upstream (`async_invalid_return_23_test`'s
/// `test3`, `test_functionExpression_async_futureOr_void__to_Object`).
#[test]
fn async_closure_object_context_void_return_is_silent() {
    let report = report_ops(
        "void v = null;\n\
         void f() {\n\
         \x20 Object Function() a = () async {\n\
         \x20   return v;\n\
         \x20 };\n\
         \x20 a();\n\
         }",
    );
    assert!(
        closure_rows(&report).is_empty(),
        "expected no closure-return row, got {:#?}",
        rows(&report)
    );
}

/// Pin: the same literal under a type-parameter-headed context — the
/// `future_flattening_A01_t5x/t7x` family's `FutureOr<R>` / `R` argument
/// slots. `futureValueType(R)` is `Object?`, so the clamp answers the
/// body's own type and no return row fires.
#[test]
fn async_closure_type_parameter_context_void_return_is_silent() {
    let report = report_ops(
        "void v = null;\n\
         T apply<T>(T Function() body) => body();\n\
         void use<R>(R Function() f) {\n\
         \x20 apply<R>(() async {\n\
         \x20   return v;\n\
         \x20 });\n\
         \x20 f;\n\
         }",
    );
    assert!(
        closure_rows(&report).is_empty(),
        "expected no closure-return row, got {:#?}",
        rows(&report)
    );
}

/// Pin: a concrete non-`Future` head (`int`) — `futureValueType(int)` is
/// `Object?` too, so the literal infers `Future<void> Function()` and the
/// only divergence left upstream is the enclosing `invalid_assignment`.
#[test]
fn async_closure_int_context_void_return_is_silent() {
    let report = report_ops(
        "void v = null;\n\
         void use() {\n\
         \x20 int Function() a = () async {\n\
         \x20   return v;\n\
         \x20 };\n\
         \x20 a;\n\
         }",
    );
    assert!(
        closure_rows(&report).is_empty(),
        "expected no closure-return row, got {:#?}",
        rows(&report)
    );
}

// ---------- Cluster B: `Future<Object?>` / `FutureOr<Object?>`
// contexts — `futureValueType` is `Object?`, the clamp answers the
// body's own type again.

/// Pin: `FutureOr<Object?> Function()` with a `void` return — upstream
/// silent (`test_functionExpression_async_void__to_FutureOr_ObjectQ`).
#[test]
fn async_closure_future_or_object_question_void_return_is_silent() {
    let report = report_ops(
        "void v = null;\n\
         void f() {\n\
         \x20 FutureOr<Object?> Function() a = () async {\n\
         \x20   return v;\n\
         \x20 };\n\
         \x20 a();\n\
         }",
    );
    assert!(
        closure_rows(&report).is_empty(),
        "expected no closure-return row, got {:#?}",
        rows(&report)
    );
}

/// Pin: `Future<Object?> Function()` with a `void` return — the
/// `async_invalid_return_32`/`35` shape, silent upstream.
#[test]
fn async_closure_future_object_question_void_return_is_silent() {
    let report = report_ops(
        "void v = null;\n\
         void f() {\n\
         \x20 Future<Object?> Function() a = () async {\n\
         \x20   return v;\n\
         \x20 };\n\
         \x20 a();\n\
         }",
    );
    assert!(
        closure_rows(&report).is_empty(),
        "expected no closure-return row, got {:#?}",
        rows(&report)
    );
}

// ---------- Controls: the destinations that DO carry the context.

/// Control: `Future<int> Function()` with a `void` return still reports —
/// `futureValueType(Future<int>)` is `int`, the clamp cannot absorb
/// `void`, and the oracle reports `'void'` against `'Future<int>'`
/// (`async_invalid_return_30` family).
#[test]
fn async_closure_future_int_context_void_return_reports() {
    let report = report_ops(
        "void v = null;\n\
         void f() {\n\
         \x20 Future<int> Function() a = () async {\n\
         \x20   return v;\n\
         \x20 };\n\
         \x20 a();\n\
         }",
    );
    let got = closure_rows(&report);
    assert_eq!(got.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        got[0].message,
        "The returned type 'void' isn't returnable from a 'Future<int>' \
         function, as required by the closure's context."
    );
}

/// Control: `Future<Object> Function()` with a `void` return reports —
/// `Object` is not `Object?`, so the clamp keeps the context face and the
/// return fails it (`async_invalid_return_23_test`'s declared sibling).
#[test]
fn async_closure_future_object_context_void_return_reports() {
    let report = report_ops(
        "void v = null;\n\
         void f() {\n\
         \x20 Future<Object> Function() a = () async {\n\
         \x20   return v;\n\
         \x20 };\n\
         \x20 a();\n\
         }",
    );
    let got = closure_rows(&report);
    assert_eq!(got.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        got[0].message,
        "The returned type 'void' isn't returnable from a 'Future<Object>' \
         function, as required by the closure's context."
    );
}

/// Control: the `void`-headed context keeps the `Future<void>` face and
/// still rejects a non-`void` return (`void Function() f = () async {
/// return 1; }` reports `'int'` against `'Future<void>'` upstream).
#[test]
fn async_closure_void_context_non_void_return_reports() {
    let report = report_ops(
        "void f() {\n\
         \x20 void Function() a = () async {\n\
         \x20   return 1;\n\
         \x20 };\n\
         \x20 a();\n\
         }",
    );
    let got = closure_rows(&report);
    assert_eq!(got.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        got[0].message,
        "The returned type 'int' isn't returnable from a 'Future<void>' \
         function, as required by the closure's context."
    );
}

/// Control: the sync literal keeps the raw context face — the clamp's
/// `K` for a non-`async` body IS the imposed type, so `Object Function()
/// { return v; }` reports `'void'` against `'Object'` upstream.
#[test]
fn sync_closure_object_context_void_return_reports() {
    let report = report_ops(
        "void v = null;\n\
         void f() {\n\
         \x20 Object Function() a = () {\n\
         \x20   return v;\n\
         \x20 };\n\
         \x20 a();\n\
         }",
    );
    let got = closure_rows(&report);
    assert_eq!(got.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        got[0].message,
        "The returned type 'void' isn't returnable from a 'Object' \
         function, as required by the closure's context."
    );
}

/// Control: the bare `return;` under an absorbing context stays silent
/// (upstream `_checkReturnWithoutValue` reads the inferred type, whose
/// `Null` contribution keeps `FutureOr<Object?>`'s value type silent).
#[test]
fn async_closure_object_context_bare_return_is_silent() {
    let report = report_ops(
        "void f() {\n\
         \x20 Object Function() a = () async {\n\
         \x20   return;\n\
         \x20 };\n\
         \x20 a();\n\
         }",
    );
    assert!(
        !code_names(&report).contains(&"return_without_value"),
        "expected no return_without_value row, got {:#?}",
        rows(&report)
    );
}
