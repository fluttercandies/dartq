//! #3166 pins: the declarations.rs / close-bracket residuals folded from the
//! #3156 lane — the typedef raw-identifier-recovery routing, the class-name
//! `built_in_name` pre-capture that fired on a spliced name, and the
//! index-expression `]` close's missing `expectedButGot` follower row
//! (oracle 3.13.7). Upstream citations live per-test.

use super::collector::{latest_features, scan_ok};

/// Parser rows of `source` as (code, start, end) UTF-16 triples, sorted.
fn sorted_rows(source: &'static str) -> Vec<(String, u32, u32)> {
    let (table, text) = scan_ok(source);
    let mut collector = super::collector::Collector::default();
    let mut parser = crate::Parser::new(text, table, latest_features(), &mut collector);
    parser.parse_unit();
    let mut rows = parser.listener.errors.clone();
    rows.sort();
    rows
}

fn code(c: &str, start: u32, end: u32) -> (String, u32, u32) {
    (c.to_string(), start, end)
}

/// `isIdentifierRecovered` is the RAW token kind (parser_impl.dart:1593:
/// `next.kind != IDENTIFIER_TOKEN`) — a built-in keyword is NOT an
/// `IDENTIFIER_TOKEN`, so `typedef void get();` and `typedef get = ...`
/// route to `ensureIdentifierPotentiallyRecovered` and carry BOTH the
/// keyword-name row and the typedef-name built-in row. A built-in kept by
/// `_isOneOfFollowingValues` on `;` (`typedef int get;`) stays the name with
/// only the caller's built-in verifier row.
#[test]
fn typedef_keyword_name_keeps_raw_identifier_faces() {
    assert_eq!(
        sorted_rows("typedef void get();\n"),
        vec![
            code("built_in_identifier_in_declaration", 13, 16),
            code("expected_identifier_but_got_keyword", 13, 16),
        ],
        "oracle: the `(`-shape keeps `get` as the name — both the \
         keyword-name row and the built-in row land on it"
    );
    assert_eq!(
        sorted_rows("typedef get = void Function();\n"),
        vec![
            code("built_in_identifier_in_declaration", 8, 11),
            code("expected_identifier_but_got_keyword", 8, 11),
        ],
        "oracle: the `=`-shape keeps `get` as the name — same two rows"
    );
    // A built-in kept by `_isOneOfFollowingValues` on `;` is the name with
    // only the verifier's built-in row — never the keyword-name row.
    assert_eq!(
        sorted_rows("typedef int get;\n"),
        vec![
            code("built_in_identifier_in_declaration", 12, 15),
            code("missing_typedef_parameters", 15, 16),
        ],
        "oracle: `typedef int get;` carries only the built-in row at `get`, \
         then the missing-parameter row at `;`"
    );
}

/// The class-name `built_in_name` capture must not fire when the recovery
/// arm splices a synthetic name — the keyword never became the declaration
/// name. `class enum E { v }` leaves `enum` for the enum reparse, so the
/// verifier's `_checkForBuiltInIdentifierAsName` has no name to report;
/// `class enum { }` keeps `enum` (`{` is a keep follower) and the built-in
/// row lands (identifier_context_impl.dart:80-86, error_verifier.dart:573).
#[test]
fn class_name_builtin_row_only_when_name_survives() {
    assert_eq!(
        sorted_rows("class enum E { v }\n"),
        vec![
            code("expected_body", 6, 10),
            code("missing_identifier", 6, 10),
        ],
        "oracle: `class enum E {{ v }}` splices a synthetic name — no \
         built-in row, `enum` reparses as the enum head"
    );
    assert_eq!(
        sorted_rows("class enum { }\n"),
        vec![
            code("built_in_identifier_in_declaration", 6, 10),
            code("expected_identifier_but_got_keyword", 6, 10),
        ],
        "oracle: `class enum {{ }}` keeps `enum` as the name — the built-in \
         and keyword-name faces both fire"
    );
}

/// A scanner-synthetic `]` end-group is relocated by `moveSynthetic`
/// (parser_impl.dart:8018) — but the `reportRecoverableError(next,
/// expectedButGot(']'))` at :8010 fires FIRST at the follower. The relocated
/// closer's own `unmatchedToken` row (length-1 at the follower) lands beside
/// it, so `intList[x try }` carries TWO `]` rows at `try`: the full-width
/// `expectedButGot` and the length-1 closer report.
#[test]
fn index_close_reports_expected_but_got_and_unmatched() {
    let rows = sorted_rows("void f() {\n  intList[x try }\n}\n");
    // `try` is at offset 23; the length-1 unmatched `]` (23,24) and the
    // 3-length `expectedButGot` (23,26) both anchor there.
    assert!(
        rows.contains(&code("expected_token", 23, 24))
            && rows.contains(&code("expected_token", 23, 26)),
        "oracle: two `]` rows at `try` — the length-1 unmatched closer and \
         the length-3 expectedButGot; got {rows:?}"
    );
}

