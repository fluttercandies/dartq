//! The #2854 S1c pins: the non-declaration operand faces at an
//! `unnecessary_type_check` is-check — the completed send's declared
//! return, the untyped collection literal's inferred element type, and
//! the closure literal's written-signature function type (upstream
//! `_checkAllTypeChecks` reads `leftNode.typeOrThrow`,
//! best_practices_verifier.dart:1011). Matching pairs fold to the
//! oracle's always-true row, non-matching pairs stay informative, and
//! the every-check suppress covers the unresolvable faces.

use crate::diagnose::test_rig::report_ops;
use crate::diagnose::types::FileReport;

/// The `unnecessary_type_check` rows of `report`, as `(offset, length)`.
fn type_check_rows(report: &FileReport) -> Vec<(u32, u32)> {
    let mut found: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unnecessary_type_check")
        .map(|d| (d.range.offset, d.range.length))
        .collect();
    found.sort_unstable();
    found
}

/// —— the completed send's static type ——

/// `(Class).toString() is String` (the `class/literal_test` census
/// shape): the send's static type is the invoked `toString`'s declared
/// return, and `String <: String` folds. The fixture declares the
/// member the real corpus inherits from `Object` — the type-literal
/// receiver's universal face rides the callee walk.
#[test]
fn class_literal_tostring_call_reports() {
    let report = report_ops(
        "class Class { String toString() => ''; }\n\
         \x20void main() {\n\
         \x20  if ((Class).toString() is String) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// `doNotReturn() is int` over an `int` callee (the
/// `DeadCodeTest/test_isExpression_type` shape): the top-level send's
/// declared return folds.
#[test]
fn top_level_call_result_reports() {
    let report = report_ops(
        "int doNotReturn() => 1;\n\
         \x20void main() {\n\
         \x20  if (doNotReturn() is int) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// The creation send's instance face (`new Foo() is Foo`, the
/// `class/cycle_test` shape): the created instance's head folds.
#[test]
fn creation_result_reports() {
    let report = report_ops(
        "class Foo {}\n\
         \x20void main() {\n\
         \x20  if (new Foo() is Foo) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// A non-matching call result stays informative: the `int` return
/// against a `String` target is a real runtime question.
#[test]
fn call_result_mismatch_stays_live() {
    let report = report_ops(
        "int doReturn() => 1;\n\
         \x20void main() {\n\
         \x20  if (doReturn() is String) {}\n\
         \x20}",
    );
    assert!(
        type_check_rows(&report).is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// —— the untyped collection literal's inferred type ——

/// `[] is List` (the `is/instanceof_test` pin): the empty list's
/// no-context type is `List<dynamic>`, and the bare-`List` target
/// folds.
#[test]
fn empty_list_reports() {
    let report = report_ops("void main() {\n  if ([] is List) {}\n}");
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// `[1, 2, 3] is List` (the `is/instanceof_test` pin): the element
/// rows' `int` faces infer `List<int>`, and the bare-`List` target
/// folds.
#[test]
fn int_list_reports_against_list() {
    let report = report_ops("void main() {\n  if ([1, 2, 3] is List) {}\n}");
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// The element type rides the checked arguments: `List<int>` against
/// `List<int>` folds.
#[test]
fn int_list_reports_against_list_int() {
    let report = report_ops("void main() {\n  if ([1, 2, 3] is List<int>) {}\n}");
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// The element type refuses: `List<int>` against `List<String>` is a
/// real runtime question.
#[test]
fn int_list_mismatch_stays_live() {
    let report = report_ops("void main() {\n  if ([1, 2, 3] is List<String>) {}\n}");
    assert!(
        type_check_rows(&report).is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// A mixed-element literal keeps the `dynamic` leaf: against
/// `List<Object>` the top face folds, against `List<int>` the
/// unprovable element stays informative (the oracle's `List<Object>`
/// inference answers the same pair).
#[test]
fn mixed_list_dynamic_leaf() {
    let fires = report_ops("void main() {\n  if ([1, 'a'] is List<Object>) {}\n}");
    assert_eq!(
        type_check_rows(&fires).len(),
        1,
        "rows: {:#?}",
        fires.diagnostics
    );
    let silent = report_ops("void main() {\n  if ([1, 'a'] is List<int>) {}\n}");
    assert!(
        type_check_rows(&silent).is_empty(),
        "rows: {:#?}",
        silent.diagnostics
    );
}

/// `{1, 2} is Set<int>`: the set-side literal infers its element the
/// same way.
#[test]
fn int_set_reports() {
    let report = report_ops("void main() {\n  if ({1, 2} is Set<int>) {}\n}");
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// `{1: 2} is Map<int, int>`: both entry sides infer.
#[test]
fn map_literal_reports() {
    let report = report_ops("void main() {\n  if ({1: 2} is Map<int, int>) {}\n}");
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// `{} is Map` (the `literal_return_ty` default): the empty braces
/// literal is a `Map<dynamic, dynamic>` in a null context.
#[test]
fn empty_braces_reports_as_map() {
    let report = report_ops("void main() {\n  if ({} is Map) {}\n}");
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// `{} is Set` stays informative: the empty braces' inferred face is a
/// map, and `Map<dynamic, dynamic>` proves nothing about `Set`.
#[test]
fn empty_braces_not_set() {
    let report = report_ops("void main() {\n  if ({} is Set) {}\n}");
    assert!(
        type_check_rows(&report).is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// —— the closure literal's written-signature function type ——

/// `(x) {} is Function`: any closure's function type is a subtype of
/// the bare `Function` interface.
#[test]
fn block_closure_reports_against_function() {
    let report = report_ops("void main() {\n  if ((x) {} is Function) {}\n}");
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// `((int x) {}) is void Function(int)`: the written formal annotation
/// rides the signature, the contravariant formal leg folds.
#[test]
fn annotated_formal_reports() {
    let report = report_ops("void main() {\n  if ((int x) {} is void Function(int)) {}\n}");
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// The written formal refuses: `void Function(String)`'s formal leg
/// asks `String <: int` — no provable fit, informative.
#[test]
fn annotated_formal_mismatch_stays_live() {
    let report = report_ops("void main() {\n  if ((int x) {} is void Function(String)) {}\n}");
    assert!(
        type_check_rows(&report).is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// The return column refuses: the block body's inferred return is no
/// `int`, so `int Function(int)` stays informative — the same silence
/// the oracle's `Null` inferred return answers.
#[test]
fn return_column_mismatch_stays_live() {
    let report = report_ops("void main() {\n  if ((int x) {} is int Function(int)) {}\n}");
    assert!(
        type_check_rows(&report).is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// `(List x) {} is F` over `typedef F = void Function(List);` (the
/// `function/type_alias6_test` shape): the aliased pair folds through
/// the written `List` formal.
#[test]
fn alias_param_closure_reports() {
    let report = report_ops(
        "typedef F = void Function(List);\n\
         \x20void main() {\n\
         \x20  if ((List x) {} is F) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}
