use super::{Check, NodeKind, RuleSpec, slice};
use crate::rules::{first_child, last_child};

/// Official: `pkg/linter/lib/src/rules/unnecessary_null_aware_assignments.dart:42-49`
/// — `x ??= null` reports at the assignment; the write must not be a
/// setter (`node.writeElement is SetterElement` bails, :43), i.e.
/// property assignments (`a.b ??= null`) stay meaningful.
///
/// Approximation (documented): the setter exclusion is taken from the
/// access shape — a [`NodeKind::DotAccess`] LHS or any assignment inside
/// a [`NodeKind::Cascade`] skips. Other exotic write elements (e.g.
/// `x[0] ??= null`, whose write element is not a setter upstream) report,
/// matching the official branch. Message: `diagnostic.g.dart`
/// `unnecessaryNullAwareAssignments`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unnecessary_null_aware_assignments",
    &[NodeKind::Assignment],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `node.operator.type == QUESTION_QUESTION_EQ &&
    // node.rightHandSide.isNullLiteral` (:45-46).
    if c.payload_word(node, 0).map(|op| c.lexeme(op)) != Some("??=") {
        return;
    }
    let Some(rhs) = last_child(c, node) else {
        return;
    };
    if c.kind(rhs) != NodeKind::LiteralNull {
        return;
    }
    let Some(lhs) = first_child(c, node) else {
        return;
    };
    if matches!(c.kind(lhs), NodeKind::DotAccess | NodeKind::CascadeAccess) {
        return;
    }
    if c.path.iter().any(|&a| c.kind(a) == NodeKind::Cascade) {
        return;
    }
    // The official reports at the whole assignment expression; the
    // production row opens at the operator's tile anchor and swallows
    // the trailing `;`, so the extent runs from the left operand's
    // first token to the `null` literal's own end.
    let Some(op_token) = c.payload_word(node, 0) else {
        return;
    };
    let Some((left, _)) = slice::binary_operands(c) else {
        return;
    };
    let Some(start) = slice::operand_utf16_start(c, &left, op_token) else {
        return;
    };
    let range = start..c.utf16_span(rhs).end;
    c.report(range, &[]);
}
