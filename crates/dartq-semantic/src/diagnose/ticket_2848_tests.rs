//! The #2848 S1b pins: the parameterized-alias fold chain at a
//! `unnecessary_type_check` is-operand — the tear-off operand's resolved
//! function type against the alias-folded checked type, the chain
//! `fold_parameterized_alias` -> `strict_subtype`'s Function arm ->
//! `fn_sig_subtype` (upstream `_checkAllTypeChecks` /
//! `TypeSystem::type_check_verdict`, best_practices_verifier.dart:987-1041).
//!
//! Faces, in the order the chain was diagnosed:
//!
//! - The old-style GENERIC function-type alias (`typedef int f1<T>(...)`)
//!   — the declaration index keeps no right-hand face for it, so the fold
//!   answers through the bounds capture's written shape (the same
//!   substitution the `=`-form's captured RHS rides): the matching
//!   `baz is f1<int>` pair reports, the misfit and the raw use stay
//!   informative.
//! - The qualified member tear-off (`b.foo is F<bool>` over a typed
//!   receiver, `super.remove is RemoveFunctionType<String, String>`) —
//!   the member read's static type is the member's declared signature as
//!   a function type, instantiated for the receiver.
//! - The nested generic groups (`f22 is F22<int>` over
//!   `typedef F22<T> = Function<A>(List<T> x) Function(int x)`) — the
//!   `<A>` group rides both sides' shapes and the count rule decides.
//! - A member returning through an alias folds its pieces (the resolved
//!   face the verdict reads); the reflexive pair reports, the
//!   non-provable one stays informative.

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

/// —— the old-style generic alias fold ——

