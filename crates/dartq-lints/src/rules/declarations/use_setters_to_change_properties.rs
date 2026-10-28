use super::{Check, NodeKind, RuleSpec, container_members, fields_declare, is_identifier_token, name_row_token, override_annotation, parameter_rows, sole_statement_tokens};
use crate::rules::{MemberQuery, NONE_TOKEN, get_set_token, member_matches, member_name, same_file_ancestors, type_before, utf16_token_span};

const STATIC_BIT: u16 = 1 << 3;

/// Official: `pkg/linter/lib/src/rules/use_setters_to_change_properties.dart:42-78`
/// — a non-override method (:45-47) with exactly one parameter (:48)
/// and a written `void` return type (:49) whose body is a single
/// `field = parameter;` statement (:53-64/:66-76) reports at the
/// method name (:61). The element faces — `writeElement
/// .canonicalElement2 is FieldElement` (:60, the target is a class
/// field or explicit setter, possibly inherited) and `rightOperand ==
/// parameterElement` (:60, the value is that lone parameter) — fold
/// lexically: the `this.`-targeted form always names a member; the
/// bare form skips when the parameter name shadows the target and
/// otherwise reads the same-class field/setter set or a concrete
/// same-file ancestor's instance members (a cross-file inherited
/// target under-reports). `node.isOverride` (:47) answers on the
/// `@override` annotation or a same-file ancestor's same-named method.
/// Message: `diagnostic.g.dart` `useSettersToChangeProperties`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "use_setters_to_change_properties",
    &[NodeKind::Method],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `node.isSetter || node.isGetter` (:45-46).
    if matches!(get_set_token(c, node), Some("get" | "set")) {
        return;
    }
    let Some(name_row) = member_name(c, node) else {
        return;
    };
    let name = c.lexeme(c.head_token(name_row));
    // `node.returnType?.type is! VoidType` (:49) — the written `void`.
    if !matches!(
        type_before(c, node, name_row),
        Some((_, NodeKind::VoidType))
    ) {
        return;
    }
    // `node.parameters.parameters.length != 1` (:48).
    let Some(params_row) = c
        .collect_direct(node)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
    else {
        return;
    };
    let params = parameter_rows(c, params_row);
    if params.len() != 1 {
        return;
    }
    let Some(param) = parameter_name_token(c, params[0]) else {
        return;
    };
    let param = c.lexeme(param);
    // `node.isOverride` (:47).
    if override_annotation(c, node) || ancestor_method(c, name) {
        return;
    }
    // The single-statement body: `field = param;` or
    // `this.field = param;` (block or `=>` flavor).
    let Some(statement) = sole_statement_tokens(c, node) else {
        return;
    };
    let Some((field, value, this_target)) = assignment_shape(c, statement) else {
        return;
    };
    if value != param {
        return;
    }
    // `leftOperand is FieldElement` (:60): the bare target skips when
    // the parameter shadows it; otherwise it must name a class field
    // or setter. A `this.`-targeted assignment is a member face by
    // grammar.
    if !this_target && (field == param || !class_member_named(c, field)) {
        return;
    }
    c.report(utf16_token_span(c, name_row_token(c, name_row)), &[]);
}

/// The name token of a formal parameter row (`FormalParameter` payload
/// word 1; a function-typed formal anchors at its name).
fn parameter_name_token(c: &Check<'_>, parameter: u32) -> Option<u32> {
    match c.kind(parameter) {
        NodeKind::FormalParameter => match c.payload(parameter).get(1) {
            Some(&name) if name != NONE_TOKEN => Some(name),
            _ => None,
        },
        NodeKind::FunctionTypedFormalParameter => Some(c.head_token(parameter)),
        _ => None,
    }
}

/// `(field lexeme, value lexeme, this-targeted)` when the token run is
/// exactly `x = v ;` / `this . x = v ;` with a bare-identifier value.
fn assignment_shape<'a>(
    c: &'a Check<'_>,
    statement: core::ops::Range<u32>,
) -> Option<(&'a str, &'a str, bool)> {
    let mut end = statement.end;
    if end > statement.start && c.lexeme(end - 1) == ";" {
        end -= 1;
    }
    let mut cursor = statement.start;
    let (field, this_target) = if c.lexeme(cursor) == "this" {
        (c.lexeme(cursor + 2), true)
    } else {
        (c.lexeme(cursor), false)
    };
    cursor += if this_target { 3 } else { 1 };
    // Exactly `= value` remains.
    if end.checked_sub(cursor) != Some(2) || c.lexeme(cursor) != "=" {
        return None;
    }
    if !is_identifier_token(c, cursor + 1) {
        return None;
    }
    Some((field, c.lexeme(cursor + 1), this_target))
}

/// Whether a same-file ancestor declares a method of `name` (the
/// resolved `lookUpMethod` face of `node.isOverride`).
fn ancestor_method(c: &Check<'_>, name: &str) -> bool {
    let Some(&container) = c.path.iter().rev().find(|&&a| {
        matches!(
            c.kind(a),
            NodeKind::ClassDeclaration
                | NodeKind::EnumDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::ExtensionDeclaration
                | NodeKind::ExtensionTypeDeclaration
        )
    }) else {
        return false;
    };
    same_file_ancestors(c, container, false).iter().any(|&a| {
        container_members(c, a)
            .1
            .iter()
            .any(|&m| !member_matches(c, m, MemberQuery::Method, name).is_empty())
    })
}

/// Whether `name` resolves to a field or setter of the container (the
/// `canonicalElement2 is FieldElement` face): the container's own
/// fields and explicit setters, or a concrete same-file ancestor's
/// instance fields and setters.
fn class_member_named(c: &Check<'_>, name: &str) -> bool {
    let Some(&container) = c
        .path
        .iter()
        .rev()
        .find(|&&a| c.kind(a) == NodeKind::ClassDeclaration)
    else {
        return false;
    };
    let own = container_members(c, container)
        .1
        .iter()
        .any(|&m| match c.kind(m) {
            NodeKind::Fields => fields_declare(c, m, name),
            NodeKind::Method => {
                get_set_token(c, m) == Some("set")
                    && !member_matches(c, m, MemberQuery::Setter, name).is_empty()
            }
            _ => false,
        });
    own || same_file_ancestors(c, container, true).iter().any(|&a| {
        container_members(c, a).1.iter().any(|&m| match c.kind(m) {
            NodeKind::Fields
                if c.payload(m).first()
                    == Some(&(dartq_parser::MemberKind::NonStaticField as u32)) =>
            {
                fields_declare(c, m, name)
            }
            NodeKind::Method => {
                get_set_token(c, m) == Some("set")
                    && !member_matches(c, m, MemberQuery::Setter, name).is_empty()
                    && dartq_ast::mod_flags(c.unit.ast, m) & STATIC_BIT == 0
            }
            _ => false,
        })
    })
}
