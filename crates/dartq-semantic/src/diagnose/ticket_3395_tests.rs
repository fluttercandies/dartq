//! #3395 pins — L-D-1, the `unchecked_use_of_nullable_value` EXTRA-row
//! cluster the recon's extension-applicability face owns.
//!
//! # The defect
//!
//! Upstream reads a nullable dereference only AFTER the access resolved:
//! `type_property_resolver` consults the applicable extensions against the
//! receiver's static type first, and an extension whose instantiated `on`
//! type accepts the receiver's nullable type resolves the member without
//! dereferencing anything (`applicable_extensions.dart`'s `applicableTo`,
//! `extension_member_resolver.dart`'s `_getApplicable`), so
//! `nullable_dereference_verifier` never sees the node. The `UNCHECKED`
//! arms that emit the shared code, however, split on the receiver's
//! nullability alone when the arm decides before the application is known
//! — the extension-body `this` operator face and the composite-receiver
//! operator face both reported an access a nullable-target extension had
//! already resolved.
//!
//! # The wiring
//!
//! [`extension_application`] answers the applicability verdict for every
//! in-scope face (the `on Object?` / `on T?` / `on T` own-parameter arms
//! included); the emit gates now ask it before reporting, exactly like the
//! member faces and the compound/prefix/postfix face already did.
//!
//! [`extension_application`]: crate::diagnose::row_resolution

use crate::diagnose::test_rig::{assert_silent, code_names, report_ops};

/// An `on Object?` extension defines the member: the nullable receiver
/// resolves the access through the extension, so nothing reports.
#[test]
fn t3395_on_object_nullable_member_silent() {
    let report = report_ops(
        "extension E on Object? { int m() => 1; } \
         void f(C? c) { c.m(); } \
         class C {}",
    );
    assert_silent(&report);
}

/// The `on T` unbounded own-parameter twin: the parameter instantiates to
/// the nullable receiver whole (`applicableTo`'s `constrainArgument`), so
/// the application is safe and the access is checked.
#[test]
fn t3395_on_own_param_nullable_receiver_silent() {
    let report = report_ops(
        "extension E<T> on T { int m() => 1; } \
         void f(C? c) { c.m(); } \
         class C {}",
    );
    assert_silent(&report);
}

/// `package:expect`'s `expectStaticType` shape spelled locally: a generic
/// own-parameter extension invoked on a nullable receiver stays silent.
#[test]
fn t3395_expect_static_type_shape_silent() {
    let report = report_ops(
        "extension StaticTypeHelper<T> on T { void expectStaticType<U>() {} } \
         void f(int? i) { i.expectStaticType<int>(); }",
    );
    assert_silent(&report);
}

/// Negative control: no applicable extension exists, so the nullable
/// member access still reports the unchecked code.
#[test]
fn t3395_plain_nullable_member_reports() {
    let report = report_ops(
        "class C { int m() => 1; } \
         void f(C? c) { c.m(); }",
    );
    assert!(
        code_names(&report).contains(&"unchecked_use_of_nullable_value"),
        "expected the unchecked row, got {:?}",
        code_names(&report)
    );
}

/// Negative control: a NON-nullable `on` type never accepts a nullable
/// receiver (`applicableTo` rejects `C?` against `on Object`), so the
/// access keeps its unchecked row.
#[test]
fn t3395_on_object_non_nullable_reports() {
    let report = report_ops(
        "extension E on Object { int m() => 1; } \
         void f(C? c) { c.m(); } \
         class C {}",
    );
    assert!(
        code_names(&report).contains(&"unchecked_use_of_nullable_value"),
        "expected the unchecked row, got {:?}",
        code_names(&report)
    );
}

/// The extension-body `this` operator face: `this[0]` inside
/// `extension E on A?` resolves E's own `[]` (the extension applies to the
/// written `on` type — `this` exactly), so the nullable `this` is never
/// dereferenced.
#[test]
fn t3395_extension_this_operator_silent() {
    let report = report_ops(
        "class A {} \
         extension E on A? { \
           A? operator [](int i) => this; \
           A? test() => this[0]; \
         }",
    );
    assert_silent(&report);
}

/// The composite-receiver operator face: `(a ?? b)` re-derives `C?`, and
/// the `on C?` extension declares `+` — the resolved application stands
/// the dereference row down.
#[test]
fn t3395_composite_operator_on_nullable_extension_silent() {
    let report = report_ops(
        "class C {} \
         extension E on C? { C operator +(C o) => o; } \
         C? g(C? a, C? b) => (a ?? b) + a;",
    );
    assert_silent(&report);
}
