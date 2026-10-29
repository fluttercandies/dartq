//! The convert-family assist producers (#2400): the twelve landed kinds of
//! the census's 23-kind convert wave, each mirroring its official
//! producer's trigger shape and edit (upstream Dart SDK source,
//! pkg/analysis_server/lib/src/services/correction/dart/convert_*.dart —
//! the file named on each producer).
//!
//! # The facts-blocked eleven
//!
//! These upstream kinds read resolved element/type identities this
//! substrate does not model; they stay out (the flutter-family posture:
//! facts-blocked, not a divergence — fewer assists, never an approximated
//! one):
//!
//! - `classToEnum` (convert_class_to_enum.dart): reads field/constructor
//!   element identity, redirect chains, and the type system (~97 element
//!   reads) to synthesize the enum constants.
//! - `classToMixin` (convert_class_to_mixin.dart): partitions the written
//!   supertype clauses by *which class element* each `super.` reference
//!   resolves into — the declaring-class identity of a super member is the
//!   missing face (the name-level model answers the receiver's own
//!   container, not the declaring ancestor).
//! - `partOfToPartUri` (convert_part_of_to_uri.dart): reads
//!   `libraryElement.firstFragment.source.fullName` — the defining unit's
//!   file path; the assist face carries no file-table access.
//! - `relativeToPackageImport` (convert_to_package_import.dart): reads
//!   the relative import's resolved `DirectiveUriWithSource` package URI —
//!   the import-resolution face (relative spelling → package: URI) is the
//!   missing edge.
//! - `switchExpression` (convert_to_switch_expression.dart): reads
//!   top-level-function/variable write identities across case bodies and
//!   the exhaustiveness type system (`isAlwaysExhaustive`).
//! - `toDeclaringParameter` (convert_to_declaring_parameter.dart): reads
//!   field-formal declaring elements and field declarations by identity.
//! - `toDotShorthand` (convert_to_dot_shorthand.dart): guards through
//!   `computeContextType` — the completion-grade context-type computer.
//! - `toInBodyConstructor` (convert_to_in_body_constructor.dart): reads
//!   parameter/field element identity through the body's reference graph.
//! - `toInitializingFormal` (convert_to_initializing_formal.dart): reads
//!   write-element resolution (`assignment.writeElement`) and field/
//!   parameter type equality.
//! - `toPrimaryConstructor` (convert_to_primary_constructor.dart): gated
//!   on the `primary-constructors` experiment; the redirecting-generative
//!   check reads the constructor's `redirectedConstructor` element.
//! - `toSuperParameters` (convert_to_super_parameters.dart): reads the
//!   super constructor element's formal table and cross-declaration type
//!   equality.

use super::*;
use dartq_ast::TOKEN_NONE;

/// The payload word at `index` read as a token index.
fn payload_token(ast: &SyntaxTable, id: u32, index: usize) -> Option<usize> {
    ast.payload(id)
        .get(index)
        .copied()
        .map(|token| token as usize)
}

/// Whether the literal node is a set literal (`has_set_entry` payload bit —
/// the bridge's own set/map decision, upstream `SetOrMapLiteral.isSet`).
fn is_set_literal(ast: &SyntaxTable, id: u32) -> bool {
    ast.kind(id) == NodeKind::SetOrMapLiteral && ast.payload(id).get(1).copied() == Some(1)
}

/// `_isUnambiguousElement`: whether an element lexically forces a set
/// literal — an expression (anything but a map entry) is; `if`/`for`
/// control-flow elements recurse into their branches/body (upstream Dart
/// SDK source, convert_to_set_literal.dart `_isUnambiguousElement`).
fn is_unambiguous_collection_element(ast: &SyntaxTable, element: u32) -> bool {
    match ast.kind(element) {
        NodeKind::MapEntry => false,
        NodeKind::IfControlFlow => direct_children(ast, element).into_iter().any(|child| {
            matches!(
                ast.kind(child),
                NodeKind::ThenControlFlow | NodeKind::ElseControlFlow
            ) && is_unambiguous_collection_element(ast, child)
        }),
        NodeKind::ForControlFlow => direct_children(ast, element)
            .into_iter()
            .last()
            .is_some_and(|body| is_unambiguous_collection_element(ast, body)),
        _ => true,
    }
}

/// Whether any element of the list literal lexically forces a set
/// (`_listHasUnambiguousElement`).
fn list_has_unambiguous_element(ast: &SyntaxTable, list: u32) -> bool {
    ast.kind(list) == NodeKind::ListLiteral
        && direct_children(ast, list)
            .into_iter()
            .any(|element| is_unambiguous_collection_element(ast, element))
}

/// The creation head's written type arguments (`<…>` between the head
/// name and the `(`; empty when unparameterized) — the text the literal
/// conversions carry into the replacement.
fn written_type_arguments(
    context: &AssistContext<'_>,
    creation_span: core::ops::Range<u32>,
    open16: u32,
) -> Option<String> {
    let text = context.text;
    let tokens = context.tokens;
    let open = token_at(tokens, open16)?;
    // From the creation head: a leading `const`/`new` keyword, then the
    // (possibly prefixed) type name, then an optional `<…>` group.
    let mut cursor = token_at(tokens, creation_span.start)?;
    while matches!(tokens.lexeme(text, cursor), "const" | "new") {
        cursor += 1;
    }
    while matches!(
        tokens.get(cursor)?.kind,
        TokenKind::Identifier | TokenKind::Period
    ) {
        cursor += 1;
    }
    if cursor >= open || tokens.get(cursor)?.kind != TokenKind::Lt {
        return Some(String::new());
    }
    let close = tokens.matching_end(cursor).filter(|&close| close < open)?;
    Some(
        slice16(
            text,
            &(tokens.get(cursor)?.utf16_offset..utf16_end(tokens, text, close)?),
        )?
        .to_owned(),
    )
}

// ---------------------------------------------------------------------------
// dart.assist.convert.toIfElement
// ---------------------------------------------------------------------------

/// `dart.assist.convert.toIfElement` (`ConvertConditionalExpressionToIfElement`,
/// upstream Dart SDK source,
/// pkg/analysis_server/lib/src/services/correction/dart/
/// convert_conditional_expression_to_if_element.dart): a conditional
/// expression inside a list or set literal rewrites to the collection
/// `if` element — `if (condition) then else`. The outermost parenthesized
/// wrapper joins the replacement (the producer's `nodeToReplace` walk);
/// the three arms read unparenthesized.
pub(crate) fn convert_to_if_element(
    context: &AssistContext<'_>,
) -> Option<Vec<(u32, u32, String)>> {
    let node = covering_node(
        context.tokens,
        context.text,
        context.ast,
        context.selection.start,
    )?;
    let conditional = ancestor_or_self_kind(
        context.tokens,
        context.text,
        context.ast,
        node,
        NodeKind::Conditional,
    )?;
    // The parent decides the offer: a list or set literal hosts the `if`
    // element; wrapping parentheses climb into the replaced node.
    let mut node_to_replace = conditional;
    let mut parent = context.ast.parent(node_to_replace)?;
    while context.ast.kind(parent) == NodeKind::ParenthesizedExpression {
        node_to_replace = parent;
        parent = context.ast.parent(node_to_replace)?;
    }
    if context.ast.kind(parent) != NodeKind::ListLiteral && !is_set_literal(context.ast, parent) {
        return None;
    }
    let (_, _, _, condition, then_arm, else_arm) = conditional_parts(context, conditional)?;
    let replaced = node_span16(context.tokens, context.text, context.ast, node_to_replace)?;
    Some(vec![(
        replaced.start,
        replaced.end - replaced.start,
        format!(
            "if ({}) {} else {}",
            slice16(context.text, &condition)?,
            slice16(context.text, &then_arm)?,
            slice16(context.text, &else_arm)?,
        ),
    )])
}

// ---------------------------------------------------------------------------
// dart.assist.convert.toSpread
// ---------------------------------------------------------------------------

/// `dart.assist.convert.toSpread` (`ConvertAddAllToSpread`, upstream Dart
/// SDK source, pkg/analysis_server/lib/src/services/correction/dart/
/// convert_add_all_to_spread.dart): a cascaded `..addAll(x)` whose
/// cascade target is a list literal moves the argument's elements into
/// the literal — a list-literal argument splices its element run
/// verbatim, every other argument rewraps (`x`, `x ?? []` → `...?x`,
/// `c ? t : []` → `if (c) ...t`), and the cascade section deletes. The
/// cursor sits on the `addAll` name (the official factory's trigger
/// node), the invocation is the cascade's first section, and the
/// list-literal argument is non-empty (the producer's own guard).
pub(crate) fn convert_to_spread(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let text = context.text;
    let tokens = context.tokens;
    let ast = context.ast;
    // The trigger: the `addAll` name itself.
    let name = covering_token(context)?;
    if !word_at(tokens, text, name, "addAll") {
        return None;
    }
    // The cascade section: the `..` operator before the name, then the
    // single argument list.
    if tokens.get(name.checked_sub(1)?)?.kind != TokenKind::Period {
        return None;
    }
    let operator = name - 1;
    let access = ast
        .parent(covering_node(tokens, text, ast, context.selection.start)?)
        .filter(|&parent| ast.kind(parent) == NodeKind::CascadeAccess)?;
    if payload_token(ast, access, 0) != Some(operator) {
        return None;
    }
    if tokens.get(name.checked_add(1)?)?.kind != TokenKind::OpenParen {
        return None;
    }
    let args_open = name + 1;
    let args_close = tokens.matching_end(args_open)?;
    // The cascade: the target leads and this section is its first access.
    let cascade = ast.parent(access)?;
    if ast.kind(cascade) != NodeKind::Cascade {
        return None;
    }
    let mut children = direct_children(ast, cascade).into_iter();
    let target = children.next()?;
    if ast.kind(target) != NodeKind::ListLiteral {
        return None;
    }
    if children.next()? != access {
        return None;
    }
    // The element text per the argument shape.
    let arg16 = utf16_span_between(text, tokens, args_open + 1, args_close);
    if slice16(text, &arg16)?.trim().is_empty() {
        return None;
    }
    let element_text = match ancestor_or_self_kind(
        tokens,
        text,
        ast,
        covering_node(tokens, text, ast, arg16.start)?,
        NodeKind::ListLiteral,
    ) {
        Some(list)
            if node_span16(tokens, text, ast, list)
                .is_some_and(|span| span.start == arg16.start && span.end == arg16.end) =>
        {
            // A list-literal argument: the element run splices verbatim.
            let elements = direct_children(ast, list);
            if elements.is_empty() {
                return None;
            }
            let first = node_span16(tokens, text, ast, elements[0])?;
            let last = node_span16(tokens, text, ast, *elements.last()?)?;
            slice16(text, &(first.start..last.end))?.to_owned()
        }
        _ => compute_spread_element_text(context, arg16.start)?,
    };
    // The insertion point: after the target's last element, else the `[`
    // (no separator — `[]..addAll(['b'])` → `['b']`).
    let insertion = match direct_children(ast, target).into_iter().last() {
        Some(last) => node_span16(tokens, text, ast, last)?.end,
        None => node_span16(tokens, text, ast, target)?.start + 1,
    };
    let inserted = match direct_children(ast, target).into_iter().last() {
        Some(_) => format!(", {element_text}"),
        None => element_text,
    };
    // Two edits: the element insertion and the section deletion (the
    // section span runs from the `..` through the argument close — the
    // official `range.node(invocation)` includes the cascade operator).
    let operator_start = tokens.get(operator)?.utf16_offset;
    let section_end = utf16_end(tokens, text, args_close)?;
    let edits = vec![
        (insertion, 0, inserted),
        (operator_start, section_end - operator_start, String::new()),
    ];
    Some(sort_edits(edits))
}

