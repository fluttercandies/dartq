use super::{Check, NodeKind, RuleSpec};

/// Official: `pkg/linter/lib/src/rules/prefer_is_not_operator.dart:33-55`
/// — `!(x is T)` (the `is` expression wrapped in parentheses under a
/// `!` prefix) reports at the prefix expression. The parenthesized +
/// prefix shape is required by the official parent chain
/// (`parent is ParenthesizedExpression && parent.parent is
/// PrefixExpression`, :38-54). Message: `diagnostic.g.dart`
/// `preferIsNotOperator`.
///
/// dartq faces over the production tiling: the `!` prefix is a
/// childless leaf emitted after its operand completes, so the check
/// rides the `is` expression's paren container and reads the row that
/// follows it in the owner window — the bare [`NodeKind::UnaryPrefix`]
/// leaf, or a container that adopted it as its head child (the
/// parenthesized condition of an `if`/`while`/`for` adopts the leaf
/// alone; a binary's adopted-begin lays the leaf back as its first
/// child when the paren operand finalizes lazily). A direct chain
/// (paren directly under the prefix) is the official shape, so a
/// double paren `!((x is int))` stays silent, as does a postfix
/// operand (`x!.m`).
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_is_not_operator",
    &[NodeKind::IsExpression],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `node.notOperator != null` bails (:39-42): payload word 0 is the
    // `not` flag of the `IsExpression` payload.
    if c.payload(node).first().copied().unwrap_or(0) != 0 {
        return;
    }
    let Some(paren) = c.ancestor(1) else {
        return;
    };
    if c.kind(paren) != NodeKind::ParenthesizedExpression {
        return;
    }
    let Some(owner) = c.ancestor(2) else {
        return;
    };
    let kids = c.collect_direct(owner);
    let Some(pos) = kids.iter().position(|&kid| kid == paren) else {
        return;
    };
    // The prefix read: the row after the paren carries the `!` —
    // directly, or as an adopting container's head child.
    let bang = match kids.get(pos + 1) {
        Some(&only) if c.kind(only) == NodeKind::UnaryPrefix => only,
        Some(&container)
            if matches!(
                c.kind(container),
                NodeKind::ParenthesizedCondition | NodeKind::BinaryExpression
            ) =>
        {
            match c.collect_direct(container).first() {
                Some(&head) if c.kind(head) == NodeKind::UnaryPrefix => head,
                _ => return,
            }
        }
        _ => return,
    };
    // Prefix `!` only (`operator.type == TokenType.BANG`, :46-48).
    if c.lexeme(c.head_token(bang)) != "!" {
        return;
    }
    // The official reports at the prefix expression: from the `!`
    // through the paren's close.
    let range = c.utf16_span(bang).start..c.utf16_span(paren).end;
    c.report(range, &[]);
}
