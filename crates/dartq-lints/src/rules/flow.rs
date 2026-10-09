//! `curly_braces_in_flow_control_structures` and `control_flow_in_finally`
//! — the flow-control family (one core, one recommended rule).

use dartq_ast::NodeKind;

use crate::engine::Check;
use crate::registry::{Group, RuleSpec};
use crate::rules::{NONE_TOKEN, first_child, last_child};

/// Official: `pkg/linter/lib/src/rules/curly_braces_in_flow_control_structures.dart:36-95`
/// — control-flow bodies must be blocks, except a single-line `if` without
/// `else` (`if (c) return;` on one line is allowed, :59-70). Message:
/// `diagnostic.g.dart` `curlyBracesInFlowControlStructures` with the
/// construct spelled into `{0}`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "curly_braces_in_flow_control_structures",
    group: Group::Core,
    problem: "Statements in {0} should be enclosed in a block.",
    correction: "Try wrapping the statement in a block.",
    kinds: &[
        NodeKind::IfStatement,
        NodeKind::WhileStatement,
        NodeKind::ForStatement,
        NodeKind::DoWhileStatement,
    ],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    match c.kind(node) {
        NodeKind::WhileStatement => {
            // Body is the last child (the condition is the first,
            // collapse map §3.4 WhileStatement row).
            if let Some(body) = last_child(c, node) {
                check_body(c, body, "a while");
            }
        }
        NodeKind::ForStatement => {
            // Body is the last child after the for-parts (:82-84).
            if let Some(body) = last_child(c, node) {
                check_body(c, body, "a for");
            }
        }
        NodeKind::DoWhileStatement => {
            // Body is the first child, before the `while (...)` condition
            // (collapse map §3.4 DoWhileStatement row: body = the children
            // segment before the While token).
            if let Some(body) = first_child(c, node) {
                check_body(c, body, "a do");
            }
        }
        NodeKind::IfStatement => check_if(c, node),
        _ => {}
    }
}

/// `_check` (curly_braces...dart:93-95): a non-block body reports.
fn check_body(c: &mut Check<'_>, body: u32, where_: &str) {
    if c.kind(body) != NodeKind::Block {
        let range = c.utf16_span(body);
        c.report(range, &[where_]);
    }
}

fn check_if(c: &mut Check<'_>, node: u32) {
    let else_present = c.payload(node).first().copied() != Some(NONE_TOKEN);
    // Direct children: condition, then-arm, [else-arm]. The scratch guard is
    // scoped so the reporting calls below can take `&mut Check`.
    let arms = {
        let kids = c.direct_children(node);
        (
            kids.get(1).copied(),
            kids.get(2).copied(),
            last_child(c, node),
        )
    };
    let Some(then) = arms.0 else {
        return;
    };
    if else_present {
        // With an `else`, both arms are checked; an else-if chain arm is
        // skipped (`elseStatement is! IfStatement`, :71-77).
        check_body(c, then, "an if");
        if let Some(else_stmt) = arms.1 {
            if c.kind(else_stmt) != NodeKind::IfStatement {
                check_body(c, else_stmt, "an if");
            }
        }
        return;
    }
    // An `if` without `else` that IS the else branch of an outer if: the
    // outer chain already requires blocks, so the then-arm is checked
    // unconditionally (:55-61).
    if let Some(parent) = c.ancestor(1) {
        if c.kind(parent) == NodeKind::IfStatement
            && c.payload(parent).first().copied() != Some(NONE_TOKEN)
            && arms.2 == Some(node)
        {
            check_body(c, then, "an if");
            return;
        }
    }
    // Single-line form allowed (:62-69): report only when the statement
    // spans lines (`lineOf(if) != lineOf(then.end)`).
    if c.kind(then) == NodeKind::Block {
        return;
    }
    let if_line = c
        .unit
        .lines
        .line_col_utf16(c.unit.source, c.utf16_span(node).start)
        .0;
    let then_end = c.utf16_span(then).end;
    let then_line = c.unit.lines.line_col_utf16(c.unit.source, then_end).0;
    if if_line != then_line {
        let range = c.utf16_span(then);
        c.report(range, &["an if"]);
    }
}

pub(crate) mod control_flow_in_finally {
    use super::{Check, Group, NONE_TOKEN, NodeKind, RuleSpec, last_child};

