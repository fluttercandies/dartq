//! The #3659 pins — two same-file families in `invocation.rs`.
//!
//! # Family 7 — `redirect_to_non_const_constructor` (104 rows)
//!
//! The emit gate read `redirect_target_is_const`, whose
//! `ctor_decl_site`/`ctor_face_exists` chain only sees the named
//! `Constructor` declaration sites: an unnamed `factory C()` leaves a
//! member site plus the `unnamed_factory_heads` list instead, a
//! class/enum/extension-type primary constructor declares no site at all,
//! and a `= C<T>.name` target reached the edge with the selector folded
//! away (`edge.target == edge.target_container`), so the named-ctor
//! answer never applied. The gate now asks the declaration-kind table
//! first — the collector's per-constructor `AugKind::Constructor` rows
//! carry `is_const` for every shape — and recovers the written selector
//! off the token stream when a generic group intervened, so the emit
//! also anchors at the whole `C<T>.name` extent like the oracle's
//! `factoryRedirectionTarget` entity (upstream Dart SDK source,
//! pkg/analyzer/lib/src/generated/error_verifier.dart:6926-6943).
//!
//! # Family 3 — `invocation_of_non_function_expression` (106 rows)
//!
//! The non-function verdict now asks the full callable question the
//! oracle's resolver poses (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/resolver/
//! function_expression_invocation_resolver.dart:66-146): the `Function`
//! supertype, an aliased function signature, a resolved `call` method, an
//! accessible extension's `call`, or a callable parameter bound — shared
//! between the `Value`, bare, and `this`/`super` arms. `this` inherits
//! exactly like `super` (the superclass-spine climb used to run for
//! `super` alone). A nullable callee answers the type-property resolver's
//! nullable arm instead — `unchecked_use_of_nullable_value`'s function
//! variant (type_property_resolver.dart:103-185) — and only an
//! `on T?`-applicable extension `call` rescues it. Emitted ranges re-wrap
//! a balanced parenthesized callee: the report entity is the `function`
//! node, parens included.
//!
//! Oracle provenance: `dart analyze` 3.13.7 over the census fixture
//! matrix — the `factory_long_test`/`redirection_test` redirect faces and
//! the `call_member`/`null_check_operator`/`static_errors` invocation
//! faces.

use super::test_rig::{assert_silent, report_ops, rows};

fn code_count(report: &crate::diagnose::FileReport, code: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .count()
}

fn non_const_redirects(report: &crate::diagnose::FileReport) -> usize {
    code_count(report, "redirect_to_non_const_constructor")
}

fn non_function_invocations(report: &crate::diagnose::FileReport) -> usize {
    code_count(report, "invocation_of_non_function_expression")
}

// —— family 7: the declaration-kind table answers the target's const ——

/// `= A` inside an extension type targets the primary constructor: its
/// `const` keyword lives on the declaration-kind row alone (a primary
/// declares no `Constructor` site), so the const redirect stays silent.
#[test]
fn const_factory_redirect_to_const_extension_type_primary_stays_silent() {
    let report = report_ops(
        "extension type const A(int it) {\n\
         \x20 const factory A.named(int it) = A;\n\
         }\n",
    );
    assert_eq!(
        non_const_redirects(&report),
        0,
        "a const primary target is const: {:?}",
        rows(&report)
    );
}

/// The generative `this(x)` redirect inside an extension type answers the
/// same const-primary face through the edge's own selector.
#[test]
fn const_generative_this_redirect_to_const_extension_type_primary_stays_silent() {
    let report = report_ops(
        "extension type const E(int it) {\n\
         \x20 const E.named(int x) : this(x);\n\
         }\n",
    );
    assert_eq!(
        non_const_redirects(&report),
        0,
        "the const primary target is const: {:?}",
        rows(&report)
    );
}

