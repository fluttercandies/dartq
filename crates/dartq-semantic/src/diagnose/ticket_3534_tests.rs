//! #3534 pins — the seven spread/map-element codes that had no variant
//! (K5) and the emission faces that own them.
//!
//! # The defect
//!
//! `SemanticCode` carried no variant for `not_iterable_spread`,
//! `not_map_spread`, `not_null_aware_null_spread`,
//! `const_spread_expected_list_or_set`, `const_spread_expected_map`,
//! `expression_in_map`, or `set_element_type_not_assignable`; the
//! existing `map_key_type_not_assignable`/`map_value_type_not_assignable`
//! and `non_constant_map_element` had no emission point on the spread
//! and dropped-element paths. Every one of these was a silent gap in
//! the census's spread cluster.
//!
//! # The contract
//!
//! The upstream ladder (upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/literal_element_verifier.dart
//! `_verifySpreadForListOrSet`/`_verifySpreadForMap` and
//! `constant_verifier.dart` `_validateListOrSetSpread`/`_validateMapSpread`):
//! a `Dynamic`/`Never` operand is silent; a `Null` operand is silent under
//! `...?` and reports `not_null_aware_null_spread` under `...`; an
//! operand whose head does not reach `Iterable` reports
//! `not_iterable_spread` in a list/set literal and `not_map_spread` in a
//! map literal; an iterable operand whose element type is not assignable
//! to the literal's element type reports the element-type code. In a
//! `const` literal a non-collection operand reports the const-shape code
//! at the operand's span; a non-entry expression in a map literal
//! reports `expression_in_map`.
//!
//! # The repro
//!
//! ```dart
//! var a = [...1];
//! var b = <int, int>{...[1]};
//! ```
//!
//! Oracle answers `not_iterable_spread` at `1` and `not_map_spread` at
//! `[1]` — dartq reported nothing before this fix.

use super::test_rig::{assert_silent, code_names, report_ops};

/// Offset of `needle` inside the first occurrence of `line`.
fn at(source: &str, line: &str, needle: &str) -> u32 {
    let line_at = source
        .find(line)
        .unwrap_or_else(|| panic!("fixture has {line:?}"));
    let needle_at = line
        .find(needle)
        .unwrap_or_else(|| panic!("{line:?} has {needle:?}"));
    (line_at + needle_at) as u32
}

/// The `(code, offset, len)` rows of `fixture`'s diagnostics, filtered to
/// `wanted` codes.
fn rows_for(fixture: &str, wanted: &[&str]) -> Vec<(String, u32, u32)> {
    report_ops(fixture)
        .diagnostics
        .iter()
        .filter(|d| wanted.contains(&d.code.name))
        .map(|d| (d.code.name.to_string(), d.range.offset, d.range.length))
        .collect()
}

// ------------------------------------------------------------------
// not_iterable_spread
// ------------------------------------------------------------------

/// `var a = [...i]` where `i` is `int`: `int` does not implement
/// `Iterable`, so `asInstanceOf(Iterable)` misses and the oracle answers
/// `not_iterable_spread` at the operand.
#[test]
fn not_iterable_spread_fires_on_int_operand() {
    let l1 = "var i = 1;\n";
    let l2 = "var a = [...i];\n";
    let fixture = format!("{l1}{l2}");
    let rows = rows_for(&fixture, &["not_iterable_spread"]);
    assert_eq!(rows.len(), 1, "expected one not_iterable_spread: {rows:?}");
    let operand_at = at(&fixture, "var a", "i");
    assert_eq!(rows[0].1, operand_at, "row must anchor at the operand");
}

/// The silent twin: `var a = [...l]` where `l` is `List<int>` — the
/// operand implements `Iterable`, no row.
#[test]
fn not_iterable_spread_silent_on_list_operand() {
    let l1 = "var l = <int>[1];\n";
    let l2 = "var a = [...l];\n";
    let fixture = format!("{l1}{l2}");
    assert!(rows_for(&fixture, &["not_iterable_spread"]).is_empty());
}

// ------------------------------------------------------------------
// not_map_spread
// ------------------------------------------------------------------

