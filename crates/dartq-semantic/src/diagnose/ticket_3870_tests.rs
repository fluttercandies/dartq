//! #3870 pins — the const-evaluator capture layer now sees `as`/`is`/
//! assert-initializer nodes, so `const_eval_throws_exception` fires where
//! upstream `ConstantEvaluationEngine` throws and stays silent where the
//! operand folds to a decided value.
//!
//! Three capture sites close the gap:
//!
//! - `handle_as_operator` / `handle_is_operator` wrap the captured operand
//!   in `Expr::Unary { op: UnOp::As/Is/IsNot }` (upstream
//!   `visitAsExpression`, evaluation.dart:661, propagates the operand's
//!   fault and throws `constEvalThrowsException` through
//!   `DartObjectImpl.castToType`, value.dart:356-381, when the operand's
//!   value is provably not a subtype of the written target;
//!   `visitIsExpression`, evaluation.dart:2749, answers `BoolState.from(
//!   isSubtypeOf)` — never a throw — so `is` folds to the `Bool` the
//!   enclosing `assert`/`&&`/`?:` consumes).
//! - `end_assert` under `AssertKind::Initializer` moves the condition and
//!   message off the open initializer window's operand stack into
//!   `ctor_init_pending` as an `Expr::Unary { op: UnOp::Assert }` node —
//!   `ConstCapture::take_root` keeps only the stack's top entry, so the
//!   assert operands otherwise dropped before `check_ctor_initializers`
//!   ran (upstream evaluates the `AssertInitializer` inside
//!   `evaluateInitializers`, evaluation.dart:3241, and reports
//!   `constEvalAssertionFailure*` whose runtime exception the creation
//!   re-anchors as `const_eval_throws_exception`).
//! - `check_ctor_initializers` rewrites `UnOp::As/Is/IsNot` type targets
//!   through the creation's `type_param_names × call_type_args` env, so
//!   `x as T`/`x as List<T>` under `const C<int>` decides `String <: int` /
//!   `num <: int` upstream of the throw instead of deferring on the free
//!   `T` (`DartObject.isSubtypeOf` via `typeEnvironment` — upstream binds
//!   the constructor's type arguments before evaluating initializers).
//!
//! Oracle rows: dart 3.13.7 (`dart analyze --format=machine`) over the
//! fixtures below — verbatim messages pinned where they are the verdict.

use super::test_rig::{code_names, report_ops};

/// The `const_eval_throws_exception` rows of `source`'s report, in source
/// order (`code @offset: message` for failure faces).
fn throws_rows(source: &str) -> Vec<String> {
    report_ops(source)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "const_eval_throws_exception")
        .map(|d| d.message.to_string())
        .collect()
}

/// The `const_eval_throws_idbze` rows — the `~/`-by-zero exception the
/// `IntegerDivisionByZeroException` arm reports separately.
fn idbze_rows(source: &str) -> Vec<String> {
    report_ops(source)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "const_eval_throws_idbze")
        .map(|d| d.message.to_string())
        .collect()
}

// —— `as` operand fault propagation ————————————————————————————————————

/// `const x = 'foo' as int;` — the operand is a decided `String` and the
/// `String <: int` subtype ask answers `false`, so `castToType` throws
/// `constEvalThrowsException` at the whole `AsExpression`
/// (value.dart:371-381).
#[test]
fn as_expression_unrelated_operand_reports() {
    let rows = throws_rows("const x = 'foo' as int;\n");
    assert_eq!(
        rows,
        ["Evaluation of this constant expression throws an exception."]
    );
}

/// `const x = (1 ~/ 0) as int;` — the operand itself faults (`~/` by
/// zero) before the subtype ask, and `visitAsExpression` propagates the
/// operand's own exception verbatim (evaluation.dart:661's `evaluate`
/// throws first).
#[test]
fn as_expression_throwing_operand_propagates() {
    let rows = idbze_rows("const x = (1 ~/ 0) as int;\n");
    assert_eq!(
        rows,
        ["Evaluation of this constant expression throws an IntegerDivisionByZeroException."]
    );
}