/// `_computeElementText`: the non-literal argument's spread spelling —
/// `x ?? []`/`x ?? const []` reads `...?x`, `c ? t : []` reads
/// `if (c) ...t`, everything else `...x` (the argument text
/// unparenthesized).
fn compute_spread_element_text(context: &AssistContext<'_>, from16: u32) -> Option<String> {
    let text = context.text;
    let tokens = context.tokens;
    let ast = context.ast;
    let arg = unwrap_parens(context, covering_node(tokens, text, ast, from16)?)?;
    let arg_span = trim_span16(text, expression_extent(tokens, text, ast, arg)?)?;
    if ast.kind(arg) == NodeKind::BinaryExpression {
        let (_, right, operator, left_span, right_span) = binary_parts(context, arg)?;
        if tokens.lexeme(text, operator) == "??" {
            let right_text = slice16(text, &right_span)?;
            if (right_text == "[]" || right_text == "const []")
                && direct_children(ast, right).is_empty()
            {
                return Some(format!("...?{}", slice16(text, &left_span)?));
            }
        }
    }
    if ast.kind(arg) == NodeKind::Conditional {
        let (condition, then_arm, else_arm, ..) = conditional_parts(context, arg)?;
        let else_text = slice16(text, &node_span16(tokens, text, ast, else_arm)?)?;
        if (else_text == "[]" || else_text == "const []")
            && direct_children(ast, else_arm).is_empty()
        {
            return Some(format!(
                "if ({}) ...{}",
                slice16(text, &node_span16(tokens, text, ast, condition)?)?,
                slice16(text, &node_span16(tokens, text, ast, then_arm)?)?,
            ));
        }
    }
    Some(format!("...{}", slice16(text, &arg_span)?))
}

// ---------------------------------------------------------------------------
// dart.assist.convert.toMapLiteral
// ---------------------------------------------------------------------------

/// `dart.assist.convert.toMapLiteral` (`ConvertToMapLiteral`, upstream Dart
/// SDK source, pkg/analysis_server/lib/src/services/correction/dart/
/// convert_to_map_literal.dart): the unparameterized-head creation
/// `Map()` / `LinkedHashMap()` (no named constructor, no arguments,
/// cursor at or before the argument list) rewrites to the map literal
/// `<…>{}` — written type arguments carried verbatim. The
/// context-inferred type-argument leg (the producer's
/// `staticTypeArguments`, which only answers in inference positions) is
/// absent: the conversion offers where the written form decides the
/// output — an explicit `<…>` carried over, or the bare head (upstream
/// renders `{}` there too); a creation whose immediate context could
/// infer non-dynamic arguments — an invocation argument — declines.
pub(crate) fn convert_to_map_literal(
    context: &AssistContext<'_>,
) -> Option<Vec<(u32, u32, String)>> {
    let creation = literal_creation_head(context, &["Map", "LinkedHashMap"])?;
    // One argument-less `(` `)` pair; no named constructor between.
    let (open, close) = creation.arguments?;
    let args_open16 = context.tokens.get(open)?.utf16_offset;
    if close != open + 1 {
        return None;
    }
    let span = creation.span;
    let type_args = written_type_arguments(context, span.clone(), args_open16)?;
    // The inference-position guard: an enclosing argument list could give
    // the creation non-dynamic type arguments upstream reads off the
    // static type.
    if type_args.is_empty() && creation.in_arguments {
        return None;
    }
    Some(vec![(
        span.start,
        span.end - span.start,
        format!("{type_args}{{}}"),
    )])
}

/// The instance-creation head a literal conversion consumes: the covering
/// node climbs to the creation row (`new`/`const`/implicit), the head
/// spells one of `names` with no named constructor (no `.` between the
/// head name and the `(`), and the cursor sits at or before the argument
/// list (the producers' `node.offset > creation.argumentList.offset`
/// guard). Answers the creation's span, its argument tokens, and whether
/// an argument-list ancestor encloses it (the inference-position face).
struct CreationHead {
    span: core::ops::Range<u32>,
    /// `(open, close)` token indices of the argument list, when it parses.
    arguments: Option<(usize, usize)>,
    /// Whether an enclosing argument list could infer the creation's type
    /// arguments (the absent staticTypeArguments leg's posture).
    in_arguments: bool,
}

/// The literal-creation head shared by the map/set producers.
fn literal_creation_head(context: &AssistContext<'_>, names: &[&str]) -> Option<CreationHead> {
    let text = context.text;
    let tokens = context.tokens;
    let ast = context.ast;
    let node = covering_node(tokens, text, ast, context.selection.start)?;
    let creation = ancestor_or_self_kind(tokens, text, ast, node, NodeKind::NewExpression)
        .or_else(|| ancestor_or_self_kind(tokens, text, ast, node, NodeKind::ImplicitCreation))
        .or_else(|| ancestor_or_self_kind(tokens, text, ast, node, NodeKind::ConstExpression))?;
    let span = expression_extent(tokens, text, ast, creation)?;
    // Cursor at or before the argument list.
    let head = token_at(tokens, span.start)?;
    let mut cursor = head;
    while matches!(tokens.lexeme(text, cursor), "const" | "new") {
        cursor += 1;
    }
    // The (possibly prefixed) type name.
    if !matches!(tokens.get(cursor)?.kind, TokenKind::Identifier)
        || !names.contains(&tokens.lexeme(text, cursor))
    {
        return None;
    }
    cursor += 1;
    // Optional prefixed spelling (`p.Map`), optional type arguments, then
    // the `(` — a named constructor (`.` before the `(`) declines.
    let mut open = None;
    loop {
        match tokens.get(cursor)?.kind {
            TokenKind::Identifier if open.is_none() => cursor += 1,
            TokenKind::Period => return None,
            TokenKind::Lt => {
                let close = tokens.matching_end(cursor)?;
                cursor = close + 1;
            }
            TokenKind::OpenParen => {
                open = Some(cursor);
                break;
            }
            _ => return None,
        }
    }
    let open = open?;
    let close = tokens.matching_end(open)?;
    if context.selection.start > tokens.get(open)?.utf16_offset {
        return None;
    }
    let in_arguments = {
        let mut current = creation;
        let mut found = false;
        while let Some(parent) = ast.parent(current) {
            if ast.kind(parent) == NodeKind::Arguments {
                found = true;
                break;
            }
            current = parent;
        }
        found
    };
    Some(CreationHead {
        span,
        arguments: Some((open, close)),
        in_arguments,
    })
}

// ---------------------------------------------------------------------------
// dart.assist.convert.toSetLiteral
// ---------------------------------------------------------------------------

/// `dart.assist.convert.toSetLiteral` (`ConvertToSetLiteral`, upstream Dart
/// SDK source, pkg/analysis_server/lib/src/services/correction/dart/
/// convert_to_set_literal.dart), both faces:
///
/// - `list.toSet()` rewrites to the set literal — the target list's
///   element run moves inside braces; the `<dynamic>` disambiguation
///   (the producer's no-type-args-no-unambiguous-element face) is absent:
///   the conversion offers only when the written form decides a set
///   (explicit type arguments or an unambiguous element).
/// - `Set()` / `Set.from(list)` / `Set.of(list)` (the `LinkedHashSet`
///   spellings included) rewrite likewise; the element-list text moves
///   for the `from`/`of` forms. The would-be-inferred decision's
///   parameter-context leg (the `correspondingParameter` type) is absent —
///   the offer requires the enclosing variable declaration's written
///   `Set` type or an unambiguous element, the two decidable legs.
pub(crate) fn convert_to_set_literal(
    context: &AssistContext<'_>,
) -> Option<Vec<(u32, u32, String)>> {
    convert_list_to_set(context).or_else(|| convert_set_creation(context))
}

/// The `list.toSet()` face.
fn convert_list_to_set(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let text = context.text;
    let tokens = context.tokens;
    let ast = context.ast;
    let node = covering_node(tokens, text, ast, context.selection.start)?;
    let send = ancestor_or_self_kind(tokens, text, ast, node, NodeKind::Send)?;
    let extent = expression_extent(tokens, text, ast, send)?;
    // `.toSet()` tail: the `(` group closes the extent, the name before
    // it is `toSet`, and the cursor sits at or before the `(`.
    let close = token_at(tokens, extent.end)?.checked_sub(1)?;
    if tokens.get(close)?.kind != TokenKind::CloseParen
        || utf16_end(tokens, text, close)? != extent.end
    {
        return None;
    }
    let open = matching_open(tokens, close, TokenKind::CloseParen)?;
    let name = open.checked_sub(1)?;
    if !word_at(tokens, text, name, "toSet")
        || tokens.get(name.checked_sub(1)?)?.kind != TokenKind::Period
    {
        return None;
    }
    if context.selection.start > tokens.get(open)?.utf16_offset {
        return None;
    }
    // The target: the list literal whose extent ends at the `.`.
    let dot = name - 1;
    let target = unwrap_parens(
        context,
        covering_node(
            tokens,
            text,
            ast,
            tokens.get(dot.checked_sub(1)?)?.utf16_offset,
        )?,
    )?;
    if ast.kind(target) != NodeKind::ListLiteral {
        return None;
    }
    let list_span = expression_extent(tokens, text, ast, target)?;
    if list_span.end != tokens.get(dot)?.utf16_offset {
        return None;
    }
    // The written type arguments decide, else an unambiguous element; the
    // `<dynamic>` face declines.
    let head = token_at(tokens, list_span.start)?;
    let (has_type_args, bracket_open) = if tokens.get(head)?.kind == TokenKind::Lt {
        let close = tokens.matching_end(head)?;
        (true, close + 1)
    } else {
        (false, head)
    };
    if !has_type_args && !list_has_unambiguous_element(ast, target) {
        return None;
    }
    let bracket_close = tokens.matching_end(bracket_open)?;
    // Two edits: the `[` becomes `{`, the `]`-through-`toSet()` tail
    // becomes `}` (the official `range.startEnd(rightBracket, invocation)`).
    let tail_start = tokens.get(bracket_close)?.utf16_offset;
    Some(vec![
        (list_span.start, 1, "{".to_owned()),
        (tail_start, extent.end - tail_start, "}".to_owned()),
    ])
}

