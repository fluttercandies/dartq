//! The #3316 pins — `use_of_void_result`'s value-consumption faces.
//!
//! Upstream `checkForUseOfVoidResult`
//! (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/generated/error_detection_helpers.dart:245-258`):
//! an expression whose `staticType` is `VoidTypeImpl` reports
//! `useOfVoidResult` wherever its value is consumed — `.at(methodName)` for
//! a `MethodInvocation`, `.at(expression)` for every other shape. The
//! consumption sites the engine already answers (binary operands, argument
//! lists, assignment right-hand sides, conditions, `await`/`throw`/`yield`,
//! interpolation, `is`, collection literals, records, member receivers) are
//! pinned here as the fixed face, so a later emit-side change cannot
//! silently drop one.
//!
//! The cluster this ticket fixes is the `[]` application's ARGUMENT: the
//! index expression of `list[x]`/`list[x] = v` is the operator's argument,
//! and the engine's receiver face covers only the row BEFORE the bracket
//! (`_reportUnresolvedIndex`, `property_element_resolver.dart:181`), so a
//! `void`-typed index reported nothing. `row_resolution.rs`'s
//! `check_index_argument_void` walks the rows strictly inside the bracket
//! pair and reports the outermost one.
//!
//! The `v() + 1` operand (`mark_operator` tags only `Intent::Read` rows, so
//! an invoked left operand carries no operator mark) is pinned here too:
//! `fold_binary`'s by-range operand fallback answers it, and the pins below
//! keep every arithmetic spelling on that path.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block; the module
//! stays inside the `diagnose` subtree so `crate::diagnose::test_rig`
//! resolves.

use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};
use crate::diagnose::types::FileReport;

/// The `use_of_void_result` rows of `report` as `(offset, length)`, in the
/// fixture's own coordinates, ascending.
fn void_spans(report: &FileReport) -> Vec<(u32, u32)> {
    let shift = u32::try_from(CORE_PRELUDE.len()).expect("prelude fits u32");
    let mut spans: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "use_of_void_result")
        .map(|d| (d.range.offset - shift, d.range.length))
        .collect();
    spans.sort_unstable();
    spans
}

/// The `(offset, length)` of the LAST `needle` in `fixture`.
fn span_of(fixture: &str, needle: &str) -> (u32, u32) {
    let at = u32::try_from(fixture.rfind(needle).expect("needle")).expect("offset fits u32");
    (at, u32::try_from(needle.len()).expect("len fits u32"))
}

/// The `(offset, 1)` of the LAST `needle` — a one-token anchor (a bare
/// identifier operand, or the `.at(methodName)` anchor
/// `checkForUseOfVoidResult` gives a `MethodInvocation` operand, which is
/// what an invoked expression's row spans.
fn callee_span(fixture: &str, needle: &str) -> (u32, u32) {
    let at = u32::try_from(fixture.rfind(needle).expect("needle")).expect("offset fits u32");
    (at, 1)
}

/// The `(offset, 1)` of the LAST `needle` whose anchor is the token at its
/// head (a bare identifier operand).
fn token_span(fixture: &str, needle: &str) -> (u32, u32) {
    let at = u32::try_from(fixture.rfind(needle).expect("needle")).expect("offset fits u32");
    (at, 1)
}

/// The single `void` row of `fixture`, asserting there is exactly one.
fn sole_void(fixture: &str) -> (u32, u32) {
    let spans = void_spans(&report_ops(fixture));
    assert_eq!(spans.len(), 1, "expected one void row: {spans:?}");
    spans[0]
}

#[test]
fn ticket_3316_index_argument_reports() {
    // `list[x]` — the read form of the `[]` formal's argument.
    let fixture = "void f(List list, void x) {\n  list[x];\n}";
    assert_eq!(sole_void(fixture), token_span(fixture, "x];"));
}

#[test]
fn ticket_3316_index_argument_in_assignment_reports() {
    // The write form passes the same argument to `[]=`.
    let fixture = "void f(List list, void x) {\n  list[x] = null;\n}";
    assert_eq!(sole_void(fixture), token_span(fixture, "x] = null"));
}

