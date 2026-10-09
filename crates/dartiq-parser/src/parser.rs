//! The parser object: cursor + listener + folded features.
//!
//! One generic struct (`L: ParserListener`); grammar families live in their
//! own modules as `impl` blocks on `Parser` (`types`, `patterns`,
//! `expr_const`, `metadata`, `formal_params`, `recovery`).

#![allow(clippy::cast_possible_truncation)] // ids are u32 by design

use dartiq_scanner::{TokenKind, TokenTable};

use crate::cursor::TokenCursor;
use crate::features::ParserFeatures;
use crate::listener::ParserListener;
use crate::recovery::is_identifier;
use crate::token_id::{TokenId, TokenSpan};

/// Maximum structural nesting before the parser forces unwinding (official
/// `recoverFromStackOverflow` cap of 500, `parser_impl.dart:6709`).
const MAX_DEPTH: u32 = 500;

/// `AsyncModifier` (official `AsyncModifier` in `parser_impl.dart`): the
/// async/sync modifier state of the function currently being parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AsyncModifier {
    Sync,
    Async,
    AsyncStar,
    SyncStar,
}

/// Recursive-descent parser over a scanned token table.
///
/// Construct with [`Parser::new`], drive with the grammar entry points
/// ([`Parser::parse_unit`], [`Parser::parse_type_entry`],
/// [`Parser::parse_pattern_entry`]).
pub struct Parser<'s, 't, L: ParserListener> {
    pub(crate) table: &'t TokenTable,
    pub(crate) cursor: TokenCursor<'s, 't>,
    pub(crate) listener: L,
    pub(crate) features: ParserFeatures,
    /// Official `isLastPatternAllowedInsideUnaryPattern`
    /// (`parser_impl.dart:342`).
    pub(crate) is_last_pattern_allowed_inside_unary_pattern: bool,
    /// Structural nesting depth guard.
    depth: u32,
    /// `asyncState`: modifier state for `inPlainSync` checks.
    pub(crate) async_state: AsyncModifier,
    /// Augment token captured by a declaration-modifier walk, handed to the
    /// declaration parser (official `context.augmentToken`).
    pub(crate) pending_augment: Option<TokenSpan>,
}

impl<'s, 't, L: ParserListener> Parser<'s, 't, L> {
    /// Folds `features` at construction (the official initializer-list fold,
    /// parser_impl.dart:316-373) and starts the cursor at the first token.
    pub fn new(
        source: &'s str,
        table: &'t TokenTable,
        features: ParserFeatures,
        listener: L,
    ) -> Self {
        Self {
            table,
            cursor: TokenCursor::new(source, table),
            listener,
            features,
            is_last_pattern_allowed_inside_unary_pattern: true,
            depth: 0,
            async_state: AsyncModifier::Sync,
            pending_augment: None,
        }
    }

    // --- cursor conveniences ------------------------------------------------

    /// Span of an id this cursor handed out.
    #[inline]
    pub(crate) fn span_of(&self, id: TokenId) -> TokenSpan {
        TokenSpan::new(id, self.cursor.span(id))
    }

    /// Span of the current (peeked) token.
    #[inline]
    pub(crate) fn peek_span(&self) -> TokenSpan {
        self.cursor.peek().span
    }

    /// Span of the real token `n` positions ahead of the cursor (compute
    /// paths only; EOF-pinned).
    #[inline]
    pub(crate) fn peek_span_at(&self, n: usize) -> TokenSpan {
        let id = (self.cursor.real_pos() + n).min(self.table.len() - 1) as TokenId;
        TokenSpan::new(id, self.cursor.span(id))
    }

    /// Real id at an absolute table offset (EOF-pinned).
    #[inline]
    pub(crate) fn real_id(&self, offset: usize) -> TokenId {
        offset.min(self.table.len() - 1) as TokenId
    }

    /// Current token kind.
    #[inline]
    pub(crate) fn peek_kind(&self) -> TokenKind {
        self.cursor.peek().kind
    }

