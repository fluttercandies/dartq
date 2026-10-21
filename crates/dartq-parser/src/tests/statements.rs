//! Statement grammar tests (#19): the full statement family — blocks,
//! if/for/for-in/while/do/switch/try, return/yield/assert/break/continue,
//! labels, local declarations, local functions — plus their recovery paths,
//! the Group A do-not-emit compliance checks (statement codes with no
//! pinned-oracle corpus surface), and the statement-level termination bombs.

use super::collector::{Collector, setup};

fn parse(source: &'static str) -> Collector {
    let mut p = setup(source);
    p.parse_unit();
    std::mem::take(p.listener)
}

// —— valid statement forms ————————————————————————————————————————————————

#[test]
fn block_bodies_parse_statements() {
    let c = parse("void f() { int x = 1; x; ; {} }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "block_function_body"),
        ("begin", "variables_declaration"),
        ("end", "variables_declaration"),
        ("handle", "expression_statement"),
        ("handle", "empty_statement"),
        ("begin", "block"),
        ("end", "block"),
        ("end", "block_function_body"),
    ]);
}

#[test]
fn local_variable_declaration_shapes() {
    let c = parse("void f() { var x = 1, y = 2; final int z; const w = 3; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "variables_declaration"),
        ("begin", "initialized_identifier"),
        ("end", "initialized_identifier"),
        ("begin", "initialized_identifier"),
        ("end", "initialized_identifier"),
        ("end", "variables_declaration"),
    ]);
}

#[test]
fn local_function_declaration() {
    let c = parse("void f() { int g<T>(int a) => a; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "local_function_declaration"),
        ("begin", "function_name"),
        ("begin", "formal_parameters"),
        ("end", "formal_parameters"),
        ("handle", "expression_function_body"),
        ("end", "local_function_declaration"),
    ]);
}

#[test]
fn local_function_block_body() {
    let c = parse("void f() { g() { return 1; } }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "local_function_declaration"),
        ("end", "local_function_declaration"),
    ]);
}

#[test]
fn dangling_member_access_consumes_next_line_identifier_not_declaration() {
    // issue_49477_prime shape (oracle error_recovery/issue_49477_prime.dart,
    // rows 7:3): the dangling dot's operand is `int` — lexically a plain
    // identifier — so the continuation consumes it SILENTLY as the member
    // name (`parseSendOrFunctionLiteral`'s continuation guard,
    // parser_impl.dart:8844-8847 — no function-literal reading after a
    // dot), the statement's missing `;` anchors on the last consumed token
    // (`int`, ensureSemicolon's findPreviousNonZeroLengthToken anchor,
    // parser_impl.dart:5106), and `getNumber() { return 42; }` still parses
    // as a local function declaration — `return 42` stays out of the
    // enclosing body's flow. The resolution rows (undefined_getter 'int',
    // unused_element) ride these events. Event variant: dartq's bare sends
    // complete through `handle_send` (the send-leaf tiling contract, #585);
    // upstream's `handleSendWithoutArguments` distinction is not observable
    // in diagnostics.
    let c = parse("void f() {\n  p.stdout.\n\n  int getNumber() { return 42; }\n}\n");
    assert_eq!(c.error_codes(), ["expected_token"], "{:?}", c.errors);
    assert_eq!(c.errors, [("expected_token".to_owned(), 26, 29)]);
    c.assert_sequence(&[
        ("handle", "send"),
        ("handle", "send"),
        ("handle", "send"),
        ("handle", "expression_statement"),
        ("begin", "local_function_declaration"),
        ("end", "local_function_declaration"),
    ]);
}

#[test]
fn local_function_missing_params_keep_expression_body() {
    // `int get g => 1;` in a function body (oracle `getter_in_function`,
    // pinned against 3.13.1): `int get` is a local variable whose `;` is
    // missing (expected_token anchored on the consumed `get`), then
    // `g => 1;` rides the local-function missing-formals recovery — the
    // synthesized `()` group is spliced and the parser resumes AFTER it
    // (`parseFormalParametersRequiredOpt` -> `insertParens`), so the `=> 1;`
    // parses as `g`'s expression body. Exactly the oracle's two rows: a
    // `missing_function_body` or a second `missing_identifier` row here
    // means the synthetic-paren drain regressed and the queued synthetic
    // `(` reached the body grammar.
    let c = parse("void f() { int get g => 1; }");
    assert_eq!(
        c.errors,
        vec![
            ("expected_token".to_string(), 15, 18),
            ("missing_function_parameters".to_string(), 21, 23),
        ],
        "oracle row set: missing `;` at `get`, missing formals at `=>`"
    );
    assert_eq!(
        c.texts,
        vec![
            "Expected to find ';'.".to_string(),
            "Functions must have an explicit list of parameters.".to_string(),
        ]
    );
    c.assert_sequence(&[
        ("begin", "local_function_declaration"),
        ("handle", "expression_function_body"),
        ("end", "local_function_declaration"),
    ]);
}

