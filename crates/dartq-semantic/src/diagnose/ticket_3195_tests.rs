//! #3195 — the extension-type representation-field faces: what the tree
//! answers today, pinned green, plus the recorded gaps.
//!
//! Probed against oracle 3.13.7 (`/tmp/t3195/v310.dart`, `vdef.dart`):
//!
//! ```text
//! // @dart=3.10                       oracle            dartq (before)
//! extension type E(it) {}             EXPECTED_REPRESENTATION_TYPE @name   MISSING
//! extension type E(var it) {}         REPRESENTATION_FIELD_MODIFIER @var   ok
//!                                     EXPECTED_REPRESENTATION_TYPE @name   MISSING
//! extension type E(final it) {}       (same pair)                          half
//! extension type E(it,) {}            EXPECTED_REPRESENTATION_TYPE @name   MISSING
//!                                     REPRESENTATION_FIELD_TRAILING_COMMA @, MISSING
//! extension type E(int it) {}         clean                                clean
//! ```
//! At the current language version the untyped spellings are legal
//! declaring formals and the oracle is silent — that half is pinned here.
//!
//! The two gaps are NOT landable inside the emit layer: both need a new
//! `SemanticCode` (missing from `codes.rs`, `factory_without_body` and
//! `representation_field_trailing_comma` are absent from the whole
//! registry — the third code of the hand-up, `factory_without_body`, is a
//! CLASS-factory row in the corpus, not an extension-type one), and the
//! `expected_representation_type` arm that already exists is gated on
//! `rep.formal.ty == Ty::UNKNOWN && !primary_constructors_enabled` and
//! never fires even at `@dart=3.10` where the flag is provably off
//! (`class A(int x)` reports `experiment_not_enabled` there).

use super::test_rig::{report_ops, report_ops_bare};
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

/// The modifier face fires at the keyword at `@dart=3.10` — the anchor the
/// type row sits beside (`3:19` modifier, `3:23` name in the oracle).
#[test]
fn var_modified_representation_reports_the_modifier_at_the_keyword() {
    let source = "// @dart=3.10\n\
                  extension type E(var it) {}\n";
    let report = report_ops_bare(source);
    assert_eq!(
        offsets(&report, "representation_field_modifier"),
        vec![(source.find("var it").expect("anchor")) as u32],
        "{:?}",
        report.diagnostics
    );
}

/// A typed representation stays row-free at the old language version.
#[test]
fn typed_representation_is_silent() {
    let report = report_ops("// @dart=3.10\nextension type E(int it) {}\n");
    assert!(
        offsets(&report, "expected_representation_type").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// At the current language version an untyped representation formal is a
/// legal declaring formal — the oracle emits nothing, and neither may we.
#[test]
fn typeless_representation_is_silent_at_the_current_version() {
    let report = report_ops("extension type E(it) {}\n");
    assert!(
        offsets(&report, "expected_representation_type").is_empty(),
        "{:?}",
        report.diagnostics
    );
}
