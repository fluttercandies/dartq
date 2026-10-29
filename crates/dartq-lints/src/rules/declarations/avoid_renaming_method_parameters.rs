use super::{Check, NodeKind, PayloadField, RuleSpec};
use crate::rules::{
    MemberQuery, class_declaration_name, has_doc_comment, member_is_concrete, member_matches,
    same_file_ancestors, utf16_token_span,
};

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/avoid_renaming_method_parameters.dart`
/// — an overriding method whose positional parameter names differ from
/// the inherited concrete method's reports at each renamed parameter
/// token, with `(name, inherited name)` (:90-114).
///
/// Approximations (documented): the inherited concrete member
/// (`getInheritedConcreteMember`, :76-79) is the same-file
/// extends/with/on chain, concrete members only (an abstract
/// declaration never counts), plus the semantic bridge's
/// inheritance-chain arm for the cross-file residue
/// ([`dartq_semantic::Analysis::inherited_member`] with
/// `concrete_only` — the same extends/with walk cross-file; the
/// inherited formals come from the entry's captured parameter table).
/// The lib-directory gate (:37
/// `isInLibDir` — the file lives under the package's `lib/`) reads the
/// unit path's `lib` component, and the private-enclosing skip (:74
/// `parentElement.isPrivate`) reads the written class name. The skips
/// port: static methods (:56), methods with a documentation comment
/// (:57), getters/setters (no parameter list, :59-61); operators ride
/// the same `MethodDeclaration` face — their parameters are positional
/// and the inherited concrete member resolves through the `Object`
/// root. Named parameters (positional only, :90) stay out. Wildcard
/// `_` parameters skip on both sides (:97-106, feature-gated, the
/// shipped default). The augmentation branch (:82-88) stays unported
/// (augmentations are scanner-gated off).
/// Message: `diagnostic.g.dart` `avoidRenamingMethodParameters`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "avoid_renaming_method_parameters",
    &[NodeKind::Method],
    check,
);

fn check(c: &mut Check<'_>) {
    // Static skip (:56) — header ModSet bit (static is bit 3).
    const STATIC_BIT: u16 = 1 << 3;
    // `if (!context.isInLibDir) return` (:37).
    let in_lib = c
        .unit
        .path
        .is_some_and(|path| path.components().any(|part| part.as_os_str() == "lib"));
    if !in_lib {
        return;
    }
    let node = c.node();
    if dartq_ast::mod_flags(c.unit.ast, node) & STATIC_BIT != 0 {
        return;
    }
    // A documentation comment on the override skips (:57).
    if has_doc_comment(c, node) {
        return;
    }
    let Some(params) = c
        .collect_direct(node)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
    else {
        return; // getters/setters/operators have no parameters (:59-61)
    };
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
    // The private-enclosing skip (:74 `parentElement.isPrivate`).
    if class_declaration_name(c, class_row).is_some_and(|name| name.starts_with('_')) {
        return;
    }
    // The method's name: the `Identifier` row, else the operator face —
    // `operator ==` names through the adjacent `OperatorName` row's
    // payload token (the official reads `node.name.lexeme` over the
    // same `MethodDeclaration` face; operators carry parameters and are
    // checked like any method).
    let name_row = c.collect_direct(node).into_iter().find(|&kid| {
        c.kind(kid) == NodeKind::Identifier
            && c.field(kid, PayloadField::Context)
                == Some(dartq_parser::IdentifierContext::MethodDeclaration as u32)
    });
    let op_row = if name_row.is_none() {
        super::adjacent_operator_name(c, node)
    } else {
        None
    };
    let name_token = match (name_row, op_row) {
        (Some(name_row), _) => c.head_token(name_row),
        (_, Some(op_row)) => c
            .field(op_row, PayloadField::Name)
            .unwrap_or_else(|| c.head_token(op_row)),
        _ => return,
    };
    let name = c.lexeme(name_token);
    let inherited_names: Vec<String> = match same_file_ancestors(c, class_row, true)
        .into_iter()
        .find_map(|ancestor| {
            crate::rules::body_members(c, ancestor)
                .into_iter()
                .find(|&member| {
                    // The concreteness test reads the member container —
                    // `member_matches` answers name rows, not the member.
                    c.kind(member) == NodeKind::Method
                        && member_is_concrete(c, member)
                        && !member_matches(c, member, MemberQuery::Method, name).is_empty()
                })
        }) {
        Some(inherited) => inherited_positional_names(c, inherited)
            .into_iter()
            .map(str::to_owned)
            .collect(),
        None => bridge_positional_names(c, class_row, name)
            .unwrap_or_else(|| core_object_positional_names(name)),
    };
    if inherited_names.is_empty() {
        return;
    }
    let mine = positional_params(c, params);
    let count = mine.len().min(inherited_names.len());
    let mut renames: Vec<(u32, String, String)> = Vec::new();
    for i in 0..count {
        let (_, mine_token) = mine[i];
        let mine_name = c.lexeme(mine_token);
        let inherited_name = &inherited_names[i];
        if is_wildcard(c, mine_name) || is_wildcard(c, inherited_name) {
            continue;
        }
        if mine_name != inherited_name.as_str() {
            renames.push((mine_token, mine_name.to_owned(), inherited_name.clone()));
        }
    }
    for (token, mine_name, inherited_name) in renames {
        let range = utf16_token_span(c, token);
        c.report(range, &[mine_name.as_str(), inherited_name.as_str()]);
    }
}

/// The `_` wildcard face (:51-52, :97-106) — feature-gated, the
/// shipped default is on.
fn is_wildcard(c: &Check<'_>, name: &str) -> bool {
    c.unit.features.wildcard_variables && name == "_"
}

/// The `(receiver, name-token)` pairs of the positional parameters,
/// required group then `[]` group, in order (:90 — named parameters
/// stay out).
fn positional_params(c: &Check<'_>, params: u32) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    for section in c.collect_direct(params) {
        match c.kind(section) {
            NodeKind::FormalParameter => {
                if let Some(pair) = param_pair(c, section) {
                    out.push(pair);
                }
            }
            NodeKind::OptionalFormalParameters => {
                // The `[]` group is positional; the `{}` group is named.
                if c.lexeme(c.head_token(section)) != "[" {
                    continue;
                }
                for param in c.collect_direct(section) {
                    if c.kind(param) == NodeKind::FormalParameter
                        && let Some(pair) = param_pair(c, param)
                    {
                        out.push(pair);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// A formal's `(receiver, name)` tokens (receiver formals keep their
/// written name — `this.x` names `x`).
fn param_pair(c: &Check<'_>, param: u32) -> Option<(u32, u32)> {
    let name = c.field(param, PayloadField::Name)?;
    (name != crate::rules::NONE_TOKEN).then_some((0, name))
}

/// The inherited method's positional parameter names, in order (the
/// member row is the lookup's answer — no second body scan).
fn inherited_positional_names<'a>(c: &'a Check<'_>, member: u32) -> Vec<&'a str> {
    let Some(params) = c
        .collect_direct(member)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
    else {
        return Vec::new();
    };
    positional_params(c, params)
        .into_iter()
        .map(|(_, token)| c.lexeme(token))
        .collect()
}

/// The cross-file arm of the inherited concrete member's lookup
/// (`getInheritedConcreteMember` — each written extends/with/on
/// supertype anchors the bridge's concrete walk): the captured
/// positional formals of the hit, in order. `None` when the bridge is
/// unmounted or no chain resolves; a hit without a parameter table (a
/// field/getter face) has no positional names.
fn bridge_positional_names(c: &Check<'_>, class_row: u32, name: &str) -> Option<Vec<String>> {
    let analysis = c.semantics()?;
    let (_, entry) = super::bridge_inherited_member(c, class_row, true, name)?;
    let formals = entry.formals.as_ref()?;
    Some(
        formals
            .positional
            .iter()
            .map(|formal| analysis.strings().resolve(formal.name).to_owned())
            .collect(),
    )
}

/// The dart:core `Object` root of the inherited-concrete lookup — the
/// implicit superclass every class chain ends at, which the official
/// `getInheritedConcreteMember` resolves through even with no written
/// supertype (`operator ==(Object o)` inherits `Object.operator ==
/// (Object other)`). The bridge anchors written supertypes only, so
/// the root face is a static table: the parameterless `toString()`
/// answers empty (no names to mismatch), `==(Object other)` and
/// `noSuchMethod(Invocation invocation)` name their one positional
/// formal (`sdk/lib/core/object.dart`).
fn core_object_positional_names(name: &str) -> Vec<String> {
    match name {
        "==" => vec!["other".to_owned()],
        "noSuchMethod" => vec!["invocation".to_owned()],
        _ => Vec::new(),
    }
}