#[test]
fn if_else_chain() {
    // `else if` chains nest (official `parseIfStatement`,
    // parser_impl.dart:10129-10150: `endIfStatement` fires only after the
    // else statement parses, so the inner ifs close first). The input has
    // TWO if heads — the trailing `else { e; }` is a plain block, not a
    // third if — and that block parses inside the inner if, before both
    // ends.
    let c = parse("void f() { if (a) { b; } else if (c) { d; } else { e; } }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "if_statement"),
        ("begin", "block"),
        ("end", "block"),
        ("begin", "if_statement"),
        ("begin", "block"),
        ("end", "block"),
        ("begin", "block"),
        ("end", "block"),
        ("end", "if_statement"),
        ("end", "if_statement"),
    ]);
}

#[test]
fn if_case_pattern_guard() {
    let c = parse("void f() { if (x case int y when z) { y; } }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "if_statement"),
        ("begin", "pattern"),
        ("end", "pattern"),
        ("end", "if_statement"),
    ]);
}

#[test]
fn while_and_do_while() {
    let c = parse("void f() { while (a) { break; } do { continue; } while (b); }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "while_statement"),
        ("handle", "break_statement"),
        ("end", "while_statement"),
        ("begin", "do_while_statement"),
        ("handle", "continue_statement"),
        ("end", "do_while_statement"),
    ]);
}

#[test]
fn classic_for_loop() {
    // The update expressions stop at the reduced engine's floor: assignment
    // and increment forms are #20, so the fixture uses operator forms.
    let c = parse("void f() { for (var i = 0; i < 10; i + 1) { s; } }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("begin", "for_statement"), ("end", "for_statement")]);
}

#[test]
fn classic_for_comma_init_recovery_triple() {
    // Ticket #2418 shape: `for (a == s, c = 0; ...)` — the initializer's
    // first segment parses as an expression, then `parseForLoopPartsRest`
    // reports the missing `;` anchored on the segment's last token
    // (`ensureSemicolon` → `findPreviousNonZeroLengthToken`,
    // parser_impl.dart:5093-5112), the comma can't begin the condition
    // expression so its identifier template reports `missing_identifier`
    // and splices a synthetic name (identifier_context_impl.dart
    // `ExpressionIdentifierContext.ensureIdentifier` — the comma is on
    // the do-not-consume exemption list), and the parts stall at the
    // real `;` which is reported as unexpected before the cursor jumps
    // to the `(` group's closer (:10369-10373). Exactly the oracle's
    // three rows, in order; a fourth row of either family means the
    // recovery looped.
    let c = parse(
        "void f(int a, int c, String s) {\n  for (a == s, c = 0; c < 1; c++) {\n    c = 1;\n  }\n}\n",
    );
    assert_eq!(
        c.errors,
        vec![
            ("expected_token".to_string(), 45, 46),
            ("missing_identifier".to_string(), 46, 47),
            ("unexpected_token".to_string(), 53, 54),
        ],
        "oracle triple: `;` at `s`, identifier at the comma, unexpected `;`"
    );
    c.assert_sequence(&[("begin", "for_statement"), ("end", "for_statement")]);
}

#[test]
fn empty_for_loop() {
    let c = parse("void f() { for (;;) { break; } }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "for_statement"),
        ("handle", "break_statement"),
        ("end", "for_statement"),
    ]);
}

#[test]
fn for_in_with_type_and_bare_identifier() {
    let c = parse("void f() { for (final int x in xs) { y; } for (e in es) { z; } }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "for_statement"),
        ("end", "for_in"),
        ("begin", "for_statement"),
        ("end", "for_in"),
    ]);
}

