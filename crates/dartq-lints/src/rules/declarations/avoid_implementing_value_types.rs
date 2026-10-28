use super::{Check, NodeKind, RuleSpec, container_members, operator_name};
use crate::rules::{SupertypeClause, same_file_ancestors, same_file_declaration, supertypes, utf16_token_span};

/// Official: `pkg/linter/lib/src/rules/avoid_implementing_value_types.dart:36-60`
/// — a class (:32, `addClassDeclaration`) whose `implements` clause
/// (:41-51) names a type that overrides `==` reports at the interface
/// type (:50). `_overridesEquals` (:55-59) resolves the interface
/// member and skips when its defining library is `dart:core`.
///
/// Approximation (documented): the interface and its concrete
/// (extends/with/on) same-file ancestor chain must declare `operator
/// ==` for the report; a name that leaves the file under-reports —
/// which covers the SDK value types (`int`, `String`) the resolved
/// face skips through the `isDartCore` guard. The report span is the
/// interface name (the written type arguments under-span the
/// upstream's full `TypeName` node). Message: `diagnostic.g.dart`
/// `avoidImplementingValueTypes`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "avoid_implementing_value_types",
    &[NodeKind::ClassDeclaration],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `node.implementsClause` (:41-44): the written supertype list and
    // its `Type` rows, both in source order (the names are owned so
    // the report borrow stays free).
    let names: Vec<(String, SupertypeClause)> = supertypes(c, node)
        .into_iter()
        .map(|(name, clause)| (name.to_string(), clause))
        .collect();
    let type_rows: Vec<u32> = c
        .collect_direct(node)
        .into_iter()
        .take_while(|&kid| {
            !matches!(c.kind(kid), NodeKind::DeclarationBody | NodeKind::EnumBody)
        })
        .filter(|&kid| c.kind(kid) == NodeKind::Type)
        .collect();
    for ((name, clause), &type_row) in names.iter().zip(&type_rows) {
        if *clause != SupertypeClause::Implements {
            continue;
        }
        // `interfaceType.element` (:47-49); an unresolved name
        // under-reports.
        let Some(row) = same_file_declaration(c, name) else {
            continue;
        };
        if !overrides_equals(c, row) {
            continue;
        }
        // `rule.reportAtNode(interface)` (:50).
        c.report(utf16_token_span(c, c.head_token(type_row)), &[]);
    }
}

/// `_overridesEquals` (:55-59) over the same-file substrate: the row
/// or a concrete (extends/with/on) same-file ancestor declares `==`.
/// The `OperatorName` tiles as the method's left sibling, not a child —
/// [`operator_name`] answers both faces.
fn overrides_equals(c: &Check<'_>, row: u32) -> bool {
    std::iter::once(row)
        .chain(same_file_ancestors(c, row, true))
        .any(|ancestor| {
            container_members(c, ancestor).1.into_iter().any(|member| {
                c.kind(member) == NodeKind::Method
                    && operator_name(c, member)
                        .and_then(|name| c.payload_word(name, 0))
                        .is_some_and(|t| c.lexeme(t) == "==")
            })
        })
}
