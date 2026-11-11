//! The #3294 pins: the `type_argument_not_matching_bounds` face census.
//!
//! The census (458 cases, 229 failing at the time of the pass) resolved to a
//! verdict layer that is already correct on every site the engine drives, and
//! to a residual that never reaches a verdict at all. This file pins both
//! halves so a later change cannot drift either direction unnoticed:
//!
//! * the reachable faces (`Engine::check_site` for a written `<...>` group,
//!   `check_clause_sites`, `check_alias_rhs_sites`, `check_bound_annotation_sites`)
//!   decide the upstream visitor's verdict — regular pass, then the
//!   super-bounded inverted pass where `_shouldAllowSuperBoundedTypes` allows it
//!   (upstream Dart SDK source, pkg/analyzer/lib/src/error/
//!   type_arguments_verifier.dart:312-539);
//! * the unreachable faces (an expression type literal, a bare-head
//!   annotation, a generic-function-type argument) are recorded as ignored
//!   pins naming the module that owns each gap, so the day that module lands
//!   the fix the pin turns green instead of the gap going silent again.
//!
//! Oracle texts are never hand-crafted: each message below is the exact
//! `type_argument_not_matching_bounds` spelling the corpus carries for the
//! same shape.

use super::test_rig::report_ops;

/// The `type_argument_not_matching_bounds` rows of `report` as
/// `offset|length|message`.
fn bounds_rows(report: &super::FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "type_argument_not_matching_bounds")
        .map(|d| format!("{}|{}|{}", d.range.offset, d.range.length, d.message))
        .collect()
}

/// A written type-argument group at an annotation reports at the offending
/// argument (upstream `test_extends` shape): `C<String>` over
/// `C<T extends int>`.
#[test]
fn annotation_written_args_report() {
    let hits = bounds_rows(&report_ops(
        "class C<T extends int> {}\nvoid f(C<String> x) {}\n",
    ));
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert!(
        hits[0].ends_with("'String' doesn't conform to the bound 'int' of the type parameter 'T'."),
        "{hits:#?}"
    );
}

/// An argument that is a top type violates nothing (`dynamic` never
/// violates a bound): `C<dynamic>` over `C<T extends num>` stays silent.
#[test]
fn dynamic_argument_stays_silent() {
    let report = report_ops("class C<T extends num> {}\nvoid f(C<dynamic> x) {}\n");
    assert_eq!(bounds_rows(&report), Vec::<String>::new(), "{report:#?}");
}

/// A nullable argument is not a subtype of the bound: `int?` over
/// `T extends num` reports.
#[test]
fn nullable_argument_reports() {
    let hits = bounds_rows(&report_ops(
        "class C<T extends num> {}\nvoid f(C<int?> x) {}\n",
    ));
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert!(
        hits[0].ends_with("'int?' doesn't conform to the bound 'num' of the type parameter 'T'."),
        "{hits:#?}"
    );
}

/// A bare supertype clause head instantiates to bounds and reports the
/// regular violation — the clause contexts deny the super-bounded pass
/// (`_shouldAllowSuperBoundedTypes`, :737-748), so the regular argument is
/// the reported one.
#[test]
fn bare_clause_head_reports_regular_arm() {
    let hits = bounds_rows(&report_ops(
        "class A<X> {}\nclass B<T extends A<T>> {}\nclass C extends B {}\n",
    ));
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert!(
        hits[0].ends_with(
            "'A<dynamic>' doesn't conform to the bound 'A<A<dynamic>>' of the type parameter 'T'."
        ),
        "{hits:#?}"
    );
}

/// A bare type alias right-hand side is a `NamedType` under the alias
/// context, which also denies the super-bounded pass: the regular arm
/// reports at the alias head.
#[test]
fn bare_alias_rhs_head_reports() {
    let hits = bounds_rows(&report_ops(
        "class A<X> {}\nclass B<T extends A<T>> {}\ntypedef H = B;\n",
    ));
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert!(
        hits[0].ends_with(
            "'A<dynamic>' doesn't conform to the bound 'A<A<dynamic>>' of the type parameter 'T'."
        ),
        "{hits:#?}"
    );
}

