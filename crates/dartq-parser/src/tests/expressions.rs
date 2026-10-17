//! Expression engine tests (#20): precedence/associativity shapes,
//! assignment/conditional/cascade/function-literal/switch-expression forms,
//! collection control-flow elements through statements, the Group B
//! do-not-emit riders (codes with no pinned-oracle corpus surface), recovery
//! shapes, and the expression termination bombs.

use dartq_scanner::TokenKind;

use crate::listener::{
    AssertKind, ConstructorReferenceContext, ExperimentalFlag, IdentifierContext, ParserListener,
};
use crate::message::Message;
use crate::parser::Parser;
use crate::token_id::TokenSpan;

/// Event + error log over the expression-family surface.
#[derive(Default)]
struct ExprLog {
    errors: Vec<String>,
    log: Vec<(&'static str, &'static str)>,
    /// Send-leaf tiled spans `(begin.offset, end.offset + end.length)` in
    /// UTF-16 — the tests/collector.rs recording convention (#585).
    sends: Vec<(u32, u32)>,
    /// Error rows with their anchor spans `(code, offset, length)` — the
    /// family-pin surface (#1317).
    error_rows: Vec<(String, u32, u32)>,
    /// Dot accesses `(is_null_aware, dot-token offset)`.
    dots: Vec<(bool, u32)>,
    /// Index accesses `(null-aware question present, `[`-token offset)`.
    indexes: Vec<(bool, u32)>,
    /// Postfix `!` null-assert token offsets.
    null_asserts: Vec<u32>,
}

impl ExprLog {
    fn codes(&self) -> &[String] {
        &self.errors
    }

    /// Subsequence check over the log.
    fn assert_sequence(&self, expected: &[(&'static str, &'static str)]) {
        let mut it = self.log.iter();
        for want in expected {
            assert!(
                it.any(|got| got == want),
                "expected {want:?} in order; log so far: {:?} (errors: {:?})",
                self.log,
                self.errors
            );
        }
    }

    fn count(&self, phase: &str, name: &str) -> usize {
        self.log
            .iter()
            .filter(|(p, n)| *p == phase && *n == name)
            .count()
    }

    /// The error rows as `(code, offset, length)` for span pins.
    fn error_rows(&self) -> Vec<(&str, u32, u32)> {
        self.error_rows
            .iter()
            .map(|(code, offset, length)| (code.as_str(), *offset, *length))
            .collect()
    }
}

impl ParserListener for ExprLog {
    fn handle_recoverable_error(&mut self, message: Message, start: TokenSpan, _: TokenSpan) {
        self.errors.push(message.code.name.to_owned());
        self.error_rows.push((
            message.code.name.to_owned(),
            start.range.offset,
            start.range.length,
        ));
    }

    fn handle_experiment_not_enabled(&mut self, _: ExperimentalFlag, _: TokenSpan, _: TokenSpan) {
        self.errors.push("experiment_not_enabled".to_owned());
    }

    fn handle_assignment_expression(&mut self, _: TokenSpan, _: TokenSpan) {
        self.log.push(("handle", "assignment_expression"));
    }
    fn begin_cascade(&mut self, _: TokenSpan) {
        self.log.push(("begin", "cascade"));
    }
    fn end_cascade(&mut self) {
        self.log.push(("end", "cascade"));
    }
    fn handle_cascade_access(&mut self, _: TokenSpan, _: TokenSpan, _: bool) {
        self.log.push(("handle", "cascade_access"));
    }
    fn begin_conditional_expression(&mut self, _: TokenSpan) {
        self.log.push(("begin", "conditional_expression"));
    }
    fn end_conditional_expression(&mut self, _: TokenSpan, _: TokenSpan, _: TokenSpan) {
        self.log.push(("end", "conditional_expression"));
    }
    fn begin_function_expression(&mut self, _: TokenSpan) {
        self.log.push(("begin", "function_expression"));
    }
    fn end_function_expression(&mut self, _: TokenSpan, _: TokenSpan) {
        self.log.push(("end", "function_expression"));
    }
    fn begin_named_function_expression(&mut self, _: TokenSpan) {
        self.log.push(("begin", "named_function_expression"));
    }
    fn end_named_function_expression(&mut self, _: TokenSpan) {
        self.log.push(("end", "named_function_expression"));
    }
    fn begin_switch_expression(&mut self, _: TokenSpan) {
        self.log.push(("begin", "switch_expression"));
    }
    fn end_switch_expression(&mut self, _: TokenSpan, _: TokenSpan) {
        self.log.push(("end", "switch_expression"));
    }
    fn begin_switch_expression_case(&mut self) {
        self.log.push(("begin", "switch_expression_case"));
    }
    fn end_switch_expression_case(
        &mut self,
        _: TokenSpan,
        _: Option<TokenSpan>,
        _: TokenSpan,
        _: TokenSpan,
    ) {
        self.log.push(("end", "switch_expression_case"));
    }
    fn begin_if_control_flow(&mut self, _: TokenSpan) {
        self.log.push(("begin", "if_control_flow"));
    }
    fn end_if_control_flow(&mut self, _: TokenSpan) {
        self.log.push(("end", "if_control_flow"));
    }
    fn end_if_else_control_flow(&mut self, _: TokenSpan) {
        self.log.push(("end", "if_else_control_flow"));
    }
    fn begin_for_control_flow(&mut self, _: Option<TokenSpan>, _: TokenSpan) {
        self.log.push(("begin", "for_control_flow"));
    }
    fn end_for_control_flow(&mut self, _: TokenSpan) {
        self.log.push(("end", "for_control_flow"));
    }
    fn end_for_in_control_flow(&mut self, _: TokenSpan) {
        self.log.push(("end", "for_in_control_flow"));
    }
    fn handle_spread_expression(&mut self, _: TokenSpan) {
        self.log.push(("handle", "spread_expression"));
    }
    fn handle_null_aware_element(&mut self, _: TokenSpan) {
        self.log.push(("handle", "null_aware_element"));
    }
    fn begin_await_expression(&mut self, _: TokenSpan) {
        self.log.push(("begin", "await_expression"));
    }
    fn end_await_expression(&mut self, _: TokenSpan, _: TokenSpan) {
        self.log.push(("end", "await_expression"));
    }
    fn end_invalid_await_expression(&mut self, _: TokenSpan, _: TokenSpan) {
        self.log.push(("end", "invalid_await_expression"));
    }
    fn handle_throw_expression(&mut self, _: TokenSpan, _: TokenSpan) {
        self.log.push(("handle", "throw_expression"));
    }
    fn handle_dot_shorthand_head(&mut self, _: TokenSpan) {
        self.log.push(("handle", "dot_shorthand_head"));
    }
    fn handle_dot_shorthand_context(&mut self, _: TokenSpan) {
        self.log.push(("handle", "dot_shorthand_context"));
    }
    fn begin_const_dot_shorthand(&mut self, _: TokenSpan) {
        self.log.push(("begin", "const_dot_shorthand"));
    }
    fn end_const_dot_shorthand(&mut self, _: TokenSpan) {
        self.log.push(("end", "const_dot_shorthand"));
    }
    fn handle_pattern_assignment(&mut self, _: TokenSpan) {
        self.log.push(("handle", "pattern_assignment"));
    }
    fn handle_unary_prefix_assignment_expression(&mut self, _: TokenSpan) {
        self.log.push(("handle", "unary_prefix_assignment"));
    }
    fn handle_unary_postfix_assignment_expression(&mut self, _: TokenSpan) {
        self.log.push(("handle", "unary_postfix_assignment"));
    }
    fn begin_implicit_creation_expression(&mut self, _: TokenSpan) {
        self.log.push(("begin", "implicit_creation_expression"));
    }
    fn end_implicit_creation_expression(&mut self, _: TokenSpan, _: TokenSpan) {
        self.log.push(("end", "implicit_creation_expression"));
    }
    fn begin_assert(&mut self, _: TokenSpan, _: AssertKind) {
        self.log.push(("begin", "assert"));
    }
    fn end_assert(
        &mut self,
        _: TokenSpan,
        _: AssertKind,
        _: TokenSpan,
        _: Option<TokenSpan>,
        _: TokenSpan,
    ) {
        self.log.push(("end", "assert"));
    }
    fn begin_new_expression(&mut self, _: TokenSpan) {
        self.log.push(("begin", "new_expression"));
    }
    fn end_new_expression(&mut self, _: TokenSpan) {
        self.log.push(("end", "new_expression"));
    }
    fn begin_const_expression(&mut self, _: TokenSpan) {
        self.log.push(("begin", "const_expression"));
    }
    fn end_const_expression(&mut self, _: TokenSpan) {
        self.log.push(("end", "const_expression"));
    }
    fn handle_is_operator(&mut self, _: TokenSpan, _: Option<TokenSpan>) {
        self.log.push(("handle", "is_operator"));
    }
    fn handle_as_operator(&mut self, _: TokenSpan) {
        self.log.push(("handle", "as_operator"));
    }
    fn handle_send(&mut self, begin: TokenSpan, end: TokenSpan) {
        self.log.push(("handle", "send"));
        self.sends
            .push((begin.range.offset, end.range.offset + end.range.length));
    }
    fn handle_dot_access(&mut self, dot: TokenSpan, _: TokenSpan, is_null_aware: bool) {
        self.log.push(("handle", "dot_access"));
        self.dots.push((is_null_aware, dot.range.offset));
    }
    fn handle_indexed_expression(
        &mut self,
        question: Option<TokenSpan>,
        open: TokenSpan,
        _: TokenSpan,
    ) {
        self.log.push(("handle", "indexed_expression"));
        self.indexes.push((question.is_some(), open.range.offset));
    }
    fn handle_non_null_assert_expression(&mut self, token: TokenSpan) {
        self.null_asserts.push(token.range.offset);
    }

