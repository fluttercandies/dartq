//! #3532 pins — spread elements never get an element type (K2+K3).
//!
//! # The defect
//!
//! `leaf_row_ty` only scanned `self.rows` and fell back to
//! `literal_leaf_ty`; list element rows drain into `deferred_miss_rows`
//! at literal close, so a spread's leaf type was invisible. Four helpers
//! lacked a spread arm: `least_leaf_ty`, `entry_leaf_tys`,
//! `exact_leaf_ty`, `least_leaf_ty_from`. K3: typed set/map literals
//! (`<int>{...}`) never bound — `push_typed_value_row` didn't drain
//! element rows so the single-row init window (`rows.len() == mark + 1`)
//! never triggered.
//!
//! # The contract
//!
//! A `...expr` element contributes the operand's ELEMENT type (upstream
//! Dart SDK source, `typed_literal_resolver.dart` `_computeElementType`'s
//! iterable arm): `Iterable`'s first argument where the operand's head
//! reaches `Iterable`/`List`/`Set`, `dynamic` where it does not. The
//! operand's own row reads the iterable whole, never the element.
//!
//! # The repro
//!
//! ```dart
//! var l = <int>[1, 2];
//! var lj = [...l];
//! take(lj);
//! ```
//!
//! Oracle answers no `argument_type_not_assignable` — `lj` reads
//! `List<int>`, matching `take`'s `List<int>` parameter. Before the fix
//! `lj` read `List<List<int>>` (the spread operand's whole type as the
//! leaf), a false error.

use super::test_rig::{assert_silent, code_names, report_ops};

/// The `(code, offset, len)` rows of `fixture`'s `argument_type_not_assignable`
/// and `return_of_invalid_type` verdicts — the false-error faces these
/// pins kill.
fn assignability_rows(fixture: &str) -> Vec<(&'static str, u32, u32)> {
    report_ops(fixture)
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == "argument_type_not_assignable"
                || d.code.name == "return_of_invalid_type"
                || d.code.name == "invalid_assignment"
        })
        .map(|d| (d.code.name, d.range.offset, d.range.length))
        .collect()
}

/// `var lj = [...l]` where `l` is `List<int>`: the spread's element type
/// binds `int`, so `lj` infers `List<int>` — passing `lj` to a
/// `List<int>` parameter and returning it from a `List<int>` function
/// stay silent. Before the fix `lj` read `List<List<int>>` (the spread
/// operand's whole type as the leaf), a false error on both.
#[test]
fn spread_of_list_infers_element_type_not_whole_type() {
    let l1 = "void take(List<int> x) {}\n";
    let l2 = "List<int> f() {\n";
    let l3 = "  var l = <int>[1, 2];\n";
    let l4 = "  var lj = [...l];\n";
    let l5 = "  take(lj);\n";
    let l6 = "  return lj;\n";
    let l7 = "}\n";
    let fixture = format!("{l1}{l2}{l3}{l4}{l5}{l6}{l7}");
    assert_eq!(
        assignability_rows(&fixture),
        [],
        "spread of `List<int>` must infer `List<int>`, not `List<List<int>>`"
    );
}

/// `var sj = {...l}` where `l` is `List<int>`: the spread's element type
/// binds `int`, so `sj` infers `Set<int>` — assigning `sj` to a
/// `Set<int>` stays silent. Before the fix `sj` read `Set<List<int>>`
/// (the spread operand's whole type), a false `invalid_assignment`.
#[test]
fn spread_of_list_in_set_infers_element_type() {
    let l1 = "var l = <int>[1, 2];\n";
    let l2 = "var sj = {...l};\n";
    let l3 = "Set<int> s = sj;\n";
    let fixture = format!("{l1}{l2}{l3}");
    assert_eq!(
        assignability_rows(&fixture),
        [],
        "spread of `List<int>` in a set literal must infer `Set<int>`, not `Set<List<int>>`"
    );
}

/// `[...?x, 1]` where `x` is `X extends Null`: the null-aware spread of a
/// `Null`-bound operand reads `dynamic` for the leaf (upstream's
/// `nullNone` arm), so the list infers `List<dynamic>` — no false
/// element-type row.
#[test]
fn null_aware_spread_of_null_bound_reads_dynamic() {
    let l1 = "void f<X extends Null>(X x) {\n";
    let l2 = "  var lj = [...?x, 1];\n";
    let l3 = "}\n";
    let fixture = format!("{l1}{l2}{l3}");
    let report = report_ops(&fixture);
    // `x` is a type parameter bound to `Null` — the null-aware spread's
    // operand reads `Null`, the `...?` admits it silently, and the leaf
    // contributes `dynamic`. No element-type or spread-kind row fires.
    let bad: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == "argument_type_not_assignable"
                || d.code.name == "invalid_assignment"
                || d.code.name == "list_element_type_not_assignable"
                || d.code.name == "not_iterable_spread"
        })
        .collect();
    assert!(bad.is_empty(), "unexpected rows: {bad:?}");
}

