use super::{Check, NodeKind, RuleSpec};

/// Official: `pkg/linter/lib/src/rules/unnecessary_new.dart:37-45` —
/// every `new` keyword of an instance creation reports at the keyword
/// (`reportAtToken(keyword)`, spanning it). `new` is the span head
/// (`NewExpression` is a width-0 kind: the derivable word is eliminated
/// into `header.first_token`).
/// Message: `diagnostic.g.dart` `unnecessaryNew`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unnecessary_new",
    &[NodeKind::NewExpression],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    let Some(start) = c.utf16_of_token(c.head_token(node)) else {
        return;
    };
    let end = c.utf16_end_of_token(c.head_token(node)).unwrap_or(start);
    c.report(start..end, &[]);
}