    /// Official: `pkg/linter/lib/src/rules/control_flow_in_finally.dart:24-107`
    /// — `break`/`continue`/`return` inside a `finally` block reports, unless
    /// the statement sits inside a closure within the block (its jump binds
    /// locally), or — for targeted `break`/`continue` — the target is inside
    /// the try statement (the jump stays local to it). Message:
    /// `diagnostic.g.dart` `controlFlowInFinally`.
    ///
    /// Approximation (documented): the official resolves the break/continue
    /// target through the label scope; here an untargeted statement uses its
    /// nearest enclosing loop/switch on the path and a targeted one its
    /// nearest enclosing `LabeledStatement`/loop — for valid code the
    /// enclosing-label chain is the target chain, so only nested labels
    /// wrapping the try can diverge.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "control_flow_in_finally",
        group: Group::Recommended,
        problem: "Use of '{0}' in a 'finally' clause.",
        correction: "Try restructuring the code.",
        kinds: &[
            NodeKind::Break,
            NodeKind::Continue,
            NodeKind::ReturnStatement,
        ],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let node_idx = c.path.len() - 1;
        let kind = match c.kind(node) {
            NodeKind::Break => "break",
            NodeKind::Continue => "continue",
            NodeKind::ReturnStatement => "return",
            _ => return,
        };
        // `node.thisOrAncestorOfType<TryStatement>()` (:45-47).
        let Some(try_idx) = c
            .path
            .iter()
            .rposition(|n| c.kind(*n) == NodeKind::TryStatement)
        else {
            return;
        };
        let try_node = c.path[try_idx];
        // `finallyBlock == null` bails (:48-54): payload word 0 is the
        // `finally` token (collapse map §3.4 TryStatementEnd).
        if c.payload(try_node).first() == Some(&NONE_TOKEN) {
            return;
        }
        // The node must be inside the finally block — the try's last child
        // (body block, catch clauses, then the finally block).
        let Some(finally_block) = last_child(c, try_node) else {
            return;
        };
        if c.kind(finally_block) != NodeKind::Block
            || c.path.get(try_idx + 1) != Some(&finally_block)
        {
            return;
        }
        let report = match c.kind(node) {
            NodeKind::Break | NodeKind::Continue => {
                let has_target = c.payload(node).first().copied().unwrap_or(0) != 0;
                let target = nearest(c, node_idx, |kind| match kind {
                    NodeKind::WhileStatement
                    | NodeKind::ForStatement
                    | NodeKind::ForIn
                    | NodeKind::DoWhileStatement
                    | NodeKind::SwitchStatement => true,
                    NodeKind::LabeledStatement => has_target,
                    _ => false,
                });
                match target {
                    // `ancestor.thisOrAncestorMatching(n == tryStatement)`
                    // (:79-91): the enabler exists when the jump target's own
                    // ancestor chain contains the try statement — the target
                    // sits below the try on the path (idx > try_idx) and the
                    // jump stays local to it.
                    Some(idx) => idx <= try_idx,
                    // Unresolved target: the closure exemption decides
                    // (`ancestor == null` branch, :69-77).
                    None => !closure_wraps(c, try_idx + 1, node_idx),
                }
            }
            _ => !closure_wraps(c, try_idx + 1, node_idx),
        };
        if report {
            let range = c.utf16_span(node);
            c.report(range, &[kind]);
        }
    }

    /// The nearest path index below `from` whose node kind satisfies `pred`.
    fn nearest(c: &Check<'_>, from: usize, pred: impl Fn(NodeKind) -> bool) -> Option<usize> {
        (0..from).rev().find(|&i| pred(c.kind(c.path[i])))
    }

    /// Whether a closure (function expression / local function) sits on the
    /// path between the finally block and the statement — the official
    /// `FunctionBody inside the finally block` exemption (:69-77).
    fn closure_wraps(c: &Check<'_>, finally_idx: usize, node_idx: usize) -> bool {
        (finally_idx + 1..=node_idx).any(|i| {
            matches!(
                c.kind(c.path[i]),
                NodeKind::FunctionExpression | NodeKind::LocalFunctionDeclaration
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use dartq_ast::NodeKind;

    use crate::testsupport::{Fixture, NONE, identifier, one_rule, unit_of, utf16_at};

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
}