/// `[...myNever, 1]` where `myNever` is `Never`: `Never` is a subtype of
/// every type — the spread is legal, the leaf contributes `Never`
/// upstream, the LUB with `int` reads `dynamic` in the under-report
/// direction. No false row.
#[test]
fn spread_of_never_reads_dynamic_leaf() {
    let l1 = "void f(Never n) {\n";
    let l2 = "  var lj = [...n, 1];\n";
    let l3 = "}\n";
    let fixture = format!("{l1}{l2}{l3}");
    let report = report_ops(&fixture);
    let bad: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == "argument_type_not_assignable"
                || d.code.name == "invalid_assignment"
                || d.code.name == "list_element_type_not_assignable"
                || d.code.name == "not_iterable_spread"
        })
        .collect();
    assert!(bad.is_empty(), "unexpected rows: {bad:?}");
}

/// `<int>{...}` under `var`: the typed set literal's element type binds
/// through the drained element rows — `var sj = <int>{1, 2}` infers
/// `Set<int>`, so assigning `sj` to a `Set<int>` stays silent.
#[test]
fn typed_set_literal_binds_element_type() {
    let l1 = "var sj = <int>{1, 2};\n";
    let l2 = "Set<int> s = sj;\n";
    let fixture = format!("{l1}{l2}");
    assert_eq!(
        assignability_rows(&fixture),
        [],
        "typed `<int>{{...}}` must infer `Set<int>`, not `Set<dynamic>`"
    );
}

/// The 69-family positive: `x is List<int>` where `x` is `List<int>`
/// fires `unnecessary_type_check` — the spread element type is now
/// visible to the leaf helpers, so `[...l] is List<int>` reads the
/// literal's inferred `List<int>` and the check is constant-true.
#[test]
fn is_check_on_spread_literal_fires_unnecessary_type_check() {
    let l1 = "var l = <int>[1, 2];\n";
    let l2 = "var x = [...l];\n";
    let l3 = "var b = x is List<int>;\n";
    let fixture = format!("{l1}{l2}{l3}");
    let report = report_ops(&fixture);
    let names = code_names(&report);
    assert!(
        names.contains(&"unnecessary_type_check"),
        "expected unnecessary_type_check, got {names:?}"
    );
}

/// The silent twin: `x is List<String>` where `x` is `List<int>` stays
/// silent (the check is informative, not constant).
#[test]
fn is_check_mismatch_on_spread_literal_stays_silent() {
    let l1 = "var l = <int>[1, 2];\n";
    let l2 = "var x = [...l];\n";
    let l3 = "var b = x is List<String>;\n";
    let fixture = format!("{l1}{l2}{l3}");
    assert_silent(&report_ops(&fixture));
}

/// A spread inside a typed nested literal propagates the element type:
/// `var x = [<int>[...l]]` where `l` is `List<int>` infers
/// `List<List<int>>` — the inner literal's written face reads
/// `List<int>` as the outer's leaf. (The untyped-nested face —
/// `[[...l]]` — rides `exact_leaf_ty`'s nested-literal under-report,
/// a separate lane.)
#[test]
fn typed_nested_spread_literal_propagates_element_type() {
    let l1 = "var l = <int>[1, 2];\n";
    let l2 = "var x = [<int>[...l]];\n";
    let l3 = "List<List<int>> y = x;\n";
    let fixture = format!("{l1}{l2}{l3}");
    assert_eq!(
        assignability_rows(&fixture),
        [],
        "typed-nested spread must propagate `List<int>` as the inner element type"
    );
}

/// A spread of `Map<K,V>` in a map literal contributes the map's entry
/// pair: `var m = {...m0}` where `m0` is `Map<int, String>` infers
/// `Map<int, String>` — assigning `m` to `Map<int, String>` stays
/// silent.
#[test]
fn spread_of_map_infers_key_value_types() {
    let l1 = "var m0 = <int, String>{1: 'a'};\n";
    let l2 = "var m = {...m0};\n";
    let l3 = "Map<int, String> m2 = m;\n";
    let fixture = format!("{l1}{l2}{l3}");
    assert_eq!(
        assignability_rows(&fixture),
        [],
        "spread of `Map<int, String>` must infer `Map<int, String>`"
    );
}
