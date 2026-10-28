use super::{Check, NodeKind, RuleSpec};

/// Official: `pkg/linter/lib/src/rules/avoid_single_cascade_in_expression_statements.dart:39-61`
/// — an expression statement consisting of a single-section cascade
/// reports at the cascade, with the equivalent `.`-operator spelled in
/// the message (`?.` for a `?..` section, `avoid_single_cascade...dart:40-51`).
/// Message: `diagnostic.g.dart` `avoidSingleCascadeInExpressionStatements`.
///
/// dartq faces over the production tiling: the cascade's window holds
/// its sections' selector pieces beside the childless `CascadeAccess`
/// markers (`buffer..write('x')` yields `[Send, CascadeAccess]`), so
/// the section count reads the marker rows; the statement face reads
/// the span-only `ExpressionStatement` leaf that trails the cascade
/// rows in the owner window (the statement's expression completes
/// before the leaf is stamped), and the leaf's span must cover the
/// cascade's. The official reports at the cascade expression — the
/// production row opens at the `..`, so the statement head (the
/// target) bounds the start.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "avoid_single_cascade_in_expression_statements",
    &[NodeKind::Cascade],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // The sections: the direct `CascadeAccess` markers (the selector
    // pieces tile beside them inside the cascade's window).
    let sections: Vec<u32> = c
        .collect_direct(node)
        .into_iter()
        .filter(|&kid| c.kind(kid) == NodeKind::CascadeAccess)
        .collect();
    let [section] = sections.as_slice() else {
        return;
    };
    // The statement face: the span-only `ExpressionStatement` leaf
    // trails the cascade rows in the owner window and covers the
    // cascade's extent. A multi-section cascade tiles each `..x` as
    // its own `Cascade` sibling (`a..b()..c()` keeps an earlier
    // `Cascade` left of the last one) — only the sole-section shape
    // reports.
    let Some(parent) = c.ancestor(1) else {
        return;
    };
    let kids = c.collect_direct(parent);
    let Some(pos) = kids.iter().position(|&kid| kid == node) else {
        return;
    };
    if kids[..pos]
        .iter()
        .any(|&kid| c.kind(kid) == NodeKind::Cascade)
    {
        return;
    }
    let Some(&statement) = kids.get(pos + 1) else {
        return;
    };
    if c.kind(statement) != NodeKind::ExpressionStatement
        || c.utf16_span(statement).start > c.utf16_span(node).start
        || c.utf16_span(statement).end < c.utf16_span(node).end
    {
        return;
    }
    // The section's cascade operator decides the message argument
    // (avoid_single_cascade...dart:40-51): `?..` prints as `?.`.
    let operator = if c.payload(*section).get(1).copied().unwrap_or(0) != 0 {
        "?."
    } else {
        "."
    };
    let range = c.utf16_span(statement).start..c.utf16_span(node).end;
    c.report(range, &[operator]);
}
