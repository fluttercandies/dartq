//! The #2855 S3 pins: the over-proved `unnecessary_type_check` extras the
//! strict-subtype verdict answered where the oracle stays informative
//! (`_checkAllTypeChecks`, best_practices_verifier.dart:987-1041 — the
//! row fires only on a proven `leftType <: rightType`, never on a
//! disjointness proof).
//!
//! Faces, per cluster:
//!
//! - The named mixin application (`class MyTypedef<K, V> = A with
//!   Mixin1<K>, Mixin2<V>` — the `mixin/type_parameter1_test` shape): the
//!   operand's supertype walk must read the alias's SUBSTITUTED faces —
//!   `B<num, String> <: Mixin1<num>` is the oracle's proven pair, while
//!   `Mixin1<String>` stays informative. The bare application (`class
//!   C0<T> = S with M`, the `type_parameters_mixin_test` shape) decides
//!   the same way.
//! - The top-level function tear-off against the alias instantiation it
//!   matches and the one it refuses (`f15 is F15<int>` / `f15 is
//!   F15<bool>`, the `function_type` shape): a concrete tear-off proves
//!   only the instantiation its declared return spells.
//! - The generic-container member tear-off (`m15 is F15<int>` over a
//!   method whose signature mentions the class's `T`): the variable leaf
//!   proves nothing against a concrete argument — informative on both
//!   instantiations.

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

/// —— the mixin-application alias ——