    /// Id of the current (peeked) token.
    #[inline]
    pub(crate) fn peek_id(&self) -> TokenId {
        self.cursor.peek_id()
    }

    /// Inserts a synthetic token at the cursor and returns its span
    /// (zero-width, ast-design-spec §5.1).
    pub(crate) fn synthetic(&mut self, kind: TokenKind, utf16_offset: u32) -> TokenSpan {
        let id = self.cursor.insert_synthetic(kind, utf16_offset);
        self.span_of(id)
    }

    /// Identifier-shaped check on the current token.
    pub(crate) fn peek_is_identifier(&self) -> bool {
        is_identifier(self.cursor.peek().kind)
    }

    // --- depth guard ------------------------------------------------------------

    /// Enters a nested structure; `false` means the cap was hit and the
    /// caller must return immediately without consuming (the enclosing loop
    /// no-progress guards then unwind).
    pub(crate) fn enter_depth(&mut self) -> bool {
        if self.depth >= MAX_DEPTH {
            return false;
        }
        self.depth += 1;
        true
    }

    pub(crate) fn leave_depth(&mut self) {
        self.depth -= 1;
    }

    // --- top-level entry -----------------------------------------------------------

    /// Official `parseUnit`: `begin/end_compilation_unit` around the
    /// top-level declaration loop, with the official no-progress recovery
    /// (report + invalid declaration + force-advance).
    pub fn parse_unit(&mut self) {
        let first = self.peek_span();
        self.listener.begin_compilation_unit(first);
        if self.peek_kind() == TokenKind::ScriptTag {
            self.cursor.advance();
        }
        let mut count = 0usize;
        while !self.cursor.at_eof() {
            let start_pos = self.cursor.peek_id();
            self.parse_top_level_declaration();
            self.listener.end_top_level_declaration(self.peek_span());
            count += 1;
            if self.cursor.peek_id() == start_pos {
                // Recovery: no progress; report and skip the current token.
                let at = self.peek_span();
                self.listener.begin_metadata_star(at);
                self.listener.end_metadata_star(0);
                self.report(
                    crate::message::msg::expected_declaration(self.cursor.lexeme(at.id)),
                    at,
                );
                self.listener.handle_invalid_top_level_declaration(at);
                self.listener.end_top_level_declaration(at);
                self.force_advance();
                count += 1;
            }
        }
        let eof = self.peek_span();
        self.listener.end_compilation_unit(count, eof);
    }

    // --- public grammar entries ----------------------------------------------------

    /// Parses an optional type (not void) at the cursor. Returns the last
    /// consumed span (`before` itself when no type is present, like the
    /// official `parseTypeNotVoid`).
    pub fn parse_type_entry(&mut self) -> TokenSpan {
        self.compute_type(false, false).parse_type_not_void(self)
    }

    /// Parses an optional type or void at the cursor.
    pub fn parse_type_or_void_entry(&mut self) -> TokenSpan {
        self.compute_type(false, false).parse_type(self)
    }

    /// Parses a required type (not void) at the cursor, reporting +
    /// synthesizing when absent.
    pub fn ensure_type_not_void_entry(&mut self) -> TokenSpan {
        self.compute_type(false, false).ensure_type_not_void(self)
    }

    /// Official `reportExperimentNotEnabled` for the `primary-constructors`
    /// gate (the only experiment-gated declaration shape this milestone
    /// parses).
    pub(crate) fn report_experiment_not_enabled(&mut self, at: TokenSpan) {
        self.listener.handle_experiment_not_enabled(
            crate::listener::ExperimentalFlag::PrimaryConstructors,
            at,
            at,
        );
    }

    /// Parses a full pattern at the cursor in `context` (official
    /// `parsePattern`, `parser_impl.dart:11582`).
    pub fn parse_pattern_entry(&mut self, context: crate::listener::PatternContext) -> TokenSpan {
        self.parse_pattern(context)
    }
}
