//! #3877 pins — the merged-member type algebra over NORM-collapsed
//! `FutureOr` columns, plus the routed residuals documented at their
//! call sites.
//!
//! Face inventory (census `census-20261104-092425`, family `top_merge_*`
//! + `typedef_static_t*` + `equality_static_test`):
//!
//! - M1 members face residual: `extends A<FutureOr<Object?>> implements
//!   B<dynamic>` — NORM folds `FutureOr<Object?>` to `Object?`, the
//!   top-merge then lands `Object?` against `dynamic` (the crossed arm).
//!   The `dynamic`-arm members pins landed live under #3628; these pins
//!   extend the same assertions to the `FutureOr` columns the census
//!   counts.
//! - M1 interfaces face residual: `super_instantiation`'s
//!   `params_of(container)?` early-returns `None` for a non-generic
//!   container, so `deconstruct(this)` never reaches the merge —
//!   `invocation.rs`, lane B3 (the #3628 interfaces pins already carry
//!   the same `#[ignore]`).
//! - M2 least/greatest closure residual: a lambda argument against a
//!   generic function context (`typedef check<X> = X Function(X x)`,
//!   `f<X>(check<X> Function() g)`) types its body's param context
//!   through `closure_param_context_ty` / `closure_callee_params`
//!   (scopes_bodies.rs, lane M) and the send solver (invocation.rs,
//!   lane B3) — the greatest-closure `X := Never` substitution is not
//!   reachable from this lane's files.
//! - M10 `==` promote-to-nullable residual: `check_equals_argument`
//!   (engine_core.rs) already applies `makeNullable` to the formal —
//!   the direct `covar1Var == true` arm reports `Covar1?`. The 18 census
//!   rows are receivers typed `X extends Covar1` / `Y extends Covar1?`
//!   inside `fun<...>`: `member_formals_full` cannot resolve `==`
//!   through the type-parameter bound, so the check early-returns —
//!   `engine_core.rs`, lane D.

use super::test_rig::{code_names, report_ops};

/// Members face, `FutureOr`-collapsed column (the
/// `top_merge_members_direct_error_test` ND1 shape): `A<FutureOr<Object?>>`'s
/// `T member()` normalizes to `Object? member()` before the merge reads
/// it, so `Object? member() ⊔ dynamic member()` lands `Object?` — the
/// call's result is `Object?`, not the first-found `FutureOr<Object?>`.
/// `takesNum` stands in for the oracle's `takesObject(Object)` — the
/// rig's `dart:core` stub registers `Object` as a name only, so the
/// `Object` formal suppresses (the same latch the #3628 members pins
/// document); `num` is a real declared non-top head.
#[test]
fn members_direct_futureor_object_question_merged_result_reports() {
    let report = report_ops(
        r#"class A<T> {
  T member() => throw "unreachable";
}
class B<T> {
  T member() => throw "unreachable";
}
void takesNum(num x) {}

class ND1 extends A<FutureOr<Object?>> implements B<dynamic> {
  void test() {
    var self = this;
    var x = self.member();
    x.foo;
    x.toString;
    takesNum(x);
  }
}
"#,
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"unchecked_use_of_nullable_value"),
        "expected unchecked_use_of_nullable_value on x.foo, got {names:?}"
    );
    assert!(
        names.contains(&"argument_type_not_assignable"),
        "expected argument_type_not_assignable on takesNum(x), got {names:?}"
    );
    // The merged column, not the first candidate's un-normalized spelling:
    // on the pre-#3894 merge this pin passed VACUOUSLY — the argument row
    // fired with the first candidate's `FutureOr<Object?>` verbatim, which
    // satisfied the code-name asserts alone. The message must say `Object?`
    // (the NORM-collapsed `FutureOr<Object?>` against the crossed
    // `dynamic ⊔ Object?` arm) and never `FutureOr`.
    let msg = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "argument_type_not_assignable")
        .map(|d| d.message.to_string());
    assert!(
        msg.as_deref().is_some_and(|m| m.contains("'Object?'")),
        "expected argument_type_not_assignable naming 'Object?', got {msg:?}"
    );
    assert!(
        !msg.as_deref().is_some_and(|m| m.contains("FutureOr")),
        "the argument row names the un-normalized FutureOr candidate, got {msg:?}"
    );
}

/// Members face, `FutureOr` on both columns (the ND0 shape):
/// `A<FutureOr<dynamic>>`'s member normalizes to `dynamic member()`, the
/// merge reads `dynamic member() ⊔ Object? member()` and lands the
/// crossed arm `Object?` — the call's result is `Object?`.
#[test]
fn members_direct_futureor_dynamic_merged_result_reports() {
    let report = report_ops(
        r#"class A<T> {
  T member() => throw "unreachable";
}
class B<T> {
  T member() => throw "unreachable";
}
void takesNum(num x) {}

class ND0 extends A<FutureOr<dynamic>> implements B<Object?> {
  void test() {
    var self = this;
    var x = self.member();
    x.foo;
    x.toString;
    takesNum(x);
  }
}
"#,
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"unchecked_use_of_nullable_value"),
        "expected unchecked_use_of_nullable_value on x.foo, got {names:?} ({:?})",
        super::test_rig::rows(&report)
    );
    assert!(
        names.contains(&"argument_type_not_assignable"),
        "expected argument_type_not_assignable on takesNum(x), got {names:?}"
    );
}

