//! The #3306 pins: the return face's two emit-side defects — the
//! expression body's `void` exemption (upstream
//! `verifyExpressionFunctionBody`) and the return row's
//! `.at(expression)` anchor.
//!
//! Upstream premises (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/error/return_type_verifier.dart`):
//!
//! * `verifyExpressionFunctionBody`:57-64 — an expression-bodied
//!   executable checks nothing when the FLATTENED declared return type is
//!   `VoidType`. `void f() => e;`, `Future<void> f() async => e;`,
//!   `FutureOr<void> f() async => e;`, and `Future<void>? f() async =>
//!   e;` all run the body expression for its value, never its type.
//! * `verifyReturnStatement`:66-76 — the statement form keeps checking;
//!   `_checkReturnExpression`:145-289 is the block-body matrix (an async
//!   `T_v` of `void` accepts only `void`/`dynamic`/`Null`).
//! * `reportTypeError`'s `.at(expression)` — every return row anchors on
//!   the whole returned expression, never the row's head token (a callee
//!   name, an untyped collection literal's `]`).

use crate::diagnose::test_rig::{CORE_PRELUDE, code_names, report_ops};
use crate::diagnose::types::FileReport;

/// The `return_of_invalid_type` row spans of `report` as `(offset,
/// length)`, in the fixture's own coordinates.
fn return_spans(report: &FileReport) -> Vec<(u32, u32)> {
    let shift = u32::try_from(CORE_PRELUDE.len()).expect("prelude fits u32");
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "return_of_invalid_type")
        .map(|d| (d.range.offset - shift, d.range.length))
        .collect()
}

/// The text a `(offset, length)` span covers inside `fixture`.
fn span_text<'a>(fixture: &'a str, span: (u32, u32)) -> &'a str {
    let start = span.0 as usize;
    let end = start + span.1 as usize;
    &fixture[start..end]
}

/// Every `return_of_invalid_type` row of the fixture, as `span: text`.
fn return_row_texts(report: &FileReport, fixture: &str) -> Vec<String> {
    return_spans(report)
        .into_iter()
        .map(|span| span_text(fixture, span).to_string())
        .collect()
}

fn has_return_row(report: &FileReport) -> bool {
    code_names(report).contains(&"return_of_invalid_type")
}

// —— the expression body's `void` exemption ————————————————

/// `Future<void> f() async => e;` is silent: `flatten(T)` is `void`, so
/// `verifyExpressionFunctionBody` never reaches `_checkReturnExpression`
/// (the block-body matrix below reports the same pair).
#[test]
fn async_arrow_body_future_void_is_silent() {
    let fixture = "Future<void> f() async => 3;\n";
    let report = report_ops(fixture);
    assert!(
        !has_return_row(&report),
        "got {:?}",
        return_row_texts(&report, fixture)
    );
}

/// The `FutureOr<void>` and nullable `Future<void>?` spellings flatten to
/// the same `void` and take the same exemption.
#[test]
fn async_arrow_body_future_or_void_and_nullable_are_silent() {
    let fixture = "\
FutureOr<void> g() async => 3;
Future<void>? h() async => 3;
";
    let report = report_ops(fixture);
    assert!(
        !has_return_row(&report),
        "got {:?}",
        return_row_texts(&report, fixture)
    );
}

/// The sync arrow body's `void` host is the same exemption with the raw
/// return type (a sync body flattens to itself).
#[test]
fn sync_arrow_body_void_is_silent() {
    let fixture = "void f() => 3;\n";
    let report = report_ops(fixture);
    assert!(
        !has_return_row(&report),
        "got {:?}",
        return_row_texts(&report, fixture)
    );
}

// —— the statement form keeps the matrix ————————————————

/// `Future<void> f() async { return 3; }` reports: only the expression
/// body is exempt, and `T_v` (`void`) accepts `void`/`dynamic`/`Null`
/// alone, so an `int` send reports over the whole expression.
#[test]
fn block_body_future_void_still_reports() {
    let fixture = "Future<void> f() async { return 3; }\n";
    let report = report_ops(fixture);
    assert_eq!(return_row_texts(&report, fixture), ["3"]);
}

/// The sync statement form reports the same pair (`void` host, `int`
/// send) — the exemption belongs to the arrow alone.
#[test]
fn block_body_void_still_reports() {
    let fixture = "void f() { return 3; }\n";
    let report = report_ops(fixture);
    assert_eq!(return_row_texts(&report, fixture), ["3"]);
}

/// A non-`void` flattened target keeps the matrix on the expression body
/// too: `Future<int> f() async => 'x';` reports.
#[test]
fn async_arrow_body_non_void_still_reports() {
    let fixture = "Future<int> f() async => 'x';\n";
    let report = report_ops(fixture);
    assert_eq!(return_row_texts(&report, fixture), ["'x'"]);
}

/// The discriminating pair in one unit: the arrow body of the `Future<
/// void>` host stays silent while the block body reports.
#[test]
fn arrow_and_block_bodies_of_the_same_host_differ() {
    let fixture = "\
Future<void> arrow() async => 3;
Future<void> block() async { return 3; }
";
    let report = report_ops(fixture);
    assert_eq!(return_row_texts(&report, fixture), ["3"]);
}

// —— the `.at(expression)` anchor ————————————————

/// `return g();` reports over the whole invocation (`g()`), not the callee
/// head the staged row carries.
#[test]
fn return_row_spans_the_whole_invocation() {
    let fixture = "\
int g() => 1;
void f() { return g(); }
";
    let report = report_ops(fixture);
    assert_eq!(return_row_texts(&report, fixture), ["g()"]);
}

/// An untyped collection literal's row sits on its `]`; the return row
/// still anchors on the whole literal.
#[test]
fn return_row_spans_the_whole_collection_literal() {
    let fixture = "void f() { return [1, 2]; }\n";
    let report = report_ops(fixture);
    assert_eq!(return_row_texts(&report, fixture), ["[1, 2]"]);
}

/// The same widening on the arrow body (`int` host, `String` body).
#[test]
fn arrow_body_row_spans_the_whole_expression() {
    let fixture = "int f() => 'abc';\n";
    let report = report_ops(fixture);
    assert_eq!(return_row_texts(&report, fixture), ["'abc'"]);
}

// —— silent negatives ————————————————

/// Committed returns stay silent on both body forms.
#[test]
fn committed_returns_stay_silent() {
    let fixture = "\
int f() => 1;
Future<int> g() async => 1;
Future<int> h() async { return 1; }
";
    let report = report_ops(fixture);
    assert!(
        !has_return_row(&report),
        "got {:?}",
        return_row_texts(&report, fixture)
    );
}
