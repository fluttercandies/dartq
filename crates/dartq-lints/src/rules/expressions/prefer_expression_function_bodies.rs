use super::{Check, NodeKind, RuleSpec};

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/prefer_expression_function_bodies.dart`
/// — a block function body whose statements are exactly one
/// `return` with an expression reports at the body (:29-37 — the
/// `expression == null` bail covers the plain `return;`, issue 4221;
/// several statements and non-return bodies pass).
///
/// dartq faces over the production tiling: the dispatch row is the
/// [`NodeKind::BlockBody`] function-body container (a nested block
/// statement is a [`NodeKind::Block`] row and never dispatches), the
/// statements are its direct children, and the expression face is the
/// return's own completion window (`return;` builds no rows). Every
/// function-like owner reports — the upstream registers the body kind
/// with no owner gate, lambdas included. Message: `diagnostic.g.dart`
/// `preferExpressionFunctionBodies`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_expression_function_bodies",
    &[NodeKind::BlockBody],
    check,
);

fn check(c: &mut Check<'_>) {
    let body = c.node();
    let statements = c.collect_direct(body);
    let [only] = statements[..] else {
        return;
    };
    if c.kind(only) != NodeKind::ReturnStatement {
        return;
    }
    // `uniqueStatement.expression == null` bail (:35): a plain
    // `return;` builds no expression rows.
    let window = c.children(only);
    if window.start >= window.end {
        return;
    }
    let range = c.utf16_span(body);
    c.report(range, &[]);
}
