use super::{
    Check, Group, NodeKind, RuleSpec, for_each_iterable, local_written_type, obvious_spelling,
};
use crate::rules::last_child;

/// Official: `pkg/linter/lib/src/rules/specify_nonobvious_local_variable_types.dart:68-145`
/// — a local variable list (:113-115, statement or C-style for
/// initializer via :70-73) with no written type (or exactly `Null`,
/// :117-119) where some variable lacks an initializer or initializes
/// from a non-obvious expression (:124-135) reports at the list for a
/// single variable (:137-138) or at each needing variable (:140-141).
/// The for-each face (:74-86): a loop variable without a written type
/// over a non-obvious iterable reports at the declaration (:84).
///
/// Approximation (documented): the obvious-type matcher is the token
/// face of [`obvious_spelling`] — a bare-identifier initializer is the
/// obvious face, so the promotion-equality flip (`test_local_promotion`)
/// under-reports; the pattern, switch and cast-declaration faces
/// (:89-110) are out of the substrate. Message: `diagnostic.g.dart`
/// `specifyNonobviousLocalVariableTypes`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "specify_nonobvious_local_variable_types",
    group: Group::Standalone,
    problem: "Specify the type of a local variable when the type is non-obvious.",
    correction: "Try adding a type annotation.",
    kinds: &[NodeKind::VariablesDeclaration],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    let Some(parent) = c.ancestor(1) else {
        return;
    };
    match c.kind(parent) {
        // `staticType != null && !staticType.isDartCoreNull` (:117-119):
        // a written type other than `Null` skips.
        NodeKind::Block | NodeKind::BlockBody | NodeKind::ForStatement => {
            if local_written_type(c, node).is_some_and(|(start, _)| c.lexeme(start) != "Null") {
                return;
            }
            specify_face(c, node);
        }
        NodeKind::ForIn => face_for_each(c, node),
        _ => {}
    }
}

/// `_visitVariableDeclarationList` (:117-144): the variables whose
/// initializer is missing or non-obvious need a type; one variable
/// reports the whole list, several report each.
fn specify_face(c: &mut Check<'_>, node: u32) {
    let mut needs = Vec::new();
    let mut declarators = 0;
    for var in c.collect_direct(node) {
        if c.kind(var) != NodeKind::InitializedIdentifier {
            continue;
        }
        declarators += 1;
        let initializer = c
            .collect_direct(var)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer);
        let obvious = initializer
            .and_then(|init| last_child(c, init))
            .is_some_and(|value| obvious_spelling(c, value).is_some());
        if !obvious {
            needs.push(var);
        }
    }
    if needs.is_empty() {
        return;
    }
    if declarators == 1 {
        // The list's written head is the keyword (`var`/`final`) — the
        // `VariablesDeclaration` row anchors at the name on a local
        // declaration, so the report points at the keyword token
        // (`reportAtNode(node)` opens the list).
        let keyword = crate::rules::variables_declaration_keyword(c, node).map(|token| {
            let start = c
                .unit
                .tokens
                .get(token as usize)
                .map(|t| t.utf16_offset)
                .unwrap_or(0);
            start..start
        });
        c.report(keyword.unwrap_or_else(|| c.utf16_span(node)), &[]);
    } else {
        for var in needs {
            c.report(c.utf16_span(var), &[]);
        }
    }
}

/// The for-each face (:74-86): no written type (or exactly `dynamic`,
/// the `DynamicType` face) and a non-obvious iterable report at the
/// declaration (:84).
fn face_for_each(c: &mut Check<'_>, node: u32) {
    if local_written_type(c, node).is_some_and(|(start, _)| c.lexeme(start) != "dynamic") {
        return;
    }
    let Some(iterable) = for_each_iterable(c, node) else {
        return;
    };
    if obvious_spelling(c, iterable).is_some() {
        return;
    }
    c.report(c.utf16_span(node), &[]);
}