/// `var b = <int, int>{...l}` where `l` is `List<int>`: `List` does not
/// implement `Map`, so `asInstanceOf(Map)` misses and the oracle answers
/// `not_map_spread` at the operand.
#[test]
fn not_map_spread_fires_on_list_operand() {
    let l1 = "var l = <int>[1];\n";
    let l2 = "var b = <int, int>{...l};\n";
    let fixture = format!("{l1}{l2}");
    let rows = rows_for(&fixture, &["not_map_spread"]);
    assert_eq!(rows.len(), 1, "expected one not_map_spread: {rows:?}");
}

/// The silent twin: `var b = <int, int>{...m}` where `m` is
/// `Map<int, int>` — the operand is a `Map`, no row.
#[test]
fn not_map_spread_silent_on_map_operand() {
    let l1 = "var m = <int, int>{1: 2};\n";
    let l2 = "var b = <int, int>{...m};\n";
    let fixture = format!("{l1}{l2}");
    assert!(rows_for(&fixture, &["not_map_spread"]).is_empty());
}

// ------------------------------------------------------------------
// not_null_aware_null_spread
// ------------------------------------------------------------------

/// `var c = [...n]` where `n` is `Null`: upstream's
/// `_verifySpreadForListOrSet` Null arm fires
/// `not_null_aware_null_spread` at the operand (the `...` without `?`).
#[test]
fn not_null_aware_null_spread_fires_on_null_operand() {
    let l1 = "Null n = null;\n";
    let l2 = "var c = [...n];\n";
    let fixture = format!("{l1}{l2}");
    let rows = rows_for(&fixture, &["not_null_aware_null_spread"]);
    assert_eq!(
        rows.len(),
        1,
        "expected one not_null_aware_null_spread: {rows:?}"
    );
}

/// The silent twin: `var c = [...?n]` where `n` is `Null` — the `?`
/// admits the null operand, no row.
#[test]
fn not_null_aware_null_spread_silent_under_null_aware_operator() {
    let l1 = "Null n = null;\n";
    let l2 = "var c = [...?n];\n";
    let fixture = format!("{l1}{l2}");
    assert!(rows_for(&fixture, &["not_null_aware_null_spread"]).is_empty());
}

// ------------------------------------------------------------------
// const_spread_expected_list_or_set
// ------------------------------------------------------------------

/// `const d = [...1]`: a `const` list's spread operand must evaluate to
/// a constant list or set — `1` evaluates to an int constant, so
/// upstream's `_validateListOrSetSpread` answers
/// `const_spread_expected_list_or_set` at the operand.
#[test]
fn const_spread_expected_list_or_set_fires_on_int_operand() {
    let fixture = "const d = [...1];\n".to_string();
    let names = code_names(&report_ops(&fixture));
    assert!(
        names.contains(&"const_spread_expected_list_or_set"),
        "expected const_spread_expected_list_or_set, got {names:?}"
    );
}

/// The silent twin: `const d = [...l]` where `l` is a const list — the
/// operand is a constant list, no row.
#[test]
fn const_spread_expected_list_or_set_silent_on_const_list() {
    let l1 = "const l = <int>[1];\n";
    let l2 = "const d = [...l];\n";
    let fixture = format!("{l1}{l2}");
    assert!(rows_for(&fixture, &["const_spread_expected_list_or_set"]).is_empty());
}

// ------------------------------------------------------------------
// const_spread_expected_map
// ------------------------------------------------------------------

/// `const e = <int, int>{...1}`: a `const` map's spread operand must be a
/// constant map — `1` is an int constant, so
/// `_validateMapSpread` answers `const_spread_expected_map`.
#[test]
fn const_spread_expected_map_fires_on_int_operand() {
    let fixture = "const e = <int, int>{...1};\n".to_string();
    let names = code_names(&report_ops(&fixture));
    assert!(
        names.contains(&"const_spread_expected_map"),
        "expected const_spread_expected_map, got {names:?}"
    );
}

