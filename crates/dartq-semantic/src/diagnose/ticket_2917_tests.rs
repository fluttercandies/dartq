//! The #2917 pins: the increment/decrement index compound-write pair
//! (`a[0]++` reporting both `[]` and `[]=`), the relational pattern's
//! operator-definedness face (`case > 0` over an operator-less class),
//! and the constructor tear-off instantiation faces (the bare generic
//! form `C.new`, the receiver instantiation `C<int>.new`, and the
//! parenthesized `((C.new)<int>)`).
//!
//! Each pin cites its census case and expected.tsv row; the oracle texts
//! are never hand-crafted:
//! - `diagnostics/undefined_operator/UndefinedOperatorTest/
//!   test_indexBoth_undefined#0` — two `undefined_operator` rows at the
//!   bracket span (`[]` then `[]=`).
//! - `diagnostics/_misc/src/dart/resolution/relational_pattern/
//!   RelationalPatternResolutionTest/test_greaterThan_unresolved#0` (and
//!   the `<`/`>=`/`<=` twins) — `undefined_operator` at the pattern's
//!   operator token.
//! - `features/_core/co19/.../Constructor-tear-offs/
//!   unnamed_constructor_A05_t02` — three `unnecessary_type_check`
//!   rows: the bare generic tear-off, the receiver instantiation, and
//!   the parenthesized instantiation, each constant-true.

use super::test_rig::{report_ops, rows};

fn undefined_operators(report: &super::FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_operator")
        .map(|d| format!("{}|{}|{}", d.range.offset, d.range.length, d.message))
        .collect()
}

fn type_checks(report: &super::FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unnecessary_type_check")
        .map(|d| format!("{}|{}", d.range.offset, d.message))
        .collect()
}

/// `a[0]++` on a class without index operators — census
/// `UndefinedOperatorTest/test_indexBoth_undefined#0`: the compound write
/// needs BOTH spellings, both rows anchored at the bracket span.
#[test]
fn postfix_index_write_reports_both_spellings() {
    let report = report_ops("class A {}\nf(A a) {\n  a[0]++;\n}\n");
    let ops = undefined_operators(&report);
    assert_eq!(ops.len(), 2, "{:#?}", rows(&report));
    assert!(
        ops.iter()
            .any(|op| op.ends_with("The operator '[]' isn't defined for the type 'A'.")),
        "{ops:#?}"
    );
    assert!(
        ops.iter()
            .any(|op| op.ends_with("The operator '[]=' isn't defined for the type 'A'.")),
        "{ops:#?}"
    );
    let offsets: Vec<u32> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_operator")
        .map(|d| d.range.offset)
        .collect();
    let lengths: Vec<u32> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_operator")
        .map(|d| d.range.length)
        .collect();
    assert_eq!(offsets[0], offsets[1], "{ops:#?}");
    assert_eq!(lengths, vec![3, 3], "{ops:#?}");
}

/// `++a[0]` — the prefix junction resolves its operand for write with
/// read exactly like the postfix face (upstream
/// prefix_expression_resolver.dart:50), so the pair reports here too.
#[test]
fn prefix_index_write_reports_both_spellings() {
    let report = report_ops("class A {}\nf(A a) {\n  ++a[0];\n}\n");
    let ops = undefined_operators(&report);
    assert_eq!(ops.len(), 2, "{:#?}", rows(&report));
    assert!(
        ops.iter().any(|op| op.contains("'[]=' isn't defined")),
        "{ops:#?}"
    );
}

/// A class declaring both index operators keeps the increment silent.
#[test]
fn index_increment_with_declared_ops_stays_silent() {
    let report = report_ops(
        "class A {\n  int operator [](int i) => 0;\n  void operator []=(int i, int v) {}\n}\nf(A a) {\n  a[0]++;\n}\n",
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name != "undefined_operator"),
        "{:#?}",
        rows(&report)
    );
}

/// `case > 0` over an operator-less class — census
/// `RelationalPatternResolutionTest/test_greaterThan_unresolved#0`
/// (expected.tsv: `undefined_operator` at 6:10|1).
#[test]
fn relational_pattern_reports_undefined_greater_than() {
    let report = report_ops(
        "class A {}\n\nvoid f(A x) {\n  switch (x) {\n    case > 0:\n      break;\n  }\n}\n",
    );
    let ops = undefined_operators(&report);
    assert_eq!(ops.len(), 1, "{:#?}", rows(&report));
    assert!(
        ops[0].ends_with("The operator '>' isn't defined for the type 'A'."),
        "{ops:#?}"
    );
    assert!(ops[0].split('|').nth(1) == Some("1"), "{ops:#?}");
}

/// `case < 0` — the census `test_lessThan_unresolved#0` twin.
#[test]
fn relational_pattern_reports_undefined_less_than() {
    let report = report_ops(
        "class A {}\n\nvoid f(A x) {\n  switch (x) {\n    case < 0:\n      break;\n  }\n}\n",
    );
    let ops = undefined_operators(&report);
    assert_eq!(ops.len(), 1, "{:#?}", rows(&report));
    assert!(
        ops[0].ends_with("The operator '<' isn't defined for the type 'A'."),
        "{ops:#?}"
    );
    assert!(ops[0].split('|').nth(1) == Some("1"), "{ops:#?}");
}

