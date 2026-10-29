//! The #2790 pins: the is-check constant-verdict decision (`_checkAllTypeChecks`,
//! best_practices_verifier.dart:987-1041) asking the strict mirror of
//! `TypeSystem.isSubtypeOf` (`_isSubtypeOf`, upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/element/subtype.dart 64-524) over the operand's
//! resolved (alias-folded, tear-off-typed) static type — both polarity
//! directions the wave-3 co-failure decomposition isolated:
//!
//! - A1 (over-proving): the verdict asked the assignability slot through
//!   the name-level lattice's same-head argument walk, whose legacy
//!   `Null`/top leniencies answer `Null → C` beyond the strict relation —
//!   `const <Null>[] is List<Object>` (the inlined co19 `Utils/expect.dart`
//!   `hasUnsoundNullSafety` getter, 1,755 solo cases) reported a
//!   constant-true the strict covariant leg refuses (`Null <: Object` is
//!   false, Left Null).
//! - A2 (under-proving): the operand never folded its alias and never
//!   typed tear-off reads — `f0 is F0<int>` / `m0 is F0<T>` over
//!   function/method tear-offs (the `function_type` suites, ~19 oracle
//!   entries per case) stayed informative.

use super::test_rig::{report_ops, report_ops_bare};
use super::*;

fn rows(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    let mut rows: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect();
    rows.sort_unstable();
    rows
}

fn type_check_rows(report: &FileReport) -> Vec<(u32, u32)> {
    rows(report, "unnecessary_type_check")
}

/// —— A1: the covariant argument leg is the strict ask ——

/// `const <Null>[] is List<Object>` (the inlined co19 `Utils/expect.dart`
/// `hasUnsoundNullSafety` getter, 1,755 solo cases): the covariant
/// argument leg asks `Null <: Object` and Left Null refuses — the old
/// assignable slot's legacy `Null` leniency fired the constant-true.
#[test]
fn list_of_null_is_not_list_of_object() {
    let src = concat!(
        "class C {\n",
        "  bool get hasUnsoundNullSafety => const <Null>[] is List<Object>;\n",
        "}\n",
    );
    let report = report_ops_bare(src);
    assert!(
        type_check_rows(&report).is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// Same head, identical arguments stay constant-true (reflexivity).
#[test]
fn list_of_null_is_list_of_null_reports() {
    let src = "void f() {\n  if (const <Null>[] is List<Null>) {}\n}\n";
    let report = report_ops(src);
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "{:?}",
        report.diagnostics
    );
}

/// `List<int> is List<num>`: the unmarked parameter's covariant default
/// (`TypeParameterElementImpl.variance` = `_variance ?? covariant`)
/// walks `int <: num` — the oracle reports the check always-true (the
/// #2462 `Box<int>`/`Box<num>` probe's List spelling).
#[test]
fn covariant_argument_fit_reports() {
    let src = concat!(
        "void f(List<int> l) {\n",
        "  if (l is List<num>) {}\n",
        "}\n",
    );
    let report = report_ops(src);
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "{:?}",
        report.diagnostics
    );
}

/// The misfit direction stays live: `List<num> is List<int>` asks
/// `num <: int` — no declared edge walks upward, the oracle is silent.
#[test]
fn covariant_argument_misfit_stays_live() {
    let src = concat!(
        "void f(List<num> l) {\n",
        "  if (l is List<int>) {}\n",
        "}\n",
    );
    let report = report_ops(src);
    assert!(
        type_check_rows(&report).is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// `List<int> is List` — the raw target reads `List<dynamic>` and the
/// covariant leg walks `int <: dynamic` (Right Top): the oracle
/// reports the check always-true.
#[test]
fn list_int_is_raw_list_reports() {
    let src = concat!("void f(List<int> l) {\n", "  if (l is List) {}\n", "}\n",);
    let report = report_ops(src);
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "{:?}",
        report.diagnostics
    );
}

/// A raw-typed operand against the raw head (`Set c1; c1 is Set`) is
/// constant-true — both sides read `Set<dynamic>`.
#[test]
fn raw_set_operand_is_raw_set_reports() {
    let src = concat!(
        "void f() {\n",
        "  Set c1 = {};\n",
        "  if (c1 is Set) {}\n",
        "}\n",
    );
    let report = report_ops(src);
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "{:?}",
        report.diagnostics
    );
}

/// —— A1: the top-type edges ——

