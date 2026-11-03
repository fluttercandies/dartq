//! #3051 pins: partial-code directive recovery — the conditional-URI
//! synthetic closer and the directive-tail junk faces.
//!
//! Roots (upstream pin the pinned Dart SDK 3.13.7,
//! `pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart`):
//!
//! * `parseConditionalUri` :1117-1141: when the condition group's scanner
//!   closer is synthetic (unclosed `(` parked at EOF), `moveSynthetic`
//!   :1130-1132 RELOCATES it to the current token and consumes it — the
//!   intervening junk stays in the stream for the candidate URI and the
//!   directive tail. A real closer elsewhere reports the intervening token
//!   (`unexpectedToken`) and jumps (:1133-1135). The relocated closer's
//!   unmatched-group row lands at the relocation point with length 1 (the
//!   analyzer translation reads `begin.endGroup.charOffset` post-parse), and
//!   the candidate URI after the `)` re-reads the junk, so the missing
//!   literal anchors on the junk — never on EOF.
//! * `ensureSemicolon` :5045-5057: the `;` anchor is
//!   `findPreviousNonZeroLengthToken` (`util.dart`) — the walk-back crosses
//!   every zero-width synthetic (the relocated closer, the spliced URI
//!   strings), landing on the last real token of the clause.
//! * Directive tails have no recovery loop (`parseExport` :1180,
//!   `parseLibraryName` :907, `parsePartOf` :1315): junk after the tail is
//!   the `ensureSemicolon` row plus the junk's own top-level reparse.
//! * `parsePartOf` :1315-1327: the name form keys on `ofKeyword.next!
//!   .isIdentifier` — a reserved keyword (`class`) takes the URI form and
//!   reports `expectedString` at the keyword.

use super::collector::{latest_features, scan_ok, Collector};

/// `(code, start, end)` parser rows for `source` under latest features.
fn rows_with(source: &'static str) -> Vec<(String, u32, u32)> {
    let (table, text) = scan_ok(source);
    let mut collector = Collector::default();
    let mut parser = crate::Parser::new(text, table, latest_features(), &mut collector);
    parser.parse_unit();
    parser.listener.errors.clone()
}

/// `import 'a' if (x == class` (oracle
/// `partial_code/import_directive` conditional-URI junk): the `(` parks a
/// synthetic `)` at EOF; `moveSynthetic` relocates it onto `class`, so the
/// candidate URI re-reads `class` — `expected_string_literal` at `class`
/// (20..25; the two identical parser rows dedup in the final face), the
/// unmatched-group row `expected_token ')'` at the relocation point
/// (20..21), and the recovery tail's `expected_token ';'` walks back over
/// the zero-width synthetics to `==` (17..19). `class` survives for the
/// top-level loop. The old skip-to-closer shape (extra `expected_string`
/// at EOF, `;` anchored on the swallowed junk) may not return.
#[test]
fn conditional_uri_synthetic_closer_relocates_over_junk() {
    let rows = rows_with("import 'a' if (x == class\n");
    assert!(
        rows.contains(&("expected_string_literal".to_string(), 20, 25)),
        "expected_string_literal at `class` (20..25), got {rows:?}"
    );
    assert!(
        rows.contains(&("expected_token".to_string(), 20, 21)),
        "expected_token ')' at the relocation point (20..21), got {rows:?}"
    );
    assert!(
        rows.contains(&("expected_token".to_string(), 17, 19)),
        "expected_token ';' walked back to `==` (17..19), got {rows:?}"
    );
    assert!(
        !rows
            .iter()
            .any(|(c, s, _)| c == "expected_string_literal" && *s != 20),
        "no expected_string row away from `class`, got {rows:?}"
    );
    assert!(
        !rows.iter().any(|(c, _, _)| c == "unexpected_token"),
        "the junk is relocated past, not reported, got {rows:?}"
    );
}

/// `import 'a' if (x == 'b'` (missing closer at EOF): the relocated `)`
/// parks at EOF — `expected_token ')'` at the EOF offset with length 1 and
/// the candidate URI's `expected_string_literal` at the same zero-width
/// spot; the `;` anchors on the last clause token `'b'` (20..23).
#[test]
fn conditional_uri_synthetic_closer_at_eof() {
    let rows = rows_with("import 'a' if (x == 'b'\n");
    assert!(
        rows.contains(&("expected_token".to_string(), 20, 23)),
        "expected_token ';' at `'b'` (20..23), got {rows:?}"
    );
    assert!(
        rows.contains(&("expected_string_literal".to_string(), 24, 24)),
        "expected_string_literal at EOF (24), got {rows:?}"
    );
    assert!(
        rows.contains(&("expected_token".to_string(), 24, 25)),
        "expected_token ')' at EOF, length 1 (24..25), got {rows:?}"
    );
}

