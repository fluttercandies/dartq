//! The #2873 pins: the wave-4 over-fire corrections across four clusters.
//!
//! 1. The super-constructor invocation's untyped initializing formals read
//!    their field's declared type BEFORE the subclass clause substitution,
//!    so `Indexable(this._value)` over `final ReadType _value` pairs
//!    against the written instantiation (`Indexable<String?, String?>`'s
//!    `String?`) and displays the substituted parameter (`E<S, T> extends
//!    A<C<S>, T>` spells `'C<S>'`).
//! 2. The tear-off argument's fn-subtype face decides on the SUBSTITUTED
//!    formal: `f<Function>(tearOff)` pairs against the written `Function`
//!    (silent), the alias instantiation unfolds (`f<VoidCb>(tearOff)`),
//!    and a resolvable class type still refuses with the substituted
//!    display (`f<C>(tearOff)`).
//! 3. A decided matrix verdict is final: the declared-closure negative no
//!    longer demotes a proven `Future<Object?> <: FutureOr<Object>`
//!    assignment (the union's value leg answers through `Object`), while a
//!    genuinely unrelated `FutureOr` pair still reports.
//! 4. A creation's written arguments ride the expression type the message
//!    faces read (`A value of type 'A<int, dynamic>' ...`), never the bare
//!    fold-sentinel head.
//!
//! Oracle provenance: the #2871 wave-4 census rows against `dart analyze`
//! (Dart 3.13, fvm 3.47.1) — the `if_null_assignment_*_index_expression`
//! families, `test_instanceCreation`/`inferDownwards` `E extends A<C<S>,
//! T>`, the co19 `nnbd/list_length` function-pair shapes, and the
//! `await_flatten`/`await_type_check` FutureOr unions.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

fn atna_rows(report: &FileReport) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "argument_type_not_assignable")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

fn invalid_assignment_rows(report: &FileReport) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

/// Byte offset of `needle` inside the fixture source, shifted by the
/// rig's core prelude (the fixture rides behind it on one line).
fn at(source: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + source.find(needle).expect("needle in fixture")) as u32
}

/// The super-invocation field-formal patch precedes the clause
/// substitution (`Test2 extends Indexable<String?, String?>`):
/// `super(null)` pairs `Null` against the substituted `String?` and stays
/// silent — never the raw `ReadType` spelling the field patch used to
/// leave after the substitution window.
#[test]
fn super_field_formal_substitutes_before_the_clause_map() {
    let source = r#"
class Indexable<ReadType, WriteType> {
  final ReadType _value;
  Indexable(this._value);
  ReadType operator [](int index) => _value;
  operator []=(int index, WriteType value) {}
}
class Test2 extends Indexable<String?, String?> {
  Test2() : super(null);
}
main() {
  Test2();
}
"#;
    let report = report_ops(source);
    assert!(atna_rows(&report).is_empty(), "{:?}", report.diagnostics);
}