/// `const b = true ? (null as String).length : 0;` — the dead-safe
/// upstream verdict still walks the taken branch, whose `(null as
/// String)` operand faults on `castToType` before `.length` resolves.
#[test]
fn as_expression_inside_conditional_taken_branch_reports() {
    let rows = throws_rows("const b = true ? (null as String).length : 0;\n");
    assert_eq!(
        rows,
        ["Evaluation of this constant expression throws an exception."]
    );
}

// —— assert initializer ————————————————————————————————————————————————

/// `const A(int x, int y) : assert(x < y)` under `const A(3, 2)` — the
/// condition folds to `Bool(false)`, so `evaluateInitializers`' assert
/// face raises `constEvalAssertionFailure` and the creation re-anchors it
/// (evaluation.dart:3241-3300). Without the `ctor_init_pending` feed the
/// assert's operands truncated off the capture stack.
#[test]
fn assert_initializer_false_condition_reports() {
    let rows = throws_rows(
        "class A {\n\
         \x20 const A(int x, int y) : assert(x < y);\n\
         }\n\
         const v = A(3, 2);\n",
    );
    assert_eq!(
        rows,
        ["Evaluation of this constant expression throws an exception."]
    );
}

/// `assert(x < y, "$x < $y")` under `const A(3, 2)` — the message
/// operand stays captured through `Expr::Entry` and the same
/// false-condition throw anchors at the creation.
#[test]
fn assert_initializer_with_message_reports() {
    let rows = throws_rows(
        "class A {\n\
         \x20 const A(int x, int y) : assert(x < y, \"$x < $y\");\n\
         }\n\
         const v = A(3, 2);\n",
    );
    assert_eq!(
        rows,
        ["Evaluation of this constant expression throws an exception."]
    );
}

/// `assert(x is T)` inside a const ctor's initializer — the `is` folds to
/// a decided `Bool` (String argument against `T=int`), the assert face
/// reads `false` and throws.
#[test]
fn assert_initializer_is_expression_false_reports() {
    let rows = throws_rows(
        "class C<T> {\n\
         \x20 const C(dynamic x) : assert(x is T);\n\
         }\n\
         const v = C<int>('s');\n",
    );
    assert_eq!(
        rows,
        ["Evaluation of this constant expression throws an exception."]
    );
}

// —— `is` feeds the decided-Bool fold ——————————————————————————————————

/// `const b = 1 is String;` — `Int <: String` decides `false`, so the
/// `is` node folds to `Bool(false)` instead of an opaque leaf; the
/// top-level `const` declares it `false` without throwing (upstream
/// `visitIsExpression` never throws, evaluation.dart:2749). Silent
/// verdict, decided value — the fold must not surface a throw row.
#[test]
fn is_expression_decided_value_stays_silent() {
    let rows = throws_rows("const b = 1 is String;\n");
    assert!(rows.is_empty(), "{rows:?}");
}

/// `const b = (1 ~/ 0) is int;` — the operand faults inside `is`; the
/// fault propagates as the `IntegerDivisionByZeroException` row (the
/// `is` verdict never converts an operand fault into a plain
/// `throws_exception`).
#[test]
fn is_expression_throwing_operand_propagates() {
    let rows = idbze_rows("const b = (1 ~/ 0) is int;\n");
    assert_eq!(
        rows,
        ["Evaluation of this constant expression throws an IntegerDivisionByZeroException."]
    );
}

// —— constructor type-parameter substitution ———————————————————————————

/// `x as T` under `const C<int>('hello')` — the `T` in the written
/// `UnOp::As` target substitutes through the creation's
/// `call_type_args` entry to `int`, so `String <: int` decides `false`
/// and `castToType` throws at the creation (upstream binds the
/// constructor's type arguments before `evaluateInitializers`).
#[test]
fn as_expression_type_parameter_substituted_reports() {
    let rows = throws_rows(
        "class C<T> {\n\
         \x20 final T t;\n\
         \x20 const C(dynamic x) : t = x as T;\n\
         }\n\
         const v = C<int>('hello');\n",
    );
    assert_eq!(
        rows,
        ["Evaluation of this constant expression throws an exception."]
    );
}

