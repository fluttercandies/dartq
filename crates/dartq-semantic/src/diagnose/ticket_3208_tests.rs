//! #3208 pins — the constructor send's call-site instantiation.
//!
//! An explicit creation (`new C(0)`, `const C(0)`) and a named-selector
//! creation (`C.of(0)`) carry no callee row: the constructor reference
//! resolves the head as a type use and the selector pushes none, so the
//! send-close solver never ran and the folded `Created` row kept the raw
//! head — `C<dynamic>` in every downstream display and judgment. Upstream
//! resolves the constructor's `FunctionType` through the same type
//! argument machinery as a method call (upstream Dart SDK source,
//! pkg/analyzer/lib/src/generated/resolver.dart
//! `visitInstanceCreationExpression`), so the created type is the class
//! instantiated with the inferred arguments: `C<int>`.
//!
//! The inference reads the group's own argument types through the
//! structural upward walk the member send's close solve runs
//! (`unify_upward`), fills a still-free parameter with its declared bound
//! or `dynamic` (the instantiate-to-bounds residual), and leaves the raw
//! head when no candidate survived.
//!
//! Every pin observes the created `Ty` through the rendered diagnostic
//! face: an `invalid_assignment` prints the value's resolved static type
//! (`DartType.getDisplayString`), which is exactly the instantiation
//! under test.

use super::test_rig::report_ops;
use crate::FileReport;

/// The rendered `Ty` of every `invalid_assignment` value, in report order.
fn assigned_types(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .map(|d| d.message.to_string())
        .collect()
}

/// Whether the fixture reported an `invalid_assignment` naming `ty`.
fn reports_value(report: &FileReport, ty: &str) -> bool {
    let needle = format!("A value of type '{ty}'");
    assigned_types(report)
        .iter()
        .any(|message| message.contains(&needle))
}

/// The unnamed keyword creation: `new C(0)` infers `C<int>` from the
/// constructor's `T` formal — `String s = c;` then rejects `C<int>`.
#[test]
fn keyword_creation_infers_type_argument() {
    let source = "\
class C<T> {
  C(T v);
}
void main() {
  var c = new C(0);
  String s = c;
}
";
    let report = report_ops(source);
    assert!(
        reports_value(&report, "C<int>"),
        "{:?}",
        report.diagnostics
    );
}

/// The `const` spelling takes the same arm: `const C(0)` answers
/// `C<int>` (the const keyword rides the same constructor reference).
#[test]
fn const_creation_infers_type_argument() {
    let source = "\
class C<T> {
  const C(T v);
}
void main() {
  var c = const C(0);
  String s = c;
}
";
    let report = report_ops(source);
    assert!(
        reports_value(&report, "C<int>"),
        "{:?}",
        report.diagnostics
    );
}

/// The named-selector creation infers through the selected
/// constructor's formals — `C<int>`, never the class's raw head.
#[test]
fn named_selector_creation_infers_type_argument() {
    let source = "\
class C<T> {
  C.of(T v);
}
void main() {
  var c = new C.of(0);
  String s = c;
}
";
    let report = report_ops(source);
    assert!(
        reports_value(&report, "C<int>"),
        "{:?}",
        report.diagnostics
    );
}

/// The class-typed named-selector spelling (`C<int>.of(0)`) keeps its
/// written instantiation — the inference never overrides explicit
/// arguments.
#[test]
fn typed_named_selector_keeps_written_arguments() {
    let source = "\
class C<T> {
  C.of(T v);
}
void main() {
  var c = C<int>.of(0);
  String s = c;
}
";
    let report = report_ops(source);
    assert!(
        reports_value(&report, "C<int>"),
        "{:?}",
        report.diagnostics
    );
}

/// The keyword-less generic named spelling (`C.of(0)` without `new`) is
/// the send path's shape: the callee row is the pre-merge qualified
/// selector and the name-level return face answers `Unknown` for a
/// generic head, so the enclosing `var` keeps `dynamic` — no
/// `invalid_assignment` here. The instantiation needs the send-path
/// fold (the class head row + selector row collapse into the creation's
/// value row), which lives in the send/declarator faces outside this
/// ticket's scope; pinned so the residual is counted, not silently lost.
/// The `new`-keyword spelling and the typed head both infer (above).
#[test]
fn unkeyed_named_selector_residual_stays_silent() {
    let source = "\
class C<T> {
  C.of(T v);
}
void main() {
  var c = C.of(0);
  String s = c;
}
";
    let report = report_ops(source);
    assert!(
        assigned_types(&report).is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// The negative pin: `new C()` carries no argument, so nothing constrains
/// `T` — the created type stays the instantiate-to-bounds face
/// `C<dynamic>` (never `C<int>`, never an invented binding).
#[test]
fn argumentless_creation_stays_to_bounds() {
    let source = "\
class C<T> {
  C();
}
void main() {
  var c = new C();
  String s = c;
}
";
    let report = report_ops(source);
    assert!(
        reports_value(&report, "C<dynamic>"),
        "{:?}",
        report.diagnostics
    );
}

/// No EXTRA row: a matching annotation accepts the inferred
/// instantiation (`C<int> c = new C(0)` is silent) — a wrongly solved
/// slot (`C<String>` and the like) would report here instead.
#[test]
fn inferred_instantiation_satisfies_matching_context() {
    let source = "\
class C<T> {
  C(T v);
}
void main() {
  C<int> c = new C(0);
}
";
    let report = report_ops(source);
    assert!(
        assigned_types(&report).is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// A partially constrained creation fills the free slot with its
/// instantiate-to-bounds face: `P(0)` over `P<T extends num, S>` answers
/// `P<int, dynamic>` (only `T` is constrained).
#[test]
fn partially_constrained_creation_fills_bounds() {
    let source = "\
class P<T extends num, S> {
  P(T v);
}
void main() {
  var p = new P(0);
  String s = p;
}
";
    let report = report_ops(source);
    assert!(
        reports_value(&report, "P<int, dynamic>"),
        "{:?}",
        report.diagnostics
    );
}

/// A function-typed argument constrains through its signature: `F<int>`
/// from `T Function(T)` against `int Function(int)`.
#[test]
fn closure_argument_infers_through_function_shape() {
    let source = "\
class F<T> {
  F(T Function(T) f);
}
void main() {
  var f = new F((int x) => x);
  String s = f;
}
";
    let report = report_ops(source);
    assert!(
        reports_value(&report, "F<int>"),
        "{:?}",
        report.diagnostics
    );
}

/// Disagreeing candidates keep no binding: `D(0, "x")` binds `T := int`
/// and `T := String`, so the created type keeps the raw head's
/// to-bounds face `D<dynamic>` rather than inventing a merge. (Upstream
/// answers the LUB `D<Object>`; the under-report direction is this
/// model's documented residual for a slot whose LUB it cannot compute.)
#[test]
fn disagreeing_arguments_keep_raw_head() {
    let source = "\
class D<T> {
  D(T a, T b);
}
void main() {
  var d = new D(0, \"x\");
  String s = d;
}
";
    let report = report_ops(source);
    assert!(
        reports_value(&report, "D<dynamic>"),
        "{:?}",
        report.diagnostics
    );
}
