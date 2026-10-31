use super::{
    Check, NodeKind, PayloadField, RuleSpec, annotation_names, constructor_name_part,
    container_members, ctor_header_has_const, is_identifier_token, range_has_const,
    top_level_annotations,
};
use crate::rules::const_ability::{is_potentially_const, potentially_const_name};
use crate::rules::{
    NONE_TOKEN, SupertypeClause, last_child, same_file_ancestors, same_file_declaration,
    supertypes, utf16_token_span, variables_declaration_keyword,
};

/// Official: `pkg/linter/lib/src/rules/prefer_const_constructors_in_immutables.dart:41-133`
/// — a non-const constructor (:43-67), redirecting factory or primary
/// constructor (:70-101) of an `@immutable` class-like with an empty
/// body and no mixins, whose invocation face can be const, reports at
/// the first token after comment and metadata (:58/:65).
///
/// Approximation (documented — the adjudicated annotation-spelling +
/// ctor-shape face, no const evaluation): the immutable annotation
/// reads the container and its same-file chain; the invocation face
/// (:103-133) folds to the token faces — an explicit `super`/`this`
/// invocation resolves its target constructor's `const` keyword over
/// the same-file substrate, a missing initializer list takes the
/// implicit-super face (no `extends` means `Object`, whose unnamed
/// constructor is const), and every `assert`/field-initializer entry
/// must be potentially constant with the constructor's parameters bound
/// (`initializer_entries_able`); `canBeConst` also folds to every
/// instance field of the class and its
/// same-file extends chain being spelled `final`/`const` (a field
/// initializer must be an obviously const literal; `late` bails). A
/// cross-file supertype under-reports. Extension types answer the
/// header-`const` face: a non-const primary constructor on an
/// `@immutable` extension type reports at the name (:82-85), a named
/// constructor needs the header `const` (:125-127). Class primary
/// constructors take the same shape as an empty-bodied constructor and
/// report at the container name (the `errorRange` face). Message:
/// `diagnostic.g.dart` `preferConstConstructorsInImmutables`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_const_constructors_in_immutables",
    &[
        NodeKind::Constructor,
        NodeKind::Factory,
        NodeKind::PrimaryConstructor,
    ],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `element.enclosingElement` (:48) — the owning class-like row.
    let Some(&container) = c.path.iter().rev().find(|&&row| {
        matches!(
            c.kind(row),
            NodeKind::ClassDeclaration | NodeKind::ExtensionTypeDeclaration
        )
    }) else {
        return;
    };
    match c.kind(node) {
        NodeKind::PrimaryConstructor => face_primary(c, container, node),
        NodeKind::Factory => face_factory(c, container, node),
        _ => face_constructor(c, container, node),
    }
}

/// The constructor face (:43-67).
fn face_constructor(c: &mut Check<'_>, container: u32, node: u32) {
    // `element.isConst` (:46) — the header `const`.
    if ctor_header_has_const(c, node) {
        return;
    }
    // `node.body is! EmptyFunctionBody` (:47).
    if !last_child(c, node).is_some_and(|body| c.kind(body) == NodeKind::EmptyBody) {
        return;
    }
    if !has_immutable(c, container) {
        return;
    }
    if c.kind(container) == NodeKind::ExtensionTypeDeclaration {
        // `_hasConstConstructorInvocation` (:125-127): the extension
        // type's primary constructor must be const — the header face.
        if !primary_header_const(c, container, node) {
            return;
        }
        c.report(utf16_token_span(c, c.head_token(node)), &[]);
        return;
    }
    if !const_able_class(c, container, node) {
        return;
    }
    // `node.firstTokenAfterCommentAndMetadata` (:58/:65) — the member's
    // first token: the modifiers (`const`, `factory`, the new-syntax
    // `new`) sit in the window before the row's name anchor.
    c.report(utf16_token_span(c, super::constructor_first_token(c, node)), &[]);
}

