use super::{Check, NodeKind, RuleSpec};

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/only_throw_errors.dart`
/// — a `throw` whose expression is a literal reports at the expression
/// (:49-60: `node.expression is Literal` — booleans, numbers, strings,
/// collections, `null`, symbols); every other operand defers to the
/// static-type face. The dartq face is the literal slice of that
/// dispatch, read through parenthesized wrappers (the type arm judges
/// the unparenthesized expression identically).
/// Approximation (documented, under-report): the typed arm
/// (`_isThrowable` :25-31) needs the element model — non-literal
/// operands (identifiers, invocations) stay silent, and a user
/// `Exception` subclass construction under-reports the same way.
/// Message: `diagnostic.g.dart` `onlyThrowErrors`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "only_throw_errors",
    &[NodeKind::ThrowExpression],
    check,
);

fn check(c: &mut Check<'_>) {
    // The production tiling: throw is a childless adopted container
    // (the row anchors at the `throw` token) — the operand tiles
    // beside it in the enclosing window. The operand rows are the
    // owner rows token-inside `(throw`, `;)`; the root is the last of
    // them (a run seals in completion order, root last). A
    // parenthesized operand keeps its own window, so the walk reads
    // through wrappers.
    let node = c.node();
    let anchor_end = c.unit.ast.token_range(node).end;
    let Some(parent) = c.unit.ast.parent(node) else {
        return;
    };
    let mut ceiling = anchor_end;
    while ceiling < c.unit.tokens.len() as u32 && c.lexeme(ceiling) != ";" {
        ceiling += 1;
    }
    let Some(mut root) = c
        .collect_direct(parent)
        .into_iter()
        .filter(|&row| {
            c.kind(row) != NodeKind::ThrowExpression
                && c.unit.ast.token_range(row).start >= anchor_end
                && c.unit.ast.token_range(row).end <= ceiling
        })
        .max()
    else {
        return;
    };
    while c.kind(root) == NodeKind::ParenthesizedExpression {
        let inner = c.children(root);
        if inner.start >= inner.end {
            return;
        }
        root = inner.end - 1;
    }
    if is_literal(c, root) {
        let range = c.utf16_span(root);
        c.report(range, &[]);
    }
}

/// The upstream `is Literal` set (:51).
fn is_literal(c: &Check<'_>, row: u32) -> bool {
    matches!(
        c.kind(row),
        NodeKind::LiteralBool
            | NodeKind::LiteralInt
            | NodeKind::LiteralDouble
            | NodeKind::LiteralNull
            | NodeKind::StringLiteral
            | NodeKind::ListLiteral
            | NodeKind::SetOrMapLiteral
            | NodeKind::SymbolLiteral
    )
}

#[cfg(test)]
mod tests {
    use crate::testsupport::{Fixture, one_rule};

    fn run(source: &str) -> Vec<crate::engine::Lint> {
        Fixture::parse(source).run(&one_rule("only_throw_errors"))
    }

    /// The literal faces report at the thrown expression (`throw 'bad'`,
    /// `throw null` — a `NullLiteral` is a literal); the typed arm's
    /// operand family (identifiers, constructor invocations) stays
    /// silent (the documented under-report register).
    #[test]
    fn literal_throw_faces() {
        let lints = run("void f() {\n  throw 'bad';\n}\n");
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Don't throw instances of classes that don't extend either 'Exception' or 'Error'."
        );

        assert_eq!(run("void f() {\n  throw null;\n}\n").len(), 1);
        assert_eq!(run("void f() {\n  throw 42;\n}\n").len(), 1);

        for source in [
            "void f() {\n  throw Exception('bad');\n}\n",
            "void f() {\n  throw state;\n}\n",
        ] {
            assert!(run(source).is_empty(), "{source}");
        }
    }
}
