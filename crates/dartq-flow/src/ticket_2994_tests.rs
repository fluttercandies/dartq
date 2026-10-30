//! #2994 — dead-code window shape pins: the owned-vs-inherited arm gate
//! (a dead arm reports only when its parent was live, mirroring upstream
//! `dead_code_verifier`'s `_firstDeadNode` top-down mark + `flowEnd`
//! containment close), the `prev_token_end` arm-end anchoring (the row
//! ends at the token PRECEDING `else`/the arm separator, not at the
//! separator's own offset — trivia between them belonged to the arm), and
//! the named-const condition face (`if (DEBUG)` over a named `const`/
//! `final` bool never dead-folds — the official's `booleanLiteral` fold
//! fires on literal nodes only, dead_code_test.dart's debugConst family).
use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// Dead-code facts of `source` as `(offset, length)` pairs, in source
/// order.
fn dead_offsets(source: &str) -> Vec<(u32, u32)> {
    let result = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        dartq_source::FileTable::new().intern(std::path::Path::new("test.dart")),
    );
    assert!(result.diagnostics.is_empty(), "{source:?}");
    let tokens: &'static TokenTable = Box::leak(Box::new(result.tokens));
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let mut flow = SharedFlow(std::rc::Rc::new(std::cell::RefCell::new(Flow::new(
        tokens, source,
    ))));
    flow.0
        .borrow_mut()
        .set_sound_flow(features.sound_flow_analysis);
    let mut parser = dartq_parser::Parser::new(source, tokens, features, &mut flow);
    parser.parse_unit();
    drop(parser);
    flow.0
        .borrow_mut()
        .take_facts()
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::DeadCode { span } => Some((span.range.offset, span.range.length)),
            _ => None,
        })
        .collect()
}

/// Dead-code facts of `source` as the exact source text they cover.
fn dead_of(source: &str) -> Vec<String> {
    dead_offsets(source)
        .into_iter()
        .map(|(offset, length)| {
            source[offset as usize..(offset + length) as usize].to_string()
        })
        .collect()
}

// —— owned vs inherited arms ————————————————————————————————————————————

#[test]
fn dead_stmts_after_return_fold_into_one_row() {
    // `return; a(); b(); c();` — the official's first dead node is `a();`,
    // the covering close trims to the block's last statement: one row over
    // the whole tail (dead_code_verifier.dart:253-279, :321-326).
    let src = "void f() {\n  return;\n  a();\n  b();\n  c();\n}";
    assert_eq!(dead_of(src), vec!["a();\n  b();\n  c();".to_string()]);
}

#[test]
fn dead_statement_conditional_arms_fold() {
    // `c ? x : y;` inside a dead statement: neither arm entered dead FROM
    // A LIVE PARENT — the enclosing statement owns the whole span.
    let src = "void f() {\n  return;\n  c ? x : y;\n}";
    assert_eq!(dead_of(src), vec!["c ? x : y;".to_string()]);
}

#[test]
fn dead_statement_collection_if_arm_folds() {
    // Same inherited fold for a collection element: `[if (c) y]` inside a
    // dead statement reports the statement, not the `y` element.
    let src = "void f() {\n  return;\n  [if (c) y];\n}";
    assert_eq!(dead_of(src), vec!["[if (c) y];".to_string()]);
}

#[test]
fn conditional_dead_then_arm_reports_own_range() {
    // `false ? 1 : 2;` — the then arm entered dead from a live parent:
    // upstream `flowEnd(thenExpression)` reports the arm's own extent
    // (dead_code_test.dart test_deadBlock_conditionalThen — `^` over `1`).
    let src = "f() {\n  false ? 1 : 2;\n}";
    assert_eq!(dead_of(src), vec!["1".to_string()]);
}

#[test]
fn conditional_dead_else_arm_reports_own_range() {
    // `true ? 1 : 2;` — the else arm's row (upstream
    // `flowEnd(elseExpression)`, type_analyzer.dart:2616).
    let src = "f() {\n  true ? 1 : 2;\n}";
    assert_eq!(dead_of(src), vec!["2".to_string()]);
}

#[test]
fn collection_if_dead_arm_reports_operand_only() {
    // `{1, if (false) 9 else await g, 3}` — the then element `9` is the
    // official's first dead node: the row anchors at the operand's first
    // token, NOT past it (`then_end` is the token preceding `else`, so
    // trivia never inflates the span).
    let src = "void f() async {\n  var s = {1, if (false) 9 else await g, 3};\n}";
    assert_eq!(dead_of(src), vec!["9".to_string()]);
}

// —— named-const conditions —————————————————————————————————————————————

