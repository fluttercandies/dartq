use super::{Check, NodeKind, RuleSpec, annotation_names, constructor_name_part, container_members, ctor_header_has_const, is_identifier_token, range_has_const, top_level_annotations};
use crate::rules::{NONE_TOKEN, SupertypeClause, last_child, same_file_ancestors, same_file_declaration, supertypes, utf16_token_span, variables_declaration_keyword};

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
/// constructor is const), an `assert` or a field initializer bails,
/// and `canBeConst` folds to every instance field of the class and its
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
    // `node.firstTokenAfterCommentAndMetadata` (:58/:65) — the row
    // head (metadata rides the left sibling; an extension-type
    // constructor's head is the type-name echo, the `errorRange`
    // face).
    c.report(utf16_token_span(c, c.head_token(node)), &[]);
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
        .collect_direct(container)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Identifier)
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
        c.report(utf16_token_span(c, c.head_token(node)), &[]);
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
    if last_child(c, node).is_some_and(|body| {
        c.kind(body) == NodeKind::PrimaryConstructorBody
            && c.collect_direct(body)
                .into_iter()
                .any(|kid| matches!(c.kind(kid), NodeKind::Block | NodeKind::BlockBody))
    }) {
        return;
    }
    if !has_immutable(c, container) {
        return;
    }
    let name = c
        .collect_direct(container)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Identifier)
        .map(|row| c.head_token(row));
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

/// `hasImmutableAnnotation` (:136-147): the container's own
/// annotations plus the same-file chain walk.
fn has_immutable(c: &Check<'_>, container: u32) -> bool {
    std::iter::once(container)
        .chain(same_file_ancestors(c, container, true))
        .any(|row| {
            top_level_annotations(c, row)
                .iter()
                .any(|&meta| annotation_names(c, meta, "immutable"))
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
    let Some(region) = initializer_region(c, node) else {
        return false;
    };
    const_invocation_face(c, container, region)
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
        if c.payload(member).first() != Some(&(dartq_parser::MemberKind::NonStaticField as u32))
        {
            continue;
        }
        for decl in c.collect_direct(member) {
            if c.kind(decl) != NodeKind::VariablesDeclaration {
                continue;
            }
            if c.payload(decl).first().copied().unwrap_or(NONE_TOKEN) != NONE_TOKEN {
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
/// (the face reads after the leading `this`); no body at all is the
/// empty region.
fn initializer_region(c: &Check<'_>, node: u32) -> Option<core::ops::Range<u32>> {
    let params_end = c
        .collect_direct(node)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
        .map(|row| c.unit.ast.token_range(row).end)?;
    if c.kind(node) == NodeKind::PrimaryConstructor {
        let body = last_child(c, node)?;
        if c.kind(body) != NodeKind::PrimaryConstructorBody {
            return Some(params_end..params_end);
        }
        let tokens = c.unit.ast.token_range(body);
        Some(tokens.start + 1..tokens.end)
    } else {
        let end = last_child(c, node).map_or(params_end, |body| c.head_token(body));
        Some(params_end..end.max(params_end))
    }
}

/// The invocation face (:103-133): a `super`/`this` invocation
/// resolves its target constructor's `const` keyword; an `assert` or
/// a field-initializer-only list bails (the const-evaluation faces);
/// no initializer list takes the implicit-super face (:129-132).
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
            "assert" => return false,
            _ => {}
        }
        cursor += 1;
    }
    false
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

/// Whether the container's header or the primary constructor itself
/// writes `const` before the name (`class const C(...)`,
/// `extension type const E(...)`).
fn primary_header_const(c: &Check<'_>, container: u32, primary: u32) -> bool {
    if c.kind(primary) == NodeKind::PrimaryConstructor && ctor_header_has_const(c, primary) {
        return true;
    }
    let name_head = c
        .collect_direct(container)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Identifier)
        .map(|row| c.head_token(row));
    name_head.is_some_and(|name_head| range_has_const(c, c.head_token(container)..name_head))
}
