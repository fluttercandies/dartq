//! #3194 pins for the two lints-internal faces #3157 left red, measured
//! against the pinned `dart analyze` 3.13.7 oracle.
//!
//! - the **conditional arms** (`t ? x : y`) — the row anchors at its `?`
//!   with the condition tiled to its left, so the run's covering row is
//!   neither first nor last there; upstream's `visitConditionalExpression`
//!   walks both arms and reports each effect-free operand
//!   (`pkg/linter/lib/src/rules/unnecessary_statements.dart:91-94`);
//! - the **comma-initializer recovery** (`for (a == s, c = 0; …)`) — the
//!   parser splices a synthetic `;` after the first segment
//!   (`pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart`,
//!   `parseForLoopPartsRest` → `ensureSemicolon`) and hands the comma tail
//!   to the updater list, so the initialization is the first segment only
//!   (`unnecessary_statements.dart:209-217` visits initialization and
//!   updaters separately).
//!
//! Every expected span is the oracle row for the same shape (`14:8:6` for
//! the recovered initializer; per-arm spans for the conditionals).

use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

/// The rows for `source`, ordered by start: `(start, end)` in UTF-16.
fn rows(source: &str) -> (Fixture, Vec<(u32, u32)>) {
    let f = Fixture::parse(source);
    let mut rows: Vec<(u32, u32)> = f
        .run_semantic(&one_rule("unnecessary_statements"))
        .into_iter()
        .map(|lint| (lint.range.start, lint.range.end))
        .collect();
    rows.sort_unstable();
    (f, rows)
}

/// Pins `source` to exactly `expected` rows, each `(needle, nth, len)`:
/// it opens at the anchor token and spans `len` UTF-16 units.
fn assert_rows(source: &str, expected: &[(&str, usize, u32)]) {
    let (f, rows) = rows(source);
    assert_eq!(rows.len(), expected.len(), "{source:?}: {rows:?}");
    for ((start, end), (needle, nth, len)) in rows.iter().zip(expected) {
        let anchor = utf16_at(&f, tok(&f.source, &f.tokens, needle, *nth));
        assert_eq!(*start, anchor, "{source:?}: {needle} #{nth}");
        assert_eq!(end - start, *len, "{source:?}: {needle} #{nth}");
    }
}

// —— the conditional arms ——————————————————————————————————————————————

/// Both arms walk; the condition does not (`:91-94`).
#[test]
fn us_conditional_arms_report_per_arm() {
    assert_rows(
        "void f(bool t, int x, int y) {\n  t ? x : y;\n}\n",
        &[("x", 1, 1), ("y", 1, 1)],
    );
}

/// A member-access arm keeps its whole selector span.
#[test]
fn us_conditional_member_arms_report_the_selector() {
    assert_rows(
        "class C {\n  int b = 1;\n}\nvoid f(C a, bool t) {\n  t ? a.b : a.b;\n}\n",
        &[("a", 1, 3), ("a", 2, 3)],
    );
}

/// A clear-effect arm stays silent (an invocation in the then-branch).
#[test]
fn us_conditional_clear_arm_stays_silent() {
    assert_rows(
        "void f(bool t, int y) {\n  t ? print(1) : y;\n}\n",
        &[("y", 1, 1)],
    );
}

// —— the comma-initializer recovery ————————————————————————————————

/// Oracle `14:8:6`: the first segment reports whole; the recovered
/// `c = 0` assignment has a clear effect.
#[test]
fn us_comma_init_reports_the_first_segment() {
    assert_rows(
        "void f(int a, int c, String s) {\n  for (a == s, c = 0; c < 1; c++) {\n    c = 1;\n  }\n}\n",
        &[("a", 1, 6)],
    );
}

/// An assignment-led list reports only the recovered expression segment.
#[test]
fn us_comma_init_after_an_assignment_segment() {
    assert_rows(
        "void f(int a, int c, String s) {\n  for (c = 0, a == s; c < 1; c++) {\n    c = 1;\n  }\n}\n",
        &[("a", 1, 6)],
    );
}

/// Both expression segments report, in order.
#[test]
fn us_comma_init_reports_each_expression_segment() {
    assert_rows(
        "void f(int a, int b, int c, String s) {\n  for (a == s, b == a, c = 0; c < 1; c++) {\n    c = 1;\n  }\n}\n",
        &[("a", 1, 6), ("b", 1, 6)],
    );
}

/// The real header condition past the recovery stays silent — only the
/// synthesized condition joins the visited parts.
#[test]
fn us_comma_init_real_condition_stays_silent() {
    assert_rows(
        "void f(int a, int c, String s) {\n  for (a == s, c = 0; a == s, c = 1; c++) {\n    c = 1;\n  }\n}\n",
        &[("a", 1, 6)],
    );
}

/// A plain header keeps its own two faces (the comma window must not
/// swallow the initialization or the updater).
#[test]
fn us_plain_for_header_keeps_both_faces() {
    assert_rows(
        "void f(int i, bool b) {\n  for (1 + 2; b; 1 + 2) {\n    i = 1;\n  }\n}\n",
        &[("1", 0, 5), ("1", 1, 5)],
    );
}
