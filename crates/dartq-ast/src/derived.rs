//! Read-side derived-field accessors (#174): every payload word eliminated as
//! derivable has exactly one function here, so the parser lanes and readers
//! consume the derivation instead of duplicating it. Each function documents
//! its rule and the SDK evidence (official dispatch site + helper End
//! constructor) that pins it.
//!
//! Two primitives back most rules. Span head: `header.first_token` — the
//! official parser always computes begin tokens as the first token after
//! metadata (`parseTopLevelDeclarationImpl`, `parseFormalParameter`), so any
//! head-when-present keyword or leading modifier *is* the span head when
//! present. Span tail: `tokens[end_token - 1]` — the header span is
//! half-open, so the last token of the official End extent is the token just
//! below `end_token`.

use dartq_scanner::{Keyword, TokenKind, TokenTable};

use crate::TokenRef;
use crate::kind::{NodeKind, flags};
use crate::table::SyntaxTable;

fn token_at(tokens: &TokenTable, index: u32) -> Option<TokenKind> {
    tokens.get(index as usize).map(|token| token.kind)
}

/// The kind's modifier-set bits (`ModSet`, collapse map §1) stored in the
/// `NodeHeader.flags` modifier mask (bits 1..=7) — semantics are the
/// class/member families of map §1, encoded by the parser lane. Zero means
/// no modifier.
#[must_use]
pub fn mod_flags(ast: &SyntaxTable, id: u32) -> u16 {
    ast.header_flags(id) & flags::MOD_MASK
}

/// The kind's first modifier token — `Some` iff [`mod_flags`] is non-zero
/// (`flags == 0` ⇒ absent, the [`crate::TOKEN_NONE`] semantics of the
/// eliminated word). Sound because the official begin token of every
/// `ModSet`-bearing kind is the first token after metadata
/// (`parseTopLevelDeclarationImpl` for the class family,
/// `parseFormalParameter`/`parseTopLevelMemberImpl` for members): a
/// non-empty modifier set starts the span.
#[must_use]
pub fn first_modifier(ast: &SyntaxTable, id: u32) -> Option<TokenRef> {
    (mod_flags(ast, id) != 0).then(|| {
        let range = ast.token_range(id);
        debug_assert!(range.start < range.end, "modifier-less span head");
        range.start
    })
}

/// Whether a nullable [`NodeKind::Type`] ends with its `?` — the eliminated
/// `question` word. Rule: the last span token is `?` iff nullable.
///
/// SDK evidence (all six `handleType` dispatch sites): the `questionMark`
/// argument is non-null only when a `QUESTION` token was consumed as the
/// syntactically last token of the type extent —
/// `SimpleNullableType.parseTypeRest`/`SimpleNullableTypeWith1Argument`
/// (type_info_impl.dart:229-234, 329-334, assert QUESTION then pass),
/// `ComplexTypeInfo.parseType` (683-691, consume the `?` only when part of
/// the type; pass it as the final token) — and null otherwise
/// (type_info_impl.dart:199, 297, 393: extents ending on identifier or `>`).
/// The same holds for the sibling kinds that keep their `?` word:
/// `endFunctionType` (707-723) and `endRecordType`
/// (parser_impl.dart:1829-1845). Inner nullability (`List<int?>`) ends the
/// *inner* span on `?`; the outer span ends on `>`, so the rule never
/// misfires.
#[must_use]
pub fn question(tokens: &TokenTable, ast: &SyntaxTable, id: u32) -> bool {
    debug_assert!(ast.kind(id) == NodeKind::Type, "Type-only derivation");
    let range = ast.token_range(id);
    range.start < range.end && token_at(tokens, range.end - 1) == Some(TokenKind::Question)
}

/// The token just below the span end — the last token of the official End
/// extent (`;`, closing brace/bracket, skipped-body end). `None` for an
/// empty (all-synthetic) span. Backs the eliminations whose map column was
/// the closing token (`Break`/`Continue` `;`, `PatternVarDeclStatement` `;`,
/// `SkippedBody` end, `NativeFunctionBody` `;`, `MapPattern` `}`,
/// `ListPattern` `]`, `Indexed` `]`).
#[must_use]
pub fn last_token(tokens: &TokenTable, ast: &SyntaxTable, id: u32) -> Option<TokenRef> {
    let range = ast.token_range(id);
    if range.start < range.end {
        debug_assert!(
            tokens.get((range.end - 1) as usize).is_some(),
            "span end outside the token table"
        );
        Some(range.end - 1)
    } else {
        None
    }
}

/// Whether the node's span head is `keyword` — the head-when-present
/// derivation (`ConstantsPattern`/`VariablesDeclaration`/`CatchClause`/
/// `DeclaredVariablePattern`/`WildcardPattern` optional keywords). Sound
/// where the grammar lets nothing else start the node with that token, which
/// holds for each row (map §3): e.g. `CatchClause` starts with `on` iff the
/// `on` form was parsed (`handleCatchBlock(onKeyword, ..)`,
/// parser_impl.dart:10861).
#[must_use]
pub fn head_is(tokens: &TokenTable, ast: &SyntaxTable, id: u32, keyword: Keyword) -> bool {
    token_at(tokens, ast.token_range(id).start) == Some(TokenKind::Keyword(keyword))
}

/// `ForInParts`/`ForControlFlow` `for` — the eliminated token. Rule:
/// `await` absent ⇒ `for` is the span head; present ⇒ `await` is the head
/// and `for` immediately follows it.
///
/// `await_word` is the row's stored `Option` word ([`crate::TOKEN_NONE`] =
/// absent), kept because the `for` token is not derivable from the span
/// head alone.
#[must_use]
pub fn for_token(ast: &SyntaxTable, id: u32, await_word: u32) -> TokenRef {
    head_after(ast, id, await_word)
}

/// `Indexed` `open` `[` — the eliminated token. Rule: `question` absent ⇒
/// the `[` is the span head; present ⇒ the `?` is the head and the `[`
/// immediately follows it (`a?[0]`).
///
/// `question_word` is the row's stored `Option` word ([`crate::TOKEN_NONE`]
/// = absent).
#[must_use]
pub fn indexed_open(ast: &SyntaxTable, id: u32, question_word: u32) -> TokenRef {
    debug_assert!(ast.kind(id) == NodeKind::Indexed, "Indexed-only derivation");
    head_after(ast, id, question_word)
}

/// `marker_word` present ⇒ it *is* the span head and the derived token is
/// the next one; absent ⇒ the derived token is the head itself.
fn head_after(ast: &SyntaxTable, id: u32, marker_word: u32) -> TokenRef {
    let head = ast.token_range(id).start;
    if marker_word == crate::TOKEN_NONE {
        head
    } else {
        debug_assert_eq!(marker_word, head, "marker must be the span head");
        head + 1
    }
}
