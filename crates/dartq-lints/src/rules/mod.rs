//! The rules, grouped by theme. Each `SPEC` cites its official
//! implementation (`pkg/linter/lib/src/rules/<rule>.dart`, Dart 3.15 main)
//! and the generated message template (`pkg/linter/lib/src/diagnostic.g.dart`).
//!
//! Selection criteria: rules whose observable behavior is pinned by the
//! syntax-level substrate (arena AST + token table + comments). A rule whose
//! official form leans on the element model or flow analysis may still port
//! when the lexical face matches on valid code — the approximation and its
//! divergences are documented on the rule. Rules not portable at all are
//! listed in the crate docs with their blocker.

pub(crate) mod collections;
pub(crate) mod const_ability;
pub(crate) mod declarations;
pub(crate) mod directives;
pub(crate) mod docs;
pub(crate) mod empty;
pub(crate) mod expressions;
pub(crate) mod flow;
pub(crate) mod flutter;
pub(crate) mod names;
pub(crate) mod pubspec;
pub(crate) mod regexp;
pub(crate) mod removed;
pub(crate) mod slice;
pub(crate) mod strings;
pub(crate) mod types;

use dartq_ast::{NodeKind, PayloadField};
use dartq_scanner::KeywordStyle;
use rustc_hash::FxHashMap;

use crate::engine::Check;

/// The check of a seam-mounted rule: its body is the official verdict as a
/// pure function over facts [`Unit`] does not carry (pubspec data, package
/// identity, element-model constant values), so the traversal never
/// dispatches it — the [`RuleSpec`] sits in [`crate::RULES`] for
/// name/group/count alignment with `kinds: &[]`, and the mounting ticket
/// (#434) owns the wiring. See `rules::pubspec` and the per-rule
/// seam-posture notes.
pub(crate) fn not_dispatched(_check: &mut Check<'_>) {}

/// First direct child of `id`, if any. A child's subtree occupies
/// `[children(child).start, child]` (a childless child's subtree is the
/// child itself), so the first child is the candidate whose subtree starts
/// exactly at the window start; candidates descend by whole subtrees.
pub(crate) fn first_child(c: &Check<'_>, id: u32) -> Option<u32> {
    let window = c.children(id);
    (window.start < window.end).then(|| {
        let mut i = window.end - 1;
        loop {
            let subtree_start = c.children(i).start;
            if subtree_start <= window.start {
                return i;
            }
            i = subtree_start - 1;
        }
    })
}

/// Last direct child of `id`, if any — the window's last entry (a node
/// completes after its whole subtree).
pub(crate) fn last_child(c: &Check<'_>, id: u32) -> Option<u32> {
    let window = c.children(id);
    (window.start < window.end).then(|| window.end - 1)
}

/// The rows whose parent pointer is `owner`, ascending — the full child set
/// when the container's completion window opens after earlier members filled
/// (the production `DeclarationBody` begins at the first non-field member, so
/// a leading field's rows predate its window and the parent pointer is the
/// only link back).
///
/// Backed by the run's whole-arena owner grouping
/// ([`RunCaches`][crate::engine::RunCaches] `owner_rows`): one linear pass on
/// first query, then one lookup per owner (#2467: each query filtered the
/// whole arena, one parent walk per row).
#[expect(
    clippy::cast_possible_truncation,
    reason = "arena row space is u32 by the AST table's payload width"
)]
pub(crate) fn rows_with_parent(c: &Check<'_>, owner: u32) -> Vec<u32> {
    if c.caches.owner_rows.borrow().is_none() {
        let mut index: FxHashMap<u32, Vec<u32>> = FxHashMap::default();
        for row in 0..c.unit.ast.len() as u32 {
            if let Some(parent) = c.unit.ast.parent(row) {
                index.entry(parent).or_default().push(row);
            }
        }
        *c.caches.owner_rows.borrow_mut() = Some(index);
    }
    c.caches
        .owner_rows
        .borrow()
        .as_ref()
        .expect("owner index built above")
        .get(&owner)
        .cloned()
        .unwrap_or_default()
}

/// Whether a block-window row delimits its statement (the flat tiling
/// keeps one statement row per statement, trailing its expression rows).
pub(crate) fn is_statement_row(c: &Check<'_>, row: u32) -> bool {
    matches!(
        c.kind(row),
        NodeKind::Block
            | NodeKind::LabeledStatement
            | NodeKind::IfStatement
            | NodeKind::ForStatement
            | NodeKind::ForIn
            | NodeKind::WhileStatement
            | NodeKind::DoWhileStatement
            | NodeKind::SwitchStatement
            | NodeKind::TryStatement
            | NodeKind::ReturnStatement
            | NodeKind::RethrowStatement
            | NodeKind::YieldStatement
            | NodeKind::InvalidYieldStatement
            | NodeKind::Assert
            | NodeKind::Break
            | NodeKind::Continue
            | NodeKind::EmptyStatement
            | NodeKind::ExpressionStatement
            | NodeKind::PatternVarDeclStatement
            | NodeKind::InvalidStatement
            | NodeKind::VariablesDeclaration
            | NodeKind::LocalFunctionDeclaration
    )
}

/// The member rows of a class-like `ancestor`: the body row's direct
/// children (the production body opens at `{` before all members — every
/// member is a body child; hand-built tables may skip the wrapper, hence
/// the `ancestor` fallback).
///
/// Memoized per row in [`RunCaches::body_member_rows`] — the override-family
/// rules re-derive the identical list per member, and the parent-pointer
/// face filters the arena (#758).
pub(crate) fn body_members(c: &Check<'_>, ancestor: u32) -> Vec<u32> {
    if let Some(rows) = c.caches.body_member_rows.borrow().get(&ancestor) {
        return rows.clone();
    }
    let body = c
        .collect_direct(ancestor)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::DeclarationBody)
        .unwrap_or(ancestor);
    let rows = rows_with_parent(c, body);
    c.caches
        .body_member_rows
        .borrow_mut()
        .insert(ancestor, rows.clone());
    rows
}

/// The written name of a class-like declaration row: the production
/// wrapper keeps the name `Identifier` as the
/// row's left sibling; hand-built trees nest it inside the row.
pub(crate) fn class_declaration_name<'a>(c: &'a Check<'_>, class_row: u32) -> Option<&'a str> {
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
        return Some(c.lexeme(c.head_token(name)));
    }
    let parent = c.unit.ast.parent(class_row)?;
    let siblings = c.collect_direct(parent);
    let pos = siblings.iter().position(|&kid| kid == class_row)?;
    let name = siblings[..pos]
        .iter()
        .rev()
        .copied()
        .find(|&kid| is_class_name(kid))?;
    Some(c.lexeme(c.head_token(name)))
}

