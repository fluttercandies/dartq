//! The #3425 pins — two model-layer gaps measured against the 3.13.7
//! oracle:
//!
//! —— T-E: a primary-constructor `var`/`final` formal declares an
//! instance field for EVERY container kind the head parses under
//! (element_builder.dart:2089-2099 gates only on `finalOrVarKeyword !=
//! null`), and the oracle's element model keeps it for mixin and
//! extension primaries too — the member-scope claim, the
//! `with`-application member walk, and the static/instance conflict face
//! all read it. An extension type keeps its own arm: the representation
//! formal is a field under every keyword shape and is always `final`.
//!
//! —— 4a: an implicitly-typed instance field's declared slot is
//! `hasImplicitType`, so the instance-member inferrer combines the
//! overridden getter/setter signatures and never reads the initializer
//! (instance_member_inferrer.dart:104-135). Baking the collector's
//! byte-scan initializer face into `var_ty` made `var foo = 0` and
//! `int foo = 0` indistinguishable — the implicit slot closed and the
//! declared side of the override check compared a type the oracle never
//! computed.

use super::test_rig::{assert_silent, code_names, report_ops, rows};

// —— Gap 1 (T-E): the declaring field exists for every primary-ctor
// container kind ——

/// A `var` declaring formal in a class primary constructor induces a
/// mutable instance field whose induced setter claims the member scope:
/// a body field of the same name collides at its own token.
#[test]
fn class_declaring_formal_field_claims_the_member_scope() {
    let report = report_ops("class B(var num foo) {\n  int foo = 0;\n}\n");
    assert!(
        code_names(&report).contains(&"duplicate_definition"),
        "{:?}",
        rows(&report)
    );
}

/// The same fixture under `mixin`: upstream still builds the field
/// (`finalOrVarKeyword != null` is the whole gate), so the body
/// re-declaration collides exactly like the class shape.
#[test]
fn mixin_declaring_formal_field_claims_the_member_scope() {
    let report = report_ops("mixin M(var num foo) {\n  int foo = 0;\n}\n");
    let names = code_names(&report);
    assert!(names.contains(&"mixin_primary_constructor"), "{names:?}");
    assert!(
        names.contains(&"duplicate_definition"),
        "the induced field must claim the scope: {names:?}"
    );
}

/// And under `extension` — the same element-model field exists upstream
/// even though the head itself is a parse error.
#[test]
fn extension_declaring_formal_field_claims_the_member_scope() {
    let report = report_ops("extension E(var num foo) on int {\n  int foo = 0;\n}\n");
    let names = code_names(&report);
    assert!(
        names.contains(&"duplicate_definition"),
        "the induced field must claim the scope: {names:?}"
    );
}

/// The `with`-application member walk sees the mixin's declaring field:
/// `class C extends S with M` checks `M`'s members against `S`, and
/// `num foo` does not validly override `S`'s `int foo` getter.
#[test]
fn mixin_declaring_field_joins_the_with_application_member_walk() {
    let report = report_ops(
        "class S { int foo = 0; }\n\
         mixin M(var num foo) {}\n\
         class C extends S with M {}\n",
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"invalid_override"),
        "the mixin's declaring field must join the member walk: {names:?}"
    );
}

/// `class B(var num foo);` — the declaring formal's induced field is
/// initialized by the primary constructor itself
/// (`updateWithParameters` marks it `initInFieldFormal`), so the oracle
/// is silent.
#[test]
fn bare_declaring_formal_is_silent() {
    let report = report_ops("class B(var num foo);\n");
    assert_silent(&report);
}

/// A formal WITHOUT `var`/`final` declares no field — `int foo` is a
/// plain parameter and nothing reports.
#[test]
fn non_declaring_formal_stays_a_plain_parameter() {
    let report = report_ops("class B(int foo);\n");
    assert_silent(&report);
}

/// The extension-type arm is untouched: the representation formal stays
/// the container's only induced field and keeps its `final` face.
#[test]
fn extension_type_representation_is_unchanged() {
    let report = report_ops("extension type E(int x) {}\n");
    assert_silent(&report);
}

// —— Gap 2 (4a): the implicit-type slot survives collection ——

/// An implicitly-typed field overriding a getter infers its type from
/// the combined superinterface signature, never from the initializer —
/// `var foo = "s"` under `int get foo` is `int`, silently compatible.
#[test]
fn implicit_field_infers_from_the_overridden_getter_not_the_initializer() {
    let report = report_ops(
        "class A { int get foo => 0; }\n\
         class B extends A { var foo = \"s\"; }\n",
    );
    assert_silent(&report);
}

/// The same shape through the overridden FIELD's induced accessors: the
/// inferrer combines `A.foo`'s `int` getter, so the `String` initializer
/// never enters the declared face.
#[test]
fn implicit_field_infers_from_the_overridden_field_getter() {
    let report = report_ops(
        "class A { int foo = 0; }\n\
         class B extends A { var foo = \"s\"; }\n",
    );
    assert_silent(&report);
}

/// A `late` instance field's implicit slot fires the same inference.
#[test]
fn late_implicit_field_infers_from_the_overridden_getter() {
    let report = report_ops(
        "class A { int get foo => 0; }\n\
         class B extends A { late var foo = \"s\"; }\n",
    );
    assert_silent(&report);
}

/// The explicitly-typed twin is unchanged: `String foo = "s"` under
/// `int get foo` reports the declared-type mismatch — the written
/// annotation leaves no implicit slot to infer through.
#[test]
fn typed_field_keeps_the_written_face() {
    let report = report_ops(
        "class A { int get foo => 0; }\n\
         class B extends A { String foo = \"s\"; }\n",
    );
    assert!(
        code_names(&report).contains(&"invalid_override"),
        "{:?}",
        rows(&report)
    );
}

// —— recorded gap (diagnose-side, not model): the induced field carries
// no pending-initializer row, so a secondary constructor still under-
// reports `not_initialized_non_nullable_instance_field` upstream — the
// staging lives in the listener's FieldDeclaration arm, which no event
// fires for a declaring formal. Pinned here so the follow-up has a
// fixture.
