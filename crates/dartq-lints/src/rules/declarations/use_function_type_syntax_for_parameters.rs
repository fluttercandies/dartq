use super::{Check, Group, NodeKind, RuleSpec};

/// Official: `pkg/linter/lib/src/rules/use_function_type_syntax_for_parameters.dart:34-49`
/// — a formal parameter written with the old function syntax
/// (`void f()`) reports at the parameter. The old syntax is its own AST
/// shape: [`NodeKind::FunctionTypedFormalParameter`]. Message: `diagnostic.g.dart`
/// `useFunctionTypeSyntaxForParameters`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "use_function_type_syntax_for_parameters",
    group: Group::Recommended,
    problem: "Use the generic function type syntax to declare the parameter '{0}'.",
    correction: "Try using the generic function type syntax.",
    kinds: &[NodeKind::FunctionTypedFormalParameter],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `node.name?.lexeme ?? ''` (:44). The production
    // `FunctionTypedFormalParameter` anchors its span at the name token
    // but may still tile the written return type's `Identifier` (a
    // `TypeReference` leaf) as a child — so the name read is the head
    // token, not a first-`Identifier` scan. A hand-built table anchors
    // at the return type and carries the name `Identifier` child — it
    // wins there.
    let name = c
        .collect_direct(node)
        .into_iter()
        .find(|&kid| {
            c.kind(kid) == NodeKind::Identifier
                && c.payload(kid).first().copied()
                    != Some(dartq_parser::IdentifierContext::TypeReference as u32)
        })
        .map_or_else(|| c.head_token(node), |kid| c.head_token(kid));
    let name = c.lexeme(name);
    let range = c.utf16_span(node);
    c.report(range, &[name]);
}
