use super::{Check, Group, NodeKind, RuleSpec, formal_parameter_rows};
use crate::rules::{MemberQuery, has_override_annotation, member_matches, same_file_ancestors};

/// Official: `pkg/linter/lib/src/rules/annotate_overrides.dart` — an
/// instance field or method (getters/setters/operators ride the same
/// `MethodDeclaration` face) that overrides an inherited member without
/// a `@override` annotation reports at the member's name token, with
/// the inherited member's name as the argument (:40-72); a `var`/
/// `final` primary-constructor formal that declares a field reports
/// the same way at its name token (:74-93).
///
/// Approximations (documented): `overriddenMember`
/// (`extensions.dart:389-406` — `getInheritedMember`, abstract or
/// concrete, interfaces included) is the same-file ancestor chain built
/// from the written supertype names (extends/with/on/implements,
/// `same_file_ancestors(concrete: false)`), plus the semantic bridge's
/// inheritance-chain arm for the cross-file residue — each written
/// supertype anchors the cross-file extends/with walk (abstract or
/// concrete; the checked member is self-declared, so the anchor rides
/// the supertypes, whose own declarations are genuinely inherited).
/// Cross-file `implements`/`on` edges stay outside the walk's
/// name-level chain and under-report. The static
/// skips (:57, :68), the augmentation skip (augmentations are
/// scanner-gated off), the extension-type skip (:58, :69) and the
/// `@override` check (:42) port directly; the primary-constructor
/// owner gate admits classes and enums only (:75-77) and its keyword
/// gate proceeds on `final`/`var` alone (:78-80). A field's (and a
/// primary formal's) lookup answers the getter name namespace (:391 —
/// the field's getter); a same-named ancestor *method* (a
/// member-namespace conflict, erroneous code) stays silent in the
/// same-file arm. The rendered argument is the overridden element's
/// `name` — analyzer setter elements are named `x=` (`displayName`
/// `x`), so a setter override renders `x=`.
/// Message: `diagnostic.g.dart` `annotateOverrides`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "annotate_overrides",
    group: Group::Recommended,
    problem: "The member '{0}' overrides an inherited member but isn't annotated with '@override'.",
    correction: "Try adding the '@override' annotation.",
    kinds: &[
        NodeKind::Fields,
        NodeKind::Method,
        NodeKind::PrimaryConstructor,
    ],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    if c.kind(node) == NodeKind::PrimaryConstructor {
        check_primary(c);
        return;
    }
    if !is_instance_member(c, node) {
        return;
    }
    // The body: the member's parent — the MemberEnd collapse enters
    // members into the body's children directly;
    // hand-built tables may skip the wrapper.
    let Some(owner) = c.ancestor(1) else {
        return;
    };
    if !matches!(
        c.kind(owner),
        NodeKind::DeclarationBody
            | NodeKind::ClassDeclaration
            | NodeKind::MixinDeclaration
            | NodeKind::EnumDeclaration
            | NodeKind::ExtensionDeclaration
    ) {
        return;
    }
    // `element.metadata.hasOverride` (:42).
    if has_override_annotation(c, owner, node) {
        return;
    }
    let Some(&class_row) = class_row_of(c) else {
        return;
    };
    // The member's own namespace: getters/setters override the same
    // accessor kind upstream (the element model), operators and plain
    // methods the method namespace.
    let query = match c.kind(node) {
        NodeKind::Method => match super::get_set_token(c, node) {
            Some("get") => MemberQuery::Getter,
            Some("set") => MemberQuery::Setter,
            _ => MemberQuery::Method,
        },
        _ => MemberQuery::Getter,
    };
    // The chain is a per-container invariant — derive it once, not per
    // member name (#758).
    let ancestors = same_file_ancestors(c, class_row, false);
    for name_row in member_names(c, node) {
        // Both faces name at the row's span head (the method's
        // `Identifier`, the field's width-0-at-name
        // `InitializedIdentifier`); the `OperatorName` row names
        // through its payload token (the `==` itself — the head is
        // the `operator` keyword). The official report anchors at the
        // name token.
        let name_token = if c.kind(name_row) == NodeKind::OperatorName {
            c.payload(name_row)
                .first()
                .copied()
                .unwrap_or_else(|| c.head_token(name_row))
        } else {
            c.head_token(name_row)
        };
        let name = c.lexeme(name_token);
        let overridden = ancestors
            .iter()
            .any(|&ancestor| inherited_member(c, ancestor, query, name).is_some())
            || bridge_overrides(c, class_row, query, name)
            || core_object_declares(query, name);
        if overridden {
            let range = crate::rules::utf16_token_span(c, name_token);
            // The official argument is the overridden element's `name`
            // (:47 `arguments: [member.name!]`) — a setter element is
            // named `x=`.
            let reported = if query == MemberQuery::Setter {
                format!("{name}=")
            } else {
                name.to_owned()
            };
            c.report(range, &[reported.as_str()]);
        }
    }
}

