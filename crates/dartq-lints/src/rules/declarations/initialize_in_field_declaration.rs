use super::{PayloadField, Check, NodeKind, RuleSpec};

/// The `late` `ModSet` bit (bridge `mod_bits`, member family).
const LATE_BIT: u16 = 1 << 5;

/// Official: `pkg/linter/lib/src/rules/initialize_in_field_declaration.dart`
/// — a primary constructor body initializer `y = expr` whose
/// expression references a primary constructor parameter reports at
/// the field name (:66-82). The field must be a same-class instance
/// field (:70-75) and not `late` (:71). Registration is feature-gated:
/// the primary-constructors `isFeatureEnabled` bail (:35) — pinned on
/// [`crate::engine::Features::primary_constructors`]. Message:
/// `diagnostic.g.dart` `initializeInFieldDeclaration`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "initialize_in_field_declaration",
    &[NodeKind::PrimaryConstructorBody],
    check,
);

fn check(c: &mut Check<'_>) {
    if !c.unit.features.primary_constructors {
        return; // registration bail (:35)
    }
    let node = c.node();
    let Some(&class_row) = c.path.iter().rev().find(|&&a| {
        matches!(
            c.kind(a),
            NodeKind::ClassDeclaration
                | NodeKind::EnumDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::ExtensionTypeDeclaration
        )
    }) else {
        return;
    };
    // The primary constructor of this class hangs beside the body (a
    // direct `ClassDeclaration` child, `NoPrimaryConstructor` when
    // absent); `node.declaration == null` bails (:62-64 — the parameter
    // names come from it).
    let Some(primary) = c
        .collect_direct(class_row)
        .into_iter()
        .find(|&member| c.kind(member) == NodeKind::PrimaryConstructor)
    else {
        return;
    };
    let param_names: Vec<String> = primary_formal_names(c, primary)
        .into_iter()
        .map(str::to_owned)
        .collect();
    if param_names.is_empty() {
        return;
    }
    for piece in c.collect_direct(node) {
        if c.kind(piece) != NodeKind::Initializers {
            continue;
        }
        for initializer in c.collect_direct(piece) {
            if c.kind(initializer) != NodeKind::Initializer {
                continue;
            }
            // The initializer row anchors at the field name (the row's
            // span head); the expression trails the `=`.
            let name_token = c.head_token(initializer);
            let name = c.lexeme(name_token);
            // A same-class instance field only (:70-75); `late` skips
            // (:71).
            let Some(is_late) = same_class_field_is_late(c, class_row, name) else {
                continue;
            };
            if is_late {
                continue;
            }
            let references = expression_references_param(c, initializer, &param_names);
            if references {
                c.report(crate::rules::utf16_token_span(c, name_token), &[]);
            }
        }
    }
}

/// The primary constructor's parameter names (payload word 1 of each
/// formal row).
fn primary_formal_names<'a>(c: &'a Check<'_>, primary: u32) -> Vec<&'a str> {
    let mut names = Vec::new();
    for params in c.collect_direct(primary) {
        if !matches!(
            c.kind(params),
            NodeKind::FormalParameters | NodeKind::OptionalFormalParameters
        ) {
            continue;
        }
        for kid in c.collect_direct(params) {
            if c.kind(kid) == NodeKind::FormalParameter
                && let Some(name) = c.field(kid, PayloadField::Name)
                && name != super::NONE_TOKEN
            {
                names.push(c.lexeme(name));
            }
        }
    }
    names
}

/// Whether a same-class instance field `name` exists, and whether it
/// is `late` (:70-75).
fn same_class_field_is_late(c: &Check<'_>, class_row: u32, name: &str) -> Option<bool> {
    for member in super::body_members(c, class_row) {
        if c.kind(member) != NodeKind::Fields {
            continue;
        }
        for decl in c.collect_direct(member) {
            if c.kind(decl) != NodeKind::VariablesDeclaration {
                continue;
            }
            for var in c.collect_direct(decl) {
                if c.kind(var) == NodeKind::InitializedIdentifier
                    && crate::rules::initialized_identifier_name(c, var)
                        .is_some_and(|tok| c.lexeme(tok) == name)
                {
                    let flags = dartq_ast::mod_flags(c.unit.ast, member);
                    return Some(
                        flags & LATE_BIT != 0 || c.lexeme(c.head_token(member)) == "late",
                    );
                }
            }
        }
    }
    None
}

/// Whether the initializer's `= expr` tail references one of the
/// primary parameters by name (the `_ParameterReferenceVisitor`
/// `visitSimpleIdentifier` face over the expression's tokens).
fn expression_references_param(c: &Check<'_>, initializer: u32, params: &[String]) -> bool {
    let range = c.unit.ast.token_range(initializer);
    let Some(eq) = range.clone().find(|&token| c.lexeme(token) == "=") else {
        return false;
    };
    (eq + 1..range.end).any(|token| params.iter().any(|param| param == c.lexeme(token)))
}
