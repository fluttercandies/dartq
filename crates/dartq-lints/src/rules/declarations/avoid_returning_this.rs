use super::{Check, NodeKind, RuleSpec, member_name, type_before};

/// Official: `pkg/linter/lib/src/rules/avoid_returning_this.dart:68-106`
/// — a class/enum/mixin method (never an extension member :91, never an
/// operator :71, never `@override` :77) whose written return type names
/// the enclosing declaration (:81-85, the `InterfaceType` element
/// identity) and whose body hands back `this` for a fluent interface
/// reports. The block-body face collects `return this;` statements in
/// order (:38-66): the first bare `this` return reports at its `this`
/// (:98), any non-`this` return before it silences the method
/// (:58-62), and returns inside nested function expressions are
/// skipped (:49-51 — their blocks are sibling rows, not the method
/// body's block). The expression-body face reports at the method name
/// (:100-104). Message: `diagnostic.g.dart` `avoidReturningThis`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "avoid_returning_this",
    &[NodeKind::Method],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // Operators skip (:71) — the `OperatorName` row names them.
    if c.collect_direct(node)
        .into_iter()
        .any(|kid| c.kind(kid) == NodeKind::OperatorName)
    {
        return;
    }
    // The name row: the `MethodDeclaration`-stamped `Identifier` (the
    // production tiling puts the return-type identifier first —
    // `A add(...)` reads `A` unfiltered).
    let Some(name) = member_name(c, node) else {
        return;
    };
    // The innermost class-like ancestor answers; an extension member
    // bails (:91) because the extension row sits between.
    let Some(&class_row) = c.path.iter().rev().skip(1).find(|&&ancestor| {
        matches!(
            c.kind(ancestor),
            NodeKind::ClassDeclaration
                | NodeKind::EnumDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::ExtensionDeclaration
        )
    }) else {
        return;
    };
    if c.kind(class_row) == NodeKind::ExtensionDeclaration {
        return;
    }
    // `@override` skips (:77): the annotation rides the member row's
    // left `MetadataStar` sibling in its body container.
    let body = c
        .collect_direct(class_row)
        .into_iter()
        .find(|&kid| matches!(c.kind(kid), NodeKind::DeclarationBody | NodeKind::EnumBody))
        .unwrap_or(class_row);
    if super::body_members(c, class_row).contains(&node)
        && crate::rules::has_override_annotation(c, body, node)
    {
        return;
    }
    // The written return type must name the enclosing declaration
    // (:81-85); unwritten (`NoType`) or any other name bails. The
    // name-equality face is the `returnType.element == parent.element`
    // identity over the written spelling.
    let Some((type_row, kind)) = type_before(c, node, name) else {
        return;
    };
    let Some(class_name) = crate::rules::class_declaration_name(c, class_row) else {
        return;
    };
    if kind != NodeKind::Type || c.lexeme(c.head_token(type_row)) != class_name {
        return;
    }
    let body_row = c.collect_direct(node).into_iter().find(|&kid| {
        matches!(
            c.kind(kid),
            NodeKind::BlockBody | NodeKind::ExpressionBody | NodeKind::Block
        )
    });
    let Some(body_row) = body_row else {
        return;
    };
    match c.kind(body_row) {
        NodeKind::BlockBody | NodeKind::Block => {
            // The returns in pre-order, nested closures pruned
            // (`visitFunctionExpression` short-circuits, :49-51): the
            // first bare `return this;` reports at its `this`; any
            // non-`this` return reached first clears the set and stops
            // the walk (:55-62).
            let mut first_this = None;
            let mut silenced = false;
            collect_returns(c, body_row, &mut first_this, &mut silenced);
            if let Some(this_token) = first_this {
                c.report(crate::rules::utf16_token_span(c, this_token), &[]);
            }
        }
        NodeKind::ExpressionBody => {
            // `=> this;` reports at the method name (:100-104). The
            // production `ExpressionBody` row spans the arrow through
            // the `;`, and the expression rows tile beside it — the
            // bare face is a `ThisExpression` sibling spanning exactly
            // the tokens between the arrow and the `;` (`=> this.x`
            // tiles a narrower `this` leaf and never qualifies).
            let range = c.unit.ast.token_range(body_row);
            let rhs = range.start + 1..range.end.saturating_sub(1);
            let is_this = c.collect_direct(node).into_iter().any(|kid| {
                c.kind(kid) == NodeKind::ThisExpression && c.unit.ast.token_range(kid) == rhs
            });
            if is_this {
                c.report(crate::rules::utf16_token_span(c, c.head_token(name)), &[]);
            }
        }
        _ => {}
    }
}

/// The `_BodyVisitor` walk: pre-order `ReturnStatement` collection over
/// the body row's subtree, stopping at the first non-`this` return
/// (which clears the collected set, :55-62) and never descending into
/// nested function expressions (:49-51). Leaves the first collected
/// `this` token behind, `None` once silenced.
fn collect_returns(c: &Check<'_>, row: u32, first_this: &mut Option<u32>, silenced: &mut bool) {
    if *silenced {
        return;
    }
    if c.kind(row) == NodeKind::ReturnStatement {
        let range = c.unit.ast.token_range(row);
        if range.len() == 3
            && c.lexeme(range.start) == "return"
            && c.lexeme(range.start + 1) == "this"
        {
            if first_this.is_none() {
                *first_this = Some(range.start + 1);
            }
        } else {
            *silenced = true;
            *first_this = None;
        }
        return;
    }
    if matches!(
        c.kind(row),
        NodeKind::FunctionExpression
            | NodeKind::NamedFunctionExpression
            | NodeKind::LocalFunctionDeclaration
    ) {
        return;
    }
    for kid in c.collect_direct(row) {
        collect_returns(c, kid, first_this, silenced);
    }
}
