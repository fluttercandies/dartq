//! Token cursor over an immutable [`TokenTable`].
#![allow(clippy::cast_possible_truncation)] // ids/offsets are u32 by design
//!
//! Replaces the official token-stream rewriter (core-spec §5.3): the scanned
//! table is never modified. Two mechanisms stand in for rewrites:
//!
//! * **Closer splitting** — the scanner emits `>>`/`>=`/`>>>`-family tokens
//!   unsplit (scanner crate docs, `abstract_scanner.dart:1479`). When the
//!   type-argument grammar consumes a closer it takes [`TokenCursor::consume_closer`],
//!   which rewrites one combined token into its `>`-leading parts: the
//!   leading `>` is consumed now and the remainder is queued so subsequent
//!   `peek`/`advance` calls drain it before the real stream resumes.
//! * **Synthetic insertion** — recovery inserts a zero-width synthetic token
//!   at the current position ([`TokenCursor::insert_synthetic`]); events
//!   reference it by id and the AST builder materializes it as a zero-length
//!   span (ast-design-spec §5.1).
//!
//! Inserted tokens live in an append-only side table addressed by ids past
//! `TokenTable::len()`. Speculation saves/restores the cursor and truncates
//! the side table, matching the official undoable rewriter
//! (`token_stream_rewriter.dart:433`).

use dartiq_scanner::{Token, TokenKind, TokenTable};
use dartiq_source::SourceRange;

use crate::token_id::{TokenId, TokenSpan};

/// One parser-inserted token: a closer-split remainder or a recovery
/// synthetic. Split remainders are not synthetic (they materialize valid
/// code, like the official `SimpleToken` splits in `util.dart:140-234`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InsertedToken {
    kind: TokenKind,
    utf16_offset: u32,
    synthetic: bool,
}

impl InsertedToken {
    /// UTF-16 length: every inserted kind has a fixed ASCII lexeme.
    fn utf16_len(self) -> u32 {
        self.kind.fixed_lexeme().map_or(1, str::len) as u32
    }

    fn range(self) -> SourceRange {
        SourceRange {
            offset: self.utf16_offset,
            length: self.utf16_len(),
        }
    }
}

/// Resolved view of the cursor's current token (real or inserted).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenView {
    pub kind: TokenKind,
    /// Id + UTF-16 span of the token.
    pub span: TokenSpan,
    /// True for scanner-recovery synthetics and parser-inserted synthetics.
    pub is_synthetic: bool,
}

/// A saved cursor state for speculative parsing (the
/// `UndoableTokenStreamRewriter.undo()` equivalent).
#[derive(Clone, Copy, Debug)]
pub(crate) struct CursorMark {
    pos: usize,
    head: usize,
    inserted_len: usize,
    last_consumed: Option<TokenId>,
}

/// Linear token cursor: pending-insertion queue first, then the scanned
/// table from `pos` on. The queue is almost always empty (zero cost); it
/// only fills under closer splits and recovery.
pub struct TokenCursor<'s, 't> {
    table: &'t TokenTable,
    source: &'s str,
    /// Index of the next *real* token (drained only after the queue empties).
    pos: usize,
    /// Inserted tokens not yet consumed start at `head`.
    inserted: Vec<InsertedToken>,
    head: usize,
    /// The id consumed by the latest [`Self::advance`] — the official
    /// `token` variable (the anchor `reportRecoverableError(token, ...)`
    /// sites report on).
    last_consumed: Option<TokenId>,
}

/// Queue-empty sentinel (`usize::MAX`); no inserted token index can collide.
const QUEUE_EMPTY: usize = usize::MAX;

impl<'s, 't> TokenCursor<'s, 't> {
    pub(crate) fn new(source: &'s str, table: &'t TokenTable) -> Self {
        Self {
            table,
            source,
            pos: 0,
            inserted: Vec::new(),
            head: QUEUE_EMPTY,
            last_consumed: None,
        }
    }

    /// The token consumed by the latest [`Self::advance`], if any.
    pub(crate) fn last_consumed_id(&self) -> Option<TokenId> {
        self.last_consumed
    }

