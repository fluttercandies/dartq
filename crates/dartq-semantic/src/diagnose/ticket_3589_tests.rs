//! The #3589 pins: a written `Never` type-argument slot is not inference
//! residue.
//!
//! `covariant_args`'s residue skip (assignability.rs, the `f == Ty::Dynamic
//! && t is an unbound formal head` arm) exists for the generic-method
//! inference shape — `accept<E>(V<E>)` over a `V<dynamic>` argument binds
//! `E = dynamic`, so the written `V<dynamic>`/`V<E>` pair never reaches the
//! subtype question. The `unbound_formal_head` predicate that detects the
//! residue answers true for any non-primitive, non-collection head that
//! resolves to no container — and `Never` is exactly that (not in
//! `FINAL_PRIMITIVES`, not a collection head, no container resolution). A
//! WRITTEN `Never` type argument therefore read as residue: `F<dynamic>`
//! into `F<Never>` skipped the element verdict and stayed silent where the
//! oracle requires `dynamic <: Never` to refuse.
//!
//! Census shape: the co19 `Instantiate-to-bound` and `Simple-bounds`
//! families (~50 `invalid_assignment` misses) — instantiate-to-bounds lands
//! a `Never` default inside written argument shapes (`F<A<Never>?>?` in
//! `Simple-bounds/static/class_l1_t01`, the `A<Never>` bound rows in
//! `Instantiate-to-bound/class/static/*`), and nested recursion reaches the
//! same `(dynamic, Never)` element pair this guard swallowed.
//!
//! Oracle provenance (3.13.7, `dart analyze --format=machine`,
//! `F<Never> y = x;` over `x: F<dynamic>`):
//!   `ERROR|COMPILE_TIME_ERROR|INVALID_ASSIGNMENT|...|3|16|1|A value of
//!    type 'F<dynamic>' can't be assigned to a variable of type
//!    'F<Never>'.`
//! The reversed pair `F<Never>` into `F<dynamic>` is silent upstream
//! (`Never <: dynamic` is true) — the fix must not fabricate it. The true
//! inference-residue control below keeps the skip for an unsolved generic
//! formal's own type-variable slot (`V<dynamic>` into `V<E>` at a generic
//! send) — dropping THAT direction would trade the fixed rows for a wave
//! of `argument_type_not_assignable` fabrications.

use super::test_rig::report_ops;

fn code_names(report: &super::FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// A written `Never` slot refuses the `dynamic` argument:
/// `F<Never> y = x;` over `x: F<dynamic>` reports `invalid_assignment`
/// (oracle row pinned in the module doc; the `unused_local_variable`
/// warning rides along upstream too — the pair is what the pin checks).
#[test]
fn dynamic_into_written_never_slot_reports() {
    let report = report_ops(
        "class F<T> {}\n\
         void m(F<dynamic> x) {\n\
         \x20 F<Never> y = x;\n\
         }\n",
    );
    assert!(
        code_names(&report).contains(&"invalid_assignment"),
        "{:?}",
        code_names(&report)
    );
}

/// The same element verdict through a nested head — `F<A<dynamic>>` into
/// `F<A<Never>>` recurses into `covariant_args` on the `(dynamic, Never)`
/// pair and reports once (the co19 `Simple-bounds` shape, where the
/// written `Never` lands inside a deeper argument list).
#[test]
fn nested_dynamic_into_written_never_slot_reports() {
    let report = report_ops(
        "class A<T> {}\n\
         class F<T> {}\n\
         void m(F<A<dynamic>> x) {\n\
         \x20 F<A<Never>> y = x;\n\
         }\n",
    );
    assert!(
        code_names(&report).contains(&"invalid_assignment"),
        "{:?}",
        code_names(&report)
    );
}

/// The reversed pair stays silent: `F<dynamic> y = x;` over
/// `x: F<Never>` is assignable upstream (`Never <: dynamic`), and the
/// narrowed guard must not fabricate a row in the other direction.
#[test]
fn never_into_dynamic_slot_stays_silent() {
    let report = report_ops(
        "class F<T> {}\n\
         void m(F<Never> x) {\n\
         \x20 F<dynamic> y = x;\n\
         }\n",
    );
    assert!(
        !code_names(&report).contains(&"invalid_assignment"),
        "{:?}",
        code_names(&report)
    );
}

/// The TRUE residue control — the skip's own reason for existing. A
/// `dynamic` argument into an unsolved generic formal's type-variable
/// slot is the call-site inference's ground (`accept<E>(V<E>)` binds
/// `E = dynamic`), so the written `V<dynamic>`/`V<E>` residue pair stays
/// silent. The `is_never_ty` narrowing must not disturb this arm: the
/// residue head `E` is no `Never`.
#[test]
fn dynamic_argument_into_unsolved_generic_formal_still_skips() {
    let report = report_ops(
        "class V<T> {\n\
         \x20 int visit() => 1;\n\
         }\n\
         class Node {\n\
         \x20 int accept<E>(V<E> visitor) => visitor.visit();\n\
         }\n\
         void m(Node node, V<dynamic> visitor) {\n\
         \x20 node.accept(visitor);\n\
         }\n",
    );
    assert!(
        !code_names(&report).contains(&"argument_type_not_assignable"),
        "{:?}",
        code_names(&report)
    );
}
