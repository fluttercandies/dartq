use dartq_ast::NodeKind;
use dartq_language::LanguageVersion;

use crate::testsupport::{Fixture, NONE, identifier, one_rule, tok, unit_of, utf16_at};

/// A legacy (`Feature.patterns` off, `<=2.19`) fixture — the library
/// version whose switch members are legacy `SwitchCase` shapes, the only
/// face `no_duplicate_case_values` processes.
fn legacy(mut f: Fixture) -> Fixture {
    f.features.patterns = false;
    f
}

/// `test/rules/curly_braces_in_flow_control_structures_test.dart`
/// `test_if_multiline` — a multi-line arm without a block reports.
#[test]
fn multiline_if_without_block_reports() {
    let source = "if (b)\n  x;";
    // if0 (1 b2 )3 x4 ;5
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::IfStatement, 0);
            b.patch(0, NONE);
            b.begin(NodeKind::ParenthesizedCondition, 1);
            b.patch(0, 1);
            b.patch(1, 3);
            b.end(NodeKind::ParenthesizedCondition, 4);
            b.begin(NodeKind::ExpressionStatement, 4);
            b.leaf_span(NodeKind::Send, 4, 5);
            b.end(NodeKind::ExpressionStatement, 6);
            b.end(NodeKind::IfStatement, 6);
        }),
    );
    let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Statements in an if should be enclosed in a block."
    );
    assert_eq!(lints[0].range.start, utf16_at(&f, 4));
}

/// The single-line form is allowed (:62-69).
#[test]
fn single_line_if_passes() {
    let source = "if (b) x;";
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::IfStatement, 0);
            b.patch(0, NONE);
            b.begin(NodeKind::ParenthesizedCondition, 1);
            b.patch(0, 1);
            b.patch(1, 3);
            b.end(NodeKind::ParenthesizedCondition, 4);
            b.begin(NodeKind::ExpressionStatement, 4);
            b.leaf_span(NodeKind::Send, 4, 5);
            b.end(NodeKind::ExpressionStatement, 6);
            b.end(NodeKind::IfStatement, 6);
        }),
    );
    assert!(
        f.run(&one_rule("curly_braces_in_flow_control_structures"))
            .is_empty()
    );
}

#[test]
fn block_arm_passes() {
    let source = "if (b) { x; }";
    // if0 (1 b2 )3 {4 x5 ;6 }7
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::IfStatement, 0);
            b.patch(0, NONE);
            b.begin(NodeKind::ParenthesizedCondition, 1);
            b.patch(0, 1);
            b.patch(1, 3);
            b.end(NodeKind::ParenthesizedCondition, 4);
            b.begin(NodeKind::Block, 5);
            b.begin(NodeKind::ExpressionStatement, 6);
            b.leaf_span(NodeKind::Send, 6, 8);
            b.end(NodeKind::ExpressionStatement, 8);
            b.end(NodeKind::Block, 8);
            b.end(NodeKind::IfStatement, 8);
        }),
    );
    assert!(
        f.run(&one_rule("curly_braces_in_flow_control_structures"))
            .is_empty()
    );
}

/// With an `else` both arms are checked unconditionally (:71-77).
#[test]
fn else_arm_reports_even_on_one_line() {
    let source = "if (b) {} else y;";
    // if0 (1 b2 )3 {4 }5 else6 y7 ;8
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::IfStatement, 0);
            b.patch(0, 6);
            b.begin(NodeKind::ParenthesizedCondition, 1);
            b.patch(0, 1);
            b.patch(1, 3);
            b.end(NodeKind::ParenthesizedCondition, 4);
            b.leaf_span(NodeKind::Block, 4, 6);
            b.begin(NodeKind::ExpressionStatement, 7);
            b.leaf_span(NodeKind::Send, 7, 8);
            b.end(NodeKind::ExpressionStatement, 9);
            b.end(NodeKind::IfStatement, 9);
        }),
    );
    let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, 7));
}

/// `while` bodies always need a block (`test_while`).
#[test]
fn while_body_reports() {
    let source = "while (b) ;";
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::WhileStatement, 0);
            identifier(b, 2, 3);
            b.leaf(NodeKind::EmptyStatement, 4);
            b.end(NodeKind::WhileStatement, 5);
        }),
    );
    let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Statements in a while should be enclosed in a block."
    );
}

/// `do` bodies always need a block (`test_do`).
#[test]
fn do_body_reports() {
    let source = "do x; while (b);";
    // do0 x1 ;2 while3 (4 b5 )6 ;7
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::DoWhileStatement, 0);
            b.begin(NodeKind::ExpressionStatement, 1);
            b.leaf_span(NodeKind::Send, 1, 2);
            b.end(NodeKind::ExpressionStatement, 3);
            identifier(b, 5, 6);
            b.end(NodeKind::DoWhileStatement, 8);
        }),
    );
    let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Statements in a do should be enclosed in a block."
    );
    assert_eq!(lints[0].range.start, utf16_at(&f, 1));
}

/// `for` bodies always need a block (`test_for`).
#[test]
fn for_body_reports() {
    let source = "for (;b;) ;";
    // for0 (1 ;2 b3 ;4 )5 ;6
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::ForStatement, 0);
            b.begin(NodeKind::ForParts, 1);
            b.patch(0, 1);
            b.patch(1, 2);
            b.patch(2, 4);
            b.patch(3, 0);
            identifier(b, 3, 4);
            b.end(NodeKind::ForParts, 6);
            b.leaf(NodeKind::EmptyStatement, 6);
            b.end(NodeKind::ForStatement, 7);
        }),
    );
    let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Statements in a for should be enclosed in a block."
    );
}

// —— real-parse arm selection ————————————————————————————————————————
//
// Production parses tile the condition into fragment rows ahead of the
// arms (the head `Identifier`, the `NoTypeArguments` leaf over the
// follower, the width-0 `NoArguments` anchor, the adopted
// `ParenthesizedCondition`), which is exactly what fixed child indexes
// used to report. These pins run through the production bridge.

/// Block-armed if with an `else` (the retired lint corpus's
/// always_put_control_body_on_new_line.dart:26 shape) — the oracle
/// emits nothing, and so does the structural arm read.
#[test]
fn else_block_arms_stay_silent_on_real_parse() {
    let source = "\
void f(bool c) {
  int x = 0;
  int y = 0;
  if (c) {
    x = 1;
  } else {
    y = 2;
  }
}
";
    let f = Fixture::parse(source);
    assert!(
        f.run(&one_rule("curly_braces_in_flow_control_structures"))
            .is_empty()
    );
}

/// `else if` chain with block arms (the retired lint corpus's
/// avoid_double_and_int_checks.dart:10 shape) — the else arm is an
/// IfStatement and is skipped, the then block passes, and the nested
/// if's own visit stays silent.
#[test]
fn else_if_block_arms_stay_silent_on_real_parse() {
    let source = "\
void f(num p) {
  if (p is double) {
    print(1);
  } else if (p is int) {
    print(2);
  }
}
";
    let f = Fixture::parse(source);
    assert!(
        f.run(&one_rule("curly_braces_in_flow_control_structures"))
            .is_empty()
    );
}

/// The `else ;` face (the retired lint corpus's flow_family.dart:15
/// shape) keeps reporting — at the else statement itself, matching the
/// oracle anchor (`reportAtNode(elseStatement)`).
#[test]
fn else_empty_statement_reports_at_the_semicolon() {
    let source = "\
void f(bool flag) {
  if (flag) {
    print(0);
  } else ;
}
";
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Statements in an if should be enclosed in a block."
    );
    let semi = tok(source, &f.tokens, ";", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, semi));
    assert_eq!(lints[0].range.end, lints[0].range.start + 1);
}

/// A line-spanning brace-less then arm reports at the arm statement
/// (`test_if_multiline` through the production bridge — the anchor is
/// the statement, never a condition fragment).
#[test]
fn multiline_braceless_then_reports_at_the_statement() {
    let source = "\
void f(bool b) {
  if (b)
    x;
}
";
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Statements in an if should be enclosed in a block."
    );
    let x = tok(source, &f.tokens, "x", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, x));
    assert_eq!(lints[0].range.end, lints[0].range.start + 2);
}

// —— control_flow_in_finally ————————————————————————————————————————

use crate::testsupport::tok as tok_at;

/// `try {} finally { break; }` — a bare break in a finally reports
/// (`test/rules/control_flow_in_finally_test.dart` `test_break`).
#[test]
fn finally_break_reports() {
    let source = "try {} finally { break; }";
    // try0 {1 }2 finally3 {4 break5 ;6 }7
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::TryStatement, 0);
            b.patch(0, tok_at(source, t, "finally", 0));
            b.leaf_span(NodeKind::Block, 1, 3);
            b.begin(NodeKind::Block, 4);
            b.begin(NodeKind::Break, 5);
            b.patch(0, 0);
            b.end(NodeKind::Break, 7);
            b.end(NodeKind::Block, 8);
            b.end(NodeKind::TryStatement, 8);
        }),
    );
    let lints = f.run(&one_rule("control_flow_in_finally"));
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].message, "Use of 'break' in a 'finally' clause.");
    assert_eq!(lints[0].range.start, utf16_at(&f, 5));
    // Upstream reports the whole statement, semicolon included
    // (`test_break` marks `[!break;!]`); the hand-built span already
    // carries it ([5, 7) — `break ;`).
    assert_eq!(lints[0].range.end, utf16_at(&f, 6) + 1);
}

/// Production-bridge pin: the production rows anchor Break/Continue at
/// the keyword alone, and the report still extends through the `;`
/// (`test_break` / `test_continue` markup). ASCII sources — UTF-16
/// offsets equal byte offsets, so `rfind` anchors read directly.
#[test]
fn finally_jump_reports_through_the_semicolon() {
    let f = Fixture::parse("void f() {\n  for (;;) {\n    try {} finally { break; }\n  }\n}\n");
    let lints = f.run(&one_rule("control_flow_in_finally"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let source = &f.source;
    let semi = source.rfind(';').expect("statement semicolon");
    let start = source.rfind("break").expect("break keyword");
    assert_eq!(lints[0].range.end as usize, semi + 1);
    assert_eq!(lints[0].range.start as usize, start);

    let f = Fixture::parse("void f() {\n  for (;;) {\n    try {} finally { continue; }\n  }\n}\n");
    let lints = f.run(&one_rule("control_flow_in_finally"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let semi = f.source.rfind(';').expect("statement semicolon");
    assert_eq!(lints[0].range.end as usize, semi + 1);
}

/// A loop inside the finally keeps the break local (:79-91 shape).
#[test]
fn finally_break_into_local_loop_passes() {
    let source = "try {} finally { for (;;) { break; } }";
    // try0 {1 }2 finally3 {4 for5 (6 ;7 ;8 )9 {10 break11 ;12 }13 }14
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::TryStatement, 0);
            b.patch(0, tok_at(source, t, "finally", 0));
            b.leaf_span(NodeKind::Block, 1, 3);
            b.begin(NodeKind::Block, 4);
            b.begin(NodeKind::ForStatement, 5);
            b.begin(NodeKind::ForParts, 6);
            b.patch(0, 6);
            b.patch(1, 7);
            b.patch(2, 8);
            b.patch(3, 0);
            b.end(NodeKind::ForParts, 10);
            b.begin(NodeKind::Block, 10);
            b.begin(NodeKind::Break, 11);
            b.patch(0, 0);
            b.end(NodeKind::Break, 13);
            b.end(NodeKind::Block, 14);
            b.end(NodeKind::ForStatement, 14);
            b.end(NodeKind::Block, 15);
            b.end(NodeKind::TryStatement, 15);
        }),
    );
    assert!(f.run(&one_rule("control_flow_in_finally")).is_empty());
}