/// The redirecting-factory face (:53-58): a non-const factory whose
/// `= Target` redirect resolves (same-file) to a const constructor.
fn face_factory(c: &mut Check<'_>, container: u32, node: u32) {
    if ctor_header_has_const(c, node) {
        return;
    }
    if !has_immutable(c, container) {
        return;
    }
    let body = last_child(c, node);
    let Some(body) = body.filter(|&row| c.kind(row) == NodeKind::RedirectingFactoryBody) else {
        return;
    };
    let tokens = c.unit.ast.token_range(body);
    let mut cursor = tokens.start;
    while cursor < tokens.end && c.lexeme(cursor) != "=" {
        cursor += 1;
    }
    cursor += 1;
    let mut segments = Vec::new();
    while cursor < tokens.end && c.lexeme(cursor) != ";" {
        if is_identifier_token(c, cursor) {
            segments.push(c.lexeme(cursor));
        }
        cursor += 1;
    }
    let container_name = c
        .unit
        .ast
        .parent(container)
        .map(|wrapper| {
            c.collect_direct(wrapper)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::Identifier)
        })
        .flatten()
        .map(|row| c.lexeme(c.head_token(row)));
    // `C` / `C.named` / `prefix.C` — the prefixed and cross-file
    // shapes under-report.
    let target = match segments.len() {
        1 if segments[0] == container_name.unwrap_or("") => (container, None),
        1 => {
            // A redirect to another class (`= B`) — the same-file face.
            let Some(row) = same_file_declaration(c, segments[0])
                .filter(|&row| c.kind(row) == NodeKind::ClassDeclaration)
            else {
                return;
            };
            (row, None)
        }
        2 if segments[0] == container_name.unwrap_or("") => (container, Some(segments[1])),
        _ => return,
    };
    if const_ctor_named(c, target.0, target.1) {
        c.report(utf16_token_span(c, super::constructor_first_token(c, node)), &[]);
    }
}

/// The primary-constructor face (:70-101).
fn face_primary(c: &mut Check<'_>, container: u32, node: u32) {
    // `element.isConst` (:73) — the header spelling.
    if primary_header_const(c, container, node) {
        return;
    }
    // `node.body?.body is! EmptyFunctionBody` (:74-76) — a body with a
    // block skips.
    if primary_body_row(c, container).is_some_and(|body| {
        c.collect_direct(body)
            .into_iter()
            .any(|kid| matches!(c.kind(kid), NodeKind::Block | NodeKind::BlockBody))
    }) {
        return;
    }
    if !has_immutable(c, container) {
        return;
    }
    let name = container_name_token(c, container);
    if c.kind(container) == NodeKind::ExtensionTypeDeclaration {
        // (:82-85) — the representation face has no super constructor.
        if let Some(name) = name {
            c.report(utf16_token_span(c, name), &[]);
        }
        return;
    }
    if !const_able_class(c, container, node) {
        return;
    }
    // `node.errorRange` (:92/:99) — the container name.
    if let Some(name) = name {
        c.report(utf16_token_span(c, name), &[]);
    }
}

/// The primary constructor's `this …` body: a MEMBER row of the
/// container's declaration body (the header `C(...)` row seals at its
/// `)`; the body is written among the members and tiles there), holding
/// `[Initializers, EmptyBody]` for `this : …;` or a `BlockBody` for
/// `this { … }`.
fn primary_body_row(c: &Check<'_>, container: u32) -> Option<u32> {
    container_members(c, container)
        .1
        .into_iter()
        .find(|&member| c.kind(member) == NodeKind::PrimaryConstructorBody)
}

/// The container's name token — an extension type stores the `Name`
/// payload word (no identifier row tiles), a class-like keeps its name
/// row (with the `TopLevelDeclaration` wrapper's identifier as the
/// fallback tiling).
fn container_name_token(c: &Check<'_>, container: u32) -> Option<u32> {
    if c.kind(container) == NodeKind::ExtensionTypeDeclaration {
        return c.field_token(container, PayloadField::Name);
    }
    super::container_name_node(c, container)
        .map(|row| c.head_token(row))
        .or_else(|| {
            c.unit.ast.parent(container).and_then(|wrapper| {
                c.collect_direct(wrapper)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::Identifier)
                    .map(|row| c.head_token(row))
            })
        })
}

/// Whether the container's header or the primary constructor itself
/// writes `const` before the name (`class const C(...)`,
/// `extension type const E(...)`).
fn primary_header_const(c: &Check<'_>, container: u32, primary: u32) -> bool {
    if c.kind(primary) == NodeKind::PrimaryConstructor && ctor_header_has_const(c, primary) {
        return true;
    }
    container_name_token(c, container).is_some_and(|name_head| {
        range_has_const(c, c.head_token(container)..name_head)
    })
}