    // --- marks / speculation -------------------------------------------------

    /// Real-stream position, for no-progress accounting.
    pub(crate) fn real_pos(&self) -> usize {
        self.pos
    }

    /// Checkpoints the cursor for speculative parsing.
    pub(crate) fn mark(&self) -> CursorMark {
        CursorMark {
            pos: self.pos,
            head: self.head,
            inserted_len: self.inserted.len(),
            last_consumed: self.last_consumed,
        }
    }

    /// Independent copy for speculative parsing (official
    /// `canParseAsConditional`'s `UndoableTokenStreamRewriter` swap): the
    /// fork shares the immutable table/source and starts from this cursor's
    /// state; insertions made while parsing the fork land in its own side
    /// table and never reach the real stream.
    pub(crate) fn fork(&self) -> TokenCursor<'s, 't> {
        TokenCursor {
            table: self.table,
            source: self.source,
            pos: self.pos,
            inserted: self.inserted.clone(),
            head: self.head,
            last_consumed: self.last_consumed,
        }
    }

    /// Rolls back to `mark`. Ids handed out after the mark become garbage and
    /// must not be referenced afterwards (they are re-issued on demand,
    /// exactly like the official undoable rewriter).
    pub(crate) fn reset(&mut self, mark: CursorMark) {
        self.pos = mark.pos;
        self.head = mark.head;
        self.inserted.truncate(mark.inserted_len);
        self.last_consumed = mark.last_consumed;
    }

    /// Real-stream position stored in a mark (for span re-derivation after
    /// the cursor moved on).
    pub(crate) fn mark_pos(mark: CursorMark) -> usize {
        mark.pos
    }

    /// Progress value of a mark (for no-progress comparisons after the fact).
    pub(crate) fn progress_at(mark: CursorMark) -> u64 {
        mark.pos as u64 * 2
    }

    /// Monotonic consumed-progress measure for no-progress guards: counts
    /// real tokens (weight 2) plus queued inserted tokens ahead of `pos`.
    pub(crate) fn progress(&self) -> u64 {
        let queue_pending = (self.inserted.len()
            - if self.head == QUEUE_EMPTY {
                self.inserted.len()
            } else {
                self.head
            }) as u64;
        self.pos as u64 * 2 + queue_pending
    }

    // --- resolution -----------------------------------------------------------

    /// Id of the current (peeked) token without consuming.
    pub(crate) fn peek_id(&self) -> TokenId {
        if self.head == QUEUE_EMPTY {
            self.pos as TokenId
        } else {
            (self.table.len() + self.head) as TokenId
        }
    }

    /// Resolves any id (scanned or inserted) to `(kind, is_synthetic)`.
    ///
    /// Ids always come from `peek`/`advance`/`insert_synthetic` of this same
    /// cursor; a garbage id from a rolled-back speculation must never reach
    /// here (that is the caller side of the [`Self::reset`] contract).
    pub(crate) fn resolve(&self, id: TokenId) -> (TokenKind, bool) {
        let index = id as usize;
        if index < self.table.len() {
            let t = self.table.get(index).expect("id below table len");
            (t.kind, t.is_synthetic())
        } else {
            let t = &self.inserted[index - self.table.len()];
            (t.kind, t.synthetic)
        }
    }

    /// UTF-16 span of any id (see [`Self::resolve`] for the id contract).
    pub(crate) fn span(&self, id: TokenId) -> SourceRange {
        let index = id as usize;
        if index < self.table.len() {
            let t = self.table.get(index).expect("id below table len");
            SourceRange {
                offset: t.utf16_offset,
                length: utf16_len_of_token(self.source, t),
            }
        } else {
            self.inserted[index - self.table.len()].range()
        }
    }