/// The primary-constructor parameter face (:74-93): each formal of a
/// class or enum primary constructor that declares a field — the
/// keyword gate admits `final`/`var` only (:78-80) — checks like a
/// field: the inherited getter namespace, reporting at the name token
/// with no `=` suffix. The owner gate (:75-77, `parent is
/// ClassDeclaration || parent is EnumDeclaration`) keeps extension-type
/// representation formals out; a nameless formal has no element
/// upstream (`declaredFragment?.element` null) and never checks.
fn check_primary(c: &mut Check<'_>) {
    let node = c.node();
    let Some(owner) = c.ancestor(1) else {
        return;
    };
    if !matches!(
        c.kind(owner),
        NodeKind::ClassDeclaration | NodeKind::EnumDeclaration
    ) {
        return;
    }
    let ancestors = same_file_ancestors(c, owner, false);
    for param in formal_parameter_rows(c, node) {
        // The formal's payload word 1 is its name token.
        let Some(&name_token) = c.payload(param).get(1) else {
            continue;
        };
        if name_token == crate::rules::NONE_TOKEN {
            continue;
        }
        let Some(keyword) = field_keyword(c, param, name_token) else {
            continue;
        };
        if !matches!(c.lexeme(keyword), "final" | "var") {
            continue;
        }
        // `element.metadata.hasOverride` (:42): the formal's
        // `MetadataStar` left sibling (formal_params.rs opens the
        // metadata before the formal begins).
        if c.unit
            .ast
            .parent(param)
            .is_some_and(|container| has_override_annotation(c, container, param))
        {
            continue;
        }
        let name = c.lexeme(name_token);
        let overridden = ancestors.iter().any(|&ancestor| {
            inherited_member(c, ancestor, MemberQuery::Getter, name).is_some()
        }) || bridge_overrides(c, owner, MemberQuery::Getter, name)
            || core_object_declares(MemberQuery::Getter, name);
        if overridden {
            let range = crate::rules::utf16_token_span(c, name_token);
            c.report(range, &[name]);
        }
    }
}

/// The cross-file arm of the overridden-member lookup: each written
/// supertype anchors the semantic bridge's inheritance-chain walk
/// (extends/with, transitively, cross-file) answering
/// `getInheritedMember` where the same-file chain runs out. Unresolved
/// chains degrade to "not inherited"; the lookup is name-keyed, so
/// setters query their `name=` model spelling.
fn bridge_overrides(c: &Check<'_>, class_row: u32, query: MemberQuery, name: &str) -> bool {
    let spelled;
    let member = match query {
        MemberQuery::Setter => {
            spelled = format!("{name}=");
            spelled.as_str()
        }
        _ => name,
    };
    super::bridge_inherited_member(c, class_row, false, member).is_some()
}

/// The dart:core `Object` instance members at the name level — the
/// implicit root of every class chain, which the official
/// `getInheritedMember` resolves through even with no written supertype
/// (`class C { bool operator ==(Object? other) => ...; }` inherits
/// `Object.==`). The semantic bridge anchors written supertypes only
/// (the `supertype_anchors` walk), so the root face is a static table
/// here: the methods `==`/`toString`/`noSuchMethod` and the getters
/// `hashCode`/`runtimeType` (the `sdk/lib/core/object.dart` names the
/// engine's `OBJECT_MEMBERS` table carries).
fn core_object_declares(query: MemberQuery, name: &str) -> bool {
    match query {
        MemberQuery::Method => matches!(name, "==" | "toString" | "noSuchMethod"),
        MemberQuery::Getter => matches!(name, "hashCode" | "runtimeType"),
        // `Object` declares no setters and no variable-backed fields.
        MemberQuery::Setter | MemberQuery::Variable => false,
    }
}

