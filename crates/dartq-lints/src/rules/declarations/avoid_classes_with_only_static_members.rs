use super::{Check, NodeKind, PayloadField, RuleSpec, body_members, get_set_token};

/// Official: `pkg/linter/lib/src/rules/avoid_classes_with_only_static_members.dart:30-71`
/// — a class whose members are all static (and whose fields are not all
/// const) reports at the class name. Skips: sealed classes (:40-41),
/// classes inheriting instance members from any non-`Object` class
/// (:43-53 `interfaceMembers`), classes with a non-default constructor
/// (:57-61, default = unnamed without required parameters,
/// `element.dart:1237-1247`), and classes with any instance method
/// (:63-64 — subsumed by the interface-member face). The report fires
/// when a static method exists or a non-const field exists (:66-67).
///
/// Approximation (documented): the inherited-instance-member face walks
/// the same-file ancestor chain (all clauses — the official
/// `interfaceMembers` includes `implements`) plus the semantic bridge's
/// three-valued supertype walk for the cross-file residue
/// ([`dartq_semantic::Analysis::supertype_has_instance_members`]):
/// `Some(true)` silences (instance members inherited cross-file),
/// `Some(false)` lets the report proceed (every written supertype is
/// indexed and instance-free), and `None` (an unresolvable chain — an
/// SDK type, an unanalyzed dependency, whose member face could carry
/// instance members) conservatively silences. Cross-file
/// `implements`/`on` edges stay outside the bridge's name-level chain
/// and under-report.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "avoid_classes_with_only_static_members",
    &[NodeKind::ClassDeclaration],
    check,
);

/// The `static` (member) / `sealed` (class) `ModSet` bit (bridge
/// `mod_bits`; `same_file::STATIC_BIT`).
const STATIC_OR_SEALED_BIT: u16 = 1 << 3;

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // Sealed classes skip (:40-41).
    if dartq_ast::mod_flags(c.unit.ast, node) & STATIC_OR_SEALED_BIT != 0 {
        return;
    }
    // Own instance members (:43-53) — the official `interfaceMembers`
    // face includes the class's own members.
    if has_instance_members(c, node) {
        return;
    }
    // Inherited instance members: the same-file ancestor chain over all
    // clauses, plus the bridge's three-valued cross-file walk.
    if crate::rules::same_file_ancestors(c, node, false)
        .iter()
        .any(|&ancestor| has_instance_members(c, ancestor))
        || bridge_supertype_has_instance_members(c, node)
    {
        return;
    }
    // Non-default constructors skip (:57-61).
    if body_members(c, node).iter().any(|&member| {
        matches!(c.kind(member), NodeKind::Constructor | NodeKind::Factory)
            && is_non_default_constructor(c, member)
    }) {
        return;
    }
    let members = body_members(c, node);
    // `methods.isNotEmpty` (:66): static methods proper — accessors are
    // the official `getters`/`setters` lists, not `methods`.
    let has_static_method = members.iter().any(|&member| {
        c.kind(member) == NodeKind::Method
            && dartq_ast::mod_flags(c.unit.ast, member) & STATIC_OR_SEALED_BIT != 0
            && get_set_token(c, member).is_none()
    });
    // `fields.any((f) => !f.isConst)` (:66-67).
    let has_non_const_field = members.iter().any(|&member| {
        c.kind(member) == NodeKind::Fields
            && c.collect_direct(member).into_iter().any(|decl| {
                c.kind(decl) == NodeKind::VariablesDeclaration
                    && crate::rules::variables_declaration_keyword(c, decl)
                        .is_none_or(|kw| c.lexeme(kw) != "const")
            })
    });
    if has_static_method || has_non_const_field {
        // The report sits at the class name (`node.namePart.typeName`,
        // :67) — the production wrapper keeps the name `Identifier` as
        // the row's left sibling (`class_declaration_name`'s contract),
        // so the span resolves through the same two-step face.
        let Some(name_row) = class_name_row(c, node) else {
            return;
        };
        c.report(
            crate::rules::utf16_token_span(c, c.head_token(name_row)),
            &[],
        );
    }
}