/// A matched value of a type declaring the operator stays silent (the
/// rig prelude's `int` declares `<`/`>`).
#[test]
fn relational_pattern_on_int_stays_silent() {
    let report =
        report_ops("void f(int x) {\n  switch (x) {\n    case > 0:\n      break;\n  }\n}\n");
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name != "undefined_operator"),
        "{:#?}",
        rows(&report)
    );
}

/// `case == 5` never reports: both equality spellings resolve `operator
/// ==` on `Object` (the census `test_equal_ofClass#0` shape expects `-`).
#[test]
fn relational_pattern_equality_stays_silent() {
    let report = report_ops(
        "class A {}\n\nvoid f(A x) {\n  switch (x) {\n    case == 5:\n      break;\n  }\n}\n",
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name != "undefined_operator"),
        "{:#?}",
        rows(&report)
    );
}

/// An applicable extension operator resolves the pattern silent (the
/// census `test_greaterThan_ofExtension#0` shape expects `-`).
#[test]
fn relational_pattern_extension_operator_stays_silent() {
    let report = report_ops(
        "class A {}\nextension E on A {\n  bool operator >(int other) => true;\n}\nvoid f(A x) {\n  switch (x) {\n    case > 0:\n      break;\n  }\n}\n",
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name != "undefined_operator"),
        "{:#?}",
        rows(&report)
    );
}

/// `C.new` on a generic class — census `unnamed_constructor_A05_t02`
/// line 23 (`Expect.isTrue(C.new is C Function<X>(int i, X d))`): the
/// tear-off carries the class's own type parameters as its signature
/// group, and the positional unification proves the check constant-true.
#[test]
fn generic_class_ctor_tearoff_is_constant_true() {
    let report = report_ops(
        "class C<T> {\n  C(int i, T t) {}\n}\nvoid f() {\n  if (C.new is C Function<X>(int i, X d)) {}\n}\n",
    );
    let utc = type_checks(&report);
    assert_eq!(utc.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        utc[0].split('|').nth(1),
        Some("Unnecessary type check; the result is always 'true'.")
    );
}

/// `C<int>.new` — census `unnamed_constructor_A05_t02` line 24: the
/// written arguments instantiate the return head AND the class's own
/// parameter in the formals (`C<int> Function(int i, int d)`).
///
/// Ignored — the selector's written group `<int>` is a TYPE-head group,
/// and `end_type_arguments` keys `call_type_args` only for headless
/// groups (`head.is_some()` skips `call_args_end`, invocation.rs
/// #2908's domain); the `completed_group` latch (#610) arms for
/// declaration heads only (bounds.rs, same foreign domain). The selector
/// face (`ctor_selector_ty`) reads `receiver_group_args`, which
/// therefore answers `None` and the bare generic form stands — no
/// verdict. Un-ignores the moment the written group of an expression
/// head reaches the table.
// un-ignored 2026-11-07: passing per stale-ignore triage (ticket #3973)
#[test]
fn instantiated_receiver_tearoff_is_constant_true() {
    let report = report_ops(
        "class C<T> {\n  C(int i, T t) {}\n}\nvoid f() {\n  if (C<int>.new is C<int> Function(int i, int d)) {}\n}\n",
    );
    let utc = type_checks(&report);
    assert_eq!(utc.len(), 1, "{:#?}", rows(&report));
}

/// `((C.new)<int>)` — census `unnamed_constructor_A05_t02` line 25: the
/// type-argument group over the parenthesized tear-off instantiates the
/// same signature, the check against `C<int> Function(int i, int d)` is
/// constant-true.
///
/// Ignored — same unreachable-entry face as the receiver twin: the
/// headless group's capture dies before keying (`call_args_end` runs
/// only when the row under the group ABUTS the group's `<`, and the
/// paren rows' fold leaves no such anchor). The paren arm walks every
/// candidate anchor (`ctor_tear_off_ty`'s byte-walk closer, the operand
/// row's own anchor, the inner paren row's offset) and finds no entry —
/// `written` stays `None`, the bare generic form stands.
#[ignore = "same expression-head group-entry hole as the receiver twin (invocation.rs/bounds.rs, #2908 domain)"]
#[test]
fn instantiated_paren_tearoff_is_constant_true() {
    let report = report_ops(
        "class C<T> {\n  C(int i, T t) {}\n}\nvoid f() {\n  if (((C.new)<int>) is C<int> Function(int i, int d)) {}\n}\n",
    );
    let utc = type_checks(&report);
    assert_eq!(utc.len(), 1, "{:#?}", rows(&report));
}

/// A genuinely different signature stays informative: `C.new` (generic in
/// the class parameter) is no `C Function<X>(int i, String d)` — the
/// formal leg refuses, no constant verdict fires.
#[test]
fn generic_tearoff_mismatched_formal_stays_silent() {
    let report = report_ops(
        "class C<T> {\n  C(int i, T t) {}\n}\nvoid f() {\n  if (C.new is C Function<X>(int i, String d)) {}\n}\n",
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name != "unnecessary_type_check"),
        "{:#?}",
        rows(&report)
    );
}
