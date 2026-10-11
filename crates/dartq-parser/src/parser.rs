//! The parser object: cursor + listener + folded features.
//!
//! One generic struct (`L: ParserListener`); grammar families live in their
//! own modules as `impl` blocks on `Parser` (`types`, `patterns`,
//! `expr_const`, `metadata`, `formal_params`, `recovery`).

#![allow(clippy::cast_possible_truncation)] // ids are u32 by design

use dartq_scanner::{TokenKind, TokenTable};

use crate::cursor::TokenCursor;
use crate::features::ParserFeatures;
use crate::listener::ParserListener;
use crate::recovery::is_identifier;
use crate::token_id::{TokenId, TokenSpan};

/// Maximum structural nesting before the parser forces unwinding (official
/// `recoverFromStackOverflow` cap of 500, `parser_impl.dart:6709`).
pub(crate) const MAX_DEPTH: u32 = 500;

/// Per-file diagnostics cap (rust-analyzer parser-bailout practice). A
/// recovery loop that cannot make progress must not stream unbounded
/// diagnostics (and the synthetic tokens and listener events that ride along
/// with each report) into memory. When the cap hits, the parser records one
/// terminal diagnostic, latches the bailout, and every grammar loop unwinds
/// without consuming further tokens.
pub(crate) const MAX_DIAGNOSTICS: usize = 10_000;

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
// The bool fields mirror the official parser's independent state variables
// (lookahead latch, bail-out, constructor-formals context, …); folding them
// into a flags word would obscure that one-to-one mapping.
#[allow(clippy::struct_excessive_bools)]
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
    /// Statement nesting depth (official `statementDepth`, the separate
    /// `parseStatement` counter with the same 500 cap).
    pub(crate) statement_depth: u32,
    /// Diagnostics emitted so far, against [`MAX_DIAGNOSTICS`].
    diagnostics_emitted: u32,
    /// Terminal bailout latch (diagnostics cap or nesting cap): set once,
    /// every grammar loop then unwinds without consuming further tokens.
    bailed: bool,
    /// `asyncState`: modifier state for `inPlainSync` checks.
    pub(crate) async_state: AsyncModifier,
    /// Whether function-literal detection is active at the primary level
    /// (official `mayParseFunctionExpressions`, `parser_impl.dart:276`).
    pub(crate) may_parse_function_expressions: bool,
    /// The constant-pattern context riding the operand parse (official
    /// `constantPatternContext`); set by `parse_pattern_precedence`, reset to
    /// `None` inside plain `parse_precedence`, read by the record-literal
    /// parse for `invalid_constant_pattern_empty_record_literal`.
    pub(crate) constant_pattern_ctx: crate::listener::ConstantPatternContext,
    /// `loopState`: break/continue admissibility of the enclosing construct.
    pub(crate) loop_state: crate::listener::LoopState,
    /// Augment token captured by a declaration-modifier walk, handed to the
    /// declaration parser (official `context.augmentToken`).
    pub(crate) pending_augment: Option<TokenSpan>,
    /// Official `_currentlyRecovering` (`parser_impl.dart:7008`): suppresses
    /// nested written-out-operator recovery while one is being speculated.
    pub(crate) currently_recovering: bool,
    /// `DirectiveContext.state` (`directive_context.dart:20`): where the
    /// unit is in the directive prelude (ordering checks, #300).
    pub(crate) directive_state: crate::directives::DirectiveState,
    /// Set while an `external` constructor's formal parameters parse:
    /// `this.x` field formals report
    /// `external_constructor_with_field_initializers` (`AstBuilder`
    /// `endClassConstructor`, `ast_builder.dart:6160`).
    pub(crate) external_ctor_field_formals: bool,
    /// Set while a constructor's formal parameters parse: `this.x` field
    /// formals there are legal; anywhere else the verifier reports
    /// `field_initializer_outside_constructor` (`error_verifier.dart:5595`).
    pub(crate) in_constructor_formals: bool,
    /// Parameter count of the most recent formal-parameter group (the
    /// `endFormalParameters` count); read by the setter's exactly-one-check
    /// (`AstBuilder` `_ensureSetterFormalParameter`, `ast_builder.dart:6457`).
    pub(crate) last_parameter_count: usize,
    /// Set while an extension type's representation formals parse; the
    /// accumulated shape drives the representation-field error codes
    /// (analyzer `error_verifier` `_checkForExtensionTypeRepresentationErrorCodes`,
    /// error_verifier.dart:5356-5460).
    pub(crate) rep_field_check: Option<RepFieldCheck>,
    /// UTF-16 offset from which the enclosing statement loop drains the
    /// spill run of a primary bracket error. Set when a closer adopted by
    /// [`crate::recovery`] is the scanner's synthetic for an unmatched
    /// begin, or located retroactively by [`crate::recovery`]'s bounded
    /// backward scan: the scan reported that error at the mismatching
    /// token, and the real closers/commas stranded behind it are the region
    /// the official enclosing jumps consume without per-token rows
    /// (`ensureCloseParen` parser_impl.dart:5043-5049, the list drain
    /// :8569-8575; the `moveSynthetic` rationale :5034-5037).
    pub(crate) spill_after: Option<u32>,
    /// Lowest table index the spill backward scan has not yet covered:
    /// fixed at each block's entry (just inside its `{`), so the scan never
    /// leaves the block whose statement loop is running.
    pub(crate) spill_scan_floor: usize,
}

