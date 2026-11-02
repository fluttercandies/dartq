//! The #3661 pins — two same-file families in `overrides.rs`.
//!
//! # Family 9 — `subtype_of_base_or_final_is_not_base_final_or_sealed`
//! (99 extra rows over 68 cases)
//!
//! The `BaseOrFinalTypeVerifier` mounts on the resolver's class,
//! class-type-alias and mixin visitors ONLY (resolver.dart's three
//! `checkElement` sites) — an enum or extension type never runs it, so
//! `enum Bar implements BaseClass` stays silent on this family no matter
//! the modifier face. The face lists are the ELEMENT's resolved
//! supertype columns, not the written clauses: `element.supertype` joins
//! only when the extends head resolves to a class
//! (`_isInterfaceTypeClass`, types_builder.dart:40-46 — `class C extends
//! SomeMixin` leaves it null), and `interfaces`/`mixins`/
//! `superclassConstraints` keep interface-typed elements only
//! (`isInterfaceTypeInterface`, types_builder.dart:23-37 — enum and
//! extension-type entries drop out). The pass now gates on
//! `Class | Mixin` declarations and filters each clause's entries by the
//! resolved element's kind.
//!
//! # Family 10 — `conflicting_static_and_instance` (97 miss + 1 extra
//! over 56 cases)
//!
//! The instance scope upstream probes is the MERGED ELEMENT's, not the
//! declaring fragment's (`_InstanceElementContext` keyed on
//! `fragment.element.firstFragment`, member_duplicate_definition_verifier
//! .dart:591-594): members introduced inside `augment` containers of any
//! unit of the library claim into the same scope, primary-constructor
//! declaring formals join through the `primaryConstructor` arm (:62-97),
//! and an extension type's representation field claims with them. The
//! check now builds `instance_names` from `resolve_container`'s merged
//! member map instead of the fragment's own rows. The one extra (a
//! recovery `static mixin M {}` row probing under the empty name) is
//! suppressed by upstream's `identifier.isSynthetic` gate (:303).
//!
//! A member carrying `augment` in a NON-leading position (`external
//! augment m();`) keeps `augmentKeyword` null upstream — it is a plain
//! declaration that claims its scope, parser-modifier error rows
//! notwithstanding (parser_impl.dart's first-slot binding). Our model
//! over-marks `is_augment` on any-position `augment`, so the claim filter
//! re-derives leading position from the token stream
//! (`member_augment_is_leading`) and re-admits such members.
//!
//! Oracle provenance: `dart analyze` 3.13.7 over the fixture matrix —
//! the enum/typedef class-modifier rows (family 9), the
//! `Augmentations/scoping_A02_t01` cross-file member conflicts, the
//! `Primary-constructors`/`Extension-types` static-collision rows, and
//! the `member_errors` non-leading `augment` asymmetry (family 10).

use super::test_rig::{
    assert_silent, report_ops, report_ops_augment, report_ops_augment_intro_experiments, rows,
};

fn code_count(report: &crate::diagnose::FileReport, code: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .count()
}

fn conflicting_count(report: &crate::diagnose::FileReport) -> usize {
    code_count(report, "conflicting_static_and_instance")
}

// —— family 9: the verifier never mounts on enums / extension types ——

/// `enum Bar implements Foo` where `Foo` is `base`: the oracle is silent
/// (checkElement never runs for an enum declaration). Cross-library
/// shape is what the census extras carry, but the same-library face is
/// the same gate — an enum never reports this family.
#[test]
fn enum_implementing_base_class_stays_silent() {
    let report = report_ops(
        "base class Foo {}\n\
         enum Bar implements Foo { v }\n",
    );
    assert_eq!(
        code_count(&report, "subtype_of_base_or_final_is_not_base_final_or_sealed"),
        0,
        "an enum never runs the base/final verifier: {:?}",
        rows(&report)
    );
}

/// The `with` arm: `enum Bar with BaseMixin` — the enum's mixin
/// application is still an enum declaration, not a class/mixin element.
#[test]
fn enum_with_base_mixin_stays_silent() {
    let report = report_ops(
        "base mixin M {}\n\
         enum Bar with M { v }\n",
    );
    assert_eq!(
        code_count(&report, "subtype_of_base_or_final_is_not_base_final_or_sealed"),
        0,
        "an enum's `with` clause never reports this family: {:?}",
        rows(&report)
    );
}

