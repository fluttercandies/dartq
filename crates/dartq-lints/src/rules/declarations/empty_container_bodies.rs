use super::{Check, Group, NodeKind, RuleSpec};

/// Official: `pkg/linter/lib/src/rules/empty_container_bodies.dart` —
/// an empty `{}` class body reports its brace pair with the container
/// kind word (:27-45). Registration is feature-gated: the
/// primary-constructors `isFeatureEnabled` bail (:35) — pinned on
/// [`crate::engine::Features::primary_constructors`].
///
/// Faces: `leftBracket.next == rightBracket` ⟺ no member rows (the
/// body's children are exactly the member rows), and
/// `rightBracket.precedingComments == null` ⟺ the source gap between
/// the braces is whitespace-only (the scanner drops comments, so the
/// bytes answer directly). The body span covers `{`..`}` inclusive
/// (bridge `begin_declaration_body`/`end_at`). Enum bodies are a
/// separate upstream face (`BlockEnumBody` is not registered) — the
/// dartq [`NodeKind::EnumBody`] is a different row kind, so it never
/// fires here. Message: `diagnostic.g.dart` `emptyContainerBodies`
/// (the `{0}` slot takes the container kind).
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "empty_container_bodies",
    group: Group::Standalone,
    problem: "Empty {0} bodies should be written using a ';' rather than '{}'.",
    correction: "Try replacing the {0} body with ';'.",
    kinds: &[NodeKind::DeclarationBody],
    check,
};

fn check(c: &mut Check<'_>) {
    if !c.unit.features.primary_constructors {
        return; // registration bail (:35)
    }
    let node = c.node();
    if !c.collect_direct(node).is_empty() {
        return; // `leftBracket.next == rightBracket` (:31)
    }
    // The empty body's span covers only the `{` — the `}` is the next
    // token (upstream's `rightBracket`, :30).
    let head = c.head_token(node);
    let Some(right) = c
        .unit
        .tokens
        .get(head as usize + 1)
        .filter(|_| c.lexeme(head + 1) == "}")
    else {
        return;
    };
    // `precedingComments == null` (:31): a comment is non-whitespace
    // bytes in the brace gap.
    let Some(open_end) = c.token(head).map(|t| t.byte_offset as usize + 1) else {
        return;
    };
    let gap = c
        .unit
        .source
        .get(open_end..right.byte_offset as usize)
        .unwrap_or_default();
    if !gap.chars().all(char::is_whitespace) {
        return;
    }
    let kind = match c.ancestor(1).map(|parent| c.kind(parent)) {
        Some(NodeKind::ClassDeclaration) => "class",
        Some(NodeKind::MixinDeclaration) => "mixin",
        Some(NodeKind::ExtensionDeclaration) => "extension",
        Some(NodeKind::ExtensionTypeDeclaration) => "extension type",
        _ => "container", // the registered faces never reach this
    };
    // The report covers the `{`..`}` pair (:33 `reportForSpan`).
    let start = c
        .unit
        .lines
        .byte_to_utf16(c.unit.source, (open_end - 1) as u32);
    let end = c
        .unit
        .lines
        .byte_to_utf16(c.unit.source, right.byte_offset + 1);
    c.report(start..end, &[kind]);
}
