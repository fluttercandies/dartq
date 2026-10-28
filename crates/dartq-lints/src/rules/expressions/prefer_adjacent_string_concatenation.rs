use super::{Check, NodeKind, PayloadField, RuleSpec};

/// Official: `pkg/linter/lib/src/rules/prefer_adjacent_string_concatenation.dart:37-46`
/// — `+` whose two operands are both string literals reports at the
/// `+` (adjacent strings need no operator). Message: `diagnostic.g.dart`
/// `preferAdjacentStringConcatenation`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_adjacent_string_concatenation",
    &[NodeKind::BinaryExpression],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    let Some(op) = c.field(node, PayloadField::Operator) else {
        return;
    };
    if c.lexeme(op) != "+" {
        return;
    }
    let string_pair = {
        let kids = c.direct_children(node);
        kids.len() >= 2
            && c.kind(kids[0]) == NodeKind::StringLiteral
            && c.kind(kids[1]) == NodeKind::StringLiteral
    };
    if !string_pair {
        return;
    }
    let Some(start) = c.utf16_of_token(op) else {
        return;
    };
    c.report(start..start, &[]);
}