/// `hasImmutableAnnotation` (:136-147): the container's own
/// annotations plus the same-file chain walk. The spelling only counts
/// behind a `package:meta/meta.dart` import of the unit — an unresolved
/// `@immutable` (a unit that never imports meta) does not resolve to the
/// element and stays silent upstream.
fn has_immutable(c: &Check<'_>, container: u32) -> bool {
    if !imports_meta(c) {
        return false;
    }
    std::iter::once(container)
        .chain(same_file_ancestors(c, container, true))
        .any(|row| {
            top_level_annotations(c, row)
                .iter()
                .any(|&meta| {
                    // `metadata.hasImmutable` resolves the element —
                    // meta ships both spellings (`immutable` the const,
                    // `Immutable` the class).
                    annotation_names(c, meta, "immutable")
                        || annotation_names(c, meta, "Immutable")
                })
        })
}

/// Whether the unit imports `package:meta/meta.dart` (any prefix — the
/// prefixed-annotation face under-reports: the corpus annotation is bare).
/// The directive rows sit inside their `TopLevelDeclaration` wrapper
/// under the unit root (the shared [`unprefixed_import_uris`] walk minus
/// the prefix cut).
fn imports_meta(c: &Check<'_>) -> bool {
    let Some(root) = c.unit.ast.root() else {
        return false;
    };
    c.collect_direct(root).into_iter().any(|top| {
        std::iter::once(top)
            .chain(c.collect_direct(top))
            .any(|row| {
                c.kind(row) == NodeKind::Import
                    && c.collect_direct(row).into_iter().any(|piece| {
                        c.kind(piece) == NodeKind::StringLiteral
                            && c.lexeme(c.head_token(piece)).trim_matches(['\'', '"'])
                                == "package:meta/meta.dart"
                    })
            })
    })
}

/// The const-ability shape (:103-133 folded to the token faces): no
/// mixins, every instance field of the class and its same-file
/// extends chain spelled `final`/`const`, and the initializer list's
/// invocation face resolving to a const target.
fn const_able_class(c: &Check<'_>, container: u32, node: u32) -> bool {
    if supertypes(c, container)
        .iter()
        .any(|&(_, clause)| clause == SupertypeClause::With)
    {
        return false; // `interfaceElement.mixins.isNotEmpty` (:50)
    }
    let mut rows = vec![container];
    rows.extend(same_file_ancestors(c, container, true));
    if !rows.iter().all(|&row| fields_all_final(c, row)) {
        return false;
    }
    let Some(region) = initializer_region(c, container, node) else {
        return false;
    };
    let invocation = const_invocation_face(c, container, region);
    let entries = initializer_entries_able(c, container, node);
    invocation && entries
}

