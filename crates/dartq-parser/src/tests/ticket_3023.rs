//! #3023 pins: the object-pattern field loop's empty-slot face —
//! `expected_token`/`missing_identifier` emitter rows in literal and
//! pattern contexts.
//!
//! Row convention: `(code, start_offset, end_offset)` — 0-based byte
//! offsets into the source, exactly as the listener stores them.
//!
//! Root (upstream pin, Dart SDK 3.13.7
//! `pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart`):
//!
//! * `parseObjectPatternRest` (:12249-12250) calls `parsePattern` and then
//!   reads `next = token.next!` with NO no-progress skip — unlike
//!   `parseListPatternSuffix` (:12002) and `parseMapPatternSuffix`
//!   (:12088), which both carry `if (identical(next, token.next))`
//!   skips. An object-pattern field whose pattern slot consumes nothing
//!   (the empty slot of `Foo(:)` or `Foo(a: , b: 1)`) therefore leaves the
//!   blocking `)`/`,` in place: it closes the field loop (:12253-12257) or
//!   separates the next field (:12258-12266) through the ordinary arms.
//!   The only row is the `missing_identifier` the empty slot's expression
//!   descent reports on the blocker.
//! * Skipping the blocker (the pre-fix shape) strands the group in
//!   `ensureCloseParen`, minting a spurious `expectedButGot(')')` at the
//!   case's `:` (and, in a `switch` case, derailing the case-colon
//!   machinery toward EOF), or a spurious `expectedButGot(',')` at the
//!   next field name.
//!
//! The list/map-pattern and record-pattern loops keep their official
//! skips/absences (guards below): `[a, , b]` and `[1, , 2]` in a case
//! report the hole's `missing_identifier` PLUS the separator
//! `expectedButGot(',')` at the following element (the official
//! `identical` skip makes the blocking comma unavailable as a separator,
//! :12002-12007), and map-pattern holes report one `missing_identifier`
//! per empty value slot.
//!
//! Oracle A/B (`dart analyze --format=machine`, parser-tier rows):
//! released-oracle probe battery over `[1,,2]` / `{1: , 2:3}` / `{(,)}` /
//! `var [a, , b] = x;` / `var {'k': } = m;` / `case [1, , 2]:` /
//! `var (a, ) = r;` / `(1, , 2)` / `{1, , 2}` / `[,]` / `{,}` / `{1:, }` /
//! `{1: 2, , 3: 4}` / `(a:, 1)` / `Foo(:)` / `Foo(a: )` / `Foo(: 1, :)` /
//! `Foo(a: , b: 1)` / `Foo(a: , : 1)` / `Foo(a: ): }` / `var (, ) = r;` /
//! `var (a, , ) = r;` / `{: 1}` / `(a 1)` / `(a: 1 b: 2)` / `{1: 2 3: 4}` /
//! `[1,]` / `const {1, , 2}` / `case (,):` — every literal-side and
//! record/list/map-pattern face already matched the oracle; the
//! object-pattern field loop was the one divergent face (fixed here).

use super::collector::{Collector, latest_features, scan_ok};

fn rows_with(source: &'static str) -> Vec<(String, u32, u32)> {
    let (table, text) = scan_ok(source);
    let mut collector = Collector::default();
    let mut parser = crate::Parser::new(text, table, latest_features(), &mut collector);
    parser.parse_unit();
    parser.listener.errors.clone()
}

/// `f(x) { switch (x) { case Foo(:): break; } }` — the empty bare-colon
/// field's blocker is the `)`; it closes the object pattern cleanly
/// (official :12253-12257) and the case's `:` follows untouched.
#[test]
fn object_pattern_bare_colon_hole_closes_on_paren() {
    let mut rows = rows_with("f(x) { switch (x) { case Foo(:): break; } }\n");
    rows.sort();
    let expected = vec![("missing_identifier".to_string(), 30, 31)];
    assert_eq!(rows, expected, "oracle rows only, got {rows:?}");
}

/// `f(x) { switch (x) { case Foo(a: ): break; } }` — the named field's
/// empty slot blocks on the `)` and closes cleanly; no
/// `expectedButGot(')')` at the case's `:`, no EOF cascade.
#[test]
fn object_pattern_named_field_hole_closes_on_paren() {
    let mut rows = rows_with("f(x) { switch (x) { case Foo(a: ): break; } }\n");
    rows.sort();
    let expected = vec![("missing_identifier".to_string(), 32, 33)];
    assert_eq!(rows, expected, "oracle rows only, got {rows:?}");
}

/// `f(x) { switch (x) { case Foo(: 1, :): break; } }` — a real first field
/// then an empty bare-colon field; the trailing `)` still closes the loop.
#[test]
fn object_pattern_second_bare_colon_hole_closes_on_paren() {
    let mut rows = rows_with("f(x) { switch (x) { case Foo(: 1, :): break; } }\n");
    rows.sort();
    let expected = vec![("missing_identifier".to_string(), 35, 36)];
    assert_eq!(rows, expected, "oracle rows only, got {rows:?}");
}

