use super::{Check, Group, NodeKind, RuleSpec};

/// Official: `pkg/linter/lib/src/rules/unnecessary_null_in_if_null_operators.dart:34-49`
/// — `a ?? null` (or `null ?? a`) reports at the `null` literal, right
/// operand first. The operator is payload word 0, operands the two
/// children (the `BinaryExpression` payload). Message:
/// `diagnostic.g.dart` `unnecessaryNullInIfNullOperators`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "unnecessary_null_in_if_null_operators",
    group: Group::Recommended,
    problem: "Unnecessary use of '??' with 'null'.",
    correction: "Try removing the '??' operator and the 'null' operand.",
    kinds: &[NodeKind::BinaryExpression],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    let Some(&op) = c.payload(node).first() else {
        return;
    };
    if c.lexeme(op) != "??" {
        return;
    }
    let (left, right) = {
        let kids = c.direct_children(node);
        if kids.len() < 2 {
            return;
        }
        (kids[0], kids[1])
    };
    let null_operand = if c.kind(right) == NodeKind::LiteralNull {
        Some(right)
    } else if c.kind(left) == NodeKind::LiteralNull {
        Some(left)
    } else {
        None
    };
    if let Some(operand) = null_operand {
        let range = c.utf16_span(operand);
        c.report(range, &[]);
    }
}
