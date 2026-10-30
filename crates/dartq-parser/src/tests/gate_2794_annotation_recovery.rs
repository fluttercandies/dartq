//! #2794 wave4 — the annotation-argument `(` recovery sub-face over
//! statement keywords, plus the `parseAssert` function-expression gate.
//!
//! Face 1 — `f() { @a( assert (true); }` (the oracle's
//! partial_code/annotation `annotation_local_leftParen_assert` row in
//! the extracted fasta-recovery corpus): upstream's arg list parses
//! `assert (true)` through `parseAssert(Assert.Expression)`
//! (`parser_impl.dart:8165` — the `assert` keyword arm of `parsePrimary`),
//! consuming the real `(` `)` pair as one positional argument; the
//! unmatched `@a(` `(` then recovers `expectedToken ')'` at `;`, the
//! local-variable name behind the metadata is missing (`missingIdentifier`
//! at `;` — upstream's `token != start` after the metadata walk lands the
//! tail in the variable-declaration lane, :10021 `if (token == start)`),
//! and the fabricated `assert` name carries the
//! `undefined_identifier 'assert'` reference (ast_builder `endAssert`'s
//! `CallInvocation`, ast_builder.dart:995-1017). This gate locks the
//! parser-visible rows and the `assert` keyword's `Expression`-context
//! identifier event.
//!
//! Face 2 — `parseAssert` opens the condition/message parse with
//! `mayParseFunctionExpressions = true` unconditionally
//! (parser_impl.dart:11067-11068, restored :11098). dartq dropped the
//! save/set/restore, so `assert((x) => x > 0)` in a constructor
//! initializer — the one position that holds
//! `may_parse_function_expressions == false` (official :4755) — misparsed
//! the closure as a parenthesized expression and spilled `=> …` into
//! recovery; a `may_parse_function_expressions == false` switch-expression
//! case body (:12405) diverged the same way. The restore lands after the
//! close-paren resolution, before the statement `;`/endAssert tail,
//! matching upstream's order.

use super::collector::{Collector, setup};

/// Count of a diagnostic code.
fn code_count(c: &Collector, code: &str) -> usize {
    c.errors.iter().filter(|(k, _, _)| k == code).count()
}

/// Whether a diagnostic code fired at `target`'s offset inside `needle`.
fn reported_at(c: &Collector, source: &str, needle: &str, target: &str, code: &str) -> bool {
    let at = source.find(needle).unwrap_or_else(|| {
        panic!("needle {needle:?} absent from source");
    }) + needle
        .find(target)
        .unwrap_or_else(|| panic!("target {target:?} absent from needle {needle:?}"));
    let at = at as u32;
    c.errors.iter().any(|(k, s, _)| k == code && *s == at)
}

/// Whether an identifier event fired for `target` inside `needle`'s span
/// in `context`.
fn named_in(c: &Collector, source: &str, needle: &str, target: &str, context: &str) -> bool {
    let at = source.find(needle).unwrap_or_else(|| {
        panic!("needle {needle:?} absent from source");
    }) + needle
        .find(target)
        .unwrap_or_else(|| panic!("target {target:?} absent from needle {needle:?}"));
    let (start, end) = (at as u32, (at + target.len()) as u32);
    c.idents
        .iter()
        .any(|(k, s, e, _)| k == context && *s == start && *e == end)
}