/// The `Set()` / `Set.from(list)` / `Set.of(list)` face.
fn convert_set_creation(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let text = context.text;
    let tokens = context.tokens;
    let creation = literal_creation_head(context, &["Set", "LinkedHashSet"])?;
    let (open, close) = creation.arguments?;
    let args_open16 = tokens.get(open)?.utf16_offset;
    let span = creation.span;
    let mut type_args = written_type_arguments(context, span.clone(), args_open16)?;
    if open + 1 == close {
        // `Set()`: the would-be-inferred decision — the variable
        // declaration's written `Set` type or an unambiguous element;
        // the parameter-context leg is absent, so an undecided posture
        // declines.
        if type_args.is_empty() && !set_would_be_inferred(context) {
            return None;
        }
        return Some(vec![(
            span.start,
            span.end - span.start,
            format!("{type_args}{{}}"),
        )]);
    }
    // `Set.from(list)` / `Set.of(list)`: exactly one list-literal argument.
    if close != open + 2 || !matches!(tokens.lexeme(text, open + 1), "from" | "of") {
        return None;
    }
    let arg16 = utf16_span_between(text, tokens, open + 1, close);
    let arg = unwrap_parens(
        context,
        covering_node(tokens, text, context.ast, arg16.start)?,
    )?;
    if context.ast.kind(arg) != NodeKind::ListLiteral {
        return None;
    }
    let arg_span = expression_extent(tokens, text, context.ast, arg)?;
    let arg_head = token_at(tokens, arg_span.start)?;
    let (arg_has_type_args, bracket_open) = if tokens.get(arg_head)?.kind == TokenKind::Lt {
        let close = tokens.matching_end(arg_head)?;
        (true, close + 1)
    } else {
        (false, arg_head)
    };
    let bracket_close = tokens.matching_end(bracket_open)?;
    // Type arguments: the constructor's, else the element list's; else
    // the undecided-inference posture declines.
    if type_args.is_empty() {
        if arg_has_type_args {
            type_args =
                written_type_arguments(context, arg_span, tokens.get(bracket_open)?.utf16_offset)?;
        }
        if type_args.is_empty() && !set_would_be_inferred(context) {
            return None;
        }
    }
    let elements = utf16_span_between(text, tokens, bracket_open + 1, bracket_close);
    let elements_text = slice16(text, &elements)?.to_owned();
    Some(vec![(
        span.start,
        span.end - span.start,
        format!("{type_args}{{{elements_text}}}"),
    )])
}

/// The decidable leg of `_setWouldBeInferred`: the enclosing variable
/// declaration's written type names the `Set` class (any type arguments —
/// the official `type.element == setElement`), else `false`. The
/// parameter-context leg is the absent face.
fn set_would_be_inferred(context: &AssistContext<'_>) -> bool {
    let Some(node) = covering_node(
        context.tokens,
        context.text,
        context.ast,
        context.selection.start,
    ) else {
        return false;
    };
    let mut current = node;
    while let Some(parent) = context.ast.parent(current) {
        if context.ast.kind(parent) == NodeKind::VariablesDeclaration {
            if let Some(type_span) = child_of_kind(context.ast, parent, NodeKind::Type)
                .and_then(|type_node| {
                    node_span16(context.tokens, context.text, context.ast, type_node)
                })
                .and_then(|span| head_name_at(context, span.start))
            {
                return type_name_is_set(&type_span);
            }
            return false;
        }
        current = parent;
    }
    false
}

/// Whether the written type head names the `Set` class (the `Set`/`Set<…>`
/// spellings; `LinkedHashSet` is a distinct head name).
fn type_name_is_set(head: &str) -> bool {
    head == "Set" || head.starts_with("Set<")
}

/// The identifier head at `offset16` — its full lexeme.
fn head_name_at(context: &AssistContext<'_>, offset16: u32) -> Option<String> {
    let index = token_at(context.tokens, offset16)?;
    Some(context.tokens.lexeme(context.text, index).to_owned())
}

// ---------------------------------------------------------------------------
// dart.assist.convert.toForElement
// ---------------------------------------------------------------------------

/// `dart.assist.convert.toForElement` (`ConvertMapFromIterableToForLiteral`,
/// upstream Dart SDK source, pkg/analysis_server/lib/src/services/
/// correction/dart/convert_map_from_iterable_to_for_literal.dart): the
/// `Map.fromIterable(xs, key: (k) => …, value: (k) => …)` creation
/// rewrites to the map-for element — the same-parameter leg the fix
/// face's rewriter implements (the different-name legs re-spell the
/// closure bodies through a parameter-reference finder and stay absent).
/// The creation head spelling is re-verified on the assist's own trigger
/// span, the covering walk standing in for the fix report.
pub(crate) fn convert_to_for_element(
    context: &AssistContext<'_>,
) -> Option<Vec<(u32, u32, String)>> {
    let text = context.text;
    let tokens = context.tokens;
    let ast = context.ast;
    let node = covering_node(tokens, text, ast, context.selection.start)?;
    let creation = ancestor_or_self_kind(tokens, text, ast, node, NodeKind::NewExpression)
        .or_else(|| ancestor_or_self_kind(tokens, text, ast, node, NodeKind::ImplicitCreation))
        .or_else(|| ancestor_or_self_kind(tokens, text, ast, node, NodeKind::ConstExpression))?;
    let span = expression_extent(tokens, text, ast, creation)?;
    let input = FixInput {
        text,
        range: span.clone(),
        message: "",
        tokens: Some(tokens),
        path: Path::new(""),
        diagnostics: &[],
    };
    let (offset, length, replacement) = map_from_iterable_for_element(&input)?;
    // The cursor lives inside the creation (the official
    // `thisOrAncestorOfType<InstanceCreationExpression>` walk).
    if !span.contains(&context.selection.start) && span.end != context.selection.start {
        return None;
    }
    Some(vec![(offset, length, replacement)])
}

// ---------------------------------------------------------------------------
// dart.assist.convert.getterToFinalField
// ---------------------------------------------------------------------------

/// `dart.assist.convert.getterToFinalField` (`ConvertIntoFinalField`,
/// upstream Dart SDK source, pkg/analysis_server/lib/src/services/
/// correction/dart/convert_into_final_field.dart): the getter with no
/// corresponding setter rewrites to a `final` field — the returned
/// expression (an arrow body's payload or a single-`return` block) or an
/// `abstract final` declaration for the empty body. The cursor sits on
/// the getter's name or head (the official `node.withAncestors` walk
/// reads only the name/type/`get` chain); a static getter never
/// converts to the abstract face (the official's own guard), and an
/// extension/extension-type getter must be static.
pub(crate) fn getter_to_final_field(
    context: &AssistContext<'_>,
) -> Option<Vec<(u32, u32, String)>> {
    let text = context.text;
    let ast = context.ast;
    let node = covering_node(context.tokens, text, ast, context.selection.start)?;
    let getter = ancestor_or_self_kind(context.tokens, text, ast, node, NodeKind::Method)?;
    // The cursor sits on the head chain: at or before the name's end (the
    // official walk reads only the name and type nodes).
    let name = direct_children(ast, getter)
        .into_iter()
        .filter(|&child| ast.kind(child) == NodeKind::Identifier)
        .last()?;
    let name_span = node_span16(context.tokens, text, ast, name)?;
    if name_span.start == name_span.end || context.selection.start >= name_span.end {
        return None;
    }
    // A getter: the `get` keyword before the name, and no parameters.
    if !prev_token_is(context, name_span.start, "get") {
        return None;
    }
    if child_of_kind(ast, getter, NodeKind::FormalParameters).is_some() {
        return None;
    }
    // Not in an extension/extension type unless static (the official
    // enclosing-element gate).
    let is_static = member_is_static(context, getter);
    if !is_static
        && let Some(container) = container_of_member(context, getter)
        && matches!(
            ast.kind(container),
            NodeKind::ExtensionDeclaration | NodeKind::ExtensionTypeDeclaration
        )
    {
        return None;
    }
    // No corresponding setter in the same container.
    let name_text = slice16(text, &name_span)?.to_owned();
    if let Some(container) = container_of_member(context, getter)
        && let Some(body) = child_of_kind(ast, container, NodeKind::DeclarationBody)
    {
        for member in direct_children(ast, body) {
            if ast.kind(member) != NodeKind::Method || member == getter {
                continue;
            }
            if let Some(member_name) = direct_children(ast, member)
                .into_iter()
                .filter(|&child| ast.kind(child) == NodeKind::Identifier)
                .last()
                .and_then(|child| node_span16(context.tokens, text, ast, child))
                && slice16(text, &member_name)? == name_text
                && prev_token_is(context, member_name.start, "set")
            {
                return None;
            }
        }
    }
    // The written return type, when any.
    let type_text = direct_children(ast, getter)
        .into_iter()
        .take_while(|&child| ast.kind(child) != NodeKind::Identifier)
        .find(|&child| matches!(ast.kind(child), NodeKind::Type | NodeKind::VoidType))
        .and_then(|type_node| node_span16(context.tokens, text, ast, type_node))
        .and_then(|span| slice16(text, &span).map(str::to_owned));
    // The body decides the face.
    let body = member_body(ast, getter)?;
    let body_span = node_span16(context.tokens, text, ast, body)?;
    let (head_start, code) = match ast.kind(body) {
        NodeKind::ExpressionBody => {
            // The arrow payload is the row completing right before the
            // body (post-order adjacency).
            let expression = body.checked_sub(1)?;
            let expression_span = node_span16(context.tokens, text, ast, expression)?;
            if expression_span.start < body_span.start || expression_span.end > body_span.end {
                return None;
            }
            let expression_text = slice16(text, &expression_span)?.to_owned();
            let initializer = if expression_text == "null" {
                String::new()
            } else {
                format!(" = {expression_text}")
            };
            let head_start = type_start_or_get(context, getter, &name_span, &type_text)?;
            (
                head_start,
                format!("final{} {name_text}{initializer};", type_prefix(&type_text)),
            )
        }
        NodeKind::Block => {
            // Exactly one statement, a `return e;`.
            let statements = statement_children(ast, body);
            if statements.len() != 1 {
                return None;
            }
            let only = statements[0];
            if ast.kind(only) != NodeKind::ReturnStatement {
                return None;
            }
            let expression = direct_children(ast, only).into_iter().next()?;
            let expression_span = node_span16(context.tokens, text, ast, expression)?;
            let expression_text = slice16(text, &expression_span)?.to_owned();
            let initializer = if expression_text == "null" {
                String::new()
            } else {
                format!(" = {expression_text}")
            };
            let head_start = type_start_or_get(context, getter, &name_span, &type_text)?;
            (
                head_start,
                format!("final{} {name_text}{initializer};", type_prefix(&type_text)),
            )
        }
        NodeKind::EmptyBody => {
            if is_static {
                return None;
            }
            let head_start = type_start_or_get(context, getter, &name_span, &type_text)?;
            (
                head_start,
                format!("abstract final{} {name_text};", type_prefix(&type_text)),
            )
        }
        _ => return None,
    };
    let end = body_span.end;
    Some(vec![(head_start, end - head_start, code)])
}