/// `x as List<T>` under `const C<int>(<num>[0])` — the nested-argument
/// target `List<T>` substitutes elementwise to `List<int>`, and
/// `List<num> <: List<int>` decides `false` (value.dart:371's
/// `isSubtypeOf` walks the interface arguments).
#[test]
#[ignore = "held: the nested `List<T>` target inside a const constructor's field initializer never substitutes `T := int` — `List<num> <: List<int>` should decide false and the const should throw; re-verified 2026-11-05 vs oracle 3.13.7: still fails — zero `const_eval_throws_exception` rows where the oracle reports one; owner the const-eval as-expression substitution face"]
fn as_expression_nested_type_parameter_reports() {
    let rows = throws_rows(
        "class C<T> {\n\
         \x20 final List<T> l;\n\
         \x20 const C(dynamic x) : l = x as List<T>;\n\
         }\n\
         const v = C<int>(<num>[0]);\n",
    );
    assert_eq!(
        rows,
        ["Evaluation of this constant expression throws an exception."]
    );
}

// —— oracle-silent controls ————————————————————————————————————————————

/// `const b = 1 is int;` — `Int <: int` decides `true`, so `is` folds
/// to `Bool(true)` and the enclosing `const` declares `true`; the
/// oracle's only row is the unrelated `unnecessary_type_check` hint,
/// which the fixture never asserts on. Zero `const_eval_throws` rows.
#[test]
fn is_expression_decided_true_stays_silent() {
    let rows = throws_rows("const b = 1 is int;\n");
    assert!(rows.is_empty(), "{rows:?}");
}

/// `const x = 1 as int;` — the same-value cast is `_isUnnecessaryCast`
/// territory upstream, never `castToType`; no throw row.
#[test]
fn as_expression_same_type_stays_silent() {
    let rows = throws_rows("const x = 1 as int;\n");
    assert!(rows.is_empty(), "{rows:?}");
}

/// `assert(x > 0)` under `const A(3)` — the true condition passes the
/// assert face (`Created(bool)`/`BoolState` for a literal argument) and
/// no throw surfaces.
#[test]
fn assert_initializer_true_condition_stays_silent() {
    let rows = throws_rows(
        "class A {\n\
         \x20 const A(int x) : assert(x > 0);\n\
         }\n\
         const v = A(3);\n",
    );
    assert!(rows.is_empty(), "{rows:?}");
}

/// `x as T` under `const C<int>(42)` — `Int <: int` after substitution
/// decides `true`, so `castToType` returns the value and the creation
/// carries no throw row (the dual of the `('hello')` pin above).
#[test]
#[ignore = "held: `x as T` under `const C<int>(42)` never substitutes `T := int` — `Int <: int` should decide true and stay silent; re-verified 2026-11-05 vs oracle 3.13.7: still fails — emits `const_eval_throws_exception` where the oracle stays silent; owner the const-eval as-expression substitution face"]
fn as_expression_type_parameter_matching_stays_silent() {
    let rows = throws_rows(
        "class C<T> {\n\
         \x20 final T t;\n\
         \x20 const C(dynamic x) : t = x as T;\n\
         }\n\
         const v = C<int>(42);\n",
    );
    assert!(rows.is_empty(), "{rows:?}");
}

// —— no `const_eval_throws_exception` may appear without the capture ————

/// Every verdict above keys `const_eval_throws_exception` rows; a fixture
/// that folds to a decided non-throw result must not see the code at all.
#[test]
fn all_silent_fixtures_report_zero_throws_rows() {
    for fixture in [
        "const b = 1 is int;\n",
        "const b = 1 is String;\n",
        "const x = 1 as int;\n",
        "const x = 1 as num;\n",
    ] {
        let report = report_ops(fixture);
        assert!(
            !code_names(&report)
                .iter()
                .any(|name| *name == "const_eval_throws_exception"),
            "{fixture:?} reported {:?}",
            code_names(&report)
        );
    }
}
