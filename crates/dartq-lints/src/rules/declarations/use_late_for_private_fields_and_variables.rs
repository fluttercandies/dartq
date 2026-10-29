use super::{
    Check, NodeKind, PayloadField, RuleSpec, container_members, ctor_header_has_const,
    fields_written_type, is_identifier_token, range_has_const,
};
use crate::rules::{initialized_identifier_name, utf16_token_span};

const STATIC_FIELD: u32 = dartq_parser::MemberKind::StaticField as u32;

/// Official: `pkg/linter/lib/src/rules/use_late_for_private_fields_and_variables.dart`
/// (experimental, :22-259) — a private nullable-typed field or
/// top-level variable (:130-152/:216-223) without `late` and without
/// an initializer (:225-234), never read as nullable and never
/// assigned a possibly-null value (:78-94/:238-258), reports at the
/// variable name (:71). A class with a const constructor skips whole
/// (:97-113); enums skip (:125-127); an extension type's instance
/// fields skip (:132); a private (or unnamed) extension visits all of
/// its variables (:134-137).
///
/// Approximation (documented): the access walk is the unit's token
/// scan — an occurrence survives only as a `!` postfix or a plain `=`
/// assignment of an obviously non-null right side; every other
/// occurrence (a read, `?.`, a compound assignment, a constructor
/// initializer, a field formal) marks the member accessed and skips
/// the report, so field formals (`this._i`) are covered by the same
/// scan. Primary constructors (:171-199) are out of the substrate —
/// their private parameters never become lateables here. The
/// const-constructor skip reads the member headers and the class
/// header's `const` face. Message: `diagnostic.g.dart`
/// `useLateForPrivateFieldsAndVariables`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "use_late_for_private_fields_and_variables",
    &[NodeKind::Fields, NodeKind::TopLevelFields],
    check,
);

#[expect(
    clippy::too_many_lines,
    reason = "one flat container/member ladder; splitting it would                   scatter the official line citations"
)]
fn check(c: &mut Check<'_>) {
    let node = c.node();
    // The container faces (:97-152): an enum skips whole; a class
    // with a const constructor skips whole; an extension type's
    // instance fields skip; a private or unnamed extension visits
    // all of its variables.
    let mut extension_private = false;
    if c.kind(node) != NodeKind::TopLevelFields {
        let container = c.path.iter().rev().find(|&&row| {
            matches!(
                c.kind(row),
                NodeKind::ClassDeclaration
                    | NodeKind::EnumDeclaration
                    | NodeKind::ExtensionDeclaration
                    | NodeKind::ExtensionTypeDeclaration
            )
        });
        match container {
            Some(&row) => match c.kind(row) {
                NodeKind::EnumDeclaration => return,
                NodeKind::ExtensionTypeDeclaration => {
                    if c.field(node, PayloadField::MemberKind) != Some(STATIC_FIELD) {
                        return;
                    }
                }
                NodeKind::ExtensionDeclaration => {
                    extension_private = extension_is_private(c, row);
                }
                _ => {
                    if has_const_constructor(c, row) {
                        return;
                    }
                }
            },
            None => return,
        }
    }
    // The candidates: private-named (or every variable in a private
    // extension), without `late`, without an initializer, and
    // nullable-typed (no written type, `dynamic`, or a `?` tail).
    let mut candidates: Vec<(&str, u32)> = Vec::new();
    let mut decl_ranges: Vec<core::ops::Range<u32>> = Vec::new();
    for member in c.collect_direct(node) {
        if c.kind(member) != NodeKind::VariablesDeclaration {
            continue;
        }
        // `variable.isLate` (:226) — the row's `LateKeyword` word.
        if c.field(member, PayloadField::LateKeyword)
            .unwrap_or(crate::rules::NONE_TOKEN)
            != crate::rules::NONE_TOKEN
        {
            continue;
        }
        // `typeSystem.isNonNullable(declaredElement.type)` (:229-231)
        // — the written spelling face.
        let nullable = match fields_written_type(c, member) {
            None => true,
            Some((start, end)) => c.lexeme(start) == "dynamic" || c.lexeme(end - 1) == "?",
        };
        if !nullable {
            continue;
        }
        let decl_range = c.unit.ast.token_range(member);
        for var in c.collect_direct(member) {
            if c.kind(var) != NodeKind::InitializedIdentifier {
                continue;
            }
            // An initializer writes the member — the unused shapes
            // under-report.
            if c.collect_direct(var)
                .into_iter()
                .any(|kid| c.kind(kid) == NodeKind::VariableInitializer)
            {
                continue;
            }
            let Some(name_token) = initialized_identifier_name(c, var) else {
                continue;
            };
            let name = c.lexeme(name_token);
            if !extension_private && !name.starts_with('_') {
                continue;
            }
            candidates.push((name, name_token));
            decl_ranges.push(decl_range.clone());
        }
    }
    if candidates.is_empty() {
        return;
    }
    // The access walk (:78-94/:238-258) — the unit's token scan.
    let total = c
        .unit
        .ast
        .root()
        .map_or(0, |root| c.unit.ast.token_range(root).end);
    'candidate: for (index, (name, name_token)) in candidates.iter().enumerate() {
        for token in 0..total {
            if !is_identifier_token(c, token) || c.lexeme(token) != *name {
                continue;
            }
            if decl_ranges[index].contains(&token) {
                continue;
            }
            let accessed_by_dot = token > 0 && c.lexeme(token - 1) == ".";
            let next = if token + 1 < total {
                c.lexeme(token + 1)
            } else {
                ""
            };
            if next == "!" {
                continue; // (:251-253) — non-null access.
            }
            if !accessed_by_dot
                && next == "="
                && token + 2 < total
                && assigns_non_null(c, token + 2)
            {
                continue; // (:85-88) — a provably non-null assignment.
            }
            // Any other occurrence marks the member accessed (:89-90).
            continue 'candidate;
        }
        c.report(utf16_token_span(c, *name_token), &[]);
    }
}

