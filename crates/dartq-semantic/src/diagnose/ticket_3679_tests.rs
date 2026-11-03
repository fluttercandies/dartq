//! #3679 pins — the nullable-receiver prefix/operator faces
//! (`unchecked_use_of_nullable_value`): the prefix `-`/`~` whose operand
//! is a member/receiver/call expression (census shape 4.6) and the binary
//! application whose operand is a `?.`-shorted or nullable member result
//! (shape 4.8).
//!
//! Every expectation is the oracle row of the corresponding upstream
//! fixture (upstream Dart SDK source, `pkg/analyzer/test/...`), same
//! anchor and byte-verbatim message:
//!
//! * `prefix_expression_test.dart` `test_minus_no_nullShorting` /
//!   `test_tilde_no_nullShorting` — `-a?.foo` / `~a?.foo` report the
//!   METHOD-variant text at the operator token (`unary-`, `~`): the
//!   `PrefixExpression` parent picks the method-invocation face
//!   (type_property_resolver.dart:128-148, reached through
//!   prefix_expression_resolver.dart's `_resolve1`).
//! * `use_of_nullable_value_test.dart`
//!   `test_prefixExpression_minus_nullable` — `-this` inside
//!   `extension E on A?` reports the same method face at `-` (`this`'s
//!   static type is the written `on` type `A?`).
//! * `use_of_nullable_value_test.dart` `test_tripleShift_nullable` —
//!   `s?.length >>> 2` reports the OPERATOR-variant text at `>>>` (the
//!   `BinaryExpression` parent's variant).
//! * `why_not_promoted/nullable_operator_call_error_test.dart` —
//!   `c.bad + 1` over an `int?` field reports the operator face at `+`,
//!   and `-c.bad` the method face at `-`.
//! * `language/null_aware/prefix_not_shortening_test.dart` — `-c?.e()`
//!   and `~c?.e()` report the method face at the operator token (the
//!   `?.` null-shorts the whole call, never the prefix application).
//!
//! The negative controls pin the family boundary: a non-null operand
//! stays silent, a `?.`-less member operand whose own type is
//! non-nullable stays silent, and `-` on a `void`-typed operand still
//! reports the `unary-` method face (the existing void arm's own row —
//! the pin proves no double row lands at the anchor).

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

/// `-a?.foo` — the method-variant text at the `-` token, the member
/// selector itself silent (the `?.` access is legal; the operand's
/// static type `int?` is what the prefix application dereferences).
#[test]
fn prefix_minus_null_aware_member_reports_unary_method_face() {
    let fixture = "\
class A {
  int get foo => 0;
}

void f(A? a) {
  -a?.foo;
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "unchecked_use_of_nullable_value"),
        vec![(at("-a?.foo;", fixture), 1)],
    );
    assert_eq!(
        messages(&report, "unchecked_use_of_nullable_value"),
        vec![
            "The method 'unary-' can't be unconditionally invoked because the receiver can be 'null'."
        ],
    );
}

/// `~a?.foo` — the same method face with the `~` spelling.
#[test]
fn prefix_tilde_null_aware_member_reports_method_face() {
    let fixture = "\
class A {
  int get foo => 0;
}

void f(A? a) {
  ~a?.foo;
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "unchecked_use_of_nullable_value"),
        vec![(at("~a?.foo;", fixture), 1)],
    );
    assert_eq!(
        messages(&report, "unchecked_use_of_nullable_value"),
        vec![
            "The method '~' can't be unconditionally invoked because the receiver can be 'null'."
        ],
    );
}

/// `-c?.e()` / `~c?.e()` — the call's `?.` shorts the whole send; the
/// prefix operand types `int?` and the method face reports at `-`/`~`
/// (upstream `prefix_not_shortening_test`).
#[test]
fn prefix_operators_on_null_aware_sends_report() {
    let fixture = "\
class C {
  int e() => 0;
}

void f(C? c) {
  -c?.e();
  ~c?.e();
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "unchecked_use_of_nullable_value"),
        vec![
            (at("-c?.e();", fixture), 1),
            (at("~c?.e();", fixture), 1),
        ],
    );
    assert_eq!(
        messages(&report, "unchecked_use_of_nullable_value"),
        vec![
            "The method 'unary-' can't be unconditionally invoked because the receiver can be 'null'.",
            "The method '~' can't be unconditionally invoked because the receiver can be 'null'.",
        ],
    );
}

/// `-this` inside `extension E on A?` — `this`'s static type IS the
/// written `on` type (`A?`), so the method face reports at `-`
/// (upstream `UncheckedUseOfNullableValueInsideExtensionTest`).
#[test]
fn prefix_minus_this_inside_nullable_extension_reports() {
    let fixture = "\
class A {
  A operator -() => this;
}