    // Pattern events the switch-expression cases and pattern assignments
    // ride on (constant/wildcard/list shapes).
    fn handle_wildcard_pattern(&mut self, _: Option<TokenSpan>, _: TokenSpan) {
        self.log.push(("handle", "wildcard_pattern"));
    }
    fn handle_list_pattern(&mut self, _: usize, _: TokenSpan, _: TokenSpan) {
        self.log.push(("handle", "list_pattern"));
    }
    fn begin_constant_pattern(&mut self, _: Option<TokenSpan>) {
        self.log.push(("begin", "constant_pattern"));
    }
    fn end_constant_pattern(&mut self, _: Option<TokenSpan>) {
        self.log.push(("end", "constant_pattern"));
    }
    fn begin_constructor_reference(&mut self, _: TokenSpan) {
        self.log.push(("begin", "constructor_reference"));
    }
    fn end_constructor_reference(
        &mut self,
        _: TokenSpan,
        _: Option<TokenSpan>,
        _: TokenSpan,
        _: ConstructorReferenceContext,
    ) {
        self.log.push(("end", "constructor_reference"));
    }

    // The statement shell the expression fixtures sit in.
    fn handle_expression_statement(&mut self, _: TokenSpan, _: TokenSpan) {
        self.log.push(("handle", "expression_statement"));
    }
    fn begin_variables_declaration(
        &mut self,
        _: TokenSpan,
        _: Option<TokenSpan>,
        _: Option<TokenSpan>,
    ) {
        self.log.push(("begin", "variables_declaration"));
    }
    fn end_variables_declaration(&mut self, _: usize, _: Option<TokenSpan>) {
        self.log.push(("end", "variables_declaration"));
    }
}

fn setup(source: &'static str) -> Parser<'static, 'static, 'static, ExprLog> {
    let config = dartq_scanner::ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut files = dartq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("test.dart"));
    let result = dartq_scanner::scan(source, config, file);
    assert!(
        result.diagnostics.is_empty(),
        "scanner diagnostics in {source:?}: {:?}",
        result.diagnostics
    );
    let table: &'static dartq_scanner::TokenTable = Box::leak(Box::new(result.tokens));
    let text: &'static str = Box::leak(source.to_string().into_boxed_str());
    let features =
        crate::ParserFeatures::from_feature_set(dartq_language::FeatureSet::for_language_version(
            dartq_language::LanguageVersion::new(3, 13),
        ));
    // The `'static` fixture shape (matching the leaked table/text) leaks
    // the log: tests read events through the parser and never reclaim it.
    Parser::new(
        text,
        table,
        features,
        Box::leak(Box::new(ExprLog::default())),
    )
}

fn parse(source: &'static str) -> ExprLog {
    let mut p = setup(source);
    p.parse_unit();
    std::mem::take(p.listener)
}

/// Parses just the expression at the cursor (unit-level entry checks).
fn expression_errors(source: &'static str) -> Vec<String> {
    let mut p = setup(source);
    let _ = p.parse_expression();
    p.listener.codes().to_vec()
}

// —— precedence and associativity ————————————————————————————————————————

