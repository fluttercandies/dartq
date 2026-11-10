//! #3590 — `unused_element` must not double-report on `augment` fragments.
//!
//! Upstream's `_reportDiagnosticForElement` anchors every
//! `unused_element`/`unused_element_parameter`/`unused_field` row at
//! `element.firstFragment.nameOffset`
//! (`pkg/analyzer/.../unused_local_elements_verifier.dart:1051-1067`) —
//! the introductory fragment's position. An `augment` declaration's
//! element merges into the same element the introductory declaration
//! declares, so the merged verdict reports once at the intro site and
//! never at the augmenting fragment's own offset.
//!
//! The `augmentations()` model row keys every `augment` fragment on the
//! name token's span — the same anchor `DeclSite::range` stores — so
//! `finalize_usage` now skips both the element-face verdict and the
//! parameter-face tail for any site whose range lands in that set. A
//! member declared inside an `augment` container WITHOUT its own
//! `augment` keyword is a new element (its firstFragment IS the augment
//! fragment) and still reports at its own site.
//!
//! Oracle rows below were captured against `dart analyze --format machine`
//! 3.13.7 with `analyzer.enable-experiment: [augmentations]` on.

use super::FileReport;
use super::test_rig::report_ops_experiments_with;
use dartq_language::Feature;