/// The `canBeConst` initializer-entry face: every `assert` condition and
/// message and every field-initializer value must be potentially
/// constant with the constructor's parameters bound (upstream runs the
/// resolved constant verifier over the pretend-`const` constructor —
/// `upstream Dart SDK source, pkg/linter/lib/src/extensions.dart`,
/// `ConstructorDeclaration.canBeConst`). A method invocation
/// (`a.toString()`) or a reference to a non-const variable (`f = notConst`)
/// is not potentially constant, so the constructor cannot be made const
/// and the rule stays silent. The `super`/`this` invocation entries are
/// the invocation face's decision (`const_invocation_face`) and are
/// skipped here.
fn initializer_entries_able(c: &Check<'_>, container: u32, node: u32) -> bool {
    let Some(parameters) = c
        .collect_direct(node)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
    else {
        return false;
    };
    let params: Vec<&str> = crate::rules::types::leak_formal_rows(c, parameters)
        .into_iter()
        .filter_map(|formal| {
            c.field(formal, PayloadField::Name)
                .filter(|&token| token != NONE_TOKEN)
                .map(|token| c.lexeme(token))
        })
        .collect();
    let initializers = if c.kind(node) == NodeKind::PrimaryConstructor {
        primary_body_row(c, container)
            .and_then(|body| {
                c.collect_direct(body)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::Initializers)
            })
    } else {
        c.collect_direct(node)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::Initializers)
    };
    let Some(list) = initializers else {
        // No initializer list: nothing but the invocation face to judge.
        return true;
    };
    for entry in c.collect_direct(list) {
        if c.kind(entry) != NodeKind::Initializer {
            continue;
        }
        let kids = c.collect_direct(entry);
        // The invocation entries belong to `const_invocation_face`.
        if kids.iter().any(|&kid| c.kind(kid) == NodeKind::SuperExpression) {
            continue;
        }
        let Some(name_send) = kids
            .iter()
            .position(|&kid| c.kind(kid) == NodeKind::Send && is_field_name_send(c, kid))
        else {
            // No field target: a `this(...)` invocation (the invocation
            // face owns it) or an `assert` entry (its condition and
            // message rows are the potentially-const face).
            for kid in kids {
                if c.kind(kid) == NodeKind::Assert
                    && !c
                        .collect_direct(kid)
                        .into_iter()
                        .all(|part| is_potentially_const(c.unit, part, &params))
                {
                    return false;
                }
            }
            continue;
        };
        // The value rows: everything past the `name =` send, minus the
        // trailing `Assignment` row — the production tiling re-covers
        // the value's tail segment with its terminator, it is not a
        // genuine assignment expression.
        let mut value = &kids[name_send + 1..];
        if let Some((last, rest)) = value.split_last() {
            if c.kind(*last) == NodeKind::Assignment {
                value = rest;
            }
        }
        for kid in value {
            match c.kind(*kid) {
                // A bare-name value (`_a = a`, `f = notConst`): a bound
                // parameter or a same-file constant only.
                NodeKind::Identifier => {
                    let name = c.lexeme(c.head_token(*kid));
                    if !potentially_const_name(c.unit, &params, name) {
                        return false;
                    }
                }
                // The chain and sentinel pieces of a compound value —
                // the owning expression rows carry the verdict.
                NodeKind::NoTypeArguments | NodeKind::NoArguments | NodeKind::Arguments => {}
                _ => {
                    if !is_potentially_const(c.unit, *kid, &params) {
                        return false;
                    }
                }
            }
        }
    }
    true
}

/// Whether the send row is a field-initializer name piece — its tokens
/// are `name =` (the value tiles as a sibling row).
fn is_field_name_send(c: &Check<'_>, send: u32) -> bool {
    let tokens = c.unit.ast.token_range(send);
    tokens.start + 1 < tokens.end && c.lexeme(tokens.start + 1) == "="
}



/// Whether every instance field of the row is spelled
/// `final`/`const` without `late`, with an initializer that is an
/// obviously const literal (`final f = Object()` bails — the
/// `canBeConst` face without const evaluation).
fn fields_all_final(c: &Check<'_>, class_row: u32) -> bool {
    for member in container_members(c, class_row).1 {
        if c.kind(member) != NodeKind::Fields {
            continue;
        }
        if c.field(member, PayloadField::MemberKind)
            != Some(dartq_parser::MemberKind::NonStaticField as u32)
        {
            continue;
        }
        for decl in c.collect_direct(member) {
            if c.kind(decl) != NodeKind::VariablesDeclaration {
                continue;
            }
            if c.field(decl, PayloadField::LateKeyword)
                .unwrap_or(NONE_TOKEN)
                != NONE_TOKEN
            {
                return false; // `late` blocks const construction
            }
            if !variables_declaration_keyword(c, decl)
                .is_some_and(|keyword| matches!(c.lexeme(keyword), "final" | "const"))
            {
                return false;
            }
            for var in c.collect_direct(decl) {
                if c.kind(var) != NodeKind::InitializedIdentifier {
                    continue;
                }
                let initializer = c
                    .collect_direct(var)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer);
                if let Some(value) = initializer.and_then(|init| last_child(c, init))
                    && !const_literal(c, value)
                {
                    return false;
                }
            }
        }
    }
    true
}

/// The obviously const literal faces (no const evaluation): the
/// literal kinds, a `const`-prefixed expression, or a parenthesized /
/// negated one.
fn const_literal(c: &Check<'_>, value: u32) -> bool {
    match c.kind(value) {
        NodeKind::LiteralInt
        | NodeKind::LiteralDouble
        | NodeKind::LiteralBool
        | NodeKind::StringLiteral
        | NodeKind::LiteralNull
        | NodeKind::ListLiteral
        | NodeKind::SetOrMapLiteral
        | NodeKind::SymbolLiteral
        | NodeKind::ConstExpression
        | NodeKind::ConstLiteral => true,
        NodeKind::UnaryPrefix | NodeKind::ParenthesizedExpression => {
            last_child(c, value).is_some_and(|row| const_literal(c, row))
        }
        _ => false,
    }
}