#[test]
fn assignment_is_right_associative() {
    let c = parse("void f() { a = b = c; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    assert_eq!(c.count("handle", "assignment_expression"), 2);
}

#[test]
fn multiplicative_binds_tighter_than_additive() {
    let c = parse("void f() { x = 1 + 2 * 3; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    // `2 * 3` parses fully inside the `+`'s right operand.
    c.assert_sequence(&[("handle", "assignment_expression")]);
}

#[test]
fn augment_builtin_reads_as_name_in_expressions() {
    // The harness scans with augmentations enabled, so `augment` arrives as
    // Keyword(Augment) (with the feature off the lexical gate makes it a
    // plain identifier, abstract_scanner.dart:2106). Either way,
    // expression-position use is a parser-level contextual decision — the
    // parser reads it as a name (official parsePrimary's isIdentifier
    // fall-through, parser_impl.dart:7558-7576).
    let c = parse("void f() { augment + 1; x.augment; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
}

#[test]
fn equality_cannot_chain_but_is_then_equality_can() {
    let c = parse("void f() { a == b == c; }");
    assert_eq!(
        c.codes(),
        ["equality_cannot_be_equality_operand"],
        "the official chain check fires once"
    );
    let c = parse("void f() { x is int == y; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("handle", "is_operator")]);
}

#[test]
fn await_before_identifier_in_name_position_reports_unexpected_token() {
    // ExpressionIdentifierContext.ensureIdentifier
    // (identifier_context_impl.dart:397-406): `await` followed by another
    // identifier does not form a valid expression-name — the oracle reports
    // `unexpectedToken` on the `await` token itself and the following token
    // becomes the name. Differential-corpus face: `.await x` selectors and
    // cascade sections (`..await x`) in expression-continuation position.
    let c = parse("void f() { a..await b; }");
    assert_eq!(c.codes(), ["unexpected_token"], "{:?}", c.errors);
}

#[test]
fn conditional_parses_and_speculates_null_aware_index() {
    // `a ? [b] : c` — the conditional speculation wins over `?[`.
    let c = parse("void f() { x = a ? [b] : c; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "conditional_expression"),
        ("end", "conditional_expression"),
    ]);

    // `a?[b]` — a null-aware index, not a conditional.
    let c = parse("void f() { x = a?[b]; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    assert_eq!(c.count("begin", "conditional_expression"), 0);
}

#[test]
fn is_as_operands_use_conditional_speculation_on_nullable_types() {
    // `x is int? ...` in a statement keeps the nullable type (hard
    // follower); no conditional misparse.
    let c = parse("void f() { y = a is int? b : c; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("handle", "is_operator"),
        ("begin", "conditional_expression"),
    ]);
}

// —— assignment, cascade, throw, await ———————————————————————————————————

#[test]
fn cascades_chain_with_null_aware_sections() {
    let c = parse("void f() { x..a()..b = 1; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "cascade"),
        ("handle", "cascade_access"),
        ("end", "cascade"),
    ]);

    // A `?..` section AFTER a previous cascade section reports once
    // (:6665-6669); a leading `?..` is fine.
    let c = parse("void f() { x?..a()..b(); }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("begin", "cascade"), ("end", "cascade")]);

    let c = parse("void f() { x..a()?..b(); }");
    assert_eq!(
        c.codes(),
        ["null_aware_cascade_out_of_order"],
        "a `?..` after a cascade section reports once"
    );
}

#[test]
fn throw_expression_shapes() {
    let c = parse("void f() { throw e; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("handle", "throw_expression")]);

    // `throw;` reports the pinned-live missing-expression code exactly once
    // (the synthetic empty string operand emits no further diagnostics).
    let c = parse("void f() { throw; }");
    assert_eq!(c.codes(), ["missing_expression_in_throw"], "{:?}", c.log);
}

#[test]
fn await_expression_events_split_by_async_state() {
    // Sync context: `await x;` is NOT an await expression — a sync `await`
    // that fails `looksLikeAwaitExpression` routes to
    // parseExpressionStatementOrDeclaration (parser_impl.dart:6805-6815;
    // `looksLikeExpressionAfterAwaitOrYield` :10591-10609 rejects `x ;`),
    // so it parses as a local variable declaration with no diagnostics and
    // no await events. The `end_invalid_await_expression` face belongs to
    // await-expression-shaped sync code (`await f();`).
    let c = parse("void f() { await x; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    assert!(
        c.log.contains(&("begin", "variables_declaration")),
        "{:?}",
        c.log
    );
    assert!(
        !c.log.contains(&("begin", "await_expression")),
        "{:?}",
        c.log
    );

    // Async context: the plain end event.
    let c = parse("void f() async { await x; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("begin", "await_expression"), ("end", "await_expression")]);
}

// —— send-leaf span tiling (#585) ————————————————————————————————————————

/// A bare-name Send leaf's tiled span extends through its follower token:
/// `parse_arguments_opt` hands back the peeked follower as the
/// `handle_send` end, so `x;`'s leaf covers the `;` and the `x = 1` LHS
/// leaf covers the `=`. Consumers must not assume leaf span == own token.
#[test]
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn bare_send_leaf_span_includes_follower_token() {
    let src = "void f() { x; }";
    let c = parse(src);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let x = src.find('x').unwrap() as u32;
    let semi = src.find(';').unwrap() as u32;
    assert_eq!(c.sends, [(x, semi + 1)]);

    let src = "void f() { x = 1; }";
    let c = parse(src);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let x = src.find('x').unwrap() as u32;
    let eq = src.find('=').unwrap() as u32;
    assert_eq!(c.sends, [(x, eq + 1)]);
}

/// The Assignment anchor adopts the just-completed RHS subtree via
/// `last_subtree_start` (the dartq-ast builder read face), so the anchored
/// Assignment's extent is the RHS Send leaf's tiled span — which includes
/// the statement `;` (`x = y;` RHS leaf ends past the `;`).
#[test]
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn assignment_rhs_send_leaf_span_includes_semicolon() {
    let src = "void f() { x = y; }";
    let c = parse(src);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    assert_eq!(c.count("handle", "assignment_expression"), 1);
    let x = src.find('x').unwrap() as u32;
    let eq = src.find('=').unwrap() as u32;
    let y = src.find('y').unwrap() as u32;
    let semi = src.find(';').unwrap() as u32;
    // LHS leaf ends on the `=`; the RHS leaf — the subtree the Assignment
    // anchor adopts — ends past the `;`.
    assert_eq!(c.sends, [(x, eq + 1), (y, semi + 1)]);
}

// —— function literals ——————————————————————————————————————————————————

#[test]
fn anonymous_function_literal_block_and_arrow() {
    let c = parse("void f() { g(() {}); }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "function_expression"),
        ("end", "function_expression"),
    ]);

    let c = parse("void f() { var h = (x) => x; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "function_expression"),
        ("end", "function_expression"),
    ]);
}

#[test]
fn generic_anonymous_function_literal() {
    let c = parse("void f() { var h = <T>(T x) { return x; }; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "function_expression"),
        ("end", "function_expression"),
    ]);
}

#[test]
fn named_function_expression_reports_named_function_expression() {
    // `function foo() {}` (JS paste) parses as a named function expression
    // with the pinned-live recovery report.
    let c = parse("void f() { var g = function foo() {}; }");
    assert_eq!(c.codes(), ["named_function_expression"], "{:?}", c.log);
    c.assert_sequence(&[
        ("begin", "named_function_expression"),
        ("end", "named_function_expression"),
    ]);
}

// —— switch expressions ——————————————————————————————————————————————————

#[test]
fn switch_expression_cases() {
    let c = parse("void f() { var y = switch (x) { 1 => 2, _ => 3 }; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "switch_expression"),
        ("begin", "switch_expression_case"),
        ("end", "switch_expression_case"),
        ("begin", "switch_expression_case"),
        ("end", "switch_expression_case"),
        ("end", "switch_expression"),
    ]);
    assert!(c.count("handle", "wildcard_pattern") >= 1);
}

#[test]
fn switch_expression_default_and_colon_recovery() {
    let c = parse("void f() { var y = switch (x) { default => 1 }; }");
    assert_eq!(c.codes(), ["default_in_switch_expression"], "{:?}", c.log);

    let c = parse("void f() { var y = switch (x) { 1: 2 }; }");
    assert_eq!(c.codes(), ["expected_token"], "{:?}", c.log);
}

// —— collection control-flow elements —————————————————————————————————————

#[test]
fn collection_if_else_elements() {
    let c = parse("void f() { var l = [if (b) 1 else 2]; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "if_control_flow"),
        ("end", "if_else_control_flow"),
    ]);
}

