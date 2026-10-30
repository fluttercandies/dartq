//! The #2868 closure context-type inference pins (`inferLocalFunction`,
//! inference_visitor_base.dart:2194-2439): the literal's context `K`
//! latches at open time from four channels — the folded declaration
//! type, the assignment target's declared type, the argument slot's
//! formal, the enclosing function-shaped return position — and the
//! final literal type rides the pushed Value row (written and
//! context-inferred formals, the context return under the cap
//! `S = T if T <: R else S = R`). The body's returns check against
//! `K`'s return, the literal's untyped formals infer `greatestClosure`
//! downward, and a matched context lights the decl/assign/argument
//! faces with no emitter change.

use crate::diagnose::test_rig::{report_ops, rows};
use crate::diagnose::types::FileReport;

/// All diagnostics of `report`, `(code, offset, length)` triples in
/// source order.
fn hits(report: &FileReport) -> Vec<(String, u32, u32)> {
    report
        .diagnostics
        .iter()
        .map(|d| (d.code.name.to_string(), d.range.offset, d.range.length))
        .collect()
}

/// All diagnostics of `report`, `(code, message)` pairs in source
/// order.
fn messages(report: &FileReport) -> Vec<(String, String)> {
    report
        .diagnostics
        .iter()
        .map(|d| (d.code.name.to_string(), d.message.to_string()))
        .collect()
}

/// Pin 1: the declaration channel through a function-RHS ALIAS —
/// `Function2<int, String>` folds to `String Function(int)`; the
/// literal's untyped `x` infers `int` downward and its `String` body
/// satisfies the context return. Fully silent.
#[test]
fn decl_alias_channel_types_the_literal_silently() {
    let report = report_ops(
        "typedef Function2<X, Y> = Y Function(X);\n\
         \x20 void f() {\n\
         \x20   Function2<int, String> l1 = (x) => \"hello\";\n\
         \x20   l1(3);\n\
         \x20 }",
    );
    assert!(hits(&report).is_empty(), "rows: {:#?}", rows(&report));
}

/// Pin 2: the return cap decides WHICH code fires — `(x) => 3`'s body
/// misses `String`, so the cap collapses the literal's return to `R`
/// and the ONLY row is `return_of_invalid_type_from_closure` (the
/// body anchor), never `invalid_assignment`.
#[test]
fn the_cap_keeps_the_failure_on_the_closure_return() {
    let report = report_ops(
        "typedef Function2<X, Y> = Y Function(X);\n\
         \x20 void f() {\n\
         \x20   Function2<int, String> l3 = (x) => 3;\n\
         \x20   l3(3);\n\
         \x20 }",
    );
    let hits = hits(&report);
    assert_eq!(hits.len(), 1, "rows: {:#?}", rows(&report));
    assert_eq!(hits[0].0, "return_of_invalid_type_from_closure");
}

/// Pin 3: a written formal the context cannot pair reports the
/// ordinary `invalid_assignment` at the literal's own span — the
/// message spells the resolved literal type against the alias use.
#[test]
fn a_written_formal_reports_the_plain_assignment() {
    const FIXTURE: &str = "typedef Function2<X, Y> = Y Function(X);\n\
         \x20 void f() {\n\
         \x20   Function2<int, String> l2 = (String x) => \"hello\";\n\
         \x20   l2(3);\n\
         \x20 }";
    let report = report_ops(FIXTURE);
    let hits = hits(&report);
    assert_eq!(hits.len(), 1, "rows: {:#?}", rows(&report));
    assert_eq!(hits[0].0, "invalid_assignment");
    let messages = messages(&report);
    assert!(
        messages[0].1.contains("String Function(String)"),
        "{messages:#?}"
    );
    // The literal's own span anchors the row: it opens at `(` of
    // `(String x) => "hello"` and spans its 21 columns.
    let lit = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "invalid_assignment")
        .expect("the row exists");
    let lit_src = "(String x) => \"hello\"";
    let offset = crate::diagnose::test_rig::CORE_PRELUDE.len() as u32
        + FIXTURE.find(lit_src).expect("the needle spells itself") as u32;
    assert_eq!(
        (lit.range.offset, lit.range.length),
        (offset, lit_src.len() as u32)
    );
}

/// Pin 4: the argument channel — the literal at a formal's slot
/// matches its schema; a written formal the slot cannot pair reports
/// `argument_type_not_assignable` at the literal's span, beside the
/// body's own closure-return row.
#[test]
fn the_argument_slot_reports_at_the_literal() {
    let report = report_ops(
        "void useNum(void Function(num) f) {}\n\
         \x20 void f() {\n\
         \x20   useNum((String x) => x.length);\n\
         \x20 }",
    );
    let hits = hits(&report);
    assert!(
        hits.iter()
            .any(|(code, ..)| code == "argument_type_not_assignable"),
        "rows: {:#?}",
        rows(&report)
    );
}