/// The `final`/`abstract final` replacement's leading type text (" " when
/// written).
fn type_prefix(type_text: &Option<String>) -> String {
    match type_text {
        Some(text) => format!(" {text}"),
        None => String::new(),
    }
}

/// The replacement's start: the written return type's head, else the
/// `get` keyword (the official `returnType ?? propertyKeywordGet`).
fn type_start_or_get(
    context: &AssistContext<'_>,
    getter: u32,
    name_span: &core::ops::Range<u32>,
    type_text: &Option<String>,
) -> Option<u32> {
    if type_text.is_some() {
        for child in direct_children(context.ast, getter) {
            if context.ast.kind(child) == NodeKind::Identifier {
                break;
            }
            if matches!(context.ast.kind(child), NodeKind::Type | NodeKind::VoidType) {
                return node_span16(context.tokens, context.text, context.ast, child)
                    .map(|span| span.start);
            }
        }
    }
    // The `get` keyword right before the name.
    let index = token_at(context.tokens, name_span.start)?.checked_sub(1)?;
    Some(context.tokens.get(index)?.utf16_offset)
}

/// Whether the member carries `static` (a token scan between the member's
/// span head — the first modifier when one is written — and its name).
fn member_is_static(context: &AssistContext<'_>, member: u32) -> bool {
    let Some(span) = node_span16(context.tokens, context.text, context.ast, member) else {
        return false;
    };
    let Some(mut index) = token_at(context.tokens, span.start) else {
        return false;
    };
    loop {
        match context.tokens.get(index) {
            Some(token) if token.utf16_offset < span.end => {}
            _ => return false,
        }
        match context.tokens.lexeme(context.text, index) {
            "static" => return true,
            "(" | "{" | ";" | "=>" | "get" | "set" => return false,
            _ => index += 1,
        }
    }
}

/// The class-like container a member is declared in.
fn container_of_member(context: &AssistContext<'_>, member: u32) -> Option<u32> {
    let mut current = context.ast.parent(member)?;
    loop {
        if matches!(
            context.ast.kind(current),
            NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::ExtensionDeclaration
                | NodeKind::ExtensionTypeDeclaration
                | NodeKind::EnumDeclaration
        ) {
            return Some(current);
        }
        current = context.ast.parent(current)?;
    }
}

// ---------------------------------------------------------------------------
// dart.assist.convert.fieldFormalToNormal
// ---------------------------------------------------------------------------

/// `dart.assist.convert.fieldFormalToNormal` (`ConvertFieldFormalToNormal`,
/// upstream Dart SDK source, pkg/analysis_server/lib/src/services/
/// correction/dart/convert_field_formal_to_normal.dart): the constructor's
/// `this.field` parameter rewrites to a normal parameter with an
/// initializing initializer — `this.x` becomes `x` plus `: x = x` (or the
/// joining comma). The typed leg only: the untyped face needs the field's
/// declared type (the official `writeType(field.type)`) and declines. A
/// named private parameter renames to its public spelling for the
/// parameter, the field keeping the private name; a function-typed suffix
/// declines.
pub(crate) fn field_formal_to_normal(
    context: &AssistContext<'_>,
) -> Option<Vec<(u32, u32, String)>> {
    let text = context.text;
    let tokens = context.tokens;
    let ast = context.ast;
    let node = covering_node(tokens, text, ast, context.selection.start)?;
    let parameter = ancestor_or_self_kind(tokens, text, ast, node, NodeKind::FormalParameter)?;
    let span = node_span16(tokens, text, ast, parameter)?;
    // The `this.` head, no function-typed suffix (the token after the
    // name is not `(`).
    let this = token_at(tokens, span.start)?;
    if !matches!(tokens.get(this)?.kind, TokenKind::Keyword(_))
        || tokens.lexeme(text, this) != "this"
        || tokens.get(this.checked_add(1)?)?.kind != TokenKind::Period
    {
        return None;
    }
    let name = this + 2;
    if tokens.get(name)?.kind != TokenKind::Identifier {
        return None;
    }
    if tokens.get(name.checked_add(1)?)?.kind == TokenKind::OpenParen {
        return None;
    }
    let name_text = tokens.lexeme(text, name).to_owned();
    // The constructor owning the parameter list (the official's
    // ConstructorDeclaration branch; the primary-constructor branch is
    // the experiment-off face).
    let parameters = ast.parent(parameter)?;
    if ast.kind(parameters) != NodeKind::FormalParameters {
        return None;
    }
    let constructor = ast.parent(parameters)?;
    if ast.kind(constructor) != NodeKind::Constructor {
        return None;
    }
    // Named parameters inside a `{}` group rename to the public spelling.
    let is_named = direct_children(ast, parameters).into_iter().any(|child| {
        ast.kind(child) == NodeKind::OptionalFormalParameters
            && node_span16(tokens, text, ast, child)
                .is_some_and(|group| group.start <= span.start && span.end <= group.end)
    });
    let mut edits: Vec<(u32, u32, String)> = Vec::new();
    let mut parameter_name = name_text.clone();
    if is_named {
        if let Some(public) = name_text.strip_prefix('_') {
            if !public.is_empty() {
                let name_start = tokens.get(name)?.utf16_offset;
                let name_len = utf16_count(&name_text);
                edits.push((name_start, name_len, public.to_owned()));
                parameter_name = public.to_owned();
            }
        }
    }
    // The written type: present (the `this.` range deletes) or absent
    // (the field's type is the missing face — declines). The type (if
    // any) sits between the parameter head and `this`, modifiers aside.
    let this_start = tokens.get(this)?.utf16_offset;
    let mut has_type = false;
    {
        let mut index = token_at(tokens, span.start)?;
        loop {
            if tokens.get(index)?.utf16_offset >= this_start {
                break;
            }
            if !matches!(
                tokens.lexeme(text, index),
                "required" | "covariant" | "final" | "const"
            ) {
                has_type = true;
                break;
            }
            index += 1;
        }
    }
    if !has_type {
        return None;
    }
    let period_end = utf16_end(tokens, text, this + 1)?;
    edits.push((this_start, period_end - this_start, String::new()));
    // The initializer: after the last initializer (`,` prefix) or right
    // after the parameter list (` :` prefix).
    let initializers = child_of_kind(ast, constructor, NodeKind::Initializers);
    match initializers {
        Some(row) => {
            let last = direct_children(ast, row).into_iter().last()?;
            let last_end = node_span16(tokens, text, ast, last)?.end;
            edits.push((last_end, 0, format!(", {name_text} = {parameter_name}")));
        }
        None => {
            let list_close = parameters_close16(context, parameters)?;
            edits.push((list_close, 0, format!(" : {name_text} = {parameter_name}")));
        }
    }
    Some(sort_edits(edits))
}

/// The parameter list's closing `)` end offset.
fn parameters_close16(context: &AssistContext<'_>, parameters: u32) -> Option<u32> {
    let span = node_span16(context.tokens, context.text, context.ast, parameters)?;
    let close = token_at(context.tokens, span.end)?.checked_sub(1)?;
    utf16_end(context.tokens, context.text, close)
}

// ---------------------------------------------------------------------------
// dart.assist.convert.ifCaseStatement
// ---------------------------------------------------------------------------

/// The preceding single-variable declaration's parts: the statement-level
/// span start (its leading keyword chain included), the name, the
/// `final`-ness, and the initializer expression row.
struct PriorDeclaration {
    statement_start: u32,
    name_text: String,
    is_final: bool,
    initializer: core::ops::Range<u32>,
}

/// The statement before `if` in the same statement list, declaring exactly
/// one variable with an initializer (upstream `asSingleVariableDeclaration`).
fn prior_declaration(context: &AssistContext<'_>, if_statement: u32) -> Option<PriorDeclaration> {
    let text = context.text;
    let ast = context.ast;
    let parent = ast.parent(if_statement)?;
    // The shared statement list: a function body (`BlockBody`), a nested
    // compound (`Block`), or a switch member group (`SwitchCase`).
    if !matches!(
        ast.kind(parent),
        NodeKind::BlockBody | NodeKind::Block | NodeKind::SwitchCase
    ) {
        return None;
    }
    let siblings = statement_children(ast, parent);
    let index = siblings.iter().position(|&s| s == if_statement)?;
    let declaration = *siblings.get(index.checked_sub(1)?)?;
    if ast.kind(declaration) != NodeKind::VariablesDeclaration {
        return None;
    }
    let mut variables = direct_children(ast, declaration)
        .into_iter()
        .filter(|&child| ast.kind(child) == NodeKind::InitializedIdentifier);
    let variable = variables.next()?;
    if variables.next().is_some() {
        return None;
    }
    // The name and the `= expr` wrapper (the [`initializer_expr_row`]
    // payload walk for span-only wrappers). The name `Identifier` is a
    // direct child of the declaration row; the initializer rides the
    // `InitializedIdentifier`'s `VariableInitializer` child.
    let name = child_of_kind(ast, declaration, NodeKind::Identifier)?;
    let name_span = node_span16(context.tokens, text, ast, name)?;
    if name_span.start == name_span.end {
        return None;
    }
    let wrapper = child_of_kind(ast, variable, NodeKind::VariableInitializer)?;
    // The initializer text region: the trimmed run from the `=` to the
    // statement's `;` (the flat expression layer's rows tile oddly around
    // receivers — `a.x` keeps `a.` beside the `x` row — so the token
    // region, not a row, is the source of truth; a `;` inside a string
    // literal is part of that literal's token, never the terminator).
    let wrapper_span = node_span16(context.tokens, text, ast, wrapper)?;
    let mut index = token_at(context.tokens, wrapper_span.start)?;
    let mut eq_end = None;
    let mut semicolon = None;
    while let Some(token) = context.tokens.get(index) {
        if token.utf16_offset >= wrapper_span.end {
            break;
        }
        match context.tokens.lexeme(text, index) {
            "=" if eq_end.is_none() => {
                eq_end = Some(token_end16(context.tokens, text, index)?);
            }
            ";" => {
                semicolon = Some(token.utf16_offset);
                break;
            }
            _ => {}
        }
        index += 1;
    }
    let initializer = trim_span16(text, eq_end?..semicolon?)?;
    // The statement start: the leading `late`/`var`/`final`/`const` chain
    // (the declaration's span begins at the name).
    let mut statement_start = name_span.start;
    let mut is_final = false;
    let mut index = token_at(context.tokens, name_span.start)?;
    while let Some(prev) = index.checked_sub(1) {
        if token_end16(context.tokens, text, prev)? > statement_start {
            break;
        }
        match context.tokens.lexeme(text, prev) {
            "var" | "late" | "const" => index = prev,
            "final" => {
                is_final = true;
                index = prev;
            }
            _ => break,
        }
        statement_start = context.tokens.get(index)?.utf16_offset;
    }
    Some(PriorDeclaration {
        statement_start,
        name_text: slice16(text, &name_span)?.to_owned(),
        is_final,
        initializer,
    })
}

