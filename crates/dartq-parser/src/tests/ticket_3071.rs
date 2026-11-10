//! #3071 pins: landed-token self-restraint in statement recovery — the
//! for-header landing gate, the `catch (…)` junk relocation, and the
//! missing-`(` condition group.
//!
//! Row convention: `(code, start_offset, end_offset)` — 0-based byte
//! offsets into the source, exactly as the listener stores them.
//!
//! Root (upstream pin, Dart SDK 3.13.7
//! `pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart`):
//!
//! * `parseForLoopPartsRest` (:10329-10331): the landing report fires only
//!   when the rest parse did not land on the `(` group's closer, and it
//!   anchors on the landing token itself. When the junk slot stalls exactly
//!   on the scanner's synthetic closer, the gate is silent — there is no
//!   up-front `}` report anywhere in the official flow.
//! * `parseTryStatement` (:10753-10810): a junk trace slot reports
//!   `catchSyntax` on the junk, splices a synthetic name, and — the group
//!   closer being synthetic — relocates it via `moveSynthetic`
//!   (:10776/:10802) in front of the junk. The catch body then stalls on
//!   the junk (`expectedBody` on the junk) and the unmatched-group row
//!   anchors at the relocation point. The junk is never skipped.
//! * `ensureParenthesizedCondition` (:8258) shares
//!   `parseExpressionInParenthesisRest` (:8376) for the inserted group, so
//!   a missing `(` still runs the condition parse: the zero-width `)`
//!   stalls it, the missing-identifier row anchors on the junk (the
//!   analyzer renders a zero-width synthetic's anchor with the following
//!   real token's extent), and `ensureCloseParen` returns the synthetic
//!   `)` as a clean close.
//!
//! Oracle A/B (dart analyze 3.13.7 `--format=machine`, parser rows only):
//! `conformance/expected.tsv`
//! `parser/src/fasta/recovery/partial_code/for_statement/ForStatementTest/test_for_statement_equals_assert#0`
//! (no `unexpected_token` row),
//! `.../test_for_statement_equals_break#0` (`Unexpected text 'break'` on
//! the junk, none on the `}`), `.../while_statement/WhileStatementTest/
//! test_while_statement_keyword_assert#0` and
//! `.../test_while_statement_condition_assert#0`,
//! `.../try_statement/TryStatementTest/test_try_statement_catch_identifierComma_assert#0`.

use super::collector::{Collector, latest_features, scan_ok};

fn rows_with(source: &'static str) -> Vec<(String, u32, u32)> {
    let (table, text) = scan_ok(source);
    let mut collector = Collector::default();
    let mut parser = crate::Parser::new(text, table, latest_features(), &mut collector);
    parser.parse_unit();
    parser.listener.errors.clone()
}

/// `for (var i = assert (true); }` (oracle
/// `test_for_statement_equals_assert#0`): the parts parse stalls exactly on
/// the scanner's synthetic `)` (inserted at the `}`), so the landed-token
/// gate stays silent — exactly three parser rows, no `unexpected_token`.
#[test]
fn for_equals_assert_landing_on_synthetic_closer_is_silent() {
    let mut rows = rows_with("f() { for (var i = assert (true); }\n");
    rows.sort();
    let expected = vec![
        ("expected_token".to_string(), 32, 33), // the `;`
        ("expected_token".to_string(), 34, 35), // unmatched-group `)` at the `}`
        ("missing_identifier".to_string(), 34, 35),
    ];
    assert_eq!(rows, expected, "oracle rows only, got {rows:?}");
}

/// `for (var i = break; }` (oracle `test_for_statement_equals_break#0`): the
/// landing gate reports on the junk (`Unexpected text 'break'` at the
/// `break`) and no `unexpected_token` ever anchors on the `}` — the
/// up-front stall latch must not mint a second row.
#[test]
fn for_equals_break_reports_on_junk_not_on_curly() {
    let rows = rows_with("f() { for (var i = break; }\n");
    assert!(
        rows.contains(&("unexpected_token".to_string(), 19, 24)),
        "one unexpected_token on the junk `break`, got {rows:?}"
    );
    assert!(
        !rows
            .iter()
            .any(|(code, start, _)| code == "unexpected_token" && *start != 19),
        "no unexpected_token away from the junk anchor, got {rows:?}"
    );
}

/// `while assert (true); }` (oracle
/// `test_while_statement_keyword_assert#0`): the inserted empty group still
/// runs the condition parse — `expected_token '('` and
/// `missing_identifier`, both on the junk `assert`, which then
/// statement-parses as the body (no `)` row from the recovery).
#[test]
fn while_missing_paren_condition_parse_reports_on_junk() {
    let rows = rows_with("f() { while assert (true); }\n");
    let expected = vec![
        ("expected_token".to_string(), 12, 18), // `assert`
        ("missing_identifier".to_string(), 12, 18),
    ];
    assert_eq!(rows, expected, "oracle rows only, got {rows:?}");
}

/// `while x) {} }` (oracle
/// `test_while_statement_condition_assert#0` family): the inserted
/// zero-width `)` stalls the condition parse and the missing-identifier row
/// borrows the junk's extent (`x`, offset 12..13).
#[test]
fn while_missing_paren_missing_identifier_borrows_junk_extent() {
    let rows = rows_with("f() { while x) {} }\n");
    assert!(
        rows.contains(&("missing_identifier".to_string(), 12, 13)),
        "missing_identifier on the junk `x` at offset 12, got {rows:?}"
    );
}

/// `try {} catch (e,  assert (true); }` (oracle
/// `test_try_statement_catch_identifierComma_assert#0`): the junk trace slot
/// reports `catchSyntax` on the junk, the synthetic closer relocates in
/// front of the junk (unmatched-group row at the relocation point), and the
/// catch body stalls on the junk (`expectedBody` on the junk). No
/// `expected_identifier_but_got_keyword` (the re-parse never sees the
/// junk), no `catch_syntax_extra_parameters` (the synthetic name suppresses
/// it), and the junk is never skipped.
#[test]
fn catch_trace_junk_relocates_closer_and_stalls_body_on_junk() {
    let rows = rows_with("f() { try {} catch (e,  assert (true); }\n");
    assert!(
        rows.contains(&("catch_syntax".to_string(), 24, 30)),
        "catch_syntax on the junk `assert`, got {rows:?}"
    );
    assert!(
        !rows
            .iter()
            .any(|(code, _, _)| code == "expected_identifier_but_got_keyword"),
        "no keyword-name face from a re-parse over the junk, got {rows:?}"
    );
    assert!(
        !rows
            .iter()
            .any(|(code, _, _)| code == "catch_syntax_extra_parameters"),
        "no extra-parameters row behind a synthetic name, got {rows:?}"
    );
    assert!(
        rows.contains(&("expected_body".to_string(), 24, 30)),
        "expected_body on the junk `assert`, got {rows:?}"
    );
    assert!(
        rows.contains(&("expected_token".to_string(), 24, 25)),
        "the unmatched-group row anchors at the relocation point, got {rows:?}"
    );
}
