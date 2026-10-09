//! Span derivation: nodes store token-index pairs; byte and UTF-16 offsets
//! derive from the token table at access time (layout spec §2). Zero-width
//! synthetic-only nodes derive a zero-length span at their insertion point.

use dartq_scanner::{Token, TokenTable};
use dartq_source::utf16_len_of;

use crate::kind::NodeKind;
use crate::table::SyntaxTable;

fn token_at(tokens: &TokenTable, index: u32) -> &Token {
    tokens
        .get(index as usize)
        .expect("node token id outside the token table")
}

/// The node's byte span `[start, end)` in the source text.
///
/// Exception kept on purpose (layout spec §2): a [`NodeKind::CompilationUnit`]
/// spans `[0, eof.end]` regardless of leading trivia, mirroring the official
/// hardcoded `CompilationUnitImpl.offset => 0` (analyzer `ast.dart:9230-9233`)
/// — do not "fix" this by derivation.
#[must_use]
pub fn byte_span(tokens: &TokenTable, ast: &SyntaxTable, id: u32) -> core::ops::Range<usize> {
    let range = ast.token_range(id);
    if ast.kind(id) == NodeKind::CompilationUnit {
        let eof = tokens.last();
        return 0..(eof.byte_offset + eof.byte_len) as usize;
    }
    let start = token_at(tokens, range.start).byte_offset as usize;
    if range.is_empty() {
        return start..start;
    }
    let last = token_at(tokens, range.end - 1);
    start..(last.byte_offset + last.byte_len) as usize
}

/// The node's span as UTF-16 code-unit offsets (the engine's external
/// position convention). Same derivation and same compilation-unit exception
/// as [`byte_span`].
#[must_use]
pub fn utf16_span(
    source: &str,
    tokens: &TokenTable,
    ast: &SyntaxTable,
    id: u32,
) -> core::ops::Range<u32> {
    let range = ast.token_range(id);
    if ast.kind(id) == NodeKind::CompilationUnit {
        let eof = tokens.last();
        return 0..eof.utf16_offset + utf16_len_of(&source[eof.byte_range()]);
    }
    let start = token_at(tokens, range.start).utf16_offset;
    if range.is_empty() {
        return start..start;
    }
    let last = token_at(tokens, range.end - 1);
    start..last.utf16_offset + utf16_len_of(&source[last.byte_range()])
}