/// `o is T` over an unannotated (dynamic) formal: `dynamic <: T` is no
/// fit (Left Top — only the tops fit), the oracle stays silent.
#[test]
fn dynamic_operand_is_type_parameter_stays_live() {
    let src = concat!(
        "class C<T> {\n",
        "  bool checkUsingIs(o) => o is T;\n",
        "}\n",
    );
    let report = report_ops_bare(src);
    assert!(
        type_check_rows(&report).is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// `a is Object?` over a `T` formal: Right Top (`Object?`) is
/// constant-true; the negated spelling maps to the 'false' row.
#[test]
fn type_parameter_is_object_question_reports() {
    let src = concat!("void f<T>(T a) {\n", "  if (a is Object?) {}\n", "}\n",);
    let report = report_ops(src);
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "{:?}",
        report.diagnostics
    );
}

/// `num? a; a is int?` stays live: Left Nullable needs `num <: int`
/// (`int <: num` is the declared edge, never the reverse).
#[test]
fn nullable_num_is_not_nullable_int() {
    let src = concat!("void f(num? a) {\n", "  if (a is int?) {}\n", "}\n",);
    let report = report_ops(src);
    assert!(
        type_check_rows(&report).is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// —— A2: the alias fold + tear-off operand faces ——

/// A top-level function tear-off against its own alias type:
/// `f0`'s static type is the declared signature; `F0<int>` folds to
/// the identical function shape — the oracle flags both rows.
#[test]
fn top_level_tearoff_is_own_alias_reports() {
    let src = concat!(
        "typedef F0<T> = int Function([Function x]);\n",
        "int f0([Function x]) => throw 'uncalled';\n",
        "void testF0() {\n",
        "  if (f0 is F0<int>) {}\n",
        "  if (f0 is int Function([Function x])) {}\n",
        "}\n",
    );
    let report = report_ops(src);
    assert_eq!(
        type_check_rows(&report).len(),
        2,
        "{:?}",
        report.diagnostics
    );
}

/// An instance method tear-off inside the class: `m0`'s static type is
/// the member's declared signature — both the alias spelling and the
/// expanded function spelling report.
#[test]
fn member_tearoff_is_own_alias_reports() {
    let src = concat!(
        "typedef F1<T> = List<Function> Function(int x);\n",
        "class C {\n",
        "  List<Function> m1(int x) => throw 'uncalled';\n",
        "  void testF1() {\n",
        "    if (m1 is F1<int>) {}\n",
        "    if (m1 is List<Function> Function(int x)) {}\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(src);
    assert_eq!(
        type_check_rows(&report).len(),
        2,
        "{:?}",
        report.diagnostics
    );
}

/// A generic member tear-off stays untyped (the instantiate-to-bounds
/// freshening is outside the declared-face model) — the under-report.
#[test]
fn generic_member_tearoff_stays_live() {
    let src = concat!(
        "typedef F4<T> = List<Function> Function(List<T> x);\n",
        "class C<T> {\n",
        "  List<Function> m4(List<T> x) => throw 'uncalled';\n",
        "  void testF4() {\n",
        "    if (m4 is F4<int>) {}\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(src);
    assert!(
        type_check_rows(&report).is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// A local declared with a function-typed alias against the same alias:
/// both sides fold to the identical function shape.
#[test]
fn alias_typed_local_is_own_alias_reports() {
    let src = concat!(
        "typedef Predicate<T> = bool Function(T value);\n",
        "void f(Predicate<int> p) {\n",
        "  if (p is Predicate<int>) {}\n",
        "}\n",
    );
    let report = report_ops(src);
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "{:?}",
        report.diagnostics
    );
}

/// An invocation operand keeps its call-result face — `confuse(f0)`
/// reads `dynamic` (the untyped helper formal), the oracle stays
/// silent there.
#[test]
fn invocation_operand_stays_live() {
    let src = concat!(
        "typedef F0<T> = int Function([Function x]);\n",
        "int f0([Function x]) => throw 'uncalled';\n",
        "confuse(f) => f;\n",
        "void testF0() {\n",
        "  if (confuse(f0) is F0<int>) {}\n",
        "}\n",
    );
    let report = report_ops(src);
    assert!(
        type_check_rows(&report).is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// —— polarity complements ——

/// A constant-true check under `is!` maps to the 'false' row — the
/// polarity pair shares the one registry code, the tier lives in the
/// message (`the result is always 'false'.`).
#[test]
fn negated_constant_true_reports_the_false_row() {
    let src = concat!("void f<T>(T a) {\n", "  if (a is! Object?) {}\n", "}\n",);
    let report = report_ops(src);
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "{:?}",
        report.diagnostics
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("always 'false'")),
        "{:?}",
        report.diagnostics
    );
}

/// A field read inside the class carries the declared type: `x is int`
/// over `int x` is constant-true.
#[test]
fn field_operand_is_own_declared_type_reports() {
    let src = concat!(
        "class C {\n",
        "  int x = 0;\n",
        "  bool t() => x is int;\n",
        "}\n",
    );
    let report = report_ops(src);
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "{:?}",
        report.diagnostics
    );
}

/// A supertype-edge question keeps the declared-edge walk:
/// `List<int> is Iterable<int>` is constant-true (`List<E> extends
/// Iterable<E>`).
#[test]
fn supertype_edge_reports() {
    let src = concat!(
        "void f(List<int> l) {\n",
        "  if (l is Iterable<int>) {}\n",
        "}\n",
    );
    let report = report_ops(src);
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "{:?}",
        report.diagnostics
    );
}
