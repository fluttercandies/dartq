//! The #2858 registration-context pins: the pattern binders declared
//! outside any `PatternVariableDeclaration` wrapper — the for-in pattern
//! head (`for (var (a, b) in xs)`, a bare `DartPattern`), the if-case
//! pattern (`if (x case (a, b))`), and the pattern assignment
//! (`(a, b) = …`) — join the frame-locals walk individually
//! (unused_local_elements_verifier.dart:565-579 visits each
//! `visitDeclaredVariablePattern` binder as its own local; only a
//! `PatternVariableDeclaration` opens the all-or-nothing window), instead
//! of riding one window group. The same-name spellings of one logical-or
//! element stay one element upstream (`UsedLocalElements.addElement`): a
//! use of any spelling spares the pair. An assignment binder declares
//! nothing — no row at all, the write never counts as a use, and reads
//! credit the original declarations.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// The `unused_local_variable` rows as `message @offset:len`, source
/// order — the exact-row pin format (one string per expected row).
fn uv_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_local_variable")
        .map(|d| format!("{} @{}:{}", d.message, d.range.offset, d.range.length))
        .collect()
}

/// The absolute fixture offset of `needle` (the rig's prelude included).
fn at(fixture: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + fixture.find(needle).unwrap()
}

/// `The value of the local variable '<name>' isn't used.` at `offset`.
fn uv(name: &str, offset: usize) -> String {
    format!(
        "The value of the local variable '{name}' isn't used. @{offset}:{}",
        name.len()
    )
}

/// A for-in statement head's binders report individually: the body's read
/// of `a` spares only `a` — `b` rides no window (the head pattern is a
/// bare `DartPattern`, no `PatternVariableDeclaration` wrapper).
#[test]
fn for_in_pattern_head_mixed_use_reports_individually() {
    let fixture =
        "void f(pairs) {\n  for (var (int a, int b) in pairs) {\n    if (a > 0) {}\n  }\n}\n";
    let report = report_ops(fixture);
    let b = at(fixture, "int b") + 4;
    assert_eq!(uv_rows(&report), [uv("b", b)], "{:?}", uv_rows(&report));
}

/// Nested pattern heads keep their windows apart: each loop's conversion
/// bounds at its own frame mark and the head's window dies with the head,
/// so the outer body's read of `a` and the inner body's read of `c` spare
/// nothing across loops — `b` and `d` both report.
#[test]
fn nested_pattern_heads_report_across_loops() {
    let fixture = "void f(pairs) {\n  for (var (a, b) in pairs) {\n    for (var (c, d) in pairs) {\n      if (a > c) {}\n    }\n  }\n}\n";
    let report = report_ops(fixture);
    let b = at(fixture, "(a, b)") + 4;
    let d = at(fixture, "(c, d)") + 4;
    assert_eq!(
        uv_rows(&report),
        [uv("b", b), uv("d", d)],
        "{:?}",
        uv_rows(&report)
    );
}

/// The collection-element form takes the same rule: the element read
/// spares `a` alone — `b` reports beside the unread list literal's `x`.
#[test]
fn for_in_pattern_element_mixed_use_reports_individually() {
    let fixture = "void f(pairs) {\n  var x = [for (var (a, b) in pairs) a];\n}\n";
    let report = report_ops(fixture);
    let b = at(fixture, "(a, b)") + 4;
    let x = at(fixture, "var x") + 4;
    assert_eq!(
        uv_rows(&report),
        [uv("x", x), uv("b", b)],
        "{:?}",
        uv_rows(&report)
    );
}

/// The all-used extreme stays silent: every head binder read in the body.
#[test]
fn for_in_pattern_head_all_used_stays_silent() {
    let fixture = "void f(pairs) {\n  for (var (int a, int b) in pairs) {\n    if (a > 0) {\n      if (b > 1) {}\n    }\n  }\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        uv_rows(&report),
        Vec::<String>::new(),
        "{:?}",
        uv_rows(&report)
    );
}

/// An if-case pattern's binders report individually: the then-branch's
/// read of `a` spares only `a` (the declaring context pops at the root
/// pattern, so the case-header sentinel's switch-slot scope never
/// applies).
#[test]
fn if_case_pattern_mixed_use_reports_individually() {
    let fixture = "void f(o) {\n  if (o case (int a, int b)) {\n    if (a > 0) {}\n  }\n}\n";
    let report = report_ops(fixture);
    let b = at(fixture, "int b") + 4;
    assert_eq!(uv_rows(&report), [uv("b", b)], "{:?}", uv_rows(&report));
}

/// The same-name spellings of one logical-or element stay joined (the co19
/// `if_case_A02` shape): the guard's read of `a` spares both `var a`
/// spellings — per-binder individuality never splits one element.
#[test]
fn if_case_same_name_or_join_stays_silent() {
    let fixture = "void f(value) {\n  if (value case var a && 0 || var a && 1 when a == 0) {}\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        uv_rows(&report),
        Vec::<String>::new(),
        "{:?}",
        uv_rows(&report)
    );
}

/// A declaration following an if-case opens its OWN window: the if-case
/// binder (`var a`) registers ungrouped and reports when unread, while the
/// declaration's all-or-nothing group stays `{c, d}` — `c`'s read spares
/// `d`, and neither spares `a`.
#[test]
fn declaration_after_if_case_opens_a_fresh_window() {
    let fixture =
        "void f(o) {\n  if (o case var a) {}\n  var (c, d) = (1, 2);\n  if (c > 0) {}\n}\n";
    let report = report_ops(fixture);
    let a = at(fixture, "var a") + 4;
    assert_eq!(uv_rows(&report), [uv("a", a)], "{:?}", uv_rows(&report));
}

/// A pattern assignment declares nothing: the write spares neither
/// binder, and the later read credits `a`'s original declaration — `b`
/// reports at its declaration, no row lands at the assignment.
#[test]
fn pattern_assignment_read_credits_the_original() {
    let fixture =
        "void f() {\n  int a = 1;\n  int b = 2;\n  (a, b) = (3, 4);\n  if (a > 0) {}\n}\n";
    let report = report_ops(fixture);
    let b = at(fixture, "int b") + 4;
    assert_eq!(uv_rows(&report), [uv("b", b)], "{:?}", uv_rows(&report));
}

/// The write-only extreme: with no reads anywhere, both original
/// declarations report and the assignment itself emits nothing (the
/// official write arm never counts as a use and creates no element).
#[test]
fn pattern_assignment_write_only_reports_at_declarations() {
    let fixture = "void f() {\n  int a = 1;\n  int b = 2;\n  (a, b) = (3, 4);\n}\n";
    let report = report_ops(fixture);
    let a = at(fixture, "int a") + 4;
    let b = at(fixture, "int b") + 4;
    assert_eq!(
        uv_rows(&report),
        [uv("a", a), uv("b", b)],
        "{:?}",
        uv_rows(&report)
    );
}

/// An assignment inside a closure binds the outer declarations: with no
/// inner rows, the closure's read resolves to the outer `a` — the
/// original stays silent, `b` (write-only) and the unread closure `g`
/// report.
#[test]
fn closure_pattern_assignment_credits_the_outer_declaration() {
    let fixture = "void f() {\n  int a = 1;\n  int b = 2;\n  var g = () {\n    (a, b) = (3, 4);\n    if (a > 0) {}\n  };\n}\n";
    let report = report_ops(fixture);
    let b = at(fixture, "int b") + 4;
    let g = at(fixture, "var g") + 4;
    assert_eq!(
        uv_rows(&report),
        [uv("b", b), uv("g", g)],
        "{:?}",
        uv_rows(&report)
    );
}
