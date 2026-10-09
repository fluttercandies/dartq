//! `curly_braces_in_flow_control_structures` — the flow-control body shape
//! rule (core set).

use dartq_ast::NodeKind;

use crate::engine::Check;
use crate::registry::{Group, RuleSpec};
use crate::rules::{first_child, last_child, NONE_TOKEN};

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

#[cfg(test)]
mod tests {
    use dartq_ast::NodeKind;

    use crate::testsupport::{identifier, NONE, one_rule, utf16_at, Fixture, unit_of};

    /// `test/rules/curly_braces_in_flow_control_structures_test.dart`
    /// `test_if_multiline` — a multi-line arm without a block reports.
    #[test]
    fn multiline_if_without_block_reports() {
        let source = "if (b)\n  x;";
        // if0 (1 b2 )3 x4 ;5
        let f = Fixture::build(source, unit_of(|b, _t| {
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
        }));
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
        let f = Fixture::build(source, unit_of(|b, _t| {
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
        }));
        assert!(
            f.run(&one_rule("curly_braces_in_flow_control_structures"))
                .is_empty()
        );
    }

    #[test]
    fn block_arm_passes() {
        let source = "if (b) { x; }";
        // if0 (1 b2 )3 {4 x5 ;6 }7
        let f = Fixture::build(source, unit_of(|b, _t| {
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
        }));
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
        let f = Fixture::build(source, unit_of(|b, _t| {
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
        }));
        let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 7));
    }

    /// `while` bodies always need a block (`test_while`).
    #[test]
    fn while_body_reports() {
        let source = "while (b) ;";
        let f = Fixture::build(source, unit_of(|b, _t| {
            b.begin(NodeKind::WhileStatement, 0);
            identifier(b, 2, 3);
            b.leaf(NodeKind::EmptyStatement, 4);
            b.end(NodeKind::WhileStatement, 5);
        }));
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
        let f = Fixture::build(source, unit_of(|b, _t| {
            b.begin(NodeKind::DoWhileStatement, 0);
            b.begin(NodeKind::ExpressionStatement, 1);
            b.leaf_span(NodeKind::Send, 1, 2);
            b.end(NodeKind::ExpressionStatement, 3);
            identifier(b, 5, 6);
            b.end(NodeKind::DoWhileStatement, 8);
        }));
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
        let f = Fixture::build(source, unit_of(|b, _t| {
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
        }));
        let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Statements in a for should be enclosed in a block."
        );
    }
}