#[test]
fn collection_for_and_for_in_elements() {
    let c = parse("void f() { var l = [for (var i = 0; i < 2; i++) i]; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("begin", "for_control_flow"), ("end", "for_control_flow")]);

    let c = parse("void f() { var l = [for (final x in xs) x]; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "for_control_flow"),
        ("end", "for_in_control_flow"),
    ]);

    let c = parse("void f() { var l = [await for (var x in xs) x]; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "for_control_flow"),
        ("end", "for_in_control_flow"),
    ]);
}

#[test]
fn collection_spread_and_null_aware_elements() {
    let c = parse("void f() { var l = [...a, ...?b]; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    assert_eq!(c.count("handle", "spread_expression"), 2);

    let c = parse("void f() { var s = {?a}; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    assert_eq!(c.count("handle", "null_aware_element"), 1);
}

#[test]
fn collection_control_flow_nesting_closes_in_order() {
    // `if` wrapping a `for`: the for completes, then the if.
    let c = parse("void f() { var l = [if (b) for (var x in xs) x]; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "if_control_flow"),
        ("begin", "for_control_flow"),
        ("end", "for_in_control_flow"),
        ("end", "if_control_flow"),
    ]);
}

#[test]
fn deep_collection_control_flow_latches_stack_overflow() {
    // 600-deep `if` elements inside one list literal: the pending machine
    // frames obey the shared 500 nesting cap — the linearized counterpart of
    // the official `expressionDepth` guard (parser_impl.dart:6939-6963),
    // whose trip reports a single `stackOverflow`, skips to the group end,
    // and splices a synthetic identifier (an event, not a diagnostic). The
    // official guard is LOCAL — no terminal bailout; the parse resumes past
    // the group and completes, so the legal-descent face is exactly one
    // code. (The leading `[` matters: an `if` at the initializer position is
    // not an expression and would legitimately prepend
    // missing_identifier/expected_token prologue codes in both parsers.)
    let source: &'static str = Box::leak(
        format!(
            "void f() {{ var l = [{}1{}]; }}",
            "if (b) [".repeat(600),
            "]".repeat(600)
        )
        .into_boxed_str(),
    );
    let mut p = setup(source);
    p.parse_unit();
    assert_eq!(
        p.listener.codes(),
        ["stack_overflow"],
        "exactly one stack_overflow report: {:?}",
        p.listener.codes()
    );
}

/// Collection for-element heads mirror the statement decision order
/// (official `ForCondition.parse`, literal_entry_info_impl.dart:24-81): the
/// for-parts scan parses a pattern head itself, so the `=`/`in` split reads
/// the token after the pattern — a pattern for-in head keeps the for-in
/// shape instead of eating the `(` as the parts keyword. Regression: the
/// collection-side head used to skip the pattern parse (silent mis-parse
/// found by the dartq-flow lane).
#[test]
fn collection_for_pattern_head_lands_on_in() {
    let c = parse("void f() { var l = [for (var (a, b) in pairs) a]; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "for_control_flow"),
        ("end", "for_in_control_flow"),
    ]);

    // Pattern head with the C-style continuation
    // (`for ( pattern = expression ; ... ; ... )`).
    let c = parse("void f() { var l = [for (var (a, b) = pair; c; i++) a]; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("begin", "for_control_flow"), ("end", "for_control_flow")]);

    // A set/map element takes the same head machinery.
    let c = parse("void f() { var m = {for (var (k, v) in pairs) k: v}; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "for_control_flow"),
        ("end", "for_in_control_flow"),
    ]);

    // The statement form keeps its decision order too.
    let c = parse("void f() { for (var (a, b) in pairs) a; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
}

// —— dot shorthands, implicit creation, new/const ————————————————————————

#[test]
fn dot_shorthand_expression() {
    let c = parse("void f() { E e = .first; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("handle", "dot_shorthand_head"),
        ("handle", "dot_shorthand_context"),
    ]);

    let c = parse("void f() { E e = const .new(1); }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "const_dot_shorthand"),
        ("handle", "dot_shorthand_head"),
        ("handle", "dot_shorthand_context"),
        ("end", "const_dot_shorthand"),
    ]);
}

#[test]
fn implicit_creation_and_new_forms() {
    let c = parse("void f() { A a = A<int>.new(1); }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "implicit_creation_expression"),
        ("end", "implicit_creation_expression"),
    ]);

    let c = parse("void f() { B b = new B(); }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("begin", "new_expression"), ("end", "new_expression")]);
}

// —— pattern assignment ——————————————————————————————————————————————————

#[test]
fn pattern_assignment_in_expression_position() {
    let c = parse("void f() { [a, b] = list; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("handle", "list_pattern"),
        ("handle", "pattern_assignment"),
        ("handle", "expression_statement"),
    ]);
}

// —— assert in expression position ————————————————————————————————————————