/// The name-bearing row of a `Method`/`TopLevelMethod` declaration — the
/// direct `Identifier` child stamped `MethodDeclaration`/
/// `TopLevelFunctionDeclaration` by the bridge's `handle_identifier`, or the
/// `OperatorName` of an operator (a direct child on hand-built tables, the
/// member's immediate left sibling on production parses).
///
/// `Method`/`TopLevelMethod`-only semantics: the production tiling places a
/// written return type's `Identifier` (a `TypeReference` leaf) INSIDE the
/// member window BEFORE the name row, so an unfiltered first-`Identifier`
/// read answers the type (`int f()` reads `int`). Local/named functions,
/// `InitializedIdentifier` rows, and `FunctionName` wrappers are out of
/// contract — those read through
/// [`declarations::member_declaration_name`], which adds the
/// `LocalFunctionDeclaration` stamp and the `FunctionName` sub-row arm.
pub(crate) fn member_name(c: &Check<'_>, parent: u32) -> Option<u32> {
    if !matches!(c.kind(parent), NodeKind::Method | NodeKind::TopLevelMethod) {
        return None;
    }
    c.direct_children(parent)
        .iter()
        .copied()
        .find(|&child| match c.kind(child) {
            NodeKind::Identifier => matches!(
                c.field(child, PayloadField::Context),
                Some(ctx)
                    if ctx == dartq_parser::IdentifierContext::MethodDeclaration as u32
                        || ctx == dartq_parser::IdentifierContext::TopLevelFunctionDeclaration
                            as u32
                        // Hand-built test tables stamp every identifier with
                        // the generic `Expression` word (`testsupport::
                        // identifier`); a `TypeReference` leaf is the only
                        // non-name `Identifier` a production member window
                        // carries, so the generic word stays a name read.
                        || ctx == dartq_parser::IdentifierContext::Expression as u32
            ),
            NodeKind::OperatorName => true,
            _ => false,
        })
        .or_else(|| {
            // Production parses tile the `OperatorName` as the member's
            // immediate left sibling inside the body wrapper, not as a
            // child (declarations/ `adjacent_operator_name`).
            let owner = c.unit.ast.parent(parent)?;
            let siblings = c.collect_direct(owner);
            let pos = siblings.iter().position(|&kid| kid == parent)?;
            let prev = siblings.get(pos.checked_sub(1)?)?;
            (c.kind(*prev) == NodeKind::OperatorName).then_some(*prev)
        })
}

/// The name token of an [`NodeKind::InitializedIdentifier`] row: the
/// production row is width-0 at the name (bridge.rs
/// `begin_initialized_identifier` begins at the name — the name is the span
/// head), so the token itself answers; hand-built tables nest an
/// `Identifier` row and keep the same span head. Built-in identifiers
/// (`set`, `get`, …) lex as `Keyword` tokens of [`KeywordStyle::BuiltIn`]
/// yet bind as plain names (a field named `set` is legal Dart), so they
/// answer too. `None` otherwise (reserved words, literals — recovery
/// shapes).
pub(crate) fn initialized_identifier_name(c: &Check<'_>, var: u32) -> Option<u32> {
    let head = c.head_token(var);
    let token = c.unit.tokens.get(head as usize)?;
    match token.kind {
        dartq_scanner::TokenKind::Identifier => Some(head),
        dartq_scanner::TokenKind::Keyword(kw) if kw.style() == KeywordStyle::BuiltIn => Some(head),
        _ => None,
    }
}

/// The `var`/`final`/`const` keyword of a [`NodeKind::VariablesDeclaration`]
/// row. Real parses store only `late` in the row's `LateKeyword` word; the
/// span head is face-dependent — a field declaration anchors the row at the
/// keyword itself (members.rs `decl_head`: `late`, else var/final/const,
/// else the type start), a local declaration at the name — so the head
/// answers directly when it lexes as the keyword, and the name-anchored
/// face recovers it with a bounded backward token walk stopped by statement
/// boundaries; hand-built tables may carry the keyword itself in that word.
pub(crate) fn variables_declaration_keyword(c: &Check<'_>, row: u32) -> Option<u32> {
    let stored = if c.kind(row) == NodeKind::VariablesDeclaration {
        c.field(row, PayloadField::LateKeyword).unwrap_or(NONE_TOKEN)
    } else {
        NONE_TOKEN
    };
    if stored != NONE_TOKEN && matches!(c.lexeme(stored), "var" | "final" | "const") {
        return Some(stored);
    }
    let head = c.head_token(row);
    if matches!(c.lexeme(head), "var" | "final" | "const") {
        return Some(head);
    }
    let mut token = head;
    for _ in 0..256 {
        token = token.checked_sub(1)?;
        match c.lexeme(token) {
            "var" | "final" | "const" => return Some(token),
            // `late` belongs to the same declaration's modifiers.
            ";" | "{" | "}" => return None,
            _ => {}
        }
    }
    None
}

/// The type node directly before the name among the declaration's direct
/// children (`Type` when a return type/variable type is written, `NoType`
/// when the grammar allows its absence). `name`'s
/// subtree ends at the previous direct sibling, so the sibling is O(1) to
/// read. `None` when the name is the first child (nothing recovered).
pub(crate) fn type_before(c: &Check<'_>, parent: u32, name: u32) -> Option<(u32, NodeKind)> {
    let window = c.children(parent);
    let subtree_start = c.children(name).start;
    (subtree_start > window.start).then(|| {
        let prev = subtree_start - 1;
        (prev, c.kind(prev))
    })
}

// —— name-style predicates ——————————————————————————————————————————————

