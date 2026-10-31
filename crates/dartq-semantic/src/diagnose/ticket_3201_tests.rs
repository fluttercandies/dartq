//! #3201 — `factory_without_body` (upstream
//! `_checkForFactoryBodyCompleteness`, error_verifier.dart:5078-5107).
//!
//! The only semantic-tier row of the factory chain: with the
//! `augmentations` experiment on, an introductory factory constructor whose
//! fragment chain never completes reports at its `errorRange` head. Every
//! expectation below is a verbatim oracle 3.13.7 row, probed in a package
//! carrying `enable-experiment: [augmentations]` (`/tmp/t3201/pk`):
//!
//! ```text
//! class A { factory named(); }        FACTORY_WITHOUT_BODY @ `factory named` (13)
//! class A { factory A.named(); }      FACTORY_WITHOUT_BODY @ `A.named`       (7)
//! class A { factory (); }             FACTORY_WITHOUT_BODY @ `factory`       (7)
//! class A { external factory named(); }                     silent
//! class A { factory A() = A; }                              silent (redirect)
//! ```
//!
//! The head anchors model `ConstructorDeclarationImpl.errorRange`: the
//! `typeName` token through the selector when one is written, else the
//! `factory` keyword through the name, the unnamed spellings collapsing to
//! the single token that is present.

use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops, report_ops_experiments_with};
use crate::FileReport;
use dartq_language::Feature;

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

/// The fixture diagnosed with the augmentations experiment on.
fn with_augmentations(source: &str) -> FileReport {
    report_ops_experiments_with(
        &format!("{CORE_PRELUDE}{source}"),
        &[],
        None,
        &[Feature::Augmentations],
    )
}

/// A fixture offset, shifted by the rig's one-line prelude.
fn at(source: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + source.find(needle).expect("anchor")) as u32
}

/// The base shape: a body-less factory with no written type name anchors at
/// the `factory` keyword through the selector (`factory named`).
#[test]
fn factory_without_body_anchors_at_the_factory_keyword() {
    let source = "class A {\n  factory named();\n}\n";
    let report = with_augmentations(source);
    assert_eq!(
        rows(&report, "factory_without_body"),
        vec![(at(source, "factory named"), 13)],
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range.offset, d.range.length))
            .collect::<Vec<_>>()
    );
}

/// A written type name anchors at that name through the selector
/// (`A.named`).
#[test]
fn factory_without_body_anchors_at_the_type_name() {
    let source = "class A {\n  factory A.named();\n}\n";
    let report = with_augmentations(source);
    assert_eq!(
        rows(&report, "factory_without_body"),
        vec![(at(source, "A.named"), 7)],
        "{:?}",
        report.diagnostics
    );
}

/// The unnamed spelling with no name token collapses to the `factory`
/// keyword alone (7 columns).
#[test]
fn factory_without_body_anchors_at_the_bare_keyword() {
    let source = "class A {\n  factory ();\n}\n";
    let report = with_augmentations(source);
    assert_eq!(
        rows(&report, "factory_without_body"),
        vec![(at(source, "factory ()"), 7)],
        "{:?}",
        report.diagnostics
    );
}

/// An `external` factory is complete — no row.
#[test]
fn external_factory_is_silent() {
    let source = "class A {\n  external factory named();\n}\n";
    let report = with_augmentations(source);
    assert!(
        rows(&report, "factory_without_body").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// A redirecting factory (`= A`) is complete — no row.
#[test]
fn redirecting_factory_is_silent() {
    let source = "class A {\n  factory A() = A;\n}\n";
    let report = with_augmentations(source);
    assert!(
        rows(&report, "factory_without_body").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// Without the experiment the row never fires (the parser's
/// `missing_function_body` owns that shape upstream).
#[test]
fn factory_without_body_needs_the_augmentations_experiment() {
    let source = "class A {\n  factory named();\n}\n";
    let report = report_ops(source);
    assert!(
        rows(&report, "factory_without_body").is_empty(),
        "{:?}",
        report.diagnostics
    );
}
