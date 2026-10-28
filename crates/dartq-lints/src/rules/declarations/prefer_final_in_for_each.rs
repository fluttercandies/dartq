use super::{Check, NodeKind, RuleSpec, closure_windows, name_captured, name_written_in_window};
use crate::rules::{initialized_identifier_name, utf16_token_span, variables_declaration_keyword};

/// Official: `pkg/linter/lib/src/rules/prefer_final_in_for_each.dart:44-60`
/// — a for-each loop variable (:45) not declared `final` (:46) and
/// never mutated in the enclosing function (:50-52) reports at the
/// name (:54-58, the name as the `{0}` argument). The pattern face
/// (:63-101) is out of the substrate.
///
/// Approximation (documented): the mutation face is the loop window's
/// token scan — an assignment, compound assignment or
/// increment/decrement of the name bails, and any occurrence inside a
/// nested function bails (a capture is judged unreadable; the resolved
/// mutation walk would answer only true writes). Message:
/// `diagnostic.g.dart` `preferFinalInForEachVariable`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_final_in_for_each",
    &[NodeKind::VariablesDeclaration],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `ForEachPartsWithDeclaration` (:37/:44) — the loop-variable row.
    let Some(parent) = c.ancestor(1) else {
        return;
    };
    if c.kind(parent) != NodeKind::ForIn {
        return;
    }
    // `loopVariable.isFinal` (:46).
    if variables_declaration_keyword(c, node)
        .is_some_and(|keyword| c.lexeme(keyword) == "final")
    {
        return;
    }
    let Some(var_row) = c
        .collect_direct(node)
        .into_iter()
        .find(|&row| c.kind(row) == NodeKind::InitializedIdentifier)
    else {
        return;
    };
    let Some(name_token) = initialized_identifier_name(c, var_row) else {
        return;
    };
    let name = c.lexeme(name_token);
    // `!function.isPotentiallyMutatedInScope(loopVariableElement)`
    // (:50-52) — the loop window's write scan plus the capture bail.
    let window = c.unit.ast.token_range(parent);
    let decl = c.unit.ast.token_range(node);
    let closures = closure_windows(c, parent);
    if name_written_in_window(c, decl.end..window.end, name)
        || name_captured(c, &closures, name)
    {
        return;
    }
    c.report(utf16_token_span(c, name_token), &[name]);
}