/// A loop outside the try does not keep the break local — it reports.
#[test]
fn finally_break_out_of_enclosing_loop_reports() {
    let source = "for (;;) { try {} finally { break; } }";
    // for0 (1 ;2 ;3 )4 {5 try6 {7 }8 finally9 {10 break11 ;12 }13 }14
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::ForStatement, 0);
            b.begin(NodeKind::ForParts, 1);
            b.patch(0, 2);
            b.patch(1, 2);
            b.patch(2, 3);
            b.patch(3, 0);
            b.end(NodeKind::ForParts, 5);
            b.begin(NodeKind::Block, 5);
            b.begin(NodeKind::TryStatement, 6);
            b.patch(0, tok_at(source, t, "finally", 0));
            b.leaf_span(NodeKind::Block, 7, 9);
            b.begin(NodeKind::Block, 10);
            b.begin(NodeKind::Break, 11);
            b.patch(0, 0);
            b.end(NodeKind::Break, 13);
            b.end(NodeKind::Block, 14);
            b.end(NodeKind::TryStatement, 14);
            b.end(NodeKind::Block, 15);
            b.end(NodeKind::ForStatement, 15);
        }),
    );
    let lints = f.run(&one_rule("control_flow_in_finally"));
    assert_eq!(lints.len(), 1);
}

/// A closure inside the finally keeps the break local (:69-77).
#[test]
fn finally_break_inside_closure_passes() {
    let source = "try {} finally { f(() { break; }); }";
    // try0 {1 }2 finally3 {4 f5 (6 (7 )8 {9 break10 ;11 }12 )13 ;14 }15
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::TryStatement, 0);
            b.patch(0, tok_at(source, t, "finally", 0));
            b.leaf_span(NodeKind::Block, 1, 3);
            b.begin(NodeKind::Block, 4);
            b.begin(NodeKind::ExpressionStatement, 5);
            b.begin(NodeKind::Send, 5);
            identifier(b, 5, 6);
            b.begin(NodeKind::Arguments, 6);
            b.begin(NodeKind::FunctionExpression, 7);
            b.begin(NodeKind::Block, 9);
            b.begin(NodeKind::Break, 10);
            b.patch(0, 0);
            b.end(NodeKind::Break, 12);
            b.end(NodeKind::Block, 13);
            b.end(NodeKind::FunctionExpression, 13);
            b.end(NodeKind::Arguments, 14);
            b.end(NodeKind::Send, 14);
            b.end(NodeKind::ExpressionStatement, 15);
            b.end(NodeKind::Block, 16);
            b.end(NodeKind::TryStatement, 16);
        }),
    );
    assert!(f.run(&one_rule("control_flow_in_finally")).is_empty());
}

/// `return` in a finally reports (`test_return`).
#[test]
fn finally_return_reports() {
    let source = "try {} finally { return; }";
    // try0 {1 }2 finally3 {4 return5 ;6 }7
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::TryStatement, 0);
            b.patch(0, tok_at(source, t, "finally", 0));
            b.leaf_span(NodeKind::Block, 1, 3);
            b.begin(NodeKind::Block, 4);
            b.leaf(NodeKind::ReturnStatement, 5);
            b.end(NodeKind::Block, 7);
            b.end(NodeKind::TryStatement, 7);
        }),
    );
    let lints = f.run(&one_rule("control_flow_in_finally"));
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].message, "Use of 'return' in a 'finally' clause.");
}

/// A try without a finally never reports.
#[test]
fn try_without_finally_passes() {
    let source = "try { break; } catch (e) {}";
    // try0 {1 break2 ;3 }4 catch5 (6 e7 )8 {9 }10
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::TryStatement, 0);
            b.patch(0, NONE);
            b.begin(NodeKind::Block, 1);
            b.begin(NodeKind::Break, 2);
            b.patch(0, 0);
            b.end(NodeKind::Break, 4);
            b.end(NodeKind::Block, 5);
            b.begin(NodeKind::CatchClause, 5);
            b.patch(0, tok_at(source, t, "catch", 0));
            b.patch(1, NONE);
            b.end(NodeKind::CatchClause, 11);
            b.end(NodeKind::TryStatement, 11);
        }),
    );
    assert!(f.run(&one_rule("control_flow_in_finally")).is_empty());
}

/// A labeled loop outside the try: the break's resolved target (the
/// loop the label binds, `visitLabeledStatement` :631-639) wraps the
/// try, so the jump crosses the finally — reports.
#[test]
fn finally_labeled_break_outside_reports() {
    let source = "loop: for (;;) { try {} finally { break loop; } }";
    // loop0 :1 for2 (3 ;4 ;5 )6 {7 try8 {9 }10 finally11 {12 break13
    // loop14 ;15 }16 }17
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::Label, 0);
            b.end(NodeKind::Label, 2);
            b.begin_adopting(NodeKind::LabeledStatement, 0, 1);
            b.begin(NodeKind::ForStatement, 2);
            b.begin(NodeKind::ForParts, 3);
            b.patch(0, 3);
            b.patch(1, 4);
            b.patch(2, 5);
            b.patch(3, 0);
            b.end(NodeKind::ForParts, 7);
            b.begin(NodeKind::Block, 7);
            b.begin(NodeKind::TryStatement, 8);
            b.patch(0, tok_at(source, t, "finally", 0));
            b.leaf_span(NodeKind::Block, 9, 11);
            b.begin(NodeKind::Block, 12);
            b.begin(NodeKind::Break, 13);
            b.patch(0, 1);
            b.end(NodeKind::Break, 16);
            b.end(NodeKind::Block, 17);
            b.end(NodeKind::TryStatement, 17);
            b.end(NodeKind::Block, 18);
            b.end(NodeKind::ForStatement, 18);
            b.end(NodeKind::LabeledStatement, 18);
        }),
    );
    let lints = f.run(&one_rule("control_flow_in_finally"));
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, 13));
}

/// A label binding a loop inside the finally block itself: the target
/// sits below the try (its own chain contains it) — no report.
#[test]
fn finally_labeled_loop_inside_finally_passes() {
    let source = "try {} finally { inner: for (;;) { break inner; } }";
    // try0 {1 }2 finally3 {4 inner5 :6 for7 (8 ;9 ;10 )11 {12 break13
    // inner14 ;15 }16 }17
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::TryStatement, 0);
            b.patch(0, tok_at(source, t, "finally", 0));
            b.leaf_span(NodeKind::Block, 1, 3);
            b.begin(NodeKind::Block, 4);
            b.begin(NodeKind::Label, 5);
            b.end(NodeKind::Label, 7);
            b.begin_adopting(NodeKind::LabeledStatement, 5, 1);
            b.begin(NodeKind::ForStatement, 7);
            b.begin(NodeKind::ForParts, 8);
            b.patch(0, 8);
            b.patch(1, 9);
            b.patch(2, 10);
            b.patch(3, 0);
            b.end(NodeKind::ForParts, 12);
            b.begin(NodeKind::Block, 12);
            b.begin(NodeKind::Break, 13);
            b.patch(0, 1);
            b.end(NodeKind::Break, 16);
            b.end(NodeKind::Block, 17);
            b.end(NodeKind::ForStatement, 17);
            b.end(NodeKind::LabeledStatement, 17);
            b.end(NodeKind::Block, 18);
            b.end(NodeKind::TryStatement, 18);
        }),
    );
    assert!(f.run(&one_rule("control_flow_in_finally")).is_empty());
}

/// Nested labels wrapping the try: both labels bind the same loop
/// through the label scope, which encloses the try — the official
/// target lookup reports.
#[test]
fn finally_nested_labels_wrapping_try_report() {
    let source = "a: b: for (;;) { try {} finally { break a; } }";
    // a0 :1 b2 :3 for4 (5 ;6 ;7 )8 {9 try10 {11 }12 finally13 {14
    // break15 a16 ;17 }18 }19
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::Label, 0);
            b.end(NodeKind::Label, 2);
            b.begin(NodeKind::Label, 2);
            b.end(NodeKind::Label, 4);
            b.begin_adopting(NodeKind::LabeledStatement, 0, 2);
            b.begin(NodeKind::ForStatement, 4);
            b.begin(NodeKind::ForParts, 5);
            b.patch(0, 5);
            b.patch(1, 6);
            b.patch(2, 7);
            b.patch(3, 0);
            b.end(NodeKind::ForParts, 9);
            b.begin(NodeKind::Block, 9);
            b.begin(NodeKind::TryStatement, 10);
            b.patch(0, tok_at(source, t, "finally", 0));
            b.leaf_span(NodeKind::Block, 11, 13);
            b.begin(NodeKind::Block, 14);
            b.begin(NodeKind::Break, 15);
            b.patch(0, 1);
            b.end(NodeKind::Break, 18);
            b.end(NodeKind::Block, 19);
            b.end(NodeKind::TryStatement, 19);
            b.end(NodeKind::Block, 20);
            b.end(NodeKind::ForStatement, 20);
            b.end(NodeKind::LabeledStatement, 20);
        }),
    );
    let lints = f.run(&one_rule("control_flow_in_finally"));
    assert_eq!(lints.len(), 1);
}

/// The resolution the label walk closes: a loop inside the finally is
/// the nearest enclosing loop but not the resolved target — `break
/// outer` binds the outer loop, whose chain lacks the try — reports
/// (the nearest-loop heuristic kept this silent).
#[test]
fn finally_inner_loop_does_not_mask_labeled_target() {
    let source = "outer: for (;;) { try {} finally { for (;;) { break outer; } } }";
    // outer0 :1 for2 (3 ;4 ;5 )6 {7 try8 {9 }10 finally11 {12 for13 (14
    // ;15 ;16 )17 {18 break19 outer20 ;21 }22 }23 }24
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::Label, 0);
            b.end(NodeKind::Label, 2);
            b.begin_adopting(NodeKind::LabeledStatement, 0, 1);
            b.begin(NodeKind::ForStatement, 2);
            b.begin(NodeKind::ForParts, 3);
            b.patch(0, 3);
            b.patch(1, 4);
            b.patch(2, 5);
            b.patch(3, 0);
            b.end(NodeKind::ForParts, 7);
            b.begin(NodeKind::Block, 7);
            b.begin(NodeKind::TryStatement, 8);
            b.patch(0, tok_at(source, t, "finally", 0));
            b.leaf_span(NodeKind::Block, 9, 11);
            b.begin(NodeKind::Block, 12);
            b.begin(NodeKind::ForStatement, 13);
            b.begin(NodeKind::ForParts, 14);
            b.patch(0, 14);
            b.patch(1, 15);
            b.patch(2, 16);
            b.patch(3, 0);
            b.end(NodeKind::ForParts, 18);
            b.begin(NodeKind::Block, 18);
            b.begin(NodeKind::Break, 19);
            b.patch(0, 1);
            b.end(NodeKind::Break, 22);
            b.end(NodeKind::Block, 23);
            b.end(NodeKind::ForStatement, 23);
            b.end(NodeKind::Block, 24);
            b.end(NodeKind::TryStatement, 24);
            b.end(NodeKind::Block, 25);
            b.end(NodeKind::ForStatement, 25);
            b.end(NodeKind::LabeledStatement, 25);
        }),
    );
    let lints = f.run(&one_rule("control_flow_in_finally"));
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, 19));
}

/// A closure resets the unlabeled-break context chain (:224), so a bare
/// break inside one has no target even with loops outside — the
/// `FunctionBody` exemption (:69-77) keeps it silent (the nearest-loop
/// heuristic reported this).
#[test]
fn finally_closure_break_with_outer_loop_passes() {
    let source = "for (;;) { try {} finally { f(() { break; }); } }";
    // for0 (1 ;2 ;3 )4 {5 try6 {7 }8 finally9 {10 f11 (12 (13 )14 {15
    // break16 ;17 }18 )19 ;20 }21 }22
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::ForStatement, 0);
            b.begin(NodeKind::ForParts, 1);
            b.patch(0, 1);
            b.patch(1, 2);
            b.patch(2, 3);
            b.patch(3, 0);
            b.end(NodeKind::ForParts, 5);
            b.begin(NodeKind::Block, 5);
            b.begin(NodeKind::TryStatement, 6);
            b.patch(0, tok_at(source, t, "finally", 0));
            b.leaf_span(NodeKind::Block, 7, 9);
            b.begin(NodeKind::Block, 10);
            b.begin(NodeKind::ExpressionStatement, 11);
            b.begin(NodeKind::Send, 11);
            identifier(b, 11, 12);
            b.begin(NodeKind::Arguments, 12);
            b.begin(NodeKind::FunctionExpression, 13);
            b.begin(NodeKind::Block, 15);
            b.begin(NodeKind::Break, 16);
            b.patch(0, 0);
            b.end(NodeKind::Break, 18);
            b.end(NodeKind::Block, 19);
            b.end(NodeKind::FunctionExpression, 19);
            b.end(NodeKind::Arguments, 20);
            b.end(NodeKind::Send, 20);
            b.end(NodeKind::ExpressionStatement, 21);
            b.end(NodeKind::Block, 22);
            b.end(NodeKind::TryStatement, 22);
            b.end(NodeKind::Block, 23);
            b.end(NodeKind::ForStatement, 23);
        }),
    );
    assert!(f.run(&one_rule("control_flow_in_finally")).is_empty());
}

