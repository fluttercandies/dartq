use super::{Check, Group, NodeKind, RuleSpec};
use crate::rules::{MemberQuery, class_declaration_name, member_matches, same_file_ancestors};

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/overridden_fields.dart` — an
/// instance field whose inherited concrete member is a variable-backed
/// getter reports at the field name, with the defining interface's name
/// as the argument (:66-81).
///
/// Approximations (documented): `getInheritedConcreteMember` (:69-71)
/// is the same-file extends/with/on chain, concrete members only, plus
/// the semantic bridge's inheritance-chain arm for the cross-file
/// residue ([`dartq_semantic::Analysis::inherited_member`] with
/// `concrete_only`); the variable-backed getter test
/// (`isOriginVariable`, :72-73) is the ancestor's `Fields` rows
/// exclusively in the same-file arm (a declared getter never matches)
/// and the entry's `MemberKind::Field` in the bridge arm (a declared
/// getter captures as `Getter`). The defining interface (:74-78) is
/// the nearest ancestor class row declaring the field in the same-file
/// arm — the most-specific concrete declaration, as the official
/// lookup resolves — and the walk's defining-container name in the
/// bridge arm. Static fields skip (:40) and augmentations are
/// scanner-gated off. The primary-constructor parameter face (:51-64)
/// rides the same chain: every `var`/`final` formal of a primary
/// constructor declares a field (the `element is
/// FieldFormalParameterElement` read over the written keyword; a
/// `this.x` receiver formal is the AST `FieldFormalParameter` shape
/// and skips, and extension-type representation formals take no
/// keyword) and checks like a field, reporting at its name token.
/// Message: `diagnostic.g.dart` `overriddenFields`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "overridden_fields",
    group: Group::Recommended,
    problem: "Field overrides a field inherited from '{0}'.",
    correction: "Try removing the field, overriding the getter and setter if necessary.",
    kinds: &[NodeKind::Fields, NodeKind::PrimaryConstructor],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    if c.kind(node) == NodeKind::PrimaryConstructor {
        check_primary(c);
        return;
    }
    // `node.isStatic` (:40).
    if c.payload(node).first() != Some(&(dartq_parser::MemberKind::NonStaticField as u32)) {
        return;
    }
    let Some(&class_row) = c.path.iter().rev().find(|&&a| {
        matches!(
            c.kind(a),
            NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::EnumDeclaration
                | NodeKind::ExtensionDeclaration
        )
    }) else {
        return;
    };
    let chain = same_file_ancestors(c, class_row, true);
    let fields: Vec<(&str, u32)> = field_names(c, node);
    // The nearest concrete variable-backed declaration in the chain
    // supplies the defining interface (:69-78); names collect owned —
    // the lookup borrows `c` immutably across the report.
    let mut hits: Vec<(u32, String)> = Vec::new();
    for (name, name_row) in fields {
        if let Some(defining) = inherited_variable_holder(c, &chain, name)
            .map(str::to_owned)
            .or_else(|| bridge_variable_holder(c, class_row, name))
        {
            hits.push((name_row, defining));
        }
    }
    for (name_row, defining) in hits {
        // The name row is the name token (`initialized_identifier_name`)
        // — the official `reportAtToken(nameToken)` anchor (:79).
        let range = crate::rules::utf16_token_span(c, name_row);
        c.report(range, &[defining.as_str()]);
    }
}

/// The primary-constructor parameter face (:51-64): each `var`/`final`
/// formal of a primary constructor declares a field and checks like
/// one — the inherited getter namespace, reporting at the name token.
/// A `this.x` receiver formal is the AST field-formal shape and skips
/// (:53); a nameless formal has no element upstream and never checks.
fn check_primary(c: &mut Check<'_>) {
    let node = c.node();
    let Some(owner) = c.ancestor(1) else {
        return;
    };
    let chain = same_file_ancestors(c, owner, true);
    let mut hits: Vec<(u32, String)> = Vec::new();
    for param in crate::rules::types::leak_formal_rows(c, node) {
        // `parameter is! FieldFormalParameter` (:53) — the `this.x`
        // receiver formal.
        let receiver = c.payload(param).first().copied();
        if receiver.is_some_and(|token| token != crate::rules::NONE_TOKEN) {
            continue;
        }
        let Some(&name_token) = c.payload(param).get(1) else {
            continue;
        };
        if name_token == crate::rules::NONE_TOKEN {
            continue;
        }
        // The declaring-field face: the `var`/`final` keyword (the
        // `element is FieldFormalParameterElement` read; `const`
        // declares no field).
        let Some(keyword) = primary_field_keyword(c, param, name_token) else {
            continue;
        };
        if !matches!(c.lexeme(keyword), "final" | "var") {
            continue;
        }
        if let Some(defining) = inherited_variable_holder(c, &chain, c.lexeme(name_token))
            .map(str::to_owned)
            .or_else(|| bridge_variable_holder(c, owner, c.lexeme(name_token)))
        {
            hits.push((name_token, defining));
        }
    }
    for (name_token, defining) in hits {
        let range = crate::rules::utf16_token_span(c, name_token);
        c.report(range, &[defining.as_str()]);
    }
}

/// The last `const`/`final`/`var` keyword written before the formal's
/// name (the primary formal's storage keyword — `None` for a
/// keywordless formal).
fn primary_field_keyword(c: &Check<'_>, param: u32, name_token: u32) -> Option<u32> {
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

/// The nearest ancestor in `chain` declaring a variable-backed field of
/// `name`, as its class-like name (the defining interface, :74-78).
fn inherited_variable_holder<'a>(
    c: &'a Check<'_>,
    chain: &[u32],
    name: &str,
) -> Option<&'a str> {
    chain.iter().find_map(|&ancestor| {
        crate::rules::body_members(c, ancestor)
            .into_iter()
            .find(|&member| {
                c.kind(member) == NodeKind::Fields
                    && !member_matches(c, member, MemberQuery::Variable, name).is_empty()
            })
            .and_then(|_| class_declaration_name(c, ancestor))
    })
}

/// The cross-file arm of the inherited concrete member's lookup
/// (`getInheritedConcreteMember` — each written extends/with/on
/// supertype anchors the bridge's concrete walk), gated to the
/// variable-backed face (`isOriginVariable`, :72-73 — a captured
/// `MemberKind::Field`; a declared getter captures as `Getter` and
/// never reports). The defining interface is the walk's
/// defining-container name; unresolved chains stay silent.
fn bridge_variable_holder(c: &Check<'_>, class_row: u32, name: &str) -> Option<String> {
    let (defining, entry) = super::bridge_inherited_member(c, class_row, true, name)?;
    (entry.kind == dartq_semantic::model::MemberKind::Field).then_some(defining)
}

/// The `(name lexeme, name row)` pairs of one `Fields` group.
fn field_names<'a>(c: &Check<'a>, node: u32) -> Vec<(&'a str, u32)> {
    let mut out = Vec::new();
    for decl in c.collect_direct(node) {
        if c.kind(decl) != NodeKind::VariablesDeclaration {
            continue;
        }
        for var in c.collect_direct(decl) {
            if c.kind(var) != NodeKind::InitializedIdentifier {
                continue;
            }
            if let Some(row) = crate::rules::initialized_identifier_name(c, var) {
                out.push((c.lexeme(row), row));
            }
        }
    }
    out
}