    /// Lexeme of any id. Inserted tokens have fixed lexemes — except the
    /// synthetic identifier, whose lexeme is the empty string (official
    /// synthetic identifiers are zero-width).
    pub(crate) fn lexeme(&self, id: TokenId) -> &'s str {
        let index = id as usize;
        if index < self.table.len() {
            self.table.lexeme(self.source, index)
        } else {
            let t = &self.inserted[index - self.table.len()];
            t.kind.fixed_lexeme().unwrap_or("")
        }
    }

    // --- core movement ----------------------------------------------------------

    /// Current token without consuming.
    pub(crate) fn peek(&self) -> TokenView {
        let id = self.peek_id();
        let (kind, is_synthetic) = self.resolve(id);
        TokenView {
            kind,
            span: TokenSpan::new(id, self.span(id)),
            is_synthetic,
        }
    }

    /// Kind of the token `n` positions ahead in the *real* stream (no queue
    /// involvement; valid only where nothing can be queued — i.e. right
    /// after a clean consume). Lookahead past EOF yields `TokenKind::Eof`.
    pub(crate) fn peek_kind_ahead(&self, n: usize) -> TokenKind {
        self.table
            .get(self.pos + n)
            .map_or(TokenKind::Eof, |t| t.kind)
    }

    /// Kind at an absolute real-stream offset (EOF-pinned past the end).
    pub(crate) fn peek_kind_ahead_abs(&self, offset: usize) -> TokenKind {
        self.table.get(offset).map_or(TokenKind::Eof, |t| t.kind)
    }

    /// Kind of the token `n` positions ahead, queued inserted tokens first,
    /// then the real stream (the linked-list `token.next!..` walk of the
    /// official parser). Valid everywhere, unlike [`Self::peek_kind_ahead`].
    pub(crate) fn look_kind(&self, n: usize) -> TokenKind {
        if self.head == QUEUE_EMPTY {
            return self.peek_kind_ahead(n);
        }
        let queued = self.inserted.len() - self.head;
        if n < queued {
            self.inserted[self.head + n].kind
        } else {
            self.peek_kind_ahead_abs(self.pos + (n - queued))
        }
    }

    /// Forward jump for the GFT type-variable pass: moves to a strictly
    /// later real token (queue must be empty). Paired with [`Self::reset`].
    pub(crate) fn jump_to(&mut self, id: TokenId) {
        debug_assert!(self.head == QUEUE_EMPTY);
        let target = id as usize;
        debug_assert!(target > self.pos && target < self.table.len());
        self.pos = target;
    }

    /// Matching close index for a real begin-group id (`TokenTable::matching_end`).
    pub(crate) fn table_matching_end(&self, id: TokenId) -> Option<TokenId> {
        let index = id as usize;
        if index >= self.table.len() {
            return None; // inserted tokens are never groups
        }
        self.table.matching_end(index).map(|i| i as TokenId)
    }

    /// Consumes the current token and returns its id. Consuming EOF is
    /// idempotent (the cursor stays pinned at the end).
    pub(crate) fn advance(&mut self) -> TokenId {
        let id = self.peek_id();
        self.last_consumed = Some(id);
        if self.head != QUEUE_EMPTY {
            self.head += 1;
            if self.head >= self.inserted.len() {
                self.head = QUEUE_EMPTY;
            }
        } else if self.pos + 1 < self.table.len() {
            self.pos += 1;
        }
        id
    }

    pub(crate) fn at_eof(&self) -> bool {
        self.peek().kind == TokenKind::Eof
    }

    // --- optional consumption -----------------------------------------------------

    /// Official `optional(lexeme, token)` (`util.dart:17`): consumes on
    /// lexeme match, silently returns `None` otherwise.
    pub(crate) fn opt_lexeme(&mut self, lexeme: &str) -> Option<TokenId> {
        if self.at_eof() {
            return None;
        }
        let id = self.peek_id();
        if self.lexeme(id) == lexeme {
            Some(self.advance())
        } else {
            None
        }
    }

    /// Consumes the current token when it has `kind`.
    pub(crate) fn opt_kind(&mut self, kind: TokenKind) -> Option<TokenId> {
        if self.peek().kind == kind {
            Some(self.advance())
        } else {
            None
        }
    }

    // --- closer splitting (official splitCloser/parseCloser) ------------------------

    /// `isCloser` (`type_info_impl.dart:1753)`: the `>`-family tokens.
    pub(crate) fn peek_is_closer(&self) -> bool {
        matches!(
            self.peek().kind,
            TokenKind::Gt
                | TokenKind::GtEq
                | TokenKind::GtGt
                | TokenKind::GtGtEq
                | TokenKind::GtGtGt
                | TokenKind::GtGtGtEq
        )
    }

    /// Consumes a `>`-family closer so that the consumed token is a plain
    /// `>` (official `parseCloser`, `type_info_impl.dart:1764)`: combined
    /// tokens (`>>`, `>=`, `>>>`, ...) split at this point, the remainder
    /// queueing for subsequent peeks. Returns the consumed `>` id.
    ///
    /// The caller must have checked [`Self::peek_is_closer`].
    pub(crate) fn consume_closer(&mut self) -> TokenId {
        debug_assert!(self.peek_is_closer(), "consume_closer requires a closer");
        let head = self.peek();
        if head.kind == TokenKind::Gt {
            return self.advance();
        }
        let base = head.span.range.offset;
        self.advance(); // drop the combined token (real or queued remainder)
        // Leading `>` first, then the remainder(s) — the queue drains in
        // order, so the leading token is consumed by the advance below.
        self.queue(InsertedToken {
            kind: TokenKind::Gt,
            utf16_offset: base,
            synthetic: false,
        });
        for (kind, at) in split_remainders(head.kind).iter().rev() {
            self.queue(InsertedToken {
                kind: *kind,
                utf16_offset: base + at,
                synthetic: false,
            });
        }
        self.advance() // the leading `>`
    }

    // --- inserted tokens --------------------------------------------------------------

    /// Queues a zero-width synthetic token at the current position (the
    /// official `rewriteAndRecover` / `insertSynthetic*` family). Returns
    /// its id.
    pub(crate) fn insert_synthetic(&mut self, kind: TokenKind, utf16_offset: u32) -> TokenId {
        let id = (self.table.len() + self.inserted.len()) as TokenId;
        self.queue(InsertedToken {
            kind,
            utf16_offset,
            synthetic: true,
        });
        id
    }

    fn queue(&mut self, token: InsertedToken) {
        if self.head == QUEUE_EMPTY {
            self.head = self.inserted.len();
        }
        self.inserted.push(token);
    }
}