#[test]
fn await_for_in() {
    let c = parse("f() async { await for (var x in xs) { y; } }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("begin", "for_statement"), ("end", "for_in")]);
}

#[test]
fn pattern_for_in() {
    let c = parse("void f() { for (var (a, b) in xs) { c; } }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("begin", "for_statement"), ("end", "for_in")]);
}

/// `skipOuterPattern`'s object-pattern arm (`parser_impl.dart:12333`): the
/// real-corpus idiom `for (var MapEntry(key: k, value: v) in m.entries)`
/// (package:args `usage.dart`) — bare, qualified, and generic type names
/// all reach the for-in rest, while `Foo<T>[...]`/`x.y()` stay expressions.
#[test]
fn object_pattern_for_in() {
    for (source, label) in [
        (
            "void f() { for (var MapEntry(key: k, value: v) in m.entries) { c; } }",
            "bare type name",
        ),
        (
            "void f() { for (var p.MapEntry(key: k) in m.entries) { c; } }",
            "qualified type name",
        ),
        (
            "void f() { for (var Box<int>(value: v) in xs) { c; } }",
            "generic type name",
        ),
        (
            "void f() { for (var <int>[a, b] in xs) { c; } }",
            "typed list pattern",
        ),
        (
            "void f() { for (var <String, int>{'k': v} in xs) { c; } }",
            "typed map pattern",
        ),
    ] {
        let c = parse(source);
        assert!(c.errors.is_empty(), "{label}: {:?}", c.errors);
        c.assert_sequence(&[("begin", "for_statement"), ("end", "for_in")]);
    }
    // A typed list literal is NOT a pattern — but `for (<expr> in xs)` is
    // not legal Dart either, so the disambiguation never sees this shape.
    // The statement form stays an expression:
    let c = parse("void f() { var x = <int>[1]; var y = Foo<int>(1); }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
}

#[test]
fn pattern_variable_declaration_statement() {
    let c = parse("void f() { final (a, b) = (1, 2); }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "pattern"),
        ("end", "pattern"),
        ("handle", "pattern_variable_declaration_statement"),
    ]);
}

#[test]
fn try_catch_on_finally() {
    let c = parse(
        "void f() { try { a; } on E catch (e, s) { b; } on F { c; } catch (e) { d; } finally { g; } }",
    );
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "try_statement"),
        ("begin", "catch_clause"),
        ("end", "catch_clause"),
        ("begin", "catch_clause"),
        ("end", "catch_clause"),
        ("begin", "catch_clause"),
        ("end", "catch_clause"),
        ("end", "try_statement"),
    ]);
}

#[test]
fn nested_try_inside_catch_body() {
    // #991 regression anchor: the well-formed input that tripped a pairing
    // panic in a deferred-seal listener. The event contract the listener
    // must implement: `end_catch_clause` closes only the parameters, the
    // catch body parses between it and `handle_catch_block`, and every
    // nested statement's events interleave strictly inside that window —
    // so each `handle_catch_block` lands while its own CatchClause is the
    // innermost open node, innermost clause first.
    let c = parse("void f() { try {} catch (e) { try {} catch (e2) { rethrow; } } }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "try_statement"),
        ("begin", "catch_clause"),
        ("end", "catch_clause"),
        ("begin", "block"),
        ("begin", "try_statement"),
        ("begin", "catch_clause"),
        ("end", "catch_clause"),
        ("begin", "block"),
        ("begin", "rethrow_statement"),
        ("end", "rethrow_statement"),
        ("end", "block"),
        ("handle", "catch_block"),
        ("end", "try_statement"),
        ("end", "block"),
        ("handle", "catch_block"),
        ("end", "try_statement"),
    ]);
}

#[test]
fn triple_nested_try_inside_catch_bodies() {
    // Depth 3 of the #991 shape: each level's `handle_catch_block` seals
    // its own clause before the enclosing level's, with the events of the
    // innermost statement fully contained by every enclosing window.
    let c = parse(
        "void f() { try {} catch (a) { try {} catch (b) { try {} catch (c) { rethrow; } } } }",
    );
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "try_statement"),
        ("begin", "catch_clause"),
        ("end", "catch_clause"),
        ("begin", "block"),
        ("begin", "try_statement"),
        ("begin", "catch_clause"),
        ("end", "catch_clause"),
        ("begin", "block"),
        ("begin", "try_statement"),
        ("begin", "catch_clause"),
        ("end", "catch_clause"),
        ("begin", "block"),
        ("begin", "rethrow_statement"),
        ("end", "rethrow_statement"),
        ("end", "block"),
        ("handle", "catch_block"),
        ("end", "try_statement"),
        ("handle", "catch_block"),
        ("end", "try_statement"),
        ("end", "block"),
        ("handle", "catch_block"),
        ("end", "try_statement"),
    ]);
}

