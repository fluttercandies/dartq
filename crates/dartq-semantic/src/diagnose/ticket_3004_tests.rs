//! The #3004 pins — the optional-parameter default-value cluster and the
//! definite-initialization faces (upstream `error_verifier.dart`
//! `_checkUseOfDefaultValuesInParameters` /
//! `ConstructorFieldsVerifier`).
//!
//! * The constructor report span is the oracle's `errorRange`: type name
//!   through selector for a secondary head (`A.named`, `A._`), the class
//!   name (through its `.name` selector) for a primary constructor — never
//!   the `(`/`.` `begin` token the site's provisional range carries.
//! * `external` generative constructors join `hasGenerativeConstructor`
//!   (the declaration face stays off) but never report per-constructor
//!   (`_addConstructor`'s `externalKeyword` early return,
//!   constructor_fields_verifier.dart:56-59); named factories are not
//!   generative at all, so a factory-only class keeps the field-level
//!   face.
//! * `implicit_super_initializer_missing_arguments` counts required-named
//!   superclass parameters too, not only required positionals
//!   (error_verifier.dart:7225-7258).
//! * `FutureOr<T>` with a nullable argument is itself nullable
//!   (`isNullable` recurses the argument, type_system.dart:1220-1222), so
//!   `FutureOr<int?> v` carries no required-initialization face.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// The `missing_default_value_for_parameter` rows as the exact anchored
/// source text, source order.
fn mdv_spans(report: &FileReport, full: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "missing_default_value_for_parameter")
        .map(|d| {
            full[d.range.offset as usize..(d.range.offset + d.range.length) as usize].to_string()
        })
        .collect()
}

/// The rows of `code` as the exact anchored source text, source order.
fn spans_of<'r>(report: &'r FileReport, full: &str, code: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| {
            full[d.range.offset as usize..(d.range.offset + d.range.length) as usize].to_string()
        })
        .collect()
}

/// A non-nullable optional formal without a default reports at the formal's
/// name; a nullable one or a `required`-marked one stays silent.
#[test]
fn optional_formal_needs_default_only_when_potentially_non_nullable() {
    let fixture = concat!(
        "void a([int x]) {}\n",
        "void b([int? x]) {}\n",
        "void c({required int x}) {}\n",
        "void d({int x = 0}) {}\n",
        "void e([int x = 3]) {}\n",
    );
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(mdv_spans(&report, &full), vec!["x".to_string()]);
}

/// A named secondary constructor's `final_not_initialized_constructor` row
/// anchors `A.named` — the type name through the selector
/// (ConstructorDeclaration.errorRange), never the head token alone.
#[test]
fn named_secondary_constructor_reports_through_selector() {
    let fixture = "class A { final int v; A.named() {} }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "final_not_initialized_constructor"),
        vec!["A.named".to_string()]
    );
}

/// The redirecting chain's still-uninitialized target reports at `A._` —
/// the unnamed-selector head's full type-name-through-selector extent.
#[test]
fn redirecting_target_reports_through_selector() {
    let fixture = concat!(
        "class A {\n",
        "  final int v;\n",
        "  A() : this._();\n",
        "  A._();\n",
        "}\n",
    );
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "final_not_initialized_constructor"),
        vec!["A._".to_string()]
    );
}

/// A primary constructor's `errorRange` is the class name itself — the
/// provisional site's `(` token is not the anchor.
#[test]
fn primary_constructor_anchors_class_name() {
    let fixture = "class A() { final int v; }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "final_not_initialized_constructor"),
        vec!["A".to_string()]
    );
}

/// A named primary constructor's `errorRange` is the class name through
/// its selector — `B.n`, never the `.` token the site carries.
#[test]
fn named_primary_constructor_anchors_through_selector() {
    let fixture = "class B.n() { final int v; }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "final_not_initialized_constructor"),
        vec!["B.n".to_string()]
    );
}

/// `external` generative constructors join the generative set (the
/// field-level declaration face stays off) but never join the
/// per-constructor report — upstream `_addConstructor` skips
/// `externalKeyword` heads outright.
#[test]
fn external_constructor_stays_silent() {
    let fixture = concat!(
        "class A { final int v; external A(); }\n",
        "class B { final int v; external B.named(); }\n",
    );
    let report = report_ops(fixture);
    assert!(
        report.diagnostics.is_empty(),
        "external ctors keep both faces off: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

/// A factory-only class keeps the declaration face: a named factory is not
/// generative (`isGenerative` on a factory element is false), so the field
/// reports `final_not_initialized` at its own name.
#[test]
fn factory_only_class_keeps_declaration_face() {
    let fixture = "class C { final int v; factory C.named() => throw 0; }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "final_not_initialized"),
        vec!["v".to_string()]
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name != "final_not_initialized_constructor"),
        "no generative ctor may report: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

/// Required-NAMED superclass formals count the same as required
/// positionals for the implicit-super face
/// (error_verifier.dart:7225-7258) — `D()` invokes `C()` implicitly and the
/// named `required x` goes unsupplied.
#[test]
fn implicit_super_counts_required_named() {
    let fixture = concat!(
        "class C { C({required int x}); }\n",
        "class D extends C { D(); }\n",
        "class E { E(int a); }\n",
        "class F extends E { F(); }\n",
    );
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "implicit_super_initializer_missing_arguments"),
        vec!["D".to_string(), "F".to_string()]
    );
}

/// `FutureOr<int?>` is a nullable type — `isNullable` recurses the
/// argument (type_system.dart:1220-1222): no required-initialization face
/// on the instance field, the static field, or the top-level variable.
#[test]
fn future_or_nullable_argument_stays_silent() {
    let fixture = concat!(
        "class A {\n",
        "  FutureOr<int?> a;\n",
        "  FutureOr<int> b;\n",
        "  static FutureOr<int?> s;\n",
        "}\n",
        "FutureOr<int?> t;\n",
    );
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "not_initialized_non_nullable_instance_field"),
        vec!["b".to_string()],
        "only the non-nullable FutureOr<int> reports"
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name != "not_initialized_non_nullable_variable"),
        "the nullable FutureOr arms stay silent: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

/// An initialized field never reports: the declaration face and the
/// verifier face both release the latch.
#[test]
fn initialized_field_stays_silent() {
    let fixture = concat!(
        "class A { final int v = 0; A(); }\n",
        "class B { final int v; B(this.v); }\n",
        "class C { int v; C() : v = 0; }\n",
    );
    let report = report_ops(fixture);
    assert!(
        report.diagnostics.is_empty(),
        "every init path releases the latch: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}