/// Remainder tokens of a combined closer split, as `(kind, char offset from
/// the token start)`; mirrors `splitGtGt`/`splitGtEq`/`splitGtFromGtGt*`
/// (`util.dart:140-234`).
pub(crate) fn split_remainders(kind: TokenKind) -> &'static [(TokenKind, u32)] {
    match kind {
        TokenKind::GtEq => &[(TokenKind::Eq, 1)],
        TokenKind::GtGt => &[(TokenKind::Gt, 1)],
        TokenKind::GtGtEq => &[(TokenKind::GtEq, 1)],
        TokenKind::GtGtGt => &[(TokenKind::GtGt, 1)],
        TokenKind::GtGtGtEq => &[(TokenKind::GtGtEq, 1)],
        _ => &[],
    }
}

/// UTF-16 length of a scanned token. Fixed-lexeme kinds are pure ASCII, so
/// the byte length is exact; sliceable kinds (identifiers, numbers, strings)
/// may contain non-ASCII and are measured from the source.
fn utf16_len_of_token(source: &str, t: &Token) -> u32 {
    if t.kind.fixed_lexeme().is_some() {
        return t.byte_len;
    }
    let text = &source[t.byte_range()];
    if text.is_ascii() {
        return t.byte_len;
    }
    text.chars().map(char::len_utf16).sum::<usize>() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ParserFeatures;
    use dartiq_language::{FeatureSet, LanguageVersion};
    use dartiq_scanner::{ScanConfig, scan};
    use dartiq_source::FileTable;

    fn cursor(source: &'static str) -> (TokenCursor<'static, 'static>, &'static str) {
        let config = ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        };
        let file = FileTable::new().intern(std::path::Path::new("test.dart"));
        let result = scan(source, config, file);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        // Leak for the test: static lifetime keeps the helper signature simple.
        let table: &'static TokenTable = Box::leak(Box::new(result.tokens));
        let source: &'static str = Box::leak(source.to_string().into_boxed_str());
        (TokenCursor::new(source, table), source)
    }

    fn features() -> ParserFeatures {
        ParserFeatures::from_feature_set(FeatureSet::for_language_version(LanguageVersion::new(
            3, 15,
        )))
    }

    #[test]
    fn features_fold_from_language_version() {
        let f = features();
        assert!(f.patterns_enabled);
        // variance is an opt-in experiment (no enabledIn), so disabled at 3.15.
        assert!(!f.variance_enabled);
    }

    #[test]
    fn opt_and_advance_drain_the_stream() {
        let (mut cursor, _src) = cursor("a < b >> c");
        assert_eq!(cursor.peek().kind, TokenKind::Identifier);
        let a = cursor.advance();
        assert_eq!(cursor.lexeme(a), "a");
        assert!(cursor.opt_lexeme("<").is_some());
        assert!(cursor.opt_kind(TokenKind::Identifier).is_some());
        assert_eq!(cursor.peek().kind, TokenKind::GtGt);
        assert!(cursor.peek_is_closer());
        let first_gt = cursor.consume_closer();
        assert_eq!(cursor.resolve(first_gt), (TokenKind::Gt, false));
        assert_eq!(cursor.peek().kind, TokenKind::Gt); // split remainder
        let advanced = cursor.advance();
        assert_eq!(cursor.resolve(advanced).0, TokenKind::Gt);
        assert_eq!(cursor.peek().kind, TokenKind::Identifier);
        let c = cursor.advance();
        assert_eq!(cursor.lexeme(c), "c");
        assert!(cursor.at_eof());
    }

    #[test]
    fn triple_gt_splits_into_three() {
        let (mut cursor, _src) = cursor("List<List<int>>> x");
        // Skip to the `>>`: List < List < int > > > — consume until first `>>`.
        while !cursor.peek_is_closer() && !cursor.at_eof() {
            cursor.advance();
        }
        let mut count = 0;
        while cursor.peek_is_closer() {
            let id = cursor.consume_closer();
            assert_eq!(cursor.resolve(id).0, TokenKind::Gt);
            count += 1;
        }
        assert_eq!(count, 3);
    }

    #[test]
    fn speculation_restores_position_and_truncates_inserted() {
        let (mut cursor, _src) = cursor("a >> b");
        while !cursor.at_eof() && cursor.peek().kind != TokenKind::GtGt {
            cursor.advance();
        }
        let mark = cursor.mark();
        let _gt = cursor.consume_closer();
        assert_eq!(cursor.peek().kind, TokenKind::Gt);
        cursor.reset(mark);
        assert_eq!(cursor.peek().kind, TokenKind::GtGt);
        // Re-splitting after reset must still work (ids re-issued).
        let gt = cursor.consume_closer();
        assert_eq!(cursor.resolve(gt).0, TokenKind::Gt);
        assert_eq!(cursor.peek().kind, TokenKind::Gt);
    }

    #[test]
    fn synthetic_insertion_and_progress() {
        let (mut cursor, _src) = cursor("a");
        let mark = cursor.mark();
        let semi = cursor.insert_synthetic(TokenKind::Semicolon, cursor.peek().span.range.offset);
        let (kind, synthetic) = cursor.resolve(semi);
        assert_eq!(kind, TokenKind::Semicolon);
        assert!(synthetic);
        assert_eq!(cursor.peek().kind, TokenKind::Semicolon); // queued in front
        cursor.advance();
        assert_eq!(cursor.peek().kind, TokenKind::Identifier); // real stream resumes
        cursor.reset(mark);
        assert_eq!(cursor.peek().kind, TokenKind::Identifier);
        let _ = features();
    }
}
