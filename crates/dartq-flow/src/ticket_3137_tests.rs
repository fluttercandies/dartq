//! #3137 — flow dead-window gates: the C-style `for` condition's post-death
//! anchor and the conditional's else-arm extent.
//!
//! Two upstream faces are pinned here.
//!
//! 1. A `for` condition that kills the flow with no node after the kill
//!    anchors nothing: the official parses the condition as a bare
//!    `Expression` (`ForParts.condition`, parser_impl.dart:10311-10321
//!    reusing `parseExpressionStatement` only as a token-level parse), so
//!    the header part's statement wrapper is never a visited node and the
//!    verifier's first dead node is the loop body
//!    (`dead_code_verifier.dart` `_visitNode`, :183-205). The updaters then
//!    get their own row (`for_updaterBegin`, :357-378) and the body's
//!    covering close its own (type_analyzer.dart:3192). Pins mirror
//!    `dead_code_test.dart`'s `test_flowEnd_forParts_condition_exists` and
//!    `test_flowEnd_forParts_condition_throw`.
//! 2. A conditional's dead else arm reports the ARM NODE's extent
//!    (`flowEnd(elseExpression)`, type_analyzer.dart:2616): the parser's
//!    returned end tiles a bare leaf (a read, a member chain) through its
//!    follower, so the row never swallows the statement's `;`. Compound
//!    arms keep their own last token unchanged.
use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// Dead-code facts of `source` as the exact source text they cover.
fn dead_of(source: &str) -> Vec<String> {
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
            FlowFact::DeadCode { span } => Some(
                source[span.range.offset as usize..(span.range.offset + span.range.length) as usize]
                    .to_string(),
            ),
            _ => None,
        })
        .collect()
}

// —— `for` condition deaths ————————————————————————————————————————————

#[test]
fn for_condition_throw_anchors_the_updaters_and_the_body() {
    // `for (; throw 0; 1) {}` — the condition dies with no node after the
    // kill, so the updaters fire their own row and the body block is the
    // first dead node (dead_code_test.dart
    // `test_flowEnd_forParts_condition_exists`: `1` and `{}`).
    let src = "void f() {\n  for (; throw 0; 1) {}\n}";
    assert_eq!(dead_of(src), vec!["1".to_string(), "{}".to_string()]);
}

#[test]
fn for_condition_death_after_a_node_keeps_that_node_as_the_anchor() {
    // `for (; g(throw 0, 1); 2) {}` — the first argument `1` is VISITED
    // after the throw's kill, so it is the official's first dead node and
    // owns one row through the body's close (dead_code_test.dart
    // `test_flowEnd_forParts_condition_throw`).
    let src = "void f(bool Function(Object?, Object?) g) {\n  for (; g(throw 0, 1); 2) {}\n}";
    assert_eq!(dead_of(src), vec!["1); 2) {}".to_string()]);
}

#[test]
fn for_condition_throw_reports_the_updater_list_then_the_body() {
    // `for (var i = 0; throw 0; i++) {}` — the dead updaters' own row
    // spans the whole updater list (`for_updaterBegin`,
    // dead_code_verifier.dart:357-378), and the body block follows.
    let src = "void f() {\n  for (var i = 0; throw 0; i++) {}\n}";
    assert_eq!(dead_of(src), vec!["i++".to_string(), "{}".to_string()]);
}

#[test]
fn for_initializer_throw_still_anchors_the_condition() {
    // `for (throw 0; true; 1) {}` — the condition BEGINS dead, so it is
    // the first dead node and the row trims to the updaters' last token
    // (`parent is ForParts`, dead_code_verifier.dart:324-326;
    // dead_code_test.dart `test_flowEnd_forParts_initializer_expression_throw`).
    let src = "void f() {\n  for (throw 0; true; 1) {}\n}";
    assert_eq!(dead_of(src), vec!["true; 1".to_string()]);
}

// —— conditional else-arm extents ——————————————————————————————————————

#[test]
fn conditional_dead_else_arm_ends_before_the_statement_terminator() {
    // `true ? 1 : b;` — the arm row is `b`, never `b;` (the parser's end
    // span tiles the bare read through its follower).
    let src = "void f(int b) { true ? 1 : b; }";
    assert_eq!(dead_of(src), vec!["b".to_string()]);
}

#[test]
fn conditional_dead_else_arm_property_access_ends_at_the_operand() {
    // `true ? p.x : p.y;` — a record property access is a bare leaf too
    // (dead_code_test.dart `test_deadBlock_conditionalElse_recordPropertyAccess`).
    let src = "void f(({int x, int y}) p) { true ? p.x : p.y; }";
    assert_eq!(dead_of(src), vec!["p.y".to_string()]);
}

#[test]
fn conditional_dead_else_arm_send_ends_at_the_closed_operand() {
    // `true ? 1 : s.length;` — the member send's own `)`/identifier ends
    // the row.
    let src = "void f(String s) { true ? 1 : s.length; }";
    assert_eq!(dead_of(src), vec!["s.length".to_string()]);
}

#[test]
fn conditional_dead_else_arm_compound_keeps_its_last_token() {
    // A binary, parenthesized, or send arm already ends on its own last
    // token: the arm's node extent is unchanged.
    let binary = "void f(int b) { true ? 1 : b + 1; }";
    assert_eq!(dead_of(binary), vec!["b + 1".to_string()]);
    let parens = "void f(int b) { true ? 1 : (b); }";
    assert_eq!(dead_of(parens), vec!["(b)".to_string()]);
    let send = "void f() { true ? 1 : g(); }";
    assert_eq!(dead_of(send), vec!["g()".to_string()]);
}

#[test]
fn conditional_dead_then_arm_stays_unchanged() {
    // `false ? b : 1;` — the then arm's own latch already ends it at the
    // colon boundary.
    let src = "void f(int b) { false ? b : 1; }";
    assert_eq!(dead_of(src), vec!["b".to_string()]);
}
