//! #3695 pins — `emit_operand_face_row`'s prefix-operator dedup asked
//! `self.report.diagnostics`, a list `resolve_rows` has already drained
//! (`mem::take` before `filter_staged` runs), so the guard could never
//! fire. Evidence pins the question the dead code left open: the only
//! overlap it could have suppressed — a bare `-x`/`~x` operand the
//! resolved lane's adjacent arm already reports at the operator token —
//! never reaches `emit_operand_face_row`: `unary_void_operand_ty`'s own
//! `staged_checks` probe bails the bare head (the parse-time
//! `NeverReceiver` staging sits at the operand's offset), and the group
//! (`-(x)`)/callee (`-f()`) groundings cannot spell the operand row's
//! `Void` face the resolved arm fires on. The dead check is removed; the
//! structural comment in `emit_operand_face_row` documents why no
//! resolved-lane dedup is needed there.
//!
//! Oracle: upstream `use_of_void_result_test.dart`
//! (`test_prefixExpression_minus_identifier_error` /
//! `test_prefixExpression_minus_invocation_error`) — `-x` over `void x`
//! reports ONE `uncheckedMethodInvocationOfNullableValue` at `-` beside
//! the operand's `use_of_void_result`.

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

/// `-x` over a `void` formal — the resolved adjacent arm reports the
/// `unary-` method face at `-` and the operand's `use_of_void_result` at
/// `x`; the teardown walk must not add a second unchecked row at the same
/// anchor (the oracle's single-row contract).
#[test]
fn prefix_minus_void_bare_read_single_unchecked_row() {
    let fixture = "\
void f(void x) {
  -x;
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "unchecked_use_of_nullable_value"),
        vec![(at("-x;", fixture), 1)],
        "exactly one unchecked row at the `-` anchor"
    );
    assert_eq!(
        rows(&report, "use_of_void_result"),
        vec![(at("x;", fixture), 1)],
        "the operand's void-consumption row stays at `x`"
    );
}

/// `~x` over a `void` formal — the `~` spelling's identical contract.
#[test]
fn prefix_tilde_void_bare_read_single_unchecked_row() {
    let fixture = "\
void f(void x) {
  ~x;
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "unchecked_use_of_nullable_value"),
        vec![(at("~x;", fixture), 1)],
        "exactly one unchecked row at the `~` anchor"
    );
}

/// `-f()` over a `void` send — the callee-close shape: the resolved lane
/// answers the send's `Void` result row beside the prefix `-`. Exactly one
/// unchecked row at `-` — the dead `report.diagnostics` dedup never
/// suppressed a twin here because the teardown walk's callee arm does not
/// ground the declared-function callee's own face (it reads
/// `void Function()`, never `Void`).
#[test]
fn prefix_minus_void_send_single_unchecked_row() {
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
        "exactly one unchecked row at the `-` anchor"
    );
    assert_eq!(
        rows(&report, "use_of_void_result"),
        vec![(at("-f();", fixture) + 1, 1)],
        "the send's void-consumption row anchors at the call's method name"
    );
}

/// The for-in/spread/yield-each faces share `emit_operand_face_row` but
/// carry no resolved-lane counterpart — the dedup's removal must not
/// disturb their single-row contract (`for (var i in c.bad)` over
/// `List<int>?` still reports once at the iterable).
#[test]
fn statement_operand_faces_unchanged() {
    let fixture = "\
class C {
  List<int>? bad;
}

void f(C c) {
  for (var i in c.bad) {}
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "unchecked_use_of_nullable_value"),
        vec![(at("c.bad)", fixture), 5)],
        "the iterable face still reports once at `c.bad`"
    );
}

/// The null-aware spread (`...?`) never asks the operand face — a `...`
/// over a nullable iterable reports once at the operand (the verifier's
/// `!node.isNullAware` arm); the dedup's removal leaves the single row.
#[test]
fn spread_operand_face_reports_once() {
    let fixture = "\
void f(List<int>? xs) {
  var ys = [...xs];
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "unchecked_use_of_nullable_value"),
        vec![(at("xs]", fixture), 2)],
        "the spread face reports once at `xs`"
    );
}