/// `const factory C() = C<T>` redirects to its own unnamed const factory:
/// the unnamed-factory const latch lives on the declaration-kind row
/// (the member site the generative scan keeps never carries `const_ctor`),
/// so the redirect stays silent — only the recursive-redirect row stands.
#[test]
fn const_factory_redirect_to_own_unnamed_factory_stays_silent() {
    let report = report_ops(
        "class C<T> {\n\
         \x20 const factory C(T x) = C<T>;\n\
         }\n",
    );
    assert_eq!(
        non_const_redirects(&report),
        0,
        "the unnamed const factory target is const: {:?}",
        rows(&report)
    );
}

/// `= A<T>.constant` targets a named const generative constructor through
/// the recovered `.constant` selector — silent.
#[test]
fn const_factory_redirect_to_named_const_generative_stays_silent() {
    let report = report_ops(
        "class A<T> {\n\
         \x20 const A.constant(T x);\n\
         }\n\
         class B<T> {\n\
         \x20 const factory B.make(T x) = A<T>.constant;\n\
         }\n",
    );
    assert_eq!(
        non_const_redirects(&report),
        0,
        "a named const generative target is const: {:?}",
        rows(&report)
    );
}

/// `= B<T>.A` targets a non-const generative constructor: the const
/// factory reports, and the row anchors at the whole `B<T>.A` extent —
/// the oracle's `factoryRedirectionTarget` entity spans the selector
/// through the generic group.
#[test]
fn const_factory_redirect_to_named_nonconst_generative_reports_at_full_extent() {
    let source = "class B<T> {\n\
                  \x20 B(T x);\n\
                  \x20 B.A(T x) : this(x);\n\
                  }\n\
                  class C<T> {\n\
                  \x20 const factory C.test(T x) = B<T>.A;\n\
                  }\n";
    let report = report_ops(source);
    let redirects: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "redirect_to_non_const_constructor")
        .collect();
    assert_eq!(redirects.len(), 1, "{:?}", rows(&report));
    // The fixture rides the one-line core prelude: the row's offset is
    // the prelude's byte length plus `B<T>.A`'s position in the fixture.
    let expected = (super::test_rig::CORE_PRELUDE.len() + source.find("B<T>.A").unwrap()) as u32;
    assert_eq!(
        redirects[0].range.offset,
        expected,
        "the emit lands on `B`: {:?}",
        rows(&report)
    );
    assert_eq!(
        redirects[0].range.length,
        6,
        "the row anchors the `B<T>.A` extent: {:?}",
        rows(&report)
    );
}

/// An enum's generative constructors are implicitly const upstream — a
/// const factory redirecting to one stays silent through the enum arm.
#[test]
fn const_factory_redirect_to_enum_generative_stays_silent() {
    let report = report_ops(
        "enum E {\n\
         \x20 v(0);\n\
         \x20 const E._(int x);\n\
         \x20 const factory E.f(int x) = E._;\n\
         }\n",
    );
    assert_eq!(
        non_const_redirects(&report),
        0,
        "an enum's const generative target is const: {:?}",
        rows(&report)
    );
}

/// `= Y<int>` targets an unnamed const factory declared on a generic
/// class — the declaration-kind row carries its `is_const` where no
/// `Constructor` site exists, so the redirect stays silent.
#[test]
fn const_factory_redirect_to_unnamed_const_factory_stays_silent() {
    let report = report_ops(
        "class Y<T> {\n\
         \x20 const factory Y(T x) = Y<T>;\n\
         \x20 const Y._(T x);\n\
         }\n\
         class X {\n\
         \x20 const factory X() = Y<int>;\n\
         }\n",
    );
    assert_eq!(
        non_const_redirects(&report),
        0,
        "the unnamed const factory target is const: {:?}",
        rows(&report)
    );
}

/// Negative control — a non-const target still reports: `const factory
/// F() = N` where `N` declares a plain generative constructor.
#[test]
fn const_factory_redirect_to_nonconst_target_still_reports() {
    let report = report_ops(
        "class N {\n\
         \x20 N();\n\
         }\n\
         class F {\n\
         \x20 const factory F() = N;\n\
         }\n",
    );
    assert_eq!(
        non_const_redirects(&report),
        1,
        "a non-const target still reports: {:?}",
        rows(&report)
    );
}

