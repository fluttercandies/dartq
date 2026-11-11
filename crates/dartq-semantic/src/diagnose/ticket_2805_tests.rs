//! The #2805 usage-cluster pins: the close-frame verdict rows for the
//! pattern/for-in faces (the `unused_local_variable` emission of
//! [`Engine::close_frame_locals`], mirroring the pinned SDK's
//! `unused_local_elements_verifier.dart`). Each test pins the exact row
//! set — message, name span, source order — for one face:
//!
//! - case-header pattern binders report individually (the verifier visits
//!   each `visitDeclaredVariablePattern` binder as its own local outside
//!   any `PatternVariableDeclaration` window; only a same-name sibling of
//!   the same switch stays coupled — the logical-or / shared-case join);
//! - a for-in loop variable is a local of the loop (the body's reads
//!   spare it, an empty body reports it);
//! - a local read inside a closure credits the declaration (the gather
//!   pass is whole-library by element identity);
//! - a pattern-declaration statement stays all-or-nothing per group.

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

/// Case-header binders of a switch statement report individually: the
/// `case String s` binder is unused and its arm-mate `a`'s guard use
/// never spares it (the sentinel group encodes the switch, not one
/// declaration window — upstream `visitDeclaredVariablePattern` with no
/// `PatternVariableDeclaration` ancestor, unused_local_elements_
/// verifier.dart:565-579).
#[test]
fn switch_statement_arm_binder_reports_individually() {
    let fixture = "void f(Object o) {\n  switch (o) {\n    case int a when a > 0:\n      break;\n    case String s:\n      break;\n  }\n}\n";
    let report = report_ops(fixture);
    let s = at(fixture, "case String s") + "case String ".len();
    assert_eq!(uv_rows(&report), [uv("s", s)], "{:?}", uv_rows(&report));
}

/// The switch-expression arms take the same individual rule: `v1`'s guard
/// and result read spare it, the untouched `v2` binder reports (the
/// co19 `execution_switch_expression_A01_t01` shape, `33:18 v2`).
#[test]
fn switch_expression_arm_binder_reports_individually() {
    let fixture = "String f(Object o) {\n  return switch (o) {\n    int v1 when v1 > 0 => 'x',\n    String v2 => 'y',\n    _ => 'z',\n  };\n}\n";
    let report = report_ops(fixture);
    let v2 = at(fixture, "String v2") + "String ".len();
    assert_eq!(uv_rows(&report), [uv("v2", v2)], "{:?}", uv_rows(&report));
}

/// Same-name arm binders of one shared case body join through a BODY use
/// only — the official join marks every transitive variable of the
/// body's joined `b` (`UsedLocalElements.addElement`). A guard reads its
/// own case's binder, so with no body use the earlier unread `b` still
/// reports (oracle 3.13.7: line 3, column 14), and a body read spares
/// both spellings; the differently-named unused `s` reports either way.
#[test]
fn same_name_arm_binders_stay_joined() {
    let fixture = "void f(Object o) {\n  switch (o) {\n    case int b:\n    case int b when b > 0:\n      break;\n    case String s:\n      break;\n  }\n}\n";
    let report = report_ops(fixture);
    let b = at(fixture, "case int b:") + "case int ".len();
    let s = at(fixture, "case String s") + "case String ".len();
    assert_eq!(
        uv_rows(&report),
        [uv("b", b), uv("s", s)],
        "{:?}",
        uv_rows(&report)
    );
    let joined = "void f(Object o) {\n  switch (o) {\n    case int b:\n    case int b when b > 0:\n      if (b > 1) {}\n      break;\n    case String s:\n      break;\n  }\n}\n";
    let report = report_ops(joined);
    let s = at(joined, "case String s") + "case String ".len();
    assert_eq!(uv_rows(&report), [uv("s", s)], "{:?}", uv_rows(&report));
}

/// The for-in loop variable is a local of the loop frame: an empty body
/// leaves it unread (`var y` and the typed `int z` both report), a body
/// read spares it (`x` in the condition) — the declaring form of the
/// official `visitDeclaredIdentifier` row.
#[test]
fn for_in_loop_variable_reports_when_body_never_reads_it() {
    let fixture = "void f() {\n  for (var x in [1, 2]) { if (x > 0) {} }\n  for (var y in [1, 2]) {}\n  for (int z in [1, 2]) {}\n}\n";
    let report = report_ops(fixture);
    let y = at(fixture, "var y in") + "var ".len();
    let z = at(fixture, "int z in") + "int ".len();
    assert_eq!(
        uv_rows(&report),
        [uv("y", y), uv("z", z)],
        "{:?}",
        uv_rows(&report)
    );
}

/// A local read inside a closure credits the declaration — the official
/// gather pass is whole-library by element identity, and the nested
/// frame's read resolves through the open frame stack, so the captured
/// `c` stays silent while the never-invoked `g` reports.
#[test]
fn closure_capture_credits_the_captured_local() {
    let fixture = "void f() {\n  int c = 1;\n  var g = () => c;\n}\n";
    let report = report_ops(fixture);
    let g = at(fixture, "var g") + "var ".len();
    assert_eq!(uv_rows(&report), [uv("g", g)], "{:?}", uv_rows(&report));
}

/// A pattern-declaration statement stays all-or-nothing per group
/// (`visitPatternVariableDeclaration`): the first group's read of `a`
/// spares the unread `b`, the second group with no reads reports both
/// binders.
#[test]
fn pattern_declaration_group_stays_all_or_nothing() {
    let fixture =
        "void f(Object o) {\n  var (a, b) = (1, 2);\n  if (a > 0) {}\n  var (c, d) = (1, 2);\n}\n";
    let report = report_ops(fixture);
    let c = at(fixture, "var (c") + "var (".len();
    let d = at(fixture, ", d)") + ", ".len();
    assert_eq!(
        uv_rows(&report),
        [uv("c", c), uv("d", d)],
        "{:?}",
        uv_rows(&report)
    );
}
