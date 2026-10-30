//! #3025 pins: statement-head `(...)` record-nullable vs conditional rescue,
//! and the switch-statement `expected 'case'` endGroup skip.
//!
//! Roots (upstream pin `.third/dart-sdk-3.13.7`
//! `pkg/_fe_analyzer_shared/lib/src/parser/`):
//!
//! * `parseExpressionStatementOrDeclarationAfterModifiers`
//!   (`parser_impl.dart:9931-9984`): the `X? y` vs `a ? b : c`
//!   disambiguation is gated on `typeInfo.isNullable &&
//!   typeInfo.couldBeExpression` — `SimpleNullable` covers `id?`, and the
//!   `Complex` record-nullable `(...)?` qualifies the same way
//!   (`type_info_impl.dart:591-598`: a record-type `(...)` carries no type
//!   arguments and no `Function` starters, so `couldBeExpression` is true
//!   and `beforeQuestionMark` marks it nullable). `(a.value == 37) ? a : b`
//!   reads `(...)` as a (recovered) record type whose `?` tail accepts the
//!   bare-name + `:` follower (`type_info_impl.dart:771-833`), then drops
//!   the type and reparses as the conditional expression it is — one
//!   oracle-clean statement, never the record-declaration `expected_token
//!   ')'` + `unexpected ':'` cascade dartq's narrower `SimpleNullable`
//!   gate minted (oracle `features/_core/cascade/language/cascade/
//!   precedence_test`).
//! * `parseSwitchBlock` (`parser_impl.dart:10933-10944`): the
//!   not-a-case arm reports `expected 'case'` at the peeked head and then
//!   walks `token.next` to the block's scanner-paired `endGroup` — the
//!   whole junk run is consumed silently inside the switch block, so the
//!   enclosing case loop sees `}` and exits. dartq previously left the
//!   cursor at the junk head for `parseStatementsInSwitchCase`, which
//!   reparsed `=>`-arms as statements and minted `expected_token ';'` +
//!   `missing_identifier` + `unexpected ','` rows the oracle never carries
//!   (oracle `co19/LanguageFeatures/Patterns/switch_expression_A04_t01`:
//!   `switch (s) { "one" => 1, _ => 0 };` costs exactly one
//!   `expected 'case'` at `"one"`).

use super::collector::{latest_features, scan_ok, Collector};
use crate::ParserFeatures;

/// `(code, start, end)` parser rows for `source` under `features`.
fn rows_with(source: &'static str, features: ParserFeatures) -> Vec<(String, u32, u32)> {
    let (table, text) = scan_ok(source);
    let mut collector = Collector::default();
    let mut parser = crate::Parser::new(text, table, features, &mut collector);
    parser.parse_unit();
    parser.listener.errors.clone()
}

/// `(a.value == 37) ? a : b ..set(42);` at statement position (oracle
/// `language/cascade/precedence_test`): the record-nullable `(...)?` rescue
/// drops the tentative record type and reparses the statement as the
/// conditional it is. No parser rows survive — the entire shape is
/// well-formed under the conditional reading.
#[test]
fn parenthesized_nullable_conditional_rescues() {
    let rows = rows_with(
        "void f(A a, A b) {\n  (a.value == 37) ? a : b\n    ..set(42);\n}\n",
        latest_features(),
    );
    assert!(
        rows.is_empty(),
        "the conditional reparse emits no parser rows, got {rows:?}"
    );
}

/// The same `(...)?` rescue when the type tail is genuinely a declaration:
/// `(int, int) x = (0, 0);` stays a declaration (no `?` after `)`), and
/// `(int, int)? x = null;` reads `?` as the type's nullable mark — the
/// rescue's follower test (`;`/`=` boundary) keeps it.
#[test]
fn record_nullable_declaration_keeps_type() {
    let rows = rows_with(
        "void f() {\n  (int, int)? x = null;\n}\n",
        latest_features(),
    );
    assert!(
        rows.is_empty(),
        "a record-nullable declaration parses clean, got {rows:?}"
    );
}

/// `int? x = null;` — the `SimpleNullable` arm the rescue always covered:
/// `=` + speculation landing NOT on `:` keeps the declaration.
#[test]
fn simple_nullable_declaration_keeps_type() {
    let rows = rows_with("void f() {\n  int? x = null;\n}\n", latest_features());
    assert!(
        rows.is_empty(),
        "a simple-nullable declaration parses clean, got {rows:?}"
    );
}

/// `x ? y : z;` — `x` is `Simple` (not nullable), so the rescue never fires
/// and the statement still parses as the conditional expression it is.
#[test]
fn bare_conditional_statement_clean() {
    let rows = rows_with("void f(bool x) {\n  x ? 1 : 2;\n}\n", latest_features());
    assert!(
        rows.is_empty(),
        "a bare conditional expression statement parses clean, got {rows:?}"
    );
}

/// `switch (s) { "one" => 1, _ => 0 };` (oracle
/// `Patterns/switch_expression_A04_t01`): a switch-expression-shaped body
/// at switch-STATEMENT position costs exactly one `expected 'case'` at the
/// junk head — the not-a-case arm skips the whole run to the block's
/// `endGroup`, and `parseStatementsInSwitchCase` sees `}` and exits. The
/// `=>`-arms never reparse as statements: no `expected ';'`, no
/// `missing_identifier`, no `unexpected ','`.
#[test]
fn switch_block_junk_skips_to_end_group() {
    let rows = rows_with(
        "void test(String s) {\n  switch (s) {\n    \"one\" => 1,\n    _ => 0\n  };\n}\n",
        latest_features(),
    );
    // `\"one\"` sits at offset 41..46 in the source; the report anchors
    // on the peeked label head (`"one"`'s string token).
    assert!(
        rows.contains(&("expected_token".to_string(), 41, 46)),
        "expected_token 'case' at the `\"one\"` head (41..46), got {rows:?}"
    );
    let case_rows: Vec<_> = rows
        .iter()
        .filter(|(c, _, _)| c == "expected_token")
        .collect();
    assert_eq!(
        case_rows.len(),
        1,
        "exactly one expected_token ('case') row survives, got {rows:?}"
    );
    assert!(
        !rows.iter().any(|(c, _, _)| c == "missing_identifier"),
        "no missing_identifier cascade, got {rows:?}"
    );
    assert!(
        !rows.iter().any(|(c, _, _)| c == "unexpected_token"),
        "no unexpected_token cascade, got {rows:?}"
    );
}

/// A real `case` in the same body still parses normally after the fix: the
/// skip path only fires on the not-a-case arm, the `case` arm is untouched.
#[test]
fn switch_block_real_cases_still_parse() {
    let rows = rows_with(
        "void test(String s) {\n  switch (s) {\n    case \"one\":\n      break;\n    default:\n      break;\n  }\n}\n",
        latest_features(),
    );
    assert!(
        rows.is_empty(),
        "a well-formed switch statement parses clean, got {rows:?}"
    );
}