/// A mixin declaration's `on` constraint against an ENUM element drops
/// out upstream (`superclassConstraints` keeps interface-typed elements
/// only — enums are not interfaces), so `mixin M on MyEnum` reports
/// nothing on this family.
#[test]
fn mixin_on_enum_constraint_stays_silent() {
    let report = report_ops(
        "enum E { v }\n\
         mixin M on E {}\n",
    );
    assert_eq!(
        code_count(&report, "subtype_of_base_or_final_is_not_base_final_or_sealed"),
        0,
        "an enum entry drops out of `superclassConstraints`: {:?}",
        rows(&report)
    );
}

/// `class C extends SomeMixin` (a mixin in the extends position) leaves
/// `element.supertype` null upstream — the `BaseOrFinalTypeVerifier`'s
/// extends arm checks the resolved supertype, not the written type, so
/// only the `with`/`implements` faces can report for this declaration.
#[test]
fn class_extends_mixin_supertype_stays_out_of_extends_arm() {
    let report = report_ops(
        "base mixin M {}\n\
         class C extends M {}\n",
    );
    // `extends M` resolves to a mixin: `supertype` is null, so the
    // extends arm reports nothing. (The `with`-position faces would
    // still fire; none are written here.)
    assert_eq!(
        code_count(&report, "subtype_of_base_or_final_is_not_base_final_or_sealed"),
        0,
        "a mixin in extends position never joins `element.supertype`: {:?}",
        rows(&report)
    );
}

/// Negative control — the verifier still mounts on classes: a plain
/// `class C extends BaseClass` where `C` carries no `base`/`final`/
/// `sealed` modifier still reports.
#[test]
fn plain_class_extending_base_still_reports() {
    let report = report_ops(
        "base class Base {}\n\
         class C extends Base {}\n",
    );
    assert_eq!(
        code_count(&report, "subtype_of_base_or_final_is_not_base_final_or_sealed"),
        1,
        "a non-base/final/sealed class extending `base` still reports: {:?}",
        rows(&report)
    );
}

/// Negative control — mixins mount the verifier too (`mixin M on
/// BaseClass` where `M` is unmodified still reports).
#[test]
fn plain_mixin_on_base_still_reports() {
    let report = report_ops(
        "base class Base {}\n\
         mixin M on Base {}\n",
    );
    assert_eq!(
        code_count(&report, "subtype_of_base_or_final_is_not_base_final_or_sealed"),
        1,
        "a non-base/final/sealed mixin on `base` still reports: {:?}",
        rows(&report)
    );
}

// —— family 10: the merged element's instance scope ——

/// Same-file `augment` container: a member introduced inside `augment
/// class C` claims into the merged element's instance scope
/// (upstream `_InstanceElementContext` is keyed on the element's first
/// fragment, shared across all fragments of all units), so a static
/// `foo` in the introduction collides with the augment's instance `foo`.
#[test]
fn augment_container_member_claims_into_merged_instance_scope() {
    let report = report_ops(
        "class C { static void foo() {} }\n\
         augment class C { void foo() {} }\n",
    );
    assert_eq!(
        conflicting_count(&report),
        1,
        "the augment container's instance member claims: {:?}",
        rows(&report)
    );
}

/// Cross-file shape — the `scoping_A02_t01` face: the introduction in
/// the main unit carries the static, the augment part carries the
/// instance member; the row anchors at the STATIC member, so it lands on
/// the INTRODUCING unit's report.
#[test]
fn cross_file_augment_member_collides_with_static() {
    let report = report_ops_augment_intro_experiments(
        "class C { static void foo() {} }\n",
        "augment class C { void foo() {} }\n",
        &[dartq_language::Feature::Augmentations],
    );
    assert_eq!(
        conflicting_count(&report),
        1,
        "a cross-unit augment member claims into the merged scope: {:?}",
        rows(&report)
    );
}