/// Representation-field shape state for one extension type primary
/// constructor (first parameter only, per the official check).
#[derive(Default)]
pub(crate) struct RepFieldCheck {
    pub(crate) count: usize,
    pub(crate) comma_after_first: Option<TokenSpan>,
    pub(crate) delimiter: Option<TokenSpan>,
    pub(crate) modifier: Option<TokenSpan>,
    pub(crate) this_or_super: bool,
    pub(crate) fn_typed: bool,
    pub(crate) has_type: bool,
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
            statement_depth: 0,
            diagnostics_emitted: 0,
            bailed: false,
            async_state: AsyncModifier::Sync,
            may_parse_function_expressions: true,
            constant_pattern_ctx: crate::listener::ConstantPatternContext::None,
            loop_state: crate::listener::LoopState::OutsideLoop,
            pending_augment: None,
            currently_recovering: false,
            directive_state: crate::directives::DirectiveState::Unknown,
            external_ctor_field_formals: false,
            in_constructor_formals: false,
            last_parameter_count: 0,
            rep_field_check: None,
            spill_after: None,
            spill_scan_floor: 0,
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

    /// Span of the token consumed by the latest advance — the construct's
    /// true end token. The official end hooks deliver exactly this (the
    /// `token` a `parseStatement`/block parse returned), never the
    /// follower: `endIfStatement(ifToken, elseToken, token)`
    /// (parser_impl.dart:10148), `endWhileStatement(whileToken, token)`
    /// (:10485-10486), `endForStatement(token)`/:10339-10340,
    /// `endForIn(token)`/:10415-10416. The peek fallback covers an empty
    /// stream.
    #[inline]
    pub(crate) fn last_consumed_span(&self) -> TokenSpan {
        self.cursor
            .last_consumed_id()
            .map(|id| self.span_of(id))
            .unwrap_or_else(|| self.peek_span())
    }

