use super::{Check, NONE_TOKEN, NodeKind, PayloadField, RuleSpec, VAR_FINAL_OR_CONST_MEMBER_BIT};

/// Official: `pkg/linter/lib/src/rules/var_with_no_type_annotation.dart`
/// — a parameter whose storage keyword is `var` with no type reports at
/// the keyword (:37-66). Registration is feature-gated INVERTED: the
/// rule registers only WITHOUT primary constructors (:34-36, the
/// declaring-parameter form supersedes it) — pinned on
/// [`crate::engine::Features::primary_constructors`].
///
/// dartq faces: super formals and function-typed parameters skip (the
/// compile-time-error families, :50/:57 — the receiver word's lexeme
/// and the function-typed child answer); field formals check like
/// plain parameters (:59-63). The keyword is the `VAR_FINAL_OR_CONST`
/// `ModSet` bit recovered by a bounded forward scan past
/// `required`/`covariant`; the written type is a
/// Type/FunctionType/RecordType child (`NoType` = unwritten). Message: `diagnostic.g.dart` `varWithNoTypeAnnotation`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "var_with_no_type_annotation",
    &[
        NodeKind::FormalParameters,
        NodeKind::OptionalFormalParameters,
    ],
    check,
);

fn check(c: &mut Check<'_>) {
    if c.unit.features.primary_constructors {
        return; // inverted registration bail (:34-36)
    }
    let node = c.node();
    for param in c.collect_direct(node) {
        if c.kind(param) != NodeKind::FormalParameter {
            continue;
        }
        // A function-typed suffix skips (:50 — RegularFormalParameter's
        // `functionTypedSuffix == null` gate).
        if c.collect_direct(param)
            .iter()
            .any(|&kid| c.kind(kid) == NodeKind::FunctionTypedFormalParameter)
        {
            continue;
        }
        // The `case _` skip (:57): super formals are a
        // compile-time-error family; the receiver word's lexeme
        // decides (field formals carry `this`).
        let receiver = c.field(param, PayloadField::Receiver).unwrap_or(NONE_TOKEN);
        if receiver != NONE_TOKEN && c.lexeme(receiver) == "super" {
            continue;
        }
        let Some(keyword) = storage_keyword(c, param).filter(|kw| c.lexeme(*kw) == "var") else {
            continue;
        };
        if c.collect_direct(param).into_iter().any(|kid| {
            matches!(
                c.kind(kid),
                NodeKind::Type | NodeKind::FunctionType | NodeKind::RecordType
            )
        }) {
            continue; // `type == null` fails
        }
        c.report(crate::rules::utf16_token_span(c, keyword), &[]);
    }
}

/// The `var`/`final`/`const` keyword token of a `VAR_FINAL_OR_CONST`-
/// marked parameter (bounded: `required`/`covariant` precede it).
fn storage_keyword(c: &Check<'_>, param: u32) -> Option<u32> {
    if dartq_ast::mod_flags(c.unit.ast, param) & VAR_FINAL_OR_CONST_MEMBER_BIT == 0 {
        return None;
    }
    let mut token = c.head_token(param);
    for _ in 0..3 {
        match c.lexeme(token) {
            "var" | "final" | "const" => return Some(token),
            "required" | "covariant" => token += 1,
            _ => return None,
        }
    }
    None
}
