//! #3645 pins — `unused_local_variable` two-face convergence
//! (census-20261102-230100: 179 missing / 172 extra).
//!
//! Landed here (this file's owning lane): the rest-element binder face of
//! `report_pattern_assignment_binders` — a `...` opener inside a pattern
//! assignment's left side is a pattern element position, so `... var c`,
//! `... final c`, and `... List<int> c` each declare `c`, whose minted
//! local element the oracle reports unused (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/error/unused_local_elements_verifier.dart`
//! `visitDeclaredVariablePattern`; co19
//! `Patterns/pattern_assignment_A04_t02` rest-element rows).
//!
//! Deferred halves (tracked on the ticket):
//! - the `<unnamed>` anchor remap — nameless declarations anchor at the
//!   enclosing executable fragment's name offset with zero length
//!   (`_reportDiagnosticForElement`: `nameOffset ??
//!   enclosingFragment?.nameOffset ?? 0`), which is #3476's work inside
//!   `close_frame_locals`;
//! - the same-name if-case join's head bound — two sequential `if (… case
//!   T x)` statements' binders are distinct elements upstream, but
//!   `join_pattern_site_binder` joins the second head's binder to the
//!   first's site group and the first's read spares it.
//!
//! Each pin carries the oracle's verdict for the same fixture
//! (`dart analyze` 3.13.7).

use super::test_rig::{CORE_PRELUDE, report_ops, rows};

/// `report_ops` mounts [`CORE_PRELUDE`] ahead of the fixture — every
/// expected offset shifts by its length.
fn at(source: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + source.find(needle).unwrap()) as u32
}

fn unused_local_rows(report: &super::FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_local_variable")
        .map(|d| format!("@{}+{}: {}", d.range.offset, d.range.length, d.message))
        .collect()
}

fn assert_unused_at(report: &super::FileReport, name: &str, offset: u32, length: u32) {
    let hit = report.diagnostics.iter().any(|d| {
        d.code.name == "unused_local_variable"
            && d.range.offset == offset
            && d.range.length == length
            && d.message.contains(name)
    });
    assert!(
        hit,
        "expected unused_local_variable for '{name}' @{offset}+{length}: {:#?}",
        rows(report)
    );
}

/// `... var c` in a pattern assignment declares `c`; the minted element is
/// unread and reports `unused_local_variable` at its name.
///
/// Oracle (co19 `Patterns/pattern_assignment_A04_t02`, `[a, b, ... var c]
/// = [3, 4, 5, 6];`): `unused_local_variable` at `c`.
#[test]
fn rest_var_binder_reports_unused() {
    let source = "main() {\n  var a = 0, b = 0;\n  [a, b, ... var c] = [3, 4, 5, 6];\n  g(a, b);\n}\nvoid g(Object? x, Object? y) {}\n";
    let report = report_ops(source);
    assert_unused_at(&report, "'c'", at(source, "var c") + 4, 1);
}

/// `... List<int> c`: a written-type rest binder names `c` after the
/// type's run — the same element reports.
///
/// Oracle (same case, `[a, b, ... List<int> c] = [3, 4, 5, 6];`):
/// `unused_local_variable` at `c`.
#[test]
fn rest_typed_binder_reports_unused() {
    let source = "main() {\n  var a = 0, b = 0;\n  [a, b, ... List<int> c] = [3, 4, 5, 6];\n  g(a, b);\n}\nvoid g(Object? x, Object? y) {}\n";
    let report = report_ops(source);
    assert_unused_at(&report, "'c'", at(source, "int> c") + 5, 1);
}

/// `... final c` in a pattern assignment declares `c` as well.
///
/// Oracle (same case, `[a, b, ... final c] = [3, 4, 5, 6];`):
/// `unused_local_variable` at `c`.
#[test]
fn rest_final_binder_reports_unused() {
    let source = "main() {\n  var a = 0, b = 0;\n  [a, b, ... final c] = [3, 4, 5, 6];\n  g(a, b);\n}\nvoid g(Object? x, Object? y) {}\n";
    let report = report_ops(source);
    assert_unused_at(&report, "'c'", at(source, "final c") + 6, 1);
}

