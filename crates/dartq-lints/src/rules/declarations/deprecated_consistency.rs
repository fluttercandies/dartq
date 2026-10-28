use super::{Check, NodeKind, RuleSpec, constructor_name_part, container_members, is_deprecated_metadata, top_level_annotations, type_name_echo};
use crate::rules::{NONE_TOKEN, initialized_identifier_name, member_annotations, tail_token, utf16_token_span};

/// Official: `pkg/linter/lib/src/rules/deprecated_consistency.dart:41-102`
/// — deprecation must agree across related declarations. Three faces
/// under one name (`MultiAnalysisRule`, three codes): a constructor of
/// a `@deprecated` class without its own annotation reports at the
/// constructor's `errorRange` (`ast.dart:10800-10804`, the class-name
/// echo through the name part); a `this.x` initializing formal whose
/// field is deprecated and parameter is not reports at the parameter
/// (:81-87); a `this.x` initializing formal whose parameter is
/// deprecated and field is not (the field is public — private fields
/// skip, :74-79) reports at the field name (:88-100). Super formals
/// are not field formals (:70) and skip; primary constructors
/// (:37/:53-55) are out of the substrate. The `hasDeprecated`
/// resolution folds to the annotation's name identifier (`deprecated`
/// the constant, `Deprecated` the class — a prefix chain's trailing
/// segment answers). Messages: `diagnostic.g.dart`
/// `deprecatedConsistency{Constructor,Field,Parameter}`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "deprecated_consistency",
    &[
        NodeKind::Constructor,
        NodeKind::Factory,
        NodeKind::FormalParameter,
    ],
    check,
);

fn check(c: &mut Check<'_>) {
    match c.kind(c.node()) {
        NodeKind::Constructor | NodeKind::Factory => check_constructor(c),
        NodeKind::FormalParameter => check_parameter(c),
        _ => {}
    }
}

fn check_constructor(c: &mut Check<'_>) {
    let node = c.node();
    // `element.enclosingElement.metadata.hasDeprecated` (:58) — the
    // owning class-like row's annotations.
    let Some(&container) = c.path.iter().rev().find(|&&a| {
        matches!(
            c.kind(a),
            NodeKind::ClassDeclaration | NodeKind::EnumDeclaration
        )
    }) else {
        return;
    };
    if !top_level_annotations(c, container)
        .iter()
        .any(|&meta| is_deprecated_metadata(c, meta))
    {
        return;
    }
    // `!element.metadata.hasDeprecated` (:58) — the constructor's own
    // star (its left sibling in the body container).
    if c.unit.ast.parent(node).is_some_and(|body| {
        member_annotations(c, body, node)
            .iter()
            .any(|&m| is_deprecated_metadata(c, m))
    }) {
        return;
    }
    // `node.errorRange` — the class-name echo through the name part.
    let Some(echo) = type_name_echo(c, node) else {
        return;
    };
    let start = c.head_token(echo);
    let end = constructor_name_part(c, node).map_or(start, |part| tail_token(c, part));
    let (Some(start16), Some(end16)) = (c.utf16_of_token(start), c.utf16_end_of_token(end))
    else {
        return;
    };
    c.report_texts(
        start16..end16,
        "Constructors in a deprecated class should be deprecated.",
        "Try marking the constructor as deprecated.",
        &[],
    );
}

fn check_parameter(c: &mut Check<'_>) {
    let node = c.node();
    // `declaredElement is FieldFormalParameterElement` (:70): the
    // `this.` receiver face (`super.x` is a super formal).
    let receiver = c.payload(node).first().copied().unwrap_or(NONE_TOKEN);
    if receiver == NONE_TOKEN || c.lexeme(receiver) != "this" {
        return;
    }
    let Some(&name_token) = c.payload(node).get(1) else {
        return;
    };
    if name_token == NONE_TOKEN {
        return;
    }
    let name = c.lexeme(name_token);
    // `field.isPrivate` (:74-79).
    if name.starts_with('_') {
        return;
    }
    let Some(&class_row) = c
        .path
        .iter()
        .rev()
        .find(|&&a| c.kind(a) == NodeKind::ClassDeclaration)
    else {
        return;
    };
    let Some((fields_row, var_row)) = same_class_field(c, class_row, name) else {
        return;
    };
    let parameter_deprecated = c.unit.ast.parent(node).is_some_and(|body| {
        member_annotations(c, body, node)
            .iter()
            .any(|&m| is_deprecated_metadata(c, m))
    });
    let field_deprecated = c.unit.ast.parent(fields_row).is_some_and(|body| {
        member_annotations(c, body, fields_row)
            .iter()
            .any(|&m| is_deprecated_metadata(c, m))
    });
    if field_deprecated && !parameter_deprecated {
        // `rule.reportAtNode(node)` (:83-87) — the parameter span.
        c.report_texts(
            c.utf16_span(node),
            "Parameters that initialize a deprecated field should be deprecated.",
            "Try marking the parameter as deprecated.",
            &[],
        );
    } else if !field_deprecated && parameter_deprecated {
        // `reportAtOffset(field.nameOffset, nameLength)` (:95-99).
        let Some(field_name) = initialized_identifier_name(c, var_row) else {
            return;
        };
        c.report_texts(
            utf16_token_span(c, field_name),
            "Fields that are initialized by a deprecated parameter should be deprecated.",
            "Try marking the field as deprecated.",
            &[],
        );
    }
}

/// The same-class field declaring `name`: `(Fields row, variable row)`.
fn same_class_field(c: &Check<'_>, class_row: u32, name: &str) -> Option<(u32, u32)> {
    for member in container_members(c, class_row).1 {
        if c.kind(member) != NodeKind::Fields {
            continue;
        }
        for decl in c.collect_direct(member) {
            if c.kind(decl) != NodeKind::VariablesDeclaration {
                continue;
            }
            for var in c.collect_direct(decl) {
                if c.kind(var) == NodeKind::InitializedIdentifier
                    && initialized_identifier_name(c, var).is_some_and(|t| c.lexeme(t) == name)
                {
                    return Some((member, var));
                }
            }
        }
    }
    None
}
