//! #3702 pins — `unary_void_operand_ty`'s two under-reported prefix faces:
//! a parenthesized group over a `void` read (`-(x)`) and a send through a
//! callable-typed formal (`-f()` on `void f()`), each grounding zero rows
//! before the fix.
//!
//! Every expectation is the oracle row of the corresponding upstream
//! fixture (upstream Dart SDK source, `pkg/analyzer/test/...`), same anchor
//! and byte-verbatim message:
//!
//! * `use_of_void_result_test.dart`
//!   `test_prefixExpression_minus_identifier_error`'s family — the prefix
//!   `unary-` application over a `void` operand reports
//!   `uncheckedMethodInvocationOfNullableValue` at the operator token
//!   (`type_property_resolver.dart`'s `isNullable` tail classifies `Void`
//!   as potentially nullable), beside the operand's own
//!   `use_of_void_result`.
//! * `use_of_void_result_test.dart`
//!   `test_prefixExpression_minus_invocation_error` — `void test(void f())`
//!   { -f(); }` reports the same method face at `-`: the
//!   `FunctionExpressionInvocation`'s `staticType` is the invoked
//!   signature's `returnType` (`Void`), never the callee's own
//!   `void Function()` face.
//!
//! The negative controls pin the boundary: a non-`void` group operand and
//! a non-`void` callable-typed callee stay silent, and the declared-
//! function `-f()` keeps exactly one `unary-` row (the resolved adjacent
//! arm's own verdict, never doubled by the teardown walk).

use super::FileReport;
use super::test_rig::{CORE_PRELUDE, report_ops};

/// Byte offset of `needle` in `fixture`, shifted past the rig's prelude
/// (ASCII fixtures: byte and UTF-16 offsets coincide).
fn at(needle: &str, fixture: &str) -> u32 {
    let hit = fixture
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in fixture:\n{fixture}"));
    CORE_PRELUDE.len() as u32 + hit as u32
}

/// The rows of one code as `(offset, length)` pairs, ascending.
fn rows(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    let mut at: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.end() - d.range.offset))
        .collect();
    at.sort_unstable();
    at
}

/// The messages of one code, in report order.
fn messages(report: &FileReport, code: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.message.to_string())
        .collect()
}

/// `-(x)` over a `void` formal — the group carries no `Void` face in the
/// value-leg tables (a `void` read lives in the `void_read_rows` side
/// table the shared join drops), so the operand grounded nothing before
/// the inner read's own void face was asked. Oracle: `unary-` at `-` and
/// `use_of_void_result` at `x`.
#[test]
fn prefix_minus_void_group_reports_unary_and_void_result() {
    let fixture = "\
void f(void x) {
  -(x);
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "unchecked_use_of_nullable_value"),
        vec![(at("-(x);", fixture), 1)],
        "exactly one unchecked row at the `-` anchor"
    );
    assert_eq!(
        messages(&report, "unchecked_use_of_nullable_value"),
        vec![
            "The method 'unary-' can't be unconditionally invoked because the receiver can be 'null'."
        ],
    );
    assert_eq!(
        rows(&report, "use_of_void_result"),
        vec![(at("(x)", fixture), 3)],
        "the operand's void-consumption row anchors at the group `(x)` — the oracle's `.at(expression)` (#3767)"
    );
}

/// `~(x)` — the `~` spelling's identical group contract.
#[test]
fn prefix_tilde_void_group_reports_method_face() {
    let fixture = "\
void f(void x) {
  ~(x);
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "unchecked_use_of_nullable_value"),
        vec![(at("~(x);", fixture), 1)],
    );
    assert_eq!(
        messages(&report, "unchecked_use_of_nullable_value"),
        vec!["The method '~' can't be unconditionally invoked because the receiver can be 'null'."],
    );
}

/// `-f()` on a `void f()` formal — the send's RESULT is `Void` even though
/// the callee's own face spells `void Function()` (a function-typed
/// formal's element-model slot). Upstream types the
/// `FunctionExpressionInvocation` by the invoked signature's return; the
/// oracle reports `unary-` at `-` plus `use_of_void_result` spanning the
/// whole `f()` invocation (upstream `at(expression)` — a value-callee
/// send anchors receiver-start through the closing `)`).
#[test]
fn prefix_minus_callable_formal_reports_unary_method_face() {
    let fixture = "\
void test(void f()) {
  -f();
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "unchecked_use_of_nullable_value"),
        vec![(at("-f();", fixture), 1)],
        "exactly one unchecked row at the `-` anchor"
    );
    assert_eq!(
        messages(&report, "unchecked_use_of_nullable_value"),
        vec![
            "The method 'unary-' can't be unconditionally invoked because the receiver can be 'null'."
        ],
    );
    assert_eq!(
        rows(&report, "use_of_void_result"),
        vec![(at("f();", fixture), 3)],
        "the send's void-consumption row spans `f()`"
    );
}

/// `~f()` on a `void f()` formal — the `~` spelling's identical contract.
#[test]
fn prefix_tilde_callable_formal_reports_method_face() {
    let fixture = "\
void test(void f()) {
  ~f();
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "unchecked_use_of_nullable_value"),
        vec![(at("~f();", fixture), 1)],
    );
    assert_eq!(
        messages(&report, "unchecked_use_of_nullable_value"),
        vec!["The method '~' can't be unconditionally invoked because the receiver can be 'null'."],
    );
}

/// The declared-function `-f()` keeps exactly one `unary-` row: the
/// resolved adjacent arm's verdict is the oracle's own, and the teardown
/// walk's callee arm must not add a twin at the same anchor.
#[test]
fn prefix_minus_declared_void_send_keeps_single_method_face() {
    let fixture = "\
void f() {}

void g() {
  -f();
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "unchecked_use_of_nullable_value"),
        vec![(at("-f();", fixture), 1)],
        "exactly one row at the `-` anchor"
    );
    assert_eq!(
        messages(&report, "unchecked_use_of_nullable_value"),
        vec![
            "The method 'unary-' can't be unconditionally invoked because the receiver can be 'null'."
        ],
    );
}

/// Negative controls: `-(x)` over an `int` formal and `-f()` on an
/// `int f()` formal carry no operand question — the group's inner face and
/// the send's result are both non-`void`, and the oracle stays silent.
#[test]
fn non_void_group_and_callable_operands_stay_silent() {
    let fixture = "\
void f(int x) {
  -(x);
}

void test(int f()) {
  -f();
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "unchecked_use_of_nullable_value"),
        Vec::<(u32, u32)>::new(),
        "no unchecked rows on non-void prefix operands"
    );
    assert_eq!(
        rows(&report, "use_of_void_result"),
        Vec::<(u32, u32)>::new(),
        "no void-consumption rows on non-void operands"
    );
}
