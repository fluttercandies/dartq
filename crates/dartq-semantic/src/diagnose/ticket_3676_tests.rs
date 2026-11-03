//! Ticket #3676 pins (census-12 lane D): the constructor argument
//! type-argument substitution faces — `A(.named(null))` and
//! `A<A>(.named(null))` checked `Null` against a formal the shorthand's
//! own `call_type_args` collision had fixed to `A` (the WRITTEN group of
//! the PARENT creation), or against `dynamic` where the shorthand's
//! instantiation publish had rewritten the parent `A(`'s solve to
//! `A<dynamic>` (the `invalid_assignment` on the enclosing declarator).
//!
//! Shape taxonomy mined from
//! `target/conformance/census-20261102-014958.report.tsv`:
//!
//! - **2.4 — the dot-shorthand constructor's context-typed formals.** A
//!   `.name` callee in argument position resolves under the argument
//!   position's contextType — the parent send's corresponding formal,
//!   instantiated by the parent's downward solve (upstream Dart SDK
//!   source, `pkg/_fe_analyzer_shared/lib/src/type_inference/
//!   type_analyzer.dart` `analyzeDotShorthand` → `pushDotShorthandContext`;
//!   `pkg/analyzer/lib/src/dart/resolver/
//!   constructor_reference_resolver.dart` — the constructor element
//!   instantiates before the argument check). The row model borrows the
//!   still-open parent group's callee row as the shorthand's "receiver",
//!   so both `call_type_args` faces keyed on that row collided with the
//!   parent's own latch: `call_args_for`'s named-selector re-key
//!   CONSUMED the parent `A<A>`'s written entry (`T := A` fixed the
//!   shorthand's formal, where the oracle's `A<dynamic>` context binds
//!   `T := dynamic` and the `null` stays silent), and
//!   `publish_creation_instantiation` wrote the shorthand's solved
//!   `A<dynamic>` onto the parent `A(`'s own offset — which the parent's
//!   close then read back as written arguments (`T := dynamic` producing
//!   the `A<dynamic>` the `invalid_assignment` reported against `A<A>`).
//! - **2.2 — the super-constructor formal's class-parameter
//!   substitution.** `resolveArgumentsToParameters` pairs each
//!   `super(...)` argument with the super constructor's formal
//!   INSTANTIATED by the subclass's written `extends` clause
//!   (`F<S, T> extends A<S, T>` calls `super(x, y)` against `A(S x, T y)`
//!   instantiated `A<S, T>` — the formals keep `S`/`T` and the
//!   `S`/`T`-typed arguments stay silent).
//!
//! Each pin below carries the oracle's verdict for the same fixture
//! (`dart analyze` 3.13.7 — all silent except the four negative controls,
//! which carry `argument_type_not_assignable` exactly once).

use super::test_rig::{assert_silent, code_names, report_ops, rows};

fn argument_type_not_assignable(report: &crate::diagnose::FileReport) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "argument_type_not_assignable")
        .count()
}

/// The shared class fixture the shorthand pins drive: an unnamed plus a
/// `named` constructor over one field-formal `T`.
const CTOR_CLASS: &str = "class A<T> {\n\
     \x20 A(this.x);\n\
     \x20 A.named(this.x);\n\
     \x20 final T x;\n\
     }\n";

/// Sub-rule (a): the shorthand's argument group under the parent's bare
/// head — `A(.named(null))` over `A<A>` context. The parent's `T := A`
/// instantiation hands the shorthand the bare head `A`, which its own
/// to-bounds resolution answers `A<dynamic>` — `null` against `dynamic`
/// is silent. The enclosing `A(...)` creation keeps its context-seeded
/// `A<A>` static type, so the declarator's assignment stays silent too.
///
/// Oracle: no `argument_type_not_assignable`, no `invalid_assignment`.
#[test]
fn shorthand_ctor_arg_under_bare_parent_stays_silent() {
    let report = report_ops(&format!(
        "{CTOR_CLASS}void main() {{\n  A<A> a = A(.named(null));\n  a.x;\n}}\n"
    ));
    assert_silent(&report);
}