#[test]
fn assert_expression_parses() {
    let c = parse("void f() { g(assert(x)); }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("begin", "assert"), ("end", "assert")]);
}

// —— do-not-emit riders (Group B) —————————————————————————————————————————

#[test]
fn riders_group_b_never_emitted() {
    // `positional_after_named_argument` is resolution-side; the parser
    // accepts the argument list.
    assert!(expression_errors("f(a: 1, 2)").is_empty());
    // `expected_list_or_map_literal` is oracle-dead: the recovery family is
    // expected_token.
    assert_eq!(
        expression_errors("const <int>5"),
        vec!["expected_token".to_string()]
    );
    // `constructor_with_type_arguments` is a 3.15 trap: `A<int> 1` recovers
    // through the generic family only.
    let codes = expression_errors("new A<int> 1");
    assert!(
        !codes.iter().any(|c| c == "constructor_with_type_arguments"),
        "{codes:?}"
    );
}

// —— recovery —————————————————————————————————————————————————————————————

#[test]
fn conditional_without_colon_stays_an_index_attempt() {
    // `a ? b` with no colon: the speculation rejects the conditional
    // reading; the `?` then cannot continue as a selector — the pinned
    // oracle's generic family surfaces.
    let codes = expression_errors("a ? b ;");
    assert!(
        codes.iter().all(|c| c != "expected_list_or_map_literal"),
        "{codes:?}"
    );
}

#[test]
fn unterminated_cascade_section_recovers() {
    let c = parse("void f() { x..; }");
    assert!(
        !c.codes().is_empty(),
        "a cascade section missing its member reports"
    );
    assert!(
        c.codes()
            .iter()
            .all(|c| c == "missing_identifier" || c == "expected_token"),
        "recovery stays in the generic family: {:?}",
        c.codes()
    );
}

#[test]
fn assignment_without_rhs_reports_missing_identifier() {
    assert_eq!(
        expression_errors("a = ;"),
        vec!["missing_identifier".to_string()]
    );
}

#[test]
fn unary_assignment_shapes_event() {
    let c = parse("void f() { ++a; b--; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("handle", "unary_prefix_assignment")]);
    assert_eq!(c.count("handle", "unary_postfix_assignment"), 1);
}

#[test]
fn written_out_operator_recovery_reports_once() {
    // `a and b` speculates the `&&` reading and reports
    // `binary_operator_written_out` (pinned-live code).
    let c = parse("void f() { var z = a and b; }");
    assert_eq!(c.codes(), ["binary_operator_written_out"], "{:?}", c.errors);
}

// —— termination ——————————————————————————————————————————————————————————

#[test]
fn deep_nesting_latches_the_bailout() {
    // 600 nested list literals inside a variable initializer: the official
    // expressionDepth guard (parser_impl.dart:6939-6963) is LOCAL — one
    // stackOverflow report, skip to the group end, synthetic-identifier
    // splice, and the parse completes without a terminal bailout.
    let source: &'static str = Box::leak(
        format!(
            "void f() {{ var x = {}1{}; }}",
            "[".repeat(600),
            "]".repeat(600)
        )
        .into_boxed_str(),
    );
    let mut p = setup(source);
    p.parse_unit();
    let codes = p.listener.codes();
    assert!(
        codes.contains(&"stack_overflow".to_owned()),
        "the terminal diagnostic is the official stackOverflow code: {:?}",
        codes.last()
    );
}

#[test]
fn deep_cascade_chain_terminates() {
    let source: &'static str =
        Box::leak(format!("void f() {{ x{}; }}", "..a".repeat(2_000)).into_boxed_str());
    let mut p = setup(source);
    p.parse_unit();
    // Terminates (no hang) with either a clean parse or the bounded
    // bailout; each cascade section consumes tokens.
    let _ = p.listener.codes();
}

#[test]
fn deep_conditional_chain_terminates() {
    let source: &'static str = Box::leak(
        format!(
            "void f() {{ var x = {}a{}; }}",
            "b?".repeat(500),
            ":c".repeat(500)
        )
        .into_boxed_str(),
    );
    let mut p = setup(source);
    p.parse_unit();
    let _ = p.listener.codes();
}

// —— engine-level unit checks —————————————————————————————————————————————

#[test]
fn dot_shorthand_requires_identifier_or_new() {
    // `.new(1)` head.
    let mut p = setup(".new(1)");
    let _ = p.parse_expression();
    assert!(p.listener.codes().is_empty(), "{:?}", p.listener.codes());
    assert_eq!(p.listener.count("handle", "dot_shorthand_head"), 1);
}

#[test]
fn token_kind_smoke() {
    // Guard the import surface used by the fixtures above.
    assert_eq!(TokenKind::Comma.fixed_lexeme(), Some(","));
    let _ = IdentifierContext::Expression;
}

// —— oracle recovery pins (#461, r5 oracle-only families) ——————————————————
//
// Each pin reproduces one whole-unit corpus file whose oracle code list the
// r5 differential run caught dartq missing or diverging on; the expected
// multisets are the oracle outputs recorded by the r5 gate-caliber run,
// cross-checked against the official parser testcases (`pkg/front_end/
// parser_testcases` in the upstream Dart SDK source).

/// Sorted code multiset of one whole-unit parse (scanner-clean inputs).
fn sorted_unit_codes(source: &'static str) -> Vec<String> {
    let mut codes = parse(source).codes().to_vec();
    codes.sort();
    codes
}

/// Sorted diagnostic multiset (scanner + parser) of one whole-unit parse,
/// for inputs the scanner itself flags (the unterminated-interpolation
/// re-scan reports its own `expected_token` for the blamed `${`).
fn sorted_unit_codes_with_scanner(source: &'static str) -> Vec<String> {
    let config = dartq_scanner::ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut files = dartq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("test.dart"));
    let result = dartq_scanner::scan(source, config, file);
    let table: &'static dartq_scanner::TokenTable = Box::leak(Box::new(result.tokens));
    let text: &'static str = Box::leak(source.to_string().into_boxed_str());
    let mut codes: Vec<String> = result
        .diagnostics
        .iter()
        .map(|d| d.code.name.to_owned())
        .collect();
    let features =
        crate::ParserFeatures::from_feature_set(dartq_language::FeatureSet::for_language_version(
            dartq_language::LanguageVersion::new(3, 13),
        ));
    let mut expr_log = ExprLog::default();
    let mut p = Parser::new(text, table, features, &mut expr_log);
    p.parse_unit();
    codes.extend(p.listener.codes().iter().cloned());
    codes.sort();
    codes
}

#[test]
fn cascade_question_type_args_lands_four_code_shape() {
    // `a?<Foo>(b);` (oracle nnbd/issue_40267_method_call_with_type_arguments):
    // the `?` opens a conditional whose then-branch parses `<Foo>` as
    // generic function-expression type variables; `(b)` closes and the `;`
    // is its body (unexpected_token from the suffix body check +
    // missing_function_body from the empty body, both consumed by the
    // function expression), the `}` stands in for the missing `:` colon and
    // else operand (missing_identifier), and the statement terminator
    // reports once more (official parser_testcases
    // nnbd/issue_40267_method_call_with_type_arguments.dart.expect:
    // 5 problems = expected_token x2, missing_function_body,
    // missing_identifier, unexpected_token).
    assert_eq!(
        sorted_unit_codes("f() {\n  var a, b;\n\n  a?.call<Foo>(b);\n  a?<Foo>(b);\n}\n"),
        [
            "expected_token",
            "expected_token",
            "missing_function_body",
            "missing_identifier",
            "unexpected_token"
        ],
        "cascade ?<Foo>(b) four-code shape"
    );
}

#[test]
fn const_record_lhs_reports_missing_selector() {
    // `const (_) = x;` (oracle patterns/
    // patternVariableDeclarationStatement_disallowsConst): the const
    // expression is a one-field record literal (trailing-comma report), and
    // handleAssignmentExpression reports missingAssignableSelector at the
    // `const` head (ast_builder.dart:3831-3837) on top of the
    // statement-level illegalAssignmentToNonAssignable rider.
    assert_eq!(
        sorted_unit_codes("f(x) {\n  const (_) = x;\n}\n"),
        [
            "illegal_assignment_to_non_assignable",
            "missing_assignable_selector",
            "record_literal_one_positional_no_trailing_comma"
        ],
        "const (_) = x selector decision"
    );
}

