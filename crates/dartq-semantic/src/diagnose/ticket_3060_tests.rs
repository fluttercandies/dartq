//! The #3060 pins — the instantiate-to-bound verdict face over
//! self-referential bounds, and the type-parameter write target.
//!
//! Root A (`type_argument_not_matching_bounds`): the verifier's subtype
//! walks run on the fact-armed system, so a self-referential bound's own
//! annotation site decides `A<X> <: A<A<A<X>>>` through the left-variable
//! bound reflexivity (`bound(X) <: A<A<X>>` — subtype.dart:490-500) and
//! stays silent exactly where the oracle is silent
//! (`Instantiate-to-bound/class/static/class_l1_t02`), while the written
//! argument sites keep firing.
//!
//! Root B (`invalid_assignment`): a type-parameter write target decides
//! against the slot's own `T`/`T?` (`writeType`,
//! assignment_expression_resolver.dart `_checkForInvalidAssignment`) — an
//! interface source is a definite rejection (the Left Variable terminus,
//! `static5_test_06`'s `final U x = const A()`), `Null` meets only the
//! nullable conjunct, a different parameter source rejects, and the same
//! parameter is reflexive.

use super::test_rig::report_ops;
use super::*;

fn codes(folded: &FileReport) -> Vec<&str> {
    folded
        .diagnostics
        .iter()
        .map(|diag| diag.code.name)
        .collect()
}

fn tanmb_count(folded: &FileReport) -> usize {
    folded
        .diagnostics
        .iter()
        .filter(|diag| diag.code.name == "type_argument_not_matching_bounds")
        .count()
}

/// The self-referential bound's own annotation site is silent: `A<X> <:
/// A<A<A<X>>>` holds through `bound(X) = A<A<X>>` reflexivity, the oracle's
/// answer at `class A<X extends A<A<X>>>` (class_l1_t02's declaration
/// line reports nothing).
#[test]
fn self_referential_bound_site_is_silent() {
    let folded = report_ops("class A<X extends A<A<X>>> {}\n");
    assert_eq!(
        codes(&folded),
        Vec::<&str>::new(),
        "the bound annotation `A<A<X>>` asks A<X> <: A<A<A<X>>> — true via the bound"
    );
}

/// The same declaration's written argument sites keep firing: `A<Null>`
/// asks `Null <: A<A<Null>>` — a definite rejection (class_l1_t02 line 74).
#[test]
fn written_argument_on_self_referential_bound_reports() {
    let folded = report_ops("class A<X extends A<A<X>>> {}\nvoid f(A<Null> x) {}\n");
    assert_eq!(
        tanmb_count(&folded),
        1,
        "`A<Null>` against the substituted bound `A<A<Null>>` reports"
    );
}

/// The direct same-head cycle at a raw use stays silent (the raw site's
/// instantiation is super-bounded — the oracle's inverted pass rescues it;
/// probed 3.13.1: `class A<T extends A<T>> {} void g(A a) {}` reports
/// nothing).
#[test]
fn same_head_cycle_raw_use_is_silent() {
    let folded = report_ops("class C<T extends C<T>> {}\nvoid g(C c) {}\n");
    assert_eq!(
        tanmb_count(&folded),
        0,
        "the raw head's grounded instantiation is super-bounded — silent"
    );
}

/// A written argument naming the top type on the same-head cycle stays
/// silent: the regular pass refuses `dynamic <: A<dynamic>` and the
/// inverted pass accepts `A<Never>` (probed 3.13.1, the a9 posture).
#[test]
fn top_type_argument_on_cycle_is_super_bounded() {
    let folded = report_ops("class A<T extends A<T>> {}\nvoid f(A<dynamic> x) {}\n");
    assert_eq!(
        tanmb_count(&folded),
        0,
        "`A<dynamic>` inverts to `A<Never>` — super-bounded, silent"
    );
}

/// The extension-type written form keeps its double report (the head and
/// the nested argument each verdict — the super pass is forbidden at
/// extension-type elements, type_arguments_verifier.dart:710-712).
#[test]
fn extension_type_written_args_report_twice() {
    let folded = report_ops(
        "extension type A<T extends A<T>>(int it) {}\nvoid f() {\n  A<A<dynamic>> x;\n}\n",
    );
    assert_eq!(
        tanmb_count(&folded),
        2,
        "the outer head and the inner `A<dynamic>` each report"
    );
}

/// A bounded alias application's written argument keeps its verdict
/// (`G<Null>` against bound `num` — typedef_l1_t02's surviving rows).
#[test]
fn alias_application_argument_reports() {
    let folded = report_ops("typedef G<X extends num> = void Function();\nvoid f(G<Null> g) {}\n");
    assert_eq!(
        tanmb_count(&folded),
        1,
        "`G<Null>` asks Null <: num — a definite rejection"
    );
}

/// Root B head pin: a field initializer against a type-parameter field is
/// the oracle's `invalid_assignment` (`static5_test_06`).
#[test]
fn field_initializer_into_type_param_reports() {
    let folded =
        report_ops("class A {\n  const A();\n}\nclass C<U> {\n  final U x = const A();\n}\n");
    assert_eq!(
        codes(&folded),
        vec!["invalid_assignment"],
        "`const A()` against the slot's own `U` is a definite rejection"
    );
}

/// A plain write over a `T?` slot reports against the slot's own type
/// (`test_typeParameter`'s `value = n` family).
#[test]
fn write_into_nullable_type_param_reports() {
    let folded = report_ops("class C {}\nvoid f<T extends C>(T? v, C c) {\n  v = c;\n}\n");
    assert_eq!(
        codes(&folded),
        vec!["invalid_assignment"],
        "`C` into `T?` — an interface source never subtypes the parameter"
    );
}

/// A different parameter source into a parameter target is a definite
/// rejection — `bound(S) <: T` ends at the interface-vs-parameter refusals
/// (probed 3.13.1: `void g<S, T>(S s, T t) { t = s; }` reports).
#[test]
fn different_param_source_into_param_target_reports() {
    let folded = report_ops("void g<S, T>(S s, T t) {\n  t = s;\n}\n");
    assert_eq!(
        codes(&folded),
        vec!["invalid_assignment"],
        "`S` into `T` — the bound chain terminates at the parameter refusal"
    );
}

/// `Null` into a non-nullable parameter target rejects; `Null` into `T?`
/// is the nullable conjunct (silent), and the same parameter is reflexive.
#[test]
fn null_and_reflexive_param_writes_follow_the_conjunct() {
    let non_nullable = report_ops("void f<T>(T v) {\n  v = null;\n}\n");
    assert_eq!(
        codes(&non_nullable),
        vec!["invalid_assignment"],
        "`Null <: T` fails the nullable conjunct"
    );
    let nullable = report_ops("void h<T>(T? v) {\n  v = null;\n}\n");
    assert_eq!(codes(&nullable), Vec::<&str>::new(), "`Null <: T?` holds");
    let reflexive = report_ops("void k<T>(T t) {\n  t = t;\n}\n");
    assert_eq!(
        codes(&reflexive),
        Vec::<&str>::new(),
        "`T <: T` is the reflexivity arm"
    );
}