/// `b is Mixin1<num>` over the alias chain (`B<num, String>` through
/// `class MyTypedef<K, V> = A with Mixin1<K>, Mixin2<V>`): the alias's
/// substituted face instantiates `Mixin1<num>` exactly — the oracle's
/// proven pair, flagged always-true.
#[test]
fn mixin_alias_positive_reports() {
    let report = report_ops(
        "mixin Mixin1<T> {}\n\
         mixin Mixin2<T> {}\n\
         class A {}\n\
         class MyTypedef<K, V> = A with Mixin1<K>, Mixin2<V>;\n\
         class B<K, V> extends MyTypedef<K, V> {}\n\
         void main() {\n\
         \x20  var b = new B<num, String>();\n\
         \x20  if (b is Mixin1<num>) {}\n\
         }",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// `b is! Mixin1<String>`: the alias's real instantiation carries
/// `Mixin1<num>`, so the negative stays informative — the args-blind
/// name-edge walk proved it before the substituted face existed (#2855's
/// over-proved extra).
#[test]
fn mixin_alias_negative_stays_informative() {
    let report = report_ops(
        "mixin Mixin1<T> {}\n\
         mixin Mixin2<T> {}\n\
         class A {}\n\
         class MyTypedef<K, V> = A with Mixin1<K>, Mixin2<V>;\n\
         class B<K, V> extends MyTypedef<K, V> {}\n\
         void main() {\n\
         \x20  var b = new B<num, String>();\n\
         \x20  if (b is! Mixin1<String>) {}\n\
         }",
    );
    assert!(
        type_check_rows(&report).is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// `c0 is M<int>` over the bare application (`class C0<T> = S with M`):
/// the raw `M` instantiation proves nothing against `M<int>` — the
/// `type_parameters_mixin_test` extra.
#[test]
fn bare_mixin_application_negative_stays_informative() {
    let report = report_ops(
        "mixin M<T> {}\n\
         class S {}\n\
         class C0<T> = S with M;\n\
         void main() {\n\
         \x20  var c0 = new C0();\n\
         \x20  if (c0 is M<int>) {}\n\
         }",
    );
    assert!(
        type_check_rows(&report).is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// `c0 is M` over the bare application: the raw face IS the mixin's raw
/// instantiation — the oracle's proven pair (`type_parameters_mixin_test`
/// line 40).
#[test]
fn bare_mixin_application_positive_reports() {
    let report = report_ops(
        "mixin M<T> {}\n\
         class S {}\n\
         class C0<T> = S with M;\n\
         void main() {\n\
         \x20  var c0 = new C0();\n\
         \x20  if (c0 is M) {}\n\
         }",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// —— the concrete tear-off against a function-alias instantiation ——

/// `f15 is F15<int>`: the tear-off's declared return matches the alias's
/// substituted face — the oracle's proven pair.
#[test]
fn top_level_tearoff_positive_reports() {
    let report = report_ops(
        "typedef F15<T> = List<T> Function(int, [List<int> x]) Function();\n\
         List<int> Function(int, [List<int> x]) f15() => throw '';\n\
         void main() {\n\
         \x20  if (f15 is F15<int>) {}\n\
         }",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// `f15 is F15<bool>`: the alias's substituted face carries
/// `List<bool>`; the tear-off's `List<int>` return refuses it —
/// informative (the `function_type` extra).
#[test]
fn top_level_tearoff_negative_stays_informative() {
    let report = report_ops(
        "typedef F15<T> = List<T> Function(int, [List<int> x]) Function();\n\
         List<int> Function(int, [List<int> x]) f15() => throw '';\n\
         void main() {\n\
         \x20  if (f15 is F15<bool>) {}\n\
         }",
    );
    assert!(
        type_check_rows(&report).is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// —— the generic-container member tear-off ——

/// `m is F<int>` over a method whose return mentions the class's own
/// parameter: the variable leaf proves nothing against a concrete
/// argument — informative on every instantiation (the `m15` extra).
#[test]
fn generic_member_tearoff_stays_informative() {
    let report = report_ops(
        "typedef F<T> = List<T> Function(int);\n\
         class C<T> {\n\
         \x20  List<T> Function(int) m() => throw '';\n\
         \x20  bool test() => m is F<int>;\n\
         }",
    );
    assert!(
        type_check_rows(&report).is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// —— the plain declared-edge positive keeps folding ——

/// `d is Base` over a plain `extends` edge: the name-level positive the
/// substituted ask never needed to answer — the keep-green of the
/// routing change.
#[test]
fn plain_extends_positive_reports() {
    let report = report_ops(
        "class Base {}\n\
         class D extends Base {}\n\
         void main() {\n\
         \x20  var d = new D();\n\
         \x20  if (d is Base) {}\n\
         }",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// —— the interface-cycle operand ——

/// `a is B` over the mutual `extends` cycle (`class A extends B`,
/// `class B extends A`): upstream clears both participants' supertypes
/// (upstream Dart SDK source,
/// pkg/analyzer/lib/src/summary2/interface_cycles.dart, the
/// `_markCircular` arm — the cycle element's `supertype` becomes
/// `objectType`), so `A <: B` reads `Object` and stays silent — the
/// cycle face dartq must not prove.
#[test]
fn mutual_extends_cycle_stays_informative() {
    let report = report_ops(
        "class A extends B {}\n\
         class B extends A {}\n\
         void main() {\n\
         \x20  var a = new A();\n\
         \x20  if (a is B) {}\n\
         }",
    );
    assert!(
        type_check_rows(&report).is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// `a is B` where only the FORWARD edge is the cycle member's cleared
/// supertype — a three-class triangle `A extends B`, `B extends C`,
/// `C extends A`: every element's supertype collapses to `Object`, so
/// `A <: B` is no fit.
#[test]
fn triangle_extends_cycle_stays_informative() {
    let report = report_ops(
        "class A extends B {}\n\
         class B extends C {}\n\
         class C extends A {}\n\
         void main() {\n\
         \x20  var a = new A();\n\
         \x20  if (a is B) {}\n\
         }",
    );
    assert!(
        type_check_rows(&report).is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// `c2 is SomeBase` where `C2 = S with C2` self-cycles and the tested
/// face is `S`: upstream resets `C2.supertype = Object` (its mixin
/// clause dropped), so `C2 <: S` is silent — the cleared-mixin face.
#[test]
fn self_mixin_cycle_to_super_stays_informative() {
    let report = report_ops(
        "class S {}\n\
         class C2 = S with C2;\n\
         void main() {\n\
         \x20  var c2 = new C2();\n\
         \x20  if (c2 is S) {}\n\
         }",
    );
    assert!(
        type_check_rows(&report).is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}
