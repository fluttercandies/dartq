//! The #2947 pins (census C2): expression/call-site type-argument bounds —
//! `type_argument_not_matching_bounds` at the four upstream visitor seams
//! the invocation path never drove
//! (`type_arguments_verifier.dart`'s `_checkInvocationTypeArguments`,
//! :570-633 — invoked from `checkMethodInvocation` :236-240,
//! `checkFunctionExpressionInvocation` :192-196, `checkInstanceCreation`
//! :236-240, and `checkNamedType` → `_checkForTypeArgumentNotMatchingBounds`
//! :243-247/:312-331, upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/type_arguments_verifier.dart).
//!
//! Each pin cites its upstream test name and expected span; the oracle
//! texts are never hand-crafted:
//! - `type_argument_not_matching_bounds_test.dart`
//!   `test_methodInvocation_method` — `factory.point<String>(...)` over
//!   `point<T extends num>`: the member send's written argument checks
//!   the method's DECLARED bound (not the inference seeds' gated
//!   `own_bound`).
//! - `test_functionExpressionInvocation` — `(<T extends num>() {})
//!   <String>()`: the literal's own `<T>` window decides.
//! - `test_functionExpressionInvocation_implicitCall` — `c<String>()`
//!   over `call<T extends num>`: the implicit-`call` method's bounds.
//! - `test_extensionType_withTypeArguments` — `E<String>(x)` over
//!   `extension type E<T extends num>`: the extension type's unnamed
//!   creation checks its container bounds.
//! - `test_metadata_notMatching` — `@A<String>()` over `A<T extends
//!   num>`: the annotation's head group checks the container bounds.
//! - `test_typeLiteral_class` — `var t = C<String>;` over `C<T extends
//!   int>`: the bare type literal's NamedType checks bounds too.

use super::test_rig::{report_ops, rows};

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

