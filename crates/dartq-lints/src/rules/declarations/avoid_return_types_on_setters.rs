use super::{Check, Group, NodeKind, RuleSpec};

/// Official: `pkg/linter/lib/src/rules/avoid_return_types_on_setters.dart:38-56`
/// — a setter with an explicit return type reports at the type.
/// Setters carry the `set` keyword as their `get_set` payload
/// (the `Method`/`TopLevelMethod` payload). Message:
/// `diagnostic.g.dart` `avoidReturnTypesOnSetters`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "avoid_return_types_on_setters",
    group: Group::Recommended,
    problem: "Unnecessary return type on a setter.",
    correction: "Try removing the return type.",
    kinds: &[NodeKind::Method, NodeKind::TopLevelMethod],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    if super::get_set_token(c, node) != Some("set") {
        return;
    }
    let Some(name) = super::member_name(c, node) else {
        return;
    };
    let Some((type_node, kind)) = super::type_before(c, node, name) else {
        return;
    };
    if kind == NodeKind::NoType {
        return;
    }
    let range = c.utf16_span(type_node);
    c.report(range, &[]);
}