#[test]
fn nested_catch_with_stack_trace_parameter() {
    // The `catch (e, s)` form at both nesting levels: the comma-bearing
    // parameter list must not disturb the clause window or the rethrow
    // inside the inner body.
    let c = parse("void f() { try {} catch (e, s) { try {} catch (e2, s2) { rethrow; } } }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "try_statement"),
        ("begin", "catch_clause"),
        ("end", "catch_clause"),
        ("begin", "block"),
        ("begin", "try_statement"),
        ("begin", "catch_clause"),
        ("end", "catch_clause"),
        ("begin", "block"),
        ("begin", "rethrow_statement"),
        ("end", "rethrow_statement"),
        ("end", "block"),
        ("handle", "catch_block"),
        ("end", "try_statement"),
        ("end", "block"),
        ("handle", "catch_block"),
        ("end", "try_statement"),
    ]);
}

#[test]
fn nested_try_inside_finally_body() {
    // Same-shape risk probe: a try statement nested in the finally body.
    // The nested statement completes entirely inside the finally block's
    // events; `handle_finally_block` fires after that block parses and
    // before the enclosing statement seals.
    let c = parse("void f() { try {} finally { try {} catch (e) {} } }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "try_statement"),
        ("begin", "block"),
        ("begin", "try_statement"),
        ("begin", "catch_clause"),
        ("end", "catch_clause"),
        ("end", "block"),
        ("handle", "catch_block"),
        ("end", "try_statement"),
        ("end", "block"),
        ("handle", "finally_block"),
        ("end", "try_statement"),
    ]);
}

#[test]
fn rethrow_inside_nested_catch_parses_clean() {
    // W2 rethrow fence, legal side: `rethrow` inside a (nested) catch body
    // builds the statement node with zero diagnostics at the parser layer.
    let c = parse("void f() { try {} catch (e) { try {} catch (e2) { rethrow; } } }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "catch_clause"),
        ("end", "catch_clause"),
        ("begin", "block"),
        ("begin", "rethrow_statement"),
        ("end", "rethrow_statement"),
        ("end", "block"),
        ("handle", "catch_block"),
    ]);
}

#[test]
fn rethrow_outside_catch_is_not_a_parser_error() {
    // W2 rethrow fence, boundary side: the catch-only restriction is a
    // resolver (semantic) check — the official `parseRethrowStatement`
    // carries no clause context, so the parser builds the node and reports
    // nothing here either.
    let c = parse("void f() { rethrow; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("begin", "rethrow_statement"), ("end", "rethrow_statement")]);
}

#[test]
fn switch_statement_with_pattern_cases() {
    let c = parse("void f() { switch (o) { case 1: a; case int x when x > 0: b; default: c; } }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "switch_statement"),
        ("begin", "switch_block"),
        ("begin", "case_expression"),
        ("end", "case_expression"),
        ("begin", "switch_case"),
        ("end", "switch_case"),
        ("begin", "case_expression"),
        ("end", "case_expression"),
        ("begin", "switch_case"),
        ("end", "switch_case"),
        ("begin", "switch_case"),
        ("end", "switch_case"),
        ("end", "switch_block"),
        ("end", "switch_statement"),
    ]);
}

#[test]
fn switch_labels_before_case() {
    let c = parse("void f() { switch (o) { l1: l2: case 1: break; } }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "switch_statement"),
        ("begin", "case_expression"),
        ("end", "case_expression"),
        ("handle", "break_statement"),
        ("end", "switch_statement"),
    ]);
}

#[test]
fn labeled_statement_and_break_target() {
    let c = parse("void f() { outer: inner: for (;;) { break outer; } }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "labeled_statement"),
        ("begin", "for_statement"),
        ("handle", "break_statement"),
        ("end", "for_statement"),
        ("end", "labeled_statement"),
    ]);
}

