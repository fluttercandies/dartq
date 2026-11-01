//! The #3557 pins: the augmentation gate of `enum_with_abstract_member` and
//! the chain-aware `missing_default_value_for_parameter*` rows.
//!
//! Oracle anchors (dart 3.13.7, the pinned oracle — upstream Dart SDK
//! source):
//!
//! * `enum_with_abstract_member` — `_reportConcreteClassWithAbstractMember`
//!   (pkg/analyzer/lib/src/error/inheritance_override.dart:909-951) walks
//!   the interface map; an element whose only implementation is an
//!   `augment` fragment without an introductory declaration is skipped
//!   (`isAugmentationWithoutAugmentedDeclaration`, :431-435), so a member
//!   written `augment` never reaches the row while the feature is on. With
//!   the feature off the keyword never lexes and the member reports as an
//!   ordinary abstract declaration. Oracle rows:
//!   `enum E { v; void foo(); }` -> 1 row; `enum E { v; augment void
//!   foo(); }` (experiment on) -> 0 rows.
//!
//! * `missing_default_value_for_parameter(_positional)` —
//!   `_checkUseOfDefaultValuesInParameters` reads the merged element's
//!   `parameterElement.hasDefaultValue` (pkg/analyzer/lib/src/generated/
//!   error_verifier.dart:7964-8036): a default written on ANY fragment of
//!   the augmentation chain silences the row on EVERY fragment. Oracle
//!   rows: `void f([int a]); augment void f([int a]) {}` -> 2 rows (one
//!   per fragment's parameter); `void f([int a]); augment void f([int a =
//!   0]) {}` -> 0 rows (the augment's default settles the chain);
//!   `void f([int a = 0]); augment void f([int a]) {}` -> 0 rows (the
//!   introduction's default settles it).

use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops_experiments_with};
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

// —— enum_with_abstract_member: the augment gate ————————————————————————

/// Oracle: 1 row at the whole `void foo();` declaration.
#[test]
fn enum_with_abstract_member_reports_the_plain_abstract_member() {
    let source = "enum E {\n  v;\n  void foo();\n}\n";
    let report = with_augmentations(source);
    assert_eq!(
        rows(&report, "enum_with_abstract_member"),
        vec![(at(source, "void foo"), 11)]
    );
}

/// The silent twin: the member carries `augment`, so it is a chain fragment
/// with no introduction — the oracle emits nothing (0 rows).
#[test]
fn enum_with_abstract_member_skips_the_augment_member() {
    let source = "enum E {\n  v;\n  augment void foo();\n}\n";
    let report = with_augmentations(source);
    assert_eq!(rows(&report, "enum_with_abstract_member"), vec![]);
}

// —— missing_default_value_for_parameter*: the chain verdict ————————————

/// Oracle: 2 rows — neither fragment supplies a default, so both the
/// introduction's and the augmentation's parameter report.
#[test]
fn missing_default_reports_both_fragments_without_a_chain_default() {
    let source = "void f([int a]);\n\naugment void f([int a]) {}\n";
    let report = with_augmentations(source);
    assert_eq!(
        rows(&report, "missing_default_value_for_parameter"),
        vec![(at(source, "a]"), 1), (at(source, "a]) {}"), 1)]
    );
}

/// The silent twin: the augment's `= 0` settles the merged element's
/// `hasDefaultValue`, so the introduction's parameter stays silent too
/// (0 rows).
#[test]
fn missing_default_silences_the_chain_when_an_augment_supplies_one() {
    let source = "void f([int a]);\n\naugment void f([int a = 0]) {}\n";
    let report = with_augmentations(source);
    assert_eq!(
        rows(&report, "missing_default_value_for_parameter"),
        vec![]
    );
}

/// The introduction's own default settles the chain the same way (0 rows).
#[test]
fn missing_default_silences_the_augment_when_the_intro_supplies_one() {
    let source = "void f([int a = 0]);\n\naugment void f([int a]) {}\n";
    let report = with_augmentations(source);
    assert_eq!(
        rows(&report, "missing_default_value_for_parameter"),
        vec![]
    );
}

/// Bisect: a plain `;`-bodied top-level function still expects defaults —
/// the deferral drains to the same single row the ungated path emits.
#[test]
fn missing_default_reports_the_plain_bodyless_function() {
    let source = "void f([int a]) {}\n";
    let report = with_augmentations(source);
    assert_eq!(
        rows(&report, "missing_default_value_for_parameter"),
        vec![(at(source, "a]"), 1)]
    );
}
