//! Ticket #3678 pins (census-12 lane K): the primary-constructor
//! field-initializer argument check — an initializing `this.x`/`super.x`
//! formal has no written annotation, so the element model's `ctor_formals`
//! parks `Unknown`. The `new`/`const` keyword path
//! (`check_explicit_creation_arguments`) never patched the formals the way
//! the send path already did (`Bare(BareInfo::Container)` arm,
//! invocation.rs:11848), leaving the `argument_type_not_assignable` question
//! undecidable — `A?` into `A` stayed silent (co19
//! `Primary-constructors/static_processing_A34_t05..t08`, census-12
//! `argument_type_not_assignable` family shape 2.3).
//!
//! Oracle: `dart analyze` 3.13.7 — the primary-constructor formals' type is
//! the field's declared type (`FieldFormalParameterElement.type`; upstream
//! Dart SDK source,
//! pkg/analyzer/lib/src/generated/error_detection_helpers.dart:72-77 —
//! `argument.correspondingParameter.type` is the element's own type, no
//! special-casing for the field-formal arm). The fix mirrors
//! `forwarded_ctor_formals`'s existing `patch_field_formals` call
//! (invocation.rs:2250-2252) onto the explicit-keyword path.

use super::test_rig::{report_ops_experiments_with, rows};
use dartq_language::Feature;

fn code_count(report: &crate::diagnose::FileReport, code: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .count()
}

fn argument_type_not_assignable(report: &crate::diagnose::FileReport) -> usize {
    code_count(report, "argument_type_not_assignable")
}

/// The primary-constructor field-declaration initializer: `[final A? a]`'s
/// induced field type `A?` checks against `B`'s `this.a` formal `A` — the
/// oracle reports `argument_type_not_assignable` beside the non-constant
/// capture (`const_with_non_constant_argument`).
#[test]
fn primary_ctor_field_initializer_nullable_arg_reports() {
    let report = report_ops_experiments_with(
        "class A { const A(); }\n\
         class B { final A a; const B(this.a); }\n\
         class const C3([final A? a]) {\n\
         \x20 final B? b = const B(a);\n\
         }\n",
        &[],
        None,
        &[Feature::PrimaryConstructors],
    );
    assert_eq!(
        argument_type_not_assignable(&report),
        1,
        "the `this.a` field-formal reads its field's declared type `A`: {:?}",
        rows(&report)
    );
    assert_eq!(
        code_count(&report, "const_with_non_constant_argument"),
        1,
        "the non-constant argument capture still reports beside it: {:?}",
        rows(&report)
    );
}

/// The `this :` body arm: the primary constructor's own initializer list —
/// the field formal's `A?` answer reaches the `const B(a)` argument check
/// inside it (the `this : b = const B(a)` shape of `static_processing_A34_t05`).
#[test]
fn primary_ctor_this_body_nullable_arg_reports() {
    let report = report_ops_experiments_with(
        "class A { const A(); }\n\
         class B { final A a; const B(this.a); }\n\
         class const C3([final A? a]) {\n\
         \x20 final B? b;\n\
         \x20 this : b = const B(a);\n\
         }\n",
        &[],
        None,
        &[Feature::PrimaryConstructors],
    );
    assert_eq!(
        argument_type_not_assignable(&report),
        1,
        "the `this :` body's field-initializer argument check fires: {:?}",
        rows(&report)
    );
}

/// The `new` keyword arm (`new B(a)`): the explicit-`new` creation shares
/// `check_explicit_creation_arguments` — the field-formal patch joins it on
/// the same gate.
#[test]
fn new_creation_nullable_arg_reports() {
    let report = report_ops_experiments_with(
        "class A {}\n\
         class B { final A a; B(this.a); }\n\
         void f(A? x) { final y = new B(x); }\n",
        &[],
        None,
        &[Feature::PrimaryConstructors],
    );
    assert_eq!(
        argument_type_not_assignable(&report),
        1,
        "the `new` keyword path answers the `this.a` formal `A`: {:?}",
        rows(&report)
    );
}

/// Negative control — a matching non-null argument stays silent: `B(x)`
/// where `x : A` is assignable to `this.a`'s formal `A`.
#[test]
fn matching_non_null_arg_stays_silent() {
    let report = report_ops_experiments_with(
        "class A { const A(); }\n\
         class B { final A a; const B(this.a); }\n\
         void f(A x) { final y = const B(x); }\n",
        &[],
        None,
        &[Feature::PrimaryConstructors],
    );
    assert_eq!(
        argument_type_not_assignable(&report),
        0,
        "a non-null argument never reports: {:?}",
        rows(&report)
    );
}

/// Negative control — the patch does not re-type a formal that already
/// carries a written annotation: `B(this.a)` where the class's `a` field is
/// `A` but the formal spells `String a` — `A?` into `String` rejects on the
/// written annotation alone (the field's `A` never shadows the written type).
#[test]
fn written_annotation_on_field_formal_keeps_its_type() {
    let report = report_ops_experiments_with(
        "class A { const A(); }\n\
         class B { final String a; B(this.a); }\n\
         void f(A? x) { final y = B(x); }\n",
        &[],
        None,
        &[Feature::PrimaryConstructors],
    );
    // `this.a`'s formal has no written annotation — the field's `String` is
    // the answer: `A?` into `String` rejects. (The written-annotation gate is
    // upstream's own rule — `FieldFormalParameterElement.type` is the
    // field's declared type only when the formal spells no type.)
    assert_eq!(
        argument_type_not_assignable(&report),
        1,
        "the field formal's declared `String` still answers: {:?}",
        rows(&report)
    );
}