#[test]
fn return_and_yield_forms() {
    let c = parse("f() sync* { yield 1; yield* g(); return; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "yield_statement"),
        ("end", "yield_statement"),
        ("begin", "yield_statement"),
        ("end", "yield_statement"),
        ("begin", "return_statement"),
        ("end", "return_statement"),
    ]);
}

#[test]
fn yield_outside_generator_is_invalid_yield_event() {
    // `yieldNotGenerator` is a resolution-domain pseudo code: the event
    // stream marks the invalid yield, and no TSV code is emitted. (The
    // official lookahead only recognizes `yield <expression>` here —
    // `yield 1;` parses as a `yield`-typed variable declaration.)
    let c = parse("void f() { yield g(); }");
    assert!(
        c.error_codes()
            .iter()
            .all(|code| code != "yield_not_generator"),
        "no dedicated pseudo code may surface: {:?}",
        c.errors
    );
    c.assert_sequence(&[("begin", "yield_statement"), ("end", "yield_statement")]);
}

#[test]
fn assert_statement_forms() {
    let c = parse("void f() { assert(a); assert(b, 'm'); assert(c, 'm',); }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "assert"),
        ("end", "assert"),
        ("begin", "assert"),
        ("end", "assert"),
        ("begin", "assert"),
        ("end", "assert"),
    ]);
}

#[test]
fn continue_in_switch_without_label_reports() {
    let c = parse("void f() { switch (x) { case 1: continue; } }");
    assert_eq!(
        c.error_codes(),
        vec!["continue_without_label_in_case".to_owned()]
    );
}

#[test]
fn break_and_continue_outside_loops_report() {
    let c = parse("void f() { break; continue; }");
    assert_eq!(
        c.error_codes(),
        vec![
            "break_outside_of_loop".to_owned(),
            "continue_outside_of_loop".to_owned(),
        ]
    );
}