/// `import 'a' if (x class` (junk in the dotted-name slot): the relocated
/// closer lands on `class`, the candidate URI reports at it (17..22), and
/// the `;` walks back to `x` (15..16) — never onto the junk itself.
#[test]
fn conditional_uri_junk_in_dotted_name_slot() {
    let rows = rows_with("import 'a' if (x class\n");
    assert!(
        rows.contains(&("expected_string_literal".to_string(), 17, 22)),
        "expected_string_literal at `class` (17..22), got {rows:?}"
    );
    assert!(
        rows.contains(&("expected_token".to_string(), 17, 18)),
        "expected_token ')' at the relocation point (17..18), got {rows:?}"
    );
    assert!(
        rows.contains(&("expected_token".to_string(), 15, 16)),
        "expected_token ';' at `x` (15..16), got {rows:?}"
    );
    assert!(
        !rows.iter().any(|(c, _, _)| c == "unexpected_token"),
        "no unexpected_token cascade, got {rows:?}"
    );
}

/// `import 'a' if (x == 'b' ]` (junk before the parked closer): same
/// relocation shape — the URI and the `')'` row land on `]` (24..25), the
/// `;` on `'b'` (20..23), and `]` survives for the top-level junk face.
#[test]
fn conditional_uri_junk_before_parked_closer() {
    let rows = rows_with("import 'a' if (x == 'b' ]\n");
    assert!(
        rows.contains(&("expected_string_literal".to_string(), 24, 25)),
        "expected_string_literal at `]` (24..25), got {rows:?}"
    );
    assert!(
        rows.contains(&("expected_token".to_string(), 24, 25)),
        "expected_token ')' at the relocation point (24..25), got {rows:?}"
    );
    assert!(
        rows.contains(&("expected_token".to_string(), 20, 23)),
        "expected_token ';' at `'b'` (20..23), got {rows:?}"
    );
}

/// `import 'a' if x == 'b' ;` (missing `(`): the `insertParens` triple
/// (`token_stream_rewriter.dart:29`, all three tokens at the follower)
/// walks the synthetic name and closer silently — the directive rows are
/// `expected_token '('` at `x` (14..15, the `expectedButGot` shared
/// code), the candidate URI's `expected_string_literal` at `x`, and the
/// tail `;` walked back to `if` (11..13); the surviving `x == 'b' ;` then
/// reparses as the top-level declaration junk the oracle also shows.
#[test]
fn conditional_uri_missing_paren_inserts_triple() {
    let rows = rows_with("import 'a' if x == 'b' ;\n");
    assert_eq!(
        rows,
        vec![
            ("expected_token".to_string(), 14, 15),
            ("expected_string_literal".to_string(), 14, 15),
            ("expected_token".to_string(), 11, 13),
            ("missing_const_final_var_or_type".to_string(), 14, 15),
            ("expected_token".to_string(), 14, 15),
            ("expected_executable".to_string(), 16, 18),
            ("expected_executable".to_string(), 19, 22),
            ("unexpected_token".to_string(), 23, 24),
        ],
        "insert-triple directive rows + declaration reparse, got {rows:?}"
    );
}

/// `export 'a' if (x == class` — the export path shares
/// `parseConditionalUriStar`, so the relocation face is identical to the
/// import shape (rows at `==` 17..19, `class` 20..25).
#[test]
fn export_conditional_uri_synthetic_closer() {
    let rows = rows_with("export 'a' if (x == class\n");
    assert!(
        rows.contains(&("expected_string_literal".to_string(), 20, 25))
            && rows.contains(&("expected_token".to_string(), 20, 21))
            && rows.contains(&("expected_token".to_string(), 17, 19)),
        "export relocation rows (;/)/string at ==|class), got {rows:?}"
    );
    assert!(
        !rows
            .iter()
            .any(|(c, s, _)| c == "expected_string_literal" && *s != 20),
        "no expected_string row away from `class`, got {rows:?}"
    );
}

