use super::{Check, NodeKind, PayloadField, RuleSpec, fields_written_type, obvious_spelling};
use crate::rules::{
    MemberQuery, last_child, member_matches, same_file_ancestors, same_file_declaration, supertypes,
};

/// Official: `pkg/linter/lib/src/rules/specify_nonobvious_property_types.dart:47-120`
/// — a top-level or field variable list (:43-44) with no written type
/// (or exactly `Null`, :63-65) where some variable needs a type
/// reports at the list for a single variable (:112-113) or at each
/// needing variable (:115-116). An instance variable whose name a
/// super-interface accessor answers skips (:71-100 — the
/// override-inference face).
///
/// Approximation (documented): the override-inference face reads the
/// same-file ancestor chain — when any written supertype name leaves
/// the file the whole instance face under-reports (the resolved
/// `allSupertypes` walk is unavailable); static and top-level
/// variables carry no such face. Message: `diagnostic.g.dart`
/// `specifyNonobviousPropertyTypes`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "specify_nonobvious_property_types",
    &[NodeKind::VariablesDeclaration],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    let Some(parent) = c.ancestor(1) else {
        return;
    };
    if !matches!(c.kind(parent), NodeKind::TopLevelFields | NodeKind::Fields) {
        return;
    }
    // `staticType != null && !staticType.isDartCoreNull` (:63-65).
    if fields_written_type(c, node).is_some_and(|(start, _)| c.lexeme(start) != "Null") {
        return;
    }
    // The instance face: `!node.isStatic` (:52).
    let instance = c.kind(parent) == NodeKind::Fields
        && c.field(parent, PayloadField::MemberKind)
            == Some(dartq_parser::MemberKind::NonStaticField as u32);
    let container = c.path.iter().rev().find(|&&row| {
        matches!(
            c.kind(row),
            NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::EnumDeclaration
                | NodeKind::ExtensionDeclaration
                | NodeKind::ExtensionTypeDeclaration
        )
    });
    // (:75-99) — the walk needs every supertype resolvable; a
    // cross-file link under-reports the whole instance face.
    let resolvable = container.is_none_or(|&row| {
        supertypes(c, row)
            .iter()
            .all(|(name, _)| same_file_declaration(c, name).is_some())
    });
    let mut needs = Vec::new();
    let mut declarators = 0;
    for var in c.collect_direct(node) {
        if c.kind(var) != NodeKind::InitializedIdentifier {
            continue;
        }
        declarators += 1;
        if instance {
            let overridden = container
                .zip(crate::rules::initialized_identifier_name(c, var))
                .is_some_and(|(&row, token)| ancestor_accessor(c, row, c.lexeme(token)));
            if overridden || !resolvable {
                continue;
            }
        }
        let initializer = c
            .collect_direct(var)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer);
        let obvious = initializer
            .and_then(|init| last_child(c, init))
            .is_some_and(|value| obvious_spelling(c, value).is_some());
        if !obvious {
            needs.push(var);
        }
    }
    if needs.is_empty() {
        return;
    }
    if declarators == 1 {
        c.report(c.utf16_span(node), &[]);
    } else {
        for var in needs {
            c.report(c.utf16_span(var), &[]);
        }
    }
}

/// Whether a same-file ancestor declares a getter, setter or field of
/// `name` (the `getGetter`/`getSetter` face — a field carries both).
fn ancestor_accessor(c: &Check<'_>, container: u32, name: &str) -> bool {
    same_file_ancestors(c, container, false).iter().any(|&row| {
        super::container_members(c, row)
            .1
            .into_iter()
            .any(|member| match c.kind(member) {
                NodeKind::Fields => super::fields_declare(c, member, name),
                NodeKind::Method => {
                    !member_matches(c, member, MemberQuery::Getter, name).is_empty()
                        || !member_matches(c, member, MemberQuery::Setter, name).is_empty()
                }
                _ => false,
            })
    })
}