#[test]
fn await_for_outside_async_is_event_only() {
    // `awaitForNotAsync` surfaces through the resolver (pseudo-shared
    // `asyncForInWrongContext`): the parser emits no TSV code for it.
    let c = parse("void f() { await for (x in xs) { y; } }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("begin", "for_statement"), ("end", "for_in")]);
}

// —— recovery paths ————————————————————————————————————————————————————————

#[test]
fn expression_statement_recovery_is_bounded() {
    // Official `{ + }` face: parseUnaryExpression rewrites the prefix `+`
    // away via rewriteAndRecover (parser_impl.dart:7904-7912) — reporting
    // unsupportedPrefixPlus (pseudo-shared with missing_identifier,
    // diagnostic.g.dart:3008-3013) and INSERTING the synthetic identifier
    // before it, so the `+` itself stays in the stream (:5112-5114) and
    // reaches the binary loop. Its RHS at `}` then reports its own
    // missing_identifier (expression-identifier template), and the
    // statement's missing `;` reports expected_but_got at the `}`. Three
    // rows; the recovery stays bounded.
    let c = parse("void f() { + }");
    assert_eq!(
        c.error_codes(),
        ["missing_identifier", "missing_identifier", "expected_token"]
    );
    c.assert_sequence(&[
        ("begin", "block_function_body"),
        ("end", "block_function_body"),
    ]);
}

#[test]
fn missing_if_parentheses_recover() {
    let c = parse("void f() { if x y; }");
    assert!(
        c.error_codes()
            .iter()
            .all(|code| code == "expected_token" || code == "missing_identifier"),
        "generic recovery family only: {:?}",
        c.errors
    );
    c.assert_sequence(&[("begin", "if_statement"), ("end", "if_statement")]);
}

#[test]
fn try_without_clauses_reports() {
    let c = parse("void f() { try {} }");
    assert_eq!(c.error_codes(), vec!["missing_catch_or_finally".to_owned()]);
}

#[test]
fn catch_missing_parens_report() {
    let c = parse("void f() { try {} catch {} }");
    assert!(
        c.error_codes().contains(&"catch_syntax".to_owned()),
        "{:?}",
        c.errors
    );
    c.assert_sequence(&[
        ("begin", "try_statement"),
        ("begin", "catch_clause"),
        ("end", "catch_clause"),
        ("end", "try_statement"),
    ]);
}

#[test]
fn catch_extra_parameters_report() {
    let c = parse("void f() { try {} catch (e, s, x) {} }");
    assert!(
        c.error_codes()
            .contains(&"catch_syntax_extra_parameters".to_owned()),
        "{:?}",
        c.errors
    );
}

#[test]
fn catch_reserved_statement_start_parks_a_synthetic_name() {
    // `catch (assert)`: the reserved statement start parks behind a synthetic
    // name in the catch scan (CatchParameterIdentifierContext.ensureIdentifier,
    // identifier_context_impl.dart:36-38 — one `catchSyntax` report, no second
    // row there). The formal re-parse's name slot then reads the stray token:
    // `assert` is a statement start whose follower `)` is an ok-next value in
    // a formal parameter, so the template's synthetic arm does not apply and
    // the keyword-consume arm reports `expectedIdentifierButGotKeyword` while
    // taking the word as the name (FormalParameterDeclarationIdentifierContext
    // .ensureIdentifier, identifier_context_impl.dart:604-621). Both rows
    // anchor on `assert`; the closer stays clean and the extra-parameters
    // report stays silent (the trace name is synthetic).
    let c = parse("void f() { try {} catch (assert) {} }");
    let codes = c.error_codes();
    assert_eq!(
        codes,
        ["catch_syntax", "expected_identifier_but_got_keyword"],
        "catchSyntax at `assert` + the re-parse keyword-name face: {:?}",
        c.errors
    );
    assert!(
        !codes.contains(&"catch_syntax_extra_parameters".to_owned()),
        "{:?}",
        c.errors
    );
}

#[test]
fn catch_reserved_word_becomes_the_name() {
    // `catch (super)`: `super` is reserved but not a statement start, so the
    // official template falls through to `return identifier`
    // (identifier_context_impl.dart:44) — the token itself is the name after
    // one `catchSyntax` report; the synthetic trace pass never runs (the
    // next token is `)`).
    let c = parse("void f() { try {} catch (super) {} }");
    let codes = c.error_codes();
    assert_eq!(
        codes.iter().filter(|c| *c == "catch_syntax").count(),
        1,
        "exactly one catch_syntax: {:?}",
        c.errors
    );
    assert!(
        !codes.contains(&"catch_syntax_extra_parameters".to_owned()),
        "{:?}",
        c.errors
    );
}

#[test]
fn switch_garbage_reports_expected_case() {
    let c = parse("void f() { switch (x) { + } }");
    let codes = c.error_codes();
    assert!(
        codes.contains(&"expected_token".to_owned()),
        "{:?}",
        c.errors
    );
    assert!(
        codes.iter().all(|code| code != "expected_case_or_default"),
        "the dedicated Group A code is oracle-dead: {codes:?}"
    );
    c.assert_sequence(&[("begin", "switch_block"), ("end", "switch_block")]);
}

#[test]
fn switch_multiple_defaults_report() {
    let c = parse("void f() { switch (x) { default: a; default: b; } }");
    assert_eq!(
        c.error_codes(),
        vec!["switch_has_multiple_default_cases".to_owned()]
    );
}

#[test]
fn switch_case_after_default_report() {
    let c = parse("void f() { switch (x) { default: a; case 1: b; } }");
    assert_eq!(
        c.error_codes(),
        vec!["switch_has_case_after_default_case".to_owned()]
    );
}

#[test]
fn for_in_qualified_head_reports_unexpected_token() {
    // `for (a.b in xs)`: the head consumed more than the loop variable.
    let c = parse("void f() { for (a.b in xs) { c; } }");
    let codes = c.error_codes();
    assert!(
        codes.contains(&"unexpected_token".to_owned()),
        "{:?}",
        c.errors
    );
    assert!(
        codes
            .iter()
            .all(|code| code != "missing_variable_in_for_each"),
        "the dedicated Group A code is oracle-dead: {codes:?}"
    );
}

#[test]
fn missing_type_before_type_arguments_in_local_decl() {
    let c = parse("void f() { final <int> x = 1; }");
    assert!(
        c.error_codes().contains(&"missing_identifier".to_owned()),
        "expectedIdentifier translates to missing_identifier: {:?}",
        c.errors
    );
}

// —— Group A do-not-emit compliance ——————————————

/// Each Group A statement situation emits only the pinned oracle's generic
/// codes — never the oracle-dead dedicated code.
#[test]
fn group_a_dead_codes_never_emitted() {
    let cases: &[(&'static str, &'static str)] = &[
        // getter_in_function
        ("void main() { get x => 1; }", "getter_in_function"),
        // setter_in_function
        ("void main() { set x(v) {} }", "setter_in_function"),
        // invalid_star_after_async
        ("f() async* => 1;", "invalid_star_after_async"),
        // invalid_sync
        ("f() sync => 1;", "invalid_sync"),
        // local_function_declaration_modifier
        (
            "void main() { static f() {} }",
            "local_function_declaration_modifier",
        ),
        // missing_variable_in_for_each
        (
            "void f() { for (in xs) {} }",
            "missing_variable_in_for_each",
        ),
        // multiple_variables_in_for_each
        (
            "void f() { for (int i, j in xs) {} }",
            "multiple_variables_in_for_each",
        ),
        // expected_case_or_default
        ("void f() { switch (x) { + } }", "expected_case_or_default"),
    ];
    for (source, dead_code) in cases {
        let c = parse(source);
        assert!(
            c.error_codes().iter().all(|code| code != dead_code),
            "{dead_code} must never be emitted for {source}: {:?}",
            c.errors
        );
    }
}

#[test]
fn local_function_modifier_reports_extraneous() {
    let c = parse("void main() { static f() {} }");
    assert_eq!(c.error_codes(), vec!["extraneous_modifier".to_owned()]);
}

// —— termination bombs ——————————————————————————————————————————————————————

#[test]
fn deep_nested_blocks_latch_stack_overflow() {
    let source: &'static str = Box::leak(
        format!("void f() {{ {}{} }}", "{".repeat(600), "}".repeat(600)).into_boxed_str(),
    );
    let mut p = setup(source);
    p.parse_unit();
    assert!(p.is_bailed(), "the nesting cap must latch the bailout");
    let codes = p.listener.error_codes();
    assert!(
        codes.contains(&"stack_overflow".to_owned()),
        "the terminal diagnostic is stackOverflow: {codes:?}"
    );
    assert!(
        codes.len() <= 4,
        "the bailout bounds total diagnostics: {codes:?}"
    );
}

