//! #3279 pins — the `conflicting_static_and_instance` enum arm's probe /
//! target split.
//!
//! An enum constant is a static field whose induced getter is a static
//! accessor, so `_checkEnumStatic` (upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/member_duplicate_definition_verifier.dart
//! :423-479) probes it like any other static member. It is never a
//! *target*, though: `_getInterfaceMember` reads
//! `InheritanceManager3.getMember`, whose interface map is built by
//! `_getTypeMembers` and drops every static member
//! (pkg/analyzer/lib/src/dart/element/inheritance_manager3.dart:1336-1355).
//! A constant therefore collides with its container's INSTANCE surface
//! only — the same rule the class arm follows — so a plain
//! `enum E { one, two }` is silent while a static member colliding with a
//! real instance member (or the implicit `Enum`/`Object` faces) reports.
//!
//! Oracle provenance for the `enhanced_enums_error_test` faces pinned
//! here: conformance/expected.tsv's
//! `features/_core/enum/language/enum/enhanced_enums_error_test` row
//! carries no `conflicting_static_and_instance` for
//! `NoConflictClassEnumValueStaticSetter` / `ConflictClassEnumValueStaticSetterType`
//! (a static member named after a constant) and names the ENUM as
//! `conflictingClassName` on every enum row
//! (`ConflictStaticInheritedFoo`, `ConflictEnumValueInheritedFoo`).
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block; the module
//! stays inside the `diagnose` subtree so `crate::diagnose::test_rig`
//! resolves.

use crate::diagnose::FileReport;
use crate::diagnose::test_rig::report_ops;

/// The `conflicting_static_and_instance` rows of one report, verbatim, in
/// report order.
fn conflicts(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "conflicting_static_and_instance")
        .map(|d| d.message.to_string())
        .collect()
}

/// The whole enum body is one probe surface and one target surface: a
/// constant is a static member and nothing static answers a static probe.
#[test]
fn plain_enum_constants_stay_silent() {
    let report = report_ops("enum E { one, two }\n");
    let rows = conflicts(&report);
    assert!(
        rows.is_empty(),
        "a constant never collides with itself: {rows:?}"
    );
}

/// A constant with an explicit `;` and a mixed batch of values stays
/// silent too (the shape the conformance residual reported as EXTRA).
#[test]
fn enum_constants_with_bodies_stay_silent() {
    let report = report_ops(
        "enum E {\n\
         \x20 one,\n\
         \x20 two,\n\
         \x20 three;\n\
         }\n",
    );
    let rows = conflicts(&report);
    assert!(
        rows.is_empty(),
        "no instance surface exists to collide with: {rows:?}"
    );
}

/// `ConflictClassEnumValueStaticSetter`: a static member named after a
/// constant answers no interface probe — the constant's own accessor is
/// static.
#[test]
fn static_setter_named_after_a_constant_stays_silent() {
    let report = report_ops(
        "enum E {\n\
         \x20 e1;\n\
         \x20 static void set e1(int _) {}\n\
         }\n",
    );
    let rows = conflicts(&report);
    assert!(
        rows.is_empty(),
        "a static member colliding with a constant is not a static/instance conflict: {rows:?}"
    );
}

/// The same rule for a static method, which has no `values` escape and no
/// constant target either.
#[test]
fn static_method_named_after_a_constant_stays_silent() {
    let report = report_ops(
        "enum E {\n\
         \x20 one;\n\
         \x20 static int one() => 1;\n\
         }\n",
    );
    let rows = conflicts(&report);
    assert!(
        rows.is_empty(),
        "the constant is a static field, not an instance member: {rows:?}"
    );
}

/// `ConflictStaticInstanceEnumValue`: the report anchors at the CONSTANT
/// (the probe) and names the enum as the conflicting owner.
#[test]
fn constant_against_instance_getter_reports_at_the_constant() {
    let report = report_ops(
        "enum E {\n\
         \x20 foo;\n\
         \x20 int get foo => 42;\n\
         }\n",
    );
    let rows = conflicts(&report);
    assert_eq!(
        rows,
        vec![
            "Class 'E' can't define static member 'foo' and have instance member \
             'E.foo' with the same name."
                .to_string()
        ],
        "one row, anchored at the constant: {rows:?}"
    );
}