#[test]
fn named_top_const_bool_if_is_silent() {
    // `const bool DEBUG = false; if (DEBUG) { ... }` — the official never
    // feeds a named element's value into `booleanLiteral`
    // (dead_code_test.dart test_deadBlock_if_debugConst_simpleIdentifier
    // expects silence).
    let src = "const bool DEBUG = false;\nvoid f() {\n  if (DEBUG) { print('a'); }\n}";
    assert_eq!(dead_of(src), Vec::<String>::new());
}

#[test]
fn named_top_const_bool_conditional_is_silent() {
    // `DEBUG ? 1 : 2;` over a named const — silent for the same reason
    // (dead_code_test.dart test_deadBlock_conditionalThen_debugConst).
    let src = "const bool DEBUG = false;\nf() {\n  DEBUG ? 1 : 2;\n}";
    assert_eq!(dead_of(src), Vec::<String>::new());
}

#[test]
fn named_top_const_bool_while_is_silent() {
    // `while (DEBUG) { ... }` — dead_code_test.dart
    // test_deadBlock_while_debugConst.
    let src = "const bool DEBUG = false;\nf() {\n  while (DEBUG) { print('a'); }\n}";
    assert_eq!(dead_of(src), Vec::<String>::new());
}

#[test]
fn local_final_bool_still_folds() {
    // The official DOES fold a local `final`/`const` read (the SSA node's
    // condition-variable state, flow_analysis.dart `_initialize`):
    // `final b = false; if (b) { ... }` rows the then block.
    let src = "void f() {\n  final b = false;\n  if (b) { print('a'); }\n}";
    assert_eq!(dead_of(src), vec!["{ print('a'); }".to_string()]);
}

// —— binary arm anchoring ———————————————————————————————————————————————

#[test]
fn and_arm_dead_entry_reports_operator_row() {
    // `false && x` — the right operand began dead and is the official's
    // first dead node; `flowEnd(rightOperand)` re-anchors at the operator
    // (dead_code_verifier.dart:281-283, :311-313).
    let src = "f() {\n  false && x;\n}";
    assert_eq!(dead_of(src), vec!["&& x".to_string()]);
}

#[test]
fn or_arm_dead_entry_reports_operator_row() {
    let src = "f() {\n  true || x;\n}";
    assert_eq!(dead_of(src), vec!["|| x".to_string()]);
}

#[test]
fn and_arm_in_dead_statement_folds() {
    // `false && x` inside a dead statement: the statement's poll already
    // opened the interval — the arm does not re-anchor.
    let src = "f() {\n  return;\n  false && x;\n}";
    assert_eq!(dead_of(src), vec!["false && x;".to_string()]);
}

#[test]
fn plus_rhs_dead_entry_reports_operator_row() {
    // `a + b` where `a` is `Never`-typed: the left operand's exit kills
    // the walk, the right operand is the first dead node, and
    // `flowEnd(binary)` reports `[operator, right.end)` —
    // `+ b` (dead_code_verifier.dart:281-283).
    let src = "f(Never a, int b) {\n  a + b;\n}";
    assert_eq!(dead_of(src), vec!["+ b".to_string()]);
}

#[test]
fn plus_in_dead_statement_folds() {
    // `return; a + b;` — the statement owns the whole span, not `+ b`.
    let src = "f() {\n  return;\n  a + b;\n}";
    assert_eq!(dead_of(src), vec!["a + b;".to_string()]);
}

#[test]
fn ifnull_rhs_dead_entry_reports_operator_row() {
    // `1 ?? x` — a guaranteed-non-null LHS deads the right operand;
    // `flowEnd` reports at `??` (dead_code_verifier.dart:311-313).
    let src = "f() {\n  1 ?? x;\n}";
    assert_eq!(dead_of(src), vec!["?? x".to_string()]);
}

#[test]
fn ifnull_in_dead_statement_folds() {
    let src = "f() {\n  return;\n  1 ?? x;\n}";
    assert_eq!(dead_of(src), vec!["1 ?? x;".to_string()]);
}

// —— arm-end exact spans ————————————————————————————————————————————————

#[test]
fn if_else_dead_block_reports_block_only() {
    // `if (true) {} else {}` — the else block's row covers exactly `{}`,
    // trivia before `else` excluded (dead_code_test.dart
    // test_deadBlock_else — `^^` over `{}`).
    let src = "f() {\n  if(true) {} else {}\n}";
    assert_eq!(dead_of(src), vec!["{}".to_string()]);
}

#[test]
fn dead_stmt_inside_dead_block_folds_into_block_row() {
    // The dead block's `flowEnd(Block)` trims to `statements.last` — the
    // inner dead statement folds into the block's own row.
    let src = "f() {\n  if(true) {} else {if (false) {}}\n}";
    assert_eq!(dead_of(src), vec!["{if (false) {}}".to_string()]);
}
