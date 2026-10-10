//! Statement grammar tests (#19): the full statement family — blocks,
//! if/for/for-in/while/do/switch/try, return/yield/assert/break/continue,
//! labels, local declarations, local functions — plus their recovery paths,
//! the Group A do-not-emit compliance checks (docs/facts/
//! corpus-expansion-plan.md §7), and the statement-level termination bombs.

use super::collector::{Collector, setup};

fn parse(source: &'static str) -> Collector {
    let mut p = setup(source);
    p.parse_unit();
    p.listener
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
    // away (rewriteAndRecover, parser_impl.dart:7904-7912), reporting
    // unsupportedPrefixPlus — pseudo-shared with missing_identifier
    // (diagnostic.g.dart:3008-3013) — and the statement's missing `;`
    // reports expected_but_got at the `}`. Exactly two diagnostics; the
    // recovery stays bounded.
    let c = parse("void f() { + }");
    assert_eq!(
        c.error_codes(),
        ["missing_identifier", "expected_token"]
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

// —— Group A do-not-emit compliance (corpus-expansion-plan §7) ——————————————

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
        let mut parser =
            crate::Parser::new(text, table, super::collector::latest_features(), listener);
        parser.parse_unit();
        assert!(parser.is_bailed(), "the cap must latch");
        listener = parser.listener;
    }
    assert_eq!(
        listener.errors.len(),
        crate::parser::MAX_DIAGNOSTICS,
        "the listener sees exactly the capped count"
    );
}
