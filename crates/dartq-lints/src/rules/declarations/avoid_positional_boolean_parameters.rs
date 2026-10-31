use super::constructor_name_part;
use super::{
    AUGMENT_MEMBER_BIT, Check, NodeKind, PayloadField, RuleSpec, formal_parameter_rows,
    member_declaration_name, same_class_field_type,
};
use crate::rules::{
    MemberQuery, NONE_TOKEN, body_members, get_set_token, member_matches, member_name,
    same_file_ancestors,
};
use dartq_ast::mod_flags;

/// Official:
/// `pkg/linter/lib/src/rules/avoid_positional_boolean_parameters.dart`
/// — the FIRST positional parameter whose type is `bool` reports at the
/// parameter (:50-54, `firstWhereOrNull(_isBoolean)` +
/// `reportAtNode(parameterToLint)`). Public declarations only
/// (:51/:62/:82/:93); getters, setters, and operators skip (:76-78); a
/// fixed signature (an augmentation or an override, :79 + the
/// extensions :129-151) skips, and a generic function type inside a
/// fixed signature skips (:116-126). The augmentation skip keeps the
/// ORIGINAL declaration of an augmented member lintable — an augmenting
/// part's `augment A(bool b)` stays silent while the plain `A(bool b)`
/// it augments reports. Message: `diagnostic.g.dart`
/// `avoidPositionalBooleanParameters`.
///
/// Approximations: the resolved parameter type folds to the written
/// type spelling — exactly `bool` (an import-prefixed or aliased
/// spelling under-reports); a `this.f` field formal reads the
/// same-class field's written type. The override face folds to the
/// same-file any-member ancestor scan — `implements` interfaces join the
/// chain, the `getInheritedMember` cut (cross-file overrides
/// under-report); augmentations read the member `ModSet` augment bit.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "avoid_positional_boolean_parameters",
    &[
        NodeKind::Constructor,
        NodeKind::TopLevelMethod,
        NodeKind::Method,
        NodeKind::LocalFunctionDeclaration,
        NodeKind::PrimaryConstructor,
        NodeKind::FunctionType,
    ],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    match c.kind(node) {
        NodeKind::PrimaryConstructor => {
            if mod_flags(c.unit.ast, node) & AUGMENT_MEMBER_BIT != 0 {
                return; // (:90)
            }
            if primary_is_private(c, node) {
                return; // (:93)
            }
            check_params(c, node, class_like_row(c));
        }
        NodeKind::FunctionType => {
            if in_fixed_signature(c, node) {
                return; // (:116-126)
            }
            check_params(c, node, None);
        }
        NodeKind::Constructor
        | NodeKind::TopLevelMethod
        | NodeKind::Method
        | NodeKind::LocalFunctionDeclaration => {
            if mod_flags(c.unit.ast, node) & AUGMENT_MEMBER_BIT != 0 {
                return; // (:48/:59/:81)
            }
            if c.kind(node) == NodeKind::Method {
                if get_set_token(c, node).is_some() {
                    return; // getters/setters (:76-77)
                }
                // Operators skip (:78) — the `OperatorName` tiles as the
                // method row's LEFT SIBLING (bridge `handleOperatorName`
                // precedes `beginMethod`), never as a child.
                if let Some(parent) = c.unit.ast.parent(node) {
                    let siblings = c.collect_direct(parent);
                    if let Some(pos) = siblings.iter().position(|&kid| kid == node) {
                        if siblings[..pos]
                            .iter()
                            .rev()
                            .any(|&kid| c.kind(kid) == NodeKind::OperatorName)
                        {
                            return;
                        }
                    }
                }
                if fixed_signature(c, node) {
                    return; // (:79)
                }
            }
            if is_private(c, node) {
                return; // (:51/:62/:82)
            }
            check_params(c, node, class_like_row(c));
        }
        _ => {}
    }
}

/// `checkParams` (:50-54): the FIRST positional `bool` parameter reports
/// — `firstWhereOrNull`, one report per declaration.
fn check_params(c: &mut Check<'_>, owner: u32, class_row: Option<u32>) {
    for param in formal_parameter_rows(c, owner) {
        if positional(c, param) && bool_typed(c, param, class_row) {
            c.report(param_span(c, param), &[]);
            return;
        }
    }
}

/// The parameter's extent as the official node spans it: through the
/// default value's last token. The row seals `end_incl` on
/// `InitializerEnd`, which the parser sets to the peek AFTER the default
/// expression (exclusive bound), so a defaulted parameter over-covers by
/// the trailing `,`/`]`/`)` token — trim back to `InitializerEnd - 1`.
fn param_span(c: &Check<'_>, param: u32) -> core::ops::Range<u32> {
    let full = c.utf16_span(param);
    match c
        .field(param, PayloadField::InitializerEnd)
        .filter(|&end| end != NONE_TOKEN)
        .and_then(|end| c.utf16_end_of_token(end - 1))
    {
        Some(end) if end > full.start => full.start..end,
        _ => full,
    }
}

