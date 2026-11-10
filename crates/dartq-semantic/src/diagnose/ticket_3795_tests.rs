//! The #3795 pins — `unused_element` / `unused_import` extra-dominated
//! clusters from the census sweep. Each test pins one oracle-verified
//! shape (dart analyze 3.13.7); comments name the cluster and upstream
//! face.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block.

use crate::diagnose::test_rig::{RigImport, report_ops, report_ops_imports};
use crate::diagnose::types::FileReport;

/// Every `unused_element` row in `report`, as `(offset, message)` pairs.
fn ue_rows(report: &FileReport) -> Vec<(u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_element")
        .map(|d| (d.range.offset, d.message.to_string()))
        .collect()
}

/// Every `unused_import` row in `report`, as `(offset, message)` pairs.
fn ui_rows(report: &FileReport) -> Vec<(u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_import")
        .map(|d| (d.range.offset, d.message.to_string()))
        .collect()
}

// --- Implicit `call` through an extension ---------------------------------

/// co19 `Extension-methods/call_member_t02`: literal receivers invoke the
/// extension `call` (`1(10)` → `on int`, `1.0(10)` → `on num`); the oracle
/// keeps both members used (unused_local_elements_verifier.dart
/// `visitFunctionExpressionInvocation`).
#[test]
fn literal_implicit_call_credits_the_applicable_extension() {
    let report = report_ops(
        r#"
extension on int {
  String call(num v) => "int($v)";
}
extension on num {
  String call(num v) => "num($v)";
}
void main() {
  1(10);
  1.0(10);
}
"#,
    );
    assert_eq!(ue_rows(&report), Vec::<(u32, String)>::new());
}

/// co19 `call_member_t10`: `this(42)` inside an extension body invokes the
/// extension's own `call` member.
#[test]
fn this_call_inside_extension_body_credits_call() {
    let report = report_ops(
        r#"
class C {}
extension on C {
  void test() {
    this(42);
  }
  String call(int v) => "call($v)";
}
void main() {
  C().test();
}
"#,
    );
    assert_eq!(ue_rows(&report), Vec::<(u32, String)>::new());
}

/// `ambiguous_extension_member_access/test_unnamed_extensions` (oracle):
/// `x()` over `List<C>` is ambiguous between `on List<A>` and `on List<B>`
/// (neither credits), while `g`/`h` apply each extension unambiguously —
/// both `call` members stay used.
#[test]
fn ambiguous_call_resolution_credits_each_unambiguous_application() {
    let report = report_ops(
        r#"
class A {}
class B {}
class C extends A implements B {}
extension on List<A> {
  int call() => 0;
}
extension on List<B> {
  int call() => 0;
}
int f(List<C> x) => x();
int g(List<A> x) => x();
int h(List<B> x) => x();
"#,
    );
    assert_eq!(ue_rows(&report), Vec::<(u32, String)>::new());
}

// --- Constructor-tear-off ambiguity operators -----------------------------

/// co19 `Constructor-tear-offs/ambiguities_A27_t02` family: `f(a<b, c> {})`
/// parses as comma-separated operator invocations (the `{` follower is not
/// a type-argument continuation), so `operator<` on `Function` and
/// `operator>` on `Type` stay used (unused_local_elements_verifier.dart
/// `visitBinaryExpression`).
#[test]
fn glued_comparison_followed_by_brace_credits_both_operators() {
    let report = report_ops(
        r#"
String f(a, [b]) => "$a, $b";
String a<T1, T2>(int x) => "a<$T1, $T2>($x)";
typedef b = int;
typedef c = String;
extension on Function {
  bool operator <(Type t) => true;
}
extension on Type {
  int operator >(int i) => i;
}
void main() {
  f(a<b,
      c> {});
}
"#,
    );
    assert_eq!(ue_rows(&report), Vec::<(u32, String)>::new());
}

/// The generic-instantiation read of the same shape (`(` follower): no
/// operator is applied, so both members still report — the disambiguation
/// must not over-credit.
#[test]
fn glued_comparison_followed_by_paren_still_reports_both_operators() {
    let report = report_ops(
        r#"
String f(a) => "$a";
String a<T1, T2>(int x) => "a<$T1, $T2>($x)";
typedef b = int;
typedef c = String;
extension on Function {
  bool operator <(Type t) => true;
}
extension on Type {
  int operator >(int i) => i;
}
void main() {
  f(a<b, c>(3));
}
"#,
    );
    let rows = ue_rows(&report);
    assert_eq!(
        rows.len(),
        2,
        "expected both operators unused, got {rows:?}"
    );
}

// --- Record-typed operand -------------------------------------------------

/// `test_expression_recordType_hasOperator` (oracle): a record-typed
/// operand resolves the extension `operator+` on the record face.
#[test]
fn record_operand_credits_extension_operator() {
    let report = report_ops(
        r#"
void f((String,) a) {
  a + 0;
}
extension on (String,) {
  int operator +(int other) => 0;
}
"#,
    );
    assert_eq!(ue_rows(&report), Vec::<(u32, String)>::new());
}

// --- `unused_import`: ambiguous bare resolution -----------------------------

/// `AmbiguousImportTest/test_is` (oracle): a bare name two imports export
/// resolves ambiguously — the use credits EVERY contributing edge, so
/// neither import reports. The single-resolution arm keeps its file match.
#[test]
fn ambiguous_bare_name_credits_every_exporting_edge() {
    let report = report_ops_imports(
        r#"
import 'package:t/lib1.dart';
import 'package:t/lib2.dart';
void f(p) {
  p is N;
}
"#,
        &[
            RigImport {
                uri: "package:t/lib1.dart",
                prefix: None,
                source: "class N {}",
            },
            RigImport {
                uri: "package:t/lib2.dart",
                prefix: None,
                source: "class N {}",
            },
        ],
    );
    assert_eq!(ui_rows(&report), Vec::<(u32, String)>::new());
}

/// The unambiguous read keeps the single-edge attribution: `N` resolves
/// through `lib1` alone, so `lib2` still reports — the ambiguous arm must
/// not over-credit a name only one edge exports.
#[test]
fn unambiguous_bare_name_leaves_the_other_edge_unused() {
    let report = report_ops_imports(
        r#"
import 'package:t/lib1.dart';
import 'package:t/lib2.dart';
void f(p) {
  p is N;
}
"#,
        &[
            RigImport {
                uri: "package:t/lib1.dart",
                prefix: None,
                source: "class N {}",
            },
            RigImport {
                uri: "package:t/lib2.dart",
                prefix: None,
                source: "class M {}",
            },
        ],
    );
    assert_eq!(ui_rows(&report).len(), 1, "lib2 must report unused");
}

// --- `unused_import`: metadata heads through a prefix -----------------------

/// `InvalidAnnotationTest/test_getter_importWithPrefix` (oracle): `@p.V`
/// resolves the annotation through the prefix scope — the import stays
/// used even though the annotation itself is invalid.
#[test]
fn prefixed_annotation_head_credits_the_import() {
    let report = report_ops_imports(
        r#"
import 'package:t/lib.dart' as p;
@p.V
void main() {}
"#,
        &[RigImport {
            uri: "package:t/lib.dart",
            prefix: Some("p"),
            source: "int get V => 0;",
        }],
    );
    assert_eq!(ui_rows(&report), Vec::<(u32, String)>::new());
}
