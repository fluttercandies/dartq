use super::{Check, NodeKind, PayloadField, RuleSpec, operator_name};
use crate::rules::{member_name, same_file_declaration, utf16_token_span};

/// Official: `pkg/linter/lib/src/rules/avoid_equals_and_hash_code_on_mutable_classes.dart:43-58`
/// — an `operator ==` or `hashCode` member of a class (never a mixin,
/// enum or extension type — `thisOrAncestorOfType<ClassDeclaration>`,
/// :61-64) whose class and every supertype lack `@immutable` reports at
/// the member's first token after comment and metadata (:51-54), with
/// the member name as the `{0}` argument (`node.name.lexeme`).
///
/// Approximation (documented): the resolved `hasImmutableAnnotation`
/// walks every supertype's metadata (`extensions.dart:254-257`); the
/// lexical face reads the class row's own annotations plus the
/// same-file-resolvable ancestor chain and bails the whole face when
/// any written supertype leaves the file — an inherited `@immutable`
/// (Flutter's `Widget`, an `Equatable` base) under-reports rather than
/// risks a false report. The annotation answers by its identifier
/// lexeme (`immutable` — the `package:meta` constant's unprefixed
/// face). Augmentation members (`node.isAugmentation`, :46) are out of
/// the substrate. A getter without a written return type and carrying
/// metadata reports at the name where the official reports at the
/// `get` keyword (a one-token shift on that rare shape). Message:
/// `diagnostic.g.dart` `avoidEqualsAndHashCodeOnMutableClasses`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "avoid_equals_and_hash_code_on_mutable_classes",
    &[NodeKind::Method],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `node.name.type == TokenType.EQ_EQ || isHashCode(node)` (:48) —
    // `==` names through the `OperatorName` payload, `hashCode` through
    // the member identifier (getter or method).
    let Some(name) = equality_or_hash_code(c, node) else {
        return;
    };
    // `node.classElement` (:61-64): the nearest CLASS row only.
    let Some(&class_row) = c
        .path
        .iter()
        .rev()
        .find(|&&a| c.kind(a) == NodeKind::ClassDeclaration)
    else {
        return;
    };
    // `!classElement.hasImmutableAnnotation` (:50).
    if !mutable_without_immutable(c, class_row) {
        return;
    }
    let first = first_member_token(c, node);
    c.report(utf16_token_span(c, first), &[name]);
}

/// The reported `{0}` name: `"=="` for the operator face, `"hashCode"`
/// for the identifier face, `None` otherwise.
fn equality_or_hash_code(c: &Check<'_>, node: u32) -> Option<&'static str> {
    // The production `OperatorName` row tiles as the method's left
    // sibling inside `DeclarationBody`, not as a child — read it through
    // [`operator_name`] which covers both faces.
    if let Some(op_row) = operator_name(c, node)
        && c.field_token(op_row, PayloadField::Name)
            .is_some_and(|t| c.lexeme(t) == "==")
    {
        return Some("==");
    }
    let name = member_name(c, node)?;
    (c.lexeme(c.head_token(name)) == "hashCode").then_some("hashCode")
}

/// `hasImmutableAnnotation` (`extensions.dart:254-257`) over the
/// same-file-resolvable chain: the class and every written supertype
/// must lack `immutable`; an unresolved supertype name bails `true`'s
/// caller through `false` (the under-report).
fn mutable_without_immutable(c: &Check<'_>, class_row: u32) -> bool {
    let mut seen = vec![class_row];
    let mut queue = vec![class_row];
    while let Some(current) = queue.pop() {
        if super::top_level_annotations(c, current)
            .iter()
            .any(|&meta| super::annotation_names(c, meta, "immutable"))
        {
            return false;
        }
        for (name, _) in crate::rules::supertypes(c, current) {
            let Some(row) = same_file_declaration(c, name) else {
                return false; // the chain leaves the file — under-report
            };
            if !seen.contains(&row) {
                seen.push(row);
                queue.push(row);
            }
        }
    }
    true
}

/// The member's first token after comment and metadata (:51-54): the
/// written return type's head when a `Type` row precedes the name (the
/// row anchors at the name), else the `get`/`set`/`operator` keyword,
/// else the name itself.
fn first_member_token(c: &Check<'_>, node: u32) -> u32 {
    // The member's written return type is its first `Type` child (the
    // `OperatorName` and the `operator` keyword tile outside the Method
    // row, so the head token would land on `==`); report at the type's
    // head — `getFirstTokenAfterCommentAndMetadata`.
    if let Some(type_row) = c
        .collect_direct(node)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Type)
    {
        return c.head_token(type_row);
    }
    let head = c.head_token(node);
    match head.checked_sub(1) {
        Some(prev) if matches!(c.lexeme(prev), "get" | "set" | "operator") => prev,
        _ => head,
    }
}
