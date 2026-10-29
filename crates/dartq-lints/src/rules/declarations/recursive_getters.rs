use super::{Check, NodeKind, PayloadField, RuleSpec, get_set_token, path_shadowed};

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/recursive_getters.dart:31-95`
/// — a getter (no parameter list) whose body references itself reports
/// at the reference. The element identity (`node.element != element`,
/// :57-60) is the in-rule shadow walk (`path_shadowed`): a same-lexeme
/// read captured between the read and the getter — by a local (`var
/// x`), a parameter or catch parameter, a for-loop variable, a
/// switch-case or if-case pattern variable, or a nested function of
/// that name — resolves to that binder, not the getter, and passes. An
/// if-case binding reaches the then branch only (the official scoping
/// visits the else branch outside the pattern scope), so an
/// else-branch read still reports. The const-literal subtree skip
/// (:61-70) covers erroneous code only and is dropped. Message:
/// `diagnostic.g.dart` `recursiveGetters`.
pub(crate) static SPEC: RuleSpec =
    crate::spec!("recursive_getters", &[NodeKind::Identifier], check,);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // Reads only (the official flags `SimpleIdentifier` references; a
    // declaration or label position never reads the getter).
    let Some(ctx) = c.field(node, PayloadField::Context) else {
        return;
    };
    if ctx != dartq_parser::IdentifierContext::Expression as u32 {
        return;
    }
    // `isSelfReference` parent guards (:57-64): a `prefixed`/`obj.x`
    // access never self-references; `this.x` does.
    if let Some(parent) = c.ancestor(1) {
        match c.kind(parent) {
            NodeKind::DotAccess => {
                let kids = c.collect_direct(parent);
                let name_side = kids.last() == Some(&node);
                let target_this = kids
                    .first()
                    .is_some_and(|&target| c.kind(target) == NodeKind::ThisExpression);
                if !(name_side && target_this) {
                    return;
                }
            }
            NodeKind::Qualified => return,
            _ => {}
        }
    }
    // The nearest enclosing getter with no parameter list
    // (:78-85: `parameters != null` bails).
    let Some(getter_index) = c.path.iter().rposition(|&ancestor| {
        matches!(
            c.kind(ancestor),
            NodeKind::Method | NodeKind::TopLevelMethod
        )
    }) else {
        return;
    };
    let getter = c.path[getter_index];
    if get_set_token(c, getter) != Some("get") {
        return;
    }
    // No parameters: no FormalParameters child (a getter's grammar has
    // none; the official's `node.parameters != null`).
    if c.children(getter)
        .any(|kid| c.kind(kid) == NodeKind::FormalParameters)
    {
        return;
    }
    // The reference must sit in the getter's body (:86-89 walk the body).
    let body = c
        .collect_direct(getter)
        .into_iter()
        .find(|&kid| matches!(c.kind(kid), NodeKind::ExpressionBody | NodeKind::BlockBody));
    let Some(body) = body else {
        return;
    };
    let name = c.lexeme(c.head_token(node));
    // The getter's own name, through the context-stamped declaration
    // name (the first name-shaped child answers the written return
    // type's leaked identifier row on production parses).
    let Some(getter_name) = super::member_declaration_name(c, getter) else {
        return;
    };
    if c.lexeme(c.head_token(getter_name)) != name {
        return;
    }
    let body_span = c.byte_span(body);
    if !body_span.contains(&c.byte_span(node).start) {
        return;
    }
    // The shadow walk — the `isSelfReference` element identity (:57-60):
    // a same-lexeme binder between the read and the getter captures it.
    if path_shadowed(c, getter_index, name) {
        return;
    }
    let range = c.utf16_span(node);
    c.report(range, &[name]);
}