// —— no_duplicate_case_values —————————————————————————————————————————

/// The default-version face (official `test_duplicateIntConstant_ok`/
/// `test_duplicateStringConstant_ok`/`test_duplicateConstClassValue_ok`/
/// `test_duplicateEnumValue_ok`): with `Feature.patterns` on (3.0+) every
/// member is a pattern case and the lint is silent — even on repeated
/// constant values.
#[test]
fn duplicate_case_value_silent_at_default_version() {
    let source = "switch (x) { case 1: break; case 1: break; }";
    // switch0 (1 x2 )3 {4 case5 16 :7 break8 ;9 case10 111 :12 break13 ;14 }15
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::SwitchStatement, 0);
            b.begin(NodeKind::SwitchBlock, 4);
            for (case_head, int_tok, colon) in [(5, 6, 7), (10, 11, 12)] {
                b.begin(NodeKind::SwitchCase, case_head);
                b.patch(0, NONE);
                b.patch(1, colon);
                b.patch(2, 1); // one matched expression
                b.begin(NodeKind::CaseExpression, case_head);
                b.patch(0, NONE);
                b.patch(1, colon);
                b.leaf(NodeKind::LiteralInt, int_tok);
                b.begin_adopting(NodeKind::ConstantPattern, int_tok, 1);
                b.end(NodeKind::ConstantPattern, int_tok + 1);
                b.end(NodeKind::CaseExpression, colon + 1);
                b.begin(NodeKind::Break, int_tok + 2);
                b.patch(0, 0);
                b.end(NodeKind::Break, int_tok + 4);
                b.end(NodeKind::SwitchCase, int_tok + 4);
            }
            b.end(NodeKind::SwitchBlock, 16);
            b.end(NodeKind::SwitchStatement, 16);
        }),
    );
    assert!(f.run(&one_rule("no_duplicate_case_values")).is_empty());
}

/// `test/rules/no_duplicate_case_values_test.dart` `test_duplicateCase`
/// (`NoDuplicateCaseValuesTestLanguage219`) — in a legacy (`patterns`
/// off) library the duplicate reports with both sources in the message.
#[test]
fn duplicate_case_value_reports_legacy() {
    let source = "switch (x) { case 1: break; case 1: break; }";
    // switch0 (1 x2 )3 {4 case5 16 :7 break8 ;9 case10 111 :12 break13 ;14 }15
    let f = legacy(Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::SwitchStatement, 0);
            b.begin(NodeKind::SwitchBlock, 4);
            for (case_head, int_tok, colon) in [(5, 6, 7), (10, 11, 12)] {
                b.begin(NodeKind::SwitchCase, case_head);
                b.patch(0, NONE);
                b.patch(1, colon);
                b.patch(2, 1); // one matched expression
                b.begin(NodeKind::CaseExpression, case_head);
                b.patch(0, NONE);
                b.patch(1, colon);
                b.leaf(NodeKind::LiteralInt, int_tok);
                b.begin_adopting(NodeKind::ConstantPattern, int_tok, 1);
                b.end(NodeKind::ConstantPattern, int_tok + 1);
                b.end(NodeKind::CaseExpression, colon + 1);
                b.begin(NodeKind::Break, int_tok + 2);
                b.patch(0, 0);
                b.end(NodeKind::Break, int_tok + 4);
                b.end(NodeKind::SwitchCase, int_tok + 4);
            }
            b.end(NodeKind::SwitchBlock, 16);
            b.end(NodeKind::SwitchStatement, 16);
        }),
    ));
    let lints = f.run(&one_rule("no_duplicate_case_values"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "The value of the case clause ('1') is equal to the value of an earlier case clause ('1')."
    );
    assert_eq!(
        lints[0].correction,
        Some("Try removing or changing the value.".to_owned())
    );
    assert_eq!(lints[0].range.start, utf16_at(&f, 11));
}

/// The deepest prefix run the parser fuse lets through (`MAX_DEPTH` − 1
/// unary operators of AST nesting) still folds and still dedupes — the
/// fold's cap must never silence a legit span, only the recovered/
/// pathological runs past it. The minuses are space-separated: an
/// unbroken `--` run lexes as `MinusMinus` (decrement) tokens, not a
/// unary chain — `case --1:` is a syntax error upstream, and the fold
/// already treats that spelling as unknown.
#[test]
fn unary_chain_under_cap_still_dedupes() {
    let run = "- ".repeat(499);
    let source = format!("switch (x) {{ case {run}1: break; case {run}1: break; }}");
    // switch0 (1 x2 )3 {4 case5 -(6..504) 1505 :506 break507 ;508
    // case509 -(510..1008) 11009 :1010 break1011 ;1012 }1013
    let f = legacy(Fixture::build(
        &source,
        unit_of(|b, _t| {
            b.begin(NodeKind::SwitchStatement, 0);
            b.begin(NodeKind::SwitchBlock, 4);
            for (head, one, colon) in [(5u32, 505, 506), (509, 1009, 1010)] {
                b.begin(NodeKind::SwitchCase, head);
                b.patch(0, NONE);
                b.patch(1, colon);
                b.patch(2, 1);
                b.begin(NodeKind::CaseExpression, head);
                b.patch(0, NONE);
                b.patch(1, colon);
                b.leaf(NodeKind::LiteralInt, one);
                b.begin_adopting(NodeKind::ConstantPattern, one - 499, 1);
                b.end(NodeKind::ConstantPattern, one + 1);
                b.end(NodeKind::CaseExpression, colon + 1);
                b.begin(NodeKind::Break, colon + 1);
                b.patch(0, 0);
                b.end(NodeKind::Break, colon + 3);
                b.end(NodeKind::SwitchCase, colon + 3);
            }
            b.end(NodeKind::SwitchBlock, 1014);
            b.end(NodeKind::SwitchStatement, 1014);
        }),
    ));
    let lints = f.run(&one_rule("no_duplicate_case_values"));
    assert_eq!(lints.len(), 1);
    let operand = format!("{run}1");
    assert_eq!(
        lints[0].message,
        format!(
            "The value of the case clause ('{operand}') is equal to the value of an earlier case clause ('{operand}')."
        )
    );
    assert_eq!(lints[0].range.start, utf16_at(&f, 510));
}

/// A pathological prefix run far past the fold's recursion cap: the
/// walk descends a bounded 501 frames, bails to the unknown-value path,
/// and the rule stays silent — no stack overflow, no invented
/// diagnostic (without the cap this span is a ~100k-frame recursion,
/// past any thread stack). The minuses are space-separated so each is a
/// real unary `-` token (an unbroken run lexes as `MinusMinus`
/// decrements, unknown to the fold for the independent syntax-error
/// reason).
#[test]
fn unary_chain_past_depth_cap_stays_silent() {
    let source = format!("switch (x) {{ case {}1: break; }}", "- ".repeat(100_000));
    // switch0 (1 x2 )3 {4 case5 -(6..100005) 1100006 :100007 break100008
    // ;100009 }100010
    let (one, colon) = (100_006u32, 100_007);
    let f = legacy(Fixture::build(
        &source,
        unit_of(|b, _t| {
            b.begin(NodeKind::SwitchStatement, 0);
            b.begin(NodeKind::SwitchBlock, 4);
            b.begin(NodeKind::SwitchCase, 5);
            b.patch(0, NONE);
            b.patch(1, colon);
            b.patch(2, 1);
            b.begin(NodeKind::CaseExpression, 5);
            b.patch(0, NONE);
            b.patch(1, colon);
            b.leaf(NodeKind::LiteralInt, one);
            b.begin_adopting(NodeKind::ConstantPattern, 6, 1);
            b.end(NodeKind::ConstantPattern, one + 1);
            b.end(NodeKind::CaseExpression, colon + 1);
            b.begin(NodeKind::Break, colon + 1);
            b.patch(0, 0);
            b.end(NodeKind::Break, colon + 3);
            b.end(NodeKind::SwitchCase, colon + 3);
            b.end(NodeKind::SwitchBlock, one + 5);
            b.end(NodeKind::SwitchStatement, one + 5);
        }),
    ));
    assert!(f.run(&one_rule("no_duplicate_case_values")).is_empty());
}

/// Distinct values and a `default:` clause (no expressions) pass — the
/// legacy (`Language219`) face.
#[test]
fn distinct_and_default_cases_pass() {
    let source = "switch (x) { case 1: break; case 2: break; default: break; }";
    // switch0 (1 x2 )3 {4 case5 16 :7 break8 ;9 case10 211 :12 break13 ;14
    // default15 :16 break17 ;18 }19
    let f = legacy(Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::SwitchStatement, 0);
            b.begin(NodeKind::SwitchBlock, 4);
            // case 1
            b.begin(NodeKind::SwitchCase, 5);
            b.patch(0, NONE);
            b.patch(1, 7);
            b.patch(2, 1);
            b.begin(NodeKind::CaseExpression, 5);
            b.patch(0, NONE);
            b.patch(1, 7);
            b.leaf(NodeKind::LiteralInt, 6);
            b.begin_adopting(NodeKind::ConstantPattern, 6, 1);
            b.end(NodeKind::ConstantPattern, 7);
            b.end(NodeKind::CaseExpression, 8);
            b.begin(NodeKind::Break, 8);
            b.patch(0, 0);
            b.end(NodeKind::Break, 10);
            b.end(NodeKind::SwitchCase, 10);
            // case 2
            b.begin(NodeKind::SwitchCase, 10);
            b.patch(0, NONE);
            b.patch(1, 12);
            b.patch(2, 1);
            b.begin(NodeKind::CaseExpression, 10);
            b.patch(0, NONE);
            b.patch(1, 12);
            b.leaf(NodeKind::LiteralInt, 11);
            b.begin_adopting(NodeKind::ConstantPattern, 11, 1);
            b.end(NodeKind::ConstantPattern, 12);
            b.end(NodeKind::CaseExpression, 13);
            b.begin(NodeKind::Break, 13);
            b.patch(0, 0);
            b.end(NodeKind::Break, 15);
            b.end(NodeKind::SwitchCase, 15);
            // default (no expressions)
            b.begin(NodeKind::SwitchCase, 15);
            b.patch(0, 15);
            b.patch(1, 16);
            b.patch(2, 0);
            b.begin(NodeKind::Break, 17);
            b.patch(0, 0);
            b.end(NodeKind::Break, 19);
            b.end(NodeKind::SwitchCase, 19);
            b.end(NodeKind::SwitchBlock, 20);
            b.end(NodeKind::SwitchStatement, 20);
        }),
    ));
    assert!(f.run(&one_rule("no_duplicate_case_values")).is_empty());
}

