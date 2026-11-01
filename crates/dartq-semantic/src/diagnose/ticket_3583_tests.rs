//! The #3583 pins: `conflicting_static_and_instance` over-reporting.
//!
//! Census attribution: 259 surplus rows against the 3.13.7 oracle split
//! into three comparator shapes the emitter answered wrong.
//!
//! 1. **Extension types have no implicit `Object` interface.** Upstream
//!    computes an extension type's static-vs-instance conflicts from
//!    `getInterface(element).conflicts`, whose `redeclared` face is built
//!    from the `implements` clause only (`InheritanceManager3`
//!    `_getInterfaceExtensionType`, upstream Dart SDK source
//!    pkg/analyzer/lib/src/dart/element/inheritance_manager3.dart:778-792
//!    — candidates accrue from `element.interfaces`, never `Object`). A
//!    `static int hashCode()` therefore reports
//!    `extension_type_declares_member_of_object` alone; the own-scope
//!    arm (`_checkClassStatic` on the extension-type face) compares own
//!    instance members only.
//! 2. **Constructors are not instance members.** `_checkClassStatic`
//!    walks `ClassMember` field/method declarations
//!    (member_duplicate_definition_verifier.dart:191-233); a factory
//!    constructor named `foo` never enters the instance scope, so
//!    `static int foo = 0` beside it reports
//!    `conflicting_constructor_and_static_member` alone.
//! 3. **An augmenting member is a fragment of the augmented
//!    declaration, not an introductory member.** A member that augments
//!    nothing (`isAugmentationWithoutAugmentedDeclaration`) never joins
//!    the merged instance scope, so the introductory `static` member it
//!    names reports nothing (oracle: only
//!    `augmentation_without_declaration`). A `augment static` member
//!    WITH a real instance target still reports (the corpus's
//!    `augmenting_class_like_declarations` shape).
//!
//! Oracle provenance: one `dart analyze --format=machine` 3.13.7 probe
//! over the s1-s10 (plain) and a1-a5 (augmentations) shape battery, plus
//! the frozen `conformance/expected.tsv` rows for the attributed cases.
//!
//! Census arithmetic over `census-20261102-045457.report.tsv` (ERROR/
//! `conflicting_static_and_instance`: 71 rows, 259 surplus occurrences):
//! 17 are double emissions of a row the oracle already records (the
//! body-close and declaration-end hooks both fire), 48 render an
//! `Object`-owned inherited conflict on an extension type, 4 are
//! constructor-name captures — shapes 1-2 silence all of them, and the
//! duplicate contract silences the first. The remaining 190 occurrences
//! are augmenting-fragment shapes. #3585 traced them: the collector is NOT
//! on this path — `fold_fragment` already drops a member carrying its own
//! `augment` keyword from the merged member map (collector.rs, the
//! `is_augment` filter), and the invalid-`augment` shape never folds at
//! all (a plain `class` head takes the claim arm, not the fragment arm).
//! The own-scope arm reads the engine's flat declaration sites instead:
//! `note_decl_site` (unused_finalization.rs) sets `DeclSite::is_augment`
//! from the constructor latch only, so a method/field site never carries
//! the member's own `augment` keyword even though the listener already
//! latches it (`open_exec_augment` at the method head, `group_augment` at
//! the field group head), and `check_conflicting_static_and_instance`
//! (overrides.rs) builds `instance_names` from those sites with no
//! `is_augment` filter. The repair is engine-side (set the bit on member
//! sites, filter the instance set), and the two `#[ignore]`d pins below
//! state the oracle truth for it. The #3585 positive controls
//! (`ticket_3585_tests.rs`) hold the other half fixed: a member WITHOUT
//! the keyword inside an `augment` body is an introduction that still
//! claims and still reports.

use super::test_rig::{report_ops, report_ops_experiments_with};
use super::FileReport;
use dartq_language::Feature;

fn conflicts(report: &FileReport) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "conflicting_static_and_instance")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

fn conflicts_augmented(source: &str) -> Vec<(u32, u32, String)> {
    let report = report_ops_experiments_with(source, &[], None, &[Feature::Augmentations]);
    conflicts(&report)
}

// —— shape 1: extension types vs the implicit `Object` face ——

