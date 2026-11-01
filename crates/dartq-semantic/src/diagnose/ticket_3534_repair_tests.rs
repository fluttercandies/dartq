//! #3534 repair pins — the corpus shapes the spread family fabricated
//! diagnostics on, where the oracle abstains.
//!
//! # The defect this guards
//!
//! The first cut of the spread verifiers answered on every operand the
//! row model could not type and on every literal the resolution left
//! ambiguous — the census's extras (329 `not_iterable_spread`, 265
//! `not_map_spread`, the const-shape pair, `expression_in_map`) were all
//! fabricated rows on undecidable shapes. The oracle's contract is the
//! abstain direction:
//!
//! - an operand whose static type is `dynamic`/`Never`/the error-dynamic
//!   shape (an unresolved name, a callee-side type parameter the
//!   inference fills from context, a head the declaration model cannot
//!   resolve) folds the whole iterability ladder silently;
//! - an ambiguous `{...}` literal resolves `dynamic` — `isMap`/`isSet`
//!   are both false upstream, so `visitSetOrMapLiteral` runs NEITHER
//!   element verifier (no `not_map_spread`, no `expression_in_map`, no
//!   const shape ask);
//! - `asInstanceOf` ignores the receiver's `?`: a `List<int>?` operand
//!   still answers `Iterable<int>` and stays silent;
//! - a `...?` spread over a `Null`-typed operand admits `null` silently
//!   on BOTH the const and the non-const faces.
//!
//! Oracle probes: `dart analyze` 3.13.7 / the extracted conformance
//! cases (`test_visitSetOrMapLiteral_ambiguous*`,
//! `SpreadElementTest::test_*_downward`, `*_spread_null`).

use super::test_rig::{code_names, report_ops};

/// The `(code, offset, len)` rows of `fixture`'s diagnostics, filtered
/// to `wanted` codes.
fn rows_for(fixture: &str, wanted: &[&str]) -> Vec<(String, u32, u32)> {
    report_ops(fixture)
        .diagnostics
        .iter()
        .filter(|d| wanted.contains(&d.code.name))
        .map(|d| (d.code.name.to_string(), d.range.offset, d.range.length))
        .collect()
}

// ------------------------------------------------------------------
// The undecidable operand abstains
// ------------------------------------------------------------------

/// An unresolved spread operand: `x` is not declared, so the oracle's
/// `typeOrThrow` reads `dynamic` and the iterability ladder folds —
/// only the `undefined_identifier` row on `x` itself is owed. The
/// pre-fix ladder fired `not_iterable_spread` on the same span.
#[test]
fn unresolved_operand_reports_only_its_own_row() {
    let fixture = "void f() { var a = [...x]; }\n";
    let names = code_names(&report_ops(fixture));
    assert!(
        names.contains(&"undefined_identifier"),
        "expected undefined_identifier on x, got {names:?}"
    );
    assert!(
        !names.contains(&"not_iterable_spread")
            && !names.contains(&"not_map_spread"),
        "the undecidable operand must abstain, got {names:?}"
    );
}

/// A generic call result whose `T` the literal context instantiates
/// upstream (`<int>[...a()]` infers `Iterable<int>`): the row model
/// keeps the callee's parameter, an undecidable head — the oracle
/// resolves it through inference and stays silent.
#[test]
fn generic_call_result_operand_abstains() {
    for src in [
        "T a<T>() => throw '';\nvoid f() { var l = <int>[...a()]; }\n",
        "T a<T>() => throw '';\nvoid f() { var s = <int>{...a()}; }\n",
        "T a<T>() => throw '';\nvoid f() { var m = <String, int>{...a()}; }\n",
    ] {
        let names = code_names(&report_ops(src));
        assert!(
            !names.contains(&"not_iterable_spread")
                && !names.contains(&"not_map_spread"),
            "{src:?}: the callee-side parameter is undecidable, got {names:?}"
        );
    }
}

/// A scope type parameter bounded by a collection reads its bound —
/// `T extends List<int>` answers `Iterable<int>` through the bound's
/// interface and stays silent (upstream `asInstanceOf`'s bound
/// resolution).
#[test]
fn bounded_type_parameter_operand_silent() {
    let fixture =
        "void f<T extends List<int>>(T t) { var a = [...t]; }\n";
    let names = code_names(&report_ops(fixture));
    assert!(
        !names.contains(&"not_iterable_spread"),
        "T extends List<int> reaches Iterable through the bound: {names:?}"
    );
}

