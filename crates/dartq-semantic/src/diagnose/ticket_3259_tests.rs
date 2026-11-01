//! #3259 pins — the `@override` presence face (`override_on_non_overriding_member`)
//! walks the element's full superinterface set, including the IMPLICIT
//! supertype the written clauses never name.
//!
//! # The wrong fact
//!
//! `face_member_any` seeded the climb with the written clauses only
//! (`superclass`/`with`/`implements`/`on`), so a container whose override
//! target lives on the implicit supertype reported a phantom
//! non-overriding row. The corpus case: `enum E { @override int get index
//! }` — upstream `override_verifier.dart:135` answers
//! `currentClass.element.getOverridden(name)`, whose interface map carries
//! the enum's `Enum` superinterface (`element.supertype` is `Enum`), so
//! `index` is a real override and the oracle stays silent. Our walk had no
//! `Enum` face to climb and emitted the row the oracle never emits —
//! `semantics_A01_t07`'s extra `WARNING|override_on_non_overriding_member`
//! at `46:11`.
//!
//! The fix seeds the same implicit-supertype face `check_override` already
//! seeds (`Object` for a superclass-less class or an `on`-less mixin,
//! `Enum` for an enum) and lets `frozen_top_member` answer the frozen
//! `Object`/`Enum` signatures when the element index carries no such
//! container.

use crate::diagnose::test_rig::{assert_silent, code_names, report_ops};

/// The codes of one fixture's report, in source order.
fn names(source: &str) -> Vec<&'static str> {
    code_names(&report_ops(source))
}

/// The corpus shape: `@override` on an enum's `index` getter is a real
/// override — `element.supertype` is `Enum`, whose `index` face the
/// interface map carries (upstream `override_verifier.dart`'s
/// `_isOverride` → `getOverridden`). Silent before the fix emitted the
/// phantom row.
#[test]
fn t3259_enum_index_override_is_silent() {
    let report = report_ops(
        "enum E { a(1), b(2); final int _i; @override int get index => _i; const E(this._i); }",
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "override_on_non_overriding_member"),
        "expected no override_on_non_overriding_member row, got {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

/// `Enum`'s inherited `Object` face rides the same seeded supertype — an
/// enum's `@override toString` overrides `Object.toString` through `Enum`
/// (the `Enum` face's superclass chain), exactly the oracle's interface map.
#[test]
fn t3259_enum_tostring_override_is_silent() {
    let report = report_ops(
        "enum E { a, b; @override String toString() => 'E'; }",
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "override_on_non_overriding_member"),
        "expected no override_on_non_overriding_member row, got {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

/// An enum member overriding NOTHING still reports — the seeded `Enum` face
/// only carries `index` (plus `Object`'s face), so a foreign name walks the
/// whole set and misses.
#[test]
fn t3259_enum_non_overriding_member_reports() {
    assert_eq!(
        names("enum E { a, b; @override int get ghost => 0; }"),
        ["override_on_non_overriding_member"]
    );
    assert_eq!(
        names("enum E { a, b; @override void ghost() {} }"),
        ["override_on_non_overriding_member"]
    );
}

/// A class member overriding nothing reports — the seeded `Object` face
/// carries no such name (the corpus's `test_class` shape).
#[test]
fn t3259_class_non_overriding_member_reports() {
    assert_eq!(
        names("class C { @override void ghost() {} }"),
        ["override_on_non_overriding_member"]
    );
    assert_eq!(
        names("class C { @override int get ghost => 0; }"),
        ["override_on_non_overriding_member"]
    );
}

/// A real override through `extends` stays silent — the regression guard
/// for the written-clause faces the walk already carried.
#[test]
fn t3259_extends_override_is_silent() {
    assert_silent(&report_ops(
        "class A { void f() {} } class B extends A { @override void f() {} }",
    ));
    assert_silent(&report_ops(
        "class A { int get g => 0; } class B extends A { @override int get g => 1; }",
    ));
}

/// An `implements` member counts as an override — the oracle's
/// `getOverridden` covers every superinterface, not just the superclass
/// spine.
#[test]
fn t3259_implements_override_is_silent() {
    assert_silent(&report_ops(
        "class A { void f() {} } class B implements A { @override void f() {} }",
    ));
}

/// A mixin-provided member counts — `with M` faces join the climb in
/// application order.
#[test]
fn t3259_mixin_provided_override_is_silent() {
    assert_silent(&report_ops(
        "mixin M { void f() {} } class C with M { @override void f() {} }",
    ));
}

/// An abstract member re-declared in a subclass is still an override — the
/// name-level `getOverridden` never asks about concrete-ness.
#[test]
fn t3259_abstract_redeclaration_is_silent() {
    assert_silent(&report_ops(
        "abstract class A { void f(); } abstract class B extends A { @override void f(); }",
    ));
}

/// A static member never enters the interface map — `getOverridden` misses
/// by construction, so `@override` on a static reports (the corpus's
/// `test_class_static` shape).
#[test]
fn t3259_static_member_reports() {
    assert_eq!(
        names("class A { static void f() {} } class B extends A { @override static void f() {} }"),
        ["override_on_non_overriding_member"]
    );
}

/// An `on`-less mixin's `Object` constraint is seeded — `toString` is
/// spared by the `object_members` short-circuit upstream of the walk, so
/// this pin guards a NON-Object name: a mixin whose `on` face provides the
/// member spares the row.
#[test]
fn t3259_mixin_on_face_override_is_silent() {
    assert_silent(&report_ops(
        "class A { void f() {} } mixin M on A { @override void f() {} }",
    ));
}
