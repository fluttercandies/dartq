//! #3049 pins: the statement-level junk-recovery faces of the for-loop
//! parts, the try/catch/finally clauses, and the parenthesized conditions,
//! each row A/B'd live against `dart analyze --format machine` 3.13.1.
//!
//! Two families:
//!
//! * Clause-head body anchors — upstream `ensureBlock`'s message flavors
//!   anchor the missing-`{` report on the CALLER's token
//!   (`reportRecoverableError(token, message)`, parser_impl.dart:4995-5002),
//!   and the try dispatch passes the clause head: the `try` keyword
//!   (:10709), the catch parameters' last consumed token (:10825), the
//!   `finally` keyword (:10835), and the on-clause type's last consumed
//!   token for a catchless on clause. Zero-width synthetic anchors (the
//!   `insertParens`/`moveSynthetic` catch repairs) walk forward onto the
//!   junk (`findNonZeroLengthToken`, util.dart:56-61).
//! * Already-matched faces pinned to stay matched: the for-loop junk rows
//!   (`expected_token ')'`/`';'`, `unexpected_token`), the `catch_syntax`
//!   family, and the condition-internal two-expression recovery
//!   (`expected_token ')'` on the second expression).

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

fn et(start: u32, end: u32) -> (String, u32, u32) {
    code("expected_token", start, end)
}

fn ut(start: u32, end: u32) -> (String, u32, u32) {
    code("unexpected_token", start, end)
}

fn eb(start: u32, end: u32) -> (String, u32, u32) {
    code("expected_body", start, end)
}

fn cs(start: u32, end: u32) -> (String, u32, u32) {
    code("catch_syntax", start, end)
}

fn mcof(start: u32, end: u32) -> (String, u32, u32) {
    code("missing_catch_or_finally", start, end)
}

fn vat(start: u32, end: u32) -> (String, u32, u32) {
    code("var_and_type", start, end)
}

fn eibgk(start: u32, end: u32) -> (String, u32, u32) {
    code("expected_identifier_but_got_keyword", start, end)
}

fn ms(start: u32, end: u32) -> (String, u32, u32) {
    code("missing_statement", start, end)
}

/// The for-loop junk faces: junk in the for-in iterable tail, the C-style
/// updaters, and after a complete `for (;;)` header. The oracle routes each
/// to a single row — `expected_token ')'` on the junk (for-in tail, the
/// for-in `ensureCloseParen` face), `unexpected_token` on the updater junk
/// (the `parseForLoopPartsRest` landing gate), and `expected_token ';'` on
/// `extra` (the body's expression statement missing its semicolon; the
/// header itself stays silent). A declaration-shaped initializer
/// (`for (x junk; y; z)`, `for (var x junk in y)`) parses clean —
/// `x junk` is a type+name pair — carrying only the resolver-level
/// `var_and_type` row.
#[test]
fn for_junk_shapes_match_oracle() {
    assert_eq!(
        sorted_rows("void f() { for (var x in y junk) {} }\n"),
        vec![et(27, 31)],
        "oracle: for-in junk routes to the iterable's expected-`)` face"
    );
    assert_eq!(
        sorted_rows("void f() async { await for (var x in y junk) {} }\n"),
        vec![et(39, 43)],
        "oracle: the await-for tail rides the same for-in face"
    );
    assert_eq!(
        sorted_rows("void f() { for (var i = 0; i < 3; i++ garbage) {} }\n"),
        vec![ut(38, 45)],
        "oracle: updater junk is the C-style landing gate's unexpected_token"
    );
    assert_eq!(
        sorted_rows("void f() { for (;;) extra {} }\n"),
        vec![et(20, 25)],
        "oracle: the complete header stays silent; `extra` is a body \
         statement missing `;`"
    );
    assert_eq!(
        sorted_rows("void f() { for (x junk; y; z) {} }\n"),
        Vec::<(String, u32, u32)>::new(),
        "oracle: `x junk` is a declaration (type + name) — no parser rows"
    );
    assert_eq!(
        sorted_rows("void f() { for (var x junk in y) {} }\n"),
        vec![vat(16, 19)],
        "oracle: `var x junk` carries the var-and-type row, then parses as \
         a for-in declaration"
    );
    assert_eq!(
        sorted_rows("void f() { for (var x in y; junk) {} }\n"),
        vec![et(26, 27)],
        "oracle: a `;` after the iterable is the expected-`)` face on it; \
         the junk is consumed without further parser rows"
    );
}

