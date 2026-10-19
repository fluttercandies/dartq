use super::{Check, Group, NodeKind, RuleSpec, async_words, within_try_block};
use crate::rules::{NONE_TOKEN, last_child};

/// Official: `pkg/linter/lib/src/rules/async_return_with_no_await.dart`
/// — a `return` (or arrow body) inside an `async` non-generator
/// function whose expression is future-typed reports at the `return`
/// keyword (the `=>` for arrow bodies, :37-44). The resolved face —
/// the expression's `Future`/`FutureOr` static type assignable to
/// `FutureOr<futureValueType(returnType)>`,
/// `async_return_visitor.dart:46-69` — approximates syntactically:
/// the expression face is its head spelling (`Future`/`FutureOr`
/// productions — a bare future-typed variable, a subtype, or a type
/// parameter under-reports), and the return-type gate reads the
/// written annotation (absent = the dynamic face; a non-future
/// annotation is already a compile error upstream). Returns lexically
/// inside a try body defer to the analyzer's try-block diagnostic and
/// never report (`isWithinTryBlock`, :40/:72-86 — catch and finally
/// bodies report). Arrow bodies process outside the try face
/// (:49-53). Closures have no written return type here and
/// under-report. Message: `diagnostic.g.dart`
/// `asyncReturnWithNoAwait`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "async_return_with_no_await",
    group: Group::Standalone,
    problem: "Returning a 'Future' without 'await'.",
    correction: "Try adding an 'await' or making the body non-async.",
    kinds: &[NodeKind::ReturnStatement, NodeKind::ExpressionBody],
    check,
};

fn check(c: &mut Check<'_>) {
    match c.kind(c.node()) {
        NodeKind::ReturnStatement => check_return(c),
        NodeKind::ExpressionBody => check_arrow(c),
        _ => {}
    }
}

fn check_return(c: &mut Check<'_>) {
    let node = c.node();
    // `node.expression == null` (:39) — a bare `return;` has no child.
    let Some(expression) = last_child(c, node) else {
        return;
    };
    // `_withinTryBlock != node.isWithinTryBlock` (:40).
    if within_try_block(c, node) {
        return;
    }
    let Some(owner) = enclosing_function(c) else {
        return;
    };
    if !async_owner(c, owner) {
        return;
    }
    // The awaited value is not itself a future — `return await e;`
    // never reports (the static-type face answers the same).
    if c.kind(expression) == NodeKind::AwaitExpression {
        return;
    }
    // The future-production face: the expression's head spelling. In
    // the flat send-chain tiling the expression's outer tail (the
    // `last_child` row) anchors at its own segment, so the head token
    // is the first token after `return`, not the tail's start.
    if !future_head(c, c.head_token(node) + 1) {
        return;
    }
    if !future_return_type(c, owner) {
        return;
    }
    c.report(crate::rules::utf16_token_span(c, c.head_token(node)), &[]);
}

fn check_arrow(c: &mut Check<'_>) {
    let node = c.node();
    // `body.returnType` reads only declaration parents
    // (`async_return_visitor.dart:89-99`): a primary constructor body
    // or a nested container is out of face.
    let Some(owner) = c.ancestor(1) else {
        return;
    };
    if !matches!(
        c.kind(owner),
        NodeKind::Method
            | NodeKind::TopLevelMethod
            | NodeKind::FunctionExpression
            | NodeKind::LocalFunctionDeclaration
            | NodeKind::NamedFunctionExpression
    ) {
        return;
    }
    // `body.isGenerator` / `!body.isAsynchronous` (:56-59) — the
    // arrow body's own modifier words (`async_words` words 2-3).
    let (async_kw, star) = async_words(c, node);
    if async_kw == NONE_TOKEN || star != NONE_TOKEN {
        return;
    }
    // The expression is the preceding sibling (the body is a
    // childless handle); its outer tail row must begin inside the
    // post-arrow region.
    let Some(expression) = preceding_expression(c, node) else {
        return;
    };
    if c.kind(expression) == NodeKind::AwaitExpression {
        return;
    }
    // The head spelling is the first token after `=>` — the tail
    // row's own start sits deeper into the chain.
    if !future_head(c, c.head_token(node) + 1) {
        return;
    }
    if !future_return_type(c, owner) {
        return;
    }
    // `node.functionDefinition` — the `=>` (the span head).
    c.report(crate::rules::utf16_token_span(c, c.head_token(node)), &[]);
}

/// The nearest enclosing function-like row (the scoping owner of the
/// return — a nested closure's own body answers first).
fn enclosing_function(c: &Check<'_>) -> Option<u32> {
    c.path[..c.path.len() - 1].iter().rev().copied().find(|&a| {
        matches!(
            c.kind(a),
            NodeKind::Method
                | NodeKind::TopLevelMethod
                | NodeKind::FunctionExpression
                | NodeKind::LocalFunctionDeclaration
                | NodeKind::NamedFunctionExpression
        )
    })
}

/// `body.isAsynchronous && !body.isGenerator` (:54-59) — the owner's
/// body modifier words.
fn async_owner(c: &Check<'_>, owner: u32) -> bool {
    let Some(body) = last_child(c, owner) else {
        return false;
    };
    let (async_kw, star) = async_words(c, body);
    async_kw != NONE_TOKEN && star == NONE_TOKEN
}

/// The future-production face: the token starts a `Future`/`FutureOr`
/// spelling.
fn future_head(c: &Check<'_>, token: u32) -> bool {
    matches!(c.lexeme(token), "Future" | "FutureOr")
}

/// The written return-type gate: a `Future`/`FutureOr` annotation
/// admits, an absent annotation is the dynamic face, anything written
/// else (`void`, a non-future type) skips.
fn future_return_type(c: &Check<'_>, owner: u32) -> bool {
    match c.collect_direct(owner).into_iter().find(|&kid| {
        matches!(
            c.kind(kid),
            NodeKind::Type | NodeKind::FunctionType | NodeKind::RecordType
        )
    }) {
        None => true, // no written annotation — the dynamic face
        Some(t) => {
            let slice = &c.unit.source[c.byte_span(t)];
            slice == "Future"
                || slice == "FutureOr"
                || slice.starts_with("Future<")
                || slice.starts_with("FutureOr<")
        }
    }
}

/// The expression's outer tail row immediately before a childless
/// `ExpressionBody` — the flat send-chain tiles every segment as an
/// owner-window sibling, so the tail anchors inside the post-arrow
/// region rather than at the expression head (a body without a
/// recovered expression answers `None`: the row before it is then a
/// declaration sibling ending before the arrow).
fn preceding_expression(c: &Check<'_>, body: u32) -> Option<u32> {
    let parent = c.unit.ast.parent(body)?;
    let kids = c.collect_direct(parent);
    let pos = kids.iter().position(|&kid| kid == body)?;
    let expression = *kids.get(pos.checked_sub(1)?)?;
    (c.unit.ast.token_range(expression).start > c.head_token(body)).then_some(expression)
}