/// A nullable operand still reaches `Iterable` — `asInstanceOf` ignores
/// the receiver's `?`, so `List<int>?` answers `Iterable<int>` and both
/// `...` and `...?` stay silent. The pre-fix `nullable` arm fired on
/// every `T?` operand.
#[test]
fn nullable_iterable_operand_stays_silent() {
    for src in [
        "void f(List<int>? l) { var a = [...l]; }\n",
        "void f(List<int>? l) { var a = [...?l]; }\n",
        "void f(List<int>? l) { var a = <int>{...l}; }\n",
    ] {
        let names = code_names(&report_ops(src));
        assert!(
            !names.contains(&"not_iterable_spread"),
            "{src:?}: `asInstanceOf` strips `?` — List<int>? reaches Iterable: {names:?}"
        );
    }
}

// ------------------------------------------------------------------
// The ambiguous literal runs no element verifier
// ------------------------------------------------------------------

/// `{...l, 1: 2}` — a set-side spread plus a map entry: the literal is
/// `ambiguous_set_or_map_literal_both` upstream and its static type is
/// `dynamic`, so the oracle runs NEITHER element verifier. Only the
/// `_both` row is owed.
#[test]
fn ambiguous_both_literal_fires_no_spread_code() {
    let fixture = "void f() { var l = <int>[1]; var a = {...l, 1: 2}; }\n";
    let names = code_names(&report_ops(fixture));
    assert!(
        names.contains(&"ambiguous_set_or_map_literal_both"),
        "expected _both, got {names:?}"
    );
    assert!(
        !names.contains(&"not_map_spread")
            && !names.contains(&"not_iterable_spread")
            && !names.contains(&"expression_in_map"),
        "ambiguous literal runs no element verifier: {names:?}"
    );
}

/// `{...m, 2}` — a map-side spread plus an expression: the same `_both`
/// ambiguity; before the can-be/must-be fold mirrored upstream, the
/// expression leaf left `canBeMap` intact and the literal resolved a
/// map — fabricating `not_map_spread` on `...m`'s Map operand AND
/// `expression_in_map` on `2`.
#[test]
fn expression_beside_map_spread_is_ambiguous_not_a_map() {
    let fixture = "void f() { var m = <int, int>{1: 2}; var a = {...m, 2}; }\n";
    let names = code_names(&report_ops(fixture));
    assert!(
        names.contains(&"ambiguous_set_or_map_literal_both"),
        "expected _both, got {names:?}"
    );
    assert!(
        !names.contains(&"not_map_spread") && !names.contains(&"expression_in_map"),
        "ambiguous literal runs no element verifier: {names:?}"
    );
}

/// `{...d}` on `dynamic` alone: `dynamic` fills every can-be slot and
/// forces neither must-be — the literal is `_either`, and the dynamic
/// operand itself is silent in the iterability ladder.
#[test]
fn dynamic_spread_literal_is_either_and_silent() {
    let fixture = "Object f(dynamic d) { var e = {...d}; return e; }\n";
    let names = code_names(&report_ops(fixture));
    assert_eq!(
        names,
        ["ambiguous_set_or_map_literal_either"],
        "{names:?}"
    );
}

/// A spread-decided const literal asks the const shape of the right
/// side: `const Set<int> x = {...l}` resolves a set through its
/// context — a `List` operand is a valid list/set spread, not a
/// `const_spread_expected_map` row (the pre-fix parser-flag arm fired
/// the map code on every spread-decided set).
#[test]
fn const_spread_decided_set_asks_the_list_or_set_shape() {
    let fixture = "const l = [1];\nconst Set<int> x = {...l};\n";
    let names = code_names(&report_ops(fixture));
    assert!(
        !names.contains(&"const_spread_expected_map")
            && !names.contains(&"const_spread_expected_list_or_set"),
        "the const shape ask follows the resolved kind: {names:?}"
    );
    // A `Map` operand inside `const Set<int> {...m}` is itself a decided
    // map-side leaf — the literal resolves MAP upstream (the element
    // info's can-be/must-be fold precedes the context fallback), so the
    // map shape ask finds a `Map` operand and stays silent; the `Set<int>`
    // assignability mismatch is the declaration's own face, not the
    // spread's.
    let map_operand = "const m = {1: 2};\nconst Set<int> y = {...m};\n";
    let names = code_names(&report_ops(map_operand));
    assert!(
        !names.contains(&"const_spread_expected_list_or_set"),
        "a decided map-side leaf resolves the literal as a map: {names:?}"
    );
    // The wrong shape DOES fire: `const Set<int> y = {...i}` where `i`
    // is `const i = 1` — `...i` is an `int` operand, so the literal
    // defers to its `Set<int>` context, resolves a set, and the operand
    // is no list/set — `const_spread_expected_list_or_set` beside the
    // non-const `not_iterable_spread` (upstream runs both faces).
    let bad = "const i = 1;\nconst Set<int> y = {...i};\n";
    let names = code_names(&report_ops(bad));
    assert!(
        names.contains(&"const_spread_expected_list_or_set"),
        "expected const_spread_expected_list_or_set on the int operand, got {names:?}"
    );
}

