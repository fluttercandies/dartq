//! The #3124 pins: the `conflicting_static_and_instance` body-close hook
//! for `enum` and `extension type` containers.
//!
//! `end_class_or_mixin_or_extension_body` invokes
//! `check_conflicting_static_and_instance` at the container's own body
//! close (the member sets are complete). The `enum` and `extension-type`
//! declaration-end events take a different listener path —
//! `end_enum_declaration` / `end_extension_type_declaration` — which
//! never ran the check, so a `static` member colliding with an
//! `instance` member inside an enum or extension-type body reported
//! nothing.
//!
//! Upstream, the same member-duplicate verifier's `_checkUnitStatic`
//! walks every top-level declaration and dispatches `_checkEnumStatic`
//! for an `EnumDeclarationImpl` and `_checkClassStatic` for an
//! `ExtensionTypeDeclarationImpl` — both driven at the declaration's
//! body close (`visitEnumDeclaration` /
//! `visitExtensionTypeDeclaration`). Upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/member_duplicate_definition_verifier.dart
//! (`_checkUnitStatic` :559-583, `_checkEnumStatic` :423-481,
//! `_checkClassStatic` :179-234; `_checkExtensionType` :524-530 routes
//! the extension-type through the class-member face).
//!
//! Oracle provenance: `dart analyze` 3.13.7 over the /tmp/dd3116
//! combination battery (`enum_static_inst`, `enum_static_index`,
//! `enum_const_member_dup`, `enum_const_vs_*`, `t_enum_static`,
//! `t_enum_const_get`, `exttype_static_inst`, `cls_mix`).

use super::test_rig::{assert_silent, report_ops};

fn conflicts(report: &crate::diagnose::FileReport) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == "conflicting_static_and_instance"
                || d.code.name == "extension_conflicting_static_and_instance"
        })
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

// —— enum: static × instance own-name collision ——

/// `static void m` + `void m` inside an enum reports
/// `conflicting_static_and_instance` at the STATIC member's name — the
/// same verdict a class body emits, reached through the enum end hook.
#[test]
fn enum_static_method_vs_instance_method_conflicts() {
    let report = report_ops(
        "enum E {\n\
         \x20 a, b;\n\
         \x20 static void m() {}\n\
         \x20 void m() {}\n\
         }\n",
    );
    let rows = conflicts(&report);
    assert_eq!(rows.len(), 1, "one static×instance row: {rows:?}");
    assert!(
        rows[0].2.contains("'m'"),
        "message names the colliding member: {rows:?}"
    );
}

/// An enum static getter named `index` collides with the implicit
/// `Enum.index` instance member — arm 2's interface probe, also gated on
/// the body-close hook now firing.
#[test]
fn enum_static_getter_index_conflicts() {
    let report = report_ops(
        "enum E {\n\
         \x20 a, b;\n\
         \x20 static int get index => 0;\n\
         }\n",
    );
    let rows = conflicts(&report);
    assert_eq!(rows.len(), 1, "implicit Enum.index probe row: {rows:?}");
    assert!(
        rows[0].2.contains("'index'"),
        "message names the member: {rows:?}"
    );
}

/// An enum CONSTANT is a static field whose induced getter joins the
/// enum's instance surface — a static member named the same reports.
#[test]
fn enum_constant_vs_static_member_conflicts() {
    let report = report_ops(
        "enum E {\n\
         \x20 a, b;\n\
         \x20 static int a = 0;\n\
         }\n",
    );
    let rows = conflicts(&report);
    assert!(
        !rows.is_empty(),
        "enum constant 'a' collides with static member 'a': {rows:?}"
    );
}

// —— extension type: static × instance own-name collision ——

/// `static void m` + `void m` inside an extension type reports
/// `conflicting_static_and_instance` — the extension-type routes through
/// the class-member face (`_checkClassStatic`), reached through the
/// extension-type end hook.
#[test]
fn extension_type_static_vs_instance_conflicts() {
    let report = report_ops(
        "extension type M(int x) {\n\
         \x20 static void m() {}\n\
         \x20 void m() {}\n\
         }\n",
    );
    let rows = conflicts(&report);
    assert_eq!(rows.len(), 1, "one static×instance row: {rows:?}");
    assert!(
        rows[0].2.contains("'m'"),
        "message names the colliding member: {rows:?}"
    );
}

// —— negative: no collision stays silent on both new hooks ——

/// An enum with no static×instance name overlap emits nothing — the new
/// hook adds the verdict, not noise.
#[test]
fn enum_clean_body_silent() {
    let report = report_ops(
        "enum E {\n\
         \x20 a, b;\n\
         \x20 static void m() {}\n\
         \x20 void n() {}\n\
         }\n",
    );
    assert!(
        conflicts(&report).is_empty(),
        "no collision — silent: {:?}",
        report.diagnostics
    );
}

/// An extension type with no static×instance name overlap emits nothing.
#[test]
fn extension_type_clean_body_silent() {
    let report = report_ops(
        "extension type M(int x) {\n\
         \x20 static void m() {}\n\
         \x20 void n() {}\n\
         }\n",
    );
    assert_silent(&report);
}