/// Whether `name` is referenced anywhere in the walked subtrees — the
/// resolved-light `_ReferenceVisitor`: every `Identifier` row whose
/// binding answers the local decl class of the same name. A mounted
/// semantic face is required (a `None` face declines the assist).
fn name_referenced_in(
    context: &AssistContext<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    name: &str,
    root: u32,
) -> bool {
    let mut spans = Vec::new();
    collect_identifier_spans(context, root, &mut spans);
    let wanted = analysis.strings().intern(name);
    spans.into_iter().any(|span| {
        analysis
            .binding_of(dartq_source::SourceRange {
                offset: span.start,
                length: span.end - span.start,
            })
            .is_some_and(|binding| {
                binding.kind == dartq_semantic::analysis::BindingKind::Local
                    && binding.target_name == wanted
            })
    })
}

/// Every `Identifier` row's span under `id` (the recursive walk stands in
/// for the upstream `RecursiveAstVisitor`).
fn collect_identifier_spans(
    context: &AssistContext<'_>,
    id: u32,
    out: &mut Vec<core::ops::Range<u32>>,
) {
    // Every `Identifier` row inside the subtree's text span: the flat
    // expression layer adopts operand rows *beside* — never beneath —
    // their containers (`y;` keeps its `Send` sibling outside the
    // statement row's window), so a child walk misses them and the span
    // is the ground truth.
    let Some(span) = node_span16(context.tokens, context.text, context.ast, id) else {
        return;
    };
    let Some(root) = context.ast.root() else {
        return;
    };
    for row in 0..=root {
        if context.ast.kind(row) != NodeKind::Identifier {
            continue;
        }
        if let Some(row_span) = node_span16(context.tokens, context.text, context.ast, row) {
            if row_span.start >= span.start
                && row_span.end <= span.end
                && row_span.start < row_span.end
            {
                out.push(row_span);
            }
        }
    }
}

/// `dart.assist.convert.ifCaseStatement` (`ConvertToIfCaseStatement`,
/// upstream Dart SDK source, pkg/analysis_server/lib/src/services/
/// correction/dart/convert_to_if_case_statement.dart): `var v = e; if (v
/// is T) {}` folds into `if (e case T v) {}` — the declaration deletes,
/// the initializer moves into the condition. The `!= null` face folds
/// into `var v?` (the `final` spelling rides the declaration's
/// `final`-ness). The reference gate (the variable must be unreferenced
/// in the else branch and after the `if`) reads through the resolved
/// local-binding face; the upstream `is!`-condition conversion (the
/// producer never checks the not-flag — the pattern binds the positive
/// type, mirroring the observable behavior verbatim) converts too.
pub(crate) fn if_case_statement(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    if !context.features.patterns {
        return None;
    }
    let text = context.text;
    let if_statement = statement_at(context)?;
    if context.ast.kind(if_statement) != NodeKind::IfStatement {
        return None;
    }
    let parts = if_parts(context, if_statement)?;
    let declaration = prior_declaration(context, if_statement)?;
    // The full condition region: the expression rows anchor oddly in the
    // flat layer (an `is` row anchors at its operator and seals before
    // type arguments), so the region rides from after the `(` to the
    // matching `)` — the upstream `range.node(condition)` face.
    let paren = child_of_kind(context.ast, if_statement, NodeKind::ParenthesizedCondition)?;
    let paren_span = node_span16(context.tokens, text, context.ast, paren)?;
    let open = token_at(context.tokens, paren_span.start)?;
    let close = context.tokens.matching_end(open)?;
    let condition_start = token_end16(context.tokens, text, open)?;
    let condition_end = context.tokens.get(close)?.utf16_offset;
    // The pattern legs, in the producer's order: `v is T`, then `v !=
    // null`.
    let pattern = match context.ast.kind(parts.condition) {
        NodeKind::IsExpression => {
            let (_, _, operator, left_span, _) = binary_parts(context, parts.condition)?;
            // The bare-identifier face: the left region IS the operand,
            // so its text matching the declared name is exactly the
            // upstream SimpleIdentifier check.
            if slice16(text, &left_span)? != declaration.name_text {
                return None;
            }
            let keyword = if declaration.is_final { "final " } else { "" };
            let op_end = utf16_end(context.tokens, text, operator)?;
            let type_text = slice16(text, &trim_span16(text, op_end..condition_end)?)?;
            format!("{keyword}{type_text} {}", declaration.name_text)
        }
        NodeKind::BinaryExpression => {
            let (_, right_root, operator, left_span, _) = binary_parts(context, parts.condition)?;
            let keyword = if declaration.is_final { "final" } else { "var" };
            if context.tokens.lexeme(text, operator) != "!="
                || context.ast.kind(right_root) != NodeKind::LiteralNull
                || slice16(text, &left_span)? != declaration.name_text
            {
                return None;
            }
            format!("{keyword} {}?", declaration.name_text)
        }
        _ => return None,
    };
    // The reference gate: the else branch and the statements after the
    // `if` must not name the variable (the then branch may — that is the
    // point of the promotion).
    let analysis = context.semantics()?;
    let parent = context.ast.parent(if_statement)?;
    let siblings = statement_children(context.ast, parent);
    let index = siblings.iter().position(|&s| s == if_statement)?;
    if let Some(else_id) = parts.else_id {
        if name_referenced_in(context, analysis, &declaration.name_text, else_id) {
            return None;
        }
    }
    for statement in &siblings[index + 1..] {
        if name_referenced_in(context, analysis, &declaration.name_text, *statement) {
            return None;
        }
    }
    // The rewrite: the declaration deletes, the condition becomes
    // `initializer case pattern`.
    let if_span = node_span16(context.tokens, text, context.ast, if_statement)?;
    let initializer_text = slice16(text, &declaration.initializer)?;
    Some(sort_edits(vec![
        (
            declaration.statement_start,
            if_span.start - declaration.statement_start,
            String::new(),
        ),
        (
            condition_start,
            condition_end.checked_sub(condition_start)?,
            format!("{initializer_text} case {pattern}"),
        ),
    ]))
}

// ---------------------------------------------------------------------------
// dart.assist.convert.ifCaseStatementChain
// ---------------------------------------------------------------------------

/// One chain group: the joined guarded-pattern text (`None` for the
/// default group) and the body statements.
struct ChainGroup {
    pattern_text: Option<String>,
    statements: Vec<u32>,
}

/// A case's body statements: the statement-kind children after the last
/// `case` head (joined/shared-body heads sit before them; a lone
/// `default`'s statements are all its statement-kind children; labels are
/// neither).
fn switch_case_statements(context: &AssistContext<'_>, case: u32) -> Vec<u32> {
    let children = direct_children(context.ast, case);
    let head_end = children
        .iter()
        .rposition(|&child| context.ast.kind(child) == NodeKind::CaseExpression);
    let start = head_end.map_or(0, |position| position + 1);
    children[start..]
        .iter()
        .copied()
        .filter(|&child| is_statement_kind(context.ast.kind(child)))
        .collect()
}

/// The guarded-pattern text of one `case` head: the head seals at its
/// colon (inclusive), so the pattern is the trimmed run between the
/// `case` keyword's end and the colon — riding the `when` clause when
/// written.
fn case_head_pattern_text(context: &AssistContext<'_>, head: u32) -> Option<String> {
    let head_span = node_span16(context.tokens, context.text, context.ast, head)?;
    let keyword_end = token_end16(
        context.tokens,
        context.text,
        token_at(context.tokens, head_span.start)?,
    )?;
    let span = trim_span16(context.text, keyword_end..head_span.end.checked_sub(1)?)?;
    Some(slice16(context.text, &span)?.to_owned())
}

/// The case groups: one `SwitchCase` row IS one upstream member group —
/// its payload's `default` keyword marks a default group (which must
/// stand alone), its `CaseExpression` children are the joined `case`
/// heads (`||`-joined upstream; a joined head carries no `when`). Any
/// other shape declines.
fn switch_chain_groups(context: &AssistContext<'_>, block: u32) -> Option<Vec<ChainGroup>> {
    let cases: Vec<u32> = direct_children(context.ast, block)
        .into_iter()
        .filter(|&child| context.ast.kind(child) == NodeKind::SwitchCase)
        .collect();
    if cases.is_empty() {
        return None;
    }
    let mut groups: Vec<ChainGroup> = Vec::with_capacity(cases.len());
    for &case in &cases {
        let has_default = context.ast.payload(case).first().copied() != Some(TOKEN_NONE);
        let statements = switch_case_statements(context, case);
        let heads: Vec<u32> = direct_children(context.ast, case)
            .into_iter()
            .filter(|&child| context.ast.kind(child) == NodeKind::CaseExpression)
            .collect();
        let pattern_text = if has_default {
            // A `default` sharing its member group with a pattern declines
            // (the upstream default group stands alone).
            if !heads.is_empty() {
                return None;
            }
            None
        } else {
            heads.first()?;
            let joined = heads.len() > 1;
            let mut texts = Vec::with_capacity(heads.len());
            for head in &heads {
                // Joined patterns carry no `when` clause.
                if joined
                    && child_of_kind(context.ast, *head, NodeKind::SwitchCaseWhenClause).is_some()
                {
                    return None;
                }
                texts.push(case_head_pattern_text(context, *head)?);
            }
            Some(texts.join(" || "))
        };
        groups.push(ChainGroup {
            pattern_text,
            statements,
        });
    }
    Some(groups)
}