#[test]
fn const_dot_shorthand_incomplete_member_recovers() {
    // `Color c = const .;` (oracle dot_shorthands/const_no_identifier_recovery):
    // the member after `.` is missing. The pinned 3.13.1 oracle faces
    // (trend-w4 gate raw output + live probe reproduction) are TWO rows:
    // `missing_identifier` on the follower `;`'s own span (kept unconsumed)
    // and `expected_token(';')` at the dot (the declaration terminator's
    // walked-back anchor over the synthetic operand). The earlier
    // single-row pin came from a stale probe dump and minted a differential
    // dartq-only `missing_identifier` gap.
    assert_eq!(
        sorted_unit_codes(
            "class Color {\n  final int x;\n  const Color.red(this.x);\n}\nvoid main() {\n  Color c = const .;\n}\n"
        ),
        ["expected_token", "missing_identifier"],
        "const dot-shorthand without a member"
    );
}

#[test]
fn keyword_blocker_inside_switch_expression_case() {
    // `switch (x) {if}` (oracle patterns/syntheticIdentifier_insideSwitchExpression):
    // the case pattern, the `=>` and the case body each meet the reserved
    // `if` without consuming it — missing_identifier for the pattern and the
    // body (deduped), expected_token for the arrow. The arrow row anchors on
    // the blocker; the case-separator recovery row anchors past it — on the
    // switch expression's closing `}` the skip lands on (the #2024
    // separator-anchor rule; frozen 0c9fda4b anchors the separator recovery
    // row on the token following the recovery point).
    let src = "void f(Object? x) => switch (x) {if};\n";
    assert_eq!(
        parse(src).error_rows,
        vec![
            ("missing_identifier".to_string(), 33, 2),
            ("expected_token".to_string(), 33, 2),
            ("expected_token".to_string(), 35, 1),
        ],
        "switch-expression case blocker: {src}"
    );
}

#[test]
fn typing_inside_interpolation_content_recovers() {
    // `${` typed mid-string (oracle error_recovery/with_outline/
    // typing_string_interpolation_1): the scanner's two-pass re-scan turns
    // the group's content back into string body closed by the inner quote
    // and reports the blamed `${` (scanner expected_token for the synthetic
    // `}`); the parser meets the content token where an identifier belongs
    // and reports exactly one missing_identifier (CFE intertwined expect
    // :204), then consumes the content as the string part — no
    // expected-`}` parser report and no member-lane cascade.
    assert_eq!(
        sorted_unit_codes_with_scanner(
            "class A {\n  method1() {}\n  method2() {}\n}\n\nclass B {\n  method1(String foo) {\n    // was:\n    // print(\"hello world\");\n    // but then we started typing inside the quotes:\n    print(\"${foo.length hello world\");\n  }\n\n  method2() {}\n}\n\nclass C {\n  method1() {}\n  method2() {}\n}\n"
        ),
        ["expected_token", "missing_identifier"],
        "typing inside a one-line interpolation"
    );
}

#[test]
fn typing_inside_interpolation_multiline_content_recovers() {
    // typing_string_interpolation_2: the re-scanned body hits the newline
    // (a single-line string ends there), so the scanner adds its
    // unterminated-string report; the parser must still land the
    // missing_identifier on the content and must NOT cascade into the
    // member lane (the r5 run showed named_function_expression /
    // class_in_class noise before the two-pass carve landed).
    let codes = sorted_unit_codes_with_scanner(
        "class A {\n  method1() {}\n  method2() {}\n}\n\nclass B {\n  method1(String foo) {\n    // was:\n    // print(\"hello world\");\n    // but then we started typing inside the quotes:\n    print(\"${foo.\n    length hello world\");\n  }\n\n  method2() {}\n}\n\nclass C {\n  method1() {}\n  method2() {}\n}\n",
    );
    assert!(
        codes.contains(&"missing_identifier".to_owned()),
        "missing_identifier must land on the content: {codes:?}"
    );
    for junk in ["named_function_expression", "class_in_class"] {
        assert!(
            !codes.contains(&junk.to_owned()),
            "{junk} must not cascade: {codes:?}"
        );
    }
}

#[test]
fn unclosed_interpolation_eof_recovery_terminates() {
    // The malformed-EOF shape (`var s = '${x y` — the string AND its open
    // `${` group run to EOF) exercising the interpolation-close recovery
    // (`parseSingleLiteralString`'s missing-`}` jump, upstream Dart SDK
    // source, pkg/_fe_analyzer_shared parser_impl.dart :8745-8750): the
    // expression parse stops on the `y`, the recovery reports once and jumps
    // to the scanner-paired closer via `skip_to_token` — EOF-pinned by
    // contract (the loop exits at eof and `advance` is idempotent there), so
    // a stale target cannot spin. Channel faces: the scanner parks the
    // suspended body's unterminated_string_literal (last-unit pin); the
    // parser's recovery row and the declaration tail's `;` row share the
    // same `y` anchor with different texts ('}' vs ';') — the official
    // `ensureSemicolon` walk-back (`findPreviousNonZeroLengthToken`,
    // util.dart:39) skips the zero-width adopted `}` and lands on `y`, so
    // the cross-text pair stands at one anchor (frozen 76dd104c 12:23:
    // cross-text rows at one anchor both surface), and the adopted
    // `${`-pair row is the third expected_token (the `${` and the string's
    // closer dedupe into it). The pin's completion is the termination
    // proof — the whole-unit parse reaches EOF with this bounded four-row
    // face, no bailout.
    let source = "var s = '${x y";
    let config = dartq_scanner::ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut files = dartq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("test.dart"));
    let result = dartq_scanner::scan(source, config, file);
    let table: &'static dartq_scanner::TokenTable = Box::leak(Box::new(result.tokens));
    let scanner_codes: Vec<&str> = result
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect();
    let text: &'static str = Box::leak(source.to_string().into_boxed_str());
    let features =
        crate::ParserFeatures::from_feature_set(dartq_language::FeatureSet::for_language_version(
            dartq_language::LanguageVersion::new(3, 13),
        ));
    let mut expr_log = ExprLog::default();
    let mut p = Parser::new(text, table, features, &mut expr_log);
    p.parse_unit();
    assert_eq!(
        expr_log.error_rows,
        vec![
            ("expected_token".to_string(), 13, 1),
            ("expected_token".to_string(), 13, 1),
            ("expected_token".to_string(), 14, 1),
        ],
        "unclosed `${{` at eof — bounded recovery face: {:?}",
        expr_log.error_rows
    );
    assert_eq!(
        scanner_codes,
        ["unterminated_string_literal"],
        "the scanner parks the suspended body"
    );
}

// —— audit #180: `>`-family residue ordering ———————————————————————————————

#[test]
fn gt_family_operators_in_statement_position() {
    // Test gap (audit #180): `>`-family operators reach expressions as
    // single tokens (`>`, `>=`, `>>=`) and as consume_closer split residue
    // after type arguments; each statement shape parses with both operand
    // reads intact and no diagnostics.
    let c = parse("void f() { a > b; a >= b; a >>= b; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("handle", "expression_statement"),
        ("handle", "expression_statement"),
        ("handle", "expression_statement"),
    ]);
}

