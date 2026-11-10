//! Ticket #3662 pins (census-15): `wrong_number_of_type_arguments` —
//! 46 misses / 244 extras from census-20261102-230100.report.tsv. The
//! extras split into four mechanisms:
//!
//! - **(a) the member-miss receiver-arity fallback** (`recv.name<T>` on a
//!   `Walk::Missing` used to fall back to the RECEIVER's declared arity —
//!   firing `x.expectStaticType<T>`-style extension/unresolved members on
//!   `num`/`Object`/`int` receivers, `o.instanceMethod<int,String>`
//!   promotion tests, `super.foo<T>` misses, `i<int>.foo()` dynamic
//!   receivers). Upstream (`type_instantiation_target.dart`) has no arity
//!   verdict for a non-constructor member name; the `undefined_*`/
//!   `disallowedTypeInstantiation` faces own those spellings.
//! - **(b) constructor selectors `A<int>.ctor<T>`** now report the
//!   dedicated `wrong_number_of_type_arguments_constructor` at the bare
//!   `<...>` group instead of the base code.
//! - **(c) the `NamedType` anchor**: annotation-position heads report the
//!   whole `name<args>` (+ written `?`) span — upstream
//!   `_buildTypeArguments` `.at(node)` — not the bare group.
//! - **(d) `declared_type_params` no longer answers `Some(0)` for
//!   non-type heads** (`var A`, `int v`, `void T()`) — the
//!   `BareInfo::Plain` top-level-decl fallback is filtered to
//!   type-defining kinds only.
//!
//! Each pin carries the oracle's verdict for the same fixture
//! (`dart analyze` 3.13.7); positives name the code and anchor verbatim.

use super::test_rig::{assert_silent, code_names, report_ops, report_ops_bare, rows};

fn code_count(report: &crate::diagnose::FileReport, name: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .count()
}

fn wnta(report: &crate::diagnose::FileReport) -> usize {
    code_count(report, "wrong_number_of_type_arguments")
}

fn wnta_ctor(report: &crate::diagnose::FileReport) -> usize {
    code_count(report, "wrong_number_of_type_arguments_constructor")
}

/// (a) A member name that misses the receiver's member table must not
/// consult the receiver's own arity — `num`/`Object`/`int` declare no
/// `expectStaticType` (it's a test-harness extension upstream), so
/// `x.expectStaticType<T>` reports only the `undefined_method` face,
/// never the receiver's type-argument count.
///
/// Oracle: `undefined_method` per send, no wrong_number_of_type_arguments.
#[test]
fn extension_member_on_core_receiver_reports_no_arity() {
    let report = report_ops(
        "void main() {\n\
         \x20 num n = 0;\n\
         \x20 Object o = n;\n\
         \x20 int i = 0;\n\
         \x20 n.expectStaticType<num>();\n\
         \x20 o.expectStaticType<num>();\n\
         \x20 i.expectStaticType<num>();\n\
         }\n",
    );
    assert_eq!(
        wnta(&report),
        0,
        "extension/unresolved member must not read the receiver arity: {:?}",
        rows(&report)
    );
    // The undefined-member face still owns the spelling.
    assert_eq!(
        code_count(&report, "undefined_method"),
        3,
        "three unresolved sends: {:?}",
        rows(&report)
    );
}

/// (a) An unresolved member name on a typed receiver: `C` has no `foo`,
/// so `c.foo<int>` is the undefined-member face, never an arity verdict.
/// The receiver's `C<T>` declaration is uninvolved.
///
/// Oracle: no wrong_number_of_type_arguments (the member miss owns it).
#[test]
fn missing_member_on_generic_receiver_reports_no_arity() {
    let report = report_ops(
        "class C<T> { C(this.x); T x; }\n\
         void main() {\n\
         \x20 C<int> c = C<int>(0);\n\
         \x20 c.foo<int>();\n\
         }\n",
    );
    assert_eq!(
        wnta(&report),
        0,
        "unresolved member name must not read the receiver arity: {:?}",
        rows(&report)
    );
}

/// (a) `super.foo<int>` where the superclass lacks `foo` — the miss is
/// the undefined-member face, not an arity verdict on `super`'s type.
///
/// Oracle: no wrong_number_of_type_arguments.
#[test]
fn missing_super_member_reports_no_arity() {
    let report = report_ops(
        "class B { void real() {} }\n\
         class D extends B { void m() { super.foo<int>(); } }\n",
    );
    assert_eq!(
        wnta(&report),
        0,
        "super-member miss must not read the super arity: {:?}",
        rows(&report)
    );
}

