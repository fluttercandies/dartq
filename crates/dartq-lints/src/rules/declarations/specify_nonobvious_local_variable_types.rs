use super::{
    Check, NodeKind, RuleSpec, for_each_iterable, is_identifier_token, local_written_type,
    obvious_spelling,
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
/// The invocation face (`pkg/linter/lib/src/util/obvious_types.dart`
/// :229-233): a method invocation is never obvious except `toString`, so
/// a bare zero-argument `bar()` initializer or iterable whose head names
/// no same-file class joins the non-obvious side.
///
/// Approximation (documented): the obvious-type matcher is the token
/// face of [`obvious_spelling`] — a bare-identifier initializer is the
/// obvious face, so the promotion-equality flip (`test_local_promotion`)
/// under-reports, and a bare-identifier element's unnameable type makes
/// any `[j, …]` fold non-obvious where the oracle's element-model types
/// can still agree (`[j, 1]` with `int j` is a `List<int>` upstream);
/// the pattern, switch and cast-declaration faces (:89-110) are out of
/// the substrate. Message: `diagnostic.g.dart`
/// `specifyNonobviousLocalVariableTypes`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "specify_nonobvious_local_variable_types",
    &[NodeKind::VariablesDeclaration],
    check,
);

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
            .is_some_and(|value| invocation_obvious(c, value));
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
/// declaration (:84) — the report opens at the loop variable's first
/// written token (the `var`/`final` keyword, else the written `dynamic`
/// run) and runs through the name, the `DeclaredIdentifier` extent the
/// markup anchors (`[!var i!]`).
fn face_for_each(c: &mut Check<'_>, node: u32) {
    if local_written_type(c, node).is_some_and(|(start, _)| c.lexeme(start) != "dynamic") {
        return;
    }
    let Some(iterable) = for_each_iterable(c, node) else {
        return;
    };
    if invocation_obvious(c, iterable) {
        return;
    }
    let start = match crate::rules::variables_declaration_keyword(c, node) {
        Some(keyword) => keyword,
        // The written-`dynamic` face: `dynamic` lexes as a keyword (not an
        // identifier), so the shared written-run walk answers `None` for
        // `dynamic i` — the type token sits right before the name, the
        // row's last token.
        None => {
            let name = c.unit.ast.token_range(node).end - 1;
            if c.lexeme(name - 1) == "dynamic" {
                name - 1
            } else {
                return;
            }
        }
    };
    let end = c.unit.ast.token_range(node).end;
    if let Some(range) = super::utf16_token_run_span(c, start..end) {
        c.report(range, &[]);
    }
}

/// The invocation face of `hasObviousType`
/// (`pkg/linter/lib/src/util/obvious_types.dart` :229-233): a method
/// invocation is obvious only as `toString` — every other call shape is
/// non-obvious. The substrate's childless `Send` leaf carries both the
/// method invocations and the plain `A()` instance creations the official
/// parser separates only through the `<…>` implicit-creation lookahead,
/// and the shared creation matcher accepts a bare `name ( )` without any
/// class check — so that one shape re-reads the head: a same-file class
/// is the instance-creation face, where a raw zero-argument send over a
/// generic class stays non-obvious (the upstream raw-generic bail,
/// :184-189) and a non-generic class is obvious; `toString` keeps the
/// upstream exception; every other head (a function, a cross-file or
/// unresolved name) is the invocation face and answers `false`. Every
/// other shape keeps the matcher's answer.
fn invocation_obvious(c: &Check<'_>, value: u32) -> bool {
    if c.kind(value) != NodeKind::Send {
        return obvious_spelling(c, value).is_some();
    }
    let tokens = c.unit.ast.token_range(value);
    let bare_call = tokens.len() == 3
        && is_identifier_token(c, tokens.start)
        && c.lexeme(tokens.start + 1) == "("
        && c.lexeme(tokens.start + 2) == ")";
    if !bare_call {
        return obvious_spelling(c, value).is_some();
    }
    let head = c.lexeme(tokens.start);
    if head == "toString" {
        return true;
    }
    crate::rules::same_file_declaration(c, head)
        .map(|row| c.kind(row) == NodeKind::ClassDeclaration && !class_generic(c, row))
        .unwrap_or(false)
}

/// Whether the same-file class row declares type variables. The
/// `TypeVariables` row tiles under the declaration's `TopLevelDeclaration`
/// wrapper (a sibling of the class-like row), not inside the class-like
/// itself — the wrapper's children read first, the class row's own
/// (hand-built tables) second.
fn class_generic(c: &Check<'_>, row: u32) -> bool {
    let wrapper = c.unit.ast.parent(row).unwrap_or(row);
    c.collect_direct(wrapper)
        .into_iter()
        .chain(c.collect_direct(row))
        .any(|kid| c.kind(kid) == NodeKind::TypeVariables)
}