/// The official `isCamelCase` (`pkg/linter/lib/src/utils.dart:22`,
/// `_camelCasePattern`: `^_*(?:\$+_+)*[$?A-Z][$?a-zA-Z\d]*$`).
pub(crate) fn is_upper_camel_case(name: &str) -> bool {
    let b = name.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i] == b'_' {
        i += 1;
    }
    // The `(\$+_+)*` groups can stop anywhere; `$` also heads the tail
    // class, so parse the group prefix exhaustively over all reachable
    // boundaries (no backtracking needed beyond this small set).
    let mut reachable = vec![i];
    let mut cursor = i;
    loop {
        let dollars = cursor;
        while cursor < b.len() && b[cursor] == b'$' {
            cursor += 1;
        }
        if cursor == dollars {
            break;
        }
        let underscores = cursor;
        while cursor < b.len() && b[cursor] == b'_' {
            cursor += 1;
        }
        if cursor == underscores {
            break; // `$`s not followed by `_`: not a group here
        }
        reachable.push(cursor);
    }
    reachable.into_iter().any(|k| {
        // One head char from `[$?A-Z]` (the official `isUpperCase` range
        // quirk included — utils.dart:40), then the tail class.
        k < b.len()
            && matches!(b[k], b'$' | b'?' | b'A'..=b'Z')
            && b[k + 1..]
                .iter()
                .all(|&ch| ch.is_ascii_alphanumeric() || ch == b'$' || ch == b'?')
    })
}

/// The official `isLowerCamelCase` (`pkg/linter/lib/src/utils.dart:31`):
/// a lone uppercase letter, `_`, or
/// `^_*[?$a-z][a-z\d?$]*(?:(?:[A-Z]|_\d)[a-z\d?$]*)*_?$`.
pub(crate) fn is_lower_camel_case(name: &str) -> bool {
    let b = name.as_bytes();
    // utils.dart:67-70 — the single-uppercase / `_` cases pass, everything
    // else (including other single chars) falls through to the regex.
    if b.len() == 1 && matches!(b[0], b'@'..=b'Z') {
        return true;
    }
    if b == b"_" {
        return true;
    }
    let mut i = 0;
    while i < b.len() && b[i] == b'_' {
        i += 1;
    }
    // Head char: [?$a-z].
    if i >= b.len() || !matches!(b[i], b'?' | b'$' | b'a'..=b'z') {
        return false;
    }
    i += 1;
    loop {
        // Middle run: [a-z\d?$]* — stops at `_`/uppercase, which force the
        // separator or the end.
        while i < b.len()
            && (b[i].is_ascii_lowercase() || b[i].is_ascii_digit() || b[i] == b'$' || b[i] == b'?')
        {
            i += 1;
        }
        if i == b.len() {
            return true;
        }
        if b[i].is_ascii_uppercase() {
            i += 1;
            continue;
        }
        if b[i] == b'_' {
            // `_\d` separator, or the regex's optional trailing `_`.
            if i + 1 < b.len() {
                if b[i + 1].is_ascii_digit() {
                    i += 2;
                    continue;
                }
                return false;
            }
            return true;
        }
        return false;
    }
}

/// `StringExtensions.hasLeadingUnderscore`
/// (`pkg/linter/lib/src/util/ascii_utils.dart:58`).
pub(crate) fn has_leading_underscore(name: &str) -> bool {
    name.starts_with('_')
}

/// `StringExtensions.isJustUnderscores` (`ascii_utils.dart:61`): non-empty
/// and all underscores (`_`, `__`, ...).
pub(crate) fn is_just_underscores(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b == b'_')
}

/// Whether `pattern` (a [`NodeKind::DeclaredVariablePattern`]) is the
/// right-hand side of an object-pattern field-name shortcut, `Foo(:name)`
/// — the official `isFieldNameShortcut` guard
/// (constant_identifier_names.dart:49-52,
/// no_leading_underscores...dart:70-73): the enclosing
/// [`NodeKind::PatternField`] carries no explicit label
/// (`handleNoName`, extensions.dart:226-231), which covers `:name`,
/// `:var name`, and the null-check/assert-wrapped `:var name?` alike
/// (the wrapper unwraps before the field test).
///
/// Production tiling: the field seals childless AFTER its sub-pattern
/// (`handle_pattern_field` is one atomic begin+end), so the pattern's
/// arena parent is the record/object container, not the field — the
/// enclosing field is the first FOLLOWING `PatternField` row that
/// touches the pattern's span (fields seal in order, so a later field
/// starts past this pattern's end; a null-check/assert wrapper shifts
/// the field's begin anchor onto the `?`/`!`, hence touching rather
/// than covering). A hand-built tree keeps the field as the direct
/// parent. The field's colon word IS the `:` token for the name-less
/// form and the label identifier when named (the
/// `simplify_variable_pattern` word-0 face).
pub(crate) fn is_field_name_shortcut(c: &Check<'_>, pattern: u32) -> bool {
    let field = c
        .ancestor(1)
        .filter(|&parent| c.kind(parent) == NodeKind::PatternField)
        .or_else(|| {
            let span = c.byte_span(pattern);
            (pattern + 1..c.unit.ast.len() as u32).find(|&row| {
                c.kind(row) == NodeKind::PatternField && {
                    let field = c.byte_span(row);
                    field.start <= span.end && span.start <= field.end
                }
            })
        });
    field.is_some_and(|field| {
        // NONE marks a positional field, a label identifier names the
        // field — only the `:` token itself is the shortcut face.
        c.payload(field)
            .first()
            .copied()
            .is_some_and(|colon| colon != NONE_TOKEN && c.lexeme(colon) == ":")
    })
}

/// The `TOKEN_NONE` sentinel as stored in payload words.
pub(crate) const NONE_TOKEN: u32 = dartq_ast::TOKEN_NONE;

