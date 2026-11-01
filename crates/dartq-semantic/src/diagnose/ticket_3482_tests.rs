//! The #3482 pins — the clause-element-kind check is a per-DECLARATION
//! verdict: `scan_header_clauses` keys each scanned header on its declaring
//! name token's offset (previously on the name itself, which collapsed
//! repeated declarations onto the LAST header and re-reported that one
//! entry at every declaration — `class C` written eighty times emitted the
//! surviving clause row seventy-nine extra times, the corpus's largest
//! duplicate family in `mixin_of_non_class` / `implements_non_class`).
//! The name-keyed substitution faces keep the element model's own
//! last-declaration collapse through `last_of_name`, so only the
//! per-declaration clause check changed shape.
//!
//! Every pin asserts the exact ROW COUNT — presence alone cannot catch the
//! multiplication.

use super::test_rig::report_ops;
use super::{Diagnostic, FileReport};

/// The rows of one clause code, in emission order.
fn rows_of<'a>(report: &'a FileReport, code: &str) -> Vec<&'a Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .collect()
}

/// One declaration carrying a `with` clause over an unresolvable head —
/// the baseline count, unchanged.
#[test]
fn single_with_miss_reports_once() {
    let report = report_ops("class C extends Object with Missing {}");
    let rows = rows_of(&report, "mixin_of_non_class");
    assert_eq!(rows.len(), 1, "{:?}", report.diagnostics);
}

/// Two declarations of the SAME name, each carrying the bad clause — the
/// per-declaration identity: each reports its own header exactly once.
/// Before the fix the second declaration's clauses were reported twice
/// (once per declaration) — four rows.
#[test]
fn two_same_name_with_misses_report_once_each() {
    let report = report_ops(
        "class C extends Object with Missing {} class C extends Object with Missing {}",
    );
    let rows = rows_of(&report, "mixin_of_non_class");
    // One row per declaration — two declarations, two distinct anchors.
    assert_eq!(rows.len(), 2, "{:?}", report.diagnostics);
    assert_ne!(
        rows[0].range.offset, rows[1].range.offset,
        "each declaration anchors its own entry"
    );
}

/// The corpus multiplier: eighty declarations of `class C` with the same
/// bad `with` clause report eighty rows — one per declaration — never the
/// 80 × last-header product the name-keyed map emitted.
#[test]
fn repeated_name_reports_per_declaration_not_per_name() {
    let decl = "class C extends Object with Missing {} ";
    let fixture = decl.repeat(80);
    let report = report_ops(&fixture);
    let rows = rows_of(&report, "mixin_of_non_class");
    assert_eq!(rows.len(), 80, "{:?}", report.diagnostics.len());
    // Every row anchors a DIFFERENT written entry — no two rows share a
    // span (the oracle never emits byte-identical duplicates).
    let mut offsets: Vec<u32> = rows.iter().map(|d| d.range.offset).collect();
    offsets.sort_unstable();
    offsets.dedup();
    assert_eq!(offsets.len(), 80, "distinct anchors: {:?}", offsets);
}

/// Two DIFFERENT classes each carrying the bad clause — the pin that proves
/// the key is the declaration and not "collapse all": distinct names still
/// report each header.
#[test]
fn two_distinct_classes_each_report() {
    let report = report_ops(
        "class A extends Object with Missing {} class B extends Object with Missing {}",
    );
    let rows = rows_of(&report, "mixin_of_non_class");
    assert_eq!(rows.len(), 2, "{:?}", report.diagnostics);
}

/// The `implements` half — `issue_48371`'s multiplied family: repeated
/// declarations of the same name each report their own `implements`
/// clause entry once.
#[test]
fn repeated_implements_miss_reports_once_per_declaration() {
    let report = report_ops("class C implements Missing {} class C implements Missing {}");
    let rows = rows_of(&report, "implements_non_class");
    assert_eq!(rows.len(), 2, "{:?}", report.diagnostics);
    assert_ne!(
        rows[0].range.offset, rows[1].range.offset,
        "each declaration anchors its own entry"
    );
}

/// A repeated enum name — `enum E with Missing` declares no `with` grammar
/// (recovery routes it through the with/implements walk): each repeated
/// declaration still reports its own `with`/`implements` entries once.
#[test]
fn repeated_enum_implements_reports_once_per_declaration() {
    let report =
        report_ops("enum E implements Missing { a } enum E implements Missing { a }");
    let rows = rows_of(&report, "implements_non_class");
    assert_eq!(rows.len(), 2, "{:?}", report.diagnostics);
}