// —— still-red family pins (#1317): FAM12 null-aware chains / FAM9 `new` ————
//
// Parser-level faces from the m0-syntax differential families: the
// null-aware chain and `new`-as-identifier decision points, pinned at the
// anchors the oracle rows use. The families' warning/resolution rows
// (invalid_null_aware_operator, unnecessary_non_null_assertion,
// undefined_getter/setter, dead_code, undefined_identifier, ...) are
// semantic-layer faces riding these events; the parse surface itself is the
// rows below (upstream Dart SDK source, pkg/_fe_analyzer_shared
// parser_impl.dart and identifier_context_impl.dart).

#[test]
fn null_aware_dot_index_missing_identifier_anchors_on_bracket() {
    // `c1?.[0];` (oracle null_shorting_index line 12, row 12:7 len=1): the
    // `?.` opens a null-aware DOT access whose operand is the `[0]` list
    // literal — parsePrimary's OPEN_SQUARE arm runs in continuation position
    // too (parser_impl.dart:8224-8227) — and the dropped operand is ONE
    // missing_identifier anchored on the `[` token itself
    // (ast_builder.doDotExpression recovery, ast_builder.dart:918; the
    // pinned oracle keeps the `[` lexeme as the property name, which is
    // where the resolution face `The getter '[' isn't defined` comes from).
    let c = parse("void f() {\n  c1?.[0];\n}\n");
    assert_eq!(c.codes(), ["missing_identifier"], "{:?}", c.error_rows());
    assert_eq!(c.error_rows(), [("missing_identifier", 17, 1)]);
    assert_eq!(c.dots, [(true, 15)]);
    assert!(c.indexes.is_empty());
}

#[test]
fn bang_then_null_aware_dot_chain_parses_clean() {
    // `a!?.toString();` (oracle issue_39723 line 7 — resolution warnings
    // only, zero parse rows): the `!` before `?.` is selector-level
    // (`_computePrecedence`'s BANG arm, parser_impl.dart:7740-7751), fires
    // one null-assert event, and `?.toString()` continues the chain as a
    // null-aware dot access.
    let c = parse("void f() {\n  a!?.toString();\n}\n");
    assert!(c.codes().is_empty(), "{:?}", c.codes());
    assert_eq!(c.null_asserts, [14]);
    assert_eq!(c.dots, [(true, 15)]);
}

#[test]
fn bang_then_question_bracket_is_null_aware_index_not_conditional() {
    // `a!?[42];` and the spaced `a! ? [42];` (oracle issue_39723 lines 9-10
    // — resolution warnings only): `?` followed by `[` keeps conditional
    // precedence only when `canParseAsConditional` succeeds
    // (parser_impl.dart:7777-7789); `[42] ;` has no `:`, so both readings
    // are null-aware index chains with no diagnostics.
    let c = parse("void f() {\n  a!?[42];\n  a! ? [42];\n}\n");
    assert!(c.codes().is_empty(), "{:?}", c.codes());
    assert_eq!(c.null_asserts, [14, 25]);
    assert_eq!(c.indexes, [(true, 16), (true, 29)]);
    assert_eq!(c.count("begin", "conditional_expression"), 0);
}

#[test]
fn null_aware_index_assignment_targets_parse_clean() {
    // `c1?[0] = 1;` and the spaced form (oracle null_shorting_index lines
    // 16/19 — no parse rows): the null-aware index chain is an assignable
    // target (the LHS ends in the index `]`), so the assignment arm adds no
    // selector reports on top of the chain. The logged anchors are the
    // chain-internal event offsets: the unspaced form anchors on the `[`,
    // the spaced form (which rides the conditional-candidate probe before
    // reverting to the index chain) anchors one token later — the diagnostics
    // face is empty on both shapes either way.
    let c = parse("void f() {\n  c1?[0] = 1;\n  c1 ? [0] = 1;\n}\n");
    assert!(c.codes().is_empty(), "{:?}", c.codes());
    assert_eq!(c.indexes, [(true, 16), (true, 32)]);
}

#[test]
fn null_aware_dot_index_assignment_keeps_bracket_row() {
    // `c1?.[0] = 1;` (oracle null_shorting_index line 13, rows 13:7
    // missing_identifier + undefined_setter): the missing-name row anchors
    // on the `[` and the assignment shape is unchanged by the dropped
    // operand.
    let c = parse("void f() {\n  c1?.[0] = 1;\n}\n");
    assert_eq!(c.codes(), ["missing_identifier"], "{:?}", c.error_rows());
    assert_eq!(c.error_rows(), [("missing_identifier", 17, 1)]);
    assert_eq!(c.dots, [(true, 15)]);
}

#[test]
fn null_aware_dot_index_chain_reports_per_link() {
    // `c1?.[0]?.[1];`: every `?.[` link of the chain takes the
    // dropped-operand face once, anchored on its own `[`.
    let c = parse("void f() {\n  c1?.[0]?.[1];\n}\n");
    assert_eq!(
        c.codes(),
        ["missing_identifier", "missing_identifier"],
        "{:?}",
        c.error_rows()
    );
    assert_eq!(
        c.error_rows(),
        [("missing_identifier", 17, 1), ("missing_identifier", 22, 1)]
    );
    assert_eq!(c.dots, [(true, 15), (true, 20)]);
}

#[test]
fn dot_paren_operand_reports_missing_identifier_at_paren() {
    // `a.(0);` / `a?.(0);` (oracle error_recovery/period_missing_name lines
    // 3/5, rows 3:5 and 5:6 len=1): the parenthesized group is not a name —
    // parsePrimary only takes the parenthesized/record reading OUTSIDE
    // continuation positions (parser_impl.dart:8219-8223), so the operand
    // face is one missing_identifier anchored on the `(`.
    let c = parse("void f() {\n  a.(0);\n  a?.(0);\n}\n");
    assert_eq!(
        c.codes(),
        ["missing_identifier", "missing_identifier"],
        "{:?}",
        c.error_rows()
    );
    assert_eq!(
        c.error_rows(),
        [("missing_identifier", 15, 1), ("missing_identifier", 25, 1)]
    );
    assert_eq!(c.dots, [(false, 14), (true, 23)]);
}

#[test]
fn new_reads_as_identifier_in_dot_continuations() {
    // `C.new()`, the `C.new` tear-off and the `.new.toString()` chain
    // (oracle new_as_identifier lines 37/41/44 — zero rows): after `.`,
    // `new` is rewritten to an identifier naming the unnamed constructor
    // (parsePrimary's `_tryRewriteNewToIdentifier`,
    // parser_impl.dart:8108 + :4101-4111), so the chain is dot accesses
    // with no diagnostics; the constructor reading is the resolution
    // layer's call.
    let c =
        parse("void f() {\n  var a = C.new();\n  var t = C.new;\n  var u = C.new.toString();\n}\n");
    assert!(c.codes().is_empty(), "{:?}", c.codes());
    assert_eq!(c.dots, [(false, 22), (false, 41), (false, 58), (false, 62)]);
    assert_eq!(c.count("begin", "constructor_reference"), 0);
}