/// Whether the current node sits inside a constant context, answering the
/// official `inConstantContext` for the parsed AST. The official computation
/// (`pkg/analyzer/lib/src/dart/ast/ast.dart:17858-17918`, `constantContext`)
/// is a parent walk with a fixed transparent set — ported node-for-node over
/// the collapse kinds (the official rule declares `canUseParsedResult`, so
/// the walk is the parse-time face, not a resolution product):
///
/// - const-context roots: an annotation ([`NodeKind::Metadata`]), an enum
///   element (`EnumConstantArguments`), an old-style `case` label
///   ([`NodeKind::SwitchCase`]), a const-marked [`NodeKind::ConstantPattern`]
///   (without `const` the pattern is *not* a context and the walk stops), and
///   a const-marked `const` expression / literal / [`NodeKind::VariablesDeclaration`];
/// - transparent: expressions and the argument/map-entry/spread/null-aware
///   wrappers around them, plus the variable-declaration chain the official
///   `VariableDeclaration` transparency covers
///   ([`NodeKind::InitializedIdentifier`] / [`NodeKind::VariableInitializer`]);
/// - anything else stops the walk: `false`.
///
/// The node's own const keyword never counts (`includeSelf: false`).
pub(crate) fn in_const_context(c: &Check<'_>) -> bool {
    for i in (0..c.path.len().saturating_sub(1)).rev() {
        let ancestor = c.path[i];
        match c.kind(ancestor) {
            NodeKind::Metadata | NodeKind::EnumElement | NodeKind::SwitchCase => return true,
            // Without its own `const` a constant pattern is not a context and
            // the official walk stops (`return null`); a non-const variable
            // declaration list stops the same way.
            NodeKind::ConstantPattern => return head_is_const(c, ancestor),
            NodeKind::VariablesDeclaration => {
                // Member rows nest under `Fields`/`TopLevelFields`; every
                // other parent shape is a local/for-parts declaration.
                let member = i > 0
                    && matches!(
                        c.kind(c.path[i - 1]),
                        NodeKind::Fields | NodeKind::TopLevelFields
                    );
                return variables_decl_is_const(c, ancestor, member);
            }
            NodeKind::ConstExpression
            | NodeKind::ConstLiteral
            | NodeKind::ListLiteral
            | NodeKind::SetOrMapLiteral
            | NodeKind::RecordLiteral => {
                if const_keyword(c, ancestor).is_some() {
                    return true;
                }
            }
            // The `const .name` shorthand brackets a written `const`
            // keyword (`begin/endConstDotShorthand`) — always a const
            // context for the shorthand inside.
            NodeKind::ConstDotShorthand => return true,
            // An enum element's argument list is a constant context —
            // the enum's own element args tile as `Arguments` rows under
            // the `EnumBody` (the `EnumElement` constant marker is a
            // sibling, not a container). Members (the `const E(…)`
            // constructor, fields) nest under `DeclarationBody`, so the
            // face keys on the path's `EnumBody` child being `Arguments`.
            NodeKind::EnumBody => {
                return c
                    .path
                    .get(i + 1)
                    .is_some_and(|&child| c.kind(child) == NodeKind::Arguments);
            }
            // The official transparent set: every expression kind plus the
            // argument/element wrappers and the variable-declaration chain.
            // `DotShorthandHead` is the `.`-anchor leaf the shorthand's
            // `new`/selector/arguments tile beside — transparent so the
            // shorthand reads its enclosing context.
            NodeKind::DotShorthandHead
            | NodeKind::Arguments
            | NodeKind::NamedArgument
            | NodeKind::MapEntry
            | NodeKind::Spread
            | NodeKind::NullAwareElement
            | NodeKind::IfControlFlow
            | NodeKind::ThenControlFlow
            | NodeKind::ElseControlFlow
            | NodeKind::InitializedIdentifier
            | NodeKind::VariableInitializer
            | NodeKind::Send
            | NodeKind::ImplicitCreation
            | NodeKind::DotAccess
            | NodeKind::Qualified
            | NodeKind::Cascade
            | NodeKind::CascadeAccess
            | NodeKind::ParenthesizedExpression
            | NodeKind::BinaryExpression
            | NodeKind::Assignment
            | NodeKind::UnaryPrefix
            | NodeKind::UnaryPostfix
            | NodeKind::NonNullAssert
            | NodeKind::Conditional
            | NodeKind::AsExpression
            | NodeKind::IsExpression
            | NodeKind::AwaitExpression
            | NodeKind::ThrowExpression
            | NodeKind::Identifier
            | NodeKind::ThisExpression
            | NodeKind::SuperExpression
            | NodeKind::StringLiteral
            | NodeKind::NewExpression => {}
            _ => return false,
        }
    }
    false
}

/// The explicit `const` keyword of a const-carrying expression node, or
/// `None`. Const expressions/carry literals keep the keyword as a payload
/// word (`LiteralListHandle`/`RecordLiteralEnd`/
/// `LiteralSetOrMapHandle`/`ConstExpressionEnd` payloads); the keyword-less forms
/// are transparent.
pub(crate) fn const_keyword(c: &Check<'_>, node: u32) -> Option<u32> {
    match c.kind(node) {
        NodeKind::ConstExpression | NodeKind::ConstLiteral => {
            head_is_const(c, node).then(|| c.head_token(node))
        }
        NodeKind::ListLiteral | NodeKind::SetOrMapLiteral | NodeKind::RecordLiteral => {
            let keyword = c.field(node, PayloadField::ConstKeyword)?;
            (keyword != NONE_TOKEN).then_some(keyword)
        }
        _ => None,
    }
}

/// Whether `node`'s span head lexeme is the `const` keyword (the head-when-
/// present eliminations of derivable words).
fn head_is_const(c: &Check<'_>, node: u32) -> bool {
    // A constant pattern anchors at the keyword itself.
    c.lexeme(c.head_token(node)) == "const"
}

/// Whether a [`NodeKind::VariablesDeclaration`] carries the `const` keyword
/// — the official `VariableDeclarationList.keyword?.keyword == CONST` face
/// (`constantContext`, ast.dart:17892-17897). `member` says the row nests
/// under `Fields`/`TopLevelFields`.
///
/// The row's head convention differs by lane: member/top-level rows head at
/// `late`, else the `varFinalOrConst` keyword, else the written type's
/// first token (upstream `beginVariablesDeclaration` — members.rs
/// `decl_head`), while local/for-parts rows head at the declared name and
/// the keyword run sits before the row's own span (statements.rs `name`).
/// `const` is a reserved word — never a type or variable name — so a member
/// head answers directly and a local head resolves by scanning backward
/// over the written type to the modifier run; the first
/// `var`/`final`/`const` reached is the list's keyword. Balanced `()[]{}`
/// groups on the way (function-type parameter lists, record types, default
/// values inside them) are skipped — a `const` nested inside one is part of
/// the type, not the declaration keyword.
fn variables_decl_is_const(c: &Check<'_>, node: u32, member: bool) -> bool {
    let head = c.head_token(node);
    if member {
        // `late` heads when present and `late const` is not a grammar
        // shape, so the head answers alone.
        return c.lexeme(head) == "const";
    }
    let mut depth = 0u32;
    let mut token = head;
    while let Some(prev) = token.checked_sub(1) {
        match c.lexeme(prev) {
            ")" | "]" | "}" => depth += 1,
            "(" | "[" | "{" => {
                if depth == 0 {
                    return false;
                }
                depth -= 1;
            }
            "const" if depth == 0 => return true,
            "var" | "final" | ";" | "=>" if depth == 0 => return false,
            _ => {}
        }
        token = prev;
    }
    false
}

/// The last token of `id`'s span (the span's `end` is exclusive).
pub(crate) fn tail_token(c: &Check<'_>, id: u32) -> u32 {
    c.unit.ast.token_range(id).end - 1
}

