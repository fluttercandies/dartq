//! Ticket #3909 pins: the `engine_core.rs` remainder packet's landed
//! half — the `e!` pair's whole-node span and `Never` result, the `super`
//! operand guard, the `Function`-interface callee's `dynamic` dispatch,
//! the `this(...)` implicit-`call` send result, and the static-member
//! tear-off face.
//!
//! Counts quoted per pin are recomputed from the gated census
//! `census-20261104-092425` (`target/conformance/`, in-workspace only).
//! Offsets are UTF-16; `at` shifts by [`CORE_PRELUDE`] because the rig
//! carries the fixture behind it on one line. All fixtures are ASCII.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// Every `code` row of `report` as `(offset, length)`.
fn spans_of(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// Byte offset of `needle` in the fixture, shifted past the core
/// prelude.
fn at(source: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + source.find(needle).expect("needle in fixture")) as u32
}

// —— A: the always-fails row anchors the whole `e!` node ————————————
//
// `visitPostfixExpression` reports `nullCheckAlwaysFails.at(node)` over
// the WHOLE postfix expression (upstream Dart SDK source,
// `pkg/analyzer/lib/src/error/best_practices_verifier.dart`:752-754), so
// `null!` spans five columns from the operand's head — never the
// operator's one. The old bang-anchored span drifted every census row of
// the family (11 missing + 12 extra: `test_nullCheck_null`'s 3:3-vs-3:4
// pair and its siblings).

/// `null!` anchors at the `null` literal's head with the `!` inside the
/// extent. Fails before: the row anchored at the `!` with length 1.
#[test]
fn bang_on_null_anchors_whole_postfix_node() {
    let source = r#"
void f() {
  null!;
}
"#;
    let report = report_ops(source);
    let rows = spans_of(&report, "null_check_always_fails");
    assert_eq!(
        rows,
        vec![(at(source, "null!"), 5)],
        "{:#?}",
        report.diagnostics
    );
}

/// The assertion's own static type is `Never` (`promoteToNonNull(Null)`
/// is bottom), so the thrown expression assigns everywhere: `f(null!)`
/// reports ONLY the always-fails row, never an `argument_type_not_
/// assignable` beside it. Fails before: the rewrite kept `Null` and the
/// argument face over-reported (`f(null!)`'s extra row).
#[test]
fn bang_on_null_result_is_never_for_arguments() {
    let source = r#"
void f(int x) {}
void g() {
  f(null!);
}
"#;
    let report = report_ops(source);
    assert_eq!(
        spans_of(&report, "null_check_always_fails"),
        vec![(at(source, "null!"), 5)],
        "{:#?}",
        report.diagnostics
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name != "argument_type_not_assignable"),
        "{:#?}",
        report.diagnostics
    );
}

// —— B: the `super` operand never reports the null-check pair ————————
//
// `_checkForUnnecessaryNullAware` returns before the strictly-non-null
// question for a `SuperExpression` target (upstream Dart SDK source,
// `pkg/analyzer/lib/src/generated/error_verifier.dart`:7299-7302), and
// `nullCheckAlwaysFails` needs a `Null`-typed operand `super` never
// carries — the oracle stays silent on `super!` and `super()!` alike
// (co19 `null_check_operator_A04_t01`'s nine rows, `issue_41597`).

/// `super!` and `super!.member()` stage nothing. Fails before: the
/// never-null receiver staged `unnecessary_non_null_assertion` at every
/// `!`.
#[test]
fn super_bang_never_reports_unnecessary() {
    let source = r#"
class A {
  void foo() {}
}
class C extends A {
  void test() {
    super!;
    super!.foo();
  }
}
"#;
    let report = report_ops(source);
    assert!(
        spans_of(&report, "unnecessary_non_null_assertion").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

// —— C: a `Function`-interface callee's send is `dynamic` ————————————
//
// `InvocationInferrer.computeInvokeReturnType` (upstream Dart SDK
// source, `pkg/analyzer/lib/src/dart/resolver/invocation_inferrer.dart`
// :727-733) types every invoke whose callee face is not a function
// signature `dynamic`: a `Function`-typed parameter's send resolves no
// signature statically, its result is `dynamic` — never the interface
// spelling itself (co19 `null_check_operator_A02_t01`/`A02_t02`'s rows:
// `f2(42)!` stays silent because `dynamic` is not strictly non-null).

/// `f4(42)!` over a `Function`-typed parameter stays silent, and the
/// send's result assigns everywhere `dynamic` does. Fails before: the
/// result kept the `Function` interface face and staged
/// `unnecessary_non_null_assertion` plus `invalid_assignment`.
#[test]
fn function_interface_callee_send_is_dynamic() {
    let source = r#"
void g(Function f4) {
  f4(42)!;
  String s = f4(42);
}
"#;
    let report = report_ops(source);
    assert!(
        spans_of(&report, "unnecessary_non_null_assertion").is_empty(),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name != "invalid_assignment"),
        "{:#?}",
        report.diagnostics
    );
}

// —— F: the `this(...)` implicit-`call` send result —————————————————
//
// A `this`/`super` keyword invoke is the implicit-`call` dispatch on the
// enclosing instance: the invoked expression's face is the container's
// `thisType` and the send's static type is its `call` member's declared
// return (co19 `null_check_operator_A03_t02`/`A03_t03`'s `this(42)!`
// rows — the `String` result is strictly non-null).

/// `this(42)!` reads the `call` member's return — the unnecessary
/// assertion stages over it, and the send result names `String` in the
/// assignment face. Fails before: the keyword row kept `Unknown` and
/// both rows were missing.
#[test]
fn this_invoke_bang_reads_call_return() {
    let source = r#"
class C {
  String call(int v) => "x";
  void test() {
    this(42)!;
    int i = this(42);
  }
}
"#;
    let report = report_ops(source);
    assert_eq!(
        spans_of(&report, "unnecessary_non_null_assertion"),
        vec![(at(source, "this(42)!") + 8, 1)],
        "{:#?}",
        report.diagnostics
    );
    let invalid = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "invalid_assignment")
        .expect("this(42) names String against int");
    assert!(
        invalid.message.contains("'String'") && invalid.message.contains("'int'"),
        "{}",
        invalid.message
    );
}

// —— E: the static-member tear-off face through the container name ———
//
// `Identifier.staticType` over a static-method read is the method's
// tear-off signature (`A.foo` reads `void Function()`,
// UnnecessaryNonNullAssertionTest.test_nonNull_method's row), so the
/// `!` on it stages over a strictly non-null function type.

/// `A.sm!` and `A.foo!` stage the unnecessary assertion at each `!`.
/// Fails before: the container-name receiver answered no tear-off face
/// and both rows were missing.
#[test]
fn static_member_tearoff_face_answers() {
    let source = r#"
class A {
  static void foo() {}
  static int sm() => 2;
}
void f() {
  A.foo!;
  A.sm!;
}
"#;
    let report = report_ops(source);
    let mut rows = spans_of(&report, "unnecessary_non_null_assertion");
    rows.sort_unstable();
    // The row anchors at the `!` itself, length 1 — the oracle's own row
    // shape for the family (`unnecessaryNonNullAssertion` carries the
    // operator's one-column span).
    let bang = |needle: &str| at(source, needle) + needle.len() as u32 - 1;
    assert_eq!(
        rows,
        vec![(bang("A.foo!"), 1), (bang("A.sm!"), 1)],
        "{:#?}",
        report.diagnostics
    );
}
