use super::{Check, NodeKind, RuleSpec};

/// Official:
/// `pkg/linter/lib/src/rules/unnecessary_primary_constructor_body.dart:42-53`
/// — a primary constructor body with no metadata, no documentation
/// comment, and no initializers whose body is empty (`;` or `{}`)
/// reports at the `this`. The rule registers unconditionally — no
/// feature gate. Message: `diagnostic.g.dart`
/// `unnecessaryPrimaryConstructorBody`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unnecessary_primary_constructor_body",
    &[NodeKind::PrimaryConstructorBody],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // The row anchors at the body delimiter (`;`/`{`/`:`), so the
    // `this` keyword — upstream's `node.thisKeyword`, the metadata /
    // doc-comment anchor and the report token — sits just before the
    // head.
    let head = c.head_token(node);
    let this = (head.saturating_sub(2)..head)
        .rev()
        .find(|&token| c.lexeme(token) == "this")
        .unwrap_or(head);
    // `node.metadata.isNotEmpty` (:44) — the member's metadata star is
    // the left sibling in the body's child list.
    if has_metadata(c, node) {
        return;
    }
    // `node.documentationComment != null` (:45) — the comment attaches
    // to the `this` token.
    if c.comments.attached(this as usize).iter().any(|comment| {
        let text = &c.unit.source[comment.range.start as usize..comment.range.end as usize];
        text.starts_with("///") || text.starts_with("/**")
    }) {
        return;
    }
    // `node.initializers.isNotEmpty` (:46).
    if c.collect_direct(node)
        .iter()
        .any(|&kid| c.kind(kid) == NodeKind::Initializers)
    {
        return;
    }
    // `body is EmptyFunctionBody` (:48-49) or a `BlockFunctionBody`
    // whose block is empty (:50-52); an arrow body never reports.
    let empty = c
        .collect_direct(node)
        .into_iter()
        .any(|kid| match c.kind(kid) {
            NodeKind::EmptyBody => true,
            // A `BlockFunctionBody` whose block is empty (:50-52): the
            // production `BlockBody` parents its statement rows
            // directly — empty means no children at all.
            NodeKind::BlockBody => c.collect_direct(kid).is_empty(),
            _ => false,
        });
    if !empty {
        return;
    }
    // `rule.reportAtToken(node.thisKeyword)` (:49/:51).
    c.report(crate::rules::utf16_token_span(c, this), &[]);
}

/// Whether the row's left sibling in its parent's child list is a
/// non-empty metadata star (`metadata.isNotEmpty`: the star row always
/// tiles; only a populated one means annotations are present).
fn has_metadata(c: &Check<'_>, node: u32) -> bool {
    let Some(parent) = c.unit.ast.parent(node) else {
        return false;
    };
    let siblings = c.collect_direct(parent);
    let Some(pos) = siblings.iter().position(|&m| m == node) else {
        return false;
    };
    pos > 0
        && c.kind(siblings[pos - 1]) == NodeKind::MetadataStar
        && !c.collect_direct(siblings[pos - 1]).is_empty()
}