/// The byte span of one token. A miss on the shared token helper degrades
/// to the zero-width span at the table tail — the caller's span arithmetic
/// stays sound and the report it feeds collapses to a skipped hit.
pub(crate) fn byte_span_of_token(c: &Check<'_>, token: u32) -> core::ops::Range<usize> {
    match c.token(token) {
        Some(t) => t.byte_offset as usize..(t.byte_offset + t.byte_len) as usize,
        None => {
            let end = c.unit.tokens.last().byte_offset as usize;
            end..end
        }
    }
}

/// The UTF-16 span of one token — the `reportAtToken` face. A miss on the
/// shared token helper degrades to a zero-width span at the unit's UTF-16
/// tail (the report the range feeds is inert either way: the helper miss
/// already debug-asserted, and a zero-width anchor past every real token is
/// not a Dart construct a rule reports).
pub(crate) fn utf16_token_span(c: &Check<'_>, token: u32) -> core::ops::Range<u32> {
    let Some(start) = c.utf16_of_token(token) else {
        let end = c.unit.tokens.last().utf16_offset;
        return end..end;
    };
    let end = c.utf16_end_of_token(token).unwrap_or(start);
    start..end
}

/// Whether the node carries a documentation comment (`node.documentationComment
/// != null`): a `///` or `/**` comment attached to its first token.
pub(crate) fn has_doc_comment(c: &Check<'_>, node: u32) -> bool {
    c.comments
        .attached(c.head_token(node) as usize)
        .iter()
        .any(|comment| {
            let text = &c.unit.source[comment.range.start as usize..comment.range.end as usize];
            text.starts_with("///") || text.starts_with("/**")
        })
}

/// The `get`/`set` keyword token of a member declaration, by lexeme —
/// the member's `GetOrSet` payload word (absent sentinel for plain
/// methods).
pub(crate) fn get_set_token<'a>(c: &'a Check<'_>, node: u32) -> Option<&'a str> {
    if !matches!(c.kind(node), NodeKind::Method | NodeKind::TopLevelMethod) {
        return None;
    }
    match c.field(node, PayloadField::GetOrSet) {
        Some(token) if token != crate::rules::NONE_TOKEN => Some(c.lexeme(token)),
        _ => None,
    }
}

// —— same-file class-hierarchy face ————————————————————————————————————————
//
// The four override-family rules (`annotate_overrides`,
// `avoid_renaming_method_parameters`, `overridden_fields`,
// `unnecessary_overrides`) read the class hierarchy the official rules get
// from the inheritance manager. The substrate carries the same facts for
// same-file declarations: supertype names are written `Type` rows in the
// header, and the unit's top-level declarations are a bounded root scan.
// Cross-file supertypes (imports, SDK types) resolve to `None` — the face
// under-reports there, the documented divergence family of this group.

/// The kind of member a lookup targets (the official
/// `lookUpMethod`/`lookUpGetter`/`lookUpSetter` split). A `Fields` row
/// answers every query kind: the official variable carries both accessors —
/// except [`MemberQuery::Variable`], which only fields answer (the
/// `isOriginVariable` getter face of `overridden_fields`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MemberQuery {
    Method,
    Getter,
    Setter,
    Variable,
}

/// The row of the same-file class-like declaration named `name`
/// (`ClassDeclaration` / `MixinDeclaration` / `EnumDeclaration`), or `None`
/// when the name is not a same-file class-like declaration (an SDK type, an
/// import — the face never sees through it).
///
/// Memoized as a name -> row index built once per run on first query
/// ([`RunCaches::class_rows`]). (#758: every lookup re-walked every
/// top-level wrapper, and the ancestor chain resolves one name per edge per
/// build.)
pub(crate) fn same_file_declaration(c: &Check<'_>, name: &str) -> Option<u32> {
    if c.caches.class_rows.borrow().is_none() {
        let index = class_row_index(c);
        *c.caches.class_rows.borrow_mut() = Some(index);
    }
    c.caches
        .class_rows
        .borrow()
        .as_ref()
        .expect("index built above")
        .get(name)
        .copied()
}

/// The name -> class-like row index of the unit: per top-level wrapper, the
/// wrapper's first class-like row paired with every class-naming
/// `Identifier` child (the [`same_file_declaration`] shape). The first
/// wrapper wins per name — the order the per-name wrapper walk answered in.
fn class_row_index(c: &Check<'_>) -> FxHashMap<String, u32> {
    const CLASS_LIKE: [NodeKind; 3] = [
        NodeKind::ClassDeclaration,
        NodeKind::MixinDeclaration,
        NodeKind::EnumDeclaration,
    ];
    let mut index = FxHashMap::default();
    let Some(root) = c.unit.ast.root() else {
        return index;
    };
    for top in c.collect_direct(root) {
        if c.kind(top) != NodeKind::TopLevelDeclaration {
            continue;
        }
        let kids = c.collect_direct(top);
        let Some(&class_row) = kids.iter().find(|&kid| CLASS_LIKE.contains(&c.kind(*kid))) else {
            continue;
        };
        for kid in kids {
            let named = c.kind(kid) == NodeKind::Identifier
                && matches!(
                    c.field(kid, PayloadField::Context),
                    Some(ctx)
                        if ctx == dartq_parser::IdentifierContext::ClassOrMixinOrExtensionDeclaration
                            as u32
                            || ctx == dartq_parser::IdentifierContext::EnumDeclaration as u32
                );
            if named {
                index
                    .entry(c.lexeme(c.head_token(kid)).to_string())
                    .or_insert(class_row);
            }
        }
    }
    index
}

/// The clause a supertype is written in — the concrete-member lookup walks
/// extends/with/on only (a concrete implementation never comes from an
/// `implements` interface), the any-member lookup walks all of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SupertypeClause {
    Extends,
    With,
    On,
    Implements,
}