fn unused_rows(source: &str) -> Vec<(u32, u32, String)> {
    let report = report_ops_experiments_with(source, &[], None, &[Feature::Augmentations]);
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_element")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

fn unused_parameter_rows(source: &str) -> Vec<(u32, u32, String)> {
    let report = report_ops_experiments_with(source, &[], None, &[Feature::Augmentations]);
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_element_parameter")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

// —— The augment-fragment stop-fabricating pins ———————————————————————

/// `augment class C { augment void _foo(); }` used to stage a second
/// `unused_element` row at the augment fragment's own offset (the
/// census's 137-entry Augmentations extra cluster). Oracle reports the
/// intro declaration once — the augment fragment carries no verdict of
/// its own.
///
/// Oracle (3.13.7):
///   WARNING|STATIC_WARNING|UNUSED_ELEMENT|…|3|8|4|The declaration '_foo' isn't referenced.
///   (line 6 `augment void _foo()` produces no row)
#[test]
fn augment_method_fragment_never_reports_unused_element() {
    let rows = unused_rows(
        "class C {\n\
         \x20 void _foo() {}\n\
         }\n\
         augment class C {\n\
         \x20 augment void _foo() {}\n\
         }\n\
         main() { C(); }\n",
    );
    assert_eq!(
        rows.len(),
        1,
        "one merged `_foo` verdict at the intro site: {rows:?}"
    );
    assert!(
        rows[0].2.contains("'_foo'"),
        "row names the merged element: {rows:?}"
    );
}

/// An `augment` enum constructor's site (`augment const E.foo(...)`)
/// merged into the introductory `E.foo`'s element: the verdict reports
/// once, at the intro fragment — never at the augment's own offset.
///
/// Oracle (3.13.7) on the `augmenting_constructors_A01_t05` shape reports
/// `E.foo` once at the intro ctor's name offset.
#[test]
fn augment_enum_constructor_fragment_never_reports_unused_element() {
    let rows = unused_rows(
        "enum E {\n\
         \x20 e0(0);\n\
         \x20 const E(int? x);\n\
         \x20 const E.foo([int? y = 0]);\n\
         }\n\
         augment enum E {\n\
         \x20 ;\n\
         \x20 augment const E.foo(int? y);\n\
         }\n\
         main() { print(E); }\n",
    );
    assert_eq!(
        rows.iter().filter(|r| r.2.contains("'E.foo'")).count(),
        1,
        "exactly one `E.foo` verdict (intro site): {rows:?}"
    );
}

/// `augment class C {}` on a private class: oracle reports `_C` once at
/// the intro fragment (the augment fragment is the same element). The
/// augment fragment used to stage a duplicate at its own name offset.
///
/// Oracle (3.13.7):
///   WARNING|STATIC_WARNING|UNUSED_ELEMENT|…|2|7|2|The declaration '_C' isn't referenced.
///   (line 3 `augment class _C` produces no row)
#[test]
fn augment_class_fragment_never_reports_unused_element() {
    let rows = unused_rows(
        "class _C {}\n\
         augment class _C {}\n\
         main() { }\n",
    );
    assert_eq!(
        rows.len(),
        1,
        "one merged `_C` verdict at the intro site: {rows:?}"
    );
}

/// `augment` top-level function: same merged-element rule — the augment
/// fragment produces no row, the intro's verdict stands alone.
///
/// Oracle (3.13.7):
///   WARNING|STATIC_WARNING|UNUSED_ELEMENT|…|1|6|7|The declaration '_helper' isn't referenced.
#[test]
fn augment_top_level_function_fragment_never_reports_unused_element() {
    let rows = unused_rows(
        "void _helper() {}\n\
         augment void _helper() {}\n\
         main() { }\n",
    );
    assert_eq!(rows.len(), 1, "one `_helper` verdict: {rows:?}");
}

/// `augment` top-level variable: same merged-element rule — the augment
/// fragment's name token is not a verdict site.
///
/// Oracle (3.13.7) reports `_topVar` once at the intro fragment.
#[test]
fn augment_top_level_variable_fragment_never_reports_unused_element() {
    let rows = unused_rows(
        "int _topVar = 0;\n\
         augment int _topVar = 0;\n\
         main() { }\n",
    );
    assert_eq!(rows.len(), 1, "one `_topVar` verdict: {rows:?}");
}

/// The augment constructor's parameter face joins the same skip — the
/// introductory parameter's element owns the `unused_element_parameter`
/// verdict and reports at the intro site's parameter offset, never at
/// the augment's own.
///
/// Oracle (3.13.7):
///   WARNING|STATIC_WARNING|UNUSED_ELEMENT|…|4|5|4|The declaration 'C._foo' isn't referenced.
///   WARNING|STATIC_WARNING|UNUSED_ELEMENT_PARAMETER|…|4|16|1|A value for optional parameter 'y' isn't ever given.
///   (line 7 `augment C._foo({int? y})` produces no row for `_foo` or `y`)
#[test]
fn augment_constructor_formals_never_report_unused_element_parameter() {
    let elements = unused_rows(
        "class C {\n\
         \x20 C();\n\
         \x20 C._foo({int? y = 0});\n\
         }\n\
         augment class C {\n\
         \x20 augment C._foo({int? y});\n\
         }\n\
         main() { }\n",
    );
    assert_eq!(
        elements.len(),
        1,
        "one `C._foo` element verdict at the intro site: {elements:?}"
    );
    let parameters = unused_parameter_rows(
        "class C {\n\
         \x20 C();\n\
         \x20 C._foo({int? y = 0});\n\
         }\n\
         augment class C {\n\
         \x20 augment C._foo({int? y});\n\
         }\n\
         main() { }\n",
    );
    assert_eq!(
        parameters.len(),
        1,
        "one `y` parameter verdict at the intro site's parameter: {parameters:?}"
    );
}

// —— The still-reports pins (no over-suppression) —————————————————————

/// A NEW member declared inside an `augment` class without its own
/// `augment` keyword is its own element — the verdict still reports at
/// its own site.
///
/// Oracle (3.13.7):
///   WARNING|STATIC_WARNING|UNUSED_ELEMENT|…|7|8|4|The declaration '_bar' isn't referenced.
#[test]
fn new_member_inside_augment_class_still_reports_unused_element() {
    let rows = unused_rows(
        "class C {\n\
         \x20 void _foo() {}\n\
         }\n\
         augment class C {\n\
         \x20 void _bar() {}\n\
         }\n\
         main() { C(); }\n",
    );
    assert!(
        rows.iter().any(|r| r.2.contains("'_bar'")),
        "`_bar` is a new element — still reports: {rows:?}"
    );
}

/// Genuinely-unused private declaration without any augment: the
/// `unused_element` verdict still reports at the written site (the
/// regression gate that the skip never swallows a real private unused).
#[test]
fn genuinely_unused_private_member_still_reports() {
    let rows = unused_rows(
        "class C {\n\
         \x20 void _orphan() {}\n\
         }\n\
         main() { C(); }\n",
    );
    assert_eq!(rows.len(), 1, "an unread `_orphan` reports: {rows:?}");
}
