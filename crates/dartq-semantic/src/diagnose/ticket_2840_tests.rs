//! The #2840 span-extent pins: the is-check verdict's whole-expression
//! span (the oracle's `.at(node)`, `_checkAllTypeChecks`) runs through a
//! written function type's trailing formal list and a record type's own
//! closer, and anchors at the operand's true start when a literal face
//! pushed the operand row at its closing bracket.
//!
//! The seam: the verdict's type-end extension took the armed head and a
//! closed type-arguments group only — `m20 is void Function(int,
//! [List<T>]) Function(int x)` stopped at the last formal's head token
//! (`788:19|49` against the oracle's `788:19|52`), and `(null is int) is
//! bool` anchored at the parenthesized operand's `)` where the oracle
//! anchors at the IsExpression's own offset. Both sides already agree on
//! every verdict here; only the highlight extent moves (the A2 S2/S2b
//! residual family, 1,814 paired rows plus the paren-operand anchors).
//! The operands are declared-typed locals — the identical-type fold the
//! instanceof/type_checks corpora prove emits — so each pin isolates the
//! span, not the verdict.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// The `unnecessary_type_check` rows as `slice @offset`, source order —
/// the exact anchored source text is the pin (a span short by a formal
/// or anchored at a closer fails the slice, not just the length).
fn check_spans(report: &FileReport, full: &str) -> Vec<String> {
    let mut rows: Vec<(u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unnecessary_type_check")
        .map(|d| {
            (
                d.range.offset,
                full[d.range.offset as usize..(d.range.offset + d.range.length) as usize]
                    .to_string(),
            )
        })
        .collect();
    rows.sort_by_key(|(offset, _)| *offset);
    rows.into_iter()
        .map(|(offset, slice)| format!("{slice} @{offset}"))
        .collect()
}

/// The absolute fixture offset of `needle` (the rig's prelude included).
fn at(fixture: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + fixture.find(needle).unwrap()
}

/// The function_type9 shape: a written function type's trailing formal
/// list — the formals' names and closing paren, over a function-typed
/// return type with its own optional formals — joins the span (the
/// corpus's `788:19|52` against the armed slot's `788:19|49`). The
/// one-level shape pins the minimal delta (`f is int Function(Null)`
/// through the `)`), the nested one the full chain.
#[test]
fn written_function_type_span_covers_trailing_formals() {
    let fixture = concat!(
        "void t() {\n",
        "  int Function(Null) f;\n",
        "  void Function(int, [String]) Function(int x) g;\n",
        "  f is int Function(Null);\n",
        "  g is void Function(int, [String]) Function(int x);\n",
        "}\n",
    );
    let report = report_ops(fixture);
    let full = format!("{CORE_PRELUDE}{fixture}");
    assert_eq!(
        check_spans(&report, &full),
        vec![
            format!(
                "f is int Function(Null) @{}",
                at(fixture, "f is int Function(Null)")
            ),
            format!(
                "g is void Function(int, [String]) Function(int x) @{}",
                at(fixture, "g is void Function(int, [String]) Function(int x)")
            ),
        ],
        "{:?}",
        report.diagnostics
    );
}

/// The no-formal-name boundary: the span ends at the written type's own
/// formal closer — the enclosing `if` group's `)` is never consumed —
/// and a type-arguments-only annotation ends at `>` as before.
#[test]
fn span_stops_at_the_type_bracket_not_the_enclosing_call() {
    let fixture = concat!(
        "void t() {\n",
        "  void Function(int) g;\n",
        "  List<int> l;\n",
        "  if (g is void Function(int)) print(1);\n",
        "  l is List<int>;\n",
        "}\n",
    );
    let report = report_ops(fixture);
    let full = format!("{CORE_PRELUDE}{fixture}");
    assert_eq!(
        check_spans(&report, &full),
        vec![
            format!(
                "g is void Function(int) @{}",
                at(fixture, "g is void Function(int)")
            ),
            format!("l is List<int> @{}", at(fixture, "l is List<int>")),
        ],
        "{:?}",
        report.diagnostics
    );
}

/// A trailing `?` rides the written extent's end (`end_function_type`
/// carries the question token for the outermost starter).
#[test]
fn nullable_function_type_span_covers_the_question() {
    let fixture = concat!(
        "void t() {\n",
        "  void Function(int)? g;\n",
        "  g is void Function(int)?;\n",
        "}\n",
    );
    let report = report_ops(fixture);
    let full = format!("{CORE_PRELUDE}{fixture}");
    assert_eq!(
        check_spans(&report, &full),
        vec![format!(
            "g is void Function(int)? @{}",
            at(fixture, "g is void Function(int)?")
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The parenthesized-operand anchor (the instanceof corpus shape): the
/// outer verdict anchors at the group's `(` — the ParenthesizedExpression's
/// own offset, `) is bool` before the fix — with the whole-expression
/// extent through the checked type.
#[test]
fn paren_operand_anchors_at_the_open_paren() {
    let fixture = "void t() {\n  (null is int) is bool;\n}\n";
    let report = report_ops(fixture);
    let full = format!("{CORE_PRELUDE}{fixture}");
    assert_eq!(
        check_spans(&report, &full),
        vec![format!(
            "(null is int) is bool @{}",
            at(fixture, "(null is int) is bool")
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The record face (the records corpus shape): the record literal
/// operand anchors at its `(` and the written record type's span runs
/// through the type's own `)` — `(1, 2) is (int, int)` in full.
#[test]
fn record_literal_and_record_type_spans_cover_their_brackets() {
    let fixture = "void t() {\n  (1, 2) is (int, int);\n}\n";
    let report = report_ops(fixture);
    let full = format!("{CORE_PRELUDE}{fixture}");
    assert_eq!(
        check_spans(&report, &full),
        vec![format!(
            "(1, 2) is (int, int) @{}",
            at(fixture, "(1, 2) is (int, int)")
        )],
        "{:?}",
        report.diagnostics
    );
}