/// The written supertypes of a class-like header, in source order with
/// their clause: the `Type` rows among the declaration's direct children,
/// stopping at the body (the `Type` is a width-0 leaf row — bridge.rs
/// `handle_type` — and the clause keywords are payload words with no
/// wrapper rows, so the direct-children walk covers the whole header;
/// `TypeArguments`/`TypeVariables` rows are skipped wholesale, each
/// collected row's clause read off the nearest preceding clause keyword,
/// itself a bounded backward token walk).
///
/// Memoized per class-like row in [`RunCaches::supertype_lists`] — the
/// header is invariant for the run. (#758: the previous form walked the
/// declaration's whole completion window per call — O(class subtree), with
/// every member type collected as a phantom name on the way — and its
/// `TypeArguments` skip never advanced the cursor, since `children(i).end
/// == i` for every row in this post-order arena, hanging on the first
/// generic declaration.)
pub(crate) fn supertypes<'a>(c: &'a Check<'_>, class_row: u32) -> Vec<(&'a str, SupertypeClause)> {
    if !c.caches.supertype_lists.borrow().contains_key(&class_row) {
        let rows: Vec<(u32, SupertypeClause)> = c
            .collect_direct(class_row)
            .into_iter()
            .take_while(|&kid| {
                !matches!(c.kind(kid), NodeKind::DeclarationBody | NodeKind::EnumBody)
            })
            .filter(|&kid| c.kind(kid) == NodeKind::Type)
            .map(|kid| {
                // The head token is the name; a `?` tail rides the same row.
                let head = c.head_token(kid);
                (head, clause_of(c, class_row, head))
            })
            .collect();
        c.caches
            .supertype_lists
            .borrow_mut()
            .insert(class_row, rows);
    }
    c.caches
        .supertype_lists
        .borrow()
        .get(&class_row)
        .expect("row inserted above")
        .iter()
        .map(|&(token, clause)| (c.lexeme(token), clause))
        .collect()
}

/// The clause keyword governing the supertype named at `head`: the nearest
/// preceding `extends`/`with`/`on`/`implements` lexeme back to the
/// declaration's span head (a header is a handful of tokens).
fn clause_of(c: &Check<'_>, class_row: u32, head: u32) -> SupertypeClause {
    let start = c.head_token(class_row);
    let mut token = head;
    while token > start {
        token -= 1;
        match c.lexeme(token) {
            "extends" => return SupertypeClause::Extends,
            "with" => return SupertypeClause::With,
            "on" => return SupertypeClause::On,
            "implements" => return SupertypeClause::Implements,
            _ => {}
        }
    }
    SupertypeClause::Implements
}

/// The same-file ancestor chain of `class_row`, each written supertype name
/// resolved through [`same_file_declaration`], in written order,
/// transitively. `concrete` walks extends/with/on links only (the official
/// concrete-member lookup); otherwise `implements` interfaces join too
/// (the any-member lookup behind `annotate_overrides`). The chain stops at
/// names that leave the file.
///
/// Memoized per `(row, concrete)` in [`RunCaches::ancestor_chains`] — the
/// chain is a per-container invariant. (#758: `annotate_overrides` rebuilt
/// the identical chain per member, each rebuild rescanning every header on
/// the way.)
pub(crate) fn same_file_ancestors(c: &Check<'_>, class_row: u32, concrete: bool) -> Vec<u32> {
    if let Some(cached) = c
        .caches
        .ancestor_chains
        .borrow()
        .get(&(class_row, concrete))
    {
        return cached.clone();
    }
    let clause_joins =
        |clause: SupertypeClause| !concrete || !matches!(clause, SupertypeClause::Implements);
    let mut chain = Vec::new();
    let mut seen = vec![class_row];
    let mut queue: Vec<u32> = vec![class_row];
    while let Some(current) = queue.pop() {
        for (name, clause) in supertypes(c, current) {
            if !clause_joins(clause) {
                continue;
            }
            let Some(row) = same_file_declaration(c, name) else {
                continue;
            };
            if seen.contains(&row) {
                continue;
            }
            seen.push(row);
            chain.push(row);
            queue.push(row);
        }
    }
    c.caches
        .ancestor_chains
        .borrow_mut()
        .insert((class_row, concrete), chain.clone());
    chain
}

/// Whether the member row carries an `@override` annotation: its left
/// sibling in the body's child list is the `MetadataStar`, whose `Metadata`
/// children name the annotations (declarations/ `has_annotations` idiom).
pub(crate) fn has_override_annotation(c: &Check<'_>, body: u32, member: u32) -> bool {
    has_annotation_named(c, body, member, "override")
}

/// The `Metadata` rows of one member: the `MetadataStar` left sibling's
/// children (declarations/ `has_annotations` idiom), empty when the
/// member carries no annotations. An operator's `OperatorName` row tiles
/// as a body-level sibling between the `MetadataStar` and the `Method`
/// row (the name-token holder lives inside the member for every other
/// shape), so the walk left skips name rows before deciding.
pub(crate) fn member_annotations(c: &Check<'_>, body: u32, member: u32) -> Vec<u32> {
    let members = c.collect_direct(body);
    let Some(index) = members.iter().position(|&m| m == member) else {
        return Vec::new();
    };
    let mut probe = index;
    while let Some(&row) = probe.checked_sub(1).and_then(|i| members.get(i)) {
        probe -= 1;
        match c.kind(row) {
            NodeKind::MetadataStar => {
                return c
                    .collect_direct(row)
                    .into_iter()
                    .filter(|&meta| c.kind(meta) == NodeKind::Metadata)
                    .collect();
            }
            NodeKind::OperatorName => continue,
            _ => return Vec::new(),
        }
    }
    Vec::new()
}

fn has_annotation_named(c: &Check<'_>, body: u32, member: u32, name: &str) -> bool {
    member_annotations(c, body, member).iter().any(|&meta| {
        c.collect_direct(meta).into_iter().any(|piece| {
            c.kind(piece) == NodeKind::Identifier && c.lexeme(c.head_token(piece)) == name
        })
    })
}