/// `import 'a' if (x.) if (y` — the first clause's real `)` ends the
/// dotted-name continuation with `missing_identifier` (17..18) and the
/// second clause is consumed by the recovery loop's
/// `parseConditionalUriStar` (`parseImportRecovery` :1019), so the
/// relocation rows land at EOF and the `;` walks back to `y` (23..24).
#[test]
fn conditional_uri_second_clause_in_recovery_loop() {
    let rows = rows_with("import 'a' if (x.) if (y\n");
    assert!(
        rows.contains(&("missing_identifier".to_string(), 17, 18)),
        "missing_identifier at `)` (17..18), got {rows:?}"
    );
    assert!(
        rows.contains(&("expected_string_literal".to_string(), 19, 21)),
        "first clause URI missing at `if` (19..21), got {rows:?}"
    );
    assert!(
        rows.contains(&("expected_string_literal".to_string(), 25, 25))
            && rows.contains(&("expected_token".to_string(), 25, 26)),
        "second clause relocation rows at EOF (25), got {rows:?}"
    );
    assert!(
        rows.contains(&("expected_token".to_string(), 23, 24)),
        "expected_token ';' at `y` (23..24), got {rows:?}"
    );
}

/// `import 'a' if (x == 'b') ;` — a complete clause with no candidate URI:
/// the only row is `expected_string_literal` on the surviving `;`
/// (25..26), which still terminates the directive (`ensureLiteralString`
/// :5030 splices the synthetic before it).
#[test]
fn conditional_uri_missing_candidate_uri() {
    let rows = rows_with("import 'a' if (x == 'b') ;\n");
    assert_eq!(
        rows,
        vec![("expected_string_literal".to_string(), 25, 26)],
        "single missing-candidate row, got {rows:?}"
    );
}

/// Directive-tail keyword junk (`export 'a' class`, oracle
/// `partial_code/export_directive`): no recovery loop — one
/// `expected_token ';'` anchored on the last clause token `'a'` (7..10);
/// `class` stays for the top-level loop. Same for the `part` tail
/// (`part 'a'; part`: `;` on the second `part` 10..14, string row at EOF).
#[test]
fn directive_tail_keyword_junk_anchors_on_last_clause_token() {
    let rows = rows_with("export 'a' class\n");
    assert!(
        rows.contains(&("expected_token".to_string(), 7, 10)),
        "expected_token ';' at `'a'` (7..10), got {rows:?}"
    );
    assert!(
        !rows.iter().any(|(c, _, _)| c == "unexpected_token"),
        "no directive-side cascade on the keyword tail, got {rows:?}"
    );
    let rows = rows_with("part 'a'; part\n");
    assert!(
        rows.contains(&("expected_token".to_string(), 10, 14))
            && rows.contains(&("expected_string_literal".to_string(), 15, 15)),
        "part tail rows (`;` on `part`, string at EOF), got {rows:?}"
    );
}

/// Library-name tails (`parseLibraryName` :907): a missing name at EOF
/// (8) splices the synthetic and anchors `;` on `library` (0..7); `==`
/// junk is consumed as the name with `missing_identifier` (8..10) and `;`
/// on it; a trailing `.` reports `missing_identifier` at EOF (11) with
/// `;` on the dot (9..10); `class` takes the keyword-consume face
/// (`expected_identifier_but_got_keyword` + `;`, both 8..13); `import` is
/// a declaration head whose follower continues the name, so the tolerant
/// consume keeps it silent and `;` lands on it (8..14).
#[test]
fn library_name_tails() {
    assert!(rows_with("library\n").contains(&("missing_identifier".to_string(), 8, 8)));
    assert!(rows_with("library\n").contains(&("expected_token".to_string(), 0, 7)));

    let rows = rows_with("library ==\n");
    assert!(rows.contains(&("missing_identifier".to_string(), 8, 10)), "{rows:?}");
    assert!(rows.contains(&("expected_token".to_string(), 8, 10)), "{rows:?}");

    let rows = rows_with("library a.\n");
    assert!(rows.contains(&("missing_identifier".to_string(), 11, 11)), "{rows:?}");
    assert!(rows.contains(&("expected_token".to_string(), 9, 10)), "{rows:?}");

    let rows = rows_with("library class\n");
    assert!(
        rows.contains(&("expected_identifier_but_got_keyword".to_string(), 8, 13)),
        "{rows:?}"
    );
    assert!(rows.contains(&("expected_token".to_string(), 8, 13)), "{rows:?}");

    let rows = rows_with("library import\n");
    assert!(
        !rows.iter().any(|(c, _, _)| {
            c == "missing_identifier" || c == "expected_identifier_but_got_keyword"
        }),
        "`import` consumes silently as the name, got {rows:?}"
    );
    assert!(rows.contains(&("expected_token".to_string(), 8, 14)), "{rows:?}");
}

