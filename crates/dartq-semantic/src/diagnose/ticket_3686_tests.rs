//! #3686 — bounded quantification: generic-function subtyping must relate
//! the signatures' own type parameters (counts + bound equality), which
//! decides three census-12 shapes at once:
//!
//! | shape | oracle verdict | pair |
//! |---|---|---|
//! | 1.2 | silent | `T Function<T>(T)` against `Foo<int>` (`typedef Foo<S> = S Function<T>(T x)`) |
//! | 3.3 | report | `F = void Function<T extends A>()` against bound `void Function<U extends B>()` |
//! | 2.8 | report + display | generic tear-off conformance; display keeps the instantiated face |
//!
//! Oracle ground truth (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/dart/element/type_system.dart`
//! `relateTypeParameters`, run by `_isFunctionSubtypeOf` ahead of the
//! return/formal checks): equal parameter counts, positionally unified
//! parameters, each written bound `isEqualTo` the other side's.

use super::test_rig::{assert_silent, report_ops, rows};

/// 1.2 — an α-equivalent pair through a parameterized alias: the oracle
/// reports nothing (`NonErrorResolverTest/test_genericTypeAlias_typeParameters#0`).
#[test]
fn generic_fn_alias_instantiated_pair_is_silent() {
    let report = report_ops(
        "typedef Foo<S> = S Function<T>(T x);\n\
         int foo<T>(T x) => 3;\n\
         void test1() {\n\
           Foo<int> y = foo;\n\
           y<String>('hello');\n\
         }\n",
    );
    assert_silent(&report);
}

/// 1.2 control — genuinely mismatched generic targets still report:
/// `String Function<T>(String)` into `String Function<S>(int)` is a real
/// `invalid_assignment` (the oracle's
/// `InferredTypeTest/test_downwardsInferenceOnGenericFunctionExpressions#0`
/// pair).
#[test]
fn generic_fn_mismatched_formal_reports() {
    let report = report_ops(
        "String foo<T>(T x) => 'x';\n\
         void test1() {\n\
           String Function<S>(int) y = foo;\n\
           print(y);\n\
         }\n",
    );
    let names: Vec<String> = rows(&report);
    assert!(
        names.iter().any(|r| r.contains("invalid_assignment")),
        "expected invalid_assignment, got {names:?}"
    );
}

/// 3.3 — a generic-function-typed value must match the target's
/// quantifier bounds (`relateTypeParameters`' bound-equality rule: each
/// written bound `isEqualTo` the other side's after positional
/// unification). A `void Function<T extends A>()` value into a
/// `void Function<U extends B>()` slot reports `invalid_assignment` —
/// the same `relateTypeParameters` verdict the `f<F>()` invocation face
/// asks (`TypeArgumentNotMatchingBoundsTest/
/// test_methodInvocation_genericFunctionTypeArgument_mismatch#0`,
/// whose call-site bound capture is a separate lane's face).
#[test]
fn generic_fn_bound_mismatch_reports() {
    let report = report_ops(
        "class A {}\n\
         class B {}\n\
         void test(void Function<T extends A>() f) {\n\
           void Function<U extends B>() x = f;\n\
           x();\n\
         }\n",
    );
    let names: Vec<String> = rows(&report);
    assert!(
        names.iter().any(|r| r.contains("invalid_assignment")),
        "expected invalid_assignment, got {names:?}"
    );
}

/// 3.3 control — matching quantifier bounds stay silent.
#[test]
fn generic_fn_bound_match_is_silent() {
    let report = report_ops(
        "class A {}\n\
         void test(void Function<T extends A>() f) {\n\
           void Function<U extends A>() x = f;\n\
           x();\n\
         }\n",
    );
    assert_silent(&report);
}

/// 2.8 — a generic tear-off against a generic context type: the oracle
/// accepts `T Function<T>(T)` into `U Function<U>(U)` (α-equivalent,
/// `InvalidAssignmentTest/
/// test_functionInstantiation_topLevelVariable_genericContext_assignable#0`).
#[test]
fn generic_tearoff_alpha_equivalent_is_silent() {
    let report = report_ops(
        "T id<T>(T t) => t;\n\
         void test() {\n\
           U Function<U>(U) f = id;\n\
           f<int>(3);\n\
         }\n",
    );
    assert_silent(&report);
}

/// 2.8 — arity-mismatched generic targets still report (the oracle's
/// `test_functionInstantiation_topLevelVariable_genericContext_nonAssignable#0`).
#[test]
fn generic_tearoff_arity_mismatch_reports() {
    let report = report_ops(
        "T id<T>(T t) => t;\n\
         void test() {\n\
           U Function<U>(U, int) f = id;\n\
           f<int>(3);\n\
         }\n",
    );
    let names: Vec<String> = rows(&report);
    assert!(
        names.iter().any(|r| r.contains("invalid_assignment")),
        "expected invalid_assignment, got {names:?}"
    );
}

/// 1.2 — the const-eval instantiated-function-type face: a generic
/// function's tear-off assigned to a matching generic alias stays silent
/// (`ConstantVisitorTest/
/// test_visitSimpleIdentifier_instantiatedFunctionType_variable#0`).
#[test]
fn generic_fn_extra_named_formal_is_silent() {
    let report = report_ops(
        "void f<T>(T a, {T? b}) {}\n\
         const void Function<T>(T a) g = f;\n\
         const void Function(int a) h = g;\n",
    );
    assert_silent(&report);
}