/// The name rows of the instance members of `member` answering `query` and
/// named `name`. Methods name through their `MethodDeclaration`-context
/// `Identifier` (or [`NodeKind::OperatorName`] for operators, split off the
/// `get_set` payload — declarations/ `get_set_token`), fields through
/// their [`NodeKind::InitializedIdentifier`] rows. The returned rows are
/// the name-bearing rows (an `Identifier`/`OperatorName`, or the field's
/// name-token holder for the field face).
pub(crate) fn member_matches(
    c: &Check<'_>,
    member: u32,
    query: MemberQuery,
    name: &str,
) -> Vec<u32> {
    let mut hits = Vec::new();
    match c.kind(member) {
        NodeKind::Method => {
            let get_set = get_set_token(c, member);
            let wanted: Option<&str> = match query {
                MemberQuery::Method => {
                    if get_set.is_some() {
                        return hits;
                    }
                    None
                }
                MemberQuery::Getter => Some("get"),
                MemberQuery::Setter => Some("set"),
                MemberQuery::Variable => return hits,
            };
            if wanted != get_set {
                return hits;
            }
            for kid in c.collect_direct(member) {
                match c.kind(kid) {
                    NodeKind::Identifier
                        if c.field(kid, PayloadField::Context)
                            == Some(
                                dartq_parser::IdentifierContext::MethodDeclaration as u32,
                            ) =>
                    {
                        if c.lexeme(c.head_token(kid)) == name {
                            hits.push(kid);
                        }
                    }
                    NodeKind::OperatorName if query == MemberQuery::Method => {
                        let op = c
                            .field(kid, PayloadField::Name)
                            .unwrap_or(u32::MAX);
                        if c.lexeme(op) == name {
                            hits.push(kid);
                        }
                    }
                    _ => {}
                }
            }
        }
        NodeKind::Fields => {
            if query == MemberQuery::Method
                || c.field(member, PayloadField::MemberKind)
                    != Some(dartq_parser::MemberKind::NonStaticField as u32)
            {
                return hits;
            }
            for decl in c.collect_direct(member) {
                if c.kind(decl) != NodeKind::VariablesDeclaration {
                    continue;
                }
                for var in c.collect_direct(decl) {
                    if c.kind(var) != NodeKind::InitializedIdentifier {
                        continue;
                    }
                    if let Some(name_row) = crate::rules::initialized_identifier_name(c, var)
                        && c.lexeme(name_row) == name
                    {
                        hits.push(var);
                    }
                }
            }
        }
        _ => {}
    }
    hits
}

/// The written-type verdicts the field-reading faces need (`void_checks`,
/// `implicit_call_tearoffs`, `null_check_on_nullable_type_parameter`).
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent written-type predicates read by several rule faces; \
              an enum encoding would force match plumbing at every read site"
)]
pub(crate) struct TypeFace<'a> {
    /// The written type's head name (`List` of `List<int>`, `T` of `T?`);
    /// `None` for `void` and function types.
    pub name: Option<&'a str>,
    /// Whether the written type carries a `?` tail.
    pub nullable: bool,
    /// Whether the written type is `void`.
    pub is_void: bool,
    /// Whether the written type is `FutureOr<void>`.
    pub is_future_or_void: bool,
    /// Whether the written type is a function type.
    pub is_function_type: bool,
    /// The written type arguments, `(name, nullable)` in order (empty for
    /// a raw or argument-less type).
    pub type_arguments: Vec<(&'a str, bool)>,
}

/// The written type face of the same-class instance field `name` (the
/// `Fields` row's written type sibling — the `prefer_initializing_formals`
/// tiling). `None` when the name is no instance field of the enclosing
/// class-like declaration.
///
/// Reads the run's per-class field index ([`field_type_rows`], the #703 F4
/// hoist): the enclosing class body was re-walked per call — once per `Send`
/// for `collection_methods_unrelated_type` — now one walk per class per run.
pub(crate) fn type_face_of_field<'a>(c: &'a Check<'_>, name: &str) -> Option<TypeFace<'a>> {
    let class_row = c.path.iter().rev().find_map(|&a| {
        matches!(
            c.kind(a),
            NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::EnumDeclaration
                | NodeKind::ExtensionDeclaration
        )
        .then_some(a)
    })?;
    let type_row = field_type_rows(c, class_row)
        .iter()
        .find(|(tok, _)| c.lexeme(*tok) == name)
        .and_then(|(_, row)| *row)?;
    Some(type_face(c, type_row))
}

/// The full [`TypeFace`] of the identifier `use` bound to `name` —
/// [`expressions::same_file::written_type`]'s shadow-correct innermost
/// binding walk with the written type row kept, so the rule reads the type
/// arguments, not just the head name. `None` over the same contract (no
/// same-file binding, or an untyped one).
pub(crate) fn written_type_face<'a>(
    c: &'a Check<'a>,
    use_node: u32,
    name: &str,
) -> Option<TypeFace<'a>> {
    let row = crate::rules::expressions::same_file::binding(c, use_node, name)?.1?;
    // A named type only: a function-type head spells the return type.
    if c.kind(row) != dartq_ast::NodeKind::Type {
        return None;
    }
    Some(type_face(c, row))
}

/// The instance-field index of `class_row`, built once per run on first
/// query and cached in the run's [`RunCaches`][crate::engine::RunCaches]:
/// the class's non-static `Fields` members as `(name token, written type
/// row)` pairs in body order (`None` row = untyped). The query's lexeme
/// find is first-wins, matching the body walk this index replaces: the
/// FIRST member carrying the name decides, untyped or not. The returned
/// borrow is a read-only view, function-local to [`type_face_of_field`].
fn field_type_rows<'a>(
    c: &'a Check<'a>,
    class_row: u32,
) -> core::cell::Ref<'a, Vec<(u32, Option<u32>)>> {
    if !c.caches.field_types.borrow().contains_key(&class_row) {
        let rows = build_field_type_rows(c, class_row);
        c.caches.field_types.borrow_mut().insert(class_row, rows);
    }
    core::cell::Ref::map(c.caches.field_types.borrow(), |cache| {
        cache.get(&class_row).expect("built above")
    })
}

/// The one class-body walk behind [`field_type_rows`]: the previous
/// per-call `type_face_of_field` loop, collecting every name instead of one.
fn build_field_type_rows(c: &Check<'_>, class_row: u32) -> Vec<(u32, Option<u32>)> {
    let mut rows = Vec::new();
    for member in body_members(c, class_row) {
        // The same face `member_matches`' Variable arm reads: a non-static
        // `Fields` row declaring through `InitializedIdentifier` rows.
        if c.kind(member) != NodeKind::Fields
            || c.field(member, PayloadField::MemberKind)
                != Some(dartq_parser::MemberKind::NonStaticField as u32)
        {
            continue;
        }
        let type_row = c.collect_direct(member).into_iter().find(|&kid| {
            matches!(
                c.kind(kid),
                NodeKind::Type | NodeKind::VoidType | NodeKind::FunctionType
            )
        });
        for decl in c.collect_direct(member) {
            if c.kind(decl) != NodeKind::VariablesDeclaration {
                continue;
            }
            for var in c.collect_direct(decl) {
                if c.kind(var) != NodeKind::InitializedIdentifier {
                    continue;
                }
                if let Some(name_row) = initialized_identifier_name(c, var) {
                    // `name_row` is a TOKEN id (`member_matches` compares its
                    // lexeme directly) — store it as-is, never re-head it.
                    rows.push((name_row, type_row));
                }
            }
        }
    }
    rows
}