/// The reverse polarity: the instance member lives in the introduction,
/// the static member arrives through the augment part — same merged
/// scope, same row.
#[test]
fn cross_file_static_in_augment_collides_with_instance() {
    let report = report_ops_augment(
        "class C { void foo() {} }\n",
        "augment class C { static void foo() {} }\n",
    );
    assert_eq!(
        conflicting_count(&report),
        1,
        "the augment's static member probes the merged scope: {:?}",
        rows(&report)
    );
}

/// An extension type's representation field claims as an instance member
/// (`extension type ET(int id)` induces the `id` getter) — a static
/// member of the same name collides (the `A03_t02`/`A04_t13` face).
#[test]
fn extension_type_rep_field_collides_with_static() {
    let report = report_ops(
        "extension type ET(int id) {\n\
         \x20 static int get id => 0;\n\
         }\n",
    );
    assert_eq!(
        conflicting_count(&report),
        1,
        "the representation field claims `id` into the instance scope: {:?}",
        rows(&report)
    );
}

/// A primary constructor's declaring formal induces an instance field —
/// `class C(var v)` claims `v` (getter+setter) before the body members,
/// so a `static v` in the body collides.
#[test]
fn primary_ctor_declaring_formal_collides_with_static() {
    let report = report_ops(
        "class C(var v) {\n\
         \x20 static int v = 0;\n\
         }\n",
    );
    assert_eq!(
        conflicting_count(&report),
        1,
        "the declaring formal's induced field claims `v`: {:?}",
        rows(&report)
    );
}

// —— family 10: the non-leading `augment` asymmetry ——

/// `external augment m();` keeps `augmentKeyword` null upstream — the
/// member is a plain declaration that CLAIMS its scope (the
/// `conflicting_modifiers`/`modifier_out_of_order` parser rows ride
/// alongside, but the scope claim still lands). An instance member
/// written this way collides with a static of the same name.
#[test]
fn external_augment_instance_member_still_claims() {
    let report = report_ops(
        "class C {\n\
         \x20 static void m() {}\n\
         \x20 external augment void m();\n\
         }\n",
    );
    assert_eq!(
        conflicting_count(&report),
        1,
        "`external augment` is a plain member upstream — it claims: {:?}",
        rows(&report)
    );
}

/// `augment`-leading is the real augmentation: `augment void m();` is a
/// member fragment that neither checks nor claims
/// (member_duplicate_definition_verifier.dart:307-309) — no conflict row.
#[test]
fn leading_augment_member_never_claims() {
    let report = report_ops(
        "class C {\n\
         \x20 static void m() {}\n\
         \x20 augment void m();\n\
         }\n",
    );
    assert_eq!(
        conflicting_count(&report),
        0,
        "a leading-`augment` member claims nothing: {:?}",
        rows(&report)
    );
}

/// A SECOND declarator in one field group keeps the group's leading
/// `augment`: `augment int a, b;` marks both `a` and `b` augmentations
/// upstream — neither claims. The header window must not stop inside the
/// group's own declarator list.
#[test]
fn leading_augment_field_group_second_declarator_never_claims() {
    let report = report_ops(
        "class C {\n\
         \x20 static int b = 0;\n\
         \x20 augment int a = 0, b = 0;\n\
         }\n",
    );
    assert_eq!(
        conflicting_count(&report),
        0,
        "every declarator of a leading-`augment` group is an augmentation: {:?}",
        rows(&report)
    );
}

/// Negative control — the own-scope arm still fires when an ordinary
/// instance member meets a same-named static (the `#2963` pin shape,
/// reasserted against the merged-scope rebuild).
#[test]
fn own_scope_static_instance_still_reports() {
    let report = report_ops(
        "class S {\n\
         \x20 static void m() {}\n\
         \x20 void m() {}\n\
         }\n",
    );
    assert_eq!(
        conflicting_count(&report),
        1,
        "the own-scope arm still fires: {:?}",
        rows(&report)
    );
}

/// A static member with no instance counterpart stays silent — the
/// merged scope never invents names.
#[test]
fn static_member_without_instance_counterpart_stays_silent() {
    let report = report_ops(
        "class C {\n\
         \x20 static void m() {}\n\
         \x20 void other() {}\n\
         }\n",
    );
    assert_silent(&report);
}