/// The try/catch clause-body anchors: the missing block report anchors on
/// the clause head — `try` keyword, the catch parameters' closer, the
/// on-clause type — never on the junk itself; the zero-width synthetic
/// closer of the `catch_syntax` repairs walks forward onto the junk.
#[test]
fn try_catch_body_anchors_match_oracle() {
    assert_eq!(
        sorted_rows("void f() { try junk {} }\n"),
        vec![eb(11, 14), et(15, 19), mcof(11, 14)],
        "oracle: both the body row and onlyTry anchor on the `try` keyword; \
         the junk re-parses as a body statement missing `;`"
    );
    assert_eq!(
        sorted_rows("void f() { try {} catch (e) junk {} }\n"),
        vec![eb(26, 27), et(28, 32)],
        "oracle: the catch body row anchors on the parameter list's `)`"
    );
    assert_eq!(
        sorted_rows("void f() { try {} catch (e junk) {} }\n"),
        vec![cs(27, 31)],
        "oracle: the catch_syntax repair leaves the block clean — no body row"
    );
    assert_eq!(
        sorted_rows("void f() { try {} catch junk (e) {} }\n"),
        vec![cs(24, 28), eb(24, 28)],
        "oracle: the inserted synthetic `)` is zero-width at `junk`; the \
         body row walks onto it"
    );
    assert_eq!(
        sorted_rows("void f() { try {} on T catch bad {} }\n"),
        vec![cs(29, 32), eb(29, 32), et(29, 32)],
        "oracle: the insertParens repair anchors the body row on `bad`, \
         alongside catch_syntax and the reparse's missing `;`"
    );
    assert_eq!(
        sorted_rows("void f() { try {} on T junk {} }\n"),
        vec![eb(21, 22), et(23, 27)],
        "oracle: a catchless on clause anchors the body row on the type's \
         last consumed token"
    );
    assert_eq!(
        sorted_rows("void f() { try {} on T {} }\n"),
        Vec::<(String, u32, u32)>::new(),
        "oracle: an on clause with a body parses clean"
    );
}

/// The finally-clause faces: the body row anchors on the `finally` keyword
/// itself (`parseBlock(token, BlockKind.finallyClause)` passes the keyword),
/// and a second clause falls to the statement grammar's keyword-junk rows.
#[test]
fn finally_clause_faces_match_oracle() {
    assert_eq!(
        sorted_rows("void f() { try {} catch (e) {} finally junk {} }\n"),
        vec![eb(31, 38), et(39, 43)],
        "oracle: the finally body row anchors on the `finally` keyword; \
         `junk` is a body statement missing `;`"
    );
    assert_eq!(
        sorted_rows("void f() { try {} finally finally {} }\n"),
        vec![eb(18, 25), eibgk(26, 33), et(26, 33), ms(26, 33),],
        "oracle: the first `finally` carries the body row; the second is \
         statement-position keyword junk (identifier row, missing \
         statement, missing `;`)"
    );
    assert_eq!(
        sorted_rows("void f() { try {} catch (e) {} finally {} finally {} }\n"),
        vec![eibgk(42, 49), et(42, 49), ms(42, 49)],
        "oracle: a complete finally clause parses clean; the second \
         `finally` is statement junk"
    );
    assert_eq!(
        sorted_rows("void f() { try {} finally {} catch (e) {} }\n"),
        vec![eibgk(29, 34), et(37, 38)],
        "oracle: `catch` after `finally` is statement junk — the invocation \
         shape `catch(e)` reparse carries the identifier row on `catch` and \
         the missing `;` on the closer"
    );
}

/// The condition-internal two-expression recovery (`if (a b)`, `while`,
/// `do-while`, `switch`): the second expression is the expected-`)` face of
/// the parenthesized condition, one row on the junk, and the statement
/// body parses on.
#[test]
fn condition_condition_shapes_match_oracle() {
    assert_eq!(
        sorted_rows("void f() { if (a b) {} }\n"),
        vec![et(17, 18)],
        "oracle: `b` is the condition's expected-`)` face"
    );
    assert_eq!(
        sorted_rows("void f() { while (x y) {} }\n"),
        vec![et(20, 21)],
        "oracle: the while condition rides the same face"
    );
    assert_eq!(
        sorted_rows("void f() { do {} while (x y); }\n"),
        vec![et(26, 27)],
        "oracle: the do-while condition rides the same face"
    );
    assert_eq!(
        sorted_rows("void f() { switch (a b) {} }\n"),
        vec![et(21, 22)],
        "oracle: the switch header condition rides the same face"
    );
}