/// `ConflictInheritedEnumValue`: the inherited instance member is a
/// target too, and the enum — not the mixin — is the rendered owner.
#[test]
fn constant_against_inherited_instance_method_names_the_enum() {
    let report = report_ops(
        "mixin M { void foo() {} }\n\
         enum E with M {\n\
         \x20 foo;\n\
         }\n",
    );
    let rows = conflicts(&report);
    assert_eq!(
        rows,
        vec![
            "Class 'E' can't define static member 'foo' and have instance member \
             'E.foo' with the same name."
                .to_string()
        ],
        "the enum's own name renders, never the interface's: {rows:?}"
    );
}

/// `ConflictEnumValueInheritedIndex`: a constant named after an implicit
/// `Enum` member reports (the constant is still a probe).
#[test]
fn constant_named_after_implicit_enum_member_reports() {
    let report = report_ops("enum E { a, index, b }\n");
    let rows = conflicts(&report);
    assert_eq!(
        rows,
        vec![
            "Class 'E' can't define static member 'index' and have instance member \
             'E.index' with the same name."
                .to_string()
        ],
        "implicit Enum.index answers the constant's probe: {rows:?}"
    );
}

/// `DeclaresStaticIndex`: a static getter named `index` collides with the
/// implicit `Enum.index`, row anchored at the getter.
#[test]
fn static_getter_named_index_collides_with_implicit_enum_index() {
    let report = report_ops(
        "enum E {\n\
         \x20 a, b;\n\
         \x20 static int get index => 0;\n\
         }\n",
    );
    let rows = conflicts(&report);
    assert_eq!(
        rows,
        vec![
            "Class 'E' can't define static member 'index' and have instance member \
             'E.index' with the same name."
                .to_string()
        ],
        "implicit Enum.index is a real target: {rows:?}"
    );
}

/// `ConflictStaticInheritedFoo`: a static member over an inherited
/// instance member reports once, with the enum as the rendered owner.
#[test]
fn static_member_against_inherited_instance_member_names_the_enum() {
    let report = report_ops(
        "mixin M { void foo() {} }\n\
         enum E with M {\n\
         \x20 e1;\n\
         \x20 static int foo() => 42;\n\
         }\n",
    );
    let rows = conflicts(&report);
    assert_eq!(
        rows,
        vec![
            "Class 'E' can't define static member 'foo' and have instance member \
             'E.foo' with the same name."
                .to_string()
        ],
        "an inherited collision still renders the enum: {rows:?}"
    );
}

/// A `values` METHOD is no accessor, but it is an instance member, not a
/// probe either — the enum arm reports nothing for it (a `values` member
/// owns the separate `values_declaration_in_enum` face).
#[test]
fn values_method_is_not_a_static_probe() {
    let report = report_ops(
        "enum E {\n\
         \x20 one;\n\
         \x20 values() {}\n\
         }\n",
    );
    let rows = conflicts(&report);
    assert!(rows.is_empty(), "no static member, no probe: {rows:?}");
}

/// The `values` exemption covers the whole ACCESSOR probe, not just the
/// own-scope hit (`_checkEnumStatic`'s `baseName != 'values'` gate wraps
/// the entire report): a static field named `values` over an instance
/// getter reports nothing on this face — the name owns
/// `values_declaration_in_enum` instead.
#[test]
fn values_static_field_over_instance_getter_stays_exempt() {
    let report = report_ops(
        "enum E {\n\
         \x20 v;\n\
         \x20 static int values = 0;\n\
         \x20 int get values => 0;\n\
         }\n",
    );
    let rows = conflicts(&report);
    assert!(rows.is_empty(), "the accessor exemption is total: {rows:?}");
}

/// `index` is `Enum`'s instance member, not `Object`'s: a class's static
/// `index` inherits nothing to collide with.
#[test]
fn class_static_index_stays_silent() {
    let report = report_ops(
        "class A {\n\
         \x20 static int index = 0;\n\
         }\n",
    );
    let rows = conflicts(&report);
    assert!(
        rows.is_empty(),
        "a class has no inherited 'index': {rows:?}"
    );
}