/// The official keys by computed constant (`computeConstantValue`, :44):
/// `case 1 + 1:` collides with `case 2:` — the later expression reports
/// with both sources (legacy face).
#[test]
fn computed_duplicate_case_values_report() {
    let source = "switch (x) { case 1 + 1: break; case 2: break; }";
    // switch0 (1 x2 )3 {4 case5 16 +7 18 :9 break10 ;11 case12 213 :14
    // break15 ;16 }17
    let f = legacy(Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::SwitchStatement, 0);
            b.begin(NodeKind::SwitchBlock, 4);
            b.begin(NodeKind::SwitchCase, 5);
            b.patch(0, NONE);
            b.patch(1, 9);
            b.patch(2, 1);
            b.begin(NodeKind::CaseExpression, 5);
            b.patch(0, NONE);
            b.patch(1, 9);
            b.leaf(NodeKind::LiteralInt, 6);
            b.leaf(NodeKind::LiteralInt, 8);
            b.begin_adopting(NodeKind::BinaryExpression, 6, 2);
            b.patch(0, 7);
            b.end(NodeKind::BinaryExpression, 9);
            b.leaf(NodeKind::ConstantPattern, 6);
            b.end(NodeKind::CaseExpression, 10);
            b.begin(NodeKind::Break, 10);
            b.patch(0, 0);
            b.end(NodeKind::Break, 12);
            b.end(NodeKind::SwitchCase, 12);
            b.begin(NodeKind::SwitchCase, 12);
            b.patch(0, NONE);
            b.patch(1, 14);
            b.patch(2, 1);
            b.begin(NodeKind::CaseExpression, 12);
            b.patch(0, NONE);
            b.patch(1, 14);
            b.leaf(NodeKind::ConstantPattern, 13);
            b.end(NodeKind::CaseExpression, 15);
            b.begin(NodeKind::Break, 15);
            b.patch(0, 0);
            b.end(NodeKind::Break, 17);
            b.end(NodeKind::SwitchCase, 17);
            b.end(NodeKind::SwitchBlock, 18);
            b.end(NodeKind::SwitchStatement, 18);
        }),
    ));
    let lints = f.run(&one_rule("no_duplicate_case_values"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "The value of the case clause ('2') is equal to the value of an earlier case clause ('1 + 1')."
    );
    assert_eq!(lints[0].range.start, utf16_at(&f, 13));
}

/// String concatenation folds: `case 'a' + 'b':` collides with
/// `case 'ab':` (legacy face).
#[test]
fn string_concatenation_duplicate_cases_report() {
    let source = "switch (x) { case 'a' + 'b': break; case 'ab': break; }";
    // switch0 (1 x2 )3 {4 case5 'a'6 +7 'b'8 :9 break10 ;11 case12
    // 'ab'13 :14 break15 ;16 }17
    let f = legacy(Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::SwitchStatement, 0);
            b.begin(NodeKind::SwitchBlock, 4);
            b.begin(NodeKind::SwitchCase, 5);
            b.patch(0, NONE);
            b.patch(1, 9);
            b.patch(2, 1);
            b.begin(NodeKind::CaseExpression, 5);
            b.patch(0, NONE);
            b.patch(1, 9);
            b.leaf(NodeKind::StringLiteral, 6);
            b.leaf(NodeKind::StringLiteral, 8);
            b.begin_adopting(NodeKind::BinaryExpression, 6, 2);
            b.patch(0, 7);
            b.end(NodeKind::BinaryExpression, 9);
            b.leaf(NodeKind::ConstantPattern, 6);
            b.end(NodeKind::CaseExpression, 10);
            b.begin(NodeKind::Break, 10);
            b.patch(0, 0);
            b.end(NodeKind::Break, 12);
            b.end(NodeKind::SwitchCase, 12);
            b.begin(NodeKind::SwitchCase, 12);
            b.patch(0, NONE);
            b.patch(1, 14);
            b.patch(2, 1);
            b.begin(NodeKind::CaseExpression, 12);
            b.patch(0, NONE);
            b.patch(1, 14);
            b.leaf(NodeKind::ConstantPattern, 13);
            b.end(NodeKind::CaseExpression, 15);
            b.begin(NodeKind::Break, 15);
            b.patch(0, 0);
            b.end(NodeKind::Break, 17);
            b.end(NodeKind::SwitchCase, 17);
            b.end(NodeKind::SwitchBlock, 18);
            b.end(NodeKind::SwitchStatement, 18);
        }),
    ));
    let lints = f.run(&one_rule("no_duplicate_case_values"));
    assert_eq!(lints.len(), 1);
    // The official `toString()` arguments carry the source spelling,
    // quotes included (the template's `('…')` wrap doubles them).
    assert_eq!(
        lints[0].message,
        "The value of the case clause (''ab'') is equal to the value of an earlier case clause (''a' + 'b'')."
    );
}

/// Expressions outside the folder's grammar evaluate unknown and never
/// dedupe (the official `hasKnownValue` skip): repeated non-constant
/// identifiers stay silent (legacy face).
#[test]
fn non_constant_case_expressions_pass() {
    let source = "switch (x) { case y: break; case y: break; }";
    // switch0 (1 x2 )3 {4 case5 y6 :7 break8 ;9 case10 y11 :12 break13
    // ;14 }15
    let f = legacy(Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::SwitchStatement, 0);
            b.begin(NodeKind::SwitchBlock, 4);
            for (case_head, y_tok, colon) in [(5, 6, 7), (10, 11, 12)] {
                b.begin(NodeKind::SwitchCase, case_head);
                b.patch(0, NONE);
                b.patch(1, colon);
                b.patch(2, 1);
                b.begin(NodeKind::CaseExpression, case_head);
                b.patch(0, NONE);
                b.patch(1, colon);
                identifier(b, y_tok, y_tok + 1);
                b.begin_adopting(NodeKind::ConstantPattern, y_tok, 1);
                b.end(NodeKind::ConstantPattern, y_tok + 1);
                b.end(NodeKind::CaseExpression, colon + 1);
                b.begin(NodeKind::Break, colon + 1);
                b.patch(0, 0);
                b.end(NodeKind::Break, colon + 3);
                b.end(NodeKind::SwitchCase, colon + 3);
            }
            b.end(NodeKind::SwitchBlock, 16);
            b.end(NodeKind::SwitchStatement, 16);
        }),
    ));
    assert!(f.run(&one_rule("no_duplicate_case_values")).is_empty());
}

/// A for-in body is the official `ForStatement.body` face (`for (e in xs)
/// stmt;`): the non-block body reports "a for" at the statement.
#[test]
fn for_in_non_block_body_reports() {
    let source = "void f(List<int> l) {\n  for (final a in l) print(a);\n}";
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Statements in a for should be enclosed in a block."
    );
    // ASCII source: the byte offset of the body statement is its utf16
    // column face.
    assert_eq!(
        lints[0].range.start,
        source.find("print(a);").unwrap() as u32
    );
    assert_eq!(
        lints[0].range.end,
        (source.find("print(a);").unwrap() + 9) as u32
    );
}

/// With an `else`, each arm reports exactly once — the then arm's
/// selection reads the window geometrically (the production tiling
/// spills the else side's inner rows around its leaf, where arena
/// adjacency lands on the wrong row).
#[test]
fn else_arms_report_each_once() {
    let source = "void f(bool c) {\n  if (c) print(1); else print(2);\n}";
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
    assert_eq!(lints.len(), 2, "{lints:?}");
    let starts: Vec<u32> = lints.iter().map(|lint| lint.range.start).collect();
    let then_at = source.find("print(1);").unwrap() as u32;
    let else_at = source.find("print(2);").unwrap() as u32;
    assert!(starts.contains(&then_at), "{lints:?}");
    assert!(starts.contains(&else_at), "{lints:?}");
    // Both arms report the full statement leaf (`print(n);`).
    for lint in &lints {
        assert_eq!(lint.range.end - lint.range.start, 9, "{lints:?}");
    }
}

/// A `do` body reports at the whole statement (the production tiling's
/// spilled head fragment — the bare `print` send head — is not the arm).
#[test]
fn do_body_reports_at_the_statement() {
    let source = "void f(bool c) {\n  do print(4); while (c);\n}";
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Statements in a do should be enclosed in a block."
    );
    assert_eq!(
        lints[0].range.start,
        source.find("print(4);").unwrap() as u32
    );
    assert_eq!(
        lints[0].range.end,
        (source.find("print(4);").unwrap() + 9) as u32
    );
}

/// Constant folding across heads: `case 1 + 2:` collides with `case 3:`
/// — the later reports at `3` with both sources spelled as written (the
/// real-parse shape, the `lib/invalid_case_patterns.dart` row).
#[test]
fn folded_duplicate_case_values_report() {
    let source = "void f(dynamic o) {\n  switch (o) {\n    case 1 + 2:\n      break;\n    case \
                      3:\n      break;\n  }\n}";
    let f = legacy(Fixture::parse(source));
    let lints = f.run(&one_rule("no_duplicate_case_values"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The value of the case clause ('3') is equal to the value of an earlier case clause \
             ('1 + 2')."
    );
    assert_eq!(lints[0].range.start, source.find("3:").unwrap() as u32);
}

// —— use_rethrow_when_possible ————————————————————————————————————————

/// `test/rules/use_rethrow_when_possible_test.dart` `test_rethrow` — the
/// caught parameter rethrown as a statement reports at the throw.
#[test]
fn rethrow_of_the_exception_reports() {
    let source = "try {} catch (e) { throw e; }";
    // try0 {1 }2 catch3 (4 e5 )6 {7 throw8 e9 ;10 }11
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::TryStatement, 0);
            b.patch(0, NONE);
            b.leaf_span(NodeKind::Block, 1, 3);
            b.begin(NodeKind::CatchClause, 3);
            b.patch(0, 3);
            b.patch(1, NONE);
            b.begin(NodeKind::FormalParameter, 5);
            b.patch(0, NONE);
            b.patch(1, 5);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.patch(4, 0);
            b.end(NodeKind::FormalParameter, 6);
            b.begin(NodeKind::Block, 7);
            b.begin(NodeKind::ExpressionStatement, 8);
            b.begin(NodeKind::ThrowExpression, 8);
            identifier(b, 9, 10);
            b.end(NodeKind::ThrowExpression, 11);
            b.end(NodeKind::ExpressionStatement, 11);
            b.end(NodeKind::Block, 12);
            b.end(NodeKind::CatchClause, 12);
            b.end(NodeKind::TryStatement, 12);
        }),
    );
    let lints = f.run(&one_rule("use_rethrow_when_possible"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Use 'rethrow' to rethrow a caught exception."
    );
    assert_eq!(lints[0].range.start, utf16_at(&f, 8));
}

/// Throwing a different name and an `on`-only catch (no exception
/// parameter) pass.
#[test]
fn rethrow_other_name_and_on_form_pass() {
    let other = "try {} catch (e) { throw x; }";
    let f = Fixture::build(
        other,
        unit_of(|b, _t| {
            b.begin(NodeKind::TryStatement, 0);
            b.patch(0, NONE);
            b.leaf_span(NodeKind::Block, 1, 3);
            b.begin(NodeKind::CatchClause, 3);
            b.patch(0, 3);
            b.patch(1, NONE);
            b.begin(NodeKind::FormalParameter, 5);
            b.patch(0, NONE);
            b.patch(1, 5);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.patch(4, 0);
            b.end(NodeKind::FormalParameter, 6);
            b.begin(NodeKind::Block, 7);
            b.begin(NodeKind::ExpressionStatement, 8);
            b.begin(NodeKind::ThrowExpression, 8);
            identifier(b, 9, 10);
            b.end(NodeKind::ThrowExpression, 11);
            b.end(NodeKind::ExpressionStatement, 11);
            b.end(NodeKind::Block, 12);
            b.end(NodeKind::CatchClause, 12);
            b.end(NodeKind::TryStatement, 12);
        }),
    );
    assert!(f.run(&one_rule("use_rethrow_when_possible")).is_empty());

    // try0 {1 }2 on3 E4 {5 throw6 e7 ;8 }9 — no exception parameter.
    let on_form = "try {} on E { throw e; }";
    let f = Fixture::build(
        on_form,
        unit_of(|b, _t| {
            b.begin(NodeKind::TryStatement, 0);
            b.patch(0, NONE);
            b.leaf_span(NodeKind::Block, 1, 3);
            b.begin(NodeKind::CatchClause, 3);
            b.patch(0, NONE);
            b.patch(1, NONE);
            b.leaf_span(NodeKind::Type, 4, 5);
            b.begin(NodeKind::Block, 5);
            b.begin(NodeKind::ExpressionStatement, 6);
            b.begin(NodeKind::ThrowExpression, 6);
            identifier(b, 7, 8);
            b.end(NodeKind::ThrowExpression, 9);
            b.end(NodeKind::ExpressionStatement, 9);
            b.end(NodeKind::Block, 10);
            b.end(NodeKind::CatchClause, 10);
            b.end(NodeKind::TryStatement, 10);
        }),
    );
    assert!(f.run(&one_rule("use_rethrow_when_possible")).is_empty());
}