/// The face of one written type row.
pub(crate) fn type_face<'a>(c: &'a Check<'_>, row: u32) -> TypeFace<'a> {
    match c.kind(row) {
        NodeKind::VoidType => TypeFace {
            name: None,
            nullable: false,
            is_void: true,
            is_future_or_void: false,
            is_function_type: false,
            type_arguments: Vec::new(),
        },
        NodeKind::FunctionType => TypeFace {
            name: None,
            nullable: false,
            is_void: false,
            is_future_or_void: false,
            is_function_type: true,
            type_arguments: Vec::new(),
        },
        _ => {
            let head = c.head_token(row);
            let name = c.lexeme(head);
            let nullable = c.byte_span(row).end > byte_span_of_token(c, head).end;
            let is_future_or_void = name == "FutureOr" && type_argument_is_void(c, row);
            TypeFace {
                name: Some(name),
                nullable,
                is_void: false,
                is_future_or_void,
                is_function_type: false,
                type_arguments: type_arguments_of(c, row),
            }
        }
    }
}

/// The `TypeArguments` sibling of a `Type` row. Production completion
/// order puts the sibling BEFORE the `Type` row (the collapse keeps the
/// `Type` span-only over its name tokens, so it seals after the argument
/// list parses — the `<int>` of `List<int>` is the left sibling under the
/// member row); hand-built trees may place it after.
fn type_arguments_sibling(c: &Check<'_>, type_row: u32) -> Option<u32> {
    let owner = c.unit.ast.parent(type_row)?;
    let members = c.collect_direct(owner);
    let index = members.iter().position(|&m| m == type_row)?;
    let arg_row = |row: &u32| c.kind(*row) == NodeKind::TypeArguments;
    members[..index]
        .iter()
        .rev()
        .find(|row| arg_row(row))
        .or_else(|| members[index + 1..].iter().find(|row| arg_row(row)))
        .copied()
}

/// The `(name, nullable)` pairs of the `TypeArguments` sibling of a
/// `Type` row, in order (empty when the sibling is absent).
fn type_arguments_of<'a>(c: &'a Check<'_>, type_row: u32) -> Vec<(&'a str, bool)> {
    let Some(args) = type_arguments_sibling(c, type_row) else {
        return Vec::new();
    };
    c.collect_direct(args)
        .into_iter()
        .filter(|&arg| c.kind(arg) == NodeKind::Type)
        .map(|arg| {
            let head = c.head_token(arg);
            let nullable = c.byte_span(arg).end > byte_span_of_token(c, head).end;
            (c.lexeme(head), nullable)
        })
        .collect()
}

/// Whether the `TypeArguments` sibling of a `Type` row holds a lone
/// `void` (the `FutureOr<void>` shape).
fn type_argument_is_void(c: &Check<'_>, type_row: u32) -> bool {
    let Some(args) = type_arguments_sibling(c, type_row) else {
        return false;
    };
    c.collect_direct(args)
        .into_iter()
        .any(|arg| c.kind(arg) == NodeKind::VoidType)
}

/// Whether the member row is abstract: a `Method` whose body is absent or
/// an `EmptyBody`/`NoBody` sentinel. A `Fields` row is always concrete (the
/// official variable carries storage).
pub(crate) fn member_is_concrete(c: &Check<'_>, member: u32) -> bool {
    match c.kind(member) {
        NodeKind::Method => {
            let body = c.collect_direct(member).into_iter().find(|&kid| {
                matches!(
                    c.kind(kid),
                    NodeKind::BlockBody
                        | NodeKind::Block
                        | NodeKind::ExpressionBody
                        | NodeKind::EmptyBody
                        | NodeKind::NoBody
                        | NodeKind::SkippedBody
                        | NodeKind::NativeFunctionBody
                        | NodeKind::InvalidFunctionBody
                )
            });
            match body {
                None => false,
                Some(body) => !matches!(c.kind(body), NodeKind::EmptyBody | NodeKind::NoBody),
            }
        }
        NodeKind::Fields => true,
        _ => false,
    }
}

/// Whether two nodes have the same token-lexeme sequence — the anchored
/// stand-in for the official `toSource()`/`toString()` comparisons
/// (`prefer_conditional_assignment` / `prefer_if_null_operators` /
/// `prefer_null_aware_operators`): comments and whitespace are dropped, token
/// lexemes are compared in order. Synthetic (recovery-inserted) tokens never
/// match real source on both sides unless mirrored, so they participate as
/// written — identical recovery on both sides compares equal.
pub(crate) fn token_seq_equal(c: &Check<'_>, a: u32, b: u32) -> bool {
    let ra = c.unit.ast.token_range(a);
    let rb = c.unit.ast.token_range(b);
    if ra.len() != rb.len() {
        return false;
    }
    ra.zip(rb)
        .all(|(ta, tb)| ta == tb || c.lexeme(ta) == c.lexeme(tb))
}

#[cfg(test)]
mod ticket_2467_tests {
    //! #2467 behavior pins: the rule faces that read the arena through
    //! [`rows_with_parent`] (the run's owner index over the O(1) parent
    //! column) answer the same verdicts on production-parse shapes.

    use crate::testsupport::{Fixture, one_rule};

    /// `prefer_final_fields` derives its candidates through
    /// `body_members` -> `rows_with_parent`: a private non-final field
    /// initialized at its declaration and never mutated reports.
    #[test]
    fn ticket_2467_prefer_final_fields_reports_the_unmutated_field() {
        let f = Fixture::parse("class C { int _x = 0; }");
        let lints = f.run(&one_rule("prefer_final_fields"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "The private field _x could be 'final'.");
    }

    /// The mutated twin stays silent — the write scan (the per-node
    /// `parent` readers the O(1) column serves) still sees the mutation.
    #[test]
    fn ticket_2467_prefer_final_fields_stays_silent_when_mutated() {
        let f = Fixture::parse("class C { int _x = 0; void m() { _x = 1; } }");
        let lints = f.run(&one_rule("prefer_final_fields"));
        assert!(lints.is_empty(), "unexpected lints: {lints:?}");
    }

    /// The override family reads the same owner index through
    /// `body_members`: a same-file override without `@override` reports.
    #[test]
    fn ticket_2467_annotate_overrides_reports_the_missing_annotation() {
        let f = Fixture::parse("class A { void m() {} } class B extends A { void m() {} }");
        let lints = f.run(&one_rule("annotate_overrides"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "The member 'm' overrides an inherited member but isn't annotated with '@override'."
        );
    }
}