/// `f(x) { switch (x) { case Foo(a: , b: 1): break; } }` — the hole's
/// blocker is the `,`; it separates the next field (official
/// :12258-12266's comma arm is not even reached — the loop's own
/// `next.isA(COMMA)` arm consumes it), so only the hole reports.
#[test]
fn object_pattern_hole_before_named_field_keeps_separator() {
    let mut rows = rows_with("f(x) { switch (x) { case Foo(a: , b: 1): break; } }\n");
    rows.sort();
    let expected = vec![("missing_identifier".to_string(), 32, 33)];
    assert_eq!(rows, expected, "oracle rows only, got {rows:?}");
}

/// `f(x) { switch (x) { case Foo(a: , : 1): break; } }` — hole, then a
/// bare-colon field; the `,` separates, the `:`-field consumes its colon.
#[test]
fn object_pattern_hole_before_bare_colon_field_keeps_separator() {
    let mut rows = rows_with("f(x) { switch (x) { case Foo(a: , : 1): break; } }\n");
    rows.sort();
    let expected = vec![("missing_identifier".to_string(), 32, 33)];
    assert_eq!(rows, expected, "oracle rows only, got {rows:?}");
}

/// `f(x) { switch (x) { case Foo(a: ): } }` — empty case body after the
/// hole; the `)` closes the pattern and the case's `:` parses the empty
/// body. No `expectedButGot(')')` and no EOF-hunting `expectedButGot(':')`.
#[test]
fn object_pattern_hole_with_empty_case_body_is_clean_after_close() {
    let mut rows = rows_with("f(x) { switch (x) { case Foo(a: ): } }\n");
    rows.sort();
    let expected = vec![("missing_identifier".to_string(), 32, 33)];
    assert_eq!(rows, expected, "oracle rows only, got {rows:?}");
}

/// `f(p) { var Foo(a: , b: 1) = p; }` — the same face in the declaration
/// context: the `,` separates, only the hole's `missing_identifier` fires.
/// The declaration context legitimately adds the hole constant pattern's
/// `refutable_pattern_in_irrefutable_context` row (a dartq parser-emitted
/// mirror of the oracle's resolution-tier row — ticket_2231 suite); the
/// contract here is the absence of any `expected_token` row.
#[test]
fn object_pattern_hole_in_declaration_context_has_no_separator_row() {
    let rows = rows_with("f(p) { var Foo(a: , b: 1) = p; }\n");
    assert!(
        rows.contains(&("missing_identifier".to_string(), 18, 19)),
        "missing_identifier on the blocking `,` at offset 18, got {rows:?}"
    );
    assert!(
        !rows.iter().any(|(code, _, _)| code == "expected_token"),
        "no expected_token separator row away from the hole, got {rows:?}"
    );
}

/// Guard: the list-pattern loop KEEPS its official no-progress skip
/// (:12002-12007) — `var [a, , b] = x;` reports the hole's
/// `missing_identifier` on the blocking comma PLUS the separator
/// `expectedButGot(',')` at the following element `b` (the skip makes the
/// blocked comma unavailable as a separator).
#[test]
fn list_pattern_hole_keeps_official_skip_and_separator_row() {
    let rows = rows_with("f(x) { var [a, , b] = x; }\n");
    assert!(
        rows.contains(&("missing_identifier".to_string(), 15, 16)),
        "missing_identifier on the blocking `,` at offset 15, got {rows:?}"
    );
    assert!(
        rows.contains(&("expected_token".to_string(), 17, 18)),
        "expected_token ',' at the following element `b`, got {rows:?}"
    );
}

/// Guard: the same two-row face inside a `switch` case list pattern.
#[test]
fn case_list_pattern_hole_keeps_official_skip_and_separator_row() {
    let rows = rows_with("f(x) { switch (x) { case [1, , 2]: break; } }\n");
    assert!(
        rows.contains(&("missing_identifier".to_string(), 29, 30)),
        "missing_identifier on the blocking `,` at offset 29, got {rows:?}"
    );
    assert!(
        rows.contains(&("expected_token".to_string(), 31, 32)),
        "expected_token ',' at the following element `2`, got {rows:?}"
    );
}

/// Guard: map-pattern value holes report one `missing_identifier` per
/// empty slot and keep the `,` as separator (official :12088 skip stays).
#[test]
fn map_pattern_holes_keep_separator() {
    let rows = rows_with("f(m) { var {k: , j: 2} = m; }\n");
    assert!(
        rows.contains(&("missing_identifier".to_string(), 15, 16)),
        "missing_identifier on the blocking `,` at offset 15, got {rows:?}"
    );
    assert!(
        !rows.iter().any(|(code, _, _)| code == "expected_token"),
        "no expected_token row: the `,` separated the next entry, got {rows:?}"
    );
}

/// Guard: two map-pattern value holes inside a case each report on their
/// own blocker (oracle: `missing_identifier` at both offsets).
#[test]
fn case_map_pattern_two_holes_report_per_slot() {
    let rows = rows_with("f(x) { switch (x) { case {k: , j: }: break; } }\n");
    assert!(
        rows.contains(&("missing_identifier".to_string(), 29, 30))
            && rows.contains(&("missing_identifier".to_string(), 34, 35)),
        "missing_identifier at both holes (29, 34), got {rows:?}"
    );
    assert!(
        !rows.iter().any(|(code, _, _)| code == "expected_token"),
        "no expected_token row: both `,` separators survive, got {rows:?}"
    );
}
