//! The #2859 pins: the assignment-side literal/closure face. A `var`
//! declarator's untyped collection or block-closure initializer writes
//! the initializer's own static type into the local's read row, so the
//! `is`-check verdict over the local folds where the oracle's
//! declaration inference makes it constant (`var x = {1, 2, 3}; x is
//! Set<int>`). Mismatching targets stay informative, an annotated
//! initializer keeps its exact written face, and the #2854 conventions
//! (the element-leaf least type, the closure's dynamic columns) carry
//! over unchanged.

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

/// —— the set literal initializer ——

/// `var x = {1, 2, 3}; x is Set<int>` (the co19 `set_literals_A02_t01`
/// shape): the declaration infers `Set<int>`, and the later is-check
/// folds.
#[test]
fn inferred_set_reports_against_element_type() {
    let report = report_ops(
        "void main() {\n\
         \x20  var x = {1, 2, 3};\n\
         \x20  if (x is Set<int>) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// The element type refuses: `Set<int>` against `Set<String>` is a real
/// runtime question.
#[test]
fn inferred_set_mismatch_stays_live() {
    let report = report_ops(
        "void main() {\n\
         \x20  var x = {1, 2, 3};\n\
         \x20  if (x is Set<String>) {}\n\
         \x20}",
    );
    assert!(
        type_check_rows(&report).is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// The bare-head target folds: `x is Set` over `Set<int>` (the
/// `exact_types_of_literals_A01_t03` shape's base ask).
#[test]
fn inferred_set_reports_against_bare_head() {
    let report = report_ops(
        "void main() {\n\
         \x20  var x = {1};\n\
         \x20  if (x is Set) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// `var c = const {1}; c is Set<int>`: the explicit-const literal
/// infers the same way.
#[test]
fn const_set_initializer_reports() {
    let report = report_ops(
        "void main() {\n\
         \x20  var c = const {1};\n\
         \x20  if (c is Set<int>) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// —— the list and map initializers ——

/// `var x = [1, 2]; x is List<int>`: the drained list leaves' faces
/// infer the element argument.
#[test]
fn inferred_list_reports() {
    let report = report_ops(
        "void main() {\n\
         \x20  var x = [1, 2];\n\
         \x20  if (x is List<int>) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// `var m = {1: 2}; m is Map<int, int>`: both entry sides infer.
#[test]
fn inferred_map_reports() {
    let report = report_ops(
        "void main() {\n\
         \x20  var m = {1: 2};\n\
         \x20  if (m is Map<int, int>) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// `var e = {}; e is Map`: the empty braces' map default (the
/// `literal_return_ty` no-context arm).
#[test]
fn empty_braces_initializer_reports_as_map() {
    let report = report_ops(
        "void main() {\n\
         \x20  var e = {};\n\
         \x20  if (e is Map) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// `var e = {}; e is Set` stays informative: the inferred face is a
/// map, and a map proves nothing about `Set`.
#[test]
fn empty_braces_initializer_not_set() {
    let report = report_ops(
        "void main() {\n\
         \x20  var e = {};\n\
         \x20  if (e is Set) {}\n\
         \x20}",
    );
    assert!(
        type_check_rows(&report).is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// —— the leaf conventions ——

/// `int` mixed with `double` lifts to `num`: `x is Set<num>` folds.
#[test]
fn int_double_leaves_lift_to_num() {
    let report = report_ops(
        "void main() {\n\
         \x20  var x = {1, 2.5};\n\
         \x20  if (x is Set<num>) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// The lift stays honest: `Set<num>` against `Set<int>` refuses.
#[test]
fn lifted_num_refuses_int() {
    let report = report_ops(
        "void main() {\n\
         \x20  var x = {1, 2.5};\n\
         \x20  if (x is Set<int>) {}\n\
         \x20}",
    );
    assert!(
        type_check_rows(&report).is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// —— the closure initializer ——

/// `var f = (int x) {}; f is void Function(int)`: the written formal
/// rides the parameter column, the contravariant leg folds.
#[test]
fn closure_initializer_reports() {
    let report = report_ops(
        "void main() {\n\
         \x20  var f = (int x) {};\n\
         \x20  if (f is void Function(int)) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// The written formal refuses the mismatch: `void Function(String)`'s
/// formal leg asks `String <: int` — informative.
#[test]
fn closure_initializer_mismatch_stays_live() {
    let report = report_ops(
        "void main() {\n\
         \x20  var f = (int x) {};\n\
         \x20  if (f is void Function(String)) {}\n\
         \x20}",
    );
    assert!(
        type_check_rows(&report).is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// An unannotated formal keeps the `dynamic` column: any argument type
/// folds against `Function` (the bare interface ask).
#[test]
fn unannotated_closure_reports_against_function() {
    let report = report_ops(
        "void main() {\n\
         \x20  var f = (x) {};\n\
         \x20  if (f is Function) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// An arrow initializer stays under-reported: the return column is the
/// body expression's type, no `dynamic` stand-in answers for it.
#[test]
fn arrow_initializer_stays_live() {
    let report = report_ops(
        "void main() {\n\
         \x20  var f = () => 1;\n\
         \x20  if (f is int Function()) {}\n\
         \x20}",
    );
    assert!(
        type_check_rows(&report).is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// —— the annotated initializer stays exact ——

/// `Set<int> x = {1}; x is Set<int>` folds through the written
/// declaration type, and `x is Set<String>` refuses — the annotated
/// slot never re-infers.
#[test]
fn annotated_initializer_stays_exact() {
    let folds = report_ops(
        "void main() {\n\
         \x20  Set<int> x = {1};\n\
         \x20  if (x is Set<int>) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&folds).len(),
        1,
        "rows: {:#?}",
        folds.diagnostics
    );
    let refuses = report_ops(
        "void main() {\n\
         \x20  Set<int> x = {1};\n\
         \x20  if (x is Set<String>) {}\n\
         \x20}",
    );
    assert!(
        type_check_rows(&refuses).is_empty(),
        "rows: {:#?}",
        refuses.diagnostics
    );
}

/// —— the inference window ——

/// A declarator without an initializer stays unknown: `var x; x is
/// Set<int>` is a real runtime question.
#[test]
fn uninitialized_var_stays_live() {
    let report = report_ops(
        "void main() {\n\
         \x20  var x;\n\
         \x20  if (x is Set<int>) {}\n\
         \x20}",
    );
    assert!(
        type_check_rows(&report).is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// The inference rides the `var`-inference window: `var y = x;` over a
/// literal-inferred `x` binds `y` the same face.
#[test]
fn inferred_face_cascades_through_var_window() {
    let report = report_ops(
        "void main() {\n\
         \x20  var x = {1};\n\
         \x20  var y = x;\n\
         \x20  if (y is Set<int>) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// An inner same-name declaration shadows: the inner list initializer
/// answers the inner read, the outer set initializer the outer read.
#[test]
fn shadowed_declarations_keep_their_own_faces() {
    let report = report_ops(
        "void main() {\n\
         \x20  var x = {1};\n\
         \x20  if (true) {\n\
         \x20    var x = [1];\n\
         \x20    if (x is List<int>) {}\n\
         \x20  }\n\
         \x20  if (x is Set<int>) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        2,
        "rows: {:#?}",
        report.diagnostics
    );
}
