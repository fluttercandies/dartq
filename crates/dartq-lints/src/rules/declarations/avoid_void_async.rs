use super::{
    Check, Group, NodeKind, RuleSpec, async_words, member_declaration_name, name_row_token,
};
use crate::rules::{NONE_TOKEN, last_child, member_name};

/// Official: `pkg/linter/lib/src/rules/avoid_void_async.dart:39-59` —
/// an `async` (non-generator) function whose return type is `void`
/// reports at its name. The registered faces (:32-33) are top-level
/// and local functions and methods — closures and function
/// expressions never report; a top-level `main` is exempt (:40). The
/// resolved `returnType is VoidType` face folds to the written type:
/// an inferred return type never reports, exactly as the element face
/// (`dynamic` is not `VoidType`). Message: `diagnostic.g.dart`
/// `avoidVoidAsync`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "avoid_void_async",
    group: Group::Standalone,
    problem: "An 'async' function should have a 'Future' return type when it doesn't return a value.",
    correction: "Try changing the return type.",
    kinds: &[
        NodeKind::TopLevelMethod,
        NodeKind::Method,
        NodeKind::LocalFunctionDeclaration,
    ],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `node.name.lexeme == 'main' && node.parent is CompilationUnit`
    // (:40) — the top-level face only; a local `main` reports
    // (upstream `test_functionLocal_main_async_arrow`).
    if c.kind(node) == NodeKind::TopLevelMethod
        && member_name(c, node).is_some_and(|name| c.lexeme(c.head_token(name)) == "main")
    {
        return;
    }
    // `fragment.isGenerator` / `!fragment.isAsynchronous`
    // (:54-55) — the body's modifier words (see `async_words`).
    let Some(body) = last_child(c, node) else {
        return;
    };
    let (async_kw, star) = async_words(c, body);
    if async_kw == NONE_TOKEN || star != NONE_TOKEN {
        return;
    }
    // `fragment.element.returnType is VoidType` (:56) — the written
    // `void` (a `Future<void>` annotation nests its `void` under the
    // type row, out of the direct-child face).
    if !c
        .collect_direct(node)
        .into_iter()
        .any(|kid| c.kind(kid) == NodeKind::VoidType)
    {
        return;
    }
    // The name: methods and top-level functions carry the stamped
    // `Identifier` directly; a local function nests it under
    // `FunctionName` — [`member_declaration_name`] covers both.
    let Some(name) = member_declaration_name(c, node) else {
        return;
    };
    c.report(
        crate::rules::utf16_token_span(c, name_row_token(c, name)),
        &[],
    );
}