// —— trend-w5B residual faces #1448: F-J `?.[` / F-K keyword-call `+ 1` ————
//
// Parse-surface pins for the m0-syntax differential anchors
// (nnbd/issue_39723*.dart, nnbd/null_shorting_index.dart,
// error_recovery/keyword_named_*_methods_2.dart). The rows these fixtures
// additionally report (undefined_getter/setter `'['`,
// invalid_null_aware_operator, return_of_invalid_type, undefined_operator,
// not_a_type, dead_code) are resolution/flow faces riding these events.

#[test]
fn bang_null_aware_dot_bracket_missing_name_anchors_on_bracket() {
    // `a!?.[42];` (oracle issue_39723 line 8, rows 8:7): the postfix `!`
    // fires one null-assert event, then the `?.` opens a null-aware DOT
    // access whose operand is the `[42]` list literal folded in by
    // parsePrimary's OPEN_SQUARE arm in continuation position
    // (parser_impl.dart:8224-8227) — one missing_identifier anchored on the
    // `[`, no index event (the bracket group is the dropped operand, not a
    // selector).
    let c = parse("void f() {\n  a!?.[42];\n}\n");
    assert_eq!(c.codes(), ["missing_identifier"], "{:?}", c.error_rows());
    assert_eq!(c.error_rows(), [("missing_identifier", 17, 1)]);
    assert_eq!(c.null_asserts, [14]);
    assert_eq!(c.dots, [(true, 15)]);
    assert!(c.indexes.is_empty());
}

#[test]
fn paren_bang_null_aware_dot_bracket_missing_name_anchors_on_bracket() {
    // `(a!)?.[42];` (oracle issue_39723_prime line 8, row 8:9): the
    // parenthesized receiver parses as a group (the `!` inside fires its
    // own null-assert event) and the `?.[` link folds the `[42]` operand
    // with the missing-name row anchored on the `[`.
    let c = parse("void f() {\n  (a!)?.[42];\n}\n");
    assert_eq!(c.codes(), ["missing_identifier"], "{:?}", c.error_rows());
    assert_eq!(c.error_rows(), [("missing_identifier", 19, 1)]);
    assert_eq!(c.null_asserts, [15]);
    assert_eq!(c.dots, [(true, 17)]);
    assert!(c.indexes.is_empty());
}

#[test]
fn keyword_is_call_recovers_as_is_expression_with_record_type_operand() {
    // `return is(x-1) + 1;` (oracle keyword_named_class_methods_2 line 184
    // parse rows): `is` in expression position reports missing_identifier
    // and is SYNTHESIZED AROUND — the reserved word stays in the stream and
    // serves as the is-operator (`ensureIdentifier`'s expression recovery,
    // identifier_context_impl.dart:407-449; the intertwined expect shows
    // handleSendWithoutArguments ending at `is`, then parseIsOperatorRest),
    // so `(x-1` parses as the record TYPE operand: the `-` reports
    // expected_token ')', the group closes at the real `)` with the
    // one-positional-field report, the return's `;` reports at that `)`,
    // and `+ 1;` continues as its own statement — prefix `+` rewrites to a
    // synthetic identifier while the `+` itself stays for the binary loop
    // (parseUnaryExpression :7904-7912), so the statement consumes `1 ;`
    // cleanly with its one missing_identifier and NO trailing
    // expected_token.
    let c = parse("void f(int x) {\n  return is(x-1) + 1;\n}\n");
    assert_eq!(
        c.error_rows(),
        [
            ("missing_identifier", 25, 2),
            ("expected_token", 29, 1),
            ("record_type_one_positional_no_trailing_comma", 31, 1),
            ("expected_token", 31, 1),
            ("missing_identifier", 33, 1),
        ],
        "is-call recovery rows: {:?}",
        c.codes()
    );
    c.assert_sequence(&[
        ("handle", "is_operator"),
        ("handle", "expression_statement"),
    ]);
}

#[test]
fn keyword_const_call_parses_const_record_and_joins_binary_plus() {
    // `return const(x-1) + 1;` (oracle keyword_named_class_methods_2 line
    // 49 parse rows): `const(x-1)` is a const record literal — one
    // positional field without a trailing comma reports once at the `)` —
    // and the binary `+ 1` joins the return expression (no expected_token
    // rows; `non_constant_record_field` and `undefined_operator '+'` are
    // resolution faces).
    let c = parse("void f(int x) {\n  return const(x-1) + 1;\n}\n");
    assert_eq!(
        c.error_rows(),
        [("record_literal_one_positional_no_trailing_comma", 34, 1)],
        "const-call recovery rows: {:?}",
        c.codes()
    );
}

#[test]
fn prefixed_and_generic_tearoffs_parse_clean() {
    // `p.C.new` and `C<int>.new` tear-offs (oracle new_as_identifier lines
    // 45/46 — only resolution rows): the qualified prefix chain and the
    // method-type-argument application both keep `.new` readable. The third
    // dot's anchor sits past the `<int>` group (C@40, `<`@41, `int` 42..45,
    // `.`@46).
    let c = parse("void f() {\n  var t = p.C.new;\n  var g = C<int>.new;\n}\n");
    assert!(c.codes().is_empty(), "{:?}", c.codes());
    assert_eq!(c.dots, [(false, 22), (false, 24), (false, 46)]);
}

#[test]
fn new_constructor_references_parse_clean() {
    // `const C.new()`, `new C<int>.new()` (oracle new_as_identifier lines
    // 29/33-34 — only resolution rows): parseNewExpression and
    // parseConstExpression route through the constructor reference, whose
    // `.new` continuation reads as the unnamed constructor
    // (parseConstructorReference :6414 with the continuation rewrite).
    let c = parse("void f() {\n  var a = const C.new();\n  var b = new C<int>.new();\n}\n");
    assert!(c.codes().is_empty(), "{:?}", c.codes());
    assert_eq!(c.count("begin", "constructor_reference"), 2);
    assert!(c.dots.is_empty());
}

#[test]
fn cascade_sections_accept_new_as_name() {
    // `a..new()`, `a?..new` and the section chain `a..b.new` (parseSend at
    // expressionContinuation: ensureIdentifier's `new` rewrite,
    // parser_impl.dart:4101-4111 with identifier_context_impl.dart:391 —
    // `allowsNewAsIdentifier => isContinuation`): the section name is the
    // unnamed constructor, consumed silently — a reserved-word report here
    // would be a dartq-only row on any corpus using `..new`.
    let c = parse("void f() {\n  a..new();\n  a?..new;\n  a..b.new;\n}\n");
    assert!(c.codes().is_empty(), "{:?}", c.codes());
    assert_eq!(
        c.dots,
        [(false, 40)],
        "only the section loop's `.new` is a dot access; the heads are cascade_access"
    );
    assert_eq!(c.count("handle", "cascade_access"), 3);
}