/// A throw inside an argument is not a statement throw (:37).
#[test]
fn rethrow_in_argument_position_passes() {
    let source = "try {} catch (e) { g(throw e); }";
    // try0 {1 }2 catch3 (4 e5 )6 {7 g8 (9 throw10 e11 )12 ;13 }14
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::TryStatement, 0);
            b.patch(0, NONE);
            b.leaf_span(NodeKind::Block, 1, 3);
            b.begin(NodeKind::CatchClause, 3);
            b.patch(0, 3);
            b.patch(1, NONE);
            b.begin(NodeKind::FormalParameter, 5);
            b.patch(0, NONE);
            b.patch(1, 5);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.patch(4, 0);
            b.end(NodeKind::FormalParameter, 6);
            b.begin(NodeKind::Block, 7);
            b.begin(NodeKind::ExpressionStatement, 8);
            b.begin(NodeKind::Send, 8);
            identifier(b, 8, 9);
            b.begin(NodeKind::Arguments, 9);
            b.begin(NodeKind::ThrowExpression, 10);
            identifier(b, 11, 12);
            b.end(NodeKind::ThrowExpression, 13);
            b.end(NodeKind::Arguments, 13);
            b.end(NodeKind::Send, 13);
            b.end(NodeKind::ExpressionStatement, 14);
            b.end(NodeKind::Block, 15);
            b.end(NodeKind::CatchClause, 15);
            b.end(NodeKind::TryStatement, 15);
        }),
    );
    assert!(f.run(&one_rule("use_rethrow_when_possible")).is_empty());
}

/// A local shadowing the catch parameter binds the throw — the lexical
/// face of the official element identity (`e` the local, not the
/// parameter): no report.
#[test]
fn rethrow_of_shadowing_local_passes() {
    let source = "try {} catch (e) { var e = 1; throw e; }";
    // try0 {1 }2 catch3 (4 e5 )6 {7 var8 e9 =10 111 ;12 throw13 e14 ;15 }16
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::TryStatement, 0);
            b.patch(0, NONE);
            b.leaf_span(NodeKind::Block, 1, 3);
            b.begin(NodeKind::CatchClause, 3);
            b.patch(0, 3);
            b.patch(1, NONE);
            b.begin(NodeKind::FormalParameter, 5);
            b.patch(0, NONE);
            b.patch(1, 5);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.patch(4, 0);
            b.end(NodeKind::FormalParameter, 6);
            b.begin(NodeKind::Block, 7);
            b.begin(NodeKind::VariablesDeclaration, 8);
            b.patch(0, NONE);
            b.begin(NodeKind::InitializedIdentifier, 9);
            identifier(b, 9, 10);
            b.begin(NodeKind::VariableInitializer, 10);
            b.leaf(NodeKind::LiteralInt, 11);
            b.end(NodeKind::VariableInitializer, 12);
            b.end(NodeKind::InitializedIdentifier, 12);
            b.end(NodeKind::VariablesDeclaration, 13);
            b.begin(NodeKind::ExpressionStatement, 13);
            b.begin(NodeKind::ThrowExpression, 13);
            identifier(b, 14, 15);
            b.end(NodeKind::ThrowExpression, 16);
            b.end(NodeKind::ExpressionStatement, 16);
            b.end(NodeKind::Block, 17);
            b.end(NodeKind::CatchClause, 17);
            b.end(NodeKind::TryStatement, 17);
        }),
    );
    assert!(f.run(&one_rule("use_rethrow_when_possible")).is_empty());
}

// —— real-parse operand and parameter reads ——————————————————————————
//
// Production parses leave the throw a childless handle (the operand
// rows precede it as siblings) and wrap the catch formals in a
// `FormalParameters` container; both reads answer on those shapes
// through the production bridge.

/// `catch (e) { throw e; }` (the retired lint corpus's
/// avoid_catches_without_on_clauses.dart:47 and
/// flow_family.dart:34 shapes) reports at the `throw e` expression.
#[test]
fn bare_catch_rethrow_reports_on_real_parse() {
    let source = "\
void f() {
  try {
    print(1);
  } catch (e) {
    throw e;
  }
}
";
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("use_rethrow_when_possible"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Use 'rethrow' to rethrow a caught exception."
    );
    let throw_tok = tok(source, &f.tokens, "throw", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, throw_tok));
    // `throw e` — the throw keyword through the operand.
    assert_eq!(lints[0].range.end, lints[0].range.start + 7);
}

/// A stack-trace formal does not disturb the exception-parameter read.
#[test]
fn rethrow_with_stack_trace_parameter_reports_on_real_parse() {
    let source = "void f() {\n  try {} catch (e, s) {\n    throw e;\n  }\n}\n";
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("use_rethrow_when_possible"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Use 'rethrow' to rethrow a caught exception."
    );
}

/// Non-simple operands, other names, an `on`-only catch, and a
/// shadowing local all stay silent on real parses.
#[test]
fn rethrow_non_simple_and_shadowed_faces_stay_silent_on_real_parse() {
    let one_rule_set = one_rule("use_rethrow_when_possible");
    // A parenthesized operand is not the parameter reference.
    let f = Fixture::parse("void f() {\n  try {} catch (e) {\n    throw (e);\n  }\n}\n");
    assert!(f.run(&one_rule_set).is_empty());
    // A member invocation is not the parameter reference.
    let f =
        Fixture::parse("void f() {\n  try {} catch (e) {\n    throw e.toString();\n  }\n}\n");
    assert!(f.run(&one_rule_set).is_empty());
    // A different name.
    let f = Fixture::parse("void f() {\n  try {} catch (e) {\n    throw x;\n  }\n}\n");
    assert!(f.run(&one_rule_set).is_empty());
    // An `on`-only catch has no exception parameter (:42-43).
    let f = Fixture::parse("void f() {\n  try {} on E {\n    throw e;\n  }\n}\n");
    assert!(f.run(&one_rule_set).is_empty());
    // A shadowing local binds the throw (`e` the local, not the
    // parameter).
    let f = Fixture::parse(
        "void f() {\n  try {} catch (e) {\n    var e = 1;\n    throw e;\n  }\n}\n",
    );
    assert!(f.run(&one_rule_set).is_empty());
    // A shadowing for-in loop variable binds the throw the same way.
    let f = Fixture::parse(
        "void f() {\n  try {} catch (e) {\n    for (var e in xs) {\n      throw e;\n    }\n  }\n}\n",
    );
    assert!(f.run(&one_rule_set).is_empty());
}

// —— avoid_returning_null_for_void ————————————————————————————————————

/// `test/rules/avoid_returning_null_for_void_test.dart` — a top-level
/// function carries the function text.
#[test]
fn returning_null_from_void_function_reports() {
    let source = "void f() { return null; }";
    // void0 f1 (2 )3 {4 return5 null6 ;7 }8
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::TopLevelMethod, 0);
            b.patch(0, NONE);
            b.leaf(NodeKind::VoidType, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::FormalParameters, 2);
            b.end(NodeKind::FormalParameters, 4);
            b.begin(NodeKind::Block, 4);
            b.begin(NodeKind::ReturnStatement, 5);
            b.leaf(NodeKind::LiteralNull, 6);
            b.end(NodeKind::ReturnStatement, 8);
            b.end(NodeKind::Block, 9);
            b.end(NodeKind::TopLevelMethod, 9);
        }),
    );
    let lints = f.run(&one_rule("avoid_returning_null_for_void"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Don't return 'null' from a function with a return type of 'void'."
    );
    assert_eq!(
        lints[0].correction,
        Some("Try removing the 'null'.".to_owned())
    );
    assert_eq!(lints[0].range.start, utf16_at(&f, 5));
}

/// Inside a class the method variant reports.
#[test]
fn returning_null_from_void_method_reports() {
    let source = "class A { void m() { return null; } }";
    // class0 A1 {2 void3 m4 (5 )6 {7 return8 null9 ;10 }11 }12
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Method, 3);
            b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.leaf(NodeKind::VoidType, 3);
            identifier(b, 4, 5);
            b.begin(NodeKind::FormalParameters, 5);
            b.end(NodeKind::FormalParameters, 7);
            b.begin(NodeKind::Block, 7);
            b.begin(NodeKind::ReturnStatement, 8);
            b.leaf(NodeKind::LiteralNull, 9);
            b.end(NodeKind::ReturnStatement, 11);
            b.end(NodeKind::Block, 12);
            b.end(NodeKind::Method, 12);
            b.end(NodeKind::DeclarationBody, 13);
            b.end(NodeKind::ClassDeclaration, 13);
            b.end(NodeKind::TopLevelDeclaration, 13);
        }),
    );
    let lints = f.run(&one_rule("avoid_returning_null_for_void"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Don't return 'null' from a method with a return type of 'void'."
    );
}

/// A non-void declared type, a closure face, and `async void` pass.
#[test]
fn returning_null_non_void_and_async_pass() {
    let typed = "int f() { return null; }";
    // int0 f1 (2 )3 {4 return5 null6 ;7 }8
    let f = Fixture::build(
        typed,
        unit_of(|b, _t| {
            b.begin(NodeKind::TopLevelMethod, 0);
            b.patch(0, NONE);
            b.leaf_span(NodeKind::Type, 0, 1);
            identifier(b, 1, 2);
            b.begin(NodeKind::FormalParameters, 2);
            b.end(NodeKind::FormalParameters, 4);
            b.begin(NodeKind::Block, 4);
            b.begin(NodeKind::ReturnStatement, 5);
            b.leaf(NodeKind::LiteralNull, 6);
            b.end(NodeKind::ReturnStatement, 8);
            b.end(NodeKind::Block, 9);
            b.end(NodeKind::TopLevelMethod, 9);
        }),
    );
    assert!(f.run(&one_rule("avoid_returning_null_for_void")).is_empty());

    // The closure is the nearest function — no written return type.
    let closure = "void f() { g(() { return null; }); }";
    // void0 f1 (2 )3 {4 g5 (6 (7 )8 {9 return10 null11 ;12 }13 )14 ;15 }16
    let f = Fixture::build(
        closure,
        unit_of(|b, _t| {
            b.begin(NodeKind::TopLevelMethod, 0);
            b.patch(0, NONE);
            b.leaf(NodeKind::VoidType, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::FormalParameters, 2);
            b.end(NodeKind::FormalParameters, 4);
            b.begin(NodeKind::Block, 4);
            b.begin(NodeKind::ExpressionStatement, 5);
            b.begin(NodeKind::Send, 5);
            identifier(b, 5, 6);
            b.begin(NodeKind::Arguments, 6);
            b.begin(NodeKind::FunctionExpression, 7);
            b.begin(NodeKind::FormalParameters, 7);
            b.end(NodeKind::FormalParameters, 9);
            b.begin(NodeKind::Block, 9);
            b.begin(NodeKind::ReturnStatement, 10);
            b.leaf(NodeKind::LiteralNull, 11);
            b.end(NodeKind::ReturnStatement, 13);
            b.end(NodeKind::Block, 14);
            b.end(NodeKind::FunctionExpression, 14);
            b.end(NodeKind::Arguments, 15);
            b.end(NodeKind::Send, 15);
            b.end(NodeKind::ExpressionStatement, 16);
            b.end(NodeKind::Block, 17);
            b.end(NodeKind::TopLevelMethod, 17);
        }),
    );
    assert!(f.run(&one_rule("avoid_returning_null_for_void")).is_empty());

    // `void f() async { return null; }` — the async body skips both
    // official branches (:75-80). The brace body is a `BlockBody`
    // carrying the `async` token in its modifier words.
    let async_fn = "void f() async { return null; }";
    // void0 f1 (2 )3 async4 {5 return6 null7 ;8 }9
    let f = Fixture::build(
        async_fn,
        unit_of(|b, t| {
            b.begin(NodeKind::TopLevelMethod, 0);
            b.patch(0, NONE);
            b.leaf(NodeKind::VoidType, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::FormalParameters, 2);
            b.end(NodeKind::FormalParameters, 4);
            b.begin(NodeKind::BlockBody, 4);
            b.patch(0, tok_at(async_fn, t, "async", 0));
            b.patch(1, NONE);
            b.begin(NodeKind::ReturnStatement, 6);
            b.leaf(NodeKind::LiteralNull, 7);
            b.end(NodeKind::ReturnStatement, 9);
            b.end(NodeKind::BlockBody, 10);
            b.end(NodeKind::TopLevelMethod, 10);
        }),
    );
    assert!(f.run(&one_rule("avoid_returning_null_for_void")).is_empty());
}

