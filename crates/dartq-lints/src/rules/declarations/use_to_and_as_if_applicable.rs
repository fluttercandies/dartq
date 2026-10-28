use super::{Check, NodeKind, RuleSpec, balanced_angle, constructor_name_part, container_members, is_identifier_token, member_declaration_name, name_row_token, override_annotation, parameter_rows, sole_statement_tokens};
use crate::rules::{get_set_token, is_upper_camel_case, same_file_ancestors, same_file_declaration, type_before, utf16_token_span};

/// Official: `pkg/linter/lib/src/rules/use_to_and_as_if_applicable.dart:45-78`
/// — a method (:45) that is not a getter (:49), takes no parameters
/// (:50-51), returns non-`void` (:52), does not start with
/// `to`/`as`/`_to`/`_as` + uppercase (:20-23/:53), overrides nothing
/// (:54, `hasInheritedMethod`), and whose body is exactly
/// `return Foo(this);` / `=> Foo(this);` (:60-77, an instance
/// creation of the single `this` argument) reports at the method name
/// (:56).
///
/// Approximation (documented): the `InstanceCreationExpression` face
/// folds lexically — an explicit `new`/`const` always creates; a bare
/// reference creates when its head is a class-name spelling (upper
/// camel), `prefix.Foo(this)` when the second segment is upper camel,
/// and `Foo.named(this)` only when `Foo` is a same-file class naming a
/// `named` constructor (a static-method call of the same shape
/// under-reports); a cross-file inherited method under the override
/// face over-reports only for an un-annotated override of a
/// cross-file member. Message: `diagnostic.g.dart`
/// `useToAndAsIfApplicable`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "use_to_and_as_if_applicable",
    &[NodeKind::Method],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `!node.isGetter` (:49); a setter is excluded by the empty
    // parameter face.
    if get_set_token(c, node) == Some("get") {
        return;
    }
    // The name is the `MethodDeclaration`-context identifier — a
    // written return type's `Identifier` (`A` in `A wrap()`) is a
    // `TypeReference` sibling, not the name.
    let Some(name_row) = member_declaration_name(c, node) else {
        return;
    };
    let name = c.lexeme(c.head_token(name_row));
    // `nodeParameters.parameters.isEmpty` (:50-51).
    let Some(params_row) = c
        .collect_direct(node)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
    else {
        return;
    };
    if !parameter_rows(c, params_row).is_empty() {
        return;
    }
    // `!_isVoid(node.returnType)` (:52) — the written `void` skips; an
    // absent return type is not void and qualifies.
    if matches!(
        type_before(c, node, name_row),
        Some((_, NodeKind::VoidType))
    ) {
        return;
    }
    // `_beginsWithAsOrTo(node.name.lexeme)` (:53).
    if begins_with_as_or_to(name) {
        return;
    }
    // `!node.hasInheritedMethod` (:54).
    if override_annotation(c, node) || ancestor_method(c, name) {
        return;
    }
    // `_checkBody` (:60-77): `=> Foo(this)` / `{ return Foo(this); }`.
    let Some(statement) = sole_statement_tokens(c, node) else {
        return;
    };
    if !returns_this_creation(c, statement) {
        return;
    }
    c.report(utf16_token_span(c, name_row_token(c, name_row)), &[]);
}

/// `_beginsWithAsOrTo` (:20-23): the `(to|as|_to|_as)[A-Z]` prefix.
fn begins_with_as_or_to(name: &str) -> bool {
    let rest = name
        .strip_prefix("_to")
        .or_else(|| name.strip_prefix("_as"))
        .or_else(|| name.strip_prefix("to"))
        .or_else(|| name.strip_prefix("as"));
    rest.is_some_and(|rest| {
        rest.chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_uppercase())
    })
}

/// Whether a same-file ancestor declares a method of `name`
/// (`hasInheritedMethod`, extensions.dart:690).
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
        container_members(c, a).1.iter().any(|&m| {
            !crate::rules::member_matches(c, m, crate::rules::MemberQuery::Method, name)
                .is_empty()
        })
    })
}

/// `_checkExpression` (:72-77): the statement's expression — `return`
/// head and outer parens stripped — is an instance creation of exactly
/// one `this` argument.
fn returns_this_creation(c: &Check<'_>, statement: core::ops::Range<u32>) -> bool {
    let mut tokens = statement;
    while !tokens.is_empty() {
        match c.lexeme(tokens.end - 1) {
            ";" => tokens.end -= 1,
            _ => break,
        }
    }
    if !tokens.is_empty() && c.lexeme(tokens.start) == "return" {
        tokens.start += 1;
    }
    // `unParenthesized` (:73).
    while tokens.end - tokens.start >= 2
        && c.lexeme(tokens.start) == "("
        && c.lexeme(tokens.end - 1) == ")"
    {
        tokens.start += 1;
        tokens.end -= 1;
    }
    if tokens.is_empty() {
        return false;
    }
    // The constructor reference: `Id (. Id)* [<…>]?` then `( this )`.
    let explicit = matches!(c.lexeme(tokens.start), "new" | "const");
    let mut cursor = if explicit {
        tokens.start + 1
    } else {
        tokens.start
    };
    if !is_identifier_token(c, cursor) {
        return false;
    }
    let mut segments = vec![cursor];
    cursor += 1;
    while c.lexeme(cursor) == "." {
        if !is_identifier_token(c, cursor + 1) {
            return false;
        }
        segments.push(cursor + 1);
        cursor += 2;
    }
    if c.lexeme(cursor) == "<" {
        let Some(close) = balanced_angle(c, cursor, tokens.end) else {
            return false;
        };
        cursor = close + 1;
    }
    if tokens.end != cursor + 3
        || c.lexeme(cursor) != "("
        || c.lexeme(cursor + 1) != "this"
        || c.lexeme(cursor + 2) != ")"
    {
        return false;
    }
    if explicit {
        return true;
    }
    // The creation classification (see the doc).
    match segments.len() {
        1 => is_upper_camel_case(c.lexeme(segments[0])),
        2 if is_upper_camel_case(c.lexeme(segments[1]))
            && !is_upper_camel_case(c.lexeme(segments[0])) =>
        {
            true // `prefix.Foo(this)` — an import-prefixed class
        }
        2 => named_constructor(c, c.lexeme(segments[0]), c.lexeme(segments[1])),
        _ => false,
    }
}

/// Whether `name` names a constructor of the same-file class `class_
/// name` (the `Foo.named(this)` creation/call disambiguator).
fn named_constructor(c: &Check<'_>, class_name: &str, name: &str) -> bool {
    let Some(row) = same_file_declaration(c, class_name) else {
        return false;
    };
    if c.kind(row) != NodeKind::ClassDeclaration {
        return false;
    }
    container_members(c, row).1.iter().any(|&member| {
        matches!(c.kind(member), NodeKind::Constructor | NodeKind::Factory)
            && constructor_name_part(c, member)
                .is_some_and(|part| c.lexeme(c.head_token(part)) == name)
    })
}
