//! #3185 pins: `illegal_assignment_to_non_assignable` is a STATEMENT-ROOT
//! rider, never a context-free arm row.
//!
//! Upstream reports it only from `handleExpressionStatement`
//! (ast_builder.dart:4206-4223): the WHOLE statement expression must be the
//! `AssignmentExpression` and its LHS must not be assignable. The assignment
//! arm itself carries `missingAssignableSelector` alone (:3824-3841). So the
//! same non-assignable LHS inside a constructor initializer, a variable
//! initializer, an annotation argument or a partial-code recovery expression
//! keeps the selector row and drops the illegal rider.
//!
//! `Parser::statement_expr_begin` records the expression statement's first
//! token; the arm's fallback fires the rider only when its own operand frame
//! starts at that token — i.e. when the assignment is the statement's root
//! node. The postfix-unary arm is context-free and keeps its row everywhere
//! (`handleUnaryPostfixAssignmentExpression`, :5627-5642).
//!
//! Measured EXTRA rows this gate removes (oracle keeps the selector and the
//! recovery rows, never the rider):
//! - `A() : super() = 0;` — ctor-initializer lane (oracle 3:9|7 MAS +
//!   invalidInitializer);
//! - `var f = const f = 0;` — variable-initializer lane (oracle 2:9|8 MAS +
//!   expectedToken).

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

fn mas(start: u32, end: u32) -> (String, u32, u32) {
    code("missing_assignable_selector", start, end)
}

fn illegal(start: u32, end: u32) -> (String, u32, u32) {
    code("illegal_assignment_to_non_assignable", start, end)
}

/// A constructor initializer's super-call target: the arm's selector row
/// survives, the statement rider does not (`super()` = 16..23). The separate
/// `invalidInitializer` row over the whole entry (16..27) landed with #3193.
#[test]
fn ticket_3185_initializer_lane_drops_the_rider() {
    assert_eq!(
        sorted_rows("class A { A() : super() = 0; }\n"),
        vec![code("invalid_initializer", 16, 27), mas(16, 23)],
        "oracle: the ctor initializer is not a statement expression — \
         MAS + the whole-entry invalidInitializer, never the rider"
    );
}

/// A variable initializer's non-assignable assignment target: selector row
/// plus the recovery's `expected_token`, no rider (`const f` = 18..26).
#[test]
fn ticket_3185_variable_initializer_lane_drops_the_rider() {
    assert_eq!(
        sorted_rows("class C { var f = const f = 0; }\n"),
        vec![code("expected_token", 24, 25), mas(18, 26)],
        "oracle: `var f = const f = 0;` is an initializer, not a statement — \
         no illegal rider"
    );
}

/// A real statement keeps BOTH rows: a literal LHS (`0 = 1;`, LHS = 11..12).
#[test]
fn ticket_3185_statement_root_keeps_the_rider() {
    assert_eq!(
        sorted_rows("void f() { 0 = 1; }\n"),
        vec![illegal(11, 12), mas(11, 12)],
        "oracle: the statement's root assignment carries both rows"
    );
}

/// A binary-expression LHS at statement root keeps both rows too
/// (`1 + 2` = 11..16).
#[test]
fn ticket_3185_statement_root_binary_lhs_keeps_the_rider() {
    assert_eq!(
        sorted_rows("void f() { 1 + 2 = 3; }\n"),
        vec![illegal(11, 16), mas(11, 16)],
        "oracle: the root assignment's whole LHS run carries both rows"
    );
}

/// A NESTED assignment inside a statement (`x = (a) = 1;`) is not the
/// statement's root node: the inner target keeps the selector row only
/// (oracle: the root LHS is `x`, which is assignable).
#[test]
fn ticket_3185_nested_assignment_in_statement_drops_the_rider() {
    assert_eq!(
        sorted_rows("void f() { x = (a) = 1; }\n"),
        vec![mas(15, 18)],
        "oracle: only the statement ROOT assignment may carry the rider"
    );
}

/// The for-INIT clause is not an expression statement upstream
/// (`parseExpression` + `handleForInitializerExpressionStatement`,
/// parser_impl.dart:10318-10324): selector only.
#[test]
fn ticket_3185_for_initializer_clause_drops_the_rider() {
    assert_eq!(
        sorted_rows("void f() { for (0 = 1;;) {} }\n"),
        vec![mas(16, 17)],
        "oracle: the for-init clause has no expression-statement rider"
    );
}

/// The for-CONDITION clause IS an expression statement upstream
/// (`parseExpressionStatement`), so the root assignment keeps both rows.
#[test]
fn ticket_3185_for_condition_clause_keeps_the_rider() {
    assert_eq!(
        sorted_rows("void f() { for (; 0 = 1;) {} }\n"),
        vec![illegal(18, 19), mas(18, 19)],
        "oracle: the for-condition clause is an expression statement"
    );
}

/// The `const`-statement re-read (`parse_expression_statement_or_const_declaration`
/// resets to the `const` and re-enters the expression-statement parser) must
/// keep both rows: the re-entry records `const` as the statement root
/// (`const (_)` = 9..18).
#[test]
fn ticket_3185_const_statement_reread_keeps_the_rider() {
    assert_eq!(
        sorted_rows("f(x) {\n  const (_) = x;\n}\n"),
        vec![
            illegal(9, 18),
            mas(9, 18),
            code("record_literal_one_positional_no_trailing_comma", 17, 18)
        ],
        "oracle: the const re-read is an expression statement — both rows stay"
    );
}
