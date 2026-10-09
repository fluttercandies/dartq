//! Token handles: ids that uniformly address scanned and parser-inserted
//! tokens, plus the span payload carried by listener events.

use dartq_source::SourceRange;

/// Handle to one token: an index into the [`TokenTable`](dartq_scanner::TokenTable)
/// for scanned tokens, or `table_len + i` into the parser's inserted-token
/// side table for closer splits and recovery synthetics (core-spec §5.3).
///
/// `u32` is ample: 100k LOC is on the order of 3M tokens.
pub type TokenId = u32;

/// A token position as carried by listener events: the token handle plus its
/// UTF-16 span, precomputed so listeners never touch byte offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenSpan {
    /// Token handle (scanned or parser-inserted).
    pub id: TokenId,
    /// UTF-16 `[offset, offset + length)` of the token lexeme.
    pub range: SourceRange,
}

impl TokenSpan {
    /// Builds a span from its parts.
    pub(crate) const fn new(id: TokenId, range: SourceRange) -> Self {
        Self { id, range }
    }
}
