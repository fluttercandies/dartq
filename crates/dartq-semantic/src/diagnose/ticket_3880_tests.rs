//! #3880/#3882 pins — the Lane B4 packet over `invocation.rs`.
//!
//! - Item 1 (#3882-M1): `super_instantiation`'s identity instantiation
//!   no longer early-returns on a NON-generic container — the evidence
//!   pins live in `ticket_3628_tests.rs` (un-ignored with the fix); this
//!   file adds no new shape beyond them.
//! - Item 2 (#3882-M2): the closure-literal body's value expression
//!   infers under the literal's matched context return, greatest-closed
//!   over the enclosing call's unsolved inference parameters — the
//!   `typedef_static_t16/17/22` family's `bool -> Never` rows. The t16
//!   evidence pin lives in `ticket_3877_tests.rs` (un-ignored); the
//!   pins here cover the covariant column (t22), the concrete-formal
//!   control, and the chain-head control.
//! - Item 3 (#3880): the block-bodied literal's inferred return LUBs
//!   its `return` statement types over the reachability seed — the
//!   evidence pins live in `ticket_3872_tests.rs` (un-ignored).
//! - #3879 M3 (#3884 chain): the creation's downward context seeds the
//!   SUPERCLASS SWAP — `A<int, String> a = B(3, "hello")` over
//!   `B<S, T> extends A<T, S>` binds through the swapped clause, so the
//!   constructor's formals check against the substituted slots.
//! - #3879 M4 (#3884 chain): a FAILED creation inference falls back to
//!   instantiate-to-bounds and still checks the arguments
//!   (`const C.subtyped(a)` reports `A<int>` against `B<dynamic>`).
//! - #3879 E8 (#3884 chain): an enum constant's `super.x` formal that
//!   resolves to the parked `Never` face suppresses its argument rows
//!   (the super-formal seam owns that outcome, upstream resolver.dart's
//!   `resolveArgumentsToParameters` never reporting them).
//! - #3884 fn-sig face: the written function-typed formal signature
//!   substitutes through the call's solve before the ask — the
//!   horizontal-inference instance-method shape stays silent.

use super::test_rig::{code_names, report_ops, rows};

/// Item 2, covariant column (the `typedef_static_t22` shape):
/// `typedef check<X> = X Function(X x)` puts `X` in both variance
/// columns. Re-verified 2026-11-05 vs oracle 3.13.7: the oracle stays
/// SILENT on this shape — `..call()` reports only the
/// `not_enough_positional_arguments` arity row, and the
/// `bool -> Never` argument verdict never fires (the
/// `least_greatest_closure.dart` `argument_type_not_assignable`
/// expectation is `unspecified`, not a pinned row). The earlier
/// expectation pinned a row dartq emitted while the inner send
/// over-resolved; that mechanism is gone, matching the oracle.
#[test]
fn lambda_context_covariant_column_never_reports() {
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
        "the oracle stays silent on ..call(true) under the covariant column, got {names:?}"
    );
}