/// The arrow-body face: `void f() => null;` reports.
#[test]
fn returning_null_from_arrow_body_reports() {
    let source = "void f() => null;";
    // void0 f1 (2 )3 =>4 null5 ;6
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::TopLevelMethod, 0);
            b.patch(0, NONE);
            b.leaf(NodeKind::VoidType, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::FormalParameters, 2);
            b.end(NodeKind::FormalParameters, 4);
            b.begin(NodeKind::ExpressionBody, 4);
            b.patch(0, 4);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.leaf(NodeKind::LiteralNull, 5);
            b.end(NodeKind::ExpressionBody, 7);
            b.end(NodeKind::TopLevelMethod, 7);
        }),
    );
    let lints = f.run(&one_rule("avoid_returning_null_for_void"));
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, 4));
}

/// `test_function_async_returnsFutureVoid_blockBody_returnNull` — the
/// async `Future<void>` branch (:77-80) reports inside a block body.
#[test]
fn returning_null_from_async_future_void_reports() {
    let f = Fixture::parse("Future<void> f() async { return null; }");
    let lints = f.run(&one_rule("avoid_returning_null_for_void"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Don't return 'null' from a function with a return type of 'void'."
    );
}

/// `test_function_async_returnsFutureVoid_expressionBody_returnNull` —
/// the async arrow reports over `async => null;` (the official
/// `ExpressionFunctionBody.beginToken` is the `async` keyword).
#[test]
fn returning_null_from_async_arrow_reports() {
    let f = Fixture::parse("Future<void> f() async => null;");
    let lints = f.run(&one_rule("avoid_returning_null_for_void"));
    assert_eq!(lints.len(), 1);
    // The official body span covers `async => null;` — from the `async`
    // keyword through the `;` (ASCII source: byte == utf16 offset).
    let async_offset = f.source.find("async").expect("async token") as u32;
    assert_eq!(lints[0].range.start, async_offset);
    assert_eq!(lints[0].range.end, f.source.len() as u32);
}

/// `test_method_inClass_blockBody_async_returnsFutureVoid_blockBody_returnNull`
/// — the async method takes the method message variant.
#[test]
fn returning_null_from_async_future_void_method_reports() {
    let f = Fixture::parse("class C { Future<void> m() async { return null; } }");
    let lints = f.run(&one_rule("avoid_returning_null_for_void"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Don't return 'null' from a method with a return type of 'void'."
    );
}

/// `test_localFunction_async_returnsFutureVoid_blockBody_returnNull` —
/// a local `Future<void> g() async` reports.
#[test]
fn returning_null_from_async_local_function_reports() {
    let f = Fixture::parse("void f() { Future<void> g() async { return null; } }");
    let lints = f.run(&one_rule("avoid_returning_null_for_void"));
    assert_eq!(lints.len(), 1);
}

/// The inferred-closure face: a `void Function()` context types the
/// anonymous closure's return as `void` (the resolved
/// `declaredFragment.element.returnType`), top-level field and local
/// variable positions alike.
#[test]
fn returning_null_from_function_typed_closure_reports() {
    for source in [
        "void Function() f = () { return null; };",
        "void f() { void Function() g = () { return null; }; }",
        "class C { void Function() f = () { return null; }; }",
        "void Function() f = () => null;",
    ] {
        let f = Fixture::parse(source);
        assert_eq!(
            f.run(&one_rule("avoid_returning_null_for_void")).len(),
            1,
            "{source}"
        );
    }
}

/// The async closure face: a `Future<void> Function()` context plus an
/// `async` body reports (:77-80 through the context type).
#[test]
fn returning_null_from_async_function_typed_closure_reports() {
    let f = Fixture::parse("Future<void> Function() f = () async { return null; };");
    assert_eq!(f.run(&one_rule("avoid_returning_null_for_void")).len(), 1);
}

/// The negative async/context faces: a nullable `Future<void>?`, a
/// `FutureOr<void>`, a non-void context (`int Function()`), and an
/// argument-position closure without a resolvable context all stay
/// silent (`isDartAsyncFuture`/`VoidType` misses, :75-80).
#[test]
fn returning_null_async_and_context_negatives_pass() {
    for source in [
        "Future<void>? f() async { return null; }",
        "FutureOr<void> f() async { return null; }",
        "Future<int> f() async { return null; }",
        "int Function() f = () { return null; };",
        "void f() { g(() { return null; }); }",
        "void f() { g(() => null); }",
        "var f = () { return null; };",
    ] {
        let f = Fixture::parse(source);
        assert!(
            f.run(&one_rule("avoid_returning_null_for_void")).is_empty(),
            "{source}"
        );
    }
}

// —— type_literal_in_constant_pattern ———————————————————————————————————

/// `case int:` — a built-in type name in constant-pattern position
/// reports over the pattern (`test/rules/type_literal_in_constant_pattern_test.dart`
/// `test_BuiltInTypePattern`).
#[test]
fn type_literal_reports_a_builtin_type_pattern() {
    let source = "switch (x) { case int: break; }";
    // switch0 (1 x2 )3 {4 case5 int6 :7 break8 ;9 }10
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::SwitchStatement, 0);
            b.begin(NodeKind::SwitchBlock, 4);
            b.begin(NodeKind::SwitchCase, 5);
            b.patch(0, NONE);
            b.patch(1, 7);
            b.patch(2, 1);
            b.begin(NodeKind::ConstantPattern, 6);
            identifier(b, 6, 7);
            b.end(NodeKind::ConstantPattern, 7);
            b.end(NodeKind::SwitchCase, 8);
            b.end(NodeKind::SwitchBlock, 10);
            b.end(NodeKind::SwitchStatement, 10);
        }),
    );
    let lints = f.run(&one_rule("type_literal_in_constant_pattern"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Use 'TypeName _' instead of a type literal."
    );
    assert_eq!(lints[0].range.start, utf16_at(&f, 6));
    assert_eq!(lints[0].range.end, utf16_at(&f, 7));
}

/// A const-marked pattern bails (:45-47), a non-builtin constant
/// (`case color:`) never matches the name set, and a property access
/// (`case Foo.bar:`) is not a bare type literal.
#[test]
fn type_literal_const_marked_and_non_type_patterns_pass() {
    let const_marked = "switch (x) { case const int: break; }";
    // switch0 (1 x2 )3 {4 case5 const6 int7 :8 break9 ;10 }11
    let f = Fixture::build(
        const_marked,
        unit_of(|b, _t| {
            b.begin(NodeKind::SwitchStatement, 0);
            b.begin(NodeKind::SwitchBlock, 4);
            b.begin(NodeKind::SwitchCase, 5);
            b.patch(0, NONE);
            b.patch(1, 7);
            b.patch(2, 1);
            b.begin(NodeKind::ConstantPattern, 6);
            identifier(b, 7, 8);
            b.end(NodeKind::ConstantPattern, 8);
            b.end(NodeKind::SwitchCase, 9);
            b.end(NodeKind::SwitchBlock, 11);
            b.end(NodeKind::SwitchStatement, 11);
        }),
    );
    assert!(
        f.run(&one_rule("type_literal_in_constant_pattern"))
            .is_empty()
    );

    let user_constant = "switch (x) { case color: break; }";
    let f = Fixture::build(
        user_constant,
        unit_of(|b, _t| {
            b.begin(NodeKind::SwitchStatement, 0);
            b.begin(NodeKind::SwitchBlock, 4);
            b.begin(NodeKind::SwitchCase, 5);
            b.patch(0, NONE);
            b.patch(1, 7);
            b.patch(2, 1);
            b.begin(NodeKind::ConstantPattern, 6);
            identifier(b, 6, 7);
            b.end(NodeKind::ConstantPattern, 7);
            b.end(NodeKind::SwitchCase, 8);
            b.end(NodeKind::SwitchBlock, 10);
            b.end(NodeKind::SwitchStatement, 10);
        }),
    );
    assert!(
        f.run(&one_rule("type_literal_in_constant_pattern"))
            .is_empty()
    );

    let property = "switch (x) { case Foo.bar: break; }";
    // switch0 (1 x2 )3 {4 case5 Foo6 .7 bar8 :9 break10 ;11 }12
    let f = Fixture::build(
        property,
        unit_of(|b, _t| {
            b.begin(NodeKind::SwitchStatement, 0);
            b.begin(NodeKind::SwitchBlock, 4);
            b.begin(NodeKind::SwitchCase, 5);
            b.patch(0, NONE);
            b.patch(1, 7);
            b.patch(2, 1);
            b.begin(NodeKind::ConstantPattern, 6);
            b.begin(NodeKind::DotAccess, 6);
            b.patch(0, 7);
            b.patch(1, NONE);
            identifier(b, 6, 7);
            identifier(b, 8, 9);
            b.end(NodeKind::DotAccess, 9);
            b.end(NodeKind::ConstantPattern, 9);
            b.end(NodeKind::SwitchCase, 10);
            b.end(NodeKind::SwitchBlock, 12);
            b.end(NodeKind::SwitchStatement, 12);
        }),
    );
    assert!(
        f.run(&one_rule("type_literal_in_constant_pattern"))
            .is_empty()
    );
}

/// Production-bridge face: the if-case `case int` reports over the
/// pattern (`test/rules/type_literal_in_constant_pattern_test.dart`
/// core-type case).
#[test]
fn type_literal_if_case_reports_the_pattern() {
    let f = Fixture::parse("void f(Object x) { if (x case int) {} }");
    let lints = f.run(&one_rule("type_literal_in_constant_pattern"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Use 'TypeName _' instead of a type literal."
    );
    let int = tok(&f.source, &f.tokens, "int", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, int));
    assert_eq!(lints[0].range.end, utf16_at(&f, int) + 3);
}

/// The `Type`-matched-value exemption (:50-59) through the syntactic
/// face on real parses: a `Type`-typed parameter, a `Type`-typed local,
/// a `.runtimeType` scrutinee, and a const-marked pattern all stay
/// silent (`test_constType_matchType`).
#[test]
fn type_literal_type_matched_value_passes() {
    let f = Fixture::parse("void f(Type x) { if (x case int) {} }");
    assert!(
        f.run(&one_rule("type_literal_in_constant_pattern"))
            .is_empty(),
        "the matched value is the Type-typed parameter"
    );

    let f = Fixture::parse("void f() { Type t = int; if (t case int) {} }");
    assert!(
        f.run(&one_rule("type_literal_in_constant_pattern"))
            .is_empty(),
        "the matched value is the Type-typed local"
    );

    let f = Fixture::parse("void f(Object x) { if (x.runtimeType case int) {} }");
    assert!(
        f.run(&one_rule("type_literal_in_constant_pattern"))
            .is_empty(),
        "a .runtimeType scrutinee is always Type"
    );

    let f = Fixture::parse("void f(Object x) { if (x case const (int)) {} }");
    assert!(
        f.run(&one_rule("type_literal_in_constant_pattern"))
            .is_empty(),
        "a const-marked pattern bails (:45-47)"
    );
}

// —— the converted audit faces (exemption-reaudit F4/F5): the
// production-bridge pins for the resolution-backed decisions ——