/// `dart.assist.convert.ifCaseStatementChain`
/// (`ConvertToIfCaseStatementChain`, upstream Dart SDK source,
/// pkg/analysis_server/lib/src/services/correction/dart/
/// convert_to_if_case_statement_chain.dart): the switch statement whose
/// every group is a lone `default` or guarded pattern(s) rewrites to the
/// `if (e case p) {} else if …` chain — joined patterns `||`, the
/// default group the `else` block, trailing `break`s dropped, bodies
/// re-indented one level, the switch replaced whole.
pub(crate) fn if_case_statement_chain(
    context: &AssistContext<'_>,
) -> Option<Vec<(u32, u32, String)>> {
    let text = context.text;
    let node = covering_node(context.tokens, text, context.ast, context.selection.start)?;
    let switch = ancestor_or_self_kind(
        context.tokens,
        text,
        context.ast,
        node,
        NodeKind::SwitchStatement,
    )?;
    let block = child_of_kind(context.ast, switch, NodeKind::SwitchBlock)?;
    let groups = switch_chain_groups(context, block)?;
    let condition = child_of_kind(context.ast, switch, NodeKind::ParenthesizedCondition)?;
    let expression = inner_expression(context, condition)?;
    let expression_text = slice16(text, &condition_text_spans(context, expression)?)?;
    let switch_span = node_span16(context.tokens, text, context.ast, switch)?;
    let indent = line_indent_of(text, switch_span.start).to_owned();
    let eol = eol_of(text);
    let mut out = String::new();
    for (index, group) in groups.iter().enumerate() {
        if index > 0 {
            out.push_str(" else ");
        }
        match &group.pattern_text {
            Some(pattern) => {
                out.push_str(&format!("if ({expression_text} case {pattern}) {{"));
            }
            None => out.push('{'),
        }
        out.push_str(eol);
        // The trailing `break` drops (unnecessary for the switch, wrong
        // for the chain).
        let statements = match group.statements.split_last() {
            Some((last, rest)) if context.ast.kind(*last) == NodeKind::Break => rest,
            _ => &group.statements[..],
        };
        if let (Some(first), Some(last)) = (statements.first(), statements.last()) {
            let first_span = node_span16(context.tokens, text, context.ast, *first)?;
            let last_span = node_span16(context.tokens, text, context.ast, *last)?;
            let lines = slice16(
                text,
                &(line_start16(text, first_span.start)?..last_span.end),
            )?;
            let first_indent = line_indent_of(text, first_span.start);
            let new_indent = format!("{indent}  ");
            out.push_str(&reindent(lines, first_indent, &new_indent, eol, true));
        }
        out.push_str(&indent);
        out.push('}');
    }
    Some(vec![(
        switch_span.start,
        switch_span.end - switch_span.start,
        out,
    )])
}

/// The expression's trimmed span (the shared `getNodeText` shape).
fn condition_text_spans(
    context: &AssistContext<'_>,
    expression: u32,
) -> Option<core::ops::Range<u32>> {
    let span = node_span16(context.tokens, context.text, context.ast, expression)?;
    trim_span16(
        context.text,
        span.start..expression_end16(context.tokens, context.text, context.ast, expression)?,
    )
}

// ---------------------------------------------------------------------------
// dart.assist.convert.forEachToForIndex
// ---------------------------------------------------------------------------

/// The unprefixed-name conflict set (upstream
/// `findPossibleLocalVariableConflicts`): every identifier spelling in the
/// enclosing block. The official collector skips label names, constructor
/// names, and prefixed receivers; this walk keeps those too — the same
/// over-approximation direction (a busier block may skip an available
/// `i`/`j`/`k`), never a wrong offer.
fn local_variable_conflicts(context: &AssistContext<'_>, block: u32) -> FxHashSet<String> {
    let mut spans = Vec::new();
    collect_identifier_spans(context, block, &mut spans);
    spans
        .into_iter()
        .filter_map(|span| slice16(context.text, &span).map(str::to_owned))
        .collect()
}

/// `dart.assist.convert.forEachToForIndex` (`ConvertIntoForIndex`,
/// upstream Dart SDK source, pkg/analysis_server/lib/src/services/
/// correction/dart/convert_into_for_index.dart): `xs.forEach((e) …)` is
/// not this producer's face — it is the for-in loop whose iterable is a
/// `List`-typed variable: `for (var e in xs) {…}` becomes `for (int i =
/// 0; i < xs.length; i++) { var-marked bodies keep their element; the
/// element assignment `e = xs[i];` inserts at the block's first line`.
/// The iterable must be a bare identifier resolving to a variable of the
/// `List` class (the resolved face); the index name is the first free
/// `i`/`j`/`k` against the enclosing block's identifiers.
pub(crate) fn for_each_to_for_index(
    context: &AssistContext<'_>,
) -> Option<Vec<(u32, u32, String)>> {
    let text = context.text;
    let tokens = context.tokens;
    let node = covering_node(tokens, text, context.ast, context.selection.start)?;
    let for_in = ancestor_or_self_kind(tokens, text, context.ast, node, NodeKind::ForIn)?;
    let for_span = node_span16(tokens, text, context.ast, for_in)?;
    let parts = child_of_kind(context.ast, for_in, NodeKind::ForInParts)?;
    let lparen = payload_token(context.ast, parts, 1)?;
    let rparen = tokens.matching_end(lparen)?;
    let rparen_end = utf16_end(tokens, text, rparen)?;
    // The cursor sits inside the loop head (the official
    // `[forStatement.offset, rightParenthesis.end)` window).
    if context.selection.start < for_span.start || context.selection.start > rparen_end {
        return None;
    }
    // The loop declares its variable (`for (var e in …)` /
    // `for (T e in …)`), the `ForEachPartsWithDeclaration` face: the head
    // material is a `VariablesDeclaration` row, and the flat layer keeps
    // the iterable identifier beside it (ForInParts itself is a
    // token-payload leaf).
    let children = direct_children(context.ast, for_in);
    let body = *children.last()?;
    if context.ast.kind(body) != NodeKind::Block {
        return None;
    }
    let head_end = children
        .iter()
        .position(|&child| context.ast.kind(child) == NodeKind::ForInParts)?;
    let declaration = children[..head_end]
        .iter()
        .copied()
        .find(|&child| context.ast.kind(child) == NodeKind::VariablesDeclaration)?;
    let variable_name = child_of_kind(context.ast, declaration, NodeKind::Identifier)?;
    let name_span = node_span16(tokens, text, context.ast, variable_name)?;
    if name_span.start == name_span.end {
        return None;
    }
    // The declared form's full text — `var e` / `final e` / `T e` — the
    // run between the `(` and the name (the token run carries the
    // written type).
    let head_start = tokens.get(lparen)?.utf16_offset + 1;
    let variable_text = slice16(text, &trim_span16(text, head_start..name_span.end)?)?.to_owned();
    // The iterable: the bare identifier in the head run, resolving to a
    // `List`-typed variable (the upstream `staticType` gate alone — no
    // binding-kind filter).
    let iterable = children[..head_end]
        .iter()
        .copied()
        .filter(|&child| context.ast.kind(child) == NodeKind::Identifier)
        .next_back()?;
    let iterable_span = node_span16(tokens, text, context.ast, iterable)?;
    let analysis = context.semantics()?;
    let source = dartq_source::SourceRange {
        offset: iterable_span.start,
        length: iterable_span.end - iterable_span.start,
    };
    match analysis.type_of(source) {
        Some(dartq_semantic::model::Ty::Named { name, nullable, .. })
            if !nullable && analysis.strings().resolve(name) == "List" => {}
        _ => return None,
    }
    let list_text = slice16(text, &iterable_span)?.to_owned();
    // The index name: the first free `i`/`j`/`k` (the upstream enclosing
    // block is a function body `BlockBody` or a nested `Block` — the
    // analyzer shapes it as one `Block` node).
    let block = (|| {
        let mut current = for_in;
        loop {
            current = context.ast.parent(current)?;
            if matches!(
                context.ast.kind(current),
                NodeKind::Block | NodeKind::BlockBody
            ) {
                return Some(current);
            }
        }
    })()?;
    let conflicts = local_variable_conflicts(context, block);
    let index_name = ["i", "j", "k"]
        .into_iter()
        .find(|name| !conflicts.contains(*name))?;
    // The insertion point: the first line inside the block (the official
    // `getLineContentEnd(leftBracket.end)` walk — trailing whitespace,
    // `\r`, `\n` skipped).
    let body_span = node_span16(tokens, text, context.ast, body)?;
    let brace_end = token_end16(tokens, text, token_at(tokens, body_span.start)?)?;
    if brace_end > body_span.start + 1 {
        return None;
    }
    let mut byte = dartq_source::utf16_offset_to_byte(text, brace_end);
    let bytes = text.as_bytes();
    while bytes.get(byte).is_some_and(|c| matches!(c, b' ' | b'\t')) {
        byte += 1;
    }
    if bytes.get(byte) == Some(&b'\r') {
        byte += 1;
    }
    if bytes.get(byte) == Some(&b'\n') {
        byte += 1;
    }
    let insert_at = dartq_source::byte_offset_to_utf16(text, byte);
    let prefix = line_indent_of(text, for_span.start);
    let header =
        format!("for (int {index_name} = 0; {index_name} < {list_text}.length; {index_name}++)");
    let insertion = format!(
        "{prefix}  {variable_text} = {list_text}[{index_name}];{}",
        eol_of(text)
    );
    Some(sort_edits(vec![
        (for_span.start, rparen_end - for_span.start, header),
        (insert_at, 0, insertion),
    ]))
}

// ---------------------------------------------------------------------------
// dart.assist.convert.switchStatement — the if-chain face
// ---------------------------------------------------------------------------

/// One built switch case: the pattern text (empty for the `default`)
/// and the body statement.
struct BuiltCase {
    pattern: String,
    is_default: bool,
    statement: u32,
}

/// The body's statement list (`selfOrBlockStatements`).
fn self_or_block_statements(context: &AssistContext<'_>, statement: u32) -> Vec<u32> {
    if context.ast.kind(statement) == NodeKind::Block {
        statement_children(context.ast, statement)
    } else {
        vec![statement]
    }
}