/// `baz is f1<int>` over `typedef int f1<T>([int? a, int? b, T? c]);`
/// (the `type_alias2_test` shape): the tear-off's optional-positionals
/// signature and the alias's substituted right-hand face coincide — the
/// oracle flags the check always-true.
#[test]
fn old_style_generic_alias_tearoff_reports() {
    let report = report_ops(
        "typedef int f1<T>([int? a, int? b, T? c]);\n\
         \x20int baz([int? a, int? b, int? c]) => -1;\n\
         \x20void main() {\n\
         \x20  if (baz is f1<int>) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// The named-formals spelling (`bar is f3<int>` over
/// `typedef int f3<T>({int? a, int? b, T? c});`): the named coverage
/// rides the substituted face the same way.
#[test]
fn old_style_named_formals_alias_tearoff_reports() {
    let report = report_ops(
        "typedef int f3<T>({int? a, int? b, T? c});\n\
         \x20int bar({int? a, int? b, int? c}) => -1;\n\
         \x20void main() {\n\
         \x20  if (bar is f3<int>) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// The misfit stays informative: `baz is f1<double>` asks `double? <:`
/// against an `int?` formal — no provable fit, the oracle is silent.
#[test]
fn old_style_generic_alias_misfit_stays_live() {
    let report = report_ops(
        "typedef int f1<T>([int? a, int? b, T? c]);\n\
         \x20int baz([int? a, int? b, int? c]) => -1;\n\
         \x20void main() {\n\
         \x20  if (baz is f1<double>) {}\n\
         \x20}",
    );
    assert!(
        type_check_rows(&report).is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// The raw use stays informative: `baz is f1` instantiates the alias to
/// bounds (`dynamic?` formals — no provable fit against `int?`), the
/// oracle is silent.
#[test]
fn old_style_generic_alias_raw_use_stays_live() {
    let report = report_ops(
        "typedef int f1<T>([int? a, int? b, T? c]);\n\
         \x20int baz([int? a, int? b, int? c]) => -1;\n\
         \x20void main() {\n\
         \x20  if (baz is f1) {}\n\
         \x20}",
    );
    assert!(
        type_check_rows(&report).is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// A parameter spelling that shadows a builtin (`typedef bool F<bool>(bool
/// a);` — the `type_alias4_test` shape): the substitution is name-based,
/// `F<bool>` reads `bool Function(bool)` and the equal tear-off pair
/// reports.
#[test]
fn param_shadowing_builtin_alias_tearoff_reports() {
    let report = report_ops(
        "typedef bool F<bool>(bool a);\n\
         \x20bool bar(bool a) => false;\n\
         \x20void main() {\n\
         \x20  if (bar is F<bool>) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// —— the qualified member tear-off ——

/// A class-method tear-off against a parameterized alias (`remove is
/// RemoveFunctionType<String, String>` — the `override_test` shape): the
/// bare member read types as the declared signature; the alias fold
/// answers `String Function(String)` and the equal pair reports. The
/// misfit `is!` stays live.
#[test]
fn class_method_tearoff_is_parameterized_alias_reports() {
    let report = report_ops(
        "typedef V RemoveFunctionType<K, V>(K key);\n\
         \x20class MapBase<K, V> {\n\
         \x20  K remove(K key) => throw 'uncalled';\n\
         \x20}\n\
         \x20class M extends MapBase<String, String> {\n\
         \x20  String remove(String key) => throw 'uncalled';\n\
         \x20  void test() {\n\
         \x20    if (remove is RemoveFunctionType<String, String>) {}\n\
         \x20    if (remove is! RemoveFunctionType<int, int>) {}\n\
         \x20  }\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// A typed receiver's method tear-off (`b.foo is F<bool>` over
/// `var b = new A<bool>()` — the `b.foo is F<bool>` face): the read
/// instantiates the container's parameter with the written argument, the
/// instantiated pair reports, and the misfit stays live.
#[test]
fn typed_receiver_member_tearoff_reports() {
    let report = report_ops(
        "typedef bool F<bool>(bool a);\n\
         \x20class A<T> {\n\
         \x20  T foo(T a) => throw 'uncalled';\n\
         \x20}\n\
         \x20void main() {\n\
         \x20  var b = new A<bool>();\n\
         \x20  if (b.foo is F<bool>) {}\n\
         \x20  if (b.foo is F<int>) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// The `super` tear-off (`super.remove is RemoveFunctionType<String,
/// String>`): the walk resolves the concrete dispatch target above the
/// class and the climb's clause substitution instantiates
/// `MapBase<String, String>.remove` to the same `String Function(String)`
/// face — the equal pair reports.
#[test]
fn super_member_tearoff_is_parameterized_alias_reports() {
    let report = report_ops(
        "typedef V RemoveFunctionType<K, V>(K key);\n\
         \x20class MapBase<K, V> {\n\
         \x20  K remove(K key) => throw 'uncalled';\n\
         \x20}\n\
         \x20class M extends MapBase<String, String> {\n\
         \x20  String remove(String key) => throw 'uncalled';\n\
         \x20  void test() {\n\
         \x20    if (super.remove is RemoveFunctionType<String, String>) {}\n\
         \x20  }\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// —— the nested generic groups ——

/// `f22 is F22<int>` over `typedef F22<T> = Function<A>(List<T> x)
/// Function(int x)` (the `function_type9_test` shape): the return type's
/// own `<A>` group rides both sides' interned shapes, the alias fold
/// substitutes `List<int>` for `List<T>`, and the identical pair reports
/// — the expanded inline spelling alongside.
#[test]
fn nested_generic_tearoff_is_parameterized_alias_reports() {
    let report = report_ops(
        "typedef F22<T> = Function<A>(List<T> x) Function(int x);\n\
         \x20Function<A>(List<int> x) f22(int x) => throw 'uncalled';\n\
         \x20void main() {\n\
         \x20  if (f22 is F22<int>) {}\n\
         \x20  if (f22 is Function<A>(List<int> x) Function(int x)) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        2,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// The member spelling inside a generic container (`m23 is F23<T>` over
/// `typedef F23<T> = void Function<A>() Function(int x)` — the alias's
/// right-hand side never mentions `T`, so the raw in-scope spelling
/// substitutes to the identical face) and the top-level twin
/// (`f23 is F23<int>`): all three checks report.
#[test]
fn nested_generic_member_tearoff_reports() {
    let report = report_ops(
        "typedef F23<T> = void Function<A>() Function(int x);\n\
         \x20void Function<A>() f23(int x) => throw 'uncalled';\n\
         \x20class C<T> {\n\
         \x20  void Function<A>() m23(int x) => throw 'uncalled';\n\
         \x20  void testF23() {\n\
         \x20    if (f23 is F23<int>) {}\n\
         \x20    if (m23 is F23<T>) {}\n\
         \x20    if (m23 is void Function<A>() Function(int x)) {}\n\
         \x20  }\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        3,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// —— the alias-returning member pieces ——

/// A member returning through an alias (`C foo()` under
/// `typedef D C();` — the `type_alias6_test` shape): the tear-off's
/// pieces fold to the resolved face, so the reflexive pair reports —
/// `d.bar is C` reads `D Function()` against the folded `C` — while the
/// non-provable `d.foo is! C` (the tear-off `D Function() Function()`
/// is no subtype of `C`) stays informative per the official single-leg
/// verdict (`_checkAllTypeChecks`: only `isSubtypeOf(left, right)`
/// reports).
///
/// The upstream census flags this file's `d.foo is! C` line as an
/// oracle row whose face the visible source cannot reproduce (the
/// conformance case's materialized source is hash-pinned, not
/// inspectable); the seam stays open for the differential phase.
#[test]
fn alias_returning_member_tearoff_folds_pieces() {
    let report = report_ops(
        "typedef D C();\n\
         \x20class D {\n\
         \x20  C foo() => throw 'uncalled';\n\
         \x20  D bar() => throw 'uncalled';\n\
         \x20}\n\
         \x20void main() {\n\
         \x20  var d = new D();\n\
         \x20  if (d.foo is! C) {}\n\
         \x20  if (d.bar is C) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
    assert!(
        report.diagnostics.iter().any(|d| {
            d.code.name == "unnecessary_type_check" && d.message.contains("always 'true'")
        }),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// —— the no-regression net ——

/// The bare top-level tear-off over a `=`-form parameterized alias stays
/// green (`f0 is F0<int>` — the #2790 pin's face, re-pinned against the
/// piece-fold conversion).
#[test]
fn top_level_tearoff_is_own_alias_stays_green() {
    let report = report_ops(
        "typedef F0<T> = int Function([Function x]);\n\
         \x20int f0([Function x]) => throw 'uncalled';\n\
         \x20void testF0() {\n\
         \x20  if (f0 is F0<int>) {}\n\
         \x20  if (f0 is int Function([Function x])) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        2,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// A generic METHOD's tear-off stays untyped (the freshening posture):
/// `m4 is F4<int>` over `List<Function> m4(List<T> x)` keeps the
/// informative silence.
#[test]
fn generic_member_tearoff_stays_live() {
    let report = report_ops(
        "typedef F4<T> = List<Function> Function(List<T> x);\n\
         \x20class C<T> {\n\
         \x20  List<Function> m4(List<T> x) => throw 'uncalled';\n\
         \x20  void testF4() {\n\
         \x20    if (m4 is F4<int>) {}\n\
         \x20  }\n\
         \x20}",
    );
    assert!(
        type_check_rows(&report).is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// The class-parameter mismatch inside the member face stays live:
/// `m22 is F22<T>` over `Function<A>(List<int> x) m22(int x)` asks
/// `List<T>` against `List<int>` — no provable fit, the oracle is silent.
#[test]
fn class_param_misfit_member_tearoff_stays_live() {
    let report = report_ops(
        "typedef F22<T> = Function<A>(List<T> x) Function(int x);\n\
         \x20class C<T> {\n\
         \x20  Function<A>(List<int> x) m22(int x) => throw 'uncalled';\n\
         \x20  void testF22() {\n\
         \x20    if (m22 is F22<T>) {}\n\
         \x20  }\n\
         \x20}",
    );
    assert!(
        type_check_rows(&report).is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// An invocation operand keeps its call-result face — the `confuse`
/// spelling stays informative over the alias fold.
#[test]
fn invocation_operand_over_alias_stays_live() {
    let report = report_ops(
        "typedef F22<T> = Function<A>(List<T> x) Function(int x);\n\
         \x20Function<A>(List<int> x) f22(int x) => throw 'uncalled';\n\
         \x20confuse(f) => f;\n\
         \x20void main() {\n\
         \x20  if (confuse(f22) is F22<int>) {}\n\
         \x20}",
    );
    assert!(
        type_check_rows(&report).is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}
