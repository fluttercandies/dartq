use super::{Check, Group, NONE_TOKEN, NodeKind, RuleSpec};

/// Official: `pkg/linter/lib/src/rules/prefer_generic_function_type_aliases.dart:44-64`
/// — every old-form `typedef` (no `=`; the analyzer's `FunctionTypeAlias`,
/// vs the `GenericTypeAlias` that carries `=`) reports at its name with the
/// generic-function-type rewrite spelled in the message. The replacement
/// is assembled from source slices in the official order
/// (`returnType + ' ' + 'Function' + typeParameters + parameters`); the
/// slices carry the written tail verbatim — a comment inside the tail
/// (between tokens of a sliced part) rides along in the message, where
/// the official `toSource()` re-render drops it (the golden pin holds
/// this written-tail face; the report position is unaffected). A
/// synthetic semicolon (recovery) skips (:48-49). Message:
/// `diagnostic.g.dart` `preferGenericFunctionTypeAliases`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "prefer_generic_function_type_aliases",
    group: Group::Core,
    problem: "Use the generic function type syntax in 'typedef's.",
    correction: "Try using the generic function type syntax ('{0}').",
    kinds: &[NodeKind::Typedef],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // Old form ⟺ no `=` token (the `Typedef` payload word 1).
    if c.payload(node).get(1).copied() != Some(NONE_TOKEN) {
        return;
    }
    // `node.semicolon.isSynthetic` bails (:48-49).
    if c.token_is_synthetic(crate::rules::tail_token(c, node)) {
        return;
    }
    let kids = c.collect_direct(node);
    let mut return_type = None;
    let mut name = None;
    let mut type_variables = None;
    let mut parameters = None;
    for kid in kids {
        match c.kind(kid) {
            NodeKind::Type | NodeKind::NoType if return_type.is_none() => {
                return_type = Some(kid);
            }
            NodeKind::Identifier if name.is_none() => name = Some(kid),
            NodeKind::TypeVariables => type_variables = Some(kid),
            NodeKind::FormalParameters => parameters = Some(kid),
            _ => {}
        }
    }
    let Some(name) = name else {
        return;
    };
    let Some(parameters) = parameters else {
        return;
    };
    let slice = |id: u32| -> &str {
        let bytes = c.byte_span(id);
        &c.unit.source[bytes]
    };
    // `'${returnTypeSource}Function$typeParameterSource$parameterSource'`
    // (:53-62) — `returnType == null` (our `NoType`) contributes nothing.
    let mut replacement = String::new();
    if let Some(rt) = return_type.filter(|rt| c.kind(*rt) != NodeKind::NoType) {
        replacement.push_str(slice(rt));
        replacement.push(' ');
    }
    replacement.push_str("Function");
    if let Some(tv) = type_variables {
        replacement.push_str(slice(tv));
    }
    replacement.push_str(slice(parameters));
    let Some(start) = c.utf16_of_token(c.head_token(name)) else {
        return;
    };
    c.report(start..start, &[&replacement]);
}
