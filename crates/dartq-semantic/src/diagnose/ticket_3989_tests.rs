//! #3989 / #3990 controls — the if-null composite join's accept and LUB
//! directions, plus the unstamped-send negative control beside the
//! chained-send stamp.
//!
//! * the `??` join's ACCEPT direction (`int? ?? int` → `int`): the same
//!   `LUB(NonNull(T1), T2)` verdict the foreign-join pin refuses through
//!   stays silent when the joined face fits the formal — the `Object`
//!   source arm in `assignable` decides misses only, never grants a
//!   pair the lattice did not already accept.
//! * the join's LUB spelling: `String ?? int` is `Object`, and the
//!   refusing row names it — the same composite under `int? ?? int`
//!   stays `int` upstream (`_resolveIfNull`,
//!   binary_expression_resolver.dart:218-252).
//! * the unstamped send's negative control (#3990): `self.member()`
//!   with no trailing group keeps its declared merged column `Object?
//!   Function()` — the stamped-send-result read arm in
//!   `expression_row_ty` answers `row.ty` only when it IS
//!   `callable_result_of(callee_return_ty)`, so an un-applied send
//!   re-derives the function face exactly as before.

use super::FileReport;
use super::test_rig::report_ops;

/// The messages of one code's rows, in source order.
fn texts(report: &FileReport, code: &str) -> Vec<String> {
    let mut rows = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .collect::<Vec<_>>();
    rows.sort_by_key(|d| d.range.offset);
    rows.iter().map(|d| d.message.to_string()).collect()
}

/// The `??` join's accept direction: `int? ?? int` joins `int`, which
/// `void f(int x)` takes without a word.
#[test]
fn if_null_composite_join_accepts_the_related_pair() {
    let report = report_ops(
        "void f(int x) {}\n\
         void m(int? a) {\n\
         \x20 f(a ?? 3);\n\
         }\n",
    );
    assert!(
        texts(&report, "argument_type_not_assignable").is_empty(),
        "LUB(NonNull(int?), int) is `int`, which the formal takes; got {:?}",
        texts(&report, "argument_type_not_assignable")
    );
}

/// The `??` join's LUB direction: `String? ?? int` joins `Object` —
/// upstream's `_resolveIfNull` answers `LUB(NonNull(T1), T2)`, never
/// the right operand's own type — and the refusing row names `Object`
/// (the `Object` source arm in `assignable` decides the miss).
#[test]
fn if_null_composite_join_spells_the_least_upper_bound() {
    let report = report_ops(
        "void f(int x) {}\n\
         void m(String? s) {\n\
         \x20 f(s ?? 3);\n\
         }\n",
    );
    let atna = texts(&report, "argument_type_not_assignable");
    assert_eq!(
        atna.len(),
        1,
        "LUB(NonNull(String?), int) is `Object`, which `int` refuses; got {atna:?}"
    );
    assert!(
        atna[0].contains("'Object'") && atna[0].contains("'int'"),
        "the row spells the joined `Object` against the `int` formal, got {atna:?}"
    );
}

/// The unstamped send's negative control: `self.member()` with no
/// trailing `()` group keeps the merged member column `Object?
/// Function()` — the applied stamp answers `row.ty` only when it is one
/// application deeper than the callee's face, so an un-applied send
/// re-derives the function column exactly as before (#3990). The
/// trailing-call pin beside it proves the stamped arm answers `Object?`.
///
/// The `implements B<dynamic Function()>` clause argument parses faithful
/// now that `scan_type_args`'s keyword arms carry the `Function()` tail
/// (#3994): DND1's substitution environment instantiates and
/// `self.member()` re-derives the merged `Object? Function()` column —
/// both the stamped and unstamped faces sit downstream of that scan.
#[test]
fn unstamped_member_send_still_rederives_the_function_column() {
    let report = report_ops(
        "class A<T> {\n\
         \x20 T member() => throw 'unreachable';\n\
         }\n\
         class B<T> {\n\
         \x20 T member() => throw 'unreachable';\n\
         }\n\
         void takesNum(num x) {}\n\
         class DND1 extends A<FutureOr<Object?> Function()> implements B<dynamic Function()> {\n\
         \x20 void test() {\n\
         \x20   var self = this;\n\
         \x20   var y = self.member();\n\
         \x20   takesNum(y);\n\
         \x20 }\n\
         }\n",
    );
    let atna = texts(&report, "argument_type_not_assignable");
    assert!(
        atna.iter().any(|m| m.contains("'Object? Function()'")),
        "the un-applied send re-derives the merged `Object? Function()` column, \
         got {atna:?}"
    );
}
