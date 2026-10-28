use super::{Check, NodeKind, RuleSpec};
use crate::rules::{const_keyword, in_const_context};

/// Official: `pkg/linter/lib/src/rules/unnecessary_const.dart:38-77` — a
/// `const` keyword on a literal / instance creation already inside a
/// constant context reports at the keyword.
///
/// The context face is the official `constantContext` walk (ported as
/// [`crate::rules::in_const_context`] — the rule declares
/// `canUseParsedResult`, so the walk is the whole story). The visitor's
/// `ConstantPattern` exemptions are structural with our payload layout: a
/// constant pattern owns the `const` keyword itself (`ConstantPattern`
/// head-when-present), so the wrapped expression
/// carries none and never dispatches. Dot-shorthand const invocations
/// (`case const .new()`) are likewise pattern-owned and skip. Message:
/// `diagnostic.g.dart` `unnecessaryConst`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unnecessary_const",
    &[
        NodeKind::ConstExpression,
        NodeKind::ConstLiteral,
        NodeKind::ListLiteral,
        NodeKind::SetOrMapLiteral,
        NodeKind::RecordLiteral,
    ],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `keyword == null || keyword.type != CONST` bails (:57-59).
    let Some(keyword) = const_keyword(c, node) else {
        return;
    };
    // `node.inConstantContext` (:60-62).
    if !in_const_context(c) {
        return;
    }
    let Some(start) = c.utf16_of_token(keyword) else {
        return;
    };
    c.report(start..start, &[]);
}