/// The forward `continue L` to a LATER switch member resolves through
/// the member-label scope (scope_analyzer.dart:846-850); the target
/// member's chain contains the try, so the finally jump stays local
/// and stays silent (the oracle probe face).
#[test]
fn finally_forward_continue_member_label_passes() {
    assert!(
        Fixture::parse(
            r"
void f(bool c) {
  try {
    print(0);
  } finally {
    switch (c) {
      case true:
        continue L;
      L:
      case false:
        print(1);
    }
  }
}
"
        )
        .run(&one_rule("control_flow_in_finally"))
        .is_empty()
    );
}

/// The same forward jump with the target member OUTSIDE the try: the
/// target's chain misses the try and the jump reports (the oracle
/// probe reports `break L` at the same shape).
#[test]
fn finally_forward_continue_outside_try_reports() {
    let f = Fixture::parse(
        r"
void f(bool c) {
  switch (c) {
    case true:
      try {
        print(0);
      } finally {
        continue L;
      }
    L:
    case false:
      print(1);
  }
}
",
    );
    let lints = f.run(&one_rule("control_flow_in_finally"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Use of 'continue' in a 'finally' clause.");
}

/// A forward `continue L` to a LATER labeled statement (no switch)
/// stays undefined officially (`LABEL_UNDEFINED` — labeled statements
/// bind only within their statement) — the unresolved-target face
/// reports.
#[test]
fn finally_forward_continue_to_later_labeled_statement_reports() {
    let f = Fixture::parse(
        r"
void f(bool c) {
  try {
    print(0);
  } finally {
    if (c) {
      continue L;
    }
    L:
    while (c) {
      print(1);
    }
  }
}
",
    );
    let lints = f.run(&one_rule("control_flow_in_finally"));
    assert_eq!(lints.len(), 1, "{lints:?}");
}

/// `continue L` onto the try statement itself (`L2: try … finally {
/// continue L2; }`): the target IS the try — the enabler matches and
/// the jump stays silent (the oracle probe face).
#[test]
fn finally_continue_onto_labeled_try_passes() {
    assert!(
        Fixture::parse(
            r"
void f(bool c) {
  L:
  try {
    print(0);
  } finally {
    if (c) {
      continue L;
    }
  }
}
"
        )
        .run(&one_rule("control_flow_in_finally"))
        .is_empty()
    );
}

/// The resolution-backed type-literal test: a same-file user type
/// reports (the under-report face is dead), a same-file value shadow
/// of a built-in name stays silent (the over-report face is dead).
#[test]
fn type_literal_resolution_backed_faces() {
    // `case MyType` on a same-file class reports.
    let f = Fixture::parse(
        r"
class MyType {}

void f(Object o) {
  if (o case MyType) {
    print(0);
  }
}
",
    );
    let lints = f.run(&one_rule("type_literal_in_constant_pattern"));
    assert_eq!(lints.len(), 1, "{lints:?}");

    // `const String = 'x';` shadows the built-in — the pattern is the
    // String constant, not a type literal.
    let f = Fixture::parse(
        r"
const String = 'x';

void f(Object o) {
  if (o case String) {
    print(0);
  }
}
",
    );
    assert!(
        f.run(&one_rule("type_literal_in_constant_pattern"))
            .is_empty()
    );

    // A local `var int` shadow — the name resolves to the value.
    let f = Fixture::parse("void f(Object o) { var int = 5; if (o case int) { print(int); } }");
    assert!(
        f.run(&one_rule("type_literal_in_constant_pattern"))
            .is_empty()
    );
}

/// The class-scope static face: a `static const` named like a built-in
/// type is a value in scope inside its container (the binding walk
/// sees instance members only) — the bare constant resolves to the
/// member and stays silent; outside the container the spelling is the
/// type and reports.
#[test]
fn type_literal_class_scope_static_shadow_passes() {
    let f = Fixture::parse(
        r"
class C {
  static const int = 3;
  void g(Object o) {
    if (o case int) {
      print(0);
    }
  }
}
",
    );
    assert!(
        f.run(&one_rule("type_literal_in_constant_pattern"))
            .is_empty(),
        "the static shadows the spelling inside C"
    );

    // Outside the container the spelling is the type again.
    let f = Fixture::parse(
        r"
class C {
  static const int = 3;
}

void f(Object o) {
  if (o case int) {
    print(0);
  }
}
",
    );
    assert_eq!(
        f.run(&one_rule("type_literal_in_constant_pattern")).len(),
        1
    );
}

/// The enclosing enum's own constants are values in scope inside the
/// enum's members — a constant named like a built-in type stays
/// silent there.
#[test]
fn type_literal_enum_constant_shadow_passes() {
    let f = Fixture::parse(
        r"
enum E {
  int;

  void g(Object o) {
    if (o case int) {
      print(0);
    }
  }
}
",
    );
    assert!(
        f.run(&one_rule("type_literal_in_constant_pattern"))
            .is_empty(),
        "the enum's own constant is a value inside E"
    );
}

/// The const-alias fold: a Type-valued constant reports like the
/// official static type (`case t` where `t` is `int`) — top-level,
/// local, and qualified-class spellings; a value alias stays silent.
#[test]
fn type_literal_const_alias_faces() {
    // Top-level `const t = int;` reports.
    let f = Fixture::parse(
        r"
const t = int;

void f(Object o) {
  if (o case t) {
    print(0);
  }
}
",
    );
    assert_eq!(
        f.run(&one_rule("type_literal_in_constant_pattern")).len(),
        1
    );

    // Local `const t = int;` reports (the oracle probe face).
    let f = Fixture::parse(
        r"
void f(Object o) {
  const t = int;
  if (o case t) {
    print(0);
  }
}
",
    );
    assert_eq!(
        f.run(&one_rule("type_literal_in_constant_pattern")).len(),
        1
    );

    // A value alias (`const s = 'x';`) stays silent.
    let f = Fixture::parse(
        r"
const s = 'x';

void f(Object o) {
  if (o case s) {
    print(0);
  }
}
",
    );
    assert!(
        f.run(&one_rule("type_literal_in_constant_pattern"))
            .is_empty()
    );
}

// —— always_put_control_body_on_new_line ————————————————————————————

/// `test/rules/always_put_control_body_on_new_line.dart` — a then-body
/// on the control line reports at the body's first token.
#[test]
fn control_body_reports_a_same_line_then_body() {
    let source = "void f(bool c) { if (c) x = 1; }";
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("always_put_control_body_on_new_line"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Statement should be on a separate line.");
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try moving the statement to a new line.")
    );
    let x = tok(&f.source, &f.tokens, "x", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, x));
}

/// A non-empty block whose first statement shares the control line
/// reports; the body on its own line passes.
#[test]
fn control_body_block_faces() {
    let f = Fixture::parse("void f(bool c) { if (c) { x = 1; } }");
    assert_eq!(
        f.run(&one_rule("always_put_control_body_on_new_line"))
            .len(),
        1
    );
    let f = Fixture::parse("void f(bool c) {\n  if (c) {\n    x = 1;\n  }\n}");
    assert!(
        f.run(&one_rule("always_put_control_body_on_new_line"))
            .is_empty()
    );
}

/// An empty block skips (:56).
#[test]
fn control_body_empty_block_passes() {
    let f = Fixture::parse("void f(bool c) { if (c) {} }");
    assert!(
        f.run(&one_rule("always_put_control_body_on_new_line"))
            .is_empty()
    );
}

/// `while`, `for`, and `for-in` bodies are checked against the
/// closing `)`.
#[test]
fn control_body_loop_faces() {
    let f = Fixture::parse(
        "void f(bool c) { while (c) x = 1; }\n\
             void g() { for (var i = 0; i < 1; i++) x = i; }\n\
             void h() { for (var e in [1]) print(e); }",
    );
    assert_eq!(
        f.run(&one_rule("always_put_control_body_on_new_line"))
            .len(),
        3
    );
}

/// The `do` body is compared against the `do` keyword's end (:38-40).
#[test]
fn control_body_do_face() {
    let f = Fixture::parse("void f(bool c) { int x = 0; do x += 1; while (c); }");
    let lints = f.run(&one_rule("always_put_control_body_on_new_line"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    // The reported token is the body's first `x` (the second in the
    // source), not the declaration's.
    let x = tok(&f.source, &f.tokens, "x", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, x));
}

/// The else branch checks against the `else` keyword (:50-53) — the
/// report sits at the else clause's begin, the block's `{`; an
/// `else if` chain skips.
#[test]
fn control_body_else_faces() {
    let f = Fixture::parse("void f(bool c) {\n  if (c) {\n    x = 1;\n  } else { y = 2; }\n}");
    let lints = f.run(&one_rule("always_put_control_body_on_new_line"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, "{", 2))
    );
    let f = Fixture::parse(
        "void f(bool c, bool d) {\n  if (c) {\n    x = 1;\n  } else if (d) {\n    y = 2;\n  }\n}",
    );
    assert!(
        f.run(&one_rule("always_put_control_body_on_new_line"))
            .is_empty()
    );
}

/// Pin: a non-block `else` body on the `else` keyword's line reports
/// through the `token_end(else_kw)` anchor and the `body_offset` head
/// lookup over a non-Block `first` — both migrated helpers feed the
/// same comparison.
#[test]
fn control_body_else_single_statement_reports() {
    let f = Fixture::parse("void f(bool c) {\n  if (c) {\n    x = 1;\n  } else y = 2;\n}");
    let lints = f.run(&one_rule("always_put_control_body_on_new_line"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, "y", 0))
    );
}

/// Pin: a `do` body block on the `do` keyword's line reports through
/// `token_end(head_token(node))` — the `do`-anchored arm — with
/// `body_offset` resolved over the block's first statement. The
/// report anchor is the body's begin token (official
/// `reportAtToken(node.beginToken)`), i.e. the block's `{`, not the
/// first statement.
#[test]
fn control_body_do_block_body_reports() {
    let f = Fixture::parse("void f(bool c) { int x = 0; do { x += 1; } while (c); }");
    let lints = f.run(&one_rule("always_put_control_body_on_new_line"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, "{", 1))
    );
}

// —— avoid_catches_without_on_clauses ———————————————————————————————

/// A bare `catch` clause with no valid use reports at the catch
/// keyword.
#[test]
fn catch_without_on_reports_a_bare_catch() {
    let source = "void f() { try {} catch (e) {} }";
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("avoid_catches_without_on_clauses"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Catch clause should use 'on' to specify the type of exception being caught."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try adding an 'on' clause before the 'catch'.")
    );
    let kw = tok(&f.source, &f.tokens, "catch", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, kw));
}

/// `on`-clause forms skip — with or without a catch clause.
#[test]
fn catch_without_on_on_forms_pass() {
    let f = Fixture::parse(
        "void f() { try {} on int catch (e) {} }\nvoid g() { try {} on int {} }",
    );
    assert!(
        f.run(&one_rule("avoid_catches_without_on_clauses"))
            .is_empty()
    );
}

/// `rethrow;` and `throw e;` bodies are valid uses (:96-117).
#[test]
fn catch_without_on_valid_uses_pass() {
    let f = Fixture::parse(
        "void f() { try {} catch (e) { rethrow; } }\n\
             void g() { try {} catch (e) { throw e; } }",
    );
    assert!(
        f.run(&one_rule("avoid_catches_without_on_clauses"))
            .is_empty()
    );
}

/// Any other body reports; the two-parameter catch form reports too.
/// (The :96-101 nested-catch rethrow fence is implemented but untested
/// here: the production parser currently mis-nests a `try` inside a
/// `catch` body — the builder rejects the row pairing — a parser-side
/// defect tracked outside this rule.)
#[test]
fn catch_without_on_other_bodies_report() {
    let f = Fixture::parse(
        "void f() { try {} catch (e) { print(e); } }\nvoid g() { try {} catch (e, s) {} }",
    );
    assert_eq!(
        f.run(&one_rule("avoid_catches_without_on_clauses")).len(),
        2
    );
}