/// Negative control — the recovered selector must not invent constness:
/// `= B<T>.A` against a non-const named target reports (the emit's row
/// count, asserted without the extent pin above).
#[test]
fn const_factory_redirect_to_named_nonconst_still_reports_once() {
    let report = report_ops(
        "class B {\n\
         \x20 B();\n\
         \x20 B.A() : this();\n\
         }\n\
         class C {\n\
         \x20 const factory C.test() = B.A;\n\
         }\n",
    );
    assert_eq!(
        non_const_redirects(&report),
        1,
        "a non-const named target still reports: {:?}",
        rows(&report)
    );
}

// —— family 3: the callable verdict asks the resolver's whole question ——

/// `1(2)` invokes the literal through an applicable extension's `call`
/// method — the oracle's type-property resolve consults the accessible
/// set (`extension Tricky on int { call }`).
#[test]
fn literal_send_through_extension_call_stays_silent() {
    let report = report_ops(
        "extension Tricky on int {\n\
         \x20 List<int> call(int to) => [0];\n\
         }\n\
         void f() {\n\
         \x20 1(2);\n\
         }\n",
    );
    assert_eq!(
        non_function_invocations(&report),
        0,
        "an applicable extension `call` answers the literal send: {:?}",
        rows(&report)
    );
}

/// `this()` inside `extension on Function` types as `Function` — the
/// `resolveToBound` FunctionType face is callable outright.
#[test]
fn this_send_inside_function_extension_stays_silent() {
    let report = report_ops(
        "extension on Function {\n\
         \x20 int call() => 0;\n\
         \x20 int m() => this();\n\
         }\n",
    );
    assert_eq!(
        non_function_invocations(&report),
        0,
        "a `Function`-typed `this` is callable: {:?}",
        rows(&report)
    );
}

/// `this(1)` inside `class C extends A` resolves `call` up the
/// superclass spine — `this` inherits exactly like `super`.
#[test]
fn this_send_through_inherited_call_stays_silent() {
    let report = report_ops(
        "class A {\n\
         \x20 int call(int x) => x;\n\
         }\n\
         class C extends A {\n\
         \x20 void m() {\n\
         \x20 this(1);\n\
         \x20 }\n\
         }\n",
    );
    assert_eq!(
        non_function_invocations(&report),
        0,
        "an inherited `call` answers `this(...)`: {:?}",
        rows(&report)
    );
}

/// `super(42)` inside `C extends A` resolves through an extension `call`
/// applicable to `A` — the type-property resolve consults the accessible
/// set at every spine hop.
#[test]
fn super_send_through_extension_call_stays_silent() {
    let report = report_ops(
        "class A {}\n\
         extension on A {\n\
         \x20 int call(int x) => x;\n\
         }\n\
         class C extends A {\n\
         \x20 void m() {\n\
         \x20 super(42);\n\
         \x20 }\n\
         }\n",
    );
    assert_eq!(
        non_function_invocations(&report),
        0,
        "an extension `call` on the superclass answers `super(...)`: {:?}",
        rows(&report)
    );
}

/// `this(2)` inside an extension whose own `call` declares the member —
/// the enclosing extension applies to its own `on` type.
#[test]
fn this_send_inside_own_call_extension_stays_silent() {
    let report = report_ops(
        "class C {}\n\
         extension E on C {\n\
         \x20 int call(int x) => x;\n\
         \x20 int m() => this(2);\n\
         }\n",
    );
    assert_eq!(
        non_function_invocations(&report),
        0,
        "the enclosing extension's `call` answers `this(...)`: {:?}",
        rows(&report)
    );
}

