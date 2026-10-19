use super::{Check, Group, NodeKind, RuleSpec};

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/use_named_constants.dart` — a const-written
/// instance creation whose value equals a `static const` field of its
/// class reports at the creation with the constant's name (:48/:41
/// `node.isConst` gate, :53-84 the field walk, `reportAtNode` :80).
///
/// dartq faces: the dispatch rows are the written-`const` wrappers
/// ([`NodeKind::ConstExpression`] / [`NodeKind::ConstLiteral`]) and the
/// contextually-const creations — a bare `Send`/`ImplicitCreation`/
/// `ConstructorReference`/`NewExpression` inside a `const` context
/// ([`crate::rules::in_const_context`] answers upstream's `isConst`,
/// which covers the `const a = A(0)` declarator-const face), plus the
/// dot-shorthand rows ([`NodeKind::ConstDotShorthand`] for the
/// written-`const .new(0)` and [`NodeKind::DotShorthandHead`] under a
/// const context for `const A a = .new(0)`). The whole structural face
/// is the shared named-constant proxy
/// ([`crate::rules::const_ability::matches_named_constant`] /
/// [`matches_shorthand_constant`]): the creation compared, token for
/// token, against the same-file class's `static const` initializers in
/// declaration order — loosened to the callee name when the class
/// stores no instance state (upstream's computed-value equality makes
/// `A(1)` match `_zero = A(0)` under a discarded-parameter ctor) — and
/// the own-class static-initializer guard included. Approximations
/// (documented, under-report): the shorthand's context type resolves
/// through the enclosing declared type only (a return/parameter
/// context type under-reports), a constant-pattern creation reports
/// only when it carries the written `const`, and cross-file classes
/// are outside the proxy's same-file resolution. Message:
/// `diagnostic.g.dart` `useNamedConstants` (the `{0}` slot carries
/// `Type.field`).
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "use_named_constants",
    group: Group::Standalone,
    problem: "Use the constant '{0}' rather than a constructor returning the same object.",
    correction: "Try using '{0}'.",
    kinds: &[
        NodeKind::ConstExpression,
        NodeKind::ConstLiteral,
        NodeKind::ConstantPattern,
        NodeKind::Send,
        NodeKind::ImplicitCreation,
        NodeKind::ConstructorReference,
        NodeKind::NewExpression,
    ],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // A written-const constant pattern (`case const A(0)`, `if (x case
    // const A(0))`): upstream `visitInstanceCreationExpression` fires
    // on the operand and reports at the creation, not the pattern —
    // the pattern row only owns `const` + `Arguments` (the callee send
    // tiles as a flat sibling), so the extent is reconstructed and the
    // report lands on it rather than on `node`.
    if c.kind(node) == NodeKind::ConstantPattern {
        if c.lexeme(c.head_token(node)) != "const" {
            return;
        }
        let Some((type_name, extent)) =
            crate::rules::const_ability::pattern_creation(c.unit, node)
        else {
            return;
        };
        let Some((type_name, field)) = crate::rules::const_ability::matches_bare_constant(
            c.unit,
            node,
            type_name,
            extent.clone(),
        ) else {
            return;
        };
        let (Some(start), Some(end)) = (
            c.utf16_of_token(extent.start),
            extent
                .end
                .checked_sub(1)
                .and_then(|token| c.utf16_end_of_token(token)),
        ) else {
            return;
        };
        let constant = format!("{type_name}.{field}");
        c.report(start..end, &[constant.as_str()]);
        return;
    }
    let found = match c.kind(node) {
        NodeKind::ConstExpression | NodeKind::ConstLiteral => {
            crate::rules::const_ability::matches_named_constant(c.unit, node)
        }
        // The dot-shorthand invocation (`A a = const .new(0)` /
        // `const A a = .new(0)`): the `Send` carries the `new (…)`
        // invocation; the `DotShorthandHead` leaf for the `.` and the
        // `ConstDotShorthand` marker for a written `const` tile as its
        // owner-run siblings before it. Const-ness is a `const` marker
        // sibling (written) or `in_const_context` (the `const A a =`
        // declarator face); the `.new` callee's class resolves through
        // the enclosing declared type.
        NodeKind::Send if shorthand_send(c, node) => {
            let const_written = shorthand_const_marker(c, node);
            if const_written || crate::rules::in_const_context(c) {
                shorthand_constant(c, node)
            } else {
                None
            }
        }
        // A bare creation inside a const context (`const a = A(0)`):
        // the written-const gate is `in_const_context`, then the same
        // written-shape proxy judges the creation against the class's
        // static consts. The `creation` face is the row itself.
        NodeKind::Send
        | NodeKind::ImplicitCreation
        | NodeKind::ConstructorReference
        | NodeKind::NewExpression => {
            // A creation already bracketed by its own `const` wrapper
            // (`const A(0)` under `case const …:`) dispatches on the
            // wrapper row — reporting the bare row too would double
            // the row.
            if c.path[..c.path.len() - 1].iter().any(|&ancestor| {
                matches!(
                    c.kind(ancestor),
                    NodeKind::ConstExpression
                        | NodeKind::ConstLiteral
                        | NodeKind::ConstDotShorthand
                )
            }) || !crate::rules::in_const_context(c)
            {
                return;
            }
            let Some((type_name, extent)) =
                crate::rules::const_ability::bare_creation_extent(c.unit, node)
            else {
                return;
            };
            crate::rules::const_ability::matches_bare_constant(c.unit, node, type_name, extent)
        }
        _ => return,
    };
    let Some((type_name, field)) = found else {
        return;
    };
    let constant = format!("{type_name}.{field}");
    c.report(c.utf16_span(node), &[constant.as_str()]);
}