/// Sub-rule (b): the shorthand under the parent's WRITTEN `<...>` group —
/// `A<A>(.named(null))`. The written args belong to the parent creation
/// alone; the shorthand's own context is still `A<dynamic>` (the pushed
/// argument-position contextType, instantiated to bounds), so `null`
/// stays silent — the parent's `T := A` never leaks into the shorthand's
/// solve.
///
/// Oracle: no `argument_type_not_assignable`.
#[test]
fn shorthand_ctor_arg_under_written_args_parent_stays_silent() {
    let report = report_ops(&format!(
        "{CTOR_CLASS}void main() {{\n  A<A> b = A<A>(.named(null));\n  b.x;\n}}\n"
    ));
    assert_silent(&report);
}

/// The two-formal fixture the later-positional pin drives: under `B<B>`
/// the second formal `T x` binds `T := B`, so the shorthand resolves
/// `B<dynamic>.named` over the bare `B` head and `(0, null)` against
/// `(dynamic, dynamic)` stays silent.
const TWO_FORMAL_CLASS: &str = "class B<T> {\n\
     \x20 B(dynamic d, this.x);\n\
     \x20 B.named(dynamic d, this.x);\n\
     \x20 final T x;\n\
     }\n";

/// Sub-rule (b′): the shorthand as a LATER positional argument still
/// resolves against the instantiated formal — `B<B>(0, .named(0, null))`
/// keeps `null` silent (the borrowed "receiver" row is a previous
/// argument's, never the parent's callee).
///
/// Oracle: no `argument_type_not_assignable`.
#[test]
fn shorthand_ctor_arg_in_later_positional_slot_stays_silent() {
    let report = report_ops(&format!(
        "{CTOR_CLASS}{TWO_FORMAL_CLASS}void main() {{\n  B<B> b = B<B>(0, .named(0, null));\n  b.x;\n}}\n"
    ));
    assert_silent(&report);
}

/// Sub-rule (c): the `new`-keyword creation's written-args face —
/// `new B<String, int>("hello", 3)` substitutes `S := String`, `T := int`
/// into `B`'s `this.y`/`this.x` formals, so `"hello"` (into `String`) and
/// `3` (into `int`) both stay silent. The named-selector spelling shares
/// the instantiation.
///
/// Oracle: no `argument_type_not_assignable`.
#[test]
fn written_args_creation_substitutes_formals() {
    let report = report_ops(
        "class A<S, T> {\n\
         \x20 S x;\n\
         \x20 T y;\n\
         \x20 A(this.x, this.y);\n\
         \x20 A.named(this.x, this.y);\n\
         }\n\
         class B<S, T> extends A<T, S> {\n\
         \x20 B(S y, T x) : super(x, y);\n\
         \x20 B.named(S y, T x) : super.named(x, y);\n\
         }\n\
         void main() {\n\
         \x20 A<int, String> a2 = new B<String, int>(\"hello\", 3);\n\
         \x20 A<int, String> a3 = new B<String, int>.named(\"hello\", 3);\n\
         \x20 a2.x;\n  a3.x;\n\
         }\n",
    );
    assert_silent(&report);
}

/// Sub-rule (d): the super-constructor formal substitution through the
/// subclass's written clause — `B<S, T> extends A<T, S>` calls
/// `super(x, y)` where the formals answer `T x`/`S y` (the clause's
/// hop substitution), and `D<S, T> extends B<T, int>` calls
/// `super(a, 3)` where `B`'s `S := T`/`T := int` re-instantiate the
/// super formals `T`/`int`. Every argument stays silent.
///
/// Oracle: no `argument_type_not_assignable` on the `super(...)`
/// arguments.
#[test]
fn super_ctor_formals_substitute_through_written_clause() {
    let report = report_ops(
        "class A<S, T> {\n\
         \x20 S x;\n\
         \x20 T y;\n\
         \x20 A(this.x, this.y);\n\
         \x20 A.named(this.x, this.y);\n\
         }\n\
         class B<S, T> extends A<T, S> {\n\
         \x20 B(S y, T x) : super(x, y);\n\
         \x20 B.named(S y, T x) : super.named(x, y);\n\
         }\n\
         class D<S, T> extends B<T, int> {\n\
         \x20 D(T a) : super(a, 3);\n\
         \x20 D.named(T a) : super.named(a, 3);\n\
         }\n",
    );
    assert_silent(&report);
}

