//! #3363 switch-expression case-separator recovery anchor pins.
//!
//! When a case body is followed by neither `,` nor `}` and no `,`/`;` exists
//! before the switch expression's closing brace, the official recovery
//! reports `expected_token '}'` on the blocker — the token right after the
//! case expression — and only then jumps to the closer
//! (`_fe_analyzer_shared/parser/parser_impl.dart:12484-12489`:
//! `reportRecoverableError(next, diag.expectedButGot('}'))` then
//! `next = closingBracket`). The row therefore never rides the closer.
//!
//! Frozen oracle faces (`parser-fe/patterns/switchExpression_recovery_*`),
//! all reporting the `}` row on the stray `:` of `int() => 0 : 1`:
//!
//! ```text
//! switchExpression_recovery_bogusTokensAfterCase            2:14 len 1
//! switchExpression_recovery_bogusTokensAfterCase_laterComma 2:14 len 1
//! switchExpression_recovery_bogusTokensAfterCase_nestedComma 2:14 len 1
//! ```
//!
//! A well-formed case list keeps silent, and the `,`-found recovery (a
//! separator exists before the closer) is a different face that still
//! reports on the blocker with the `,` template — pinned here only as a
//! no-`}`-row regression guard.

use super::collector::{Collector, latest_features, scan_ok};
use crate::parser::Parser;

const CLOSE_BRACE: &str = "Expected to find '}'.";

/// Parses `source` and returns every recoverable row as
/// `(code, start_utf16, end_utf16, text)`.
fn rows(source: &'static str) -> Vec<(String, u32, u32, String)> {
    let (table, text) = scan_ok(source);
    let mut parser = Parser::new(
        text,
        table,
        latest_features(),
        Box::leak(Box::new(Collector::default())),
    );
    parser.parse_unit();
    parser
        .listener
        .errors
        .iter()
        .zip(parser.listener.texts.iter())
        .map(|((code, start, end), text)| (code.clone(), *start, *end, text.clone()))
        .collect()
}

/// Only the case-separator `}` rows — the face this ticket pins.
fn close_brace_rows(source: &'static str) -> Vec<(String, u32, u32, String)> {
    rows(source)
        .into_iter()
        .filter(|r| r.0 == "expected_token" && r.3 == CLOSE_BRACE)
        .collect()
}

/// Offset of the first byte of `needle` in `source` (UTF-16 == byte offset
/// here: every pinned face is ASCII).
fn at(source: &str, needle: &str) -> u32 {
    u32::try_from(source.find(needle).expect("needle in source")).unwrap()
}

#[test]
fn bogus_tokens_after_case_anchor_on_the_blocker() {
    // `int() => 0 : 1` inside a switch expression: the stray `:` is the
    // blocker, the `}` on the next line is the closer the skip lands on.
    let source = "f(x) => switch(x) {\n  int() => 0 : 1\n};";
    let blocker = at(source, ": 1");
    assert_eq!(
        close_brace_rows(source),
        vec![(
            "expected_token".to_string(),
            blocker,
            blocker + 1,
            CLOSE_BRACE.to_string()
        )],
        "the `}}` row must ride the stray `:`, not the closer"
    );
}

#[test]
fn later_comma_still_anchors_on_the_blocker() {
    // The `,` after the switch expression's `}` is outside the group, so the
    // in-group search finds no separator and the closer-jump face applies.
    let source = "f(x) => [switch(x) {\n  int() => 0 : 1\n}, 0];";
    let blocker = at(source, ": 1");
    assert_eq!(
        close_brace_rows(source),
        vec![(
            "expected_token".to_string(),
            blocker,
            blocker + 1,
            CLOSE_BRACE.to_string()
        )],
        "an out-of-group comma must not move the `}}` row off the blocker"
    );
}

#[test]
fn nested_comma_inside_case_body_anchors_on_the_blocker() {
    // The record literal's inner comma is skipped by
    // `findNextCommaOrSemicolon` (group jump), so the closer-jump face
    // applies and the row stays on the blocker.
    let source = "f(x) => switch(x) {\n  int() => 0 : (1, 2)\n};";
    let blocker = at(source, ": (");
    assert_eq!(
        close_brace_rows(source),
        vec![(
            "expected_token".to_string(),
            blocker,
            blocker + 1,
            CLOSE_BRACE.to_string()
        )],
        "a comma nested inside the case body must not swallow the `}}` row"
    );
}

#[test]
fn well_formed_case_list_is_silent() {
    let source = "f(x) => switch(x) {\n  0 => 1,\n  _ => 2\n};";
    assert_eq!(
        close_brace_rows(source),
        Vec::new(),
        "a well-formed case list reports no `}}` row"
    );
}
