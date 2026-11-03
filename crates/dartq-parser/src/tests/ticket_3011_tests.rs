//! #3011 pins: combinator/directive tail recovery + type-variable name-slot
//! keep-set + `await for` synthetic-`in` dispatch.
//!
//! Roots (upstream pin the pinned Dart SDK 3.13.7
//! `pkg/_fe_analyzer_shared/lib/src/parser/`):
//!
//! * `CombinatorIdentifierContext.ensureIdentifier`
//!   (`identifier_context_impl.dart:133-168`): an identifier-shaped head
//!   that starts the next top-level declaration leaves the combinator name
//!   slot when its own follower is not one of the context's values (`;` `,`
//!   `if` `as` `show` `hide` EOF) — `export 'a.dart' hide A, export 'a.dart';`
//!   splices a synthetic name before `export`, which the top-level loop
//!   reparses as the next directive. Non-identifier junk (`@`) takes the
//!   consume-in-doubt branch: `missing_identifier` at the junk, then the
//!   directive tail anchors its `expected_token ';'` on the junk itself.
//! * `TypeVariableDeclarationIdentifierContext.ensureIdentifier`
//!   (`identifier_context_impl.dart:1589-1603`): `extends`/`super` (and the
//!   member-start keywords) sit in the keep-set, so `void b< extends int>()`
//!   reports `missing_identifier` at `extends` and parses `extends int` as
//!   the bound — never `expectedIdentifierButGotKeyword` + a missing-comma
//!   `expected_token ','` cascade. The scan side commits the group through
//!   `recovered_group_end` (`type_info_impl.dart:1395-1423`), which steps
//!   past the junk token before probing closers — `< extends int>` ends on
//!   the real `>`.
//! * `parseForLoopPartsMid` (`parser_impl.dart:10269-10281`): the await-for
//!   junk arm inserts a synthetic `in` *ahead of* `next` — `token.next`
//!   becomes the synthetic — so `parseForInRest` consumes it and the junk
//!   stays for the iterable/`)` recovery instead of falling to the C-style
//!   `parseForRest` (`expected ';'` + `unexpected <junk>` cascades).

use super::collector::{latest_features, scan_ok, Collector};

/// `(code, start, end)` parser rows for `source` under latest features.
fn rows_with(source: &'static str, features: crate::ParserFeatures) -> Vec<(String, u32, u32)> {
    let (table, text) = scan_ok(source);
    let mut collector = Collector::default();
    let mut parser = crate::Parser::new(text, table, features, &mut collector);
    parser.parse_unit();
    parser.listener.errors.clone()
}

/// `export 'a.dart' hide A, export 'a.dart';` (oracle
/// `partial_code/export_directive/hideComma_export`): the combinator list
/// ends at `export` (built-in keyword heading a declaration, `'a.dart'`
/// follower not in the keep-set) — `missing_identifier` @`export` (offset
/// 25, len 6), `expected_token ';'` anchored on the `,` (offset 23, the
/// last consumed real token behind the synthetic name). The second
/// `export` reparses as a directive (its `uri_does_not_exist` row is
/// analyzer-side, outside the parser face).
#[test]
fn combinator_list_ends_before_next_directive() {
    let rows = rows_with(
        "export 'a.dart' hide A, export 'a.dart';\n",
        latest_features(),
    );
    assert!(
        rows.contains(&("missing_identifier".to_string(), 24, 30)),
        "missing_identifier at `export` (24..30), got {rows:?}"
    );
    assert!(
        rows.contains(&("expected_token".to_string(), 22, 23)),
        "expected_token ';' anchored on `,` (22..23), got {rows:?}"
    );
    assert!(
        !rows.iter().any(|(c, _, _)| c == "unexpected_token"),
        "no unexpected_token cascade may survive, got {rows:?}"
    );
    // `import 'a' show export;` — the `;` follower keeps `export` as the
    // last combinator name (the keep-set arm), no missing_identifier.
    assert!(
        !rows_with("import 'a' show export;\n", latest_features())
            .iter()
            .any(|(c, _, _)| c == "missing_identifier")
    );
}

/// `import "...show @` (oracle `recovery_parser/combinator_badIdentifier`):
/// the `@` junk takes the consume-in-doubt branch — one `missing_identifier`
/// at `@` (offset 26) and the directive tail's `expected_token ';'` anchored
/// on `@` (27 → the consumed junk, not `show`).
#[test]
fn combinator_junk_consumes_in_doubt() {
    let rows = rows_with("import \"/testB.dart\" show @\n", latest_features());
    assert!(
        rows.contains(&("expected_token".to_string(), 26, 27)),
        "expected_token ';' anchored on the consumed `@` (26..27), got {rows:?}"
    );
}

/// `void b< extends int>()` (oracle `_misc/assignment` /
/// `missing_type_parameter_name` family): `extends` sits in the
/// type-variable name keep-set, so one `missing_identifier` @`extends`
/// (offset 8, len 7) and no `expected_token ','` cascade — the group ends
/// on the real `>` (`recovered_group_end` steps past the junk).
#[test]
fn type_variable_missing_name_before_extends() {
    let rows = rows_with("void b< extends int>() {}\n", latest_features());
    assert!(
        rows.contains(&("missing_identifier".to_string(), 8, 15)),
        "missing_identifier at `extends` (8..15), got {rows:?}"
    );
    assert!(
        !rows.iter().any(|(c, _, _)| {
            c == "expected_token" || c == "expected_identifier_but_got_keyword"
        }),
        "no keyword-consume or missing-comma cascade, got {rows:?}"
    );
}

/// `await for ( break; }` (oracle `for_each_statement/await_leftParen_break`):
/// the junk parses as the initializer-expression, the single
/// `expected_token 'in'` fires at the junk, the synthetic `in` is consumed
/// by `parseForInRest`, and `ensureCloseParen` anchors `')'` — the C-style
/// `parseForRest` arms (`expected ';'`, `unexpected 'break'`/`'}'`) may
/// never fire.
#[test]
fn await_for_junk_takes_for_in_dispatch() {
    let rows = rows_with("f() async { await for ( break; }\n", latest_features());
    assert!(
        !rows
            .iter()
            .any(|(c, _, _)| c == "unexpected_token"),
        "no unexpected_token junk rows, got {rows:?}"
    );
    // `break` spans 24..29; the for-in recovery anchors `')'` at its head
    // (24..25) exactly as the oracle's single row — the upstream
    // `expectedIdentifier`/`expectedToken('in')` rows at the same offset
    // collapse in the analyzer surface, which is why the oracle carries
    // only `')'`.
    assert!(
        rows.contains(&("expected_token".to_string(), 24, 25)),
        "expected_token ')' anchored on `break` head (24..25), got {rows:?}"
    );
    assert!(
        !rows
            .iter()
            .any(|(c, s, _)| c == "expected_token" && *s == 30),
        "no C-style `expected ';'` cascade on the `;`, got {rows:?}"
    );
}
