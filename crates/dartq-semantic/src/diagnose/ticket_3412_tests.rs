//! #3412 pins — the generic constructor send's static type
//! ([`Engine::callee_return_ty`]'s generic-constructor arm).
//!
//! A dot-send on a class head (`G.value(1)`, `G<int>.value(1)`) is an
//! `InstanceCreationExpression` upstream, so the invocation node's static
//! type is the CREATED instance — the head instantiated by the written
//! type arguments and the call-site inference. The name-level read cannot
//! run that inference itself, and it used to answer `Unknown` for every
//! generic head, so the send's result starved each consumer (the
//! assignment staging above all).
//!
//! The arm now answers the instantiation the creation machinery PUBLISHED
//! for the class segment (the written `<...>` group's `call_type_args`
//! entry, or a solved fold), keyed on the class head's own offset — the
//! walk from the merged selector back to its class identifier is
//! [`Engine::callee_return_ty`]'s private `creation_class_offset`. A
//! creation the machinery published nothing for (the argument-solved
//! spelling `G.value(1)`, whose solve the send close keeps to itself)
//! keeps `Unknown`: a raw head or a to-bounds fill stands in for an
//! instantiation nobody decided and stages rows the oracle never reports,
//! so the under-report face is pinned as hard as the positive one.

use super::test_rig::report_ops;
use super::*;

/// Every `code` row of `report` as `(offset, length, message)`.
fn rows_of(report: &FileReport, code: &str) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

/// The written instantiation is the send's own static type: `G<int>
/// .value(1)` types the created instance `G<int>`, so assigning it to a
/// `G<String>` slot reports the mismatch the oracle reports — the send
/// result, never the callee's absent return.
#[test]
fn written_instantiation_reaches_the_send_result() {
    let source = r#"
class G<T> {
  G.value(int n);
}
void f() {
  G<String> g = G<int>.value(1);
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "invalid_assignment");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    let message = &rows[0].2;
    assert!(
        message.contains("'G<int>'") && message.contains("'G<String>'"),
        "{message}"
    );
}

/// The class-segment walk crosses a multi-argument written group: the
/// `Pair<int, String>` head's balanced `<...>` (a comma inside) is skipped
/// on the way back to the class identifier, so the two-parameter creation
/// keys its own published instantiation and the send result stays
/// `Pair<int, String>`.
#[test]
fn multi_argument_instantiation_reaches_the_send_result() {
    let source = r#"
class Pair<K, V> {
  Pair.value(int n);
}
void f(String s) {
  s = Pair<int, String>.value(1);
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "invalid_assignment");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    let message = &rows[0].2;
    assert!(
        message.contains("'Pair<int, String>'") && message.contains("'String'"),
        "{message}"
    );
}

/// The non-generic control: a head with no parameters keeps the plain
/// created face (`D`, never `D<...>`), so the send result still types the
/// assignment it always did.
#[test]
fn non_generic_ctor_send_still_names_the_head() {
    let source = r#"
class D {
  D.named(int n);
}
void f(String s) {
  s = D.named(1);
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "invalid_assignment");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    let message = &rows[0].2;
    assert!(
        message.contains("'D'") && message.contains("'String'"),
        "{message}"
    );
}

/// The agreeing twin: the created `G<int>` fits the `G<int>` slot, so the
/// arm's positive answer stages nothing.
#[test]
fn agreeing_written_instantiation_stays_silent() {
    let source = r#"
class G<T> {
  G.value(int n);
}
void f(G<int> g) {
  g = G<int>.value(1);
}
"#;
    let report = report_ops(source);
    assert!(
        rows_of(&report, "invalid_assignment").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

/// The argument-solved spelling publishes its instantiation too: `G.value(1)`
/// solves `T = int` from the call, so the created `G<int>` reaches the
/// assignment staging and reports the `String`-slot mismatch the oracle
/// reports. (This pin's inverse used to assert the under-report — the solve
/// was computed then discarded before it could publish; #3415's named-selector
/// rider lands the `call_type_args` latch that lets `creation_row_ty` resolve
/// it, so the face now reports.)
#[test]
fn argument_solved_instantiation_reports_the_mismatch() {
    let source = r#"
class G<T> {
  G.value(T t);
}
void f(String s) {
  s = G.value(1);
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "invalid_assignment");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    let message = &rows[0].2;
    assert!(
        message.contains("'G<int>'") && message.contains("'String'"),
        "{message}"
    );
}