/// The silent twin: `const e = <int, int>{...m}` where `m` is a const
/// map — the operand is a constant map, no row.
#[test]
fn const_spread_expected_map_silent_on_const_map() {
    let l1 = "const m = <int, int>{1: 2};\n";
    let l2 = "const e = <int, int>{...m};\n";
    let fixture = format!("{l1}{l2}");
    assert!(rows_for(&fixture, &["const_spread_expected_map"]).is_empty());
}

// ------------------------------------------------------------------
// expression_in_map
// ------------------------------------------------------------------

/// `var e = {1: 2, 3}`: a bare expression (no `:`) in a map-resolved
/// literal fires `expression_in_map` at the expression's span — the
/// oracle resolves `{1: 2, 3}` as a map (entries present).
#[test]
fn expression_in_map_fires_on_bare_expression() {
    let fixture = "var e = <int, int>{1: 2, 3};\n".to_string();
    let names = code_names(&report_ops(&fixture));
    assert!(
        names.contains(&"expression_in_map"),
        "expected expression_in_map, got {names:?}"
    );
}

/// The silent twin: `{1: 2}` alone is all entries — no row.
#[test]
fn expression_in_map_silent_on_pure_entries() {
    let fixture = "var e = <int, int>{1: 2};\n".to_string();
    assert!(rows_for(&fixture, &["expression_in_map"]).is_empty());
}

// ------------------------------------------------------------------
// set_element_type_not_assignable
// ------------------------------------------------------------------

/// `var s = <int>{...l}` where `l` is `List<String>`: the spread's
/// element type `String` is not assignable to the declared element type
/// `int` — upstream's `_verifySpreadForListOrSet` assignability arm
/// answers `set_element_type_not_assignable` at the operand.
#[test]
fn set_element_type_not_assignable_fires_on_mismatch() {
    let l1 = "var l = <String>['a'];\n";
    let l2 = "var s = <int>{...l};\n";
    let fixture = format!("{l1}{l2}");
    let names = code_names(&report_ops(&fixture));
    assert!(
        names.contains(&"set_element_type_not_assignable"),
        "expected set_element_type_not_assignable, got {names:?}"
    );
}

/// The silent twin: `var s = <int>{...l}` where `l` is `List<int>` —
/// the element type matches, no row.
#[test]
fn set_element_type_not_assignable_silent_on_match() {
    let l1 = "var l = <int>[1];\n";
    let l2 = "var s = <int>{...l};\n";
    let fixture = format!("{l1}{l2}");
    assert!(rows_for(&fixture, &["set_element_type_not_assignable"]).is_empty());
}

// ------------------------------------------------------------------
// map_key/value_type_not_assignable through a spread
// ------------------------------------------------------------------

/// `var b = <int, int>{...m}` where `m` is `Map<String, String>`: the
/// spread's key/value types are not assignable to the declared
/// `int`/`int` — the oracle answers both
/// `map_key_type_not_assignable` and `map_value_type_not_assignable` at
/// the operand.
#[test]
fn map_key_value_type_not_assignable_fire_through_spread() {
    let l1 = "var m = <String, String>{'a': 'b'};\n";
    let l2 = "var b = <int, int>{...m};\n";
    let fixture = format!("{l1}{l2}");
    let names = code_names(&report_ops(&fixture));
    assert!(
        names.contains(&"map_key_type_not_assignable"),
        "expected map_key_type_not_assignable, got {names:?}"
    );
    assert!(
        names.contains(&"map_value_type_not_assignable"),
        "expected map_value_type_not_assignable, got {names:?}"
    );
}

/// The silent twin: `var b = <int, int>{...m}` where `m` is
/// `Map<int, int>` — key and value both match.
#[test]
fn map_key_value_type_not_assignable_silent_on_match() {
    let l1 = "var m = <int, int>{1: 2};\n";
    let l2 = "var b = <int, int>{...m};\n";
    let fixture = format!("{l1}{l2}");
    assert!(
        rows_for(
            &fixture,
            &[
                "map_key_type_not_assignable",
                "map_value_type_not_assignable",
            ],
        )
        .is_empty()
    );
}
