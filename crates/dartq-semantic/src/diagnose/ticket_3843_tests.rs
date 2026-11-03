//! #3843 — the formal-parameter type pair's judgment inputs
//! (`augmentation_formal_parameter_type_mismatch`).
//!
//! Upstream Dart SDK 3.13.7, `pkg/analyzer/lib/src/generated/
//! error_verifier.dart:2878-2902` (`checkFormalParameterPair`):
//!
//! ```text
//! var actualType = formalParameter.explicitFragmentType;   // the AUG's own
//! if (actualType != null) {                                // annotation only
//!   var expectedType = firstParameter.element.type;        // the INTRO's
//!   if (actualType is InvalidType || expectedType is InvalidType) return;
//!   if (actualType != expectedType) report(...);
//! }
//! ```
//!
//! Three consequences this file pins, each probed against the pinned oracle
//! (`dart analyze --format=machine`, package carrying
//! `enable-experiment: [augmentations]`):
//!
//! ```text
//! void f(Foo x);      + augment void f(int x) {}   silent (expected InvalidType)
//! void f(int x);      + augment void f(Bar x) {}   silent (actual InvalidType)
//! void f(dynamic p);  + augment void f(Object? p) {}
//!   augmentation_formal_parameter_type_mismatch @ `Object?` ('Object?' vs 'dynamic')
//! class C { int x; C(x); } + augment class C { augment C(int this.x); }
//!   augmentation_formal_parameter_type_mismatch @ `int` ('int' vs 'dynamic')
//!   — the intro formal is a PLAIN `x`: sharing a field's spelling does not
//!   make its element type the field type
//! class C<T> { void foo(T x); } + augment class C<T> { augment void foo(T x) {} }
//!   silent (one type parameter, both faces)
//! class C<T> { void foo(T x); } + augment class C<T> { augment void foo(int x) {} }
//!   augmentation_formal_parameter_type_mismatch @ `int` ('int' vs 'T')
//! ```
//!
//! The positional `super.x` park (`Ty::Never`, no written annotation) is the
//! one arm that stays silent here while the oracle reports: its element type
//! is the ASSOCIATED super-constructor parameter's, a cross-container lookup
//! owned by the invocation layer (`associated_super_formal_ty`,
//! `diagnose/invocation.rs`), which this pass has no handle on.

use crate::FileReport;
use crate::diagnose::test_rig::{report_ops_augment_experiments, report_ops_experiments_with};
use dartq_language::Feature;

const FTM: &str = "augmentation_formal_parameter_type_mismatch";

/// The two-unit rig, experiment on, reading the AUGMENTING unit's report.
fn aug(main: &str, augment: &str) -> FileReport {
    report_ops_augment_experiments(main, augment, &[Feature::Augmentations])
}

/// The single-unit rig, experiment on.
fn same(source: &str) -> FileReport {
    report_ops_experiments_with(source, &[], None, &[Feature::Augmentations])
}

/// The fixture's rows of one code as `(offset, length)`, ascending.
fn rows(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    let mut out = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect::<Vec<_>>();
    out.sort_unstable();
    out
}

/// Every produced row, for failure output: (code, offset, len, message).
fn dump(report: &FileReport) -> Vec<(&'static str, u32, u32, &str)> {
    report
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.name,
                d.range.offset,
                d.range.length,
                d.message.as_str(),
            )
        })
        .collect()
}

/// A fixture offset in the AUGMENTING unit (the rig mounts its source
/// verbatim).
fn at_aug(source: &str, needle: &str) -> u32 {
    source.find(needle).expect("anchor") as u32
}

/// The single message a row carries, verbatim.
fn message(report: &FileReport, code: &str) -> String {
    report
        .diagnostics
        .iter()
        .find(|d| d.code.name == code)
        .map(|d| d.message.to_string())
        .unwrap_or_default()
}

// ── the InvalidType bail, both faces ─────────────────────────────────────
// Upstream returns on `actualType is InvalidType || expectedType is
// InvalidType`: neither direction is a mismatch row.

#[test]
fn unresolvable_expected_face_is_silent() {
    // Oracle 3.13.7: only `undefined_class` on the intro's `Foo` — the
    // formal pair reports nothing (the intro's element type is InvalidType).
    let main = "void f(Foo x);\n";
    let augment = "augment void f(int x) {}\n";
    let report = aug(main, augment);
    assert_eq!(
        rows(&report, FTM),
        Vec::<(u32, u32)>::new(),
        "{:?}",
        dump(&report)
    );
}

