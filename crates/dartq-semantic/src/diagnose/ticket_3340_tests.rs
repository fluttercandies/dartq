//! #3340 pins — the declaration-initialization faces meet augmentation.
//!
//! Upstream keys `ConstructorFieldsVerifier` state per ELEMENT
//! (`_Interface.constructors` is a `Map.identity` and `forConstructor`
//! caches with `??=`), so an `augment` constructor merges into the
//! introductory element: the merged row is reported once, at the FIRST
//! declaration's `errorRange` (`_addConstructor` keeps the range of the
//! call that created the state). The flush here runs once per DECLARATION
//! (each container end), so every pass must scope itself to its own
//! declaration — otherwise the merged row is reported again at the augment
//! head and the intro rows are emitted twice.
//!
//! Pins below:
//!
//! * an `augment` block that declares no constructor leaves the intro
//!   rows alone (no second emission from the augment's own flush);
//! * an augmenting constructor reports at the introductory head, never at
//!   its own;
//! * the enum's own `index` field carries no required-initialization
//!   obligation (`_forInterface` skips `EnumElement.index`).

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// The rows of `code` as the exact anchored source text, source order.
fn spans_of(report: &FileReport, full: &str, code: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| {
            full[d.range.offset as usize..(d.range.offset + d.range.length) as usize].to_string()
        })
        .collect()
}

/// An extension type's representation faces one row per generative
/// constructor, and an `augment` block that declares nothing of its own
/// must not re-run the already-decided intro rows.
#[test]
fn extension_type_augment_block_does_not_repeat_intro_rows() {
    let fixture = concat!(
        "extension type ET(int? v) {\n",
        "  ET.foo(int _);\n",
        "  ET.bar([int _]);\n",
        "}\n",
        "augment extension type ET {\n",
        "}\n",
    );
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "final_not_initialized_constructor"),
        vec!["ET.foo".to_string(), "ET.bar".to_string()],
        "each constructor reports once, at its own head; the augment adds none"
    );
}

/// The same shape without the augment block: the plain pass is unchanged
/// (the scoping must not silence a single-declaration container).
#[test]
fn extension_type_without_augment_still_reports_per_constructor() {
    let fixture = concat!(
        "extension type ET(int? v) {\n",
        "  ET.foo(int _);\n",
        "  ET.bar([int _]);\n",
        "}\n",
    );
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "final_not_initialized_constructor"),
        vec!["ET.foo".to_string(), "ET.bar".to_string()],
    );
}

/// An augmenting constructor merges into the introductory element, whose
/// `errorRange` is the first declaration's: the row anchors at `K`, never
/// at the augmenting head.
#[test]
fn augmenting_constructor_reports_at_the_intro_head() {
    let fixture = concat!(
        "class K {\n",
        "  final int v;\n",
        "  K();\n",
        "}\n",
        "augment class K {\n",
        "  augment K();\n",
        "}\n",
    );
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "final_not_initialized_constructor"),
        vec!["K".to_string()],
        "one merged row at the introductory constructor"
    );
}

/// The enum's own `index` field is outside the required-initialization
/// set (`_forInterface`'s `EnumElement` carve-out), while a real final
/// field of the same enum still reports.
#[test]
fn enum_index_field_carries_no_initialization_obligation() {
    let fixture = concat!(
        "enum E1 {\n",
        "  e1;\n",
        "  const E1();\n",
        "  final int index;\n",
        "}\n",
    );
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert!(
        spans_of(&report, &full, "final_not_initialized_constructor").is_empty(),
        "the implicit enum index owns no initializer: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

/// A covered field is silent: the initializer list releases it for the
/// constructor that writes it, and the enum carve-out never widens to
/// other fields.
#[test]
fn initializer_list_coverage_stays_silent() {
    let fixture = concat!("class C {\n", "  final int v;\n", "  C() : v = 1;\n", "}\n",);
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert!(
        spans_of(&report, &full, "final_not_initialized_constructor").is_empty(),
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}