/// The const dot-shorthand's named-constant match: the `.new` callee's
/// class is the enclosing context's declared type (the `const A a =
/// .new(0)` declarator face). `node` is the shorthand's `Send` row
/// (the `new (…)` invocation).
fn shorthand_constant<'a>(c: &'a Check<'_>, node: u32) -> Option<(&'a str, &'a str)> {
    let type_name = shorthand_context_type(c)?;
    crate::rules::const_ability::matches_shorthand_constant(c.unit, node, type_name)
}

/// Whether the `Send` is a dot-shorthand invocation — the
/// `DotShorthandHead` leaf for the `.` tiles immediately before its
/// span in the enclosing owner run (`.new(0)` lands as head-leaf +
/// `new (…)` send).
fn shorthand_send(c: &Check<'_>, node: u32) -> bool {
    let start = c.head_token(node);
    let Some(parent) = c.ancestor(1) else {
        return false;
    };
    c.children(parent).any(|kid| {
        c.kind(kid) == NodeKind::DotShorthandHead && c.unit.ast.token_range(kid).end == start
    })
}

/// Whether the shorthand's `Send` carries a written `const` — its
/// parent is the `ConstDotShorthand` wrapper (`const .new(0)`; the
/// wrapper's own span is the `const` keyword leaf).
fn shorthand_const_marker(c: &Check<'_>, _node: u32) -> bool {
    c.ancestor(1)
        .is_some_and(|parent| c.kind(parent) == NodeKind::ConstDotShorthand)
}

/// The declared type the shorthand's `.new` resolves against — the
/// enclosing `VariablesDeclaration`'s written type (`const A a =
/// .new(0)`). `None` when no declared type names the context (the
/// return/parameter type faces under-report).
fn shorthand_context_type<'a>(c: &'a Check<'_>) -> Option<&'a str> {
    let decl = c
        .path
        .iter()
        .rev()
        .find(|&ancestor| c.kind(*ancestor) == NodeKind::VariablesDeclaration)?;
    written_decl_type(c, *decl)
}

/// The written type a `VariablesDeclaration`/`Fields` run spells.
/// Production tiling: for `A a = …` the `Type` row tiles as the
/// declaration's left sibling (the declaration itself heads at the
/// name `a`); the run is the parent's window scanned backward from
/// `decl` for the `Type` row.
fn written_decl_type<'a>(c: &'a Check<'_>, decl: u32) -> Option<&'a str> {
    // Inside the declaration first (a member `const A a` keeps the
    // type in the same row's window).
    if let Some(ty) = c
        .collect_direct(decl)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Type)
    {
        return Some(c.lexeme(c.head_token(ty)));
    }
    // The left-sibling face: the type precedes the declaration row.
    let parent = c.unit.ast.parent(decl)?;
    let siblings = c.collect_direct(parent);
    let pos = siblings.iter().position(|&kid| kid == decl)?;
    siblings[..pos]
        .iter()
        .rev()
        .find(|&&kid| c.kind(kid) == NodeKind::Type)
        .map(|&ty| c.lexeme(c.head_token(ty)))
}
