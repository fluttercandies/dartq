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

use crate::FileReport;
use crate::diagnose::test_rig::{
    CORE_PRELUDE, report_ops, report_ops_augment_intro_experiments, report_ops_experiments_with,
};
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

/// The chain arm (#3203): an introductory factory plus a body-less
/// `augment` leaves two incomplete fragments, so the sibling code fires at
/// the same head with the selector named (oracle 3.13.7: "The factory
/// constructor 'named' must have a body or redirection after all
/// augmentations are applied.").
#[test]
fn factory_not_complete_after_augmentations_names_the_selector() {
    let main = "class A {\n  factory named();\n}\n";
    let aug = "augment class A {\n  augment factory named();\n}\n";
    let report = report_ops_augment_intro_experiments(main, aug, &[Feature::Augmentations]);
    assert_eq!(
        rows(&report, "factory_not_complete_after_augmentations"),
        vec![(at(main, "factory named"), 13)],
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (
                d.code.name,
                d.range.offset,
                d.range.length,
                d.message.as_str()
            ))
            .collect::<Vec<_>>()
    );
    let message = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "factory_not_complete_after_augmentations")
        .map(|d| d.message.to_string())
        .expect("row");
    assert!(
        message.contains("'named'") && message.ends_with("augmentations are applied."),
        "{message}"
    );
}

/// The unnamed face reads `new` (probe: `factory A();` + augment).
#[test]
fn factory_not_complete_after_augmentations_names_new() {
    let main = "class A {\n  factory A();\n}\n";
    let aug = "augment class A {\n  augment factory A();\n}\n";
    let report = report_ops_augment_intro_experiments(main, aug, &[Feature::Augmentations]);
    let message = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "factory_not_complete_after_augmentations")
        .map(|d| d.message.to_string())
        .expect("row");
    assert!(message.contains("'new'"), "{message}");
    assert_eq!(
        rows(&report, "factory_not_complete_after_augmentations"),
        vec![(at(main, "factory A()") + 8, 1)],
        "{:?}",
        report.diagnostics
    );
}

/// An `augment` that supplies a body completes the chain — no row at all.
#[test]
fn completing_augment_leaves_no_row() {
    let main = "class A {\n  factory named();\n}\n";
    let aug = "augment class A {\n  augment factory named() => throw 0;\n}\n";
    let report = report_ops_augment_intro_experiments(main, aug, &[Feature::Augmentations]);
    assert!(
        rows(&report, "factory_without_body").is_empty()
            && rows(&report, "factory_not_complete_after_augmentations").is_empty(),
        "{:?}",
        report.diagnostics
    );
}