extension E on A? {
  void bar() {
    -this;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "unchecked_use_of_nullable_value"),
        vec![(at("-this;", fixture), 1)],
    );
    assert_eq!(
        messages(&report, "unchecked_use_of_nullable_value"),
        vec![
            "The method 'unary-' can't be unconditionally invoked because the receiver can be 'null'."
        ],
    );
}

/// `-c.bad` over an `int?` field — the member expression's own type is
/// nullable without any `?.`, so the prefix operand reports the method
/// face at `-` (upstream `nullable_operator_call_error_test`).
#[test]
fn prefix_minus_nullable_member_result_reports() {
    let fixture = "\
class C2 {
  int? bad;
}

userDefinableUnaryOp(C2 c) {
  -c.bad;
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "unchecked_use_of_nullable_value"),
        vec![(at("-c.bad;", fixture), 1)],
    );
    assert_eq!(
        messages(&report, "unchecked_use_of_nullable_value"),
        vec![
            "The method 'unary-' can't be unconditionally invoked because the receiver can be 'null'."
        ],
    );
}

/// `s?.length >>> 2` — the `?.`-shorted member result widens to `int?`
/// for the operator's operand question: the operator-variant text at the
/// `>>>` token, 3 units long (upstream `test_tripleShift_nullable`).
#[test]
fn triple_shift_on_null_aware_member_reports_operator_face() {
    let fixture = "\
m(String? s) {
  s?.length >>> 2;
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "unchecked_use_of_nullable_value"),
        vec![(at(">>>", fixture), 3)],
    );
    assert_eq!(
        messages(&report, "unchecked_use_of_nullable_value"),
        vec![
            "The operator '>>>' can't be unconditionally invoked because the receiver can be 'null'."
        ],
    );
}

/// `x?.bitLength + 1` — the `+` face at the operator token (the same
/// `?.`-shorted operand the triple-shift pin carries).
#[test]
fn binary_plus_on_null_aware_member_reports_operator_face() {
    let fixture = "\
class A {
  int get bitLength => 0;
}

void f(A? x) {
  x?.bitLength + 1;
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "unchecked_use_of_nullable_value"),
        vec![(at("+ 1;", fixture), 1)],
    );
    assert_eq!(
        messages(&report, "unchecked_use_of_nullable_value"),
        vec![
            "The operator '+' can't be unconditionally invoked because the receiver can be 'null'."
        ],
    );
}

/// `x?[0] + 1` — the null-aware index's `?` shorts the whole access:
/// the operator face reports at `+` (the `]` value row joins the
/// `null_aware_members` latch at the fold).
#[test]
fn binary_plus_on_null_aware_index_reports_operator_face() {
    let fixture = "\
void f(List<int>? x) {
  x?[0] + 1;
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "unchecked_use_of_nullable_value"),
        vec![(at("+ 1;", fixture), 1)],
    );
    assert_eq!(
        messages(&report, "unchecked_use_of_nullable_value"),
        vec![
            "The operator '+' can't be unconditionally invoked because the receiver can be 'null'."
        ],
    );
}

/// `c.bad + 1` over an `int?` field — the member result's own `?`
/// answers the operand question without any `?.` in the chain (upstream
/// `nullable_operator_call_error_test`'s first row).
#[test]
fn binary_plus_on_nullable_member_result_reports() {
    let fixture = "\
class C1 {
  int? bad;
}

userDefinableBinaryOpLhs(C1 c) {
  if (c.bad == null) return;
  c.bad + 1;
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "unchecked_use_of_nullable_value"),
        vec![(at("+ 1;", fixture), 1)],
    );
    assert_eq!(
        messages(&report, "unchecked_use_of_nullable_value"),
        vec![
            "The operator '+' can't be unconditionally invoked because the receiver can be 'null'."
        ],
    );
}

/// The non-null controls: `-a.foo`, `~a.foo`, `x.bitLength + 1`, and
/// `x[0] + 1` on non-nullable receivers all stay silent — the `?.`-less
/// member access carries no operand question.
#[test]
fn non_nullable_operands_stay_silent() {
    let fixture = "\
class A {
  int get foo => 0;
  int get bitLength => 0;
}

void f(A a, A? x, List<int> l) {
  if (x == null) return;
  -a.foo;
  ~a.foo;
  x.bitLength + 1;
  l[0] + 1;
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "unchecked_use_of_nullable_value"),
        Vec::<(u32, u32)>::new(),
        "no unchecked rows on non-nullable operands"
    );
}

/// The `void` operand keeps the existing arm's own row: `-f()` over a
/// `void` send reports the `unary-` METHOD face once — the new pass
/// dedups against the staged row rather than double-reporting or
/// flipping the face to the operator variant.
#[test]
fn prefix_minus_void_send_keeps_single_method_face() {
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
