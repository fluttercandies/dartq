//! #3423 — the operator face of a potentially nullable receiver consults
//! the applicable extension BEFORE the nullability verdict.
//!
//! The nullable branch of the property consult runs the `Object` face, then
//! the applicable-extension face, and only then reads the dereference
//! (upstream Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
//! type_property_resolver.dart:98-110). `int? x; x + null` over
//! `extension on int? { int operator +(Null o) }` therefore resolves through
//! the extension and reports nothing — neither
//! `unchecked_use_of_nullable_value` nor `undefined_operator`/
//! `argument_type_not_assignable`. The fold skipped that ordering: it read
//! the head's interface face (`int.+` from the fixture prelude) and reported
//! the argument against ITS formal, while the extension's declared return is
//! also never numerically refined
//! (pkg/analyzer/lib/src/dart/element/type_system.dart:2266-2277 answers the
//! declared type unchanged for an `ExtensionElement` encloser).
//!
//! Oracle provenance: `dart analyze` 3.13.1, probed per shape before the
//! fix; every expectation below is a live `<probe>` run, not a derivation.

use super::test_rig::{assert_silent, code_names, report_ops, rows};
use super::*;

/// The report's code names in row-key order (`(severity, code, file)` is
/// set-shaped, so the emission order is not part of the expectation).
fn sorted_names(report: &FileReport) -> Vec<&'static str> {
    let mut names = code_names(report);
    names.sort_unstable();
    names
}

/// Shape (a): the covering `on int?` face owns `x + null` over an `int?`
/// receiver. The argument checks against the extension's `Null` formal (the
/// member face's `int` formal would reject it) and the node folds to the
/// declared `int` — so the assignment below is clean too. Probed silent.
#[test]
fn nullable_extension_operator_is_silent() {
    let report = report_ops(
        "extension on int? { int operator +(Null o) => 0; } \
         void f() { int? x; int y = x + null; y + 1; }\n",
    );
    assert_silent(&report);
}

/// Shape (a2): the same face, inspected through a mismatched argument. The
/// row names the extension's `Null` formal — proof the extension face (not
/// the receiver's `int` member face) answered the signature.
#[test]
fn nullable_extension_formal_answers_the_argument() {
    let report = report_ops(
        "extension on int? { int operator +(Null o) => 0; } \
         void f() { int? x; x + 1; }\n",
    );
    assert_eq!(
        sorted_names(&report),
        vec!["argument_type_not_assignable"],
        "{:?}",
        rows(&report)
    );
}

/// Shape (b): no covering extension — the oracle's pair stands: the
/// dereference row at the operator and the recovery face's argument row
/// (`int?`'s non-null interface declares `+(int)`).
#[test]
fn nullable_receiver_without_extension_reports_the_pair() {
    let report = report_ops("void f() { int? x; x + null; }\n");
    assert_eq!(
        sorted_names(&report),
        vec![
            "argument_type_not_assignable",
            "unchecked_use_of_nullable_value"
        ],
        "{:?}",
        rows(&report)
    );
}

/// Shape (e), upstream control: a NON-nullable `on` type does not resolve
/// ahead of the verdict — `int?` is not a subtype of `int`, so
/// `applicableTo` rejects the face and the dereference row stays
/// (`UnsafeSingle` is not the covering verdict). The extension's own
/// operator is then unreferenced, and the oracle reports that too (probed:
/// `unused_element` beside the pair).
#[test]
fn non_nullable_on_extension_keeps_the_dereference_row() {
    let report = report_ops(
        "extension on int { int operator +(Null o) => 0; } \
         void f() { int? x; x + null; }\n",
    );
    assert_eq!(
        sorted_names(&report),
        vec![
            "argument_type_not_assignable",
            "unchecked_use_of_nullable_value",
            "unused_element"
        ],
        "{:?}",
        rows(&report)
    );
}

/// Shape (c), control: a non-nullable receiver keeps the plain argument row
/// — the ordering arm changes nothing where no nullability applies.
#[test]
fn non_nullable_receiver_keeps_the_argument_row() {
    let report = report_ops("void f() { int x = 1; x + null; }\n");
    assert_eq!(
        sorted_names(&report),
        vec!["argument_type_not_assignable"],
        "{:?}",
        rows(&report)
    );
}

/// Shape (d): the compound twin. The oracle is silent here — probed:
/// `extension on int? { int operator +(Null o) }` over `int? x; x += null`
/// reports nothing, because the augmented read resolves the extension's
/// operator and `_refineNumericInvocationTypeNullSafe` leaves an
/// extension-declared return alone.
///
/// Two faces outside this ticket still diverge, measured on this fixture:
///
/// - `row_resolution.rs`'s compound operator face emits
///   `unchecked_use_of_nullable_value` (the `Method` spelling of `+`)
///   because its suppression gate reads `operator_face_extension_resolved`,
///   which spells the operator through `operator_spellings` — and a compound
///   read writes no `Intent::Operator`, so the gate sees no spelling and the
///   covering face never suppresses. The spelling to probe is the one
///   `adjacent_operator` already answered.
/// - `assignability.rs`'s `check_compound_assignment` runs
///   `refine_numeric_binary` over the extension-declared return, so `int?` /
///   `Null` refine to `double` and the write-back stages `invalid_assignment`.
///   The refinement must be skipped whenever the signature came from an
///   extension (the ordering arm reads it from
///   `Engine::nullable_extension_operator_sig`).
// un-ignored 2026-11-07: passing per stale-ignore triage (ticket #3973)
#[test]
fn compound_nullable_extension_operator_is_silent() {
    let report = report_ops(
        "extension on int? { int operator +(Null o) => 0; } \
         class C { int? x; void f() { x += null; } }\n",
    );
    assert_silent(&report);
}