/// Item 2, concrete-formal control: a NON-generic outer callee's context
/// return carries no unsolved parameter, so the raw context types the
/// inner call — `f2(check<int> Function() g)` lands
/// `void Function(int)` and `..call(true)` reports `bool -> int`
/// (oracle-verified: the same row the analyzer reports).
///
/// Held (#3995): the `closure_body_context` arg-slot owner-search
/// (invocation.rs) now reaches `f2(` and the `=>` body send binds its `X`
/// to `int` (probe-verified `inferred=[int]`), but the cascade
/// `captureTypeArgument()..call(true)`'s VALUE is still typed as
/// `call`'s `void` return instead of the `check<int>` target — an
/// `assignability.rs` cascade-section typing regression in the same wave
/// (`use_of_void_result` + `return_of_invalid_type_from_closure` fire in
/// place of the `bool -> int` argument verdict). Routed to the
/// assignability lane.
#[test]
#[ignore = "held: cascade `x..call()` value typed `void` (call's return) not `check<int>` (target) inside a `=>` body — assignability.rs emit_use_of_void_result :5401 reads from==Void; the check<int>-target read and the `bool -> int` arg verdict route to the assignability lane; closure_body_context's X:=int solve (invocation.rs) already landed"]
fn lambda_context_concrete_formal_types_inner_call() {
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

/// Item 2, chain-head control: a send that starts the body but hands its
/// value to a member chain (`id(true).toString()`) never carries the
/// body's context — the chain's result owns it. No
/// `argument_type_not_assignable` fires on the inner call (the oracle's
/// rows for this shape are the closure-return check's, not the inner
/// send's).
#[test]
fn lambda_context_chain_head_stays_uncontexted() {
    let report = report_ops(
        r#"
typedef check<X> = void Function(X x);
T id<T>(T x) => x;

void f<X>(check<X> Function() g) => g();

void main() {
  f(() => id(true).toString());
}
"#,
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"argument_type_not_assignable"),
        "the chain head must not take the body context, got {names:?}"
    );
}

/// M3, the SWAP face (the `test_downwardsInferenceOnInstanceCreations`
/// B-family rows 55/56): `B<S, T> extends A<T, S>` under the context
/// `A<int, String>` binds `T := int, S := String` through the swapped
/// supertype clause, so `B(S y, T x)`'s formals read
/// `(String, int)` — `new B(3, "hello")` reports both directions.
#[test]
fn creation_context_swap_checks_swapped_formals() {
    let report = report_ops(
        r#"
class A<S, T> {
  S x;
  T y;
  A(this.x, this.y);
}

class B<S, T> extends A<T, S> {
  B(S y, T x) : super(x, y);
}

void main() {
  A<int, String> a0 = new B(3, "hello");
}
"#,
    );
    let rows = rows(&report);
    assert!(
        rows.iter()
            .any(|r| r.contains("argument_type_not_assignable")
                && r.contains("'int' can't be assigned to the parameter type 'String'")),
        "expected int -> String on the first swapped formal, got {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|r| r.contains("argument_type_not_assignable")
                && r.contains("'String' can't be assigned to the parameter type 'int'")),
        "expected String -> int on the second swapped formal, got {rows:?}"
    );
}

/// M3, the diamond face (the C-family rows 67/68): `C<S> extends
/// B<S, S>` collapses both slots onto one parameter, so the context
/// `A<int, int>` seeds `S := int` and `C(S a)`'s formal reads `int` —
/// `new C("hello")` reports String -> int.
#[test]
fn creation_context_swap_diamond_collapses_slots() {
    let report = report_ops(
        r#"
class A<S, T> {
  S x;
  T y;
  A(this.x, this.y);
}

class B<S, T> extends A<T, S> {
  B(S y, T x) : super(x, y);
}

class C<S> extends B<S, S> {
  C(S a) : super(a, a);
}

void main() {
  A<int, int> a0 = new C("hello");
}
"#,
    );
    let rows = rows(&report);
    assert!(
        rows.iter()
            .any(|r| r.contains("argument_type_not_assignable")
                && r.contains("'String' can't be assigned to the parameter type 'int'")),
        "expected String -> int on the collapsed formal, got {rows:?}"
    );
}

/// M3, the optional-positional face (the F-family rows 94/95): the
/// seed substitutes through the optional positional formals exactly as
/// the required ones — the raw `T` spelling never reaches the verdict
/// (the two census EXTRAS on this shape named it).
#[test]
fn creation_context_swap_named_optional_positionals() {
    let report = report_ops(
        r#"
class A<S, T> {
  S x;
  T y;
  A(this.x, this.y);
}

class F<S, T> extends A<S, T> {
  F.named(S x, T y, [S a, T b]) : super(x, y);
}

void main() {
  A<int, String> a4 = new F.named(3, "hello", "hello", 3);
}
"#,
    );
    let rows = rows(&report);
    assert!(
        rows.iter()
            .any(|r| r.contains("argument_type_not_assignable")
                && r.contains("'String' can't be assigned to the parameter type 'int'")),
        "expected String -> int on the swapped optional, got {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|r| r.contains("argument_type_not_assignable")
                && r.contains("'int' can't be assigned to the parameter type 'String'")),
        "expected int -> String on the swapped optional, got {rows:?}"
    );
    assert!(
        !rows.iter().any(|r| r.contains("parameter type 'T'")),
        "the raw parameter spelling must not reach the verdict, got {rows:?}"
    );
}