/// A shadowing local inside a closure body re-owns the name: the throw
/// reads the closure's own local, not the caught exception (the element
/// identity), so the clause reports.
#[test]
fn catch_without_on_shadowed_closure_throw_reports() {
    let f = Fixture::parse("void f() { try {} catch (e) { g(() { var e = 1; throw e; }); } }");
    assert_eq!(
        f.run(&one_rule("avoid_catches_without_on_clauses")).len(),
        1
    );
}

/// A closure parameter of the same lexeme captures the same way.
#[test]
fn catch_without_on_shadowed_closure_parameter_reports() {
    let f = Fixture::parse("void f() { try {} catch (e) { xs.forEach((e) { throw e; }); } }");
    assert_eq!(
        f.run(&one_rule("avoid_catches_without_on_clauses")).len(),
        1
    );
}

/// A nested block's local shadows within its block only; the throw
/// inside it reads the local and the clause reports.
#[test]
fn catch_without_on_shadowed_nested_block_throw_reports() {
    let f = Fixture::parse("void f() { try {} catch (e) { { var e = 1; throw e; } } }");
    assert_eq!(
        f.run(&one_rule("avoid_catches_without_on_clauses")).len(),
        1
    );
}

/// A loop variable of the same lexeme shadows across the loop body.
#[test]
fn catch_without_on_shadowed_loop_variable_reports() {
    let f = Fixture::parse("void f() { try {} catch (e) { for (var e in xs) { throw e; } } }");
    assert_eq!(
        f.run(&one_rule("avoid_catches_without_on_clauses")).len(),
        1
    );
}

/// The capture walk must not over-capture: a throw through an
/// unshadowing closure is still a valid use (the official's throw face
/// reads through closures), and a same-block local of a DIFFERENT name
/// leaves the exception's reads untouched.
#[test]
fn catch_without_on_throw_through_closure_still_passes() {
    let f = Fixture::parse(
        "void f() { try {} catch (e) { g(() { throw e; }); } }\n\
             void h() { try {} catch (e) { var t = 1; throw e; } }",
    );
    assert!(
        f.run(&one_rule("avoid_catches_without_on_clauses"))
            .is_empty()
    );
}

/// The object-pattern field face of the raw-type exemption — the
/// official `test_constType_matchType_nested`: a constant nested in an
/// object pattern against a `Type`-written field stays silent (if-case
/// and switch-case containers); a non-`Type` field keeps the report.
#[test]
fn type_literal_object_pattern_type_field_passes() {
    let type_field = |src: &str| {
        let f = Fixture::parse(src);
        assert!(
            f.run(&one_rule("type_literal_in_constant_pattern"))
                .is_empty(),
            "the matched value is the Type-typed field: {src}"
        );
    };
    type_field(
        r"
class A {
  final Type type;
  A(this.type);
}

void f(A x) {
  if (x case A(type: int)) {}
}
",
    );
    type_field(
        r"
class A {
  final Type type;
  A(this.type);
}

void f(A x) {
  switch (x) {
    case A(type: int):
      break;
  }
}
",
    );
    // Nested object patterns: the innermost container decides — B's
    // `Type` field exempts, the outer field's own type is irrelevant.
    type_field(
        r"
class A {
  final B b;
  A(this.b);
}

class B {
  final Type type;
  B(this.type);
}

void f(A x) {
  if (x case A(b: B(type: int))) {}
}
",
    );

    // A non-`Type` field keeps the name reading.
    let f = Fixture::parse(
        r"
class A {
  final int type;
  A(this.type);
}

void f(A x) {
  if (x case A(type: int)) {}
}
",
    );
    assert_eq!(
        f.run(&one_rule("type_literal_in_constant_pattern")).len(),
        1,
        "an int-typed field is no exemption"
    );
    // An untyped field keeps the name reading too.
    let f = Fixture::parse(
        r"
class A {
  final type;
  A(this.type);
}

void f(A x) {
  if (x case A(type: int)) {}
}
",
    );
    assert_eq!(
        f.run(&one_rule("type_literal_in_constant_pattern")).len(),
        1,
        "an untyped field is no exemption"
    );
    // The nullable `Type?` is not a subtype of `Type` (the official
    // `isSubtypeOf` gate) — the nested constant reports.
    let f = Fixture::parse(
        r"
class A {
  final Type? type;
  A(this.type);
}

void f(A x) {
  if (x case A(type: int)) {}
}
",
    );
    assert_eq!(
        f.run(&one_rule("type_literal_in_constant_pattern")).len(),
        1,
        "a Type?-typed field is no exemption"
    );
}

/// The matched-value exemption's nullability face on the scrutinee:
/// `Type x` exempts, `Type? x` reports (the official
/// `isSubtypeOf(matchedValueType, typeType)` — `Type?` is no subtype
/// of `Type`; `test_constType_matchObjectNullable` shape).
#[test]
fn type_literal_nullable_type_scrutinee_reports() {
    let f = Fixture::parse("void f(Object x) { if (x case int) {} }");
    assert_eq!(
        f.run(&one_rule("type_literal_in_constant_pattern")).len(),
        1
    );
    let f = Fixture::parse("void f(Type? x) { if (x case int) {} }");
    assert_eq!(
        f.run(&one_rule("type_literal_in_constant_pattern")).len(),
        1,
        "a Type? matched value is not of type Type"
    );
    let f = Fixture::parse("void f(Type x) { if (x case int) {} }");
    assert!(
        f.run(&one_rule("type_literal_in_constant_pattern"))
            .is_empty()
    );
}

// —— require_trailing_commas —————————————————————————————————————————

const MULTILINE_ARGS: &str = "void f() {\n  g(\n    1,\n    2\n  );\n}\n";

/// `test/rules/require_trailing_commas.dart` `FunctionDeclaration`
/// multiline shape: a multi-line argument list without a trailing
/// comma reports at the closing paren.
#[test]
fn multiline_argument_list_reports() {
    let f = Fixture::parse(MULTILINE_ARGS);
    let lints = f.run(&one_rule("require_trailing_commas"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Missing a required trailing comma.");
    // The `)` closing g's argument list (the `f()` param list is empty
    // and skips).
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, ")", 1))
    );
}

#[test]
fn trailing_comma_argument_list_passes() {
    let f = Fixture::parse("void f() {\n  g(\n    1,\n    2,\n  );\n}\n");
    assert!(f.run(&one_rule("require_trailing_commas")).is_empty());
}

/// A group whose delimiters share a line needs no comma (:132-136).
#[test]
fn single_line_argument_list_passes() {
    let f = Fixture::parse("void f() {\n  g(1, 2);\n}\n");
    assert!(f.run(&one_rule("require_trailing_commas")).is_empty());
}

/// The registration gate: the rule registers below the 3.7 tall-style
/// milestone and stays silent from 3.7 (:52-56); an unset version face
/// applies no gate (the official null-version posture registers).
#[test]
fn version_gate_silences_the_rule_from_3_7() {
    let mut f = Fixture::parse(MULTILINE_ARGS);
    f.version = Some(LanguageVersion::new(3, 6));
    assert_eq!(
        f.run(&one_rule("require_trailing_commas")).len(),
        1,
        "below the milestone the rule registers and reports"
    );
    let mut f = Fixture::parse(MULTILINE_ARGS);
    f.version = Some(LanguageVersion::new(3, 7));
    assert!(
        f.run(&one_rule("require_trailing_commas")).is_empty(),
        "from 3.7 the formatter enforces the commas; the rule is silent"
    );
    let f = Fixture::parse(MULTILINE_ARGS);
    assert_eq!(f.run(&one_rule("require_trailing_commas")).len(), 1);
}

#[test]
fn multiline_formal_parameters_report() {
    let f = Fixture::parse("void f(\n  int a,\n  int b\n) {}\n");
    let lints = f.run(&one_rule("require_trailing_commas"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, ")", 0))
    );
}

/// The report pins the optional group's closer (:100-115,
/// `rightDelimiter ?? rightParenthesis`).
#[test]
fn named_group_reports_at_its_delimiter() {
    let f = Fixture::parse("void f(\n  int a, {\n  int b = 1\n}) {}\n");
    let lints = f.run(&one_rule("require_trailing_commas"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, "}", 0))
    );
}

#[test]
fn formal_parameters_trailing_comma_pass() {
    let f = Fixture::parse("void f(\n  int a,\n  int b,\n) {}\n");
    assert!(f.run(&one_rule("require_trailing_commas")).is_empty());
}

/// A block-bodied function literal as the multiline last argument is
/// exempt (:141-143).
#[test]
fn function_literal_last_argument_passes() {
    let f = Fixture::parse("void f() {\n  g(\n    1,\n    () {\n      h();\n    }\n  );\n}\n");
    assert!(f.run(&one_rule("require_trailing_commas")).is_empty());
}

/// A multiline string literal as the last argument is exempt
/// (:145-147); a single-line one is not (:137-139).
#[test]
fn string_literal_last_argument_exception_follows_multiline() {
    let f = Fixture::parse("void f() {\n  g(\n    1,\n    '''\nhi\n'''\n  );\n}\n");
    assert!(f.run(&one_rule("require_trailing_commas")).is_empty());
    let f = Fixture::parse("void f() {\n  g(\n    1,\n    'two'\n  );\n}\n");
    assert_eq!(f.run(&one_rule("require_trailing_commas")).len(), 1);
}

/// A collection literal as the multiline last argument is exempt
/// (:159-160).
#[test]
fn collection_last_argument_passes() {
    let f = Fixture::parse("void f() {\n  g(\n    1,\n    [\n      2,\n    ]\n  );\n}\n");
    assert!(f.run(&one_rule("require_trailing_commas")).is_empty());
}

#[test]
fn multiline_list_literal_reports() {
    let f = Fixture::parse("void f() {\n  var l = [\n    1,\n    2\n  ];\n}\n");
    let lints = f.run(&one_rule("require_trailing_commas"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, "]", 0))
    );
}

#[test]
fn multiline_assert_reports_and_trailing_comma_passes() {
    let f = Fixture::parse("void f(bool a) {\n  assert(\n    a,\n    'msg'\n  );\n}\n");
    let lints = f.run(&one_rule("require_trailing_commas"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, ")", 1))
    );
    let f = Fixture::parse("void f(bool a) {\n  assert(\n    a,\n    'msg',\n  );\n}\n");
    assert!(f.run(&one_rule("require_trailing_commas")).is_empty());
    let f = Fixture::parse("void f(bool a) {\n  assert(a, 'msg');\n}\n");
    assert!(f.run(&one_rule("require_trailing_commas")).is_empty());
}

/// The extension-type primary constructor's parameter list skips
/// (:89-96).
#[test]
fn extension_type_parameters_skip() {
    let f = Fixture::parse("extension type E(\n  int i\n) {}\n");
    assert!(f.run(&one_rule("require_trailing_commas")).is_empty());
}

/// An anonymous function call as the multiline last argument is exempt
/// when its own argument parens share a line (:149-157).
#[test]
fn anonymous_function_call_last_argument_passes() {
    let f = Fixture::parse(
        "void f() {\n  assert(\n    () {\n      return true;\n    }()\n  );\n}\n",
    );
    assert!(f.run(&one_rule("require_trailing_commas")).is_empty());
}

/// Pin: a multiline anonymous call whose own argument parens span
/// lines fails the `anonymous_call_on_one_line` exemption — the
/// `token_bytes` lookups over `args_span.start`/`args_span.end - 1`
/// resolve to different lines, so the enclosing group still reports.
/// Official visits both argument lists: the inner `}(1, 2)` list also
/// reports, since its last argument fits on one line and grants no
/// exception.
#[test]
fn anonymous_function_call_multiline_own_args_reports() {
    let f = Fixture::parse(
        "void f() {\n  g(\n    () {\n      h();\n    }(\n      1,\n      2\n    )\n  );\n}\n",
    );
    let lints = f.run(&one_rule("require_trailing_commas"));
    assert_eq!(lints.len(), 2, "{lints:?}");
    // The enclosing `g(...)`'s `)`, then the inner invocation's `)`.
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, ")", 4))
    );
    assert_eq!(
        lints[1].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, ")", 3))
    );
}