    /// Queue-aware span of the token `n` positions ahead — queued inserted
    /// tokens first, then the real stream. Event paths under queued recovery
    /// must read through this face (or [`TokenCursor::look_kind`]): the raw
    /// table reads desync from the tokens the parser actually sees. O(1);
    /// EOF-pinned past the end.
    #[inline]
    pub(crate) fn look_span_at(&self, n: usize) -> TokenSpan {
        let id = self.cursor.look_id(n);
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

    // --- depth + bailout guards ---------------------------------------------

    /// Enters a nested structure; `false` means a cap was hit (nesting cap,
    /// or the bailout already latched) and the caller must return immediately
    /// without consuming — the enclosing loops' no-progress guards unwind.
    /// Hitting the nesting cap latches the bailout (official
    /// `recoverFromStackOverflow`: report `stackOverflow` once and stop).
    pub(crate) fn enter_depth(&mut self) -> bool {
        if self.bailed {
            return false;
        }
        if self.depth >= MAX_DEPTH {
            self.bail(crate::message::msg::stack_overflow(MAX_DEPTH));
            return false;
        }
        self.depth += 1;
        true
    }

    pub(crate) fn leave_depth(&mut self) {
        self.depth -= 1;
    }

    /// Whether the terminal bailout latched: every grammar loop checks this
    /// and unwinds without consuming further tokens.
    pub(crate) fn is_bailed(&self) -> bool {
        self.bailed
    }

    /// Latches the terminal bailout, recording exactly one bounded diagnostic
    /// (`rust-analyzer` cuts the parse the same way). Idempotent: later cap
    /// hits and descent attempts are silent.
    pub(crate) fn bail(&mut self, message: crate::message::Message) {
        if self.bailed {
            return;
        }
        self.bailed = true;
        let at = self.peek_span();
        self.listener.handle_recoverable_error(message, at, at);
    }

    /// Accounting for one diagnostic against [`MAX_DIAGNOSTICS`]; `false`
    /// means the caller must suppress the report (the cap latched now or
    /// earlier).
    pub(crate) fn admit_diagnostic(&mut self) -> bool {
        if self.bailed {
            return false;
        }
        self.diagnostics_emitted += 1;
        if self.diagnostics_emitted as usize == MAX_DIAGNOSTICS {
            self.bail(crate::message::msg::too_many_errors(MAX_DIAGNOSTICS));
            return false;
        }
        true
    }

    // --- top-level entry -----------------------------------------------------------

    /// Official `parseUnit`: `begin/end_compilation_unit` around the
    /// top-level declaration loop, with the official no-progress recovery
    /// (report + invalid declaration + force-advance).
    pub fn parse_unit(&mut self) {
        let first = self.peek_span();
        self.listener.begin_compilation_unit(first);
        if self.peek_kind() == TokenKind::ScriptTag {
            // `checkScriptTag` (directive_context.dart:31-37): the scanner
            // only produces the tag as the first token, so the state always
            // moves Unknown → Script.
            self.directive_state = crate::directives::DirectiveState::Script;
            self.cursor.advance();
        }
        let mut count = 0usize;
        while !self.cursor.at_eof() && !self.bailed {
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
        if self.admit_diagnostic() {
            self.listener.handle_experiment_not_enabled(
                crate::listener::ExperimentalFlag::PrimaryConstructors,
                at,
                at,
            );
        }
    }

    /// Parses a full pattern at the cursor in `context` (official
    /// `parsePattern`, `parser_impl.dart:11582`).
    pub fn parse_pattern_entry(&mut self, context: crate::listener::PatternContext) -> TokenSpan {
        self.parse_pattern(context)
    }

    /// Builds an independent speculative parser over a snapshot of this
    /// parser's token stream (official `canParseAsConditional`'s listener +
    /// rewriter swap): events and diagnostics of the speculation are dropped
    /// into a [`crate::listener::SpeculationListener`], and insertions stay
    /// in the fork's own cursor side table. The real parser is untouched.
    pub(crate) fn fork_for_speculation(
        &self,
    ) -> Parser<'s, 't, crate::listener::SpeculationListener> {
        Parser {
            table: self.table,
            cursor: self.cursor.fork(),
            listener: crate::listener::SpeculationListener { errors: 0 },
            features: self.features,
            is_last_pattern_allowed_inside_unary_pattern: self
                .is_last_pattern_allowed_inside_unary_pattern,
            depth: self.depth,
            statement_depth: self.statement_depth,
            diagnostics_emitted: self.diagnostics_emitted,
            bailed: self.bailed,
            async_state: self.async_state,
            may_parse_function_expressions: self.may_parse_function_expressions,
            constant_pattern_ctx: self.constant_pattern_ctx,
            loop_state: self.loop_state,
            pending_augment: self.pending_augment,
            currently_recovering: self.currently_recovering,
            directive_state: self.directive_state,
            external_ctor_field_formals: self.external_ctor_field_formals,
            in_constructor_formals: self.in_constructor_formals,
            last_parameter_count: self.last_parameter_count,
            rep_field_check: None,
            spill_after: self.spill_after,
            spill_scan_floor: self.spill_scan_floor,
        }
    }
}