/// Negative control: a bare `... rest` spread names an ASSIGNED variable,
/// never a declaring binder — no `unused_local_variable` row may anchor at
/// the spread's own name token (upstream mints no element for it; the
/// declared `rest` local's verdict is the assignment-write family's own
/// call, outside this pin).
///
/// Oracle (`[a, b, ...rest] = [1, 2, 3];`): no row at the spread token.
#[test]
fn rest_bare_spread_anchors_no_row() {
    let source = "main() {\n  var a = 0, b = 0, rest = <int>[];\n  [a, b, ...rest] = [3, 4, 5, 6];\n  g(a, b);\n}\nvoid g(Object? x, Object? y) {}\n";
    let report = report_ops(source);
    let spread_at = at(source, "...rest") + 3;
    let hit = report
        .diagnostics
        .iter()
        .any(|d| d.code.name == "unused_local_variable" && d.range.offset == spread_at);
    assert!(
        !hit,
        "spread name token must not anchor a fabricated row: {:#?}",
        unused_local_rows(&report)
    );
}

/// A plain method-body local still reports — the 179-miss side's negative
/// control against the enqueue gate.
///
/// Oracle (`void m() { var foo = 1; }`): `unused_local_variable` at `foo`.
#[test]
fn plain_method_body_local_reports() {
    let source = "class A {\n  void m() {\n    var foo = 1;\n  }\n}\n";
    let report = report_ops(source);
    assert_unused_at(&report, "'foo'", at(source, "foo"), 3);
}

/// An if-case binder is its own element: a never-read binder reports.
///
/// Oracle (`if (x case int y) {}`): `unused_local_variable` at `y`.
#[test]
fn if_case_binder_reports() {
    let source = "void f(Object? x) {\n  if (x case int y) {}\n}\n";
    let report = report_ops(source);
    assert_unused_at(&report, "'y'", at(source, "int y") + 4, 1);
}

/// A switch-case binder reports at its own name.
///
/// Oracle (`case int a: break;`): `unused_local_variable` at `a`.
#[test]
fn switch_case_binder_reports() {
    let source = "void f(Object? x) {\n  switch (x) {\n    case int a:\n      break;\n  }\n}\n";
    let report = report_ops(source);
    assert_unused_at(&report, "'a'", at(source, "int a") + 4, 1);
}

/// Deferred-half pin (skip until #3645's `close_frame_locals` half lands):
/// a nameless recovered local anchors at the enclosing executable's name
/// with zero length — `f() { var {} }` reports `<unnamed>` at `f`'s name,
/// not at the recovered declaration's token.
///
/// Oracle (`f() { var {} }`, parser recovery `LocalVariableTest` family):
/// `unused_local_variable '<unnamed>'` at `f`'s name offset, length 0.
#[test]
fn nameless_local_anchors_at_enclosing_name() {
    let source = "f() { var {} }\n";
    let report = report_ops(source);
    let f_at = at(source, "f");
    let hit = report.diagnostics.iter().any(|d| {
        d.code.name == "unused_local_variable"
            && d.range.offset == f_at
            && d.range.length == 0
            && d.message.contains("<unnamed>")
    });
    assert!(
        hit,
        "expected <unnamed> row anchored at f ({f_at}+0): {:#?}",
        rows(&report)
    );
}

/// Deferred-half pin (skip until #3645's `close_frame_locals` half lands):
/// a recovered `PatternVariableDeclaration`'s nameless binders all anchor
/// at the enclosing name — the oracle's single set element per function.
///
/// Oracle (co19 `Patterns/variable_declaration_A01_t05`): `<unnamed>` at
/// `main`'s name offset, length 0.
#[test]
fn nameless_pattern_decl_anchors_at_enclosing_name() {
    let source = "main() {\n  var [a1, _];\n}\n";
    let report = report_ops(source);
    let main_at = at(source, "main");
    let hit = report.diagnostics.iter().any(|d| {
        d.code.name == "unused_local_variable"
            && d.range.offset == main_at
            && d.range.length == 0
            && d.message.contains("<unnamed>")
    });
    assert!(
        hit,
        "expected <unnamed> row anchored at main ({main_at}+0): {:#?}",
        rows(&report)
    );
}

/// Deferred-half pin (skip until #3645's `scopes_bodies` half lands): two
/// sequential `if (… case …)` statements' same-named binders are distinct
/// elements — the second head's unread binder still reports.
///
/// Oracle: `unused_local_variable` at the second `x`.
#[test]
fn second_if_case_same_name_binder_reports() {
    let source = "void f() {\n  if (123 as dynamic case int x) {\n    g(x);\n  }\n  if ('s' as dynamic case int x) {\n  }\n}\nvoid g(Object? a) {}\n";
    let report = report_ops(source);
    let second = (CORE_PRELUDE.len() + source.rfind("int x").unwrap()) as u32 + 4;
    let hit = report.diagnostics.iter().any(|d| {
        d.code.name == "unused_local_variable"
            && d.range.offset == second
            && d.message.contains("'x'")
    });
    assert!(hit, "expected 'x' @{second}: {:#?}", rows(&report));
}
