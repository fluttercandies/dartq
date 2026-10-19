use super::{
    Check, Group, NodeKind, RuleSpec, local_written_type, name_written_in_window,
    utf16_token_run_span,
};
use crate::rules::{
    initialized_identifier_name, last_child, utf16_token_span, variables_declaration_keyword,
};

/// Official: `pkg/linter/lib/src/rules/prefer_final_locals.dart:166-189`
/// — a non-`const`/non-`final` local variable list (:167) inside a
/// function body (:169) where every variable carries `=` and an
/// initializer (:173-175) and none is a wildcard or mutated in scope
/// (:178-179) reports at the keyword, else the written type
/// (:183-188). The pattern faces (:47-163) are out of the substrate.
///
/// Approximation (documented): the mutation face is the enclosing
/// function's token scan after the declaration — an assignment,
/// compound assignment or increment/decrement of any variable name
/// bails (a shadowing redeclaration inside the window under-reports);
/// plain reads — including inside closures — pass, matching the
/// resolved `isPotentiallyMutatedInScope`. Message: `diagnostic.g.dart`
/// `preferFinalLocals`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "prefer_final_locals",
    group: Group::Standalone,
    problem: "Local variables should be final.",
    correction: "Try making the variable final.",
    kinds: &[NodeKind::VariablesDeclaration],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `thisOrAncestorOfType<FunctionBody>` (:169) — the local faces
    // (statement, block body, C-style for initializer).
    let Some(parent) = c.ancestor(1) else {
        return;
    };
    if !matches!(
        c.kind(parent),
        NodeKind::Block | NodeKind::BlockBody | NodeKind::ForStatement
    ) {
        return;
    }
    // `node.isConst || node.isFinal` (:167).
    if variables_declaration_keyword(c, node)
        .is_some_and(|keyword| matches!(c.lexeme(keyword), "final" | "const"))
    {
        return;
    }
    let Some(&owner) = c.path.iter().rev().find(|&&row| {
        matches!(
            c.kind(row),
            NodeKind::TopLevelMethod
                | NodeKind::Method
                | NodeKind::Constructor
                | NodeKind::Factory
                | NodeKind::LocalFunctionDeclaration
                | NodeKind::FunctionExpression
        )
    }) else {
        return;
    };
    let mut names = Vec::new();
    for var in c.collect_direct(node) {
        if c.kind(var) != NodeKind::InitializedIdentifier {
            continue;
        }
        // `variable.equals == null || variable.initializer == null`
        // (:173-175).
        let initializer = c
            .collect_direct(var)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer);
        if initializer.is_none_or(|init| last_child(c, init).is_none()) {
            return;
        }
        // `declaredElement.isWildcardVariable` (:178).
        let Some(name_token) = initialized_identifier_name(c, var) else {
            return;
        };
        let name = c.lexeme(name_token);
        if name == "_" {
            return;
        }
        names.push(name);
    }
    if names.is_empty() {
        return;
    }
    // `function.isPotentiallyMutatedInScope` (:179).
    let window = c.unit.ast.token_range(owner);
    let decl = c.unit.ast.token_range(node);
    if names
        .iter()
        .any(|name| name_written_in_window(c, decl.end..window.end, name))
    {
        return;
    }
    // `rule.reportAtToken(keyword)`, else the type (:183-188).
    if let Some(keyword) = variables_declaration_keyword(c, node) {
        c.report(utf16_token_span(c, keyword), &[]);
    } else {
        let Some((start, end)) = local_written_type(c, node) else {
            return;
        };
        let Some(range) = utf16_token_run_span(c, start..end) else {
            return;
        };
        c.report(range, &[]);
    }
}
