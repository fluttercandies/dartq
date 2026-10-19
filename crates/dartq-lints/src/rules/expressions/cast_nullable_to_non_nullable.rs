use super::{Check, Group, NodeKind, RuleSpec, slice, static_type_faces as faces};
use crate::rules::{type_face, utf16_token_span};

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/cast_nullable_to_non_nullable.dart` — an
/// `as` cast whose source static type is nullable and whose written
/// target is non-nullable reports at the cast expression (:38-49; the
/// rewrite is the `!` assertion). `dynamic` sources are excluded
/// (:41-42 — the downcast is legal there). The dartq face reads the
/// source's flow type at the operand run and the written target row
/// inside the cast container (`Type`'s `?` tail). Approximation
/// (documented, under-report): an operand without a static-type face
/// (a chain, a creation, a literal) stays silent. Message:
/// `diagnostic.g.dart` `castNullableToNonNullable`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "cast_nullable_to_non_nullable",
    group: Group::Standalone,
    problem: "Don't cast a nullable value to a non-nullable type.",
    correction: "Try adding a not-null assertion ('!') to make the type non-nullable.",
    kinds: &[NodeKind::AsExpression],
    check,
};

fn check(c: &mut Check<'_>) {
    // The wave posture: the rule reads the semantic face; face absent
    // is the plain-run posture and stays silent (the engine contract).
    let Some(analysis) = c.semantics() else {
        return;
    };
    let node = c.node();
    // The operand tiles as the owner siblings before the cast
    // container (the container anchors at the `as` operator).
    let Some((kids, pos)) = slice::owner_slice(c) else {
        return;
    };
    let operand = slice::statement_run(c, &kids[..pos]);
    // The source gate (:41-45): nullable, and never `dynamic`.
    match faces::expr_type(c, analysis, &operand) {
        Some(ty) if faces::is_nullable(ty) => {}
        _ => return,
    }
    // The target gate (:45-46): the written type is non-nullable.
    let Some(type_row) = c
        .collect_direct(node)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Type)
    else {
        return;
    };
    let face = type_face(c, type_row);
    if face.nullable || face.name.is_none() {
        return;
    }
    let Some(first) = operand.first() else {
        return;
    };
    let range = utf16_token_span(c, c.head_token(*first)).start..c.utf16_span(node).end;
    c.report(range, &[]);
}
