use super::{
    Check, NodeKind, PayloadField, RuleSpec, container_members, fields_declare, name_row_token,
};
use crate::rules::{
    MemberQuery, get_set_token, has_override_annotation, member_matches, member_name,
    same_file_ancestors, utf16_token_span,
};

const STATIC_BIT: u16 = 1 << 3;

/// Official: `pkg/linter/lib/src/rules/avoid_setters_without_getters.dart:38-83`
/// — a setter member of a class, enum or extension type (mixins are
/// excluded upstream, :34) with no corresponding getter reports at the
/// setter name (:79). The resolved faces — `getOverridden` (:66-67,
/// any overriding setter skips), `interface.getGetter` (:72-74, the
/// declared explicit getter) and `getInheritedConcreteMember`
/// (:75-76, the inherited concrete getter) — approximate over the
/// same-file substrate: the override face answers on the `@override`
/// annotation or a same-file ancestor's same-named setter; the getter
/// face answers on the container's own explicit getter or field (a
/// field carries the readable-property face; upstream's
/// `InterfaceElement.getters` list question is untestable here — see
/// the report boundary) or a concrete (extends/with/on) same-file
/// ancestor's instance getter or field. Cross-file ancestors
/// under-report. Message: `diagnostic.g.dart`
/// `avoidSettersWithoutGetters`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "avoid_setters_without_getters",
    &[
        NodeKind::ClassDeclaration,
        NodeKind::EnumDeclaration,
        NodeKind::ExtensionTypeDeclaration,
    ],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    let (body, members) = container_members(c, node);
    for member in members {
        // Setters only (:55-56).
        if c.kind(member) != NodeKind::Method || get_set_token(c, member) != Some("set") {
            continue;
        }
        // The `MethodDeclaration`-stamped `Identifier` — a written
        // return type tiles a `TypeReference` `Identifier` ahead of
        // the name, so the first-`Identifier` read is wrong; setters
        // never name through an operator.
        let Some(name_row) = member_name(c, member) else {
            continue;
        };
        let name = c.lexeme(c.head_token(name_row));
        // `getOverridden(name)` (:66-67): an overriding setter skips.
        if has_override_annotation(c, body, member) || ancestor_declares(c, node, name, true) {
            continue;
        }
        if !has_getter(c, node, name) {
            c.report(utf16_token_span(c, name_row_token(c, name_row)), &[]);
        }
    }
}

/// Whether any same-file ancestor (all clauses when `any_clause`, else
/// the concrete extends/with/on chain) declares a setter of `name`.
fn ancestor_declares(c: &Check<'_>, container: u32, name: &str, any_clause: bool) -> bool {
    same_file_ancestors(c, container, !any_clause)
        .iter()
        .any(|&a| {
            container_members(c, a)
                .1
                .iter()
                .any(|&m| !member_matches(c, m, MemberQuery::Setter, name).is_empty())
        })
}

/// `interface.getGetter(name)` (:72-74) then the inherited concrete
/// getter (:75-76): the container's own explicit getter or field, else
/// a concrete same-file ancestor's instance getter or field.
fn has_getter(c: &Check<'_>, container: u32, name: &str) -> bool {
    let (_, members) = container_members(c, container);
    for member in members {
        match c.kind(member) {
            NodeKind::Method => {
                if get_set_token(c, member) == Some("get")
                    && !member_matches(c, member, MemberQuery::Getter, name).is_empty()
                {
                    return true;
                }
            }
            NodeKind::Fields if fields_declare(c, member, name) => {
                return true;
            }
            _ => {}
        }
    }
    same_file_ancestors(c, container, true)
        .iter()
        .any(|&ancestor| {
            container_members(c, ancestor)
                .1
                .iter()
                .any(|&member| match c.kind(member) {
                    NodeKind::Method => {
                        get_set_token(c, member) == Some("get")
                            && !member_matches(c, member, MemberQuery::Getter, name).is_empty()
                            && dartq_ast::mod_flags(c.unit.ast, member) & STATIC_BIT == 0
                    }
                    NodeKind::Fields
                        if c.field(member, PayloadField::MemberKind)
                            == Some(dartq_parser::MemberKind::NonStaticField as u32) =>
                    {
                        fields_declare(c, member, name)
                    }
                    _ => false,
                })
        })
}