/// `extension type E(int it) { static int hashCode() => 0; }` reports
/// `extension_type_declares_member_of_object` upstream, never
/// `conflicting_static_and_instance`: an extension type's conflict face
/// reads the `implements` clause, not `Object`.
#[test]
fn extension_type_static_object_getter_is_silent() {
    let report = report_ops(
        "extension type E(int it) {\n\
         \x20 static int get hashCode => 0;\n\
         }\n",
    );
    assert!(conflicts(&report).is_empty(), "{:?}", conflicts(&report));
}

/// The method spelling (`static dynamic noSuchMethod(Invocation i)`) is
/// the same face: no implicit-`Object` probe for extension types.
#[test]
fn extension_type_static_object_method_is_silent() {
    let report = report_ops(
        "extension type E(int it) {\n\
         \x20 static dynamic noSuchMethod(Invocation i) => null;\n\
         }\n",
    );
    assert!(conflicts(&report).is_empty(), "{:?}", conflicts(&report));
}

/// The implicit `Object` face still answers for a CLASS: a static
/// `hashCode` collides with the inherited `Object.hashCode` getter
/// (oracle: `Class 'C' can't define static member 'hashCode' and have
/// instance member 'Object.hashCode'`). Dropping the extension-type
/// probe must not drop the class probe.
#[test]
fn class_static_object_getter_still_reports() {
    let report = report_ops(
        "class C {\n\
         \x20 static int get hashCode => 0;\n\
         }\n",
    );
    let rows = conflicts(&report);
    assert_eq!(rows.len(), 1, "inherited Object probe: {rows:?}");
    assert!(
        rows[0].2.contains("'Object.hashCode'"),
        "owner renders as Object: {rows:?}"
    );
}

/// Own-scope extension-type collision still reports exactly once
/// (oracle s1: `Class 'A' can't define static member 'foo' and have
/// instance member 'A.foo'`).
#[test]
fn extension_type_own_static_vs_instance_reports_once() {
    let report = report_ops(
        "extension type A(int it) {\n\
         \x20 static void foo() {}\n\
         \x20 void foo() {}\n\
         }\n",
    );
    let rows = conflicts(&report);
    assert_eq!(rows.len(), 1, "one own-scope row: {rows:?}");
    assert!(rows[0].2.contains("'A.foo'"), "{rows:?}");
}

/// An `implements` -clause instance member still collides (oracle s3:
/// `Class 'B3' can't define static member 'foo' and have instance member
/// 'A3.foo'`), exactly once.
#[test]
fn extension_type_interface_static_vs_instance_reports_once() {
    let report = report_ops(
        "extension type A(int it) {\n\
         \x20 void foo() {}\n\
         }\n\
         extension type B(int it) implements A {\n\
         \x20 static void foo() {}\n\
         }\n",
    );
    let rows = conflicts(&report);
    assert_eq!(rows.len(), 1, "one interface-conflict row: {rows:?}");
    assert!(rows[0].2.contains("'A.foo'"), "{rows:?}");
}

// —— shape 2: constructors are not instance members ——

/// A factory constructor named `foo` never joins the instance scope, so
/// the static field `foo` beside it reports
/// `conflicting_constructor_and_static_member` alone (oracle s7).
#[test]
fn factory_constructor_name_is_not_an_instance_member() {
    let report = report_ops(
        "class C {\n\
         \x20 factory foo() => throw 0;\n\
         \x20 static int foo = 0;\n\
         }\n",
    );
    assert!(conflicts(&report).is_empty(), "{:?}", conflicts(&report));
}

/// The enum face: a factory constructor and a static field of the same
/// name stay silent for this code (the constructor/static-method face
/// owns the verdict).
#[test]
fn enum_factory_constructor_name_is_not_an_instance_member() {
    let report = report_ops(
        "enum E {\n\
         \x20 v;\n\
         \x20 const E();\n\
         \x20 factory foo() => throw 0;\n\
         \x20 static int foo = 0;\n\
         }\n",
    );
    assert!(conflicts(&report).is_empty(), "{:?}", conflicts(&report));
}

// —— shape 3: augmenting members are fragments, not members ——