/// An alias head at an annotation checks the ALIAS's own parameters
/// (`type.alias != null`, :321-325) — `F<String>` over `F<T extends num>`.
#[test]
fn alias_head_written_args_report() {
    let hits = bounds_rows(&report_ops(
        "typedef F<T extends num> = void;\nF<String> x = null;\n",
    ));
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert!(
        hits[0].ends_with("'String' doesn't conform to the bound 'num' of the type parameter 'T'."),
        "{hits:#?}"
    );
}

/// A class type alias's mixin application is a supertype clause of the
/// synthesized class — `_shouldAllowSuperBoundedTypes`' `ClassTypeAlias` arm
/// denies the super pass, so the regular argument reports.
#[test]
fn class_type_alias_clause_reports() {
    let hits = bounds_rows(&report_ops(
        "class A {}\nclass B {}\nmixin C {}\nclass G<E extends A> {}\nclass D = G<B> with C;\n",
    ));
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert!(
        hits[0].ends_with("'B' doesn't conform to the bound 'A' of the type parameter 'E'."),
        "{hits:#?}"
    );
}

/// The creation site and the generic-call site both carry their written
/// arguments into the same verdict (`G<B>()`, `g<String>('x')`), in
/// declaration order.
#[test]
fn creation_and_call_args_report() {
    let hits = bounds_rows(&report_ops(
        "class A {}\nclass B {}\nclass G<E extends A> {\n  G();\n}\n\
         void g<T extends num>(T x) {}\n\
         void main() {\n  G<B>();\n  g<String>('x');\n}\n",
    ));
    assert_eq!(hits.len(), 2, "{hits:#?}");
    assert!(
        hits[0].ends_with("'B' doesn't conform to the bound 'A' of the type parameter 'E'."),
        "{hits:#?}"
    );
    assert!(
        hits[1].ends_with("'String' doesn't conform to the bound 'num' of the type parameter 'T'."),
        "{hits:#?}"
    );
}

/// A dependent pair decides each argument against its own substituted
/// bound: `C<D<int>, D<dynamic>>` over `C<T extends D<T>, U extends D<T>>`
/// reports `T` only — `D<dynamic>` satisfies `D<D<int>>` through the
/// covariant `dynamic` argument.
#[test]
fn dependent_bounds_report_only_the_failing_argument() {
    let hits = bounds_rows(&report_ops(
        "class D<S> {}\nclass C<T extends D<T>, U extends D<T>> {}\n\
         void f(C<D<int>, D<dynamic>> x) {}\n",
    ));
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert!(
        hits[0].ends_with(
            "'D<int>' doesn't conform to the bound 'D<D<int>>' of the type parameter 'T'."
        ),
        "{hits:#?}"
    );
}

/// The super-bounded arm stays silent where the inverted instantiation is
/// well-bounded: `B` (bare, over `T extends A<T>`) instantiates to
/// `B<A<dynamic>>`, whose inversion `B<A<Never>>` is regular-bounded.
#[test]
fn bare_head_super_bounded_stays_silent() {
    let report = report_ops("class A<X> {}\nclass B<T extends A<T>> {}\nvoid f(B b) {}\n");
    assert_eq!(bounds_rows(&report), Vec::<String>::new(), "{report:#?}");
}

/// A function-typed argument is decided both ways: `void Function(String)`
/// violates `T extends void Function(int)`, `void Function(num)` satisfies
/// it (parameter contravariance).
#[test]
fn function_type_argument_decides_both_ways() {
    let hits = bounds_rows(&report_ops(
        "class C<T extends void Function(int)> {}\nvoid f(C<void Function(String)> x) {}\n",
    ));
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert!(
        hits[0].contains("doesn't conform to the bound 'void Function(int)'"),
        "{hits:#?}"
    );
    let silent = report_ops(
        "class C<T extends void Function(int)> {}\nvoid f(C<void Function(num)> x) {}\n",
    );
    assert_eq!(bounds_rows(&silent), Vec::<String>::new(), "{silent:#?}");
}