/// Sub-rule (e): the enum-constant arguments — `v1<int>(10)` writes
/// `T := int`, `v2(20)`/`v3('abc')` infer `T := int`/`String` upward.
/// Each `this.f` field-formal reads its substituted face, so every
/// constant's argument stays silent.
///
/// Oracle: no `argument_type_not_assignable` (the corpus's
/// `test_enum_enhanced_unnamed` fragment).
#[test]
fn enum_const_args_substitute_class_params() {
    let report = report_ops(
        "enum E<T> {\n\
         \x20 v1<int>(10),\n\
         \x20 v2(20),\n\
         \x20 v3('abc');\n\
         \x20 final T f;\n\
         \x20 const E(this.f);\n\
         }\n\
         const x1 = E.v1;\n\
         const x2 = E.v2;\n\
         const x3 = E.v3;\n",
    );
    assert_silent(&report);
}

/// The wrapper class whose formal keeps the shorthand's context
/// CLASS-shaped: under `C<int>` the `A<T>` formal binds `T := int`, so
/// the `.named` shorthand resolves `A<int>.named` and its `x : int`
/// formal still rejects `'str'` (a bare `A<int> a = A(.named('str'))`
/// hands the shorthand the scalar context `int` instead — the `.named`
/// lookup misses there entirely, which is a different code face).
const SHORTHAND_FORMAL_CLASS: &str = "class C<T> {\n\
     \x20 C(this.w);\n\
     \x20 C.named(this.w);\n\
     \x20 final A<T> w;\n\
     }\n";

/// Negative control — the plain written-selector spelling still reports:
/// `A<int> a = A.named('str')` resolves `A<int>.named`, `'str'` into the
/// `x : int` formal is `argument_type_not_assignable`.
///
/// Oracle: one `argument_type_not_assignable`.
#[test]
fn written_selector_arg_type_mismatch_reports() {
    let report = report_ops(&format!(
        "{CTOR_CLASS}void main() {{\n  A<int> a = A.named('str');\n  a.x;\n}}\n"
    ));
    assert_eq!(
        argument_type_not_assignable(&report),
        1,
        "'str' into the instantiated `x : int` formal rejects: {:?}",
        rows(&report)
    );
}

/// Negative control — the shorthand's own instantiated formals still
/// reject under a class-shaped context: `C<int> c = C(.named('str'))`
/// pushes `A<int>` as the shorthand's context, so `A<int>.named`'s
/// `x : int` formal rejects `'str'` — the pushed context's `T := int`
/// really reaches the shorthand's solve.
///
/// Oracle: one `argument_type_not_assignable`.
#[test]
fn shorthand_ctor_arg_type_mismatch_reports() {
    let report = report_ops(&format!(
        "{CTOR_CLASS}{SHORTHAND_FORMAL_CLASS}void main() {{\n  C<int> c = C(.named('str'));\n  c.w;\n}}\n"
    ));
    assert_eq!(
        argument_type_not_assignable(&report),
        1,
        "the shorthand context `A<int>` keeps the `x : int` rejection: {:?}",
        rows(&report)
    );
}

/// Negative control — the same mismatch under the parent's written group:
/// `C<int> d = C<int>(.named('str'))` — the written `<int>` belongs to
/// the parent `C` alone, but the pushed context still answers `A<int>`,
/// so the shorthand's `x : int` formal rejects `'str'`.
///
/// Oracle: one `argument_type_not_assignable`.
#[test]
fn shorthand_ctor_arg_mismatch_under_written_parent_reports() {
    let report = report_ops(&format!(
        "{CTOR_CLASS}{SHORTHAND_FORMAL_CLASS}void main() {{\n  C<int> d = C<int>(.named('str'));\n  d.w;\n}}\n"
    ));
    assert_eq!(
        argument_type_not_assignable(&report),
        1,
        "the written parent's pushed context still answers `A<int>`: {:?}",
        rows(&report)
    );
}

/// Negative control — `code_names` keeps the failure legible: the
/// written-selector mismatch reports `argument_type_not_assignable` and
/// nothing else.
#[test]
fn negative_controls_report_only_argument_type_not_assignable() {
    let report = report_ops(&format!(
        "{CTOR_CLASS}void main() {{\n  A<int> a = A.named('str');\n  a.x;\n}}\n"
    ));
    assert_eq!(code_names(&report), ["argument_type_not_assignable"]);
}
