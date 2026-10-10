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
pub(crate) mod declarations;
pub(crate) mod directives;
pub(crate) mod docs;
pub(crate) mod empty;
pub(crate) mod expressions;
pub(crate) mod flow;
pub(crate) mod names;
pub(crate) mod strings;
pub(crate) mod types;

use dartq_ast::NodeKind;

use crate::engine::Check;

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

/// The name-bearing direct child (Identifier or OperatorName) of a member
/// declaration node — operators name through [`NodeKind::OperatorName`],
/// everything else through an `Identifier` (collapse map §3.3/§3.5).
pub(crate) fn member_name(c: &Check<'_>, parent: u32) -> Option<u32> {
    c.direct_children(parent)
        .iter()
        .copied()
        .find(|&child| matches!(c.kind(child), NodeKind::Identifier | NodeKind::OperatorName))
}

/// The type node directly before the name among the declaration's direct
/// children (`Type` when a return type/variable type is written, `NoType`
/// when the grammar allows its absence — collapse map §3.3/§3.8). `name`'s
/// subtree ends at the previous direct sibling, so the sibling is O(1) to
/// read. `None` when the name is the first child (nothing recovered).
pub(crate) fn type_before<'a>(c: &'a Check<'_>, parent: u32, name: u32) -> Option<(u32, NodeKind)> {
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
/// right-hand side of an object-pattern field-name shortcut, `Foo(:name)` —
/// the official `isFieldNameShortcut` guard: the parent `PatternField`
/// carries a `NamedRecordField` whose name equals the bound variable's name
/// (constant_identifier_names.dart:49-52, no_leading_underscores...dart:70-73).
pub(crate) fn is_field_name_shortcut(c: &Check<'_>, pattern: u32) -> bool {
    let Some(&name) = c.payload(pattern).first() else {
        return false;
    };
    let Some(parent) = c.ancestor(1) else {
        return false;
    };
    if c.kind(parent) != NodeKind::PatternField {
        return false;
    }
    c.collect_direct(parent).into_iter().any(|field| {
        c.kind(field) == NodeKind::NamedRecordField
            && c.collect_direct(field).into_iter().any(|id| {
                c.kind(id) == NodeKind::Identifier && c.lexeme(c.head_token(id)) == c.lexeme(name)
            })
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
    for &ancestor in c.path.iter().take(c.path.len() - 1).rev() {
        match c.kind(ancestor) {
            NodeKind::Metadata | NodeKind::EnumElement | NodeKind::SwitchCase => return true,
            // Without its own `const` a constant pattern is not a context and
            // the official walk stops (`return null`); a non-const variable
            // declaration list stops the same way.
            NodeKind::ConstantPattern | NodeKind::VariablesDeclaration => {
                return head_is_const(c, ancestor);
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
            // The official transparent set: every expression kind plus the
            // argument/element wrappers and the variable-declaration chain.
            NodeKind::Arguments
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
/// word (collapse map §3.5 `LiteralListHandle`/`RecordLiteralEnd`/
/// `LiteralSetOrMapHandle`/`ConstExpressionEnd` rows); the keyword-less forms
/// are transparent.
pub(crate) fn const_keyword(c: &Check<'_>, node: u32) -> Option<u32> {
    match c.kind(node) {
        NodeKind::ConstExpression | NodeKind::ConstLiteral => {
            head_is_const(c, node).then(|| c.head_token(node))
        }
        NodeKind::ListLiteral | NodeKind::SetOrMapLiteral | NodeKind::RecordLiteral => {
            let keyword = c.payload(node).first().copied()?;
            (keyword != NONE_TOKEN).then_some(keyword)
        }
        _ => None,
    }
}

/// Whether `node`'s span head lexeme is the `const` keyword (the head-when-
/// present eliminations of collapse map §1).
fn head_is_const(c: &Check<'_>, node: u32) -> bool {
    c.lexeme(c.head_token(node)) == "const"
}

/// The last token of `id`'s span (the span's `end` is exclusive).
pub(crate) fn tail_token(c: &Check<'_>, id: u32) -> u32 {
    c.unit.ast.token_range(id).end - 1
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
