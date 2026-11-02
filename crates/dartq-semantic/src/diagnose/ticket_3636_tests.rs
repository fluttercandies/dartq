//! #3636 pins: the nameless field/member anchor remap — an unnamed
//! (`<unnamed>`) field or member inside a container reports at the
//! enclosing container's declared-name token at zero length, matching the
//! oracle's `<unnamed>` element location (the parent fragment's name —
//! `class C { var }` upstream is `unused_field` at `C`, `|0`). Members of
//! one container that all collapse to that same (offset, 0) site fold to
//! ONE row through the usage tail's identical-row dedup — the oracle's
//! listener collects into a `Set<Diagnostic>`. Oracle evidence:
//! `unused_field|lib/test.dart|2:7|0` across the `field_declaration_*
//! _noName` family and `test_expectedExecutable_inClass_afterVoid` (two
//! unnamed members → one row), plus `variable_declaration_A06_t02` (seven
//! unnamed static fields → one row). Oracle: dart 3.13.7.

use super::test_rig::{CORE_PRELUDE, report_ops};
use crate::FileReport;

/// The `(offset, length)` pairs of `code` rows, source-ordered.
fn rows_of(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == code)
        .map(|row| (row.range.offset, row.range.length))
        .collect()
}

/// Byte offset of `needle`'s `nth` occurrence inside `source`, shifted
/// past [`CORE_PRELUDE`]. ASCII fixtures make byte and UTF-16 offsets
/// coincide.
fn at(source: &str, needle: &str, nth: usize) -> u32 {
    source
        .match_indices(needle)
        .nth(nth)
        .unwrap_or_else(|| panic!("`{needle}` occurrence #{nth} missing in {source:?}"))
        .0 as u32
        + CORE_PRELUDE.len() as u32
}

/// A single nameless field inside `class C` reports `unused_field` at the
/// container's name token with zero length — never at the declaration's
/// own recovered range. (`class C { var }` upstream is `unused_field` at
/// `C`, `|0`.)
#[test]
fn nameless_field_anchors_at_container_name() {
    let source = "class C { var }\n";
    let report = report_ops(source);
    assert_eq!(
        rows_of(&report, "unused_field"),
        vec![(at(source, "C", 0), 0)],
        "source: {source}"
    );
}

/// The same remap over a mixin container: `mixin M { var }` parses a
/// nameless field whose row anchors at `M`, `|0` (the mixin twin of
/// RecoveryParserTest/test_incompleteField_var). `mixin M { int ; }` is
/// no nameless face: `int` recovers as the field's NAME
/// (`missing_const_final_var_or_type`, the partial_code
/// `field_declaration_type_noName_*` family) — a public field, no row.
#[test]
fn nameless_field_anchors_at_mixin_name() {
    let source = "mixin M { var }\n";
    let report = report_ops(source);
    assert_eq!(
        rows_of(&report, "unused_field"),
        vec![(at(source, "M", 0), 0)],
        "source: {source}"
    );
}

/// A nameless method inside `class C` reports `unused_element` at the
/// container's name token with zero length — the member arm remaps the
/// same way the field arm does.
#[test]
fn nameless_method_anchors_at_container_name() {
    let source = "class C { void () {} }\n";
    let report = report_ops(source);
    assert_eq!(
        rows_of(&report, "unused_element"),
        vec![(at(source, "C", 0), 0)],
        "source: {source}"
    );
}

/// Two nameless members of one container collapse to the same anchor
/// site — the oracle's diagnostic set folds them to ONE row
/// (`test_expectedExecutable_inClass_afterVoid`'s two `missing_identifier`
/// members emit a single `unused_field` row at `C`; the
/// `variable_declaration_A06_t02` seven-member family folds to one). The
/// fixture is that parser test's own source.
#[test]
fn two_nameless_members_fold_to_one_row() {
    let source = "class C { void 2 void }\n";
    let report = report_ops(source);
    assert_eq!(
        rows_of(&report, "unused_field"),
        vec![(at(source, "C", 0), 0)],
        "source: {source}"
    );
}

/// Named members keep their own anchors — a private unread field still
/// reports at its name token, not the container's. The remap touches the
/// nameless face only. (The containers stay public so the container
/// element itself never joins the `unused_*` row set.)
#[test]
fn named_members_keep_their_own_anchors() {
    let source = "class C { int _f; }\n";
    let report = report_ops(source);
    assert_eq!(
        rows_of(&report, "unused_field"),
        vec![(at(source, "_f", 0), 2)],
        "source: {source}"
    );
    let source = "class C { void _m() {} }\n";
    let report = report_ops(source);
    assert_eq!(
        rows_of(&report, "unused_element"),
        vec![(at(source, "_m", 0), 2)],
        "source: {source}"
    );
}
