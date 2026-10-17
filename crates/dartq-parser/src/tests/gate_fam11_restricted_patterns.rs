//! FAM11 restricted-pattern recovery pins: switch-case heads whose patterns
//! are shapes the grammar forbids (function-typed variable patterns, named
//! function expressions, asserts, `??`/`++`/`--` constant-pattern operands).
//! Each test pins the oracle's exact `(code, start, end)` rows against the
//! recorded differential (`const_patterns.dart` / `const_patterns_binary.dart`
//! shapes) — recovery cascades must surface once and only where the oracle
//! reports.

use super::collector::{Collector, setup};

fn parse(source: &'static str) -> Collector {
    let mut p = setup(source);
    p.parse_unit();
    std::mem::take(p.listener)
}

fn switch_case(case_head: &'static str) -> &'static str {
    // Line 3 starts at offset 27; the case head starts at offset 31.
    Box::leak(
        format!("void f(o) {{\n  switch (o) {{\n    {case_head}\n  }}\n}}\n").into_boxed_str(),
    )
}

fn rows(c: &Collector) -> Vec<(String, u32, u32)> {
    c.errors.clone()
}

fn r(code: &'static str, start: u32, end: u32) -> (String, u32, u32) {
    (code.to_string(), start, end)
}

#[test]
fn case_void_fun_reports_colon_semicolon_and_stray_colon_once() {
    // `case void fun() {}:` — the pattern eats `void fun` as a declared
    // variable; the `(` takes the missing-`:`, `() {}` becomes a function
    // literal statement missing `;`, the stray `:` gets the missing-
    // identifier + no-progress faces. The duplicate `;` report (the stray-
    // colon statement re-anchors on the same `}`) collapses.
    let src = switch_case("case void fun() {}: // Error");
    assert_eq!(
        rows(&parse(src)),
        vec![
            r("expected_token", 44, 45),     // ':' expected at the `(`
            r("expected_token", 48, 49),     // ';' expected at the `}` of `() {}`
            r("missing_identifier", 49, 50), // the stray `:`
            r("unexpected_token", 49, 50),
        ],
        "{src}"
    );
}

#[test]
fn case_assert_is_an_assert_expression() {
    // `case assert(false):` — the official parsePrimary 'assert' arm parses
    // the assert expression with no parser row (semantic rows only).
    let src = switch_case("case assert(false): // Error");
    assert!(parse(src).errors.is_empty(), "{src}");
}

#[test]
fn case_const_assert_is_an_assert_expression() {
    let src = switch_case("case const assert(false): // Error");
    assert!(parse(src).errors.is_empty(), "{src}");
}

#[test]
fn case_const_void_fun_is_a_named_function_expression() {
    // `case const void fun() {}:` — one `named_function_expression` row at
    // `fun`; the literal's `()` + body close the case head at the real `:`.
    let src = switch_case("case const void fun() {}: // Error");
    assert_eq!(
        rows(&parse(src)),
        vec![r("named_function_expression", 47, 50)],
        "{src}"
    );
}

#[test]
fn case_if_null_question_marks_cascade_face() {
    // `case 1 ?? 2:` — the pattern stops at `??`; the missing-`:` lands on
    // the `??` (the `rewriteAndRecover` anchor), the statement lane splices
    // a synthetic identifier and the precedence loop consumes `?? 2` as ONE
    // statement (the operator stays in the stream — the official
    // `TokenType.isOperator` keep-set of the expression-identifier recovery
    // template), so the `;` row anchors on the walk-back anchor `2`, and
    // the stray `:` gets the missing-identifier + no-progress faces. Exact
    // frozen-oracle set: case 0c9fda4b L85 (`const_patterns_binary.dart`),
    // `expected_token ';'` at column 15 length 1 = the `2` (41..42).
    let src = switch_case("case 1 ?? 2: // Error");
    assert_eq!(
        rows(&parse(src)),
        vec![
            r("expected_token", 38, 40),     // ':' expected at the `??`
            r("missing_identifier", 38, 40), // the `??`
            r("expected_token", 41, 42),     // ';' expected at the `2`
            r("missing_identifier", 42, 43), // the stray `:`
            r("unexpected_token", 42, 43),
        ],
        "{src}"
    );
}

#[test]
fn postfix_increment_decrement_case_heads_are_parser_silent() {
    // `case o++:` / `case o--:` — postfix `++`/`--` consume silently in the
    // constant-pattern selector loop (official precedence-loop POSTFIX arm).
    for head in ["case o++: // Error", "case o--: // Error"] {
        let src = switch_case(head);
        assert!(parse(src).errors.is_empty(), "{src}");
    }
}

#[test]
fn prefix_increment_decrement_case_heads_are_parser_silent() {
    for head in ["case ++o: // Error", "case --o: // Error"] {
        let src = switch_case(head);
        assert!(parse(src).errors.is_empty(), "{src}");
    }
}

#[test]
fn const_prefix_reports_at_the_dotted_member_head() {
    // `case const prefix.value:` — the explicit-const gate fires at the last
    // member when no `.`/`(`/`<` follows (official parseSend gate).
    let src = switch_case("case const prefix.value: // Error");
    assert_eq!(
        rows(&parse(src)),
        vec![r("invalid_constant_const_prefix", 49, 54)],
        "{src}"
    );
}

#[test]
fn const_prefix_reports_at_the_final_member_of_a_chain() {
    let src = switch_case("case const prefix.Class.named: // Error");
    assert_eq!(
        rows(&parse(src)),
        vec![r("invalid_constant_const_prefix", 55, 60)],
        "{src}"
    );
}

#[test]
fn negation_reports_per_dotted_member() {
    // `case -prefix.value:` — numeric-literal-only negation reports at the
    // send head AND each member (the official context "reset" is a no-op
    // `==` comparison, parser_impl.dart:8116).
    let src = switch_case("case -prefix.value: // Error");
    assert_eq!(
        rows(&parse(src)),
        vec![
            r("invalid_constant_pattern_negation", 37, 43), // `prefix`
            r("invalid_constant_pattern_negation", 44, 49), // `value`
        ],
        "{src}"
    );
}

#[test]
fn const_await_operand_carries_no_const_pattern_checks() {
    // `case const await 0:` — the await-expression operand parses with a
    // reset context (official parseAwaitExpression).
    let src = switch_case("case const await 0: // Error");
    assert!(parse(src).errors.is_empty(), "{src}");
}

#[test]
fn duplicate_const_replaces_the_empty_record_face() {
    // `case const const ():` — the duplicate-`const` row replaces the
    // operand's own faces; `case const ():` keeps the empty-record row.
    let dup = parse(switch_case("case const const (): // Error"));
    assert_eq!(
        rows(&dup),
        vec![r("invalid_constant_pattern_duplicate_const", 42, 47)]
    );
    let single = parse(switch_case("case const (): // Error"));
    assert_eq!(
        // The official report anchors on `begin` — the record's `(` token
        // (:8388-8393, single-token `reportRecoverableError` anchor), not
        // the whole `()` group.
        rows(&single),
        vec![r("invalid_constant_pattern_empty_record_literal", 42, 43)]
    );
}
