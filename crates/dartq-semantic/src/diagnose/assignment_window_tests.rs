//! The assignment verdict's window (`invalid_assignment`'s span) and the row
//! stack invariant it rides.
//!
//! The oracle anchors the row at the whole right-hand expression
//! (`AssignmentExpression.rightHandSide`), so a multi-row window spans its
//! first operand row through the result row's end while a single-row window
//! is that row's own range. The window's head is therefore resolved by
//! OFFSET, never by row-stack scan order: the stack is a fold discipline, not
//! an ordering guarantee (`fold_conditional` keeps the arm rows live under a
//! result row dated at the condition's head, and `fold_binary` re-pushes a
//! folded row at the head of the chain it consumed), so a "first row in scan
//! order" head can sit past the result row's end. That subtraction is where
//! the analyzer aborted with `attempt to subtract with overflow` on a corpus
//! slice; the frame now bounds the candidate by the result row's end and the
//! length by `checked_sub`, so no input reaches an underflow.
//!
//! Oracle provenance (`dart analyze --format=machine`, SDK 3.13.7): the
//! reporting shapes carry `INVALID_ASSIGNMENT` over the whole right-hand side
//! (`c ? a : s` — nine columns), and the shapes that fit report nothing.

use super::test_rig::{CORE_PRELUDE, assert_silent, code_names, report_ops};

/// The `invalid_assignment` row's span over `source`, with `window` the
/// right-hand expression the oracle anchors at (resolved in the
/// prelude-prefixed text the rig diagnoses).
fn window_span(source: &str, window: &str) -> (u32, u32) {
    let report = report_ops(source);
    assert_eq!(
        code_names(&report),
        ["invalid_assignment"],
        "{:?}",
        code_names(&report)
    );
    let row = &report.diagnostics[0];
    let full = format!("{CORE_PRELUDE}{source}");
    let at = u32::try_from(full.find(window).expect("window text")).expect("span fits u32");
    assert_eq!(row.range.offset, at, "window head");
    assert_eq!(
        u32::try_from(window.len()).expect("span fits u32"),
        row.range.length
    );
    (at, row.range.length)
}

/// The conditional right side reports at the whole ternary: the window opens
/// at the condition's head, not at the then arm's row.
#[test]
fn conditional_right_side_spans_the_whole_expression() {
    let source = "int h(bool c, int a, String s) {\n\
                  \x20 int x = 0;\n\
                  \x20 x = c ? a : s;\n\
                  \x20 return x;\n\
                  }\n";
    window_span(source, "c ? a : s");
}

/// The if-null right side shares the face: `??=` checks the same window, so
/// the ternary spans it exactly as the plain `=` does.
#[test]
fn if_null_right_side_spans_the_whole_expression() {
    let source = "int? h(bool c, int a, String s) {\n\
                  \x20 int? y;\n\
                  \x20 y ??= c ? a : s;\n\
                  \x20 return y;\n\
                  }\n";
    window_span(source, "c ? a : s");
}

/// A member-plus-binary right side folds to a row dated at the chain's head
/// (`fold_binary` walks the dotted chain back) — the window still starts
/// there and the assignment is silent.
#[test]
fn binary_chain_right_side_assigns_silently() {
    let source = "class C {\n\
                  \x20 int v = 0;\n\
                  }\n\
                  int g(C o, int a) {\n\
                  \x20 int x = 0;\n\
                  \x20 x = o.v + a;\n\
                  \x20 return x;\n\
                  }\n";
    assert_silent(&report_ops(source));
}

/// A parenthesized right side resolves to the group's own row — the
/// single-row window keeps the row's range.
#[test]
fn parenthesized_right_side_assigns_silently() {
    let source = "int g(int a) {\n\
                  \x20 int x = 0;\n\
                  \x20 x = (a);\n\
                  \x20 return x;\n\
                  }\n";
    assert_silent(&report_ops(source));
}

/// A conditional whose arms fit the target reports nothing: the window head
/// moves to the condition's row without disturbing the verdict.
#[test]
fn conditional_right_side_assigns_silently_when_assignable() {
    let source = "int g(bool c, int a, int b) {\n\
                  \x20 int x = 0;\n\
                  \x20 x = c ? a : b;\n\
                  \x20 return x;\n\
                  }\n";
    assert_silent(&report_ops(source));
}