#[test]
fn statement_flood_respects_diagnostics_cap() {
    // Each `.` in statement position produces two diagnostics and exactly
    // one token of progress, so 15k dots blow far past the cap.
    let source: &'static str =
        Box::leak(format!("void f() {{ {} }}", ". ".repeat(15_000)).into_boxed_str());
    let (table, text) = super::collector::scan_ok(source);
    let mut listener = super::collector::Collector::default();
    {
        let mut parser = crate::Parser::new(
            text,
            table,
            super::collector::latest_features(),
            &mut listener,
        );
        parser.parse_unit();
        assert!(parser.is_bailed(), "the cap must latch");
    }
    assert_eq!(
        listener.errors.len(),
        crate::parser::MAX_DIAGNOSTICS,
        "the listener sees exactly the capped count"
    );
}

// —— expression-statement missing-`;` face —————————————————————————————————

#[test]
fn expression_statement_missing_semicolon_matches_oracle() {
    // 3.13.1 oracle probe (`dart analyze --format=machine` on
    // `void f() { x }`): `EXPECTED_TOKEN` "Expected to find ';'." spanning
    // the `x` itself — offset 11, length 1. That is the general
    // `ensureSemicolon` anchor (`findPreviousNonZeroLengthToken`,
    // parser_impl.dart:5106/:11400), not a fixture special case.
    let c = parse("void f() { x }");
    assert_eq!(c.error_codes(), ["expected_token"]);
    assert_eq!(c.errors[0].1, 11);
    assert_eq!(c.errors[0].2, 12);
    assert_eq!(c.texts[0], "Expected to find ';'.");
}

// —— statement-position as/is rescue ————————————————————————————————————

