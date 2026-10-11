//! Pattern grammar tests.

use super::collector::setup;
use crate::listener::PatternContext;

#[test]
fn constant_pattern_int() {
    let mut p = setup("1");
    let end = p.parse_pattern_entry(PatternContext::Matching);
    assert_eq!(p.cursor.lexeme(end.id), "1");
    p.listener
        .assert_sequence(&[("begin", "pattern"), ("end", "pattern")]);
}

#[test]
fn wildcard_pattern() {
    let mut p = setup("_");
    let _ = p.parse_pattern_entry(PatternContext::Matching);
    p.listener
        .assert_sequence(&[("handle", "wildcard_pattern")]);
}

#[test]
fn declared_variable_pattern() {
    let mut p = setup("var x");
    let _ = p.parse_pattern_entry(PatternContext::Declaration);
    p.listener
        .assert_sequence(&[("handle", "declared_variable_pattern")]);
    assert!(
        p.listener
            .error_codes()
            .contains(&"variable_pattern_keyword_in_declaration_context".to_string())
    );
}

#[test]
fn list_pattern_with_rest() {
    let mut p = setup("[1, ...rest]");
    let _ = p.parse_pattern_entry(PatternContext::Matching);
    p.listener.assert_sequence(&[("handle", "list_pattern")]);
    assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
}

#[test]
fn map_pattern_entry() {
    let mut p = setup("{'k': _}");
    let _ = p.parse_pattern_entry(PatternContext::Matching);
    p.listener.assert_sequence(&[("handle", "map_pattern")]);
    assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
}

#[test]
fn record_pattern_requires_trailing_comma_for_single_field() {
    let mut p = setup("(1)");
    let _ = p.parse_pattern_entry(PatternContext::Matching);
    // One positional field without a trailing comma is a parenthesized
    // pattern, not a record.
    p.listener
        .assert_sequence(&[("handle", "parenthesized_pattern")]);
    assert!(p.listener.errors.is_empty());
}

#[test]
fn cast_and_null_check_patterns() {
    let mut p = setup("x as int");
    let _ = p.parse_pattern_entry(PatternContext::Matching);
    p.listener.assert_sequence(&[("handle", "cast_pattern")]);

    let mut p = setup("x?");
    let _ = p.parse_pattern_entry(PatternContext::Matching);
    p.listener
        .assert_sequence(&[("handle", "null_check_pattern")]);
}

#[test]
fn logical_or_pattern() {
    let mut p = setup("1 || 2");
    let _ = p.parse_pattern_entry(PatternContext::Matching);
    p.listener.assert_sequence(&[
        ("begin", "pattern"),
        ("begin", "binary_pattern"),
        ("end", "binary_pattern"),
        ("end", "pattern"),
    ]);
}

#[test]
fn relational_pattern() {
    let mut p = setup("< 3");
    let _ = p.parse_pattern_entry(PatternContext::Matching);
    p.listener
        .assert_sequence(&[("handle", "relational_pattern")]);
}

#[test]
fn object_pattern_fields() {
    let mut p = setup("Rect(width: 2, height: x)");
    let _ = p.parse_pattern_entry(PatternContext::Matching);
    p.listener.assert_sequence(&[("handle", "object_pattern")]);
    assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
}

#[test]
fn constant_pattern_binary_over_collection() {
    // `1 + 2` over-collects then reports like the oracle.
    let mut p = setup("1 + 2");
    let _ = p.parse_pattern_entry(PatternContext::Matching);
    assert!(
        p.listener
            .error_codes()
            .contains(&"invalid_constant_pattern_binary".to_string())
    );
}

#[test]
fn patterns_disabled_reports_experiment() {
    let (table, text) = super::collector::scan_ok("1");
    let features = crate::ParserFeatures {
        patterns_enabled: false,
        variance_enabled: false,
        primary_constructors_enabled: true,
        augmentations_enabled: false,
        enhanced_parts_enabled: false,
    };
    let mut p = crate::Parser::new(
        text,
        table,
        features,
        super::collector::Collector::default(),
    );
    let end = p.parse_pattern_entry(PatternContext::Matching);
    let _ = end;
    assert_eq!(
        p.listener.error_codes(),
        vec!["experiment_not_enabled".to_string()]
    );
}

// —— keyword blocker inside delimited patterns (#461) ——————————————————————
//
// A reserved word (`if`) cannot start a pattern operand. The official
// recovery reports `missingIdentifier` on it and leaves it unconsumed; the
// enclosing delimited loop then either skips it (list, whose official loop
// has the per-iteration no-progress skip) or reports the missing delimiter
// at it (parenthesized/record/map). Oracle:
// `pkg/front_end/parser_testcases/patterns/syntheticIdentifier_*.dart`.

/// Sorted code multiset of one pattern-entry parse.
fn sorted_pattern_codes(source: &'static str) -> Vec<String> {
    let mut p = setup(source);
    let _ = p.parse_pattern_entry(PatternContext::Matching);
    let mut codes = p.listener.error_codes();
    codes.sort();
    codes
}

#[test]
fn keyword_blocker_inside_list_pattern_skips() {
    // The list loop's no-progress skip consumes the blocker, so `]` closes
    // cleanly: exactly one missing_identifier (official
    // syntheticIdentifier_insideListPattern: one problem).
    assert_eq!(
        sorted_pattern_codes("[if]"),
        ["missing_identifier"],
        "list blocker"
    );
}

#[test]
fn keyword_blocker_inside_parenthesized_pattern_reports_closer() {
    // The parenthesized loop has no no-progress skip (official
    // parseParenthesizedPatternOrRecordPattern :12229-12231): the blocker
    // survives to ensureCloseParen, which reports `expected ')'` at it and
    // jumps to the group's real closer (official
    // syntheticIdentifier_insideParenthesizedPattern: two problems).
    let mut p = setup("(if)");
    let _ = p.parse_pattern_entry(PatternContext::Matching);
    p.listener
        .assert_sequence(&[("handle", "parenthesized_pattern")]);
    assert_eq!(
        sorted_pattern_codes("(if)"),
        ["expected_token", "missing_identifier"],
        "parenthesized blocker"
    );
}

#[test]
fn keyword_blocker_inside_record_pattern_reports_closer() {
    // Same shape as the parenthesized blocker, as the second record field
    // (official syntheticIdentifier_insideRecordPattern: missing_identifier
    // + `expected ')'` before it, record with 2 fields).
    let mut p = setup("(_, if)");
    let _ = p.parse_pattern_entry(PatternContext::Matching);
    p.listener.assert_sequence(&[("handle", "record_pattern")]);
    assert_eq!(
        sorted_pattern_codes("(_, if)"),
        ["expected_token", "missing_identifier"],
        "record blocker"
    );
}

#[test]
fn keyword_blocker_inside_map_pattern_spawns_entry_recovery() {
    // The map loop's stale-`next` skip leaves the FIRST entry's blocker for
    // the `,` recovery: the blocker is re-parsed as the next key (`:` then
    // fails), skipped as the second value, and `}` closes. Official
    // syntheticIdentifier_insideMapPattern: 3 x missing_identifier +
    // `expected ','` + `expected ':'`.
    let mut p = setup("{0: if}");
    let _ = p.parse_pattern_entry(PatternContext::Matching);
    p.listener.assert_sequence(&[("handle", "map_pattern")]);
    assert_eq!(
        sorted_pattern_codes("{0: if}"),
        [
            "expected_token",
            "expected_token",
            "missing_identifier",
            "missing_identifier",
            "missing_identifier"
        ],
        "map blocker"
    );
}
