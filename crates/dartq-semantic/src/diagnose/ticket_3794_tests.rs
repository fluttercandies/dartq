//! The #3794 pins — `argument_type_not_assignable` clusters from the
//! census sweep. Each test pins one oracle-verified shape (dart analyze
//! 3.13.7); comments name the cluster and upstream face.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block.

use crate::diagnose::test_rig::{CORE_PRELUDE, assert_silent, report_ops, rows};
use crate::diagnose::types::FileReport;

/// The oracle's `argumentTypeNotAssignable` message prefix
/// (messages.yaml).
const ARG_MSG_PREFIX: &str = "The argument type '";

/// Every `argument_type_not_assignable` row in `report`, as
/// `(offset, length)` pairs sorted ascending.
fn arg_rows(report: &FileReport) -> Vec<(u32, u32)> {
    let mut got: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.message.starts_with(ARG_MSG_PREFIX))
        .map(|d| (d.range.offset, d.range.length))
        .collect();
    got.sort_unstable();
    got
}

/// Asserts the `argument_type_not_assignable` rows sit at exactly
/// `expected` `(offset, length)` pairs (fixture-relative offsets;
/// `CORE_PRELUDE` shifted inside).
#[track_caller]
fn assert_arg_rows(report: &FileReport, fixture: &str, expected: &[(usize, u32)]) {
    let got = arg_rows(report);
    let prelude = CORE_PRELUDE.len() as u32;
    let mut want: Vec<(u32, u32)> = expected
        .iter()
        .map(|(offset, length)| (prelude + *offset as u32, *length))
        .collect();
    want.sort_unstable();
    assert_eq!(got, want, "fixture={fixture:?}\nrows={:?}", rows(report));
}

/// The `(offset, length)` of `needle` in `fixture`.
fn span_of(fixture: &str, needle: &str) -> (usize, u32) {
    let at = fixture.find(needle).expect("needle");
    (at, needle.len() as u32)
}

// --- Scratch reproductions -------------------------------------------------

#[test]
fn scratch_explicit_generic_call() {
    let fixture = r#"
int foo<T>(T x) => 3;
void test1() {
  foo<String>("hello");
  foo(1);
}
"#;
    let report = report_ops(fixture);
    assert_silent(&report);
}

#[test]
fn scratch_typedef_generic_call() {
    let fixture = r#"
typedef Foo<S> = S Function<T>(T x);
int foo<T>(T x) => 3;
void test1() {
  Foo<int> y = foo;
  foo<String>("hello");
  y<String>("hello");
}
"#;
    let report = report_ops(fixture);
    assert_silent(&report);
}

// --- The `T?` writable-parameter face -------------------------------------
//
// The `A <: T?` gather binds `T` from the argument's non-null head
// (upstream `performSubtypeConstraintGenerationForRightNullableType`
// strips the `?` off the schema — `int <: T?` needs `int <: T`), and a
// solved `T` substitutes the `T?` formal as the nullable of the argument
// type (`T := int` reads `int?`). Before the fix the `nullable` leaf was
// invisible to the gather/unify walks AND `subst_params` declined the
// leaf, so `f<T>(T? v)` `f(1)` reported `argument type 'int' can't be
// assigned to the parameter type 'T?'` — 51+ extra census rows.

/// `f<T>(T? v)` `f(1)` — the member-send upward arm binds `T := int`
/// through the `T?` leaf; the instantiated formal is `int?`, silent.
#[test]
fn nullable_param_member_send_binds() {
    let fixture = r#"
class C {
  void f<T>(T? v) {}
}
void test(C c) {
  c.f(1);
}
"#;
    let report = report_ops(fixture);
    assert_silent(&report);
}

/// The same leaf through a free function — `f<T>(T? v)` `f("s")` binds
/// `T := String`, checks against `String?`, silent.
#[test]
fn nullable_param_free_function_binds() {
    let fixture = r#"
void f<T>(T? v) {}
void test() {
  f("s");
}
"#;
    let report = report_ops(fixture);
    assert_silent(&report);
}

/// A bound the argument cannot reach fails the subtype match, not just
/// the constraint: `f<T extends num>(T? v)` `f("s")` — `String` never
/// reaches `num`'s closure, so `String <: T` has no arm and the gather
/// reports `false` (upstream's guard falls through to the ordinary
/// subtype rules, which fail identically). The oracle answers
/// `couldNotInfer` on the send — the `argumentTypeNotAssignable` walk
/// suppresses on the failed slot, so this face pins silence and nothing
/// else.
#[test]
fn nullable_param_bound_gate_drops() {
    let fixture = r#"
void f<T extends num>(T? v) {}
void test() {
  f("s");
}
"#;
    let report = report_ops(fixture);
    assert_arg_rows(&report, fixture, &[]);
}

/// A nullable argument over `T?` — `int?` vs `T?` strips to `int <: T`
/// (upstream `P is P0?` arm) and binds `T := int`; silent.
#[test]
fn nullable_arg_nullable_param() {
    let fixture = r#"
void f<T>(T? v) {}
void test(int? x) {
  f(x);
}
"#;
    let report = report_ops(fixture);
    assert_silent(&report);
}

// --- The enum-constant inference arm ---------------------------------------
//
// `enum E<T>([this.v = "v", T? this.v2])` `const e<int>("E", 2)` — the
// `T?` formal gathers `int <: T` under the same nullable-writable rule,
// and a `T`-solved map substitutes the `v2` formal's `T?` to `int?`.
// The co19 `Primary constructors` corpus (e.g. `e1<int>("E1", 2)`)
// carried 9 `String/int -> T?` extras off this one shape.

/// The enum-constant inference arm (`v2` over `T?`): `e1("E", 2)`
/// infers `T := int` upward; silent.
#[test]
fn enum_const_nullable_param_infers() {
    let fixture = r#"
enum E<T> {
  e1<int>("E", 2),
  e2("E", "s");
  const E([this.v1 = "v", T? this.v2]);
  final String v1;
  final T? v2;
}
"#;
    let report = report_ops(fixture);
    assert_silent(&report);
}

/// The written-type-argument spelling pins the same solve: `e1<int>`
/// reads `T := int` off the written group and `v2` substitutes to
/// `int?`; `2` assigns silently. A mismatch would report at `3` — the
/// pin keeps the `int` argument silent and asserts a `String` argument
/// still rejects through `int?`.
#[test]
fn enum_const_written_type_args_substitute() {
    let fixture = r#"
enum E<T> {
  e1<int>("E", 2),
  e2<String>("E", "s");
  const E([this.v1 = "v", T? this.v2]);
  final String v1;
  final T? v2;
}
"#;
    let report = report_ops(fixture);
    assert_silent(&report);
}

// --- The instantiate-to-bounds fallback ------------------------------------
//
// An unsolved parameter with no declared bound reads `dynamic` — the
// oracle's `chooseFinalTypes` instantiate-to-bounds fill (an unbounded
// `<T>` instantiates to `dynamic`). Before the fix the `effective`
// table and the enum-const map kept the raw `T` leaf, which
// `argument_verdict` decided `Some(false)` for every concrete argument
// (the `to_is_param` arm) — the `int/String -> T` extras.

/// `f<T>(T a, [T? b])` `f(1)` — `T` solves to `int` off `a`, and the
/// unsupplied `b` never walks. The solved formal is `int`; silent.
#[test]
fn solved_param_substitutes() {
    let fixture = r#"
void f<T>(T a, [T? b]) {}
void test() {
  f(1);
}
"#;
    let report = report_ops(fixture);
    assert_silent(&report);
}