/// `f() { @a( assert (true); }` — the annotation's `(` swallows
/// `assert (true)` as one assert-expression positional argument
/// (`begin_assert`/`end_assert` pair, the `assert` keyword's identifier
/// event in `Expression` context — the reference the analyzer reports as
/// `undefined_identifier 'assert'`), then `;` takes the unmatched-`(`
/// `expectedToken ')'` recovery and the local variable's missing name
/// (`missing_identifier` at `;`, the oracle's two col-24 rows).
#[test]
fn annotation_arg_assert_expression_recovery() {
    let source = "f() { @a( assert (true); }";
    let mut p = setup(source);
    p.parse_unit();
    let c = &p.listener;
    // The assert-expression event pair fired — `assert (true)` parsed as
    // ONE argument's assert expression, consuming `(true)`.
    c.assert_sequence(&[("begin", "assert"), ("end", "assert")]);
    // The `assert` keyword carries the unqualified-read reference — the
    // `CallInvocation`'s fabricated `assert` name (ast_builder
    // `endAssert`), which the oracle reports as
    // `undefined_identifier 'assert'`.
    assert!(named_in(c, source, "@a( assert", "assert", "Expression"));
    // The unmatched `@a(` `(` reports `expectedToken ')'` at `;`.
    assert!(reported_at(c, source, "(true);", ";", "expected_token"));
    assert_eq!(code_count(c, "expected_token"), 1, "{:?}", c.errors);
    // The local variable behind the metadata is nameless at `;` — the
    // variable-declaration lane's `ensureIdentifier` row.
    assert!(reported_at(c, source, "(true);", ";", "missing_identifier"));
    assert_eq!(code_count(c, "missing_identifier"), 1, "{:?}", c.errors);
}

/// `class C { C() : assert((x) => x > 0); }` — the constructor
/// initializer's `may_parse_function_expressions == false` posture
/// (parser_impl.dart:4755) does NOT reach inside `parseAssert`: the
/// condition parses with function expressions enabled
/// (:11067-11068), so `(x) => x > 0` reads as a closure — one
/// `expression_function_body`, zero recovery rows.
#[test]
fn assert_in_initializer_parses_closure_condition() {
    let source = "class C { C() : assert((x) => x > 0); }";
    let mut p = setup(source);
    p.parse_unit();
    let c = &p.listener;
    c.assert_sequence(&[
        ("begin", "assert"),
        ("handle", "expression_function_body"),
        ("end", "assert"),
    ]);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
}

/// `var y = switch (x) { assert((v) => v > 0) => 1 };` — the
/// switch-expression case PATTERN's
/// `may_parse_function_expressions == false` posture
/// (parser_impl.dart:12405) likewise does not reach `parseAssert`'s
/// condition: `assert` in constant-pattern position parses as
/// `Assert.Expression` (the `parsePrimary` 'assert' arm :8165, entered
/// through the pattern operand's expression parse) and its closure still
/// parses.
#[test]
fn assert_expression_in_switch_case_pattern_parses_closure_condition() {
    let source = "var y = switch (x) { assert((v) => v > 0) => 1 };";
    let mut p = setup(source);
    p.parse_unit();
    let c = &p.listener;
    c.assert_sequence(&[
        ("begin", "assert"),
        ("handle", "expression_function_body"),
        ("end", "assert"),
    ]);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
}

/// The statement form is untouched: `assert((x) => x > 0);` in a block
/// parses the same closure — `may_parse_function_expressions` was
/// already true there, the save/restore is a no-op (the flag's default
/// posture, parser_impl.dart:277).
#[test]
fn assert_statement_closure_condition_unchanged() {
    let source = "f() { assert((x) => x > 0); }";
    let mut p = setup(source);
    p.parse_unit();
    let c = &p.listener;
    c.assert_sequence(&[
        ("begin", "assert"),
        ("handle", "expression_function_body"),
        ("end", "assert"),
    ]);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
}

/// `f() { @a( break; }` — the statement-keyword sibling of the assert
/// face: `break` cannot start an expression, so the arg's identifier
/// recovery splices a synthetic name BEFORE the keyword
/// (`missing_identifier` at `break`, the `Expression` context's
/// statement-start branch keeps the keyword unconsumed), the unmatched
/// `(` reports `expectedToken ')'` at `break`, and `break;` then parses
/// as a statement (the oracle's `break_outside_of_loop` shape —
/// parser-visible as the `break_statement` event).
#[test]
fn annotation_arg_statement_keyword_recovery() {
    let source = "f() { @a( break; }";
    let mut p = setup(source);
    p.parse_unit();
    let c = &p.listener;
    assert!(reported_at(c, source, "@a( break", "break", "missing_identifier"));
    assert!(reported_at(c, source, "@a( break", "break", "expected_token"));
    c.assert_sequence(&[("handle", "break_statement")]);
}