#[test]
fn statement_as_is_rescue_keeps_the_leading_operand() {
    // Official gate (`parser_impl.dart:10026-10046`): `beforeType ==
    // start` + built-in `as`/`is` with a follower that is not `=`/`;`/`,`
    // re-parses from the statement's first token
    // (`parseExpressionStatement(start)`), so `x as int;` is a full
    // `x as int` expression statement — the operand tentatively consumed
    // as the type is re-read, not dropped (a dropped read surfaces
    // downstream as an unused_local_variable false positive plus
    // undefined_identifier false positives).
    let c = parse("void f() { x as int; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("handle", "expression_statement")]);

    let c = parse("void f() { x is int; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("handle", "expression_statement")]);
}

// —— audit #180: ensure_semicolon synthetic + as/is rescue ————————————————

#[test]
fn ensure_semicolon_synthetic_is_zero_width_anchored_on_last_consumed() {
    // F1: the inserted `;` is a zero-width synthetic — recovery artifacts
    // never own real width (closer-split remainders are the width-keeping
    // case). F4: the report anchors on the last consumed token
    // (`findPreviousNonZeroLengthToken`, parser_impl.dart:5106/:11400) —
    // here the `++` at offsets 2..4.
    let mut p = setup("x ++");
    p.cursor.advance(); // `x`
    p.cursor.advance(); // `++`
    let semi = p.ensure_semicolon();
    assert_eq!(semi.range.length, 0);
    assert_eq!(p.listener.errors[0], ("expected_token".to_string(), 2, 4));
}

#[test]
fn ensure_identifier_leaves_separators_for_the_grammar() {
    // F2 (identifier_context_impl.dart:581-618, formalParameterDeclaration):
    // a `,` in name position takes the insert-without-consuming branch and
    // the synthetic is drained to, so the parameter list resumes at the
    // separator. The official composition reports the run of empty
    // parameters ONCE: the first `int` reads as param 1's whole parameter
    // (computeType with inDeclaration: false returns noType for `int,` —
    // type_info.dart:376-384 — and the provisional name-take at
    // parser_impl.dart:2228 survives the parseType re-derive at :2352), so
    // only the genuinely empty param 2 reports at the second comma, then
    // the loop consumes it and `int b` parses clean. Oracle 3.13.1
    // (`dart analyze --format=machine`): exactly one row,
    // MISSING_IDENTIFIER at 1|12|1, text "Expected an identifier.".
    let c = parse("void f(int,, int b) {}");
    assert_eq!(c.error_codes(), ["missing_identifier"]);
    assert_eq!((c.errors[0].1, c.errors[0].2), (11, 12), "{:?}", c.errors);
    assert_eq!(c.texts, ["Expected an identifier."]);
}

// —— #2093: assert-as-expression receiver reference ————————————————————————

/// Listener capturing the `assert` reference face: identifier offsets +
/// contexts and whether any `handle_send` fired (the assert receiver must
/// stay a bare-name read — an invoked intent mints the wrong code).
#[derive(Default)]
struct AssertRefLog {
    identifiers: Vec<(u32, crate::listener::IdentifierContext)>,
    sends: usize,
}

impl crate::listener::ParserListener for AssertRefLog {
    fn handle_identifier(
        &mut self,
        token: crate::listener::Token,
        context: crate::listener::IdentifierContext,
    ) {
        self.identifiers.push((token.range.offset, context));
    }
    fn handle_send(&mut self, _: crate::listener::Token, _: crate::listener::Token) {
        self.sends += 1;
    }
}

#[test]
fn assert_expression_emits_the_keyword_as_a_name_read() {
    // Upstream `endAssert` (upstream Dart SDK source, pkg/analyzer/lib/src/
    // fasta/ast_builder.dart:1083-1096) fabricates a `CallInvocation` whose
    // receiver names the `assert` keyword; `_resolveUnqualifiedNameRead`
    // then surfaces `undefined_identifier 'assert'` (oracle
    // const_patterns.dart 47/68 — `case assert(false):`, `case const
    // assert(false):`). The parser-side fact is the identifier event; no
    // send rides it.
    use crate::listener::IdentifierContext;
    let mut p = crate::expr_const::test_support::parser("assert(false)", AssertRefLog::default());
    let _ = p.parse_expression();
    assert_eq!(
        p.listener.identifiers,
        vec![(0, IdentifierContext::Expression)]
    );
    assert_eq!(p.listener.sends, 0);
}

#[test]
fn assert_expression_in_constant_pattern_keeps_the_reference() {
    // Same fabricated name inside `case assert(false):` — the
    // constant-pattern lane's `assert` arm routes through `parse_assert(
    // AssertKind::Expression)`, so the reference event fires there too.
    use crate::listener::{IdentifierContext, PatternContext};
    let mut p = crate::expr_const::test_support::parser("assert(false)", AssertRefLog::default());
    let _ = p.parse_pattern_entry(PatternContext::Matching);
    assert_eq!(
        p.listener.identifiers,
        vec![(0, IdentifierContext::Expression)]
    );
    assert_eq!(p.listener.sends, 0);
}
