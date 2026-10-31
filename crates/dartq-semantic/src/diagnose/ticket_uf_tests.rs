//! #3126 — `unnecessary_final`: the `FieldFormalParameter`/
//! `SuperFormalParameter` reach, mirrored
//! (best_practices_verifier.dart:1040-1049 fires only from
//! `visitFieldFormalParameter`/`visitSuperFormalParameter`
//! :496-504/:902-910).
//!
//! The warning reports iff `final this.x`/`final super.x` sits inside a
//! **declaration formal list** — the `FormalParameterList` the parser
//! lowers those nodes into — including the `[`/`{` optional/named group
//! nested in it (`C({required final this.x})`, `C([final this.x])`). It
//! stays silent inside a `Function(…)` type-annotation list (the parser
//! lowers `final this.x` there to a plain parameter — the
//! `extraneous_modifier` face, never a field formal), a parenthesized
//! expression, a call's argument list, a `for`/`if-case` head, and a
//! primary-constructor head (best_practices_verifier.dart:101/:818-824).

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

fn uf_rows(report: &FileReport) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unnecessary_final")
        .count()
}

// —— Declaration formal lists report ————————————————————————————

/// A `final this.` in a constructor's formal list reports
/// (visitFieldFormalParameter, :496-504).
#[test]
fn field_formal_reports() {
    let report = report_ops("class C { int x = 0; C(final this.x); }");
    assert_eq!(uf_rows(&report), 1);
}

/// A `final super.` in a constructor's formal list reports
/// (visitSuperFormalParameter, :902-910).
#[test]
fn super_formal_reports() {
    let fixture = "class B { int x = 0; B(this.x); }\nclass C extends B { C(final super.x); }";
    let report = report_ops(fixture);
    assert_eq!(uf_rows(&report), 1);
}

/// `final this.` inside the `[`/`{` optional/named group nested in the
/// formal list still reports — the parser lowers it to a
/// `FieldFormalParameter` regardless of the group wrapper.
#[test]
fn named_and_optional_groups_report() {
    let report = report_ops("class C { int x = 0; C({required final this.x}); }");
    assert_eq!(uf_rows(&report), 1, "named group reports");
    let report = report_ops("class C { int x = 0; C([final this.x = 0]); }");
    assert_eq!(uf_rows(&report), 1, "optional positional group reports");
}

/// A `final super.` inside the named group reports.
#[test]
fn super_named_group_reports() {
    let fixture =
        "class B { int x = 0; B({this.x = 0}); }\nclass C extends B { C({final super.x}); }";
    let report = report_ops(fixture);
    assert_eq!(uf_rows(&report), 1);
}

/// A lambda's `final this.` reports — `var f = (final this.x) => x`
/// parses the `final this.x` as a `FieldFormalParameter` (the `)`-`=>`
/// decl-body signature).
#[test]
fn lambda_formal_reports() {
    let report = report_ops("var f = (final this.x) => 0;");
    assert_eq!(uf_rows(&report), 1);
}

/// A method's `final this.` reports (declaration-body context).
#[test]
fn method_formal_reports() {
    let report = report_ops("class C { int x = 0; void m(final this.x) {} }");
    assert_eq!(uf_rows(&report), 1);
}

// —— Non-formal parens stay silent —————————————————————————————

/// A `Function(…)` type-annotation list is not a declaration formal
/// list — `final this.x` lowers to a plain parameter there.
#[test]
fn function_type_list_silent() {
    let report = report_ops("typedef F = void Function(final this.x);");
    assert_eq!(uf_rows(&report), 0);
    let report = report_ops("void g(void Function(final this.x) cb) {}");
    assert_eq!(uf_rows(&report), 0);
}

/// A parenthesized expression is not a formal list — `var f = (final
/// this.x);` (the `)`-`;` follow in a statement context) stays silent.
#[test]
fn paren_expression_silent() {
    let report = report_ops("class C { int x = 0; void m() { var f = (final this.x); } }");
    assert_eq!(uf_rows(&report), 0);
}

/// A call's argument list is not a formal list — `g(final this.x)` and
/// `print(final this.x)` inside a statement body stay silent.
#[test]
fn call_argument_list_silent() {
    let report = report_ops("class C { int x = 0; void m() { g(final this.x); } void g(int v) {} }");
    assert_eq!(uf_rows(&report), 0);
}

/// A record type inside a formal list — `f((final this.x,) r)` — is not
/// itself a formal list.
#[test]
fn record_type_nested_silent() {
    let report = report_ops("void f((final this.x,) r) {}");
    assert_eq!(uf_rows(&report), 0);
}

/// A `for`/`if` head is not a formal list.
#[test]
fn loop_and_case_heads_silent() {
    let report = report_ops("class C { int x = 0; void m() { for (final this.x in [1]) {} } }");
    assert_eq!(uf_rows(&report), 0);
    let report = report_ops("void f(Object o) { if (o case final this.x) {} }");
    assert_eq!(uf_rows(&report), 0);
}

/// A primary-constructor head suppresses the warning
/// (best_practices_verifier.dart:101/:818-824).
#[test]
fn primary_ctor_head_silent() {
    let report = report_ops("class C(final this.x) { final int x = 0; }");
    assert_eq!(uf_rows(&report), 0);
}