/// The token region between the parameter group's `)` and the body —
/// the initializer list. A primary constructor scans its `this`-body
/// member row (which starts at the token after the leading `this`: the
/// `:` of its initializers or its `{`); no body at all is the empty
/// region.
fn initializer_region(
    c: &Check<'_>,
    container: u32,
    node: u32,
) -> Option<core::ops::Range<u32>> {
    let params_end = c
        .collect_direct(node)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
        .map(|row| c.unit.ast.token_range(row).end)?;
    if c.kind(node) == NodeKind::PrimaryConstructor {
        let Some(body) = primary_body_row(c, container) else {
            return Some(params_end..params_end);
        };
        let tokens = c.unit.ast.token_range(body);
        let start = if c.lexeme(tokens.start) == "this" {
            tokens.start + 1
        } else {
            tokens.start
        };
        Some(start..tokens.end)
    } else {
        let end = last_child(c, node).map_or(params_end, |body| c.head_token(body));
        Some(params_end..end.max(params_end))
    }
}

/// The invocation face (:103-133): a `super`/`this` invocation
/// resolves its target constructor's `const` keyword; every other entry
/// — `assert`, field initializers — leaves the decision to the
/// implicit-super face (:129-132) and `canBeConst`.
fn const_invocation_face(c: &Check<'_>, container: u32, region: core::ops::Range<u32>) -> bool {
    let mut cursor = region.start;
    while cursor < region.end && c.lexeme(cursor) != ":" {
        cursor += 1;
    }
    if cursor == region.end {
        return implicit_super_const(c, container);
    }
    cursor += 1;
    while cursor < region.end {
        match c.lexeme(cursor) {
            "super" => return explicit_super_const(c, container, cursor),
            "this" => return const_ctor_named(c, container, named_after(c, cursor).as_deref()),
            _ => {}
        }
        cursor += 1;
    }
    // Field initializers and `assert` entries never decide the invocation
    // face (:103-133 keys only on the super/this invocations) — the
    // implicit-super face answers and `canBeConst` carries their
    // const-ness (a parameter reference is the canonical const-constructor
    // field initializer).
    implicit_super_const(c, container)
}

/// The constructor name written after `super`/`this`, when any
/// (`super.named()` / `this.named()`).
fn named_after(c: &Check<'_>, keyword: u32) -> Option<String> {
    let dot = keyword + 1;
    (c.lexeme(dot) == ".").then(|| c.lexeme(dot + 1).to_string())
}

/// The implicit `super()` face (:129-132): no `extends` means
/// `Object`, whose unnamed constructor is const; a same-file extends
/// class needs a const unnamed constructor; a cross-file link
/// under-reports.
fn implicit_super_const(c: &Check<'_>, container: u32) -> bool {
    let extends = supertypes(c, container)
        .into_iter()
        .find(|&(_, clause)| clause == SupertypeClause::Extends);
    match extends {
        None => true,
        Some((name, _)) => {
            same_file_declaration(c, name).is_some_and(|row| const_ctor_named(c, row, None))
        }
    }
}

/// The explicit `super(...)` face (:110-116): the direct extends
/// class's named/unnamed constructor must be const.
fn explicit_super_const(c: &Check<'_>, container: u32, keyword: u32) -> bool {
    let name = named_after(c, keyword);
    let extends = supertypes(c, container)
        .into_iter()
        .find(|&(_, clause)| clause == SupertypeClause::Extends);
    let Some((class_name, _)) = extends else {
        return false;
    };
    same_file_declaration(c, class_name)
        .is_some_and(|row| const_ctor_named(c, row, name.as_deref()))
}

/// Whether the class-like row declares a constructor of `name`
/// (`None` = unnamed) whose header writes `const`.
fn const_ctor_named(c: &Check<'_>, class_row: u32, name: Option<&str>) -> bool {
    container_members(c, class_row).1.into_iter().any(|member| {
        c.kind(member) == NodeKind::Constructor
            && constructor_name_part(c, member).map(|part| c.lexeme(c.head_token(part))) == name
            && ctor_header_has_const(c, member)
    })
}
