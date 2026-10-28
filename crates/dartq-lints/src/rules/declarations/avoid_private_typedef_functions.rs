use super::{Check, NodeKind, RuleSpec};

/// Official: `pkg/linter/lib/src/rules/avoid_private_typedef_functions.dart`
/// — a private typedef used at most once reports at its name. Both
/// declaration forms count: the old `typedef int _Td();` (the official
/// `FunctionTypeAlias`) and the new `typedef _Td = int Function();`
/// (`GenericTypeAlias`) — the generic form skips generic parameters
/// (:61) and aliased named/record types (:62-63, `typedef _Td =
/// List<String>` is not a function typedef). The use count is the
/// official `_CountVisitor` over `context.allUnits` (:68-79) — the
/// driver-mounted library face ([`crate::engine::type_use_counts`] folded across
/// the registration-order units, the set
/// `Analysis::library_units` publishes) answers it by name lexeme, so a
/// typedef used only in sibling units counts exactly as upstream (the
/// former same-file face's cross-file divergence family is closed). A
/// run without the mounted face stays silent — no same-file fallback
/// count is fabricated. Message: `diagnostic.g.dart`
/// `avoidPrivateTypedefFunctions`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "avoid_private_typedef_functions",
    &[NodeKind::Typedef],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // New form: the generic-parameter and named/record-type faces skip
    // (:61-63 — the official `type is NamedType` gate; `void` is a
    // `NamedType` upstream, so `typedef _V = void;` skips too); the
    // old form (`=` absent) always counts.
    if c.unit
        .ast
        .token_range(node)
        .any(|token| c.lexeme(token) == "=")
    {
        let aliased = c.collect_direct(node).into_iter().find(|&kid| {
            matches!(
                c.kind(kid),
                NodeKind::Type
                    | NodeKind::FunctionType
                    | NodeKind::RecordType
                    | NodeKind::VoidType
            )
        });
        if c.kind(aliased.unwrap_or(node)) != NodeKind::FunctionType {
            return;
        }
        if c.collect_direct(node)
            .into_iter()
            .any(|kid| c.kind(kid) == NodeKind::TypeVariables)
        {
            return;
        }
    }
    // The name row: the first direct `Identifier` whose head token is
    // not shared with a direct type-row sibling. The old form tiles
    // `Identifier(ret-type)` + `Type(ret-type)` at the same token before
    // the name; the new form's name identifier stands alone before `=`.
    let type_heads: Vec<_> = c
        .collect_direct(node)
        .into_iter()
        .filter(|&kid| {
            matches!(
                c.kind(kid),
                NodeKind::Type | NodeKind::FunctionType | NodeKind::VoidType
            )
        })
        .map(|kid| c.head_token(kid))
        .collect();
    let Some(name) = c.collect_direct(node).into_iter().find(|&kid| {
        c.kind(kid) == NodeKind::Identifier && !type_heads.contains(&c.head_token(kid))
    }) else {
        return;
    };
    let name_lexeme = c.lexeme(c.head_token(name));
    if !name_lexeme.starts_with('_') {
        return;
    }
    // The use count: the mounted library face — every `Type` row across
    // the library's units headed by the name (the `_CountVisitor`
    // `visitNamedType` face over `context.allUnits`, folded by the
    // driver through `type_use_counts`). A name with no `Type` row
    // anywhere answers 0 uses — a typedef used nowhere reports, exactly
    // as upstream's zero count does. Unmounted — a run without the
    // driver face — stays silent: an absent face is not a zero count.
    let Some(uses) = c.library_type_uses() else {
        return;
    };
    if uses.get(name_lexeme).copied().unwrap_or(0) <= 1 {
        c.report(crate::rules::utf16_token_span(c, c.head_token(name)), &[]);
    }
}
