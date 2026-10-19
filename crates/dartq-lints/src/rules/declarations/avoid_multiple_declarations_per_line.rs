use super::{Check, Group, NodeKind, RuleSpec};

/// Official: `pkg/linter/lib/src/rules/avoid_multiple_declarations_per_line.dart:30-49`
/// — a declaration list with more than one variable reports once, at
/// the second variable's name (despite the name, the official face is
/// per-declaration-list, not per-line). The classic `for` initializer
/// list skips (:38-40 `ForPartsWithDeclarations`). Message:
/// `diagnostic.g.dart` `avoidMultipleDeclarationsPerLine`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "avoid_multiple_declarations_per_line",
    group: Group::Standalone,
    problem: "Multiple variables declared on a single line.",
    correction: "Try splitting the variable declarations into multiple lines.",
    kinds: &[NodeKind::VariablesDeclaration],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `parent is ForPartsWithDeclarations` (:38-40): the production
    // wrapper parents the for initializer's declaration list straight
    // to the `ForStatement` row (probe), the hand-built shape uses a
    // `ForParts` row — both owner faces skip.
    if c.ancestor(1).is_some_and(|parent| {
        matches!(c.kind(parent), NodeKind::ForStatement | NodeKind::ForParts)
    }) {
        return;
    }
    let second = c
        .collect_direct(node)
        .into_iter()
        .filter(|&kid| c.kind(kid) == NodeKind::InitializedIdentifier)
        .nth(1);
    let Some(second) = second else {
        return;
    };
    let Some(name) = crate::rules::initialized_identifier_name(c, second) else {
        return;
    };
    c.report(crate::rules::utf16_token_span(c, name), &[]);
}
