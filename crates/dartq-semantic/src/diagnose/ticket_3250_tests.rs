//! #3250 pins — the void-callee face of a bare invocation send.
//!
//! Upstream `FunctionExpressionInvocationResolver.resolve` reads the
//! invoked expression's own static type before the callability walk
//! (upstream Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
//! function_expression_invocation_resolver.dart:86-96) and hands it to
//! `_checkForUseOfVoidResult` (:214-222): a callee whose static type IS
//! `void` — a `void`-annotated parameter or local, a `void`-typed
//! variable — reports `use_of_void_result` AT the callee expression and
//! leaves the invocation unresolved (`DynamicTypeImpl`), so no
//! non-function verdict follows.
//!
//! dartq read only the `Ty::Named` (alias / `Function` head) and record
//! faces of a bare callee and returned silently for every other shape, so
//! `void f(void x) { x(); }` produced nothing where the oracle fires once.
//! The emit now sits in `check_invocation_face`'s non-`Named` arm.
//!
//! The face's boundary is the callee's OWN type, never its result: a
//! function-typed variable (`void Function() f`) is a function value, so
//! `f();` stays the legal discard — the negatives below pin exactly the
//! shape the recon ticket mis-attributed to this family (probed against
//! the oracle: `void Function() f = () {}; void g() { f(); }` → silent).

use crate::diagnose::test_rig::{code_names, report_ops, rows, CORE_PRELUDE};

/// The message the oracle spells for every face (messages.yaml
/// `useOfVoidResult`).
const VOID_MSG: &str = "This expression has a type of 'void' so its value can't be used.";

/// The offsets of one code's rows, ascending.
fn offsets(report: &crate::diagnose::types::FileReport, code: &str) -> Vec<u32> {
    let mut at = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.range.offset)
        .collect::<Vec<_>>();
    at.sort_unstable();
    at
}

/// The source offset of the fixture's LAST `needle`, as a report offset
/// (the rig prepends [`CORE_PRELUDE`]).
fn at_last(fixture: &str, needle: &str) -> u32 {
    u32::try_from(CORE_PRELUDE.len() + fixture.rfind(needle).expect("needle")).expect("offset")
}

/// Every row of one code carries the oracle's message.
fn assert_void_msg(report: &crate::diagnose::types::FileReport) {
    assert!(
        report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "use_of_void_result")
            .all(|d| d.message == VOID_MSG),
        "{:?}",
        rows(report)
    );
}

/// A `void`-annotated parameter invoked bare: the callee `x` is `void`,
/// which is the report condition and nothing else — the invocation's own
/// (unresolved → `dynamic`) result face adds no row.
#[test]
fn ticket_3250_void_parameter_invocation_reports_at_callee() {
    let fixture = "\
void h(void x) {
  x();
}";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "use_of_void_result"),
        [at_last(fixture, "x()")],
        "{:?}",
        rows(&report)
    );
    assert_eq!(code_names(&report), ["use_of_void_result"], "{:?}", rows(&report));
    assert_void_msg(&report);
}

/// The same face through a `void`-annotated local: the local's declared
/// type is the callee expression's static type.
#[test]
fn ticket_3250_void_local_invocation_reports_at_callee() {
    let fixture = "\
void h() {
  void x = null;
  x();
}";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "use_of_void_result"),
        [at_last(fixture, "x()")],
        "{:?}",
        rows(&report)
    );
    assert_void_msg(&report);
}

/// A `void`-typed top-level variable invoked bare reports the same way.
#[test]
fn ticket_3250_void_top_level_variable_invocation_reports() {
    let fixture = "\
void v;
void h() {
  v();
}";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "use_of_void_result"),
        [at_last(fixture, "v()")],
        "{:?}",
        rows(&report)
    );
}

/// The negative the recon ticket's face description named as a positive: a
/// `void`-RETURNING function-typed variable is a function value, so its
/// invocation is the legal discard (oracle probe: silent).
#[test]
fn ticket_3250_void_function_typed_variable_discard_stays_silent() {
    let report = report_ops(
        "void Function() f = () {};\n\
         void g() {\n\
         \x20 f();\n\
         }",
    );
    assert!(code_names(&report).is_empty(), "{:?}", rows(&report));
}

/// The parameter spelling of the same negative.
#[test]
fn ticket_3250_void_function_typed_parameter_discard_stays_silent() {
    let report = report_ops("void h(void Function() f) {\n  f();\n}");
    assert!(code_names(&report).is_empty(), "{:?}", rows(&report));
}

/// The consumed-result shape stays one row, at the callee: upstream's
/// early return leaves the invocation `dynamic`, so the declaration's
/// assignability face never fires on top of the void report.
#[test]
fn ticket_3250_consumed_void_invocation_reports_once() {
    let fixture = "\
void h(void x) {
  var y = x();
}";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "use_of_void_result"),
        [at_last(fixture, "x()")],
        "{:?}",
        rows(&report)
    );
    let invalidity: Vec<&str> = code_names(&report)
        .into_iter()
        .filter(|code| *code != "unused_local_variable")
        .collect();
    assert_eq!(invalidity, ["use_of_void_result"], "{:?}", rows(&report));
}

/// Two void-typed callees report one row each, at their own callee.
#[test]
fn ticket_3250_two_void_invocations_report_each() {
    let fixture = "\
void h(void x) {
  x();
  x();
}";
    let report = report_ops(fixture);
    let second = at_last(fixture, "x()");
    let first = u32::try_from(CORE_PRELUDE.len() + fixture.find("x()").expect("first")).expect("offset");
    assert_eq!(
        offsets(&report, "use_of_void_result"),
        [first, second],
        "{:?}",
        rows(&report)
    );
}

/// Regression guard: calling a `void` function or a `void` method is the
/// ordinary discard — neither is a void-typed CALLEE.
#[test]
fn ticket_3250_void_function_and_method_sends_stay_silent() {
    let report = report_ops(
        "void v() {}\n\
         class A {\n\
         \x20 void m() {}\n\
         }\n\
         void h(A a) {\n\
         \x20 v();\n\
         \x20 a.m();\n\
         }",
    );
    assert!(code_names(&report).is_empty(), "{:?}", rows(&report));
}