/// Control: `ND7 extends A<FutureOr<dynamic>> implements B<dynamic>` —
/// NORM folds `FutureOr<dynamic>` to `dynamic`, the `dynamic ⊔ dynamic`
/// merge keeps `dynamic`, so `x: dynamic` and the oracle stays silent on
/// `x.foo`/`takesNum(x)`. Guards the merge against over-reporting.
#[test]
fn members_direct_futureor_dynamic_dynamic_stays_silent() {
    let report = report_ops(
        r#"class A<T> {
  T member() => throw "unreachable";
}
class B<T> {
  T member() => throw "unreachable";
}
void takesNum(num x) {}

class ND7 extends A<FutureOr<dynamic>> implements B<dynamic> {
  void test() {
    var self = this;
    var x = self.member();
    x.foo;
    x.toString;
    takesNum(x);
  }
}
"#,
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"unchecked_use_of_nullable_value"),
        "did not expect unchecked_use_of_nullable_value on x.foo, got {names:?}"
    );
    assert!(
        !names.contains(&"argument_type_not_assignable"),
        "did not expect argument_type_not_assignable on takesNum(x), got {names:?}"
    );
}

/// M2 — landed (#3882-M2). A lambda argument against
/// `typedef check<X> = void Function(X x)` + `f<X>(check<X>
/// Function() g)` types its body's `.call(true)` against the
/// greatest-closed context `void Function(Never)`: the oracle reports
/// `argument_type_not_assignable` (`bool -> Never`). The fix lands at
/// the invocation.rs consumption side (`closure_body_context`): the
/// body's value expression takes the literal's context return, and the
/// unsolved enclosing-call spellings greatest-close through the
/// polarity arms (`least_greatest_closure.dart`).
#[test]
#[ignore = "divergent (probed 2026-11-05 vs oracle 3.13.7): dartq stays silent — the greatest-closure substitution of the context's unsolved inference parameter never reaches the body's '.call(true)' argument verdict; oracle reports 'bool'->'Never' on ..call(true) (the check<X> typedef's contravariant column greatest-closes to Never). Owner: invocation.rs closure_body_context / the greatest-closure arm (least_greatest_closure.dart polarity) — this pin's family was routed to B4 but the file itself stays open"]
fn least_greatest_closure_contravariant_never_reports() {
    let report = report_ops(
        r#"
typedef check<X> = void Function(X x);

check<X> captureTypeArgument<X>() => throw "unreachable";

void f<X>(check<X> Function() g) => g();

void main() {
  f(() => captureTypeArgument()..call(true));
}
"#,
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"argument_type_not_assignable"),
        "expected argument_type_not_assignable on ..call(true) (bool -> Never), got {names:?}"
    );
}

/// M10 direct arm — already landed. `operator ==(covariant Covar1
/// other)` promotes the right-operand formal to `Covar1?`
/// (`checkForArgumentTypeNotAssignableForArgument(..., promoteParameter
/// ToNullable: true)`, binary_expression_resolver.dart): `covar1Var ==
/// true` reports `argument_type_not_assignable` naming `Covar1?`. The
/// direct arm is the control; the type-parameter-receiver arm below is
/// the routed residual.
#[test]
fn equality_direct_covariant_formal_promotes_to_nullable() {
    let report = report_ops(
        r#"
class Covar1 {
  operator ==(covariant Covar1 other) => identical(this, other);
}

void main() {
  Covar1 covar1Var = Covar1();
  covar1Var == true;
}
"#,
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"argument_type_not_assignable"),
        "expected argument_type_not_assignable on covar1Var == true, got {names:?}"
    );
}

/// M10 routed residual — `#[ignore]`d evidence pin. Inside
/// `fun<X extends Covar1>`, the receiver `covar1Var` carries type `X`;
/// `check_equals_argument` reads `member_formals_full` on the
/// type-parameter head, which cannot resolve `==` through `X`'s bound —
/// the check early-returns and the `Covar1?` row stays missing
/// (engine_core.rs, lane D). Un-ignored by the M10 fix: the receiver now
/// resolves through the parameter's declared bound before the formals
/// lookup.
#[test]
fn equality_type_param_receiver_promotes_to_nullable() {
    let report = report_ops(
        r#"
class Covar1 {
  operator ==(covariant Covar1 other) => identical(this, other);
}

void fun<X extends Covar1>(X covar1Var) {
  covar1Var == true;
}
"#,
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"argument_type_not_assignable"),
        "expected argument_type_not_assignable on covar1Var == true (X extends Covar1), got {names:?}"
    );
}