/// The substituted parameter rides the MESSAGE (`E<S, T> extends A<C<S>,
/// T>`): `super(null, a)` reports the pair against `'C<S>'`, the clause
/// map's written instantiation, at the `null` argument's own span —
/// never the raw superclass parameter `'S'`.
#[test]
fn super_argument_message_displays_the_clause_instantiation() {
    let source = r#"
class A2<S, T> {
  S x;
  T y;
  A2(this.x, this.y);
}
class C2<S> {}
class E2<S, T> extends A2<C2<S>, T> {
  E2(T a) : super(null, a);
}
"#;
    let report = report_ops(source);
    assert_eq!(
        atna_rows(&report),
        [(
            at(source, "null,"),
            4,
            "The argument type 'Null' can't be assigned to the parameter type 'C2<S>'. "
                .to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The unnamed creation's own argument pairs against the constructor
/// formal substituted with the written type arguments: `Indexable<String?,
/// String?>(null)` stays silent (`_value` reads `String?`).
#[test]
fn creation_argument_substitutes_the_written_type_arguments() {
    let source = r#"
class Indexable<ReadType, WriteType> {
  final ReadType _value;
  Indexable(this._value);
  ReadType operator [](int index) => _value;
  operator []=(int index, WriteType value) {}
}
main() {
  Indexable<String?, String?>(null);
}
"#;
    let report = report_ops(source);
    assert!(atna_rows(&report).is_empty(), "{:?}", report.diagnostics);
}

/// The tear-off against a generic formal decides on the SUBSTITUTED
/// parameter (`co19 nnbd/list_length_t01`): `f<Function>(foo, foo)` pairs
/// `void Function()` against the written `Function` supertype — silent,
/// never the out-of-scope `T` spelling the refusal arm used to emit.
#[test]
fn tear_off_against_solved_type_parameter_reads_the_written_type() {
    let source = r#"
class A {}
void foo() {}
void f<T>(T t1, T t2) {}
main() {
  f<Function>(foo, foo);
}
"#;
    let report = report_ops(source);
    assert!(atna_rows(&report).is_empty(), "{:?}", report.diagnostics);
}

/// The alias instantiation unfolds (`f<VoidCb>(foo)`): the solved
/// parameter leaf reads the alias RHS (`void Function()`) and the
/// identical pair stays silent.
#[test]
fn tear_off_against_alias_solved_parameter_unfolds_the_alias() {
    let source = r#"
typedef VoidCb = void Function();
void foo() {}
void f<T>(T t) {}
main() {
  f<VoidCb>(foo);
}
"#;
    let report = report_ops(source);
    assert!(atna_rows(&report).is_empty(), "{:?}", report.diagnostics);
}

/// The refusal keeps firing where the SUBSTITUTED type is an unrelated
/// class (`f<C>(foo)`): a function value subtypes no plain class, and the
/// message displays the substituted spelling (`'C'`), never the raw `T`.
#[test]
fn tear_off_refusal_displays_the_substituted_class_type() {
    let source = r#"
class C {}
void foo() {}
void f<T>(T t) {}
main() {
  f<C>(foo);
}
"#;
    let report = report_ops(source);
    assert_eq!(
        atna_rows(&report),
        [(
            at(source, "foo);"),
            3,
            "The argument type 'void Function()' can't be assigned to the parameter type 'C'. "
                .to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The refusal posture holds on a non-generic class formal: a tear-off
/// subtypes no plain class, and the refused arm still answers after the
/// substitution hoist (`g(foo)` against `g(D d)`).
#[test]
fn tear_off_refusal_holds_on_a_plain_class_formal() {
    let source = r#"
class D {}
void foo() {}
void g(D d) {}
main() {
  g(foo);
}
"#;
    let report = report_ops(source);
    assert_eq!(
        atna_rows(&report),
        [(
            at(source, "foo);"),
            3,
            "The argument type 'void Function()' can't be assigned to the parameter type 'D'. "
                .to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// A decided matrix verdict is final (#2873): `Future<Object?>` assigns
/// into `FutureOr<Object>` through the union's value leg (`Future<Object?>
/// <: Object`), and the declared-closure negative — blind to the
/// Future→FutureOr union — no longer demotes it
/// (await_flatten_test 43:30).
#[test]
fn future_or_union_value_leg_survives_the_declared_negative() {
    let source = r#"
class Future<T> {}
class FutureOr<T> {}
class Cell {}
void main(Future<Cell?> f) {
  FutureOr<Object> x = f;
}
"#;
    let report = report_ops(source);
    assert!(
        invalid_assignment_rows(&report).is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// The union's future leg keeps its covariance: `FutureOr<Future<int>>`
/// accepts `Future<int>` through the value leg (`Future<int> <: Future<int>`)
/// — silent.
#[test]
fn future_or_union_future_of_future_value_leg_assigns() {
    let source = r#"
class Future<T> {}
class FutureOr<T> {}
void main(Future<int> f) {
  FutureOr<Future<int>> y = f;
}
"#;
    let report = report_ops(source);
    assert!(
        invalid_assignment_rows(&report).is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// A genuinely unrelated `FutureOr` pair still reports: `Future<Other>`
/// misses both union legs against `FutureOr<Cell>` and the message spells
/// the full instantiations.
#[test]
fn unrelated_future_or_pair_still_reports() {
    let source = r#"
class Future<T> {}
class FutureOr<T> {}
class Cell {}
class Other {}
void main(Future<Other> f) {
  FutureOr<Cell> x = f;
}
"#;
    let report = report_ops(source);
    assert_eq!(
        invalid_assignment_rows(&report),
        [(
            at(source, "f;"),
            1,
            "A value of type 'Future<Other>' can't be assigned to a variable of type 'FutureOr<Cell>'."
                .to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The creation's written arguments ride the message slot (#2873): the
/// staged assignment renders the instantiated value type
/// (`'A<int, dynamic>'`), never the bare fold-sentinel head (`'A'`).
#[test]
fn creation_instantiation_rides_the_assignment_message() {
    let source = r#"
class A<S, T> {}
main() {
  A<int, String> y = A<int, dynamic>();
}
"#;
    let report = report_ops(source);
    assert_eq!(
        invalid_assignment_rows(&report),
        [(
            at(source, "A<int, dynamic>();"),
            17,
            "A value of type 'A<int, dynamic>' can't be assigned to a variable of type 'A<int, String>'."
                .to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The creation's written arguments ride the argument message slot: the
/// staged argument renders the instantiated value type against the
/// declared parameter (`'A<int, dynamic>'` vs `'A<int, String>'`), never
/// the bare head.
#[test]
fn creation_instantiation_rides_the_argument_message() {
    let source = r#"
class A<S, T> {}
void take(A<int, String> a) {}
main() {
  take(A<int, dynamic>());
}
"#;
    let report = report_ops(source);
    assert_eq!(
        atna_rows(&report),
        [(
            at(source, "A<int, dynamic>());"),
            17,
            "The argument type 'A<int, dynamic>' can't be assigned to the parameter type 'A<int, String>'. "
                .to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The compatible instantiation stays silent: the written arguments the
/// expression type now reattaches must not invent a rejection where the
/// declared pair assigns (`A<int, String>()` into `A<int, String>`).
#[test]
fn compatible_creation_instantiation_stays_silent() {
    let source = r#"
class A<S, T> {}
main() {
  A<int, String> y = A<int, String>();
}
"#;
    let report = report_ops(source);
    assert!(
        invalid_assignment_rows(&report).is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// The corpus shape with function-typed written arguments
/// (`if_null_assignment_index_expression_test` 204:50 + 293/315/337 — the
/// last live cluster-1 sub-shape, 4 extra atna entries in the wave-4
/// report): the creation `Indexable<C1<int> Function()?, Function?>(null)`
/// must pair `Null` against `_value` substituted to
/// `C1<int> Function()?` — silent. Current posture: the `null` argument
/// re-parents into the `[0]` index send's argument group (probed: the
/// `check_arguments` stage fires once, `null` against the `[]` index
/// formal `int`), so the pair mis-binds and over-fires. The probe run
/// localizing the re-parenting hook was cut short by an out-of-scope
/// compile break in `overrides.rs`; the repro stands here for the
/// follow-up.
#[test]
// un-ignored 2026-11-07: passing per stale-ignore triage (ticket #3973)
fn function_typed_written_arguments_substitute_into_the_formal() {
    let source = r#"
class C1<T> {}
class C2<T> {}
class CallableClass<T> {
  T call() => throw '';
}
class Indexable<ReadType, WriteType> {
  final ReadType _value;
  Indexable(this._value);
  ReadType operator [](int index) => _value;
  operator []=(int index, WriteType value) {}
}
main() {
  var callableClassC2Int = CallableClass<C2<int>>();
  Indexable<C1<int> Function()?, Function?>(null)[0] ??= callableClassC2Int;
}
"#;
    let report = report_ops(source);
    assert!(atna_rows(&report).is_empty(), "{:?}", report.diagnostics);
}
