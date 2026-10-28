use super::{Check, NodeKind, RuleSpec, same_file, slice};
use crate::rules::{type_before, type_face, utf16_token_span};

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/unnecessary_await_in_return.dart`
/// — `return await e;` or an arrow `=> await e;` whose awaited type is
/// `Future` and whose enclosing function's return type is `Future`
/// reports at the `await` keyword (:44-88). The enclosing-function walk
/// stops at the first function-like declaration; a return inside a
/// `try` block never reports (:77-79 — removing the `await` there
/// changes behaviour).
///
/// dartq faces over the production tiling: the return-position face is
/// the await's parent — a `return` statement, or an arrow-body
/// declaration — read through parenthesized wrappers (the upstream
/// `unParenthesized`). The awaited-type and return-type faces are the
/// same-file written-type channel: the awaited operand must be a bare
/// name bound to a written `Future`, and the nearest enclosing
/// declaration must carry a written `Future` return type — an
/// `async`-marked anonymous function stands in for its inferred
/// `Future`. Approximations (documented, under-report): invocation and
/// member-access operands, inferred or omitted return types, and
/// generic-type-argument mismatches stay silent.
/// Message: `diagnostic.g.dart` `unnecessaryAwaitInReturn`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unnecessary_await_in_return",
    &[NodeKind::AwaitExpression],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // The ancestors nearest-first; the parenthesized wrappers are read
    // through (the upstream `unParenthesized`).
    let ancestors: Vec<u32> = c.path.iter().copied().rev().skip(1).collect();
    let mut index = 0;
    let Some(mut container) = ancestors.first().copied() else {
        return;
    };
    while c.kind(container) == NodeKind::ParenthesizedExpression {
        index += 1;
        match ancestors.get(index) {
            Some(&row) => container = row,
            None => return,
        }
    }
    // The return-position face (:44-54).
    match c.kind(container) {
        NodeKind::ReturnStatement => {}
        NodeKind::Method
        | NodeKind::TopLevelMethod
        | NodeKind::FunctionExpression
        | NodeKind::LocalFunctionDeclaration
            if has_arrow_body(c, container) => {}
        _ => return,
    }
    // The awaited-type face (:59-62): a bare name bound to a written
    // `Future`. The operand tiles as a send group
    // (`Identifier, NoTypeArguments, NoArguments, Send`) — strip it.
    let window = c.children(node);
    if window.start >= window.end {
        return;
    }
    let run: Vec<u32> = window.collect();
    let [only] = slice::access_run(c, &run)[..] else {
        return;
    };
    if c.kind(only) != NodeKind::Identifier {
        return;
    }
    let name = c.lexeme(c.head_token(only));
    let Some((_, Some(ty))) = same_file::binding(c, node, name) else {
        return;
    };
    if type_face(c, ty).name != Some("Future") {
        return;
    }
    // The return-type walk (:64-87): the nearest function-like
    // declaration outward; the `try`-block bail (:77-79).
    for &row in &ancestors[index..] {
        match c.kind(row) {
            NodeKind::Block
                if c.unit
                    .ast
                    .parent(row)
                    .is_some_and(|parent| c.kind(parent) == NodeKind::TryStatement) =>
            {
                return;
            }
            NodeKind::Method | NodeKind::TopLevelMethod => {
                if return_type_is_future(c, row) {
                    report(c, node);
                }
                return;
            }
            NodeKind::FunctionExpression => {
                if is_async_marked(c, row) {
                    report(c, node);
                }
                return;
            }
            NodeKind::LocalFunctionDeclaration => {
                if return_type_is_future(c, row) || is_async_marked(c, row) {
                    report(c, node);
                }
                return;
            }
            NodeKind::Constructor | NodeKind::Factory | NodeKind::CompilationUnit => return,
            _ => {}
        }
    }
}

fn report(c: &mut Check<'_>, node: u32) {
    // The report sits on the `await` keyword (:87).
    let range = utf16_token_span(c, c.head_token(node));
    c.report(range, &[]);
}

/// The childless `ExpressionBody` handle leaves the arrow expression as
/// the declaration row's own children.
fn has_arrow_body(c: &Check<'_>, declaration: u32) -> bool {
    c.collect_direct(declaration)
        .into_iter()
        .any(|kid| c.kind(kid) == NodeKind::ExpressionBody)
}

/// The written return type of a `Method`/`TopLevelMethod`/
/// `LocalFunctionDeclaration` row is `Future` (the
/// `isDartAsyncFuture` face — nullability and type arguments do not
/// change the written-name verdict). A local function's name nests
/// under `FunctionName` — `member_declaration_name` covers all three.
fn return_type_is_future(c: &Check<'_>, declaration: u32) -> bool {
    let Some(name) = crate::rules::declarations::member_declaration_name(c, declaration) else {
        return false;
    };
    let Some((type_row, _)) = type_before(c, declaration, name) else {
        return false;
    };
    type_face(c, type_row).name == Some("Future")
}

/// Whether the declaration's token range carries the `async` marker
/// (the inferred-`Future` return type of an `async` anonymous face).
fn is_async_marked(c: &Check<'_>, declaration: u32) -> bool {
    c.unit
        .ast
        .token_range(declaration)
        .any(|token| c.lexeme(token) == "async")
}

#[cfg(test)]
mod tests {
    use crate::testsupport::{Fixture, one_rule};

    fn run(source: &str) -> Vec<crate::engine::Lint> {
        Fixture::parse(source).run(&one_rule("unnecessary_await_in_return"))
    }

    /// The written-`Future` faces: a `return await` over a written
    /// `Future` parameter in a `Future`-returning function reports at
    /// the `await`; the arrow-body face reports the same way; a
    /// non-`Future` awaited type, a non-`Future` return type, and a
    /// non-return position stay silent.
    #[test]
    fn future_return_faces() {
        let lints = run("Future<int> g(Future<int> x) async {\n  return await x;\n}\n");
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "Unnecessary 'await'.");

        assert_eq!(
            run("Future<int> g(Future<int> x) async => await x;\n").len(),
            1
        );

        for source in [
            // The awaited name is not a written `Future`.
            "Future<int> g(int x) async {\n  return await x;\n}\n",
            // The return type is not a written `Future`.
            "int g(Future<int> x) async {\n  return await x;\n}\n",
            // Not the returned expression.
            "Future<int> g(Future<int> x) async {\n  var y = await x;\n  return y;\n}\n",
        ] {
            assert!(run(source).is_empty(), "{source}");
        }
    }
}
