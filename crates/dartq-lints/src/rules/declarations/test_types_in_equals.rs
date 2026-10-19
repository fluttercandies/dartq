use super::{Check, Group, NodeKind, RuleSpec, formal_parameter_rows, operator_name};
use crate::rules::class_declaration_name;

/// Official: `pkg/linter/lib/src/rules/test_types_in_equals.dart:35-48`
/// — inside an `operator ==` method with exactly one parameter, an
/// `as` cast of that parameter (`other as C`) reports at the cast
/// with the owner's name (:50-64: the class/enum/mixin name, an
/// extension's `on` type source, or `unknown`). The enclosing face is
/// `MethodDeclaration` — top-level functions never qualify. Message:
/// `diagnostic.g.dart` `testTypesInEquals`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "test_types_in_equals",
    group: Group::Standalone,
    problem: "Missing type test for '{0}' in '=='.",
    correction: "Try testing the type of '{0}'.",
    kinds: &[NodeKind::AsExpression],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `node.thisOrAncestorOfType<MethodDeclaration>()` (:36) — the
    // dartq `Method` row; top-level methods are out of face.
    let Some(&method) = c.path[..c.path.len() - 1]
        .iter()
        .rev()
        .find(|&&a| c.kind(a) == NodeKind::Method)
    else {
        return;
    };
    // `!_isEqualsOverride(declaration)` (:66-70).
    if !is_equals_override(c, method) {
        return;
    }
    // `expression is! SimpleIdentifier` (:38) — the cast operand's
    // root row. The production `as` tiles its operand pieces as the
    // row's left-sibling run (the `other` send group leaks above the
    // adopted window), so the operand reads through the operator split.
    let Some((left, _)) = crate::rules::slice::binary_operands_at(c, node) else {
        return;
    };
    let operand = match crate::rules::slice::access_run(c, &left).as_slice() {
        [row] if c.kind(*row) == NodeKind::Identifier => *row,
        _ => return,
    };
    // `expression.name == parameterName` (:44) — the single
    // parameter's written name.
    let Some(params_row) = c
        .collect_direct(method)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
    else {
        return;
    };
    let params = formal_parameter_rows(c, params_row);
    let [param] = params.as_slice() else {
        return;
    };
    let Some(&param_name) = c.payload(*param).get(1) else {
        return;
    };
    if param_name == crate::rules::NONE_TOKEN
        || c.lexeme(c.head_token(operand)) != c.lexeme(param_name)
    {
        return;
    }
    let type_name = owner_type_name(c, method);
    // The official `reportAtNode` covers the whole `as` expression —
    // the operand head through the row end (the row's own span drops
    // the leaked left operand).
    let start = c.utf16_span(operand).start;
    c.report(start..c.utf16_span(node).end, &[type_name.as_str()]);
}

/// `declaration.isOperator && name.lexeme == '==' &&
/// parameters.length == 1` (:66-70) — `==` is a `Method` whose
/// `OperatorName` payload token lexemes to `==`.
fn is_equals_override(c: &Check<'_>, method: u32) -> bool {
    // The `OperatorName` completes as the Method's left sibling inside
    // the body wrapper (`operator_name` reads child-then-adjacent).
    let is_eq = operator_name(c, method)
        .and_then(|row| c.payload_word(row, 0))
        .is_some_and(|t| c.lexeme(t) == "==");
    if !is_eq {
        return false;
    }
    let Some(params_row) = c
        .collect_direct(method)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
    else {
        return false;
    };
    formal_parameter_rows(c, params_row).len() == 1
}

/// `_getTypeName` (:50-64): the owner container's written name; an
/// extension answers its `on` type source, anything else `unknown`.
fn owner_type_name(c: &Check<'_>, method: u32) -> String {
    let mut current = method;
    while let Some(parent) = c.unit.ast.parent(current) {
        match c.kind(parent) {
            NodeKind::ClassDeclaration
            | NodeKind::EnumDeclaration
            | NodeKind::MixinDeclaration => {
                return class_declaration_name(c, parent)
                    .unwrap_or("unknown")
                    .to_owned();
            }
            NodeKind::ExtensionDeclaration => {
                let extended = c.collect_direct(parent).into_iter().find(|&kid| {
                    matches!(
                        c.kind(kid),
                        NodeKind::Type | NodeKind::FunctionType | NodeKind::RecordType
                    )
                });
                if let Some(t) = extended {
                    return c.unit.source[c.byte_span(t)].to_owned();
                }
                return "unknown".to_owned();
            }
            _ => {}
        }
        current = parent;
    }
    "unknown".to_owned()
}