/// `...?` over a `Null` constant admits `null` silently — upstream's
/// `spread.isNull && element.isNullAware` `continue` on BOTH the const
/// and the non-const faces.
#[test]
fn null_aware_spread_over_null_constant_stays_silent() {
    for src in [
        "const a = null;\nconst Map<String, int> x = {'k': 0, ...?a};\n",
        "const a = null;\nconst Set<String> x = {'k', ...?a};\n",
        "const a = null;\nconst l = [1, ...?a];\n",
    ] {
        let names = code_names(&report_ops(src));
        assert!(
            !names.contains(&"const_spread_expected_map")
                && !names.contains(&"const_spread_expected_list_or_set")
                && !names.contains(&"not_null_aware_null_spread")
                && !names.contains(&"not_map_spread")
                && !names.contains(&"not_iterable_spread"),
            "{src:?}: `...?` over null is silent on every spread face: {names:?}"
        );
    }
}

/// The `...` counterpart: a `Null` operand under a non-aware spread is
/// `not_null_aware_null_spread`, and in a const literal the same
/// operand ALSO owes its const-shape row — upstream's
/// `toListValue() == null` path reports `const_spread_expected_map` on
/// `...a` where `a` is `null`.
#[test]
fn non_aware_spread_over_null_reports_both_faces() {
    let fixture = "const a = null;\nconst Map<String, int> x = {'k': 0, ...a};\n";
    let names = code_names(&report_ops(fixture));
    assert!(
        names.contains(&"not_null_aware_null_spread"),
        "expected not_null_aware_null_spread, got {names:?}"
    );
    assert!(
        names.contains(&"const_spread_expected_map"),
        "expected const_spread_expected_map, got {names:?}"
    );
}

/// A typed list's element ask reads the spread's ELEMENT contribution,
/// never the operand whole (type_analyzer.dart's spread-element
/// resolution): a `Null`/non-iterable/undecidable operand is the
/// iterability verifier's row alone, and a raw `Iterable` donates
/// `dynamic` — all silent under `list_element_type_not_assignable`.
/// The census extra came from falling back to the operand's own type
/// when the element face had no contribution.
#[test]
fn list_element_ask_abstains_on_non_iterable_spread_operands() {
    for (src, owed) in [
        // `...?` over a `Null` operand admits null silently — no
        // iterability row, no element row.
        ("void f(Null a) { var l = <int>[0, ...?a]; }\n", &[][..]),
        // `...` over `Null` owes `not_null_aware_null_spread` ONLY.
        (
            "void f(Null a) { var l = <int>[0, ...a]; }\n",
            &["not_null_aware_null_spread"][..],
        ),
        // A non-iterable operand owes `not_iterable_spread` ONLY — the
        // element ask never fires on the operand's own type.
        (
            "void f(int i) { var l = <String>['x', ...i]; }\n",
            &["not_iterable_spread"][..],
        ),
        // A raw `Iterable` donates a `dynamic` element — assignable,
        // silent.
        ("void f(Iterable it) { var l = <int>[0, ...it]; }\n", &[][..]),
    ] {
        let names = code_names(&report_ops(src));
        assert!(
            !names.contains(&"list_element_type_not_assignable"),
            "{src:?}: the element ask reads the spread's element face, got {names:?}"
        );
        for code in owed {
            assert!(
                names.contains(code),
                "{src:?}: expected {code}, got {names:?}"
            );
        }
    }
    // The genuine case still fires: a `List<String>` spread into
    // `<int>[...]` contributes `String` — not assignable to `int`.
    let bad = "void f(List<String> s) { var l = <int>[0, ...s]; }\n";
    let names = code_names(&report_ops(bad));
    assert!(
        names.contains(&"list_element_type_not_assignable"),
        "the real element mismatch still fires: {names:?}"
    );
}