/// Pin 5: the context return flows INTO the literal's body — a
/// collection literal body under a `List<String>` context return
/// types its elements against `String` (`list_element_type_not_
/// assignable` inside the body).
#[test]
fn the_context_return_flows_into_the_body() {
    let report = report_ops(
        "typedef Function2<X, Y> = Y Function(X);\n\
         \x20 void f() {\n\
         \x20   Function2<int, List<String>> g = (int x) => [3];\n\
         \x20   g(3);\n\
         \x20 }",
    );
    let hits = hits(&report);
    assert!(
        hits.iter()
            .any(|(code, ..)| code == "list_element_type_not_assignable"),
        "rows: {:#?}",
        rows(&report)
    );
}

/// Negative: `dynamic` context supplies nothing — the literal's own
/// face stays untyped and the body's member read keeps the
/// under-report posture.
#[test]
fn a_dynamic_context_supplies_nothing() {
    let report = report_ops(
        "void f() {\n\
         \x20   dynamic d = (x) => 3;\n\
         \x20   d(3);\n\
         \x20 }",
    );
    assert!(hits(&report).is_empty(), "rows: {:#?}", rows(&report));
}

/// Negative: the bare `Function` head is no function type — the
/// upstream `typeContext is FunctionType` arm refuses it.
#[test]
fn a_bare_function_head_supplies_nothing() {
    let report = report_ops(
        "void f() {\n\
         \x20   Function d = (x) => 3;\n\
         \x20   d(3);\n\
         \x20 }",
    );
    assert!(hits(&report).is_empty(), "rows: {:#?}", rows(&report));
}

/// Negative: a context-free `var` group supplies nothing — the
/// literal's formals keep `dynamic` and the row stays untyped.
#[test]
fn a_contextless_var_supplies_nothing() {
    let report = report_ops(
        "void f() {\n\
         \x20   var d = (int x) => 3;\n\
         \x20   d(3);\n\
         \x20 }",
    );
    assert!(hits(&report).is_empty(), "rows: {:#?}", rows(&report));
}

/// The assignment channel: `l = (x) => 3` over a `Function2` local —
/// the literal IS the right-hand side, the target's folded type is
/// the context. The cap reports the closure-return row only.
#[test]
fn the_assignment_channel_reports_the_closure_return() {
    let report = report_ops(
        "typedef Function2<X, Y> = Y Function(X);\n\
         \x20 void f() {\n\
         \x20   Function2<int, String> l = (x) => \"s\";\n\
         \x20   l = (x) => 3;\n\
         \x20   l(3);\n\
         \x20 }",
    );
    let hits = hits(&report);
    assert_eq!(hits.len(), 1, "rows: {:#?}", rows(&report));
    assert_eq!(hits[0].0, "return_of_invalid_type_from_closure");
}

/// The return channel: `return (x) => 3` inside a
/// `Function2`-returning function — the literal's context is the
/// enclosing declared return's folded shape.
#[test]
fn the_return_channel_reports_the_closure_return() {
    let report = report_ops(
        "typedef Function2<X, Y> = Y Function(X);\n\
         \x20 Function2<int, String> f() {\n\
         \x20   return (x) => 3;\n\
         \x20 }",
    );
    let hits = hits(&report);
    assert_eq!(hits.len(), 1, "rows: {:#?}", rows(&report));
    assert_eq!(hits[0].0, "return_of_invalid_type_from_closure");
}

/// The downward-inferred formal binds its `greatestClosure`: under an
/// `int` context formal the body's `String y = x` initializer reports
/// `invalid_assignment` — `x` IS `int` (a `dynamic` bind would keep
/// the read silent, the pre-#2868 posture).
#[test]
fn the_inferred_formal_types_the_body() {
    let report = report_ops(
        "typedef Function2<X, Y> = Y Function(X);\n\
         \x20 void f() {\n\
         \x20   Function2<int, String> l = (x) {\n\
         \x20     String y = x;\n\
         \x20     return y;\n\
         \x20   };\n\
         \x20   l(3);\n\
         \x20 }",
    );
    let hits = hits(&report);
    assert_eq!(hits.len(), 1, "rows: {:#?}", rows(&report));
    assert_eq!(hits[0].0, "invalid_assignment");
}
