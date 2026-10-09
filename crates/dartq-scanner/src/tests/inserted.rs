//! Parser-inserted token materialization: `TokenTable::append_inserted`
//! folds the parser's side table into the main storage, re-seats Eof last,
//! and leaves every original token untouched — the contiguous unified table
//! the AST's span derivation reads.

use super::scan_src;
use crate::{Token, TokenKind, TokenTable};

/// `(kind, byte_offset, byte_len, utf16_offset, is_synthetic)` per token.
fn snapshot(table: &TokenTable) -> Vec<(TokenKind, u32, u32, u32, bool)> {
    table
        .as_slice()
        .iter()
        .map(|t| {
            (
                t.kind,
                t.byte_offset,
                t.byte_len,
                t.utf16_offset,
                t.is_synthetic(),
            )
        })
        .collect()
}

/// Realistic merge shapes on `f(a>>b);`: the parser consumes the `>>`
/// (`GtGt` at 3..5) as the first `>` and inserts the remainder `>` at 4
/// (closer split, not synthetic), plus a missing-`;` recovery synthetic,
/// zero-width at the EOF offset (the scanner's own EOF-synthetic shape).
fn split_fixture() -> (Token, Token) {
    const SRC: &str = "f(a>>b);";
    (
        Token::inserted(TokenKind::Gt, 4, 1, 4, false),
        Token::inserted(
            TokenKind::Semicolon,
            SRC.len() as u32,
            0,
            SRC.len() as u32,
            true,
        ),
    )
}

#[test]
fn append_inserted_keeps_contiguous_with_eof_last() {
    let src = "f(a>>b);";
    let r = scan_src(src);
    let orig_len = r.tokens.len();
    let mut table = r.tokens;
    let (gt, semi) = split_fixture();
    table.append_inserted([gt, semi]);

    assert_eq!(table.len(), orig_len + 2);
    let eof = table.get(table.len() - 1).expect("merged table non-empty");
    assert_eq!(eof.kind, TokenKind::Eof);
    assert_eq!(eof.byte_offset as usize, src.len());
    assert_eq!(eof.byte_len, 0);
    // Contiguity: appended tokens fill the Eof sentinel's former slot onward.
    assert_eq!(table.get(orig_len - 1).map(|t| t.kind), Some(TokenKind::Gt));
    assert_eq!(
        table.get(orig_len).map(|t| t.kind),
        Some(TokenKind::Semicolon)
    );
}

#[test]
fn append_inserted_preserves_original_tokens() {
    let src = "f(a>>b);";
    let r = scan_src(src);
    let mut table = r.tokens;
    let before = snapshot(&table);
    let begin_end = table.matching_end(1); // `(` (idx 1) closes at `)` — byte 6, idx 5
    assert_eq!(begin_end, Some(5));

    let (gt, semi) = split_fixture();
    table.append_inserted([gt, semi]);

    let after = snapshot(&table);
    assert_eq!(&after[..before.len() - 1], &before[..before.len() - 1]);
    assert_eq!(table.matching_end(1), begin_end, "end_of must stay valid");
    // Original slices unchanged (the `>>` lexeme still slices whole).
    assert_eq!(table.lexeme(src, 3), ">>");
    // The moved Eof is byte-identical to the pre-merge sentinel.
    assert_eq!(after.last(), before.last());
}

#[test]
fn append_inserted_empty_is_noop() {
    let src = "x;";
    let r = scan_src(src);
    let mut table = r.tokens;
    let before = snapshot(&table);

    table.append_inserted([]);

    assert_eq!(snapshot(&table), before);
    assert_eq!(table.last().kind, TokenKind::Eof);
}

#[test]
fn inserted_token_resolves_through_table_after_merge() {
    // dartq_ast::byte_span/utf16_span resolve node ids through
    // `TokenTable::get` alone; dartq-ast is not (and cannot be, per the
    // dependency direction) a dev-dependency here, so the span-read
    // precondition is pinned at the table level: after the merge every id —
    // inserted ones included — resolves to its exact offsets/flags.
    let src = "f(a>>b);";
    let r = scan_src(src);
    let orig_len = r.tokens.len();
    let mut table = r.tokens;
    let (gt, semi) = split_fixture();
    table.append_inserted([gt, semi]);

    let remainder = table
        .get(orig_len - 1)
        .expect("split remainder id resolves");
    assert_eq!(remainder.kind, TokenKind::Gt);
    assert_eq!(remainder.byte_offset, 4);
    assert_eq!(remainder.byte_len, 1);
    assert_eq!(remainder.utf16_offset, 4);
    assert!(!remainder.is_synthetic());
    assert_eq!(table.lexeme(src, orig_len - 1), ">");

    let synthetic = table.get(orig_len).expect("synthetic id resolves");
    assert_eq!(synthetic.kind, TokenKind::Semicolon);
    assert_eq!(synthetic.byte_offset as usize, src.len());
    assert_eq!(synthetic.byte_len, 0);
    assert!(synthetic.is_synthetic());
    assert_eq!(table.lexeme(src, orig_len), "");
    assert_eq!(synthetic.kind.fixed_lexeme(), Some(";"));
}