#[test]
fn ticket_3316_index_argument_compound_reports() {
    // A compound application reads the argument once, before the write.
    let fixture = "void f(List list, void x) {\n  list[x] += 1;\n}";
    assert_eq!(sole_void(fixture), token_span(fixture, "x] += 1"));
}

#[test]
fn ticket_3316_index_argument_invocation_anchors_on_the_callee() {
    // An invoked index expression reports at its method name
    // (`.at(methodName)`), the same anchor the binary operand face uses.
    let fixture = "void v() {}\nvoid f(List list) {\n  list[v()];\n}";
    let spans = void_spans(&report_ops(fixture));
    assert_eq!(spans, vec![callee_span(fixture, "v()")], "callee-name anchor");
}

#[test]
fn ticket_3316_index_argument_void_typed_list_stays_silent() {
    // `<void>[x]`: the element type is `void`, so the value is used — the
    // ARGUMENT is a plain int here, so nothing reports.
    let fixture = "void f(List list) {\n  list[0];\n}";
    assert!(void_spans(&report_ops(fixture)).is_empty());
}

#[test]
fn ticket_3316_index_receiver_still_reports() {
    // The receiver face beside the new argument arm is untouched: `x[0]`
    // anchors on the whole bracket pair (`_reportUnresolvedIndex`).
    let fixture = "void f(void x) {\n  x[0];\n}";
    assert_eq!(sole_void(fixture), span_of(fixture, "[0]"));
}

#[test]
fn ticket_3316_binary_arithmetic_invocation_operands_report() {
    // The #3234 residual: `mark_operator` tags only `Intent::Read` rows, so
    // an invoked left operand keeps `Intent::Invoke` and the operator mark
    // never matches. `fold_binary`'s by-range fallback answers every
    // arithmetic spelling, each exactly once.
    for spelling in ["+", "*", "-", "/"] {
        let fixture = format!("void v() {{}}\nvoid g() {{\n  v() {spelling} 1;\n}}");
        let spans = void_spans(&report_ops(&fixture));
        assert_eq!(
            spans,
            vec![callee_span(&fixture, "v()")],
            "`v() {spelling} 1`"
        );
    }
}

#[test]
fn ticket_3316_binary_relation_invocation_operand_reports() {
    let fixture = "void v() {}\nvoid g() {\n  v() == 1;\n}";
    assert_eq!(sole_void(fixture), callee_span(fixture, "v()"));
}

#[test]
fn ticket_3316_non_void_operand_stays_silent() {
    let fixture = "int f() => 1;\nvoid g() {\n  f() + 1;\n}";
    assert!(void_spans(&report_ops(fixture)).is_empty());
}

#[test]
fn ticket_3316_argument_and_assignment_faces_report() {
    let argument = "void g(dynamic x) {}\nvoid f(void x) {\n  g(x);\n}";
    assert_eq!(sole_void(argument), token_span(argument, "x);"));
    let assignment = "void v() {}\nvoid f() {\n  var a;\n  a = v();\n}";
    assert_eq!(sole_void(assignment), callee_span(assignment, "v();"));
}

#[test]
fn ticket_3316_property_target_reports() {
    // A property access consumes its target's value and anchors on the
    // SELECTOR (`.at(propertyName)`, property_element_resolver.dart:544).
    let fixture = "void f(void x) {\n  x.foo;\n}";
    assert_eq!(sole_void(fixture), span_of(fixture, "foo"));
}

#[test]
fn ticket_3316_discards_stay_silent() {
    // An expression statement's value is discarded, and a `void`-typed
    // declaration target accepts a `void` initializer — no consumption.
    for fixture in [
        "void f(void x) {\n  x;\n}",
        "void v() {}\nvoid g() {\n  v();\n}",
        "void f(void x) {\n  void v = x;\n}",
        "void g(void x) {}\nvoid f(void x) {\n  g(x);\n}",
    ] {
        assert!(
            void_spans(&report_ops(fixture)).is_empty(),
            "silent: {fixture:?}"
        );
    }
}