/// (a) A `dynamic` receiver's member send can never resolve —
/// `i<int>.foo()` is the undefined-member face, never an arity verdict.
///
/// Oracle: no wrong_number_of_type_arguments.
#[test]
fn dynamic_receiver_member_reports_no_arity() {
    let report = report_ops(
        "void main() {\n\
         \x20 dynamic i = 0;\n\
         \x20 i<int>.foo();\n\
         }\n",
    );
    assert_eq!(
        wnta(&report),
        0,
        "dynamic receiver member must not read an arity: {:?}",
        rows(&report)
    );
}

/// (d) A bare `name<T>` head over a non-type declaration is not a type
/// instantiation at all — `var A`, `int v`, `void T()` heads used to
/// answer `Some(0)` from the top-level-decl fallback.
///
/// Oracle: no wrong_number_of_type_arguments (notAType/undefined faces
/// own those spellings).
#[test]
fn non_type_heads_report_no_arity() {
    let report = report_ops(
        "var A = 0;\n\
         int v = 0;\n\
         void T() {}\n\
         void main() {\n\
         \x20 A<int> x = 0;\n\
         \x20 v<int> y = 0;\n\
         \x20 T<int> z;\n\
         \x20 x.abs(); y.abs(); z;\n\
         }\n",
    );
    assert_eq!(
        wnta(&report),
        0,
        "non-type heads must not answer an arity: {:?}",
        rows(&report)
    );
}

/// (b) `A<int>.new<int>` — the unnamed-constructor selector's group:
/// constructors never declare type parameters, so the dedicated
/// `_constructor` code reports at the bare `<int>` group (the census's
/// `test_class_generic_new_typeArgs` shape, oracle `7:13|5`). The `A<int>`
/// head is correct arity (1 given, 1 declared).
///
/// Oracle: one wrong_number_of_type_arguments_constructor.
#[test]
fn unnamed_ctor_type_args_report_ctor_code() {
    let report = report_ops(
        "class A<T> {\n  A.new();\n  A(this.x);\n  T x;\n}\n\n\
         void bar() {\n  A<int>.new<int>;\n}\n",
    );
    assert_eq!(
        wnta_ctor(&report),
        1,
        "A<int>.new<int> is the constructor-selector face: {:?}",
        rows(&report)
    );
    assert_eq!(
        wnta(&report),
        0,
        "the head is correct arity: {:?}",
        rows(&report)
    );
}

/// (b) `A<int>.foo<int>` — a named-constructor selector's group reports
/// `_constructor` at the `<int>` group (upstream
/// `type_instantiation_target.dart` ConstructorElement arm,
/// `.at(typeArgumentList)`); the `A<int>` head is correct arity.
///
/// Oracle: one wrong_number_of_type_arguments_constructor.
#[test]
fn named_ctor_type_args_report_ctor_code() {
    let report = report_ops(
        "class A<T> {\n  A.foo();\n  A(this.x);\n  T x;\n}\n\n\
         void bar() {\n  A<int>.foo<int>;\n}\n",
    );
    assert_eq!(
        wnta_ctor(&report),
        1,
        "A<int>.foo<int> is the constructor-selector face: {:?}",
        rows(&report)
    );
    assert_eq!(
        wnta(&report),
        0,
        "the head is correct arity: {:?}",
        rows(&report)
    );
}

/// (c) The `NamedType` anchor covers the whole `name<args>` —
/// `var c = C<int>()` over `class C` (declared 0) reports
/// `wrong_number_of_type_arguments` anchored at `C<int>` (6 bytes:
/// `C`,`<`,`i`,`n`,`t`,`>`), not the bare group. (The `new C<int>()`
/// keyword spelling's head row latches a stale pre-expression range —
/// a row-model gap upstream of the arity emit, not the anchor rule.)
///
/// Oracle: one wrong_number_of_type_arguments spanning `C<int>`.
#[test]
fn implicit_creation_reports_named_type_anchor() {
    let report = report_ops(
        "class C {}\n\
         void main() { var c = C<int>(); c; }\n",
    );
    let diags: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "wrong_number_of_type_arguments")
        .collect();
    assert_eq!(diags.len(), 1, "one base-code row: {:?}", rows(&report));
    assert_eq!(
        diags[0].range.length,
        6,
        "span is `C<int>`: {:?}",
        rows(&report)
    );
}