/// An in-bounds annotation, creation, and call stay silent (the
/// regression direction of the whole face).
#[test]
fn in_bounds_arguments_stay_silent() {
    let report = report_ops(
        "class C<T extends num> {\n  C(T x);\n  void m<S extends num>(S s) {}\n}\n\
         void g<T extends num>(T x) {}\n\
         void f(C c) {\n  c.m<int>(0);\n  g<int>(0);\n  C<int>(0);\n}\n",
    );
    assert_eq!(bounds_rows(&report), Vec::<String>::new(), "{report:#?}");
}

// —— the gaps: ignored pins naming the owning module ————————————————

/// A bare-head parameter annotation (`void f(G g)`) is never checked: the
/// `PendingBoundedCheck::Site` capture is pushed by `Engine::end_group`
/// (`diagnose/invocation.rs`), which only fires for a written `<...>` group,
/// so a head with no arguments reaches no verdict. The verdict itself is
/// ready for it — `Bounds::check_named`'s `args.is_empty()` arm already
/// instantiates to bounds and reports with the raw-type context row.
///
/// Ignored until `diagnose/invocation.rs` (or the listener's `in_site` gate)
/// queues the group-less head; upstream `foo(G g)` over
/// `typedef G<X extends A<X>> = void Function<Y extends X>();` reports at
/// the head (co19 `typedef_l2_*`, `TypeArgumentNotMatchingBoundsTest`'s
/// `test_notRegularBounded_notSuperBounded_parameter_invariant`).
// un-ignored 2026-11-07: passing per stale-ignore triage (ticket #3973)
#[test]
fn bare_head_annotation_reports() {
    let hits = bounds_rows(&report_ops(
        "typedef A<X> = X Function(X);\n\
         typedef G<X extends A<X>> = void Function<Y extends X>();\n\
         foo(G g) {}\n",
    ));
    assert_eq!(hits.len(), 1, "{hits:#?}");
}

/// A generic-function-type argument is never judged: the bound and the
/// argument are both generic function types, whose conformance needs the
/// generic-function subtyping rule (`int Function(int)` is NOT a subtype of
/// `T Function<T>(T)`), which the `dartq-types` subtype walk cannot decide —
/// the verdict suppresses rather than answer.
///
/// Ignored until `crates/dartq-types` decides generic-function conformance;
/// upstream `CB<int Function(int)>` over `CB<T extends F>`,
/// `typedef F = T Function<T>(T)`, reports.
// un-ignored 2026-11-07: passing per stale-ignore triage (ticket #3973)
#[test]
fn generic_function_type_argument_reports() {
    let hits = bounds_rows(&report_ops(
        "typedef F = T Function<T>(T);\nclass CB<T extends F> {}\n\
         void f(CB<int Function(int)> a) {}\n",
    ));
    assert_eq!(hits.len(), 1, "{hits:#?}");
}

/// A bare-head extension type (`void f(A a)` over
/// `extension type A<T extends A<T>>(int it)`) is the same group-less site
/// as [`bare_head_annotation_reports`]; the extension-type head additionally
/// denies the super-bounded pass (`_shouldAllowSuperBoundedTypes`'
/// extension arm), so the regular arm is the one that must report.
///
/// Ignored for the same reason; upstream `test_extensionType_superBounded`
/// reports `'A<dynamic>' doesn't conform to the bound 'A<A<dynamic>>'`.
// un-ignored 2026-11-07: passing per stale-ignore triage (ticket #3973)
#[test]
fn bare_head_extension_type_reports() {
    let hits = bounds_rows(&report_ops(
        "extension type A<T extends A<T>>(int it) {}\nvoid f(A a) {}\n",
    ));
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert!(
        hits[0].ends_with(
            "'A<dynamic>' doesn't conform to the bound 'A<A<dynamic>>' of the type parameter 'T'."
        ),
        "{hits:#?}"
    );
}