/// Whether the type text names a non-interface shape for the
/// `is`-pattern decision: record types (the `(` head) and function types
/// (the `Function` construct) carry the `x _` pattern; everything else
/// named is undecided here (the interface/alias split is the missing
/// face) and declines.
fn is_pattern_suffix_wildcard(
    context: &AssistContext<'_>,
    type_span: &core::ops::Range<u32>,
) -> Option<bool> {
    let text = context.text;
    let trimmed = trim_span16(text, type_span.clone())?;
    let head = slice16(text, &(trimmed.start..trimmed.start + 1))?;
    if head == "(" {
        return Some(true);
    }
    // The `Function` construct: a `Function` word followed by `<` or `(`.
    let close = token_at(context.tokens, trimmed.end).unwrap_or(context.tokens.len());
    let mut index = token_at(context.tokens, trimmed.start)?;
    while index < close {
        if word_at(context.tokens, text, index, "Function") {
            match context.tokens.get(index + 1).map(|t| t.kind) {
                Some(TokenKind::Lt) | Some(TokenKind::OpenParen) => return Some(true),
                _ => return Some(false),
            }
        }
        index += 1;
    }
    // `dynamic` is no interface type (the `dynamic _` pattern).
    let head_lexeme = context
        .tokens
        .lexeme(text, token_at(context.tokens, trimmed.start)?);
    if head_lexeme == "dynamic" {
        return Some(true);
    }
    // A type-parameter head is no interface type either.
    let analysis = context.semantics()?;
    if analysis
        .binding_of(dartq_source::SourceRange {
            offset: trimmed.start,
            length: trimmed.end - trimmed.start,
        })
        .is_some_and(|binding| binding.kind == dartq_semantic::analysis::BindingKind::TypeParam)
    {
        return Some(true);
    }
    None
}

/// The if-chain cases (upstream `_buildCases`): the then branch's
/// condition reads as a pattern (`e case p` clause, `!= null`, a
/// relational against a literal, or `is T` with a lexically decided
/// non-interface type), the else recurses through `else if` with every
/// expression spelling the first's, and a non-if else becomes the
/// `default`. Returns the cases with the shared expression text.
#[allow(clippy::type_complexity)]
fn build_switch_cases(
    context: &AssistContext<'_>,
    if_statement: u32,
    first_expression: Option<&str>,
) -> Option<(Vec<BuiltCase>, String)> {
    let text = context.text;
    let tokens = context.tokens;
    let ast = context.ast;
    let parts = if_parts(context, if_statement)?;
    let condition = parts.condition;
    let (expression_text, pattern) = if parts.has_case {
        // `e case p`: the clause splits the parenthesized condition at its
        // depth-0 `case` keyword — the leading run is the expression, the
        // trailing run the guarded pattern. The expression is a bare
        // identifier unless this is the chain's last link.
        let else_is_if = parts
            .else_id
            .is_some_and(|else_id| ast.kind(else_id) == NodeKind::IfStatement);
        let paren = child_of_kind(ast, if_statement, NodeKind::ParenthesizedCondition)?;
        let paren_span = node_span16(tokens, text, ast, paren)?;
        let open = token_at(tokens, paren_span.start)?;
        let close = tokens.matching_end(open)?;
        let case_kw = depth0_find(tokens, text, open + 1, close, |_, lexeme| lexeme == "case")?;
        let expression_span = trim_span16(
            text,
            tokens.get(open)?.utf16_offset + 1..tokens.get(case_kw)?.utf16_offset,
        )?;
        let pattern_span = trim_span16(
            text,
            token_end16(tokens, text, case_kw)?..tokens.get(close)?.utf16_offset,
        )?;
        let bare = covering_node(tokens, text, ast, expression_span.start)
            .filter(|node| ast.kind(*node) == NodeKind::Identifier)
            .and_then(|node| node_span16(tokens, text, ast, node))
            .is_some_and(|span| span == expression_span);
        if !bare && else_is_if {
            return None;
        }
        (
            slice16(text, &expression_span)?.to_owned(),
            slice16(text, &pattern_span)?.to_owned(),
        )
    } else {
        pattern_of_bool_condition(context, condition)?
    };
    match first_expression {
        Some(first) if first != expression_text => return None,
        _ => {}
    }
    let mut cases = vec![BuiltCase {
        pattern,
        is_default: false,
        statement: parts.then_id,
    }];
    match parts.else_id {
        Some(else_id) if ast.kind(else_id) == NodeKind::IfStatement => {
            let first = first_expression.unwrap_or(&expression_text);
            let (nested, _) = build_switch_cases(context, else_id, Some(first))?;
            cases.extend(nested);
        }
        Some(else_id) => cases.push(BuiltCase {
            pattern: String::new(),
            is_default: true,
            statement: else_id,
        }),
        None => {}
    }
    Some((cases, expression_text))
}

/// The bool-condition pattern faces (upstream `_patternOfBoolCondition`):
/// `x != null` → `_?`; a relational against a literal → the operator and
/// literal; `x is T` → `T()` for interface heads — decided here as the
/// record/function/dynamic/type-parameter wildcards, the alias face
/// declining (fewer assists, never an approximated one).
fn pattern_of_bool_condition(
    context: &AssistContext<'_>,
    condition: u32,
) -> Option<(String, String)> {
    let text = context.text;
    match context.ast.kind(condition) {
        NodeKind::BinaryExpression => {
            let (_, right_root, operator, left_span, right_span) =
                binary_parts(context, condition)?;
            let operator_text = context.tokens.lexeme(text, operator);
            if operator_text == "!=" && context.ast.kind(right_root) == NodeKind::LiteralNull {
                return Some((slice16(text, &left_span)?.to_owned(), "_?".to_owned()));
            }
            if matches!(operator_text, "<" | ">" | "<=" | ">=")
                && matches!(
                    context.ast.kind(right_root),
                    NodeKind::LiteralInt
                        | NodeKind::LiteralDouble
                        | NodeKind::LiteralBool
                        | NodeKind::StringLiteral
                        | NodeKind::LiteralNull
                )
            {
                let op_start = context.tokens.get(operator)?.utf16_offset;
                let pattern = slice16(text, &(op_start..right_span.end))?.to_owned();
                return Some((slice16(text, &left_span)?.to_owned(), pattern));
            }
            None
        }
        NodeKind::IsExpression => {
            let (_, _, _, expression_span, type_span) = binary_parts(context, condition)?;
            let suffix = match is_pattern_suffix_wildcard(context, &type_span)? {
                true => " _",
                false => "()",
            };
            Some((
                slice16(text, &expression_span)?.to_owned(),
                format!("{}{suffix}", slice16(text, &type_span)?),
            ))
        }
        _ => None,
    }
}

/// The if-chain leg of `dart.assist.convert.switchStatement`
/// (`ConvertIfStatementToSwitchStatement`, upstream Dart SDK source,
/// pkg/analysis_server/lib/src/services/correction/dart/
/// convert_to_switch_statement.dart): the case chain rewrites to the
/// switch statement — bodies re-indented under their labels, empty
/// non-last branches carrying `break;`, the if replaced whole.
fn convert_if_chain_to_switch(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    if !context.features.patterns {
        return None;
    }
    let text = context.text;
    let if_statement = statement_at(context)?;
    if context.ast.kind(if_statement) != NodeKind::IfStatement {
        return None;
    }
    let (cases, expression_text) = build_switch_cases(context, if_statement, None)?;
    let if_span = node_span16(context.tokens, text, context.ast, if_statement)?;
    let indent = line_indent_of(text, if_span.start).to_owned();
    let case_indent = format!("{indent}  ");
    let statement_indent = format!("{case_indent}  ");
    let eol = eol_of(text);
    let mut out = format!("switch ({expression_text}) {{");
    out.push_str(eol);
    for (index, case) in cases.iter().enumerate() {
        let is_last = index + 1 == cases.len();
        if case.is_default {
            out.push_str(&format!("{case_indent}default:"));
        } else {
            out.push_str(&format!("{case_indent}case {}:", case.pattern));
        }
        out.push_str(eol);
        let statements = self_or_block_statements(context, case.statement);
        if statements.is_empty() {
            if !is_last {
                out.push_str(&format!("{statement_indent}break;"));
                out.push_str(eol);
            }
            continue;
        }
        let first = node_span16(context.tokens, text, context.ast, *statements.first()?)?;
        let last = node_span16(context.tokens, text, context.ast, *statements.last()?)?;
        let lines = slice16(text, &(line_start16(text, first.start)?..last.end))?;
        let first_indent = line_indent_of(text, first.start);
        out.push_str(&reindent(lines, first_indent, &statement_indent, eol, true));
    }
    out.push_str(&indent);
    out.push('}');
    Some(vec![(if_span.start, if_span.end - if_span.start, out)])
}

// ---------------------------------------------------------------------------
// dart.assist.convert.switchStatement — the switch-expression face
// ---------------------------------------------------------------------------

/// The case rewrites shared by the three container faces (upstream
/// `_rewriteCases`): the pure untyped `_` pattern becomes `default`,
/// every other pattern gains the `case ` prefix, the `=>` region becomes
/// `:` plus the re-indented `{prefix}` line, the trailing comma becomes
/// `;` (or the `;` inserts), and the statement's own `;` deletes.
fn rewrite_switch_expression_cases(
    context: &AssistContext<'_>,
    block: u32,
    indent: &str,
    prefix: &str,
) -> Option<Vec<(u32, u32, String)>> {
    let text = context.text;
    let tokens = context.tokens;
    let mut edits: Vec<(u32, u32, String)> = Vec::new();
    for case in direct_children(context.ast, block)
        .into_iter()
        .filter(|&child| context.ast.kind(child) == NodeKind::SwitchExpressionCase)
    {
        let mut children: Vec<u32> = direct_children(context.ast, case).into_iter().collect();
        let expression = children.pop()?;
        // The guarded pattern: adopted type rows precede the head (a
        // `NoType` row stands for the absent written type, a real `Type`
        // row when written) and an optional `when` clause follows it. The
        // guarded end (the last child before the expression) anchors the
        // `=>`-region rewrite, keeping any `when` text (upstream rewrites
        // `range.endStart(guardedPattern, case_.expression)`).
        let mut written_type = false;
        let mut head = None;
        for &child in &children {
            match context.ast.kind(child) {
                NodeKind::NoType => {}
                NodeKind::Type => written_type = true,
                _ => {
                    head = Some(child);
                    break;
                }
            }
        }
        let pattern = head?;
        let pattern_span = node_span16(tokens, text, context.ast, pattern)?;
        // The guarded pattern's own start: the first adopted row (a
        // written type leads it) or the head — upstream inserts at
        // `guardedPattern.offset`.
        let guarded_start = node_span16(tokens, text, context.ast, children[0])?.start;
        let has_when = context.ast.payload(case).first().copied() != Some(TOKEN_NONE);
        // The pure untyped wildcard: no written type and no `when`
        // (upstream `GuardedPattern.isPureUntypedWildcard`); the typed
        // `X _` face keeps the `case ` prefix.
        let is_pure_wildcard =
            !written_type && !has_when && context.ast.kind(pattern) == NodeKind::WildcardPattern;
        if is_pure_wildcard {
            edits.push((
                pattern_span.start,
                pattern_span.end - pattern_span.start,
                "default".to_owned(),
            ));
        } else {
            edits.push((guarded_start, 0, "case ".to_owned()));
        }
        let guarded_end = node_span16(tokens, text, context.ast, *children.last()?)?.end;
        let expression_span = node_span16(tokens, text, context.ast, expression)?;
        let expression_end = expression_end16(tokens, text, context.ast, expression)?;
        edits.push((
            guarded_end,
            expression_span.start.checked_sub(guarded_end)?,
            format!(":\n{indent}    {prefix}"),
        ));
        // The trailing comma becomes `;`; absent, the `;` inserts (the
        // case's span ends at the follower — the comma or the block's
        // `}`).
        let case_end = node_span16(tokens, text, context.ast, case)?.end;
        let comma = token_at(tokens, case_end)?;
        let comma16 = tokens.get(comma)?.utf16_offset;
        if tokens.lexeme(text, comma) == "," {
            edits.push((comma16, 1, ";".to_owned()));
        } else {
            edits.push((expression_end, 0, ";".to_owned()));
        }
    }
    Some(edits)
}