/// `part of` tails (`parsePartOf` :1315): junk and reserved keywords take
/// the URI form — `expected_string_literal` at the junk with `;` on `of`
/// (5..7); a trailing `.` in the name form reports `missing_identifier` at
/// EOF (11) with `;` on the dot (9..10); a clean URI form anchors `;` on
/// the string (8..11); a clean dotted name anchors `;` on `b` (10..11).
#[test]
fn part_of_tails() {
    let rows = rows_with("part of\n");
    assert!(
        rows.contains(&("expected_string_literal".to_string(), 8, 8))
            && rows.contains(&("expected_token".to_string(), 5, 7)),
        "{rows:?}"
    );

    let rows = rows_with("part of ==\n");
    assert!(
        rows.contains(&("expected_string_literal".to_string(), 8, 10))
            && rows.contains(&("expected_token".to_string(), 5, 7)),
        "{rows:?}"
    );

    let rows = rows_with("part of class\n");
    assert!(
        rows.contains(&("expected_string_literal".to_string(), 8, 13))
            && rows.contains(&("expected_token".to_string(), 5, 7)),
        "reserved keyword takes the URI form, got {rows:?}"
    );

    let rows = rows_with("part of a.\n");
    assert!(
        rows.contains(&("missing_identifier".to_string(), 11, 11))
            && rows.contains(&("expected_token".to_string(), 9, 10)),
        "{rows:?}"
    );

    let rows = rows_with("part of 'a'\n");
    assert_eq!(rows, vec![("expected_token".to_string(), 8, 11)], "{rows:?}");

    let rows = rows_with("part of a.b\n");
    assert_eq!(rows, vec![("expected_token".to_string(), 10, 11)], "{rows:?}");
}

/// Import tails: bare `import` (string at EOF 7, `;` on the keyword
/// 0..6); `as` with no prefix name (missing_identifier at EOF 14, `;` on
/// `as` 11..13); a bare `deferred` carries into the recovery loop and
/// reports both `missing_prefix_in_deferred_import` and the `;` on the
/// keyword (11..19).
#[test]
fn import_tails() {
    let rows = rows_with("import\n");
    assert!(
        rows.contains(&("expected_string_literal".to_string(), 7, 7))
            && rows.contains(&("expected_token".to_string(), 0, 6)),
        "{rows:?}"
    );

    let rows = rows_with("import 'a' as\n");
    assert!(
        rows.contains(&("missing_identifier".to_string(), 14, 14))
            && rows.contains(&("expected_token".to_string(), 11, 13)),
        "{rows:?}"
    );

    let rows = rows_with("import 'a' deferred\n");
    assert!(
        rows.contains(&("missing_prefix_in_deferred_import".to_string(), 11, 19))
            && rows.contains(&("expected_token".to_string(), 11, 19)),
        "{rows:?}"
    );
}

/// Combinator tails (`parseIdentifierList` :1250 through the `Combinator`
/// context): `==` junk takes the consume-in-doubt branch —
/// `missing_identifier` at `==` (16..18) and the `;` anchored on it; a
/// trailing `,` splices the name at EOF (18) and anchors `;` on the comma
/// (17..18).
#[test]
fn combinator_tails() {
    let rows = rows_with("export 'a' show ==\n");
    assert!(
        rows.contains(&("missing_identifier".to_string(), 16, 18))
            && rows.contains(&("expected_token".to_string(), 16, 18)),
        "{rows:?}"
    );

    let rows = rows_with("export 'a' show A,\n");
    assert!(
        rows.contains(&("missing_identifier".to_string(), 19, 19))
            && rows.contains(&("expected_token".to_string(), 17, 18)),
        "{rows:?}"
    );
}

/// Clean directives stay silent: a fully-formed conditional chain and a
/// clean `part of` carry zero rows (regression guard for the relocation
/// arm — the real-closer path reports nothing).
#[test]
fn clean_directives_stay_silent() {
    assert!(rows_with("import 'a' if (x == 'b') 'c' if (y) 'd';\n").is_empty());
    assert!(rows_with("export 'a' show b hide c;\n").is_empty());
    assert!(rows_with("library foo.bar;\n").is_empty());
    assert!(rows_with("part of foo.bar;\n").is_empty());
}