/// (c) `A<A>?` — a declared-2 head given 1 arg inside a nullable
/// annotation reports the whole `A<A>?` span (5 bytes: `A`,`<`,`A`,`>`,`?`)
/// — upstream `.at(node)` includes the `?` (`test_class_tooFew`'s `3:1|5`).
///
/// Oracle: one wrong_number_of_type_arguments spanning `A<A>?`.
#[test]
fn nullable_annotation_anchor_includes_question_mark() {
    let report = report_ops_bare(
        "class A<T, U> { A(this.x, this.y); T x; U y; }\n\
         void f(A<A>? a) {}\n",
    );
    let diags: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "wrong_number_of_type_arguments")
        .collect();
    assert_eq!(diags.len(), 1, "one base-code row: {:?}", rows(&report));
    assert_eq!(
        diags[0].range.length,
        5,
        "span is `A<A>?` (name..>..?): {:?}",
        rows(&report)
    );
}

/// The typedef-instantiation face: `Fn<int>` over `typedef Fn<T,U>`
/// reports the base code at the bare `<int>` group (the
/// `TypeInstantiation`'s typeArgumentList, `.at(typeArgumentList)`).
///
/// Oracle: one wrong_number_of_type_arguments at `<int>` (5 bytes).
/// (`Fn<int>?` as an annotation goes through the `NamedType` face and
/// reports the whole `Fn<int>?` span — the expression read below is the
/// `TypeInstantiation` face.)
#[test]
fn typedef_instantiation_reports_at_group() {
    let report = report_ops(
        "typedef Fn<T, U> = void Function(T, U);\n\
         void main() { var g = Fn<int>; g; }\n",
    );
    let diags: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "wrong_number_of_type_arguments")
        .collect();
    assert_eq!(diags.len(), 1, "one base-code row: {:?}", rows(&report));
    assert_eq!(
        diags[0].range.length,
        5,
        "span is the `<int>` group: {:?}",
        rows(&report)
    );
}

/// The typedef-constructor-reference face: `MyList<int>.filled` over
/// `typedef MyList = List` reports the `NamedType`'s `MyList<int>` span
/// (a `.` after the group makes it a ConstructorReference, `.at(node)`).
/// `MyList` declares 0 params over `List<E>`'s 1.
///
/// Oracle: one wrong_number_of_type_arguments spanning `MyList<int>` (11
/// bytes).
#[test]
fn typedef_ctor_ref_reports_named_type_anchor() {
    let report = report_ops(
        "typedef MyList = List;\n\
         void main() { var x = MyList<int>.filled; x; }\n",
    );
    let diags: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "wrong_number_of_type_arguments")
        .collect();
    assert_eq!(diags.len(), 1, "one base-code row: {:?}", rows(&report));
    assert_eq!(
        diags[0].range.length,
        11,
        "span is `MyList<int>`: {:?}",
        rows(&report)
    );
}

/// `Never<int>` — the builtin bottom type is a type-defining element of
/// arity 0, so a written group reports the base code.
///
/// Oracle: one wrong_number_of_type_arguments.
#[test]
fn never_type_args_report() {
    let report = report_ops("void f(Never<int> x) {}\n");
    assert_eq!(
        wnta(&report),
        1,
        "Never is a 0-arity type head: {:?}",
        rows(&report)
    );
}

/// Negative control — a correctly-sized group stays silent: `C<int>`
/// over `class C<T>` declares 1, given 1; `List<int>` over `List<E>`
/// likewise.
///
/// Oracle: silent.
#[test]
fn correct_arity_stays_silent() {
    let report = report_ops(
        "class C<T> { C(this.x); T x; }\n\
         void main() {\n\
         \x20 C<int> c = C<int>(0);\n\
         \x20 List<int> l = List<int>();\n\
         \x20 c.x; l.length;\n\
         }\n",
    );
    assert_silent(&report);
}

/// Negative control — `code_names` keeps the failure legible: the
/// `Never<int>` fixture reports only `wrong_number_of_type_arguments`.
#[test]
fn only_the_base_code_reports() {
    let report = report_ops("void f(Never<int> x) {}\n");
    assert_eq!(code_names(&report), ["wrong_number_of_type_arguments"]);
}