/// The cross-file arm of the inherited-instance-member gate: the
/// bridge's three-valued supertype walk. `Some(true)` silences
/// (instance members inherited cross-file); `None` (an unresolvable
/// chain — an SDK type, an unanalyzed dependency) conservatively
/// silences; only `Some(false)` — every written supertype indexed and
/// instance-free — lets the report proceed. Without the bridge mounted
/// the same-file arm alone decides (the plain-run posture).
fn bridge_supertype_has_instance_members(c: &Check<'_>, class_row: u32) -> bool {
    let Some(analysis) = c.semantics() else {
        return false;
    };
    let Some(class_name) = crate::rules::class_declaration_name(c, class_row) else {
        return false;
    };
    analysis.supertype_has_instance_members(analysis.strings().intern(class_name)) != Some(false)
}

/// The class-like declaration's name row: the direct child with a
/// class-name identifier context, else the row's left sibling (the
/// production wrapper — `class_declaration_name`'s two-step face,
/// kept row-bearing for the report span).
fn class_name_row(c: &Check<'_>, class_row: u32) -> Option<u32> {
    let is_class_name = |kid: u32| {
        c.kind(kid) == NodeKind::Identifier
            && matches!(
                c.field(kid, PayloadField::Context),
                Some(ctx)
                    if ctx
                        == dartq_parser::IdentifierContext::ClassOrMixinOrExtensionDeclaration
                            as u32
                        || ctx == dartq_parser::IdentifierContext::EnumDeclaration as u32
            )
    };
    if let Some(name) = c
        .collect_direct(class_row)
        .into_iter()
        .find(|&kid| is_class_name(kid))
    {
        return Some(name);
    }
    let parent = c.unit.ast.parent(class_row)?;
    let siblings = c.collect_direct(parent);
    let pos = siblings.iter().position(|&kid| kid == class_row)?;
    siblings[..pos]
        .iter()
        .rev()
        .copied()
        .find(|&kid| is_class_name(kid))
}

/// Whether any member row of the class-like declaration is an instance
/// member: a non-static `Fields` row or a non-static `Method` row.
fn has_instance_members(c: &Check<'_>, class_row: u32) -> bool {
    body_members(c, class_row)
        .iter()
        .any(|&member| match c.kind(member) {
            NodeKind::Fields => {
                c.field(member, PayloadField::MemberKind)
                    == Some(dartq_parser::MemberKind::NonStaticField as u32)
            }
            NodeKind::Method => {
                dartq_ast::mod_flags(c.unit.ast, member) & STATIC_OR_SEALED_BIT == 0
            }
            _ => false,
        })
}

/// Non-default = named, or carrying a required parameter (the official
/// `isDefaultConstructor`, `element.dart:1237-1247`). The row anchors
/// at the declared name (bridge `begin_constructor`), so the named
/// form is the `.suffix` layout after the head token (`F.named()`); a
/// nameless row (recovery) reads as non-default — the conservative
/// face.
fn is_non_default_constructor(c: &Check<'_>, ctor: u32) -> bool {
    let head = c.head_token(ctor);
    let dot = c
        .unit
        .tokens
        .get(head as usize + 1)
        .is_some_and(|_| c.lexeme(head + 1) == ".");
    if dot {
        return true;
    }
    c.collect_direct(ctor).into_iter().any(|kid| {
        matches!(
            c.kind(kid),
            NodeKind::FormalParameters | NodeKind::OptionalFormalParameters
        ) && c.collect_direct(kid).into_iter().any(|param| {
            c.kind(param) == NodeKind::FormalParameter
                && matches!(
                    c.field(param, PayloadField::FormalParameterKind),
                    Some(k)
                        if k == dartq_parser::FormalParameterKind::RequiredPositional as u32
                            || k == dartq_parser::FormalParameterKind::RequiredNamed as u32
                )
        })
    })
}