/// A nullable callee whose type carries no nullable-applicable `call`
/// reports `unchecked_use_of_nullable_value`'s function variant — the
/// type-property resolver's nullable arm — never the non-function
/// verdict.
#[test]
fn nullable_field_send_reports_unchecked_invocation() {
    let report = report_ops(
        "class C {\n\
         \x20 int? f;\n\
         \x20 void m() {\n\
         \x20 f(1);\n\
         \x20 }\n\
         }\n",
    );
    assert_eq!(
        code_count(&report, "unchecked_use_of_nullable_value"),
        1,
        "the nullable arm reports its function variant: {:?}",
        rows(&report)
    );
    assert_eq!(
        non_function_invocations(&report),
        0,
        "the non-function verdict never fires for a nullable callee: {:?}",
        rows(&report)
    );
}

/// The nullable-applicable extension rescues the same send: `extension
/// on int?` declares `call`, so `int?`-typed invocation stays silent.
#[test]
fn nullable_send_through_nullable_extension_call_stays_silent() {
    let report = report_ops(
        "extension on int? {\n\
         \x20 int call(int x) => x;\n\
         }\n\
         class C {\n\
         \x20 int? f;\n\
         \x20 void m() {\n\
         \x20 f(1);\n\
         \x20 }\n\
         }\n",
    );
    assert_eq!(
        non_function_invocations(&report),
        0,
        "an `on int?` `call` answers the nullable send: {:?}",
        rows(&report)
    );
    assert_eq!(
        code_count(&report, "unchecked_use_of_nullable_value"),
        0,
        "no unchecked row for a rescued send: {:?}",
        rows(&report)
    );
}

/// A parenthesized callee reports at the `function` node's extent — the
/// parens ride the span: `(1)` is three columns, not the literal's one. A
/// literal callee keeps the row in the `Value` arm (bare rows defer to
/// the pass-2 face, whose span is the inner token's — a row_resolution
/// gap outside this ticket's scope).
#[test]
fn parenthesized_nonfunction_send_reports_at_wrapped_extent() {
    let report = report_ops("void f() {\n  (1)(2);\n}\n");
    let found: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invocation_of_non_function_expression")
        .collect();
    assert_eq!(found.len(), 1, "{:?}", rows(&report));
    assert_eq!(
        found[0].range.length,
        3,
        "the row spans `(1)` — parens included: {:?}",
        rows(&report)
    );
}

/// Negative control — a plain non-function send still reports: `i()` on
/// an `int` local with no applicable `call` anywhere.
#[test]
fn plain_int_send_still_reports() {
    let report = report_ops(
        "void f() {\n\
         \x20 var i = 0;\n\
         \x20 i();\n\
         }\n",
    );
    assert_eq!(
        non_function_invocations(&report),
        1,
        "a non-function callee still reports: {:?}",
        rows(&report)
    );
}

/// Negative control — a declared `int` field invoked bare reports the
/// non-function verdict (no `call` member, no extension).
#[test]
fn int_field_send_still_reports() {
    let report = report_ops(
        "class C {\n\
         \x20 int f = 0;\n\
         \x20 void m() {\n\
         \x20 f(1);\n\
         \x20 }\n\
         }\n",
    );
    assert_eq!(
        non_function_invocations(&report),
        1,
        "a non-function field send still reports: {:?}",
        rows(&report)
    );
}

/// The `Function` supertype callee stays silent — `Function f; f();`
/// discards through the callable face.
#[test]
fn function_typed_send_stays_silent() {
    let report = report_ops("void f(Function g) {\n  g();\n}\n");
    assert_eq!(
        non_function_invocations(&report),
        0,
        "a `Function`-typed callee is callable: {:?}",
        rows(&report)
    );
}

/// A whole-file silence sweep over the covered shapes keeps the pins
/// honest about what the fixtures emit alongside the family rows.
#[test]
fn covered_invocation_shapes_leave_no_family_residue() {
    let report = report_ops(
        "extension Tricky on int {\n\
         \x20 List<int> call(int to) => [0];\n\
         }\n\
         void f() {\n\
         \x20 1(2);\n\
         }\n",
    );
    assert_silent(&report);
}