/// Whether the token at `rhs` starts an obviously non-null expression
/// (the `isNonNullable(rhsType)` face): a numeric, string or bool
/// literal, a collection literal, `const`/`new`, or a creation-headed
/// identifier.
fn assigns_non_null(c: &Check<'_>, rhs: u32) -> bool {
    match c.unit.tokens.get(rhs as usize).map(|token| token.kind) {
        Some(
            dartq_scanner::TokenKind::Int
            | dartq_scanner::TokenKind::Double
            | dartq_scanner::TokenKind::Hexadecimal
            | dartq_scanner::TokenKind::String,
        ) => true,
        _ => match c.lexeme(rhs) {
            "true" | "false" | "const" | "new" | "[" | "{" => true,
            word if is_identifier_token(c, rhs) => word
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_uppercase()),
            _ => false,
        },
    }
}

/// Whether the extension is name-private or unnamed
/// (`parent.declaredFragment!.element.isPrivate`, :134-137).
fn extension_is_private(c: &Check<'_>, extension: u32) -> bool {
    c.collect_direct(extension)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Identifier)
        .is_none_or(|name| c.lexeme(c.head_token(name)).starts_with('_'))
}

/// Whether any constructor of the class writes `const` (:99-105 —
/// `late_final_field_with_const_constructor`, so the class skips
/// whole), including the class header's primary-const spelling
/// (`class const C(...)`).
fn has_const_constructor(c: &Check<'_>, class_row: u32) -> bool {
    if container_members(c, class_row)
        .1
        .into_iter()
        .filter(|&member| c.kind(member) == NodeKind::Constructor)
        // The Constructor row anchors at the declared name — `const`
        // lands before the span head, so the header face walks back.
        .any(|ctor| ctor_header_has_const(c, ctor))
    {
        return true;
    }
    c.collect_direct(class_row)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Identifier)
        .map(|name| c.head_token(name))
        .is_some_and(|name_head| range_has_const(c, c.head_token(class_row)..name_head))
}
