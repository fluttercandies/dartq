//! Expression engine tests (#20): precedence/associativity shapes,
//! assignment/conditional/cascade/function-literal/switch-expression forms,
//! collection control-flow elements through statements, the Group B
//! do-not-emit riders (docs/facts/corpus-expansion-plan.md §7), recovery
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
}

impl ParserListener for ExprLog {
    fn handle_recoverable_error(&mut self, message: Message, _: TokenSpan, _: TokenSpan) {
        self.errors.push(message.code.name.to_owned());
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

fn setup(source: &'static str) -> Parser<'static, 'static, ExprLog> {
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
    Parser::new(text, table, features, ExprLog::default())
}

fn parse(source: &'static str) -> ExprLog {
    let mut p = setup(source);
    p.parse_unit();
    p.listener
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
    assert!(!c.log.contains(&("begin", "await_expression")), "{:?}", c.log);

    // Async context: the plain end event.
    let c = parse("void f() async { await x; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("begin", "await_expression"), ("end", "await_expression")]);
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
    c.assert_sequence(&[
        ("begin", "for_control_flow"),
        ("end", "for_control_flow"),
    ]);

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
    // 600 nested list literals inside a variable initializer.
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
    assert!(
        p.is_bailed(),
        "the structural nesting cap must latch the bailout"
    );
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
// multisets are the oracle outputs recorded in
// docs/audits/gate-m0/gate-caliber-r5/report.json, cross-checked against the
// official parser testcases under .third/dart-sdk/pkg/front_end/parser_testcases.

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
    let mut p = Parser::new(text, table, features, ExprLog::default());
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
    // the member after `.` is missing; the official reports one
    // missing_identifier at the follower and the `;` survives as the
    // variable-declaration terminator (CFE intertwined expect: report
    // ExpectedIdentifier -> handleIdentifier -> handleDotShorthandHead ->
    // endConstDotShorthand, `;` unconsumed).
    assert_eq!(
        sorted_unit_codes(
            "class Color {\n  final int x;\n  const Color.red(this.x);\n}\nvoid main() {\n  Color c = const .;\n}\n"
        ),
        ["missing_identifier"],
        "const dot-shorthand without a member"
    );
}

#[test]
fn keyword_blocker_inside_switch_expression_case() {
    // `switch (x) {if}` (oracle patterns/syntheticIdentifier_insideSwitchExpression):
    // the case pattern, the `=>` and the case body each meet the reserved
    // `if` without consuming it — missing_identifier x2, then the arrow and
    // the case separator report expected_token ('=>' and '}').
    assert_eq!(
        sorted_unit_codes("void f(Object? x) => switch (x) {if};\n"),
        [
            "expected_token",
            "expected_token",
            "missing_identifier",
            "missing_identifier"
        ],
        "switch-expression case blocker"
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