/// M4, the failed-inference i2b fallback (the
/// `constructor2_test_05` row 61): `const C.subtyped(a)` with
/// `a: A<int>` cannot infer `U` (the argument is not a `B<U>`), so the
/// i2b fallback fills `U := dynamic` and the argument check still runs
/// — `A<int>` against `B<dynamic>` reports. The `const` head also
/// pairs the const twin beneath the argument row.
///
/// Routed residual — landed: the i2b map lands and fires
/// (`U := dynamic`, probe-verified at the named-walker map), and the
/// final verdict now rejects `A<int>` against `B<dynamic>` through the
/// FORMAL's supertype unfold (`assignable`'s formal-head subclass face,
/// `assignability.rs`, lane D). Un-ignored with the fix; re-verified
/// 2026-11-05 vs oracle 3.13.7: both rows fire (the `A<int>` argument
/// verdict + the `const_constructor_param_type_mismatch` twin).
#[test]
fn creation_failed_inference_falls_back_to_i2b() {
    let report = report_ops(
        r#"
class A<T> {
  const A();
}

class B<S> extends A<S> {
  const B();
}

class C<U> {
  final A<U> a;
  const C.subtyped(B<U> this.a);
}

void main() {
  const A<int> a = const B<int>();
  const C.subtyped(a);
}
"#,
    );
    let rows = rows(&report);
    assert!(
        rows.iter()
            .any(|r| r.contains("argument_type_not_assignable")
                && r.contains("'A<int>' can't be assigned to the parameter type 'B<dynamic>'")),
        "expected A<int> -> B<dynamic> on the i2b fallback, got {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|r| r.contains("const_constructor_param_type_mismatch")),
        "expected the const twin beneath the argument row, got {rows:?}"
    );
}

/// E8, the parked `Never` super-formal (the `grammar_A12_t01` eight
/// rows): an enum's `const E1(super.x)` over a mixin SETTER resolves
/// the super-formal to the parked `Never` face, and the enum-constant
/// argument checks suppress on it — the super-formal seam owns that
/// outcome, never the enum walker.
#[test]
fn enum_const_never_super_formal_stays_silent() {
    let report = report_ops(
        r#"
mixin M {
  void set x(int val) {}
}

enum E1 with M {
  e1(1),
  e2(2);

  const E1(super.x);
}
"#,
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"argument_type_not_assignable"),
        "the Never super-formal leg must stay silent, got {names:?}"
    );
    assert!(
        !names.contains(&"const_constructor_param_type_mismatch"),
        "the Never super-formal leg must stay silent on the const twin, got {names:?}"
    );
}

/// #3884's fn-sig face: the written `void Function(T) f` formal of a
/// generic instance method substitutes through the call's solve before
/// the closure-literal ask — `b.instanceMethod(0, (x) { … })` solves
/// `T := int` from the first argument and the re-bound closure checks
/// against `void Function(int)`, staying silent (the horizontal
/// sibling's row the raw spelling produced).
#[test]
fn instance_method_closure_formal_reads_solved_param() {
    let report = report_ops(
        r#"
abstract class B {
  T instanceMethod<T>(T t, void Function(T) f);
}

void main() {
  B b = C();
  b.instanceMethod(0, (x) {
    x.abs();
  });
}

class C extends B {
  T instanceMethod<T>(T t, void Function(T) f) => t;
}
"#,
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"argument_type_not_assignable"),
        "the closure formal must read the solved parameter, got {names:?}"
    );
}