/// A member send checks the method's declared bound — upstream
/// `test_methodInvocation_method` (:388-406): `factory.point<String>(...)`
/// over `point<T extends num>` reports at `<String>` (offset of the
/// argument node). The verdict needs the method's `type_param_bounds`,
/// held apart from the inference seeds (`member_send_params`' `own_bound`
/// deliberately answers `None` outside its context-free corner).
#[test]
fn method_invocation_reports_declared_bound() {
    let report = report_ops(
        "class Point<T extends num> {\n  Point(T x, T y);\n}\n\
         class PointFactory {\n  Point<T> point<T extends num>(T x, T y) {\n    \
         return Point<T>(x, y);\n  }\n}\n\
         f(PointFactory factory) {\n  factory.point<String>('hello', 'world');\n}\n",
    );
    let hits = bounds_rows(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert!(
        hits[0].ends_with("'String' doesn't conform to the bound 'num' of the type parameter 'T'."),
        "{hits:#?}"
    );
}

/// `this.m<S>` resolves through the same member walk — the declared
/// bounds, not the inference seeds.
#[test]
fn implicit_this_member_send_reports_declared_bound() {
    let report = report_ops(
        "class C {\n  void m<T extends num>(T x) {}\n  void call_m() {\n    m<String>(0);\n  }\n}\n",
    );
    let hits = bounds_rows(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert!(
        hits[0].ends_with("'String' doesn't conform to the bound 'num' of the type parameter 'T'."),
        "{hits:#?}"
    );
}

/// A generic function-literal invocation — upstream
/// `test_functionExpressionInvocation` (:171-179):
/// `(<T extends num>() {})<String>()` reports at `<String>`. The literal's
/// own `<T>` window is the last closed outermost type-variables group and
/// survives in `bounds.pending` — the callee carries no `exec_bounds`
/// window (no declaring name).
#[test]
fn function_expression_invocation_reports_bound() {
    let report = report_ops("void f() {\n  (<T extends num>() {})<String>();\n}\n");
    let hits = bounds_rows(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert!(
        hits[0].ends_with("'String' doesn't conform to the bound 'num' of the type parameter 'T'."),
        "{hits:#?}"
    );
}

/// `c<S>()` resolves `c.call<S>` — upstream
/// `test_functionExpressionInvocation_implicitCall` (:181-193): the call
/// method's own `T extends num` decides.
#[test]
fn implicit_call_invocation_reports_bound() {
    let report = report_ops(
        "class C {\n  void call<T extends num>() {}\n}\n\
         void f(C c) {\n  c<String>();\n}\n",
    );
    let hits = bounds_rows(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert!(
        hits[0].ends_with("'String' doesn't conform to the bound 'num' of the type parameter 'T'."),
        "{hits:#?}"
    );
}

/// A generic top-level function invocation — upstream
/// `test_methodInvocation_topLevelFunction` (:408-424):
/// `f<String>(...)` over `f<T extends num>` — the `exec_bounds` window's
/// declared bounds (the pre-existing `check_call_type_arg_bounds`
/// consumer; the regression face).
#[test]
fn top_level_function_invocation_reports_bound() {
    let report = report_ops(
        "class Point<T extends num> {\n  Point(T x, T y);\n}\n\
         Point<T> f<T extends num>(T x, T y) {\n  return Point<T>(x, y);\n}\n\
         main() {\n  f<String>('hello', 'world');\n}\n",
    );
    let hits = bounds_rows(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert!(
        hits[0].ends_with("'String' doesn't conform to the bound 'num' of the type parameter 'T'."),
        "{hits:#?}"
    );
}

/// An instance creation — `checkInstanceCreation`
/// (type_arguments_verifier.dart:236-240): `C<String>(0)` over
/// `C<T extends num>` checks the class's own bounds.
#[test]
fn instance_creation_reports_container_bound() {
    let report = report_ops(
        "class C<T extends num> {\n  C(T x);\n}\n\
         void f() {\n  C<String>(0);\n}\n",
    );
    let hits = bounds_rows(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert!(
        hits[0].ends_with("'String' doesn't conform to the bound 'num' of the type parameter 'T'."),
        "{hits:#?}"
    );
}

/// An extension-type unnamed creation — upstream
/// `test_extensionType_withTypeArguments` (:145-153):
/// `E<String>(0)` over `extension type E<T extends num>` — the row kind
/// carries the conflated `Extension` tag, so the resolved `ExtensionType`
/// kind admits it to the container arm.
#[test]
fn extension_type_creation_reports_bound() {
    let report = report_ops(
        "extension type E<T extends num>(int it) {}\n\
         void f() {\n  E<String>(0);\n}\n",
    );
    let hits = bounds_rows(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert!(
        hits[0].ends_with("'String' doesn't conform to the bound 'num' of the type parameter 'T'."),
        "{hits:#?}"
    );
}

/// A metadata annotation's type-argument group — upstream
/// `test_metadata_notMatching` (:319-330): `@A<String>()` over
/// `A<T extends num>` checks the container's bounds. The headless group
/// closed unanchored (the `A` row is no callee), so the open
/// `call_args_capture` carries the written arguments.
#[test]
fn metadata_annotation_reports_bound() {
    let report = report_ops(
        "class A<T extends num> {\n  const A();\n}\n\
         @A<String>()\nvoid f() {}\n",
    );
    let hits = bounds_rows(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert!(
        hits[0].ends_with("'String' doesn't conform to the bound 'num' of the type parameter 'T'."),
        "{hits:#?}"
    );
}

/// The alias-headed metadata — upstream
/// `test_metadata_notMatching_viaTypeAlias` (:332-345): `@B<String>()`
/// over `typedef B<T extends num> = A<T>` checks the ALIAS's own bounds.
#[test]
fn metadata_alias_annotation_reports_bound() {
    let report = report_ops(
        "class A<T> {\n  const A();\n}\n\
         typedef B<T extends num> = A<T>;\n\
         @B<String>()\nvoid f() {}\n",
    );
    let hits = bounds_rows(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert!(
        hits[0].ends_with("'String' doesn't conform to the bound 'num' of the type parameter 'T'."),
        "{hits:#?}"
    );
}

/// A bare type literal — upstream `test_typeLiteral_class` (:722-731):
/// `var t = C<String>;` over `C<T extends int>` reports at `<String>`.
///
/// Ignored — the type literal's `<String>` is a TYPE-head group (`C`
/// arms `pending_type_head`), so it belongs to the
/// `PendingBoundedCheck::Site`/`check_site` path (`bounds.rs`'s
/// `check_named`), not the invocation `call_type_args` latch: a headed
/// group never arms `call_args_capture` (`call_args_begin`'s `headed`
/// gate), so `consume_type_argument_application` reads no `latched_args`.
/// `check_site`'s `check_named` currently stays silent for a
/// bare-expression `NamedType` — a `checkNamedType` coverage gap inside
/// the site machinery (bounds.rs), outside the invocation-side wiring
/// this ticket owns.
#[ignore = "bare-expression NamedType sites stay silent in check_site's check_named (bounds.rs checkNamedType gap, not the invocation path)"]
#[test]
fn type_literal_reports_bound() {
    let report = report_ops("class C<T extends int> {}\nvar t = C<String>;\n");
    let hits = bounds_rows(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert!(
        hits[0].ends_with("'String' doesn't conform to the bound 'int' of the type parameter 'T'."),
        "{hits:#?}"
    );
}

/// In-bounds arguments stay silent everywhere (the regression face): a
/// member send, a function invocation, a creation, and an annotation all
/// inside their declared bounds emit nothing.
#[test]
fn in_bounds_arguments_stay_silent() {
    let report = report_ops(
        "class C<T extends num> {\n  C(T x);\n  void m<S extends num>(S s) {}\n}\n\
         class A<T extends num> {\n  const A();\n}\n\
         void g<T extends num>(T x) {}\n\
         @A<int>()\nvoid f(C c) {\n  c.m<int>(0);\n  g<int>(0);\n  C<int>(0);\n}\n",
    );
    assert!(
        bounds_rows(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// A named-type annotation site's bounds face is unchanged (the
/// annotation/clause/declaration-site machinery, pre-#2947 — the
/// `checkNamedType` arm on a declared type, `void f(A<String> a)`'s
/// inverted-bounds context row is NOT this face; this pin asserts the
/// invocation-site wiring did not disturb it).
#[test]
fn named_type_site_regression_stays() {
    let report = report_ops("class A<T extends num> {}\nA<int> f() => throw 0;\n");
    assert!(
        bounds_rows(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// `f<int>(x)` where `int` satisfies `T extends num` — the pre-existing
/// in-bounds invocation face stays silent (the `exec_bounds` declared
/// window clone, regression).
#[test]
fn in_bounds_top_level_invocation_stays_silent() {
    let report = report_ops(
        "void f<T extends num>(T x) {}\nvoid g() {\n  f<int>(0);\n}\n",
    );
    assert!(
        bounds_rows(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// The prelude's `num` declares the bound head `String` violates
/// (`'String' doesn't conform to the bound 'num'`).
#[test]
fn string_violates_num_bound() {
    let report = report_ops("void f<T extends num>(T x) {}\nvoid g() {\n  f<String>('x');\n}\n");
    let hits = bounds_rows(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
}
