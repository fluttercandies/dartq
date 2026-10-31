//! #3198 — `expected_representation_type` fires on the SYNTACTIC absence
//! of the representation type.
//!
//! The oracle decides on `inner.type == null`
//! (`error_verifier.dart:5449-5452`), never on a semantic type value, and
//! the row lives in the primary-constructors-DISABLED arm only: the pins
//! therefore carry `// @dart=3.10` (the feature ships at 3.13; the
//! current-version silence is pinned in `ticket_3195_tests.rs`). Probed
//! against oracle 3.13.7 (`ExtensionTypeResolutionTest
//! .test_primaryConstructor_formalParameters_noTypeAnnotation*_language310`).
//!
//! The distinction the element model cannot make: an untyped formal is
//! materialized as `Object?` by the collector, exactly the type a written
//! `Object?` annotation spells — the pins pair those two shapes.

use super::test_rig::report_ops_bare;
use crate::FileReport;

/// The offsets of one code's rows, ascending.
fn offsets(report: &FileReport, code: &str) -> Vec<u32> {
    let mut at = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.range.offset)
        .collect::<Vec<_>>();
    at.sort_unstable();
    at
}

/// A bare formal name carries no type: the row anchors at the name token
/// (`extension type A(it) {}` → `it`).
#[test]
fn bare_representation_name_reports_at_the_name() {
    let source = "// @dart=3.10\n\
                  extension type A(x) {}\n";
    let report = report_ops_bare(source);
    assert_eq!(
        offsets(&report, "expected_representation_type"),
        vec![source.find("x)").expect("anchor") as u32],
        "{:?}",
        report.diagnostics
    );
}

/// A written type is the whole requirement, whatever it spells — `int`.
#[test]
fn written_primitive_type_is_silent() {
    let report = report_ops_bare("// @dart=3.10\nextension type A(int x) {}\n");
    assert!(
        offsets(&report, "expected_representation_type").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// `dynamic` is a written annotation too (the oracle tests the syntax,
/// not `isDynamic`).
#[test]
fn written_dynamic_is_silent() {
    let report = report_ops_bare("// @dart=3.10\nextension type A(dynamic x) {}\n");
    assert!(
        offsets(&report, "expected_representation_type").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// The sharp edge: a written `Object?` spells the type the collector
/// SYNTHESIZES for the untyped formal — silent, where `A(x)` reports.
#[test]
fn written_object_nullable_is_silent() {
    let report = report_ops_bare("// @dart=3.10\nextension type A(Object? x) {}\n");
    assert!(
        offsets(&report, "expected_representation_type").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// Metadata precedes the formal (`@deprecated it`, the corpus's
/// `..._noTypeAnnotation_withMetadata_language310`): still no type.
#[test]
fn metadata_before_the_name_still_reports() {
    let source = "// @dart=3.10\n\
                  extension type A(@deprecated x) {}\n";
    let report = report_ops_bare(source);
    assert_eq!(
        offsets(&report, "expected_representation_type"),
        vec![source.find("x)").expect("anchor") as u32],
        "{:?}",
        report.diagnostics
    );
}

/// A modifier alone does not satisfy the requirement (`final x`; the
/// modifier's own row is the parser's).
#[test]
fn modifier_without_a_type_still_reports() {
    let source = "// @dart=3.10\n\
                  extension type A(final x) {}\n";
    let report = report_ops_bare(source);
    assert_eq!(
        offsets(&report, "expected_representation_type"),
        vec![source.find("x)").expect("anchor") as u32],
        "{:?}",
        report.diagnostics
    );
}

/// A modifier plus a written type is silent (`final int x`).
#[test]
fn modifier_with_a_type_is_silent() {
    let report = report_ops_bare("// @dart=3.10\nextension type A(final int x) {}\n");
    assert!(
        offsets(&report, "expected_representation_type").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// The named primary constructor is the same law (`extension type
/// A.c(x)` — the verifier reads `namePart.formalParameters` regardless of
/// the constructor name).
#[test]
fn named_primary_constructor_bare_name_reports() {
    let source = "// @dart=3.10\n\
                  extension type A.c(x) {}\n";
    let report = report_ops_bare(source);
    assert_eq!(
        offsets(&report, "expected_representation_type"),
        vec![source.find("x)").expect("anchor") as u32],
        "{:?}",
        report.diagnostics
    );
}

/// The named primary constructor with a written type stays silent.
#[test]
fn named_primary_constructor_with_a_type_is_silent() {
    let report = report_ops_bare("// @dart=3.10\nextension type A.c(int x) {}\n");
    assert!(
        offsets(&report, "expected_representation_type").is_empty(),
        "{:?}",
        report.diagnostics
    );
}
