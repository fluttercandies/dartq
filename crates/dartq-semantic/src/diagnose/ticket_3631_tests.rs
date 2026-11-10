//! The #3631 pins: the exhaustiveness space model's list-index columns
//! (`HeadKey`/`RestKey`/`TailKey` on `SingleSpace.additionalProperties`,
//! upstream key.dart/`createListSpace` shared.dart:832-911). A list
//! pattern now contributes its head/rest/tail split instead of the whole
//! unknown space, so a `List<E>` scrutinee's cases cover exactly the
//! sizes their patterns spell.
//!
//! Oracle rows: dart 3.13.7 (the pinned conformance oracle) over the
//! census-11 cases — the shapes mirror
//! `LanguageFeatures/Patterns/Exhaustiveness/exhaustiveness_list_A0*_*`
//! (`switch (list) { case []: …, case [_, ...]: }` covers; a gap reports
//! the first unmatched size's witness `'[]'`/`'[_]'`).
//! Official sources: `_fe_analyzer_shared` exhaustiveness.dart:193-198
//! (`keysOfInterest` collected off `additionalProperties`), types/list.dart
//! (`ListTypeRestriction`, `ListTypeStaticType.getSubtypes`),
//! shared.dart:396-416 (`getAdditionalFieldType`) and :832-911
//! (`createListSpace`).

use super::test_rig::report_ops;

/// The `code` rows of `report` as message texts, in source order.
fn rows_of(source: &str, code: &str) -> Vec<String> {
    report_ops(source)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.message.to_string())
        .collect()
}

// —— non_exhaustive_switch_expression ———————————————————————————————————

/// `[]` alone spells no head/tail key, so `ListTypeStaticType.getSubtypes`
/// splits `List<int>` into the single open `[...]` subtype (`maxSize` 0 —
/// upstream Dart SDK source, pkg/_fe_analyzer_shared/lib/src/exhaustiveness/
/// types/list.dart:65-121). `[]` does not cover it, and the witness prints
/// that subtype's own spelling (`witnessToDart`, same file).
#[test]
fn empty_list_case_witnesses_the_open_rest() {
    let rows = rows_of(
        "int f(List<int> l) => switch (l) { [] => 0 };\n",
        "non_exhaustive_switch_expression",
    );
    assert_eq!(
        rows,
        [
            "The type 'List<int>' isn't exhaustively matched by the switch cases since it doesn't match the pattern '[...]'."
        ]
    );
}

/// `[]` + `[_, ...]` covers every size: the oracle stays silent
/// (exhaustiveness_list_A03_t01's exhaustive arm).
#[test]
fn empty_and_rest_cases_cover_every_size() {
    let rows = rows_of(
        "int f(List<int> l) => switch (l) { [] => 0, [_, ...] => 1 };\n",
        "non_exhaustive_switch_expression",
    );
    assert!(rows.is_empty(), "{rows:?}");
}

/// `[_]` + `[_, ...]` covers every non-empty size: the gap is `[]`, the
/// witness its spelling (the A03 family gap at size 0).
#[test]
fn covered_sizes_leave_the_empty_witness() {
    let rows = rows_of(
        "int f(List<int> l) => switch (l) { [_] => 0, [_, ...] => 1 };\n",
        "non_exhaustive_switch_expression",
    );
    assert_eq!(
        rows,
        [
            "The type 'List<int>' isn't exhaustively matched by the switch cases since it doesn't match the pattern '[]'."
        ]
    );
}

/// A bare `...` rest marks every size: `[...]` alone covers the whole
/// `List<int>` space (the `hasRest && size == 0` unrestricted arm,
/// types/list.dart:279-282).
#[test]
fn bare_rest_alone_covers() {
    let rows = rows_of(
        "int f(List<int> l) => switch (l) { [...] => 0 };\n",
        "non_exhaustive_switch_expression",
    );
    assert!(rows.is_empty(), "{rows:?}");
}

// —— non_exhaustive_switch_statement ————————————————————————————————————

/// A statement switch only demands exhaustiveness over an always-exhaustive
/// scrutinee type (`mustBeExhaustive: isAlwaysExhaustive(...)`, upstream
/// Dart SDK source, pkg/analyzer/lib/src/dart/constant/constant_verifier.dart
/// :498) — `bool`, `Null`, enums, sealed classes, and records/`FutureOr`
/// of those (pkg/analyzer/lib/src/dart/element/type_system.dart:815-856).
/// `List` is none of them: the `[]` gap stays unreported.
#[test]
fn statement_switch_over_a_list_is_not_always_exhaustive() {
    let rows = rows_of(
        "void f(List<int> l) { switch (l) { case [_]: case [_, ...]: } }\n",
        "non_exhaustive_switch_statement",
    );
    assert!(rows.is_empty(), "{rows:?}");
}

// —— unreachable_switch_case ————————————————————————————————————————————

/// A second `[]` case is unreachable — the first already covers the
/// empty-list space (the case space overlaps whole).
#[test]
fn duplicate_empty_case_is_unreachable() {
    let rows = rows_of(
        "int f(List<int> l) => switch (l) { [] => 0, [] => 1, _ => 2 };\n",
        "unreachable_switch_case",
    );
    assert_eq!(rows.len(), 1, "{rows:?}");
}

/// `[_, ...]` after `[...]` is unreachable — the unrestricted rest
/// already covers every size the narrower rest spells.
#[test]
fn narrower_rest_after_unrestricted_rest_is_unreachable() {
    let rows = rows_of(
        "int f(List<int> l) => switch (l) { [...] => 0, [_, ...] => 1 };\n",
        "unreachable_switch_case",
    );
    assert_eq!(rows.len(), 1, "{rows:?}");
}

/// `[_]` after `[_, ...]` is unreachable: the size-1 space is inside the
/// rest's `size >= 1` cover (the `other.hasRest → size >= other.size`
/// subtype arm, types/list.dart:286-289).
#[test]
fn singleton_after_rest_is_unreachable() {
    let rows = rows_of(
        "int f(List<int> l) => switch (l) { [_, ...] => 0, [_] => 1 };\n",
        "unreachable_switch_case",
    );
    assert_eq!(rows.len(), 1, "{rows:?}");
}

/// `[]` after `[_, ...]` stays live — the empty size is the one size the
/// rest does not cover.
#[test]
fn empty_case_after_rest_stays_live() {
    let rows = rows_of(
        "int f(List<int> l) => switch (l) { [_, ...] => 0, [] => 1 };\n",
        "unreachable_switch_case",
    );
    assert!(rows.is_empty(), "{rows:?}");
}

// —— dead_code ——————————————————————————————————————————————————————————

/// A covered case is the exhaustiveness walk's `unreachable_switch_case`
/// alone: `dead_code` follows flow-analysis reachability, and the first
/// `[]` is refutable, so the second body stays reachable. The corpus shows
/// the split — dead code only after an irrefutable head
/// (language/patterns/switch_trivial_exhaustiveness_error_test, after
/// `case _:`; language/patterns/flow_analysis/list_pattern_error_test,
/// after `case [...]:`).
#[test]
fn duplicate_empty_case_body_stays_live() {
    let source =
        "void f(List<int> l) { switch (l) { case []: return; case []: print(1); default: } }\n";
    assert_eq!(rows_of(source, "unreachable_switch_case").len(), 1);
    let rows = rows_of(source, "dead_code");
    assert!(rows.is_empty(), "{rows:?}");
}