/// The statement's `;` right after the switch expression.
fn trailing_semicolon_edit(
    context: &AssistContext<'_>,
    expression: u32,
) -> Option<(u32, u32, String)> {
    let end = expression_end16(context.tokens, context.text, context.ast, expression)?;
    let index = token_at(context.tokens, end)?;
    if context.tokens.lexeme(context.text, index) != ";" {
        return None;
    }
    let at = context.tokens.get(index)?.utf16_offset;
    Some((at, 1, String::new()))
}

/// The switch-expression leg of `dart.assist.convert.switchStatement`
/// (`ConvertSwitchExpressionToSwitchStatement`, upstream Dart SDK
/// source, pkg/analysis_server/lib/src/services/correction/dart/
/// convert_to_switch_statement.dart): the container faces are the return
/// statement, the single-variable declaration (the written type is kept —
/// the `writeType` inference face is absent, so `var`/`final`-untyped
/// forms decline), and the plain identifier assignment; each becomes the
/// switch statement with per-case `{prefix}` lines.
fn convert_switch_expression_to_switch(
    context: &AssistContext<'_>,
) -> Option<Vec<(u32, u32, String)>> {
    let text = context.text;
    let tokens = context.tokens;
    let node = covering_node(tokens, text, context.ast, context.selection.start)?;
    let switch_expression =
        ancestor_or_self_kind(tokens, text, context.ast, node, NodeKind::SwitchExpression)?;
    let switch_span = node_span16(tokens, text, context.ast, switch_expression)?;
    let block = child_of_kind(
        context.ast,
        switch_expression,
        NodeKind::SwitchExpressionBlock,
    )?;
    if direct_children(context.ast, block).is_empty() {
        return None;
    }
    let indent = line_indent_of(text, switch_span.start).to_owned();
    let mut edits: Vec<(u32, u32, String)> = Vec::new();
    // The container walk: the switch expression's parent decides.
    let parent = context.ast.parent(switch_expression)?;
    let prefix;
    if context.ast.kind(parent) == NodeKind::ReturnStatement {
        // `return switch…;` — the `return ` prefix deletes.
        let return_span = node_span16(tokens, text, context.ast, parent)?;
        let keyword = token_at(tokens, return_span.start)?;
        if tokens.lexeme(text, keyword) != "return" {
            return None;
        }
        let keyword_start = tokens.get(keyword)?.utf16_offset;
        edits.push((
            keyword_start,
            switch_span.start - keyword_start,
            String::new(),
        ));
        prefix = "return ".to_owned();
    } else {
        // The initializer wrappers climb to the declaration.
        let mut variable = None;
        if context.ast.kind(parent) == NodeKind::VariableInitializer {
            if let Some(initialized) = context.ast.parent(parent) {
                if context.ast.kind(initialized) == NodeKind::InitializedIdentifier {
                    if let Some(declaration) = context.ast.parent(initialized) {
                        if context.ast.kind(declaration) == NodeKind::VariablesDeclaration {
                            variable = Some((initialized, declaration));
                        }
                    }
                }
            }
        }
        if let Some((_, declaration)) = variable {
            // Exactly one variable, with the type written (the inference
            // face declines).
            let mut variables = direct_children(context.ast, declaration)
                .into_iter()
                .filter(|&child| context.ast.kind(child) == NodeKind::InitializedIdentifier);
            let _ = variables.next()?;
            if variables.next().is_some() {
                return None;
            }
            // The name rides the declaration level (the adopted rows: the
            // identifier lands beside the initializer wrapper).
            let name = child_of_kind(context.ast, declaration, NodeKind::Identifier)?;
            let name_span = node_span16(tokens, text, context.ast, name)?;
            // The written type: the declaration's span starts at the name
            // (the adopted type row sits outside it), so the token before
            // the name decides — a declaration keyword is the untyped
            // `var`/`final` face, which declines (the writeType inference
            // face is absent).
            let prev = token_at(tokens, name_span.start)?.checked_sub(1)?;
            if matches!(
                tokens.lexeme(text, prev),
                "var" | "final" | "const" | "late"
            ) {
                return None;
            }
            let name_text = slice16(text, &name_span)?.to_owned();
            edits.push((
                name_span.end,
                switch_span.start - name_span.end,
                format!(";\n{indent}"),
            ));
            prefix = format!("{name_text} = ");
        } else if context.ast.kind(parent) == NodeKind::Assignment {
            // `x = switch…;` — the plain-identifier left side.
            let mut children = direct_children(context.ast, parent).into_iter();
            let lhs = children.next()?;
            let rhs = children.next_back()?;
            if rhs != switch_expression || context.ast.kind(lhs) != NodeKind::Identifier {
                return None;
            }
            let grandparent = context.ast.parent(parent)?;
            if context.ast.kind(grandparent) != NodeKind::ExpressionStatement {
                return None;
            }
            let lhs_span = node_span16(tokens, text, context.ast, lhs)?;
            let operator = context.ast.payload(parent).first().copied()?;
            if operator == TOKEN_NONE {
                return None;
            }
            let operator_text = tokens.lexeme(text, operator as usize);
            let lhs_text = slice16(text, &lhs_span)?.to_owned();
            edits.push((
                lhs_span.start,
                switch_span.start - lhs_span.start,
                String::new(),
            ));
            prefix = format!("{lhs_text} {operator_text} ");
        } else {
            return None;
        }
    }
    edits.extend(rewrite_switch_expression_cases(
        context, block, &indent, &prefix,
    )?);
    edits.push(trailing_semicolon_edit(context, switch_expression)?);
    Some(sort_edits(edits))
}

/// `dart.assist.convert.switchStatement` — both faces (the shared
/// `DartAssistKind.convertToSwitchStatement`): the if-chain and the
/// switch-expression containers.
pub(crate) fn convert_to_switch_statement(
    context: &AssistContext<'_>,
) -> Option<Vec<(u32, u32, String)>> {
    convert_if_chain_to_switch(context).or_else(|| convert_switch_expression_to_switch(context))
}

// ---------------------------------------------------------------------------
// dart.assist.convert.packageToRelativeImport
// ---------------------------------------------------------------------------

/// The slash segments of a relative path, empties dropped.
fn path_segments(value: &str) -> Vec<&str> {
    value.split('/').filter(|s| !s.is_empty()).collect()
}

/// The path's first segment (the package name of a `package:` URI path).
fn first_segment(value: &str) -> &str {
    value.split('/').next().unwrap_or("")
}

/// The POSIX `path.relative(target, from)`: the shared prefix drops, the
/// `from` remainder becomes `..` steps.
fn posix_relative(target: &str, from: &str) -> String {
    let target = path_segments(target);
    let from = path_segments(from);
    let mut common = 0;
    while common < target.len() && common < from.len() && target[common] == from[common] {
        common += 1;
    }
    let mut parts: Vec<&str> = vec![".."; from.len() - common];
    parts.extend(target[common..].iter().copied());
    if parts.is_empty() {
        return ".".to_owned();
    }
    parts.join("/")
}

/// `dart.assist.convert.packageToRelativeImport`
/// (`ConvertToRelativeImport`, upstream Dart SDK source,
/// pkg/analysis_server/lib/src/services/correction/dart/
/// convert_to_relative_import.dart): the `package:` import resolving
/// within the same package rewrites to the relative path — the written
/// URI's path taken against the unit's own directory, the quotes kept.
/// The resolution face is the unit's own `package:` library URI plus the
/// same-package spelling; the part-file directory (the unit's own URI
/// versus the library's) declines, as do interpolated and
/// non-simple-quote literals.
pub(crate) fn package_to_relative_import(
    context: &AssistContext<'_>,
) -> Option<Vec<(u32, u32, String)>> {
    let text = context.text;
    let ast = context.ast;
    let node = covering_node(context.tokens, text, ast, context.selection.start)?;
    let import = ancestor_or_self_kind(context.tokens, text, ast, node, NodeKind::Import)?;
    let uri = child_of_kind(ast, import, NodeKind::StringLiteral)?;
    let uri_span = node_span16(context.tokens, text, ast, uri)?;
    // The simple single-quote faces (the expanded-by--1 range keeps the
    // quote characters in place).
    let quote = slice16(text, &(uri_span.start..uri_span.start + 1))?;
    if quote != "'" && quote != "\"" || uri_span.end <= uri_span.start + 1 {
        return None;
    }
    let content = slice16(text, &(uri_span.start + 1..uri_span.end - 1))?;
    let target = content.strip_prefix("package:")?;
    if target.contains('$') {
        return None;
    }
    // The unit's own package attribution; a `part of` file carries the
    // library's URI, not its own directory — declines.
    let analysis = context.semantics()?;
    let own = analysis.library_uri();
    let own_path = own.strip_prefix("package:")?;
    let root = ast.root()?;
    if direct_children(ast, root)
        .into_iter()
        .any(|child| ast.kind(child) == NodeKind::PartOf)
    {
        return None;
    }
    if first_segment(target).is_empty() || first_segment(target) != first_segment(own_path) {
        return None;
    }
    let own_dir = own_path.rsplit_once('/').map_or("", |(dir, _)| dir);
    Some(vec![(
        uri_span.start + 1,
        uri_span.end - 1 - (uri_span.start + 1),
        posix_relative(target, own_dir),
    )])
}