/// The `constFinalOrVarKeyword` of a primary formal (:77): the last
/// `const`/`final`/`var` keyword written before the name — `None` for
/// a keywordless formal (`int x`).
fn field_keyword(c: &Check<'_>, param: u32, name_token: u32) -> Option<u32> {
    let mut keyword = None;
    for token in c.unit.ast.token_range(param) {
        if token >= name_token {
            break;
        }
        if matches!(c.lexeme(token), "const" | "final" | "var") {
            keyword = Some(token);
        }
    }
    keyword
}

/// The first member of `ancestor`'s body matching `query` by `name`.
fn inherited_member(
    c: &Check<'_>,
    ancestor: u32,
    query: MemberQuery,
    name: &str,
) -> Option<u32> {
    crate::rules::body_members(c, ancestor)
        .into_iter()
        .find(|&member| {
            if !member_matches(c, member, query, name).is_empty() {
                return true;
            }
            // The production `OperatorName` row is the method's left
            // sibling, not its child — `member_matches` only scans the
            // child window, so operators need the sibling read.
            query == MemberQuery::Method
                && c.kind(member) == NodeKind::Method
                && super::get_set_token(c, member).is_none()
                && super::operator_name(c, member).is_some_and(|op_row| {
                    c.payload(op_row)
                        .first()
                        .is_some_and(|&token| c.lexeme(token) == name)
                })
        })
}

/// The instance-member gates shared with the other override rules:
/// fields by their kind payload, methods by the header `ModSet` bit
/// (bridge `mod_bits` — static is bit 3), extension-type members out of
/// face (:58, :69).
fn is_instance_member(c: &Check<'_>, node: u32) -> bool {
    const STATIC_BIT: u16 = 1 << 3;
    match c.kind(node) {
        NodeKind::Fields => {
            c.payload(node).first() == Some(&(dartq_parser::MemberKind::NonStaticField as u32))
                && !in_extension_type(c)
        }
        NodeKind::Method => {
            dartq_ast::mod_flags(c.unit.ast, node) & STATIC_BIT == 0 && !in_extension_type(c)
        }
        _ => false,
    }
}

fn in_extension_type(c: &Check<'_>) -> bool {
    c.path
        .iter()
        .any(|&a| c.kind(a) == NodeKind::ExtensionTypeDeclaration)
}

/// The nearest enclosing class-like row (extension types excluded —
/// their members never report).
fn class_row_of<'a>(c: &'a Check<'a>) -> Option<&'a u32> {
    c.path.iter().rev().find(|&&a| {
        matches!(
            c.kind(a),
            NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::EnumDeclaration
                | NodeKind::ExtensionDeclaration
        )
    })
}

/// The checked name rows: the method's name (`Identifier` in the
/// method context, or the operator name — the `OperatorName` row tiles
/// as the method's left sibling in the body container), a field
/// group's every declared variable (:60-62 — the
/// `InitializedIdentifier` rows, whose span head is the name token).
fn member_names(c: &Check<'_>, node: u32) -> Vec<u32> {
    if c.kind(node) == NodeKind::Method {
        let mut names: Vec<u32> = c
            .collect_direct(node)
            .into_iter()
            .filter(|&kid| {
                c.kind(kid) == NodeKind::Identifier
                    && c.payload(kid).first()
                        == Some(&(dartq_parser::IdentifierContext::MethodDeclaration as u32))
            })
            .collect();
        if let Some(op_row) = super::adjacent_operator_name(c, node) {
            names.push(op_row);
        }
        names
    } else {
        let mut names = Vec::new();
        for decl in c.collect_direct(node) {
            if c.kind(decl) != NodeKind::VariablesDeclaration {
                continue;
            }
            for var in c.collect_direct(decl) {
                if c.kind(var) == NodeKind::InitializedIdentifier
                    && crate::rules::initialized_identifier_name(c, var).is_some()
                {
                    names.push(var);
                }
            }
        }
        names
    }
}