/// `class A { static int foo = 0; } augment class A { augment int foo = 0; }`
/// augments nothing: the invalid augmenting member never joins the
/// instance scope, so the introductory static member stays silent
/// (oracle a1: `augmentation_without_declaration` only).
///
/// Ignored: the own-scope arm builds its instance set from the engine's
/// flat declaration sites (`check_conflicting_static_and_instance`,
/// overrides.rs), and a member site never carries the member's own
/// `augment` keyword — `note_decl_site` (unused_finalization.rs) sets
/// `DeclSite::is_augment` from the constructor latch only — so the
/// emitter cannot tell this augmenting fragment from an introduction. The
/// collector's merged member map already excludes it (`fold_fragment`'s
/// `is_augment` filter) and is not what this arm reads. The pin states
/// the oracle truth so the engine-side repair lands against a failing
/// witness.
#[test]
#[ignore = "member decl sites never carry the member's own augment keyword (engine note_decl_site + overrides instance_names filter)"]
fn augment_without_target_does_not_collide() {
    let rows = conflicts_augmented(
        "class A {\n\
         \x20 static int foo = 0;\n\
         }\n\
         augment class A {\n\
         \x20 augment int foo = 0;\n\
         }\n",
    );
    assert!(rows.is_empty(), "{rows:?}");
}

/// `class C { int foo = 0; } augment class C { augment static int foo = 0; }`
/// keeps reporting: the augmenting static member's target exists, and
/// upstream compares it against the merged instance scope (oracle a4:
/// one row at the augmenting member's name).
#[test]
fn augment_static_member_with_target_still_reports() {
    let rows = conflicts_augmented(
        "class C {\n\
         \x20 int foo = 0;\n\
         }\n\
         augment class C {\n\
         \x20 augment static int foo = 0;\n\
         }\n",
    );
    assert_eq!(rows.len(), 1, "augmenting static conflict: {rows:?}");
    assert!(rows[0].2.contains("'C.foo'"), "{rows:?}");
}

/// The invalid-`augment` parser-fe shape (`augment` members inside a
/// non-augment class) reports no static-vs-instance row: the members
/// never become declarations of the container (oracle
/// `parser-fe/augmentation/member_declarations`: zero such rows).
///
/// Ignored for the same engine-side reason as
/// [`augment_without_target_does_not_collide`]: the two `augment`-shaped
/// members reach the declaration-site capture as an ordinary
/// instance/static pair. This shape is also the direct disproof of a
/// collector attribution — a plain `class` head takes the claim arm of
/// `close_container`, so `fold_fragment` never runs here at all.
#[test]
#[ignore = "member decl sites never carry the member's own augment keyword (engine note_decl_site + overrides instance_names filter)"]
fn augment_members_in_non_augment_container_are_silent() {
    let rows = conflicts_augmented(
        "class C {\n\
         \x20 augment method() {}\n\
         \x20 augment static void method() {}\n\
         }\n",
    );
    assert!(rows.is_empty(), "{rows:?}");
}

// —— the inherited arm still fires where upstream fires ——

/// A superclass instance member collides with a subclass static member
/// (oracle s5: `Class 'T' can't define static member 'foo' and have
/// instance member 'S.foo'`).
#[test]
fn inherited_instance_vs_subclass_static_reports() {
    let report = report_ops(
        "class S {\n\
         \x20 void foo() {}\n\
         }\n\
         class T extends S {\n\
         \x20 static void foo() {}\n\
         }\n",
    );
    let rows = conflicts(&report);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].2.contains("'S.foo'"), "{rows:?}");
}

/// A mixin's instance member collides with the applying class's static
/// member (oracle s9: `Class 'X' can't define static member 'foo' and
/// have instance member 'M.foo'`).
#[test]
fn mixin_instance_vs_static_reports() {
    let report = report_ops(
        "mixin M {\n\
         \x20 void foo() {}\n\
         }\n\
         class X with M {\n\
         \x20 static void foo() {}\n\
         }\n",
    );
    let rows = conflicts(&report);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].2.contains("'M.foo'"), "{rows:?}");
}

/// A superclass STATIC member never answers the inherited probe: the
/// subclass's instance member of the same name stays silent (oracle s6).
#[test]
fn inherited_static_vs_subclass_instance_is_silent() {
    let report = report_ops(
        "class S {\n\
         \x20 static void foo() {}\n\
         }\n\
         class T extends S {\n\
         \x20 void foo() {}\n\
         }\n",
    );
    assert!(conflicts(&report).is_empty(), "{:?}", conflicts(&report));
}

/// The own-scope class arm is unchanged by the #3583 narrowing: a
/// static member colliding with its own instance member reports once.
#[test]
fn class_own_static_vs_instance_still_reports() {
    let report = report_ops(
        "class A {\n\
         \x20 static void foo() {}\n\
         \x20 void foo() {}\n\
         }\n",
    );
    let rows = conflicts(&report);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].2.contains("'A.foo'"), "{rows:?}");
}


