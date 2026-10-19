use super::{
    Check, Group, NodeKind, RuleSpec, annotation_names, container_members, fields_declare,
    member_declaration_name,
};
use crate::rules::{
    MemberQuery, member_annotations, member_matches, same_file_ancestors, utf16_token_span,
};

const STATIC_BIT: u16 = 1 << 3;

/// The `Object` members every interface inherits
/// (`getInheritedMember` answers them for any extension type).
const OBJECT_MEMBERS: [&str; 5] = ["==", "hashCode", "noSuchMethod", "runtimeType", "toString"];

/// Official: `pkg/linter/lib/src/rules/annotate_redeclares.dart:40-73`
/// (experimental) — a non-static method member of an extension type
/// (:42-44) that redeclares an inherited member without `@redeclare`
/// reports at the member name (:60, the name as the `{0}` argument).
/// Static members skip (:47); annotated members skip (:53).
///
/// Approximation (documented): `_redeclaresMember` (:65-73) reads
/// `extensionType.getInheritedMember(name)` off the element model; the
/// lexical face answers the `Object` members (inherited by every
/// interface) plus the same-file-resolvable interface chain (the
/// `implements`/`extends`/`with`/`on` supertypes and their transitive
/// same-file links — a member of the representation type itself is not
/// in the interface and never answers, matching the resolved face).
/// A supertype name that leaves the file under-reports. Message:
/// `diagnostic.g.dart` `annotateRedeclares`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "annotate_redeclares",
    group: Group::Standalone,
    problem: "The member '{0}' is redeclaring but isn't annotated with '@redeclare'.",
    correction: "Try adding the '@redeclare' annotation.",
    kinds: &[NodeKind::Method],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `node.parent?.parent is ExtensionTypeDeclaration` (:48-50).
    let Some(&ext_type) = c
        .path
        .iter()
        .rev()
        .find(|&&row| c.kind(row) == NodeKind::ExtensionTypeDeclaration)
    else {
        return;
    };
    // `node.isStatic` (:47).
    if dartq_ast::mod_flags(c.unit.ast, node) & STATIC_BIT != 0 {
        return;
    }
    let Some(name_token) = member_name_token(c, node) else {
        return;
    };
    let name = c.lexeme(name_token);
    // `element.metadata.hasRedeclare` (:53) — the member's own star.
    if c.unit.ast.parent(node).is_some_and(|body| {
        member_annotations(c, body, node)
            .iter()
            .any(|&meta| annotation_names(c, meta, "redeclare"))
    }) {
        return;
    }
    // `_redeclaresMember` (:65-73): `getInheritedMember(name) != null`.
    let inherited = OBJECT_MEMBERS.contains(&name) || interface_declares(c, ext_type, name);
    if inherited {
        c.report(utf16_token_span(c, name_token), &[name]);
    }
}

/// The member's name token: the declaration-context identifier's head
/// (a written return type's `Identifier` is a `TypeReference` sibling,
/// not the name — [`member_declaration_name`] reads the stamped face),
/// or the operator payload token for an operator name.
fn member_name_token(c: &Check<'_>, member: u32) -> Option<u32> {
    let name_row = member_declaration_name(c, member)?;
    match c.kind(name_row) {
        NodeKind::OperatorName => {
            let token = c.payload(name_row).first().copied()?;
            (token != crate::rules::NONE_TOKEN).then_some(token)
        }
        _ => Some(c.head_token(name_row)),
    }
}

/// Whether any same-file-resolvable supertype of the extension type
/// declares a member of `name` (the `getInheritedMember` face — every
/// member kind answers).
fn interface_declares(c: &Check<'_>, ext_type: u32, name: &str) -> bool {
    same_file_ancestors(c, ext_type, false).iter().any(|&row| {
        container_members(c, row)
            .1
            .into_iter()
            .any(|member| match c.kind(member) {
                NodeKind::Fields => fields_declare(c, member, name),
                NodeKind::Method => {
                    !member_matches(c, member, MemberQuery::Method, name).is_empty()
                        || !member_matches(c, member, MemberQuery::Getter, name).is_empty()
                        || !member_matches(c, member, MemberQuery::Setter, name).is_empty()
                }
                _ => false,
            })
    })
}