#[test]
fn unresolvable_actual_face_is_silent() {
    // Oracle 3.13.7: only `undefined_class` on the augmentation's `Bar`.
    let main = "void f(int x);\n";
    let augment = "augment void f(Bar x) {}\n";
    let report = aug(main, augment);
    assert_eq!(
        rows(&report, FTM),
        Vec::<(u32, u32)>::new(),
        "{:?}",
        dump(&report)
    );
}

#[test]
fn unresolvable_faces_stay_silent_in_one_unit() {
    let source = "void f(Foo x);\naugment void f(Bar x) {}\n";
    let report = same(source);
    assert_eq!(
        rows(&report, FTM),
        Vec::<(u32, u32)>::new(),
        "{:?}",
        dump(&report)
    );
}

// ── the positional `super.x` park ────────────────────────────────────────
// `collector.rs` parks an unwritten positional `super.p` formal as
// `Ty::Never`; the element type is the associated super-constructor
// parameter's. Comparing the park would print a `Never` no source wrote
// (`'String?' must be the same as ... 'Never'`) — the suppressed direction
// is the honest one until the association is reachable from this pass.

#[test]
fn positional_super_formal_park_never_prints_never() {
    let main = "class A {\n  A([int? p]);\n}\n\
                class B extends A {\n  B([super.p]);\n}\n";
    let augment = "augment class B {\n  augment B([String? super.p]);\n}\n";
    let report = aug(main, augment);
    assert_eq!(
        rows(&report, FTM),
        Vec::<(u32, u32)>::new(),
        "{:?}",
        dump(&report)
    );
    assert!(
        !message(&report, FTM).contains("Never"),
        "{:?}",
        dump(&report)
    );
}

// ── the judged faces ─────────────────────────────────────────────────────

#[test]
fn plain_formal_sharing_a_field_name_stays_dynamic() {
    // `C(x)` is a PLAIN formal: the `int x` field does not lend it a type
    // (`A(this.p1)` would — the field-formal arm — but that shape's element
    // type is not reachable from this pass yet).
    let main = "class C {\n  int x;\n  C(x);\n}\n";
    let augment = "augment class C {\n  augment C(int this.x);\n}\n";
    let report = aug(main, augment);
    assert_eq!(
        rows(&report, FTM),
        vec![(at_aug(augment, "int this.x"), 3)],
        "{:?}",
        dump(&report)
    );
    assert_eq!(
        message(&report, FTM),
        "The augmentation's formal parameter type 'int' must be the same as \
         the declaration's formal parameter type 'dynamic'."
    );
}

#[test]
fn written_dynamic_versus_object_question_reports() {
    // `dynamic` and `Object?` are the same top type semantically but
    // different `DartType` instances — upstream's `!=` fires.
    let main = "void f(dynamic p);\n";
    let augment = "augment void f(Object? p) {}\n";
    let report = aug(main, augment);
    assert_eq!(
        rows(&report, FTM),
        vec![(at_aug(augment, "Object?"), 7)],
        "{:?}",
        dump(&report)
    );
    assert_eq!(
        message(&report, FTM),
        "The augmentation's formal parameter type 'Object?' must be the same as \
         the declaration's formal parameter type 'dynamic'."
    );
}

#[test]
fn shared_type_parameter_pair_is_silent() {
    // Both faces spell the container's own `T`: one signature variable.
    let main = "class C<T> {\n  void foo(T x);\n}\n";
    let augment = "augment class C<T> {\n  augment void foo(T x) {}\n}\n";
    let report = aug(main, augment);
    assert_eq!(
        rows(&report, FTM),
        Vec::<(u32, u32)>::new(),
        "{:?}",
        dump(&report)
    );
}

#[test]
fn type_parameter_versus_int_reports() {
    let main = "class C<T> {\n  void foo(T x);\n}\n";
    let augment = "augment class C<T> {\n  augment void foo(int x) {}\n}\n";
    let report = aug(main, augment);
    assert_eq!(
        rows(&report, FTM),
        vec![(at_aug(augment, "int x"), 3)],
        "{:?}",
        dump(&report)
    );
    assert_eq!(
        message(&report, FTM),
        "The augmentation's formal parameter type 'int' must be the same as \
         the declaration's formal parameter type 'T'."
    );
}
