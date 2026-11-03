//! Ticket #3703 pins — the record literal's element→slot partition mapped
//! source order onto the wrong lists: the split assumed every named field
//! trails every positional field (`named = fields[len - names.len()..]`),
//! so a literal whose named field precedes a positional one gave the named
//! slot the positional field's type and vice versa — `var (n: (x: a3,
//! b3), (y: c3, d3)) = (n: (x: 42, -1), (y: 1, 0));` folded the
//! initializer as `((y: 1, 0) , {n: (x: 42, -1)})`'s mirror and reported
//! two fabricated `pattern_type_mismatch_in_irrefutable_context` rows per
//! field (`(int, {int y}) isn't assignable to (Object?, {Object? x})` and
//! the swapped twin), where the oracle is silent (co19
//! `LanguageFeatures/Patterns/record_A02_t08`,
//! `parser-fe/patterns/issue_51176` — 20 census extras).
//!
//! Upstream keys each named entry by its own `name:` label — a record
//! literal's `fields` carry positional and named entries interleaved in
//! source order, while the folded record type's `positional`/`named` lists
//! are separate (upstream Dart SDK source,
//! `pkg/_fe_analyzer_shared/lib/src/type_inference/type_analyzer.dart`
//! record-literal inference, `pkg/analyzer/lib/src/dart/ast/ast.dart`
//! `RecordLiteral`). The partition must therefore ask "is THIS field
//! named?" per window, never "how many named fields trail?".

use super::test_rig::{code_names, report_ops, rows};
use crate::diagnose::FileReport;

/// The `pattern_type_mismatch_in_irrefutable_context` rows, ascending.
fn mismatch_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "pattern_type_mismatch_in_irrefutable_context")
        .map(|d| format!("@{}: {}", d.range.offset, d.message))
        .collect()
}

/// The census repro: the literal's named field `n` precedes its positional
/// field. The fold must key `n` to `(x: 42, -1)` and the positional slot to
/// `(y: 1, 0)` — the pattern `(n: (x: a3, b3), (y: c3, d3))` then reads
/// `(Object?, {Object? x})` and `(Object?, {Object? y})` against matching
/// shapes and stays silent.
///
/// Oracle (`record_A02_t08`, `issue_51176`): no mismatch rows.
#[test]
fn named_field_first_keeps_its_own_type() {
    let report = report_ops(
        "void f() {\n\
         \x20 var (n: (x: a3, b3), (y: c3, d3)) = (n: (x: 42, -1), (y: 1, 0));\n\
         }\n",
    );
    assert!(
        mismatch_rows(&report).is_empty(),
        "named-first literal folds (int, {{int n}}) — the swap over-reports: {:#?}",
        rows(&report)
    );
}

/// Control — the direction the trailing-split already served: the named
/// field LAST (`(1, 0, n: (x: 42, -1))`) maps `n` to the named record and
/// the two positional slots to `1, 0`. This must stay silent after the
/// partition stops keying on field count.
///
/// Oracle: silent (same fixture family, trailing-named shape).
#[test]
fn named_field_last_still_keeps_its_type() {
    let report = report_ops(
        "void f() {\n\
         \x20 var (a, b, n: (x: c)) = (1, 0, n: (x: 42));\n\
         }\n",
    );
    assert!(
        mismatch_rows(&report).is_empty(),
        "named-last literal keeps the trailing split honest: {:#?}",
        rows(&report)
    );
}

/// Fully interleaved — `(a, n: b, c)`: each entry must land on its own
/// slot. The literal folds `({int n}, int, int)`'s source-order mirror as
/// positional `[int, int]` + named `{n: int}`; the matching pattern reads
/// real types through and stays silent.
///
/// Oracle: silent.
#[test]
fn interleaved_fields_map_each_entry_to_its_own_slot() {
    let report = report_ops(
        "void f() {\n\
         \x20 var (a, n: b, c) = (1, n: 2, 3);\n\
         }\n",
    );
    assert!(
        mismatch_rows(&report).is_empty(),
        "interleaved literal folds (int, int, {{int n}}): {:#?}",
        rows(&report)
    );
}

/// Negative control — a genuine mismatch must still report: the literal's
/// named slot carries `(x: int)` while the pattern's `n` field requires
/// `(Object?, {Object? y})` (a DIFFERENT name than the literal writes), so
/// the record-shape gate honestly fails and the typed leaf reports. This
/// pins that the partition repair does not simply silence record patterns.
///
/// Oracle: `pattern_type_mismatch_in_irrefutable_context` at the leaf.
#[test]
fn genuine_named_slot_mismatch_still_reports() {
    let report = report_ops(
        "void f() {\n\
         \x20 var (n: (y: int a)) = (n: (x: 42));\n\
         }\n",
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"pattern_type_mismatch_in_irrefutable_context"),
        "a real named-shape mismatch must still report: {:#?}",
        rows(&report)
    );
}

/// Negative control — positional count mismatch still reports: the literal
/// carries one positional field where the pattern requires two, so the
/// shape gate fails honestly (independent of the named-vs-positional
/// partition).
///
/// Oracle: `pattern_type_mismatch_in_irrefutable_context` at the leaves.
#[test]
fn genuine_positional_count_mismatch_still_reports() {
    let report = report_ops(
        "void f() {\n\
         \x20 var (int a, int b) = (n: 1, 2);\n\
         }\n",
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"pattern_type_mismatch_in_irrefutable_context"),
        "a positional-count mismatch must still report: {:#?}",
        rows(&report)
    );
}

/// The two-field swap face, oracle-verbatim: a bare record literal (no
/// pattern) whose named field precedes the positional one must fold
/// `positional=[(y:1,0)]` + `named={n:(x:42,-1)}` — the literal's own type
/// display and any consumer of the folded `Ty` reads that split. Pinning
/// through the pattern path (the observable face the census rows hit): a
/// literal `(n: named_record, positional_record)` against a matching
/// pattern is silent only when the named slot gets the named field's type.
///
/// Oracle: silent.
#[test]
fn swapped_slots_fold_silently_when_partition_is_per_field() {
    let report = report_ops(
        "void f() {\n\
         \x20 var (n: int x, int y) = (n: 1, 2);\n\
         }\n",
    );
    assert!(
        mismatch_rows(&report).is_empty(),
        "named-first literal folds (int, {{int n}}): {:#?}",
        rows(&report)
    );
}