/// `p.isPositional` — the required-positional and optional-positional
/// parameter kinds (the row's `FormalParameterKind` word, bridge
/// `end_formal_parameter`).
fn positional(c: &Check<'_>, param: u32) -> bool {
    matches!(
        c.field(param, PayloadField::FormalParameterKind),
        Some(kind)
            if kind == dartq_parser::FormalParameterKind::RequiredPositional as u32
                || kind == dartq_parser::FormalParameterKind::OptionalPositional as u32
    )
}

/// The `bool` face of the parameter's type: the written type slice
/// answers; a `this.f` field formal reads the same-class field's
/// written type (the element's declared-type face).
fn bool_typed(c: &Check<'_>, param: u32, class_row: Option<u32>) -> bool {
    let written = c.collect_direct(param).into_iter().find(|&kid| {
        matches!(
            c.kind(kid),
            NodeKind::Type | NodeKind::FunctionType | NodeKind::RecordType
        )
    });
    if let Some(t) = written {
        return &c.unit.source[c.byte_span(t)] == "bool";
    }
    let receiver = c.field(param, PayloadField::Receiver).unwrap_or(NONE_TOKEN);
    if receiver == NONE_TOKEN || c.lexeme(receiver) != "this" {
        return false;
    }
    let Some(name) = c.field(param, PayloadField::Name) else {
        return false;
    };
    if name == NONE_TOKEN {
        return false;
    }
    let Some(class_row) = class_row else {
        return false;
    };
    same_class_field_type(c, class_row, c.lexeme(name))
        .is_some_and(|t| &c.unit.source[c.byte_span(t)] == "bool")
}

/// The written-name privacy face (:51/:62/:82): a constructor's name
/// part, else the declaration name, starting with `_`. A local
/// function's name nests under `FunctionName` — read through
/// [`member_declaration_name`].
fn is_private(c: &Check<'_>, owner: u32) -> bool {
    let name = match c.kind(owner) {
        NodeKind::Constructor => constructor_name_part(c, owner),
        _ => member_declaration_name(c, owner),
    };
    name.is_some_and(|name| c.lexeme(c.head_token(name)).starts_with('_'))
}

/// The named-section privacy face (:93): a
/// `PrimaryConstructorDeclaration`-context name starting with `_`.
fn primary_is_private(c: &Check<'_>, primary: u32) -> bool {
    let ctx = dartq_parser::IdentifierContext::PrimaryConstructorDeclaration as u32;
    c.collect_direct(primary).into_iter().any(|kid| {
        c.kind(kid) == NodeKind::Identifier
            && c.field(kid, PayloadField::Context) == Some(ctx)
            && c.lexeme(c.head_token(kid)).starts_with('_')
    })
}

/// `hasFixedSignature` (:129-138): an augmentation, or a method a
/// same-file supertype declares under the ANY-member cut
/// (`getInheritedMember` — `implements` interfaces join the extends/with
/// chain, `extensions.dart` :574-597; cross-file supertypes
/// under-report).
fn fixed_signature(c: &Check<'_>, method: u32) -> bool {
    if mod_flags(c.unit.ast, method) & AUGMENT_MEMBER_BIT != 0 {
        return true;
    }
    let Some(name) = member_name(c, method) else {
        return false;
    };
    let name = c.lexeme(c.head_token(name));
    let Some(class_row) = class_like_row(c) else {
        return false;
    };
    same_file_ancestors(c, class_row, false)
        .iter()
        .any(|&ancestor| {
            body_members(c, ancestor)
                .iter()
                .any(|&member| !member_matches(c, member, MemberQuery::Method, name).is_empty())
        })
}

/// `_isInFixedSignature` (:116-126): the walk stops at a body region
/// (not fixed) or a `MethodDeclaration` (its own fixed face); any
/// other container keeps climbing.
fn in_fixed_signature(c: &Check<'_>, node: u32) -> bool {
    let mut current = node;
    while let Some(parent) = c.unit.ast.parent(current) {
        match c.kind(parent) {
            NodeKind::BlockBody | NodeKind::ExpressionBody => return false,
            NodeKind::Method => return fixed_signature(c, parent),
            _ => {}
        }
        current = parent;
    }
    false
}

/// The owning class-like row (the field-formal reads need it).
fn class_like_row(c: &Check<'_>) -> Option<u32> {
    c.path[..c.path.len() - 1].iter().rev().copied().find(|&a| {
        matches!(
            c.kind(a),
            NodeKind::ClassDeclaration | NodeKind::EnumDeclaration | NodeKind::MixinDeclaration
        )
    })
}
