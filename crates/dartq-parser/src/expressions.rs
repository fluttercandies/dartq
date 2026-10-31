//! The general-expression engine (official `parseExpression` family,
//! `_fe_analyzer_shared-91.0.0/lib/src/parser/parser_impl.dart`).
//!
//! Mirrors the official architecture function by function:
//! `parseExpression` :6416 → `parsePrecedenceExpression` :6527 →
//! `_parsePrecedenceExpressionLoop` :6617 (with `_computePrecedence`
//! :7773), the assignment arm, `parseConditionalExpressionRest` :6503 with
//! `canParseAsConditional` :6479 speculation, `parseCascadeExpression`
//! :7091, `parseUnaryExpression` :7172, `parseArgumentOrIndexStar` :7320,
//! `parsePrimary` :7440, `parseSendOrFunctionLiteral` :8174, `parseSend`
//! :8828, `parseArgumentsRest` :9160, the function-literal family
//! (`parseFunctionExpression` :5806, `parseFunctionLiteral` :5819,
//! `parseNamedFunctionRest` :5854, `parseAsyncOptBody` :5895), switch
//! expressions (`parseSwitchExpression` :11790), throw/await
//! (`parseThrowExpression` :10046, `parseAwaitExpression` :10017), assert
//! expressions (`parseAssert` :10455), pattern assignment
//! (`parsePatternAssignment` :11776 via `looksLikeOuterPatternEquals`
//! :11689), the written-out-operator recovery
//! (`_attemptPrecedenceLevelRecovery` :6901), collection control-flow
//! elements (`literal_entry_info.dart` + `literal_entry_info_impl.dart`),
//! and dot shorthands (`_isDotShorthand` :6499).
//!
//! Decision order is observable (diagnostics and event order), so every arm
//! keeps the official order. The pinned-oracle riders hold:
//! `expected_list_or_map_literal`, `positional_after_named_argument`, and
//! `constructor_with_type_arguments` are never emitted — recovery uses the
//! `expected_token`/`missing_identifier` generic family the pinned 3.13.1
//! oracle produces (the Group B do-not-emit contract: no oracle corpus row
//! produces these codes). Codes
//! with no pinned-oracle surface (`super_null_aware`,
//! `set_or_map_literal_too_many_type_arguments`, `assert_as_expression`,
//! `await_not_async`, `record_literal_zero_fields_with_trailing_comma`)
//! parse structurally and emit events only.
//!
//! The constant-*pattern* engine stays in `expr_const` (the official
//! `parseConstExpression` is likewise a separate entry): patterns keep their
//! reduced precedence walk and constant-pattern over-collection diagnostics
//! there. Shared machinery (collections, strings, arguments) lives in
//! `literals.rs` and here and parses full expressions — exactly where the
//! official shares one implementation.

use dartq_scanner::{Keyword, TokenKind};

use crate::expr_const::SELECTOR_PRECEDENCE;
use crate::listener::{
    AssertKind, BlockKind, ConstructorReferenceContext, IdentifierContext, MemberKind,
    ParserListener,
};
use crate::message::msg;
use crate::parser::{AsyncModifier, Parser};
use crate::recovery::is_identifier;
use crate::statements::AwaitYieldCtx;
use crate::token_id::{TokenId, TokenSpan};
use crate::type_args::TypeParamOrArg;
use crate::types::TypeInfo;

/// Precedence floors (official `token.dart:18-34`; the binary tiers 4-14 and
/// the selector tier live in `expr_const`).
pub(crate) const ASSIGNMENT_PRECEDENCE: u8 = 1;
pub(crate) const CASCADE_PRECEDENCE: u8 = 2;
pub(crate) const CONDITIONAL_PRECEDENCE: u8 = 3;
/// `PREFIX_PRECEDENCE` (`token.dart:2280`): `~`'s own level. It is the only
/// token whose `type.precedence` reaches the loop without a dedicated arm —
/// `_computePrecedence` falls through to `return type.precedence` for it
/// (:7744), so infix `~` lands in the plain-binary else arm and parses as a
/// binary expression (the semantic `not_binary_operator` is the analyzer's
/// row, never a parser one — `var a = 5 ~ 3;` costs zero parser rows in the
/// oracle). `!` shares the level upstream but `_computePrecedence` remaps it
/// to POSTFIX/SELECTOR before it can arrive.
pub(crate) const PREFIX_PRECEDENCE: u8 = 15;
pub(crate) const POSTFIX_PRECEDENCE: u8 = 16;

/// The assignment operators of the binary walk (`token.precedence ==
/// ASSIGNMENT_PRECEDENCE`).
fn is_assignment_kind(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Eq
            | TokenKind::StarEq
            | TokenKind::SlashEq
            | TokenKind::TildeSlashEq
            | TokenKind::PercentEq
            | TokenKind::PlusEq
            | TokenKind::MinusEq
            | TokenKind::LtLtEq
            | TokenKind::GtGtEq
            | TokenKind::GtGtGtEq
            | TokenKind::AmpersandEq
            | TokenKind::CaretEq
            | TokenKind::BarEq
            | TokenKind::QuestionQuestionEq
    )
}

/// Pure precedence of an operator kind, without the `?`/`!` speculation
/// (used by the written-out-operator candidate levels, and as the official
/// `Token.isOperator` notion in initializer-entry recovery).
pub(crate) fn kind_precedence(kind: TokenKind) -> Option<u8> {
    if let Some(level) = crate::expr_const::binary_level(kind) {
        return Some(level);
    }
    Some(match kind {
        k if is_assignment_kind(k) => ASSIGNMENT_PRECEDENCE,
        TokenKind::PeriodPeriod | TokenKind::QuestionPeriodPeriod => CASCADE_PRECEDENCE,
        TokenKind::Question => CONDITIONAL_PRECEDENCE,
        TokenKind::Tilde => PREFIX_PRECEDENCE,
        TokenKind::Bang | TokenKind::PlusPlus | TokenKind::MinusMinus => POSTFIX_PRECEDENCE,
        TokenKind::Period | TokenKind::QuestionPeriod => SELECTOR_PRECEDENCE,
        _ => return None,
    })
}

/// The written-out binary operators of
/// `_tokenRecoveryReplacements` (:6980): the candidate replacements in
/// official order.
fn written_out_candidates(lexeme: &str) -> Option<&'static [TokenKind]> {
    Some(match lexeme {
        "xor" => &[TokenKind::Caret],
        "and" => &[TokenKind::Ampersand, TokenKind::AmpersandAmpersand],
        "or" => &[TokenKind::Bar, TokenKind::BarBar],
        "shl" => &[TokenKind::LtLt],
        "shr" => &[TokenKind::GtGt],
        _ => return None,
    })
}

impl<L: ParserListener> Parser<'_, '_, '_, L> {
    /// `_computePrecedence(next, forPattern: false)` (`parser_impl.dart:7773`)
    /// at the cursor: the level of an infix/postfix operator, `None` when the
    /// token cannot continue an expression. The `?[` null-aware bracket and
    /// postfix-`!` selector shapes decide via [`Self::can_parse_as_conditional`].
    pub(crate) fn precedence_of(&mut self, kind: TokenKind) -> Option<u8> {
        if let Some(level) = crate::expr_const::binary_level(kind) {
            return Some(level);
        }
        Some(match kind {
            k if is_assignment_kind(k) => ASSIGNMENT_PRECEDENCE,
            TokenKind::PeriodPeriod | TokenKind::QuestionPeriodPeriod => CASCADE_PRECEDENCE,
            TokenKind::Question => {
                // "?[" can be a null-aware bracket or a conditional; only the
                // null-aware reading has selector precedence (:7791-7802).
                if self.cursor.look_kind(1) == TokenKind::OpenSquare
                    && !self.can_parse_as_conditional()
                {
                    SELECTOR_PRECEDENCE
                } else {
                    CONDITIONAL_PRECEDENCE
                }
            }
            // `~` carries PREFIX_PRECEDENCE in the token table
            // (`token.dart:2276-2283`) and `_computePrecedence` returns it
            // unmolested (:7744) — it is the only token that reaches the loop
            // at the prefix tier, and the plain-binary else arm consumes it
            // exactly like a real binary operator (zero parser rows; the
            // `not_binary_operator` row is the analyzer's semantic face).
            TokenKind::Tilde => PREFIX_PRECEDENCE,
            TokenKind::Bang => {
                // Postfix `!` is a selector when a selector token follows
                // (:7756-7767), else a plain postfix null assertion.
                match self.cursor.look_kind(1) {
                    TokenKind::Period
                    | TokenKind::Question
                    | TokenKind::OpenParen
                    | TokenKind::OpenSquare
                    | TokenKind::QuestionPeriod => SELECTOR_PRECEDENCE,
                    _ => POSTFIX_PRECEDENCE,
                }
            }
            TokenKind::PlusPlus | TokenKind::MinusMinus => POSTFIX_PRECEDENCE,
            // `(`, `[` and `[]` carry SELECTOR_PRECEDENCE in the token table
            // (`token.dart:2090/2098/1988`); `[]=` is NO_PRECEDENCE there.
            TokenKind::Period
            | TokenKind::QuestionPeriod
            | TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::Index => SELECTOR_PRECEDENCE,
            _ => return None,
        })
    }

    // ------------------------------------------------------------------
    // speculation (official canParseAsConditional :6479)
    // ------------------------------------------------------------------

    /// Whether `? <expressionWithoutCascade> : <expressionWithoutCascade>`
    /// parses without diagnostics from the `?` at the cursor. Runs on a
    /// forked parser (events dropped, stream untouched), so the caller can
    /// re-parse the winning reading for real.
    pub(crate) fn can_parse_as_conditional(&mut self) -> bool {
        let mut spec_listener = crate::listener::SpeculationListener { errors: 0 };
        let mut spec = self.fork_for_speculation(&mut spec_listener);
        spec.cursor.advance(); // the `?` itself
        let _ = spec.parse_expression_without_cascade();
        if spec.listener.errors == 0 && spec.peek_kind() == TokenKind::Colon {
            spec.cursor.advance();
            let _ = spec.parse_expression_without_cascade();
            if spec.listener.errors == 0 {
                return true;
            }
        }
        false
    }

    // ------------------------------------------------------------------
    // entry (official parseExpression :6416)
    // ------------------------------------------------------------------

    /// Official `parseExpression`: pattern assignment when an outer pattern
    /// is followed by `=`, `throw` at expression position, else the
    /// precedence engine at the assignment floor with cascades allowed. The
    /// official per-entry `expressionDepth` recovery is subsumed by the
    /// structural depth guard in [`Self::parse_precedence`].
    pub(crate) fn parse_expression(&mut self) -> TokenSpan {
        if self.features.patterns_enabled && self.looks_like_outer_pattern_equals() {
            return self.parse_pattern_assignment();
        }
        if self.peek_kind() == TokenKind::Keyword(Keyword::Throw) {
            return self.parse_throw_expression(true);
        }
        self.parse_precedence(ASSIGNMENT_PRECEDENCE, true)
    }

    /// Official `parseExpressionWithoutCascade` :6462.
    pub(crate) fn parse_expression_without_cascade(&mut self) -> TokenSpan {
        if self.peek_kind() == TokenKind::Keyword(Keyword::Throw) {
            return self.parse_throw_expression(false);
        }
        self.parse_precedence(ASSIGNMENT_PRECEDENCE, false)
    }

    /// The bare-`SuperExpression` shape probe of the official
    /// `reportErrorIfSuper` family (ast_builder.dart:5781-5792): the
    /// sub-expression is exactly the `super` token. `begin`/`end` are the
    /// sub-expression's first and last consumed tokens — a single-token run
    /// ending on the keyword; any selector, prefix operator, or wrapper
    /// (parenthesized/await/unary) either rebinds the end span or widens the
    /// run (`-super`, `await super`, `(super)`, `super.x` all fail the
    /// probe, matching the official expression-shape test).
    pub(crate) fn is_bare_super_expression(&self, begin: TokenSpan, end: TokenSpan) -> bool {
        begin.id == end.id && self.span_kind(end.id) == TokenKind::Keyword(Keyword::Super)
    }

    /// The frame-lean form of [`Parser::is_bare_super_expression`] for deep
    /// recursion: written-out operator chains recurse once per operator
    /// through this file, and at `-O0` every local is stack frame, so the
    /// begin span is never held across the descent. Instead the caller
    /// pairs the end check with the token immediately before the descent —
    /// the operator/bracket/keyword span the arm already holds. Real token
    /// ids are dense scan indexes (inserted tokens number from
    /// `table.len()`), so "the end token directly follows `prev`" restores
    /// the exact `begin == end` identity; a synthetic end can never be a
    /// `super` token, and no parsed expression ends on the `super` keyword
    /// unless it is the bare `super` itself (`super` admits no trailing
    /// continuation — a trailing `super` swallowed by token-level recovery
    /// implies a non-`super` begin, which the identity check rejects).
    pub(crate) fn ends_bare_super_after(&self, prev: TokenSpan, end: TokenSpan) -> bool {
        end.id == prev.id + 1 && self.span_kind(end.id) == TokenKind::Keyword(Keyword::Super)
    }

    /// The kind-only half of [`Parser::ends_bare_super_after`] for sites
    /// whose predecessor token can be a spliced synthetic (the conditional
    /// colon): an inserted id sits off the real sequence, so the identity
    /// check alone would drop the face — there the kind check carries it.
    pub(crate) fn ends_bare_super(&self, end: TokenSpan) -> bool {
        self.span_kind(end.id) == TokenKind::Keyword(Keyword::Super)
    }

    /// `missingAssignableSelector` over the whole LHS run
    /// (`handleRecoverableError(…, lhs.beginToken, lhs.endToken)`,
    /// ast_builder.dart:3831-3836) plus the statement-level
    /// `illegalAssignmentToNonAssignable` rider anchored on the same run
    /// (:4429-4432; `const [r] = g()` anchors both on the `const` head).
    /// Split out so the run's end spans never occupy the recursion frame of
    /// [`Parser::parse_precedence_inner`] (written-out operator chains
    /// recurse once per operator through it).
    fn report_non_assignable_target(&mut self, lhs_begin: TokenSpan) {
        let lhs_end = self.last_consumed_span();
        let lhs_span = self.span_through(lhs_begin, lhs_end);
        self.report(msg::missing_assignable_selector(), lhs_span);
        self.report(msg::illegal_assignment_to_non_assignable(), lhs_span);
    }

    /// The kind of a token id, reading inserted ids through the cursor (the
    /// assignment arm's read-back idiom).
    pub(crate) fn span_kind(&self, id: TokenId) -> TokenKind {
        if (id as usize) < self.table.len() {
            self.kind_at(self.offset_of(id))
        } else {
            self.cursor.resolve(id).0
        }
    }

    /// Whether the just-parsed operand ends in an assignable shape: the
    /// official test is the expression shape (`_isAssignable`,
    /// ast_builder.dart:6607 — names, property extractions, index targets),
    /// proxied by the operand's last token as in the assignment arm. A `]`
    /// ending reads as the index-target shape — a list literal there is the
    /// rare face and stays on the safe silent side.
    pub(crate) fn operand_ends_assignable(&self) -> bool {
        matches!(
            self.cursor.last_consumed_id().map(|id| self.span_kind(id)),
            Some(kind) if is_identifier(kind) || kind == TokenKind::CloseSquare
        )
    }

    /// `handleRecoverableError(diag, beginToken, endToken)` row extent
    /// (the analyzer's offset/length formula): one
    /// `[begin.offset, end.offset + end.length)` span.
    pub(crate) fn span_through(&self, begin: TokenSpan, end: TokenSpan) -> TokenSpan {
        crate::token_id::TokenSpan::new(
            begin.id,
            dartq_source::SourceRange {
                offset: begin.range.offset,
                length: end.range.offset + end.range.length - begin.range.offset,
            },
        )
    }

    // ------------------------------------------------------------------
    // precedence engine (official parsePrecedenceExpression :6527)
    // ------------------------------------------------------------------

    /// Official `parsePrecedenceExpression`: the dot-shorthand head, the
    /// bang + method-type-arguments block, then the binary walk. The
    /// constant-pattern context is always `none` here (patterns parse
    /// through `expr_const`), so the official pattern-only arms drop out.
    fn parse_precedence(&mut self, precedence: u8, allow_cascades: bool) -> TokenSpan {
        if self.is_bailed() {
            return self.peek_span();
        }
        if self.at_depth_cap() {
            // Official `parseExpression` fuse (parser_impl.dart:6936): a
            // LOCAL recovery — report once, splice a synthetic identifier
            // after the group skip, and let the enclosing frames unwind
            // through their closers by kind. The oracle's post-fuse rows
            // ride that normal unwind, never the fuse itself (see
            // [`Self::recover_stack_overflow_expression`]). While the fuse
            // is latched the trip is not re-reported — the official
            // counter decrements on every exit, so its unwind never
            // re-enters above the cap; the latch reproduces that
            // report-once-per-form face for the collection-control-flow
            // frames that hold their slots until their closers pop.
            if self.depth_fuse_latched {
                return self.peek_span();
            }
            self.depth_fuse_latched = true;
            return self.recover_stack_overflow_expression();
        }
        if !self.enter_depth() {
            return self.peek_span();
        }
        // Plain (non-pattern) expressions drop the constant-pattern context;
        // restore it on exit so an enclosing pattern operand keeps its own.
        let saved_ctx = self.constant_pattern_ctx;
        self.constant_pattern_ctx = crate::listener::ConstantPatternContext::None;
        let end = self.parse_precedence_inner(precedence, allow_cascades);
        self.constant_pattern_ctx = saved_ctx;
        end
    }

    fn parse_precedence_inner(&mut self, precedence: u8, allow_cascades: bool) -> TokenSpan {
        // `_isDotShorthand` (:6499): `.` followed by an identifier or `new`.
        let is_dot_shorthand = self.peek_kind() == TokenKind::Period
            && (is_identifier(self.cursor.look_kind(1))
                || self.cursor.look_kind(1) == TokenKind::Keyword(Keyword::New));
        // The first token of this frame's operand: the official
        // assignment-target reports anchor on the whole LHS's `beginToken`
        // (`missingAssignableSelector` at `lhs.beginToken`,
        // ast_builder.dart:3831-3836; `illegalAssignmentToNonAssignable` at
        // the invalid target's begin, :4429-4432), and every assignment arm
        // in this frame carries exactly this operand as its LHS.
        let lhs_begin = self.peek_span();
        let mut left;
        let mut type_arg = TypeParamOrArg::None;
        if is_dot_shorthand {
            // The head is the `.` itself; the loop at selector precedence
            // parses the shorthand member (:6586-6607).
            left = self.peek_span();
        } else if self.peek_kind() == TokenKind::Period {
            // Recovery: an incomplete dot shorthand like `var x = .`
            // (:6539-6551). `ensure_identifier` reports; the head/context
            // events keep the event shape of a complete shorthand. No unary
            // parse runs in this arm (official falls straight to the loop).
            let dot = self.peek_span();
            self.cursor.advance();
            left = self.ensure_identifier(IdentifierContext::ExpressionContinuation);
            self.listener.handle_dot_shorthand_head(dot);
            self.report_dot_shorthands_gate(dot);
            self.listener.handle_dot_shorthand_context(dot);
            let end =
                self.precedence_loop(precedence, allow_cascades, type_arg, left, false, lhs_begin);
            self.leave_depth();
            return end;
        } else {
            left = self.parse_unary(allow_cascades);
            let (end, ta) = self.parse_method_type_args_block(left);
            left = end;
            type_arg = ta;
        }

        if is_dot_shorthand {
            let dot = self.peek_span();
            left = self.precedence_loop(
                SELECTOR_PRECEDENCE,
                allow_cascades,
                type_arg,
                left,
                true,
                lhs_begin,
            );
            // Capture a trailing null assert before the context event
            // (:6587-6600).
            if self.peek_kind() == TokenKind::Bang {
                let bang = self.peek_span();
                self.listener.handle_non_null_assert_expression(bang);
                self.cursor.advance();
                left = bang;
            }
            self.listener.handle_dot_shorthand_context(dot);
            type_arg = TypeParamOrArg::None;
        }

        let end =
            self.precedence_loop(precedence, allow_cascades, type_arg, left, false, lhs_begin);
        self.leave_depth();
        end
    }

    /// The binary walk (`_parsePrecedenceExpressionLoop` :6617). `left` is
    /// the already-parsed operand span; `type_arg` carries a pending method
    /// type-argument group into the selector arms; `is_dot_shorthand` marks
    /// the first selector step as the dot-shorthand member; `lhs_begin` is
    /// the operand's first token (the official assignment-target report
    /// anchor, `lhs.beginToken`).
    #[allow(clippy::too_many_lines)] // mirrors the official arm structure
    fn precedence_loop(
        &mut self,
        precedence: u8,
        allow_cascades: bool,
        mut type_arg: TypeParamOrArg,
        mut left: TokenSpan,
        mut is_dot_shorthand: bool,
        lhs_begin: TokenSpan,
    ) -> TokenSpan {
        // Whether the operand currently ends in an assignable `]` shape: a
        // selector-attached index group (`a[i]`) or a `.`/`?.` bracket
        // operand replaced by a synthetic name (`c1?.[0]` — null-aware
        // property extraction). The official assignability test is the
        // parsed EXPRESSION shape (`_isAssignable`, ast_builder.dart:6607):
        // index expressions and property extractions are assignable, a list
        // literal is not — both end in `]`, so the shape is carried here
        // instead of re-derived from the last token. Set by the selector
        // index arms and the dot bracket-operand face; every other operand
        // rebinding (binary, cascade, postfix, written-out operators)
        // yields a different shape and clears it.
        let mut lhs_ends_index = false;
        // An identifier here has no precedence (`NO_PRECEDENCE`, :7079) —
        // both the `Some`-below-floor and `None` cases land in the entry
        // recovery attempt (:6644-6659). An accepted written-out operator
        // re-binds `left` and re-resolves the follower's level in this
        // frame — the official walk continues its own loop (:6901-6920).
        // A re-entry here would deepen the native stack once per link of
        // an unbounded `a xor 1 xor 1 ...` chain, invisible to the depth
        // guard (the entry runs outside `enter_depth`).
        let mut level = loop {
            if let Some(l) = self.precedence_of(self.peek_kind())
                && l >= precedence
            {
                break l;
            }
            // Entry-floor recovery attempt (currentLevel = -1).
            if let Some((new_left, _)) = self.attempt_written_out_operator(-1, allow_cascades) {
                left = new_left;
                lhs_ends_index = false;
                is_dot_shorthand = false;
                continue;
            }
            return left;
        };
        let mut last_binary_level: Option<u8> = None;
        let mut last_cascade = false;
        loop {
            let kind = self.peek_kind();
            let Some(token_level) = self.precedence_of(kind) else {
                break;
            };
            if token_level > level {
                break;
            }
            if token_level == CASCADE_PRECEDENCE {
                if !allow_cascades {
                    break;
                }
                // `?..` after a previous cascade section is out of order
                // (:6665-6669).
                if kind == TokenKind::QuestionPeriodPeriod && last_cascade {
                    self.report(msg::null_aware_cascade_out_of_order(), self.peek_span());
                }
                last_cascade = true;
                left = self.parse_cascade_expression(left);
                lhs_ends_index = false;
            } else if token_level == ASSIGNMENT_PRECEDENCE {
                // AstBuilder assignment-target checks: `handleAssignment-
                // Expression` (ast_builder.dart:3824-3841) reports
                // `missingAssignableSelector` for EVERY non-assignable LHS
                // (a bare `super`, a record literal, a `null` literal, ...);
                // a bare `this` is not assignable there either
                // (`_isAssignable`, ast_builder.dart:6607-6620, admits only
                // names, property/index extractions and dot shorthands).
                // `illegalAssignmentToNonAssignable` is the statement-level
                // rider on the invalid assignment target
                // (`endExpressionStatement`, ast_builder.dart:4405-4433);
                // dartq pins it on this arm because the statement layer has
                // no separate target re-check. The statement-level
                // postfix variant (:5849-5860) keeps its own shape.
                // `Token.isOperator`-style proxy for the assignability check
                // (:5856): the LHS must end in an identifier (or the `]` of
                // a selector-attached index target — `lhs_ends_index`
                // distinguishes it from a list literal's closer). The final
                // LHS token's kind: a table id is read back by offset (a
                // span id can alias an inserted token); an inserted
                // (synthetic) token is read by id — its offset points at the
                // real token it stands in for, never at itself.
                let lhs_last = self.cursor.last_consumed_id().map(|id| self.span_kind(id));
                let operator = self.peek_span();
                match lhs_last {
                    // Official `_isAssignable` (ast_builder.dart:6607) tests
                    // the parsed EXPRESSION shape — a bare name qualifies,
                    // and `parsePrimary` reads pseudo/built-in keywords as
                    // names (:7558-7576), so the name-ending face is
                    // official `token.isIdentifier`: identifiers plus
                    // pseudo/built-in keywords (`out = 1`), reserved words
                    // never (`is_identifier`, recovery.rs).
                    Some(kind) if is_identifier(kind) => {}
                    // An index target (`a[i] = 1`, `super[i] = 1`) — the
                    // `]` must close a selector-attached group; a
                    // literal-topped `]` is a list literal and falls through
                    // to the non-assignable arm (`const [r] = g()`).
                    Some(TokenKind::CloseSquare) if lhs_ends_index => {}
                    // `missingAssignableSelector(lhs.beginToken,
                    // lhs.endToken)` (ast_builder.dart:3831-3836): the bare
                    // `this` target is non-assignable exactly like `super`
                    // (`_isAssignable`, :6607 — names, property/index
                    // extractions), so the row rides the arm
                    // (`this = 3;`, `f(this = 3);`, `var x = this = 3;`,
                    // `for (this = 3;;)`, `C() : x = this = 3;`). The
                    // redirecting-initializer head recovery keeps its
                    // silence: its spliced re-read `<synthetic> = this = 0`
                    // builds a this-target assignment the oracle never tracks
                    // (`C() : this = 0;` carries expected_token +
                    // missing_assignment_in_initializer, never the selector),
                    // and that formation is exactly the one whose DIRECTLY
                    // enclosing assignment spliced its operator
                    // ([`Parser::assignment_op_spliced`]).
                    Some(TokenKind::Keyword(Keyword::This))
                        if !self.assignment_op_spliced =>
                    {
                        self.report(msg::missing_assignable_selector(), left);
                    }
                    Some(TokenKind::Keyword(Keyword::This)) => {}
                    Some(TokenKind::Keyword(Keyword::Super)) => {
                        self.report(msg::missing_assignable_selector(), left);
                    }
                    // Both rows span the whole LHS run, anchored on its first
                    // token — [`Parser::report_non_assignable_target`].
                    _ => self.report_non_assignable_target(lhs_begin),
                }
                // Right associative: recurse at the same level (:6671-6691).
                // The `>>=`-without-triple-shift recovery is dead at the pin
                // (3.13.1 ships with triple shift on, and the scanner lexes
                // `>>>=` natively then).
                let is_if_null = self.peek_kind() == TokenKind::QuestionQuestionEq;
                self.cursor.advance();
                // The operator realness flows one frame down: a spliced `=`
                // marks every assignment nested in this RHS as an artifact of
                // the initializer head recovery's re-read (the field clears
                // after the recursion, so it never goes stale).
                self.assignment_op_spliced = (operator.id as usize) >= self.table.len();
                // The `??=` RHS split's begin face (official
                // `ifNullExpression_rightBegin`): the RHS runs on the null
                // path alone, so the flow snapshots the live skip path
                // before the RHS's events run.
                if is_if_null {
                    self.listener.begin_if_null_assignment_rhs(operator);
                }
                let end = if self.peek_kind() == TokenKind::Keyword(Keyword::Throw) {
                    self.parse_throw_expression(allow_cascades)
                } else {
                    self.parse_precedence(level, allow_cascades)
                };
                self.assignment_op_spliced = false;
                // `reportErrorIfSuper(rhs)` (ast_builder.dart:3653): a bare
                // `super` as the assigned value carries the selector-missing
                // row (`x = super;`).
                if self.ends_bare_super_after(operator, end) {
                    self.report(msg::missing_assignable_selector(), end);
                }
                self.listener.handle_assignment_expression(operator, end);
                left = end;
            } else if token_level == POSTFIX_PRECEDENCE {
                match kind {
                    TokenKind::PlusPlus | TokenKind::MinusMinus => {
                        let operator = self.peek_span();
                        // `handleUnaryPostfixAssignmentExpression`
                        // (ast_builder.dart:5627-5642): a non-assignable
                        // operand reports `illegalAssignmentToNonAssignable`
                        // at the operator token (`super++` anchors on the
                        // `++`).
                        if !self.operand_ends_assignable() {
                            self.report(msg::illegal_assignment_to_non_assignable(), operator);
                        }
                        self.listener
                            .handle_unary_postfix_assignment_expression(operator);
                        self.cursor.advance();
                        left = operator;
                    }
                    TokenKind::Bang => {
                        let bang = self.peek_span();
                        self.listener.handle_non_null_assert_expression(bang);
                        self.cursor.advance();
                        left = bang;
                    }
                    _ => break,
                }
            } else if token_level == SELECTOR_PRECEDENCE {
                match kind {
                    TokenKind::Period | TokenKind::QuestionPeriod => {
                        let dot = self.peek_span();
                        self.cursor.advance();
                        // Anonymous-methods experiment (:7292-7295): the
                        // gate reads the token after the dot before
                        // anything else; the anonymous arm fires no
                        // null-aware split and no dot-access member —
                        // begin/endAnonymousMethodInvocation carry the
                        // member instead. With the experiment off (or an
                        // ordinary member following) the ordinary path
                        // below stands, which is the oracle's own posture:
                        // the upstream not-enabled report is an upstream
                        // unimplemented stub and never fires.
                        if self.features.anonymous_methods_enabled && self.begins_anonymous_method()
                        {
                            left = self.parse_anonymous_method(dot);
                            lhs_ends_index = false;
                        } else {
                            // The null-aware split rides the `?`: the member
                            // (and any invocation arguments on it) runs on the
                            // receiver's not-null path — same `rightBegin`
                            // timing as the `?[` face.
                            if kind == TokenKind::QuestionPeriod {
                                self.listener.handle_null_aware_member_begin(dot);
                            }
                            // AstBuilder `doDotExpression` (ast_builder.dart:918):
                            // the operand after `.` must be a name — a literal or
                            // bracket operand (`a?.[0]`) is reported as a missing
                            // identifier and replaced by a synthetic name. The
                            // report anchors here only when the operand parsed as
                            // a complete expression (literal, bracket group, or
                            // keyword head): junk that fell through to the
                            // identifier recovery (`x.;`, `a.class`) already took
                            // its single `expectedIdentifier` row there, and a
                            // second anchor here would double the oracle's one
                            // row (`x.;` surfaces exactly one missing_identifier).
                            let operand = self.peek_span();
                            let operand_kind = self.peek_kind();
                            left = self.parse_primary(IdentifierContext::ExpressionContinuation);
                            if matches!(
                                operand_kind,
                                TokenKind::Int
                                    | TokenKind::Hexadecimal
                                    | TokenKind::Double
                                    | TokenKind::String
                                    | TokenKind::Hash
                                    | TokenKind::OpenParen
                                    | TokenKind::OpenSquare
                                    | TokenKind::Index
                                    | TokenKind::OpenCurly
                                    | TokenKind::Lt
                            ) || matches!(
                                operand_kind,
                                TokenKind::Keyword(
                                    Keyword::True
                                        | Keyword::False
                                        | Keyword::Null
                                        | Keyword::This
                                        | Keyword::Super
                                        | Keyword::Const
                                        | Keyword::Void
                                        | Keyword::Assert
                                )
                            ) {
                                self.report(msg::missing_identifier(), operand);
                            }
                            // A bracket operand (`c1?.[0] = 1`) is replaced by a
                            // synthetic name: the official shape is (null-aware)
                            // property extraction, an assignable target (oracle
                            // null_shorting_index line 13 reports only the
                            // missing_identifier row on the assignment — no
                            // selector rows). The last real token is the `]`, so
                            // the shape flag must carry the assignability.
                            lhs_ends_index =
                                matches!(operand_kind, TokenKind::OpenSquare | TokenKind::Index);
                            if is_dot_shorthand {
                                self.listener.handle_dot_shorthand_head(dot);
                                self.report_dot_shorthands_gate(dot);
                                is_dot_shorthand = false;
                            } else {
                                self.listener.handle_dot_access(
                                    dot,
                                    left,
                                    kind == TokenKind::QuestionPeriod,
                                );
                            }
                            let (end, ta) = self.parse_method_type_args_block(left);
                            left = end;
                            type_arg = ta;
                        }
                    }
                    TokenKind::OpenParen | TokenKind::OpenSquare => {
                        let (end, ta, ends_index) =
                            self.parse_argument_or_index_star(left, type_arg, false);
                        left = end;
                        type_arg = ta;
                        lhs_ends_index = ends_index;
                    }
                    TokenKind::Question => {
                        // Selector-level `?` is a null-aware index (decided
                        // by `precedence_of` via speculation).
                        let (end, ta, ends_index) =
                            self.parse_argument_or_index_star(left, type_arg, true);
                        left = end;
                        type_arg = ta;
                        lhs_ends_index = ends_index;
                    }
                    TokenKind::Index => {
                        // Split `[]` so the index machinery sees `[`
                        // (official `rewriteSquareBrackets` :4743).
                        self.rewrite_square_brackets();
                        let (end, ta, ends_index) =
                            self.parse_argument_or_index_star(left, type_arg, false);
                        left = end;
                        type_arg = ta;
                        lhs_ends_index = ends_index;
                    }
                    TokenKind::Bang => {
                        let bang = self.peek_span();
                        self.listener.handle_non_null_assert_expression(bang);
                        self.cursor.advance();
                        left = bang;
                        lhs_ends_index = false;
                    }
                    _ => {
                        // Recovery (:6796-6801).
                        let at = self.peek_span();
                        self.report(msg::unexpected_token(self.cursor.lexeme(at.id)), at);
                        self.cursor.advance();
                        left = at;
                        lhs_ends_index = false;
                    }
                }
            } else if kind == TokenKind::Keyword(Keyword::Is) {
                // `reportErrorIfSuper(expression)` (ast_builder.dart:4619):
                // the is-operand carries the selector-missing row when it is
                // the bare `super` expression (`super is int`). The frame's
                // first token is the operand's begin — an `await super` or
                // `super.x` operand widens the run and stays silent.
                if self.is_bare_super_expression(lhs_begin, left) {
                    self.report(msg::missing_assignable_selector(), left);
                }
                left = self.parse_is_operator_rest();
                lhs_ends_index = false;
            } else if kind == TokenKind::Keyword(Keyword::As) {
                // `reportErrorIfSuper(expression)` (ast_builder.dart:3619):
                // the as-operand's bare-`super` face, same shape as `is`.
                if self.is_bare_super_expression(lhs_begin, left) {
                    self.report(msg::missing_assignable_selector(), left);
                }
                left = self.parse_as_operator_rest();
                lhs_ends_index = false;
            } else if kind == TokenKind::Question {
                left = self.parse_conditional_expression_rest();
                lhs_ends_index = false;
            } else {
                // Plain binary (:6818-6851). Equality/relational chains are
                // rejected once per tier.
                if level == crate::expr_const::EQUALITY_PRECEDENCE
                    || level == crate::expr_const::RELATIONAL_PRECEDENCE
                {
                    if last_binary_level == Some(level) {
                        self.report(msg::equality_cannot_be_equality_operand(), self.peek_span());
                    } else {
                        last_binary_level = Some(level);
                    }
                }
                let operator = self.peek_span();
                self.listener.begin_binary_expression(operator);
                self.cursor.advance();
                let right = self.parse_precedence(level + 1, allow_cascades);
                // `reportErrorIfSuper(right)` (ast_builder.dart:1079): a bare
                // `super` as the binary's right operand carries the
                // selector-missing row (`a + super`); the left operand is
                // never checked.
                if self.ends_bare_super_after(operator, right) {
                    self.report(msg::missing_assignable_selector(), right);
                }
                let rhs_last = self.last_consumed_span();
                self.listener.handle_binary_rhs_end(operator, rhs_last);
                self.listener.end_binary_expression(operator, right);
                left = right;
                lhs_ends_index = false;
            }
            // Loop bookkeeping (:6874-6920). A `None` precedence is an
            // identifier position — exactly where the official recovery
            // flag fires — so the written-out attempt runs there.
            let next_level: u8 = match self.precedence_of(self.peek_kind()) {
                Some(l) => l,
                None => {
                    if let Some((new_left, _replacement_level)) =
                        self.attempt_written_out_operator(-1, allow_cascades)
                    {
                        left = new_left;
                        lhs_ends_index = false;
                        // Official post-acceptance bookkeeping: recompute the
                        // follower's level and re-run the `<= level` check
                        // with it (:6905-6920).
                        match self.precedence_of(self.peek_kind()) {
                            Some(nl) if nl >= precedence => nl,
                            _ => break,
                        }
                    } else {
                        break;
                    }
                }
            };
            if next_level > level {
                break;
            }
            if next_level < precedence {
                break;
            }
            level = next_level;
        }
        left
    }

    /// `_attemptPrecedenceLevelRecovery` (:6901) reduced to its pinned-live
    /// form: an identifier named `xor`/`and`/`or`/`shl`/`shr` at a binary
    /// position. Candidates are tried in official order (a candidate above
    /// the current level is unreachable and skipped); the first that
    /// speculatively parses cleanly to a good follower is reported
    /// (`binary_operator_written_out`) and consumed as the operator.
    /// Returns `(right operand, replacement precedence)` for the caller's
    /// `left`/`level` when a candidate was accepted.
    fn attempt_written_out_operator(
        &mut self,
        current_level: i16,
        allow_cascades: bool,
    ) -> Option<(TokenSpan, u8)> {
        // Official `_currentlyRecovering` gate: no nested attempts while one
        // is being speculated (:7049, :7077).
        if self.currently_recovering {
            return None;
        }
        let candidates = written_out_candidates(self.cursor.lexeme(self.peek_id()))?;
        if !is_identifier(self.peek_kind()) {
            return None;
        }
        let operator_span = self.peek_span();
        let operator_lexeme = self.cursor.lexeme(operator_span.id);
        for &replacement in candidates {
            let replacement_level = kind_precedence(replacement).expect("candidate operator");
            if current_level >= 0 && i16::from(replacement_level) > current_level {
                continue;
            }
            // Speculative: identifier consumed as the operator, right operand
            // parsed at the replacement's level; accepted when the parse made
            // progress without errors and stopped at a clean follower
            // (:6943-6966).
            let accepted = {
                let mut spec_listener = crate::listener::SpeculationListener { errors: 0 };
                let mut spec = self.fork_for_speculation(&mut spec_listener);
                spec.currently_recovering = true;
                spec.cursor.advance();
                let _ = spec.parse_precedence(replacement_level + 1, allow_cascades);
                let progressed = spec.cursor.peek_id() != operator_span.id;
                let follower_ok = matches!(
                    spec.peek_kind(),
                    TokenKind::Semicolon
                        | TokenKind::Comma
                        | TokenKind::CloseParen
                        | TokenKind::OpenCurly
                        | TokenKind::CloseCurly
                        | TokenKind::Bar
                        | TokenKind::BarBar
                        | TokenKind::Ampersand
                        | TokenKind::AmpersandAmpersand
                        | TokenKind::Eof
                ) || (is_identifier(spec.peek_kind())
                    && written_out_candidates(spec.cursor.lexeme(spec.peek_id())).is_some());
                progressed && spec.listener.errors == 0 && follower_ok
            };
            if accepted {
                self.report(
                    msg::binary_operator_written_out(
                        operator_lexeme,
                        replacement.fixed_lexeme().unwrap_or_default(),
                    ),
                    operator_span,
                );
                self.listener.begin_binary_expression(operator_span);
                self.cursor.advance();
                let right = self.parse_precedence(replacement_level + 1, allow_cascades);
                let rhs_last = self.last_consumed_span();
                self.listener.handle_binary_rhs_end(operator_span, rhs_last);
                self.listener.end_binary_expression(operator_span, right);
                return Some((right, replacement_level));
            }
        }
        None
    }

    /// The shared bang + method-type-arguments block (official, inlined at
    /// the `parsePrecedenceExpression` head :6552-6578 and after every
    /// selector in the loop and in cascades). Consumes an optional postfix
    /// `!` (only when a `<` group follows), parses the group, and returns
    /// `(end, group)` — the group resets to none when no `(` follows, with
    /// `handle_type_argument_application` then emitted.
    pub(crate) fn parse_method_type_args_block(
        &mut self,
        current: TokenSpan,
    ) -> (TokenSpan, TypeParamOrArg) {
        let is_bang =
            self.peek_kind() == TokenKind::Bang && self.cursor.look_kind(1) == TokenKind::Lt;
        // The official probe is `bangToken.next` (`computeMethodTypeArguments`,
        // `type_info.dart:485`): one past the `!` when one is present,
        // otherwise the `<` is the peek itself (`g<A, A>;`, `c.m<D>(...)`).
        let lt_offset = usize::from(is_bang);
        let type_arg = self.compute_type_param_or_arg_at(lt_offset, false);
        if matches!(type_arg, TypeParamOrArg::None) {
            // No group: the official `token` is left unchanged.
            return (current, type_arg);
        }
        // `_mayFollowTypeArgs` (`type_info.dart:521`, testable enumeration
        // :565-574): the split successor of the group's closer decides type
        // arguments vs relational operators (`f(a<b,c>(d))` vs
        // `f(a<b,c>-d)`). A raw compound closer's split remainder `>` is
        // never an allowed follower.
        let before_group = if is_bang {
            self.peek_id()
        } else {
            // The block only runs after a parsed operand, so the cursor has
            // always consumed — a `None` here means a rejected speculative
            // parse left the cursor at the group head. Degrade to the peek
            // id (the group's own head) instead of panicking: the follower
            // probe reads a neighboring token and the parse stays forward.
            self.cursor
                .last_consumed_id()
                .unwrap_or_else(|| self.peek_id())
        };
        // `computeMethodTypeArguments` (type_info.dart:485-491) keeps the
        // group only while the scan never recovered: a recovered scan is a
        // misjudged relational chain (`f<int>(a < b)` parses `a < b` as a
        // relational argument, zero syntax rows), never type arguments.
        if type_arg.ends_on_raw_compound()
            || type_arg.recovered()
            || !self.may_follow_type_args_at(type_arg.skip(before_group))
        {
            return (current, TypeParamOrArg::None);
        }
        if is_bang {
            let bang = self.peek_span();
            self.listener.handle_non_null_assert_expression(bang);
            self.cursor.advance();
        }
        let lt = self.peek_span();
        let end = type_arg.parse_arguments(self);
        if self.peek_kind() != TokenKind::OpenParen {
            // `e<a, b>;` — an applied, non-invoked group (:6569-6577).
            // `reportErrorIfSuper(receiver)` (handleTypeArgumentApplication,
            // ast_builder.dart:5608): a bare `super` receiver of a
            // standalone type-argument group (`super<int>;`,
            // `super<int>.x`) carries the selector-missing row; the invoked
            // flavor (`super<int>(3)`) checks the arguments instead and
            // stays silent. `current` rides the receiver's end token — it
            // is the `super` keyword only for the bare receiver.
            if self.ends_bare_super(current) {
                self.report(msg::missing_assignable_selector(), current);
            }
            self.listener.handle_type_argument_application(lt);
            return (end, TypeParamOrArg::None);
        }
        (end, type_arg)
    }

    /// `_mayFollowTypeArgs`'s allowed follower set (the `TokenType` names
    /// behind the 256-entry table, `type_info.dart:565-574`) read at the id
    /// after `closer`.
    fn may_follow_type_args_at(&self, closer: crate::token_id::TokenId) -> bool {
        matches!(
            self.kind_at(self.offset_of(closer) + 1),
            TokenKind::OpenParen
                | TokenKind::Period
                | TokenKind::EqEq
                | TokenKind::BangEq
                | TokenKind::CloseParen
                | TokenKind::CloseSquare
                | TokenKind::CloseCurly
                | TokenKind::Semicolon
                | TokenKind::Colon
                | TokenKind::Comma
                | TokenKind::Eof
        )
    }

    /// `rewriteSquareBrackets` (:4743/:5119) for the linear cursor: consume
    /// the `[]` token and queue a `[`/`]` pair so the index machinery parses
    /// a bracket group. A real `[]` splits into real tokens — `[` at its own
    /// offset, `]` one past it — so missing-index recovery anchors on the
    /// real `]` (the oracle's `foo[]` face is `missing_identifier` at the
    /// `]`, length 1, never a zero-width synthetic anchor); a synthetic `[]`
    /// keeps the synthetic posture.
    pub(crate) fn rewrite_square_brackets(&mut self) {
        let head = self.cursor.peek();
        let offset = head.span.range.offset;
        self.cursor.advance();
        if head.is_synthetic {
            let open = self.synthetic(TokenKind::OpenSquare, offset);
            let close = self.synthetic(TokenKind::CloseSquare, offset);
            let _ = (open, close);
        } else {
            self.cursor.queue_split(TokenKind::OpenSquare, offset);
            self.cursor.queue_split(TokenKind::CloseSquare, offset + 1);
        }
    }

    /// Splits `?.` into `?` + `.` at a collection-entry boundary (official
    /// `_splitFollowingQuestionPeriod` :8237): the `?` becomes a null-aware
    /// marker, the `.` starts a dot-shorthand value.
    pub(crate) fn split_question_period(&mut self) {
        let offset = self.peek_span().range.offset;
        self.cursor.advance();
        let question = self.synthetic(TokenKind::Question, offset);
        let period = self.synthetic(TokenKind::Period, offset + 1);
        let _ = (question, period);
    }
}

impl<L: ParserListener> Parser<'_, '_, '_, L> {
    // ------------------------------------------------------------------
    // unary (official parseUnaryExpression :7172)
    // ------------------------------------------------------------------

    /// Official `parseUnaryExpression`. The constant-pattern over-collection
    /// reports drop out (patterns parse through `expr_const`).
    fn parse_unary(&mut self, allow_cascades: bool) -> TokenSpan {
        match self.peek_kind() {
            TokenKind::Keyword(Keyword::Await) => {
                if self.async_state == AsyncModifier::Sync
                    && !self.looks_like_await_or_yield_expression(AwaitYieldCtx::UnaryExpression, 0)
                {
                    // `await` reads as an identifier here (parsePrimary via
                    // parseSend, :7187-7193).
                    return self.parse_primary(IdentifierContext::Expression);
                }
                // Recovery: looks like an expression preceded by `await`;
                // fall through so the await parse reports (official comment).
                self.parse_await_expression(allow_cascades)
            }
            TokenKind::Plus => {
                // `unsupportedPrefixPlus` (:7904-7912): `rewriteAndRecover`
                // inserts the synthetic identifier AT the `+` and KEEPS the
                // `+` in the stream — parsePrimary parses the synthetic as a
                // bare send ending at the `+`, and the binary loop then takes
                // the `+` as its operator with the literal RHS
                // (`beginBinaryExpression(+)` in the official
                // keyword_named_class_methods_2.intertwined.expect trace of
                // `return is(x-1) + 1;`). Consuming the `+` here would strand
                // the RHS and mint a spurious `expected_token ';'` at it.
                // The CFE code is pseudo-shared with the analyzer's
                // missing_identifier (diagnostic.g.dart:3008-3013), the
                // pinned oracle surface, so report that directly.
                let plus = self.peek_span();
                self.report(msg::missing_identifier(), plus);
                self.synthetic(TokenKind::Identifier, plus.range.offset);
                self.parse_primary(IdentifierContext::Expression)
            }
            TokenKind::Bang | TokenKind::Tilde | TokenKind::Minus => {
                let is_bang = self.peek_kind() == TokenKind::Bang;
                let operator = self.peek_span();
                self.cursor.advance();
                let end = self.parse_precedence(POSTFIX_PRECEDENCE, allow_cascades);
                // `handleUnaryPrefixExpression` (ast_builder.dart:5795-5803):
                // every prefix operator except `-`/`~` runs
                // `reportErrorIfSuper` on the operand (`!super` reports;
                // `-super`/`~super` stay silent).
                if is_bang && self.span_kind(end.id) == TokenKind::Keyword(Keyword::Super) {
                    self.report(msg::missing_assignable_selector(), end);
                }
                self.listener.handle_unary_prefix_expression(operator);
                end
            }
            TokenKind::PlusPlus | TokenKind::MinusMinus => {
                let operator = self.peek_span();
                self.cursor.advance();
                let end = self.parse_precedence(POSTFIX_PRECEDENCE, allow_cascades);
                // `handleUnaryPrefixAssignmentExpression`
                // (ast_builder.dart:5643-5659): a non-assignable operand
                // reports `missingAssignableSelector` at the operand's
                // endToken (`++super` anchors on the `super`).
                if !self.operand_ends_assignable() {
                    self.report(msg::missing_assignable_selector(), end);
                }
                self.listener
                    .handle_unary_prefix_assignment_expression(operator);
                end
            }
            _ => {
                // Implicit-creation lookahead (:7264-7283): `identifier
                // ('.' identifier)? '<' typeArguments '.' (identifier|new) '('`.
                if let Some(open_angle) = self.looks_like_implicit_creation() {
                    return self.parse_implicit_creation_expression(open_angle);
                }
                self.parse_primary(IdentifierContext::Expression)
            }
        }
    }

    /// The implicit-creation head probe of `parseUnaryExpression`
    /// (:7264-7283): returns the `<` span when the cursor sits on
    /// `identifier ('.' identifier)? <typeArguments '.' (identifier|new) '('`.
    fn looks_like_implicit_creation(&mut self) -> Option<TokenSpan> {
        if !is_identifier(self.peek_kind()) {
            return None;
        }
        let mut name_pos = self.cursor.real_pos();
        if self.kind_at(name_pos + 1) == TokenKind::Period {
            let after_dot = name_pos + 2;
            if !is_identifier(self.kind_at(after_dot)) {
                return None;
            }
            name_pos = after_dot;
        }
        if self.kind_at(name_pos + 1) != TokenKind::Lt {
            return None;
        }
        let type_param =
            self.compute_type_param_or_arg_at(name_pos + 1 - self.cursor.real_pos(), false);
        if matches!(type_param, TypeParamOrArg::None) {
            return None;
        }
        // `skip` expects the token BEFORE `<` (the name), matching the
        // official `typeArg.skip(token)` pointer.
        let after_group = type_param.skip(self.real_id(name_pos));
        let after_pos = self.offset_of(after_group) + 1;
        if self.kind_at(after_pos) != TokenKind::Period {
            return None;
        }
        let after_period = self.kind_at(after_pos + 1);
        let nameish =
            is_identifier(after_period) || after_period == TokenKind::Keyword(Keyword::New);
        if !nameish || self.kind_at(after_pos + 2) != TokenKind::OpenParen {
            return None;
        }
        Some(self.span_of(self.real_id(name_pos + 1)))
    }

    /// Official `parseImplicitCreationExpression` :8953: begin event,
    /// constructor reference, invocation arguments, end event.
    fn parse_implicit_creation_expression(&mut self, open_angle: TokenSpan) -> TokenSpan {
        let begin = self.peek_span();
        self.listener.begin_implicit_creation_expression(begin);
        self.parse_constructor_reference(ConstructorReferenceContext::Implicit);
        let end = self.parse_constructor_invocation_arguments(self.last_consumed_span());
        self.listener
            .end_implicit_creation_expression(begin, open_angle);
        end
    }

    /// Official `parseAwaitExpression` :10017. `await_not_async` has no
    /// pinned-oracle surface, so the sync-context misuse is event-shaped
    /// only (`endInvalidAwaitExpression`), mirroring the yield precedent.
    fn parse_await_expression(&mut self, allow_cascades: bool) -> TokenSpan {
        let await_kw = self.peek_span();
        self.cursor.advance();
        self.listener.begin_await_expression(await_kw);
        let end = self.parse_precedence(POSTFIX_PRECEDENCE, allow_cascades);
        // `reportErrorIfSuper` (ast_builder.dart:1061): a bare `super`
        // operand carries the selector-missing row (`await super`).
        if self.ends_bare_super_after(await_kw, end) {
            self.report(msg::missing_assignable_selector(), end);
        }
        if self.async_state == AsyncModifier::Sync {
            self.listener.end_invalid_await_expression(await_kw, end);
        } else {
            self.listener.end_await_expression(await_kw, end);
        }
        end
    }

    /// Official `parseThrowExpression` :10046.
    fn parse_throw_expression(&mut self, allow_cascades: bool) -> TokenSpan {
        let throw_kw = self.peek_span();
        self.cursor.advance();
        if self.peek_kind() == TokenKind::Semicolon {
            // `throw;` (:10050-10064): report and queue a synthetic empty
            // string operand; the string path consumes it and the `;`
            // follows for the caller.
            let at = self.peek_span();
            self.report(msg::missing_expression_in_throw(), at);
            self.synthetic(TokenKind::String, at.range.offset);
        }
        let end = if allow_cascades {
            self.parse_expression()
        } else {
            self.parse_expression_without_cascade()
        };
        self.listener.handle_throw_expression(throw_kw, end);
        end
    }

    // ------------------------------------------------------------------
    // selectors (official parseArgumentOrIndexStar :7320)
    // ------------------------------------------------------------------

    /// Official `parseArgumentOrIndexStar`: the `(...)`/`[...]` selector
    /// loop. `begin` is the receiving-expression span, `type_arg` a pending
    /// method type-argument group, `checked_null_aware` whether the caller
    /// already resolved the `?[` ambiguity. The third return value is
    /// whether the loop's FINAL step was an index group (the operand ends in
    /// a selector-attached `]`, the assignable index shape).
    fn parse_argument_or_index_star(
        &mut self,
        begin: TokenSpan,
        mut type_arg: TypeParamOrArg,
        checked_null_aware: bool,
    ) -> (TokenSpan, TypeParamOrArg, bool) {
        let mut end = begin;
        let mut ends_index = false;
        loop {
            let mut potential_null_aware = self.peek_kind() == TokenKind::Question
                && self.cursor.look_kind(1) == TokenKind::OpenSquare;
            if potential_null_aware && !checked_null_aware && self.can_parse_as_conditional() {
                potential_null_aware = false;
            }
            if self.peek_kind() == TokenKind::OpenSquare || potential_null_aware {
                let mut question: Option<TokenSpan> = None;
                if self.peek_kind() == TokenKind::Question {
                    question = Some(self.peek_span());
                    self.cursor.advance();
                }
                let open = self.peek_span();
                // The bracket is a delimiter, not the operand: official
                // `parseExpression(next)` starts at `next.next`
                // (`parser_impl.dart:8028`), so consume `[` before the
                // operand — otherwise `[i]` re-parses as a list literal.
                self.cursor.advance();
                // The null-aware split rides the `?`: the operand runs on
                // the receiver's not-null path (official
                // `nullAwareAccess_rightBegin`), so listeners need the
                // marker before the operand's own events stream.
                if let Some(question) = question {
                    self.listener
                        .handle_null_aware_index_operand_begin(question);
                }
                let mut empty_operand: Option<crate::token_id::TokenId> = None;
                if self.peek_kind() == TokenKind::CloseSquare {
                    // Empty index operand (`a[]`, `a[ ]`): the adjacent
                    // bracket pair reaches here split (official
                    // `rewriteSquareBrackets` :4743, sdk issue 42267) and the
                    // missing operand takes the expression template's
                    // synthetic face — one `missing_identifier` anchored on
                    // the `]`, which then closes the index (oracle
                    // issue_42267 lines 4-7: never an extra
                    // `expected_token ']'`).
                    let close = self.peek_span();
                    self.report(msg::missing_identifier(), close);
                    let id = self
                        .cursor
                        .insert_synthetic(TokenKind::Identifier, close.range.offset);
                    // The synthetic splices in at the peek: with no queued
                    // split closer (a real `]` follows) it lands BEFORE the
                    // `]`, behind one (the `[]`-token face) AFTER it. Consume
                    // it in the first case so the closer below is the real
                    // `]` either way — the official splice order is
                    // `[` operand `]` (`rewriteSquareBrackets` :4743).
                    if self.cursor.peek_id() == id {
                        self.cursor.advance();
                    }
                    empty_operand = Some(id);
                } else {
                    let saved = self.may_parse_function_expressions;
                    self.may_parse_function_expressions = true;
                    let index_end = self.parse_expression();
                    self.may_parse_function_expressions = saved;
                    // `reportErrorIfSuper(index)` (ast_builder.dart:4504):
                    // the bracket CONTENT carries the selector-missing row
                    // when it is the bare `super` expression (`g[super]`);
                    // `super[0]` checks the content `0`, never the target.
                    if self.ends_bare_super_after(open, index_end) {
                        self.report(msg::missing_assignable_selector(), index_end);
                    }
                }
                // The closer consume is per-face: the empty-operand face
                // always leaves a `]`-kind token current (real, or the queued
                // split closer), and the expression face recovers below.
                let close;
                if self.peek_kind() == TokenKind::CloseSquare {
                    close = self.peek_span();
                    self.cursor.advance();
                } else if let Some(close_id) = self.cursor.table_matching_end(open.id) {
                    // Recovery (parser_impl.dart:8017-8032): report at the
                    // follower (always — the `isSynthetic` check only decides
                    // HOW the closer is adopted, never whether the row
                    // fires). A scanner-synthetic closer is relocated onto
                    // the expression end (`rewriter.moveSynthetic` :8025-8027)
                    // so the junk run stays current for the enclosing
                    // recovery layer; a real `]` absorbs the junk run
                    // (skip-to-endGroup :8028-8030).
                    let (_, synthetic) = self.cursor.resolve(close_id);
                    if synthetic {
                        // Upstream fires `reportRecoverableError(next,
                        // expectedButGot(']'))` (parser_impl.dart:8010-8013)
                        // at the follower BEFORE `moveSynthetic` relocates the
                        // scanner's synthetic `]` (:8018) — two rows land on
                        // the follower: the full-width `expectedButGot` and
                        // the relocated closer's length-1 unmatched-token row
                        // (`translate_error_token` `unmatchedToken` arm). The
                        // real-`]` and no-pair branches carry their own
                        // `expectedButGot`; the synthetic path needs it here.
                        self.report(msg::expected_but_got("]"), self.peek_span());
                        close = self.move_synthetic_close(
                            open,
                            close_id,
                            TokenKind::CloseSquare,
                            "]",
                        );
                    } else {
                        self.report(msg::expected_but_got("]"), self.peek_span());
                        self.skip_to_token(close_id);
                        close = self.peek_span();
                        self.cursor.advance();
                    }
                } else {
                    // No scanner pair at all: report and close on a synthetic
                    // `]` at the follower.
                    self.report(msg::expected_but_got("]"), self.peek_span());
                    let at = self.peek_span();
                    close = self.synthetic(TokenKind::CloseSquare, at.range.offset);
                    self.cursor.advance();
                }
                // The `[]`-token face leaves the synthetic operand current
                // once the split `]` is consumed; step over it so it cannot
                // resurface as a phantom operand downstream. One bounded
                // step, never an id scan: `advance` is idempotent at EOF, so
                // a search drain here could not terminate (and the official
                // parser never scans for an id — its loop bails at EOF).
                if empty_operand.is_some_and(|id| self.cursor.peek_id() == id) {
                    self.cursor.advance();
                }
                self.listener
                    .handle_indexed_expression(question, open, close);
                ends_index = true;
                let (next_end, ta) = self.parse_method_type_args_block(end);
                end = next_end;
                type_arg = ta;
            } else if self.peek_kind() == TokenKind::OpenParen {
                if matches!(type_arg, TypeParamOrArg::None) {
                    let next = self.peek_span();
                    self.listener.handle_no_type_arguments(next);
                }
                end = self.parse_arguments_rest();
                self.listener.handle_send(begin, end);
                ends_index = false;
                let (next_end, ta) = self.parse_method_type_args_block(end);
                end = next_end;
                type_arg = ta;
            } else {
                break;
            }
        }
        (end, type_arg, ends_index)
    }

    // ------------------------------------------------------------------
    // conditional (official parseConditionalExpressionRest :6503)
    // ------------------------------------------------------------------

    /// Official `parseConditionalExpressionRest`; the cursor sits on the `?`.
    /// The then-arm boundary rides [`ParserListener::end_conditional_expression_then_arm`]
    /// (the official dead-arm rows span the arm's own range); the else arm's
    /// start is derivable at `endConditionalExpression` from the colon.
    fn parse_conditional_expression_rest(&mut self) -> TokenSpan {
        let question = self.peek_span();
        self.cursor.advance();
        self.listener.begin_conditional_expression(question);
        let then_begin = self.peek_span();
        let then = self.parse_expression_without_cascade();
        // `reportErrorIfSuper(thenExpression)` (ast_builder.dart:1301): a
        // bare `super` then-arm carries the selector-missing row.
        if self.is_bare_super_expression(then_begin, then) {
            self.report(msg::missing_assignable_selector(), then);
        }
        let then_end = self.last_consumed_span();
        let colon = self.expect_lexeme_but_got(":");
        self.listener
            .end_conditional_expression_then_arm(question, then_begin, colon, then_end);
        self.listener.handle_conditional_expression_colon();
        let end = self.parse_expression_without_cascade();
        // `reportErrorIfSuper(elseExpression)` (ast_builder.dart:1300). A
        // spliced (synthetic) colon numbers off the real sequence, so the
        // identity check alone would drop the face — there the kind check
        // alone carries it ([`Parser::ends_bare_super`]).
        if self.ends_bare_super(end)
            && (end.id == colon.id + 1 || (colon.id as usize) >= self.table.len())
        {
            self.report(msg::missing_assignable_selector(), end);
        }
        // The else arm's own last token: the `end` above tiles a bare-send
        // operand through its follower, so the dead else-arm row's extent
        // rides this hook (official `flowEnd(elseExpression)`,
        // type_analyzer.dart:2616).
        self.listener
            .end_conditional_expression_else_arm(self.last_consumed_span());
        self.listener
            .end_conditional_expression(question, colon, end);
        end
    }

    // ------------------------------------------------------------------
    // is/as (official parseIsOperatorRest :9306 / parseAsOperatorRest :9342)
    // ------------------------------------------------------------------

    /// Official `parseIsOperatorRest`: `is '!'? type`, then the chained
    /// is/as recovery skip.
    fn parse_is_operator_rest(&mut self) -> TokenSpan {
        let operator = self.peek_span();
        self.cursor.advance();
        let not = self
            .cursor
            .opt_kind(TokenKind::Bang)
            .map(|id| self.span_of(id));
        self.listener.begin_is_operator_type(operator);
        let type_info = self.compute_type_after_is_or_as();
        let left = type_info.ensure_type_not_void(self);
        self.listener.end_is_operator_type(operator);
        self.listener.handle_is_operator(operator, not);
        self.skip_chained_as_is_operators();
        left
    }

    /// Official `parseAsOperatorRest`: `as type`, then the chained skip.
    fn parse_as_operator_rest(&mut self) -> TokenSpan {
        let operator = self.peek_span();
        self.cursor.advance();
        self.listener.begin_as_operator_type(operator);
        let type_info = self.compute_type_after_is_or_as();
        let left = type_info.ensure_type_not_void(self);
        self.listener.end_as_operator_type(operator);
        self.listener.handle_as_operator(operator);
        self.skip_chained_as_is_operators();
        left
    }

    /// Official `skipChainedAsIsOperators` :9359: consecutive `is`/`as` are
    /// illegal; report each and skip its type without events.
    fn skip_chained_as_is_operators(&mut self) {
        loop {
            let operator = match self.peek_kind() {
                TokenKind::Keyword(Keyword::Is | Keyword::As) => self.peek_span(),
                _ => return,
            };
            self.report(
                msg::unexpected_token(self.cursor.lexeme(operator.id)),
                operator,
            );
            self.cursor.advance();
            if self.peek_kind() == TokenKind::Bang {
                self.cursor.advance();
            }
            let type_info = self.compute_type_after_is_or_as();
            type_info.skip_type_raw(self);
        }
    }

    // ------------------------------------------------------------------
    // cascades (official parseCascadeExpression :7091)
    // ------------------------------------------------------------------

    /// `parseSend` at an expression-continuation position with the official
    /// `ensureIdentifier` `new`-rewrite folded in: `expressionContinuation`
    /// allows `new` as an identifier (identifier_context_impl.dart:391,
    /// `allowsNewAsIdentifier => isContinuation`), so `..new()` names the
    /// unnamed constructor and is consumed silently
    /// (`_tryRewriteNewToIdentifier`, parser_impl.dart:4101-4111 — no
    /// report). The generic `ensure_identifier` rewrite set covers the
    /// constructor-reference and method-declaration continuations; cascade
    /// sections parse their names through `parse_send` directly, so this is
    /// their expression-side counterpart.
    fn parse_send_continuation(&mut self) -> TokenSpan {
        if self.peek_kind() == TokenKind::Keyword(Keyword::New) {
            let new_kw = self.peek_span();
            self.cursor.advance();
            self.listener
                .handle_identifier(new_kw, IdentifierContext::ExpressionContinuation);
            return self.parse_send_from_identifier(new_kw);
        }
        self.parse_send(IdentifierContext::ExpressionContinuation)
    }

    /// Upstream `_beginsAnonymousMethod` (parser_impl.dart:7524-7547): the
    /// cursor sits on the token after the `.`/`?.`/`..`/`?..` punctuation.
    /// `{` or `=>` open the member directly; a `(` whose endGroup match is
    /// followed by `{` or `=>` opens an explicitly parameterized one. The
    /// probe reads the raw token table (the `endGroup.next` face); a
    /// recovery synthetic spliced later between the group's closer and the
    /// body would make the body check in
    /// [`Self::parse_anonymous_method`] see a different follower — that
    /// gap is exactly what its missing-body arm covers.
    fn begins_anonymous_method(&self) -> bool {
        match self.peek_kind() {
            TokenKind::OpenCurly | TokenKind::Function => true,
            TokenKind::OpenParen => {
                let after = self
                    .cursor
                    .table_matching_end(self.peek_id())
                    .map(|close| self.kind_at(self.offset_of(close) + 1));
                matches!(after, Some(TokenKind::OpenCurly | TokenKind::Function))
            }
            _ => false,
        }
    }

    /// Upstream `_parseAnonymousMethod` (parser_impl.dart:7550-7590); the
    /// cursor sits on the token after the punctuation and
    /// [`Self::begins_anonymous_method`] just returned true. Explicit
    /// `(params)` go through the formal-parameter parser with
    /// [`crate::listener::MemberKind::AnonymousMethod`], otherwise the
    /// parameters are implicit; the body is a block or an arrow expression
    /// (no cascade), with the upstream missing-body recovery splicing a
    /// synthetic `=>` when neither shape survived to the parse. The member
    /// fires no dot-access/null-aware events — `begin`/
    /// `endAnonymousMethodInvocation` carry it instead.
    fn parse_anonymous_method(&mut self, punctuation: TokenSpan) -> TokenSpan {
        self.listener.begin_anonymous_method_invocation(punctuation);
        if self.peek_kind() == TokenKind::OpenParen {
            let _ = self.parse_formal_parameters_rest(MemberKind::AnonymousMethod);
        } else {
            self.listener.handle_implicit_formal_parameters(punctuation);
        }
        let mut function_definition = None;
        let is_expression;
        let end = match self.peek_kind() {
            TokenKind::OpenCurly => {
                is_expression = false;
                self.parse_block(BlockKind::FunctionBody, crate::message::BlockKind::Body)
            }
            TokenKind::Function => {
                is_expression = true;
                function_definition = Some(self.peek_span());
                // The cursor sits ON the `=>` (dartq's on-token convention;
                // upstream passes the arrow and parses from its `.next`).
                self.cursor.advance();
                self.parse_expression_without_cascade()
            }
            _ => {
                // Upstream `expectedButGot2(expected: "{", expected2: "=>")`
                // — the pseudo-shared re-render surfaces only the first
                // expected (error_converter.dart:89-96), the plain
                // `expected_but_got("{")` face. The synthetic `=>` is
                // spliced in front of the real follower and the expression
                // parse resumes after it.
                let at = self.peek_span();
                self.report(msg::expected_but_got("{"), at);
                let arrow = self
                    .cursor
                    .insert_synthetic(TokenKind::Function, at.range.offset);
                function_definition = Some(self.span_of(arrow));
                is_expression = true;
                self.cursor.advance();
                self.parse_expression_without_cascade()
            }
        };
        self.listener.end_anonymous_method_invocation(
            punctuation,
            function_definition,
            end,
            is_expression,
        );
        end
    }

    /// Official `parseCascadeExpression`; the cursor sits on the `..`/`?..`
    /// operator, `left` the cascade target expression span.
    fn parse_cascade_expression(&mut self, left: TokenSpan) -> TokenSpan {
        let cascade_operator = self.peek_span();
        let is_null_aware_cascade = self.peek_kind() == TokenKind::QuestionPeriodPeriod;
        self.cursor.advance();
        self.listener.begin_cascade(cascade_operator);
        let mut end = if self.peek_kind() == TokenKind::OpenSquare {
            self.parse_argument_or_index_star(left, TypeParamOrArg::None, false)
                .0
        } else if self.features.anonymous_methods_enabled && self.begins_anonymous_method() {
            // Anonymous-methods experiment (:7770-7774): the initial
            // section's member is an anonymous method; no
            // handleCascadeAccess fires for it.
            self.parse_anonymous_method(cascade_operator)
        } else {
            let sent = self.parse_send_continuation();
            self.listener
                .handle_cascade_access(cascade_operator, sent, is_null_aware_cascade);
            sent
        };
        // Cascade sections until no progress (:7121-7161).
        loop {
            let before = self.cursor.peek_id();
            if self.peek_kind() == TokenKind::Period
                || self.peek_kind() == TokenKind::QuestionPeriod
            {
                let period = self.peek_span();
                let is_null_aware = self.peek_kind() == TokenKind::QuestionPeriod;
                self.cursor.advance();
                if self.features.anonymous_methods_enabled && self.begins_anonymous_method() {
                    // Anonymous-methods experiment (:7795-7798): a later
                    // section's member is an anonymous method; no
                    // null-aware split and no dot-access fire for it.
                    end = self.parse_anonymous_method(period);
                } else {
                    if is_null_aware {
                        self.listener.handle_null_aware_member_begin(period);
                    }
                    end = self.parse_send_continuation();
                    self.listener.handle_dot_access(period, end, is_null_aware);
                }
            } else if self.peek_kind() == TokenKind::Bang {
                let bang = self.peek_span();
                self.listener.handle_non_null_assert_expression(bang);
                self.cursor.advance();
                end = bang;
            }
            let (ta_end, ta) = self.parse_method_type_args_block(end);
            end = ta_end;
            let type_arg = ta;
            if self.peek_kind() == TokenKind::Index {
                // Split `[]` here or the cascade stops parsing (:7145-7151).
                self.rewrite_square_brackets();
            }
            let (sel_end, _, _) = self.parse_argument_or_index_star(end, type_arg, false);
            end = sel_end;
            if self.cursor.peek_id() == before {
                break;
            }
        }
        if let Some(level) = self.precedence_of(self.peek_kind())
            && level == ASSIGNMENT_PRECEDENCE
        {
            let assignment = self.peek_span();
            self.cursor.advance();
            let rhs = self.parse_expression_without_cascade();
            self.listener.handle_assignment_expression(assignment, rhs);
            end = rhs;
        }
        self.listener.end_cascade();
        end
    }
}

impl<L: ParserListener> Parser<'_, '_, '_, L> {
    // ------------------------------------------------------------------
    // primary (official parsePrimary :7440)
    // ------------------------------------------------------------------

    /// Official `parsePrimary`. `context` distinguishes the expression head
    /// from a `.`-selector continuation (official `IdentifierContext`).
    #[allow(clippy::too_many_lines)] // mirrors the official branch structure
    pub(crate) fn parse_primary(&mut self, context: IdentifierContext) -> TokenSpan {
        match self.peek_kind() {
            TokenKind::Int | TokenKind::Hexadecimal => {
                let token = self.peek_span();
                self.cursor.advance();
                if self.cursor.lexeme(token.id).contains('_') {
                    self.listener.handle_literal_int_with_separators(token);
                } else {
                    self.listener.handle_literal_int(token);
                }
                token
            }
            TokenKind::Double => {
                let token = self.peek_span();
                self.cursor.advance();
                if self.cursor.lexeme(token.id).contains('_') {
                    self.listener.handle_literal_double_with_separators(token);
                } else {
                    self.listener.handle_literal_double(token);
                }
                token
            }
            TokenKind::String => self.parse_literal_string(),
            TokenKind::Hash => {
                let hash = self.peek_span();
                self.cursor.advance();
                self.listener.begin_literal_symbol(hash);
                self.parse_symbol_rest(hash)
            }
            TokenKind::Keyword(kw) => match kw {
                Keyword::True | Keyword::False => {
                    let token = self.peek_span();
                    self.cursor.advance();
                    self.listener.handle_literal_bool(token);
                    token
                }
                Keyword::Null => {
                    let token = self.peek_span();
                    self.cursor.advance();
                    self.listener.handle_literal_null(token);
                    token
                }
                Keyword::This => self.parse_this_expression(context),
                Keyword::Super => self.parse_super_expression(context),
                Keyword::Augment
                    if self.cursor.look_kind(1) == TokenKind::Keyword(Keyword::Super) =>
                {
                    self.parse_augment_super_expression(context)
                }
                Keyword::New if context == IdentifierContext::ExpressionContinuation => {
                    // After `.`, `new` names the unnamed constructor and
                    // reads as an identifier (official
                    // `_tryRewriteNewToIdentifier` :3684 runs at parsePrimary
                    // entry, so `A.new`/`.new` reach parseSend as one).
                    let identifier = self.peek_span();
                    self.cursor.advance();
                    self.listener.handle_identifier(identifier, context);
                    self.parse_send_from_identifier(identifier)
                }
                Keyword::New => self.parse_new_expression(),
                Keyword::Const => self.parse_const_expression(),
                Keyword::Void => self.parse_send_or_function_literal(context),
                Keyword::Assert => {
                    // `assert` in expression position; the dedicated
                    // assert-as-expression report has no pinned-oracle
                    // surface, so this is the structural event only.
                    self.parse_assert(AssertKind::Expression);
                    self.peek_span()
                }
                Keyword::Switch if self.features.patterns_enabled => self.parse_switch_expression(),
                // Pseudo/built-in keywords read as names (:7558-7576); in a
                // plain-sync context `yield`/`async` skip the
                // function-literal detection (official fall-through order).
                k if !matches!(self.async_state, AsyncModifier::Sync)
                    && matches!(k, Keyword::Yield | Keyword::Async) =>
                {
                    self.parse_send(context)
                }
                k if is_identifier(TokenKind::Keyword(k)) => {
                    self.parse_send_or_function_literal(context)
                }
                Keyword::Return => {
                    // Recovery (:7577-7582).
                    let token = self.peek_span();
                    self.report(msg::unexpected_token(self.cursor.lexeme(token.id)), token);
                    self.cursor.advance();
                    self.parse_primary(context)
                }
                _ => self.parse_send(context),
            },
            TokenKind::Identifier => {
                // Official parsePrimary's IDENTIFIER arm routes through
                // parseSendOrFunctionLiteral (named-function-expression
                // detection).
                self.parse_send_or_function_literal(context)
            }
            TokenKind::OpenParen => {
                self.parse_parenthesized_expression_function_literal_or_record_literal()
            }
            TokenKind::OpenSquare | TokenKind::Index => {
                // `[...]` literal with no type arguments (:7610-7613).
                let next = self.peek_span();
                self.listener.handle_no_type_arguments(next);
                self.parse_list_literal(None)
            }
            TokenKind::OpenCurly => {
                let next = self.peek_span();
                self.listener.handle_no_type_arguments(next);
                self.parse_set_or_map_literal(None)
            }
            TokenKind::Lt => self.parse_literal_list_set_map_or_function(None),
            _ => {
                // Recovery (:7622-7624): parse_send's ensure_identifier
                // reports and synthesizes.
                self.parse_send(context)
            }
        }
    }

    /// Official `parseThisExpression` :7686: the `this` event, plus
    /// constructor-forwarding arguments when `(` follows.
    fn parse_this_expression(&mut self, context: IdentifierContext) -> TokenSpan {
        let this_kw = self.peek_span();
        self.cursor.advance();
        self.listener.handle_this_expression(this_kw, context);
        if self.peek_kind() == TokenKind::OpenParen {
            let next = self.peek_span();
            self.listener.handle_no_type_arguments(next);
            let end = self.parse_arguments_rest();
            self.listener.handle_send(this_kw, end);
            return end;
        }
        this_kw
    }

    /// Official `parseSuperExpression` :7698. `super_null_aware` has no
    /// pinned-oracle surface, so `super?.` parses structurally without a
    /// report.
    fn parse_super_expression(&mut self, context: IdentifierContext) -> TokenSpan {
        let super_kw = self.peek_span();
        self.cursor.advance();
        self.listener.handle_super_expression(super_kw, context);
        if self.peek_kind() == TokenKind::OpenParen {
            let next = self.peek_span();
            self.listener.handle_no_type_arguments(next);
            let end = self.parse_arguments_rest();
            self.listener.handle_send(super_kw, end);
            return end;
        }
        // `super?.x` (`diag.superNullAware`, parser_impl.dart:8466).
        if self.peek_kind() == TokenKind::QuestionPeriod {
            let at = self.peek_span();
            self.report(msg::invalid_operator_questionmark_period_for_super(), at);
        }
        super_kw
    }

    /// Official `parseAugmentSuperExpression` :7720.
    fn parse_augment_super_expression(&mut self, context: IdentifierContext) -> TokenSpan {
        let augment_kw = self.peek_span();
        self.cursor.advance();
        let super_kw = self.peek_span();
        self.cursor.advance();
        self.listener
            .handle_augment_super_expression(augment_kw, super_kw, context);
        if self.peek_kind() == TokenKind::OpenParen {
            let next = self.peek_span();
            self.listener.handle_no_type_arguments(next);
            let end = self.parse_arguments_rest();
            self.listener.handle_send(augment_kw, end);
            return end;
        }
        super_kw
    }

    /// Official `parseNewExpression` :8680, with the literal-with-`new`
    /// recovery family (`literal_with_class_and_new` /
    /// `literal_with_new`, both pinned-live codes).
    fn parse_new_expression(&mut self) -> TokenSpan {
        let new_kw = self.peek_span();
        self.cursor.advance();
        // `new Map<...>{...}` / `new Set<...>{...}` / `new List<...>[...]`:
        // drop the class prefix and parse the literal (:8693-8730).
        if is_identifier(self.peek_kind()) {
            let identifier = self.peek_span();
            let value = self.cursor.lexeme(identifier.id);
            let opens_curly = matches!(value, "Map" | "Set");
            let opens_square = value == "List";
            if (opens_curly || opens_square) && self.cursor.look_kind(1) != TokenKind::Period {
                let type_param = self.compute_type_param_or_arg(false);
                let after = type_param.skip(self.peek_id());
                let follower = self.kind_at(self.offset_of(after) + 1);
                let literal = (opens_curly && follower == TokenKind::OpenCurly)
                    || (opens_square
                        && matches!(follower, TokenKind::OpenSquare | TokenKind::Index));
                if literal {
                    // Official `reportRecoverableErrorWithEnd(newKeyword,
                    // identifier, …)` (parser_impl.dart:8901/8922): the row
                    // spans `new` through the class identifier's end
                    // (`new List` — manual face
                    // literal_with_class_and_new/1: 1:20|8).
                    self.report_with_end(
                        msg::literal_with_class_and_new(
                            &value.to_lowercase(),
                            self.cursor.lexeme(identifier.id),
                        ),
                        new_kw,
                        identifier,
                    );
                    self.cursor.advance();
                    return self.parse_primary(IdentifierContext::Expression);
                }
            }
        } else {
            // `new {`, `new [`, `new []`, `new <...>{...}`: drop the `new`
            // and parse the literal (:8731-8761).
            let follower = self.peek_kind();
            let is_group_open = matches!(
                follower,
                TokenKind::OpenCurly | TokenKind::OpenSquare | TokenKind::Index
            );
            let is_generic_open = follower == TokenKind::Lt && {
                let type_param = self.compute_type_param_or_arg_on_lt(false);
                // The literal opener is read after the group (official
                // `typeArg.skip(newKeyword).next!`), computed from the `<`
                // offset so a raw-compound closer reads its split remainder.
                matches!(
                    self.type_arg_follower(&type_param, self.cursor.real_pos()),
                    TokenKind::OpenCurly | TokenKind::OpenSquare | TokenKind::Index
                )
            };
            if is_group_open || is_generic_open {
                self.report(msg::literal_with_new(), new_kw);
                return self.parse_primary(IdentifierContext::Expression);
            }
        }
        self.listener.begin_new_expression(new_kw);
        self.parse_constructor_reference(ConstructorReferenceContext::New);
        let end = self.parse_constructor_invocation_arguments(self.last_consumed_span());
        self.listener.end_new_expression(new_kw);
        end
    }

    /// Official `parseConstructorInvocationArguments` :8884: arguments (or
    /// the invalid-type-arguments recovery). [token] is the last-consumed
    /// span — the official `token` parameter — so the missing-`(` report
    /// anchors there (parser_impl.dart:8878-8884 reports on `token`): the
    /// consumed identifier (`const Foo;`) or the type-argument closer, never
    /// the bare follower. The pinned oracle never surfaces
    /// `constructor_with_type_arguments` (#96 cull), so the type-argument arm
    /// emits only `handle_invalid_type_arguments` and the generic
    /// `(`-missing recovery.
    pub(crate) fn parse_constructor_invocation_arguments(&mut self, token: TokenSpan) -> TokenSpan {
        if self.peek_kind() == TokenKind::OpenParen {
            return self.parse_arguments_rest();
        }
        let type_arg = self.compute_type_param_or_arg(false);
        if matches!(type_arg, TypeParamOrArg::None) {
            // Official `reportRecoverableError(token, ...)` with its
            // `findNonZeroLengthToken` anchor adjustment (parser_impl.dart:
            // 8878-8884 + parser/util.dart:56): a real anchor reports there
            // (the consumed identifier, e.g. `const Foo;`), while a
            // zero-width synthetic anchor — the enum-element empty name —
            // surfaces on the stream-next token, the current peek (corpus
            // 5ad4a307: `expected_token` at the `;`, length 1).
            let (anchor_kind, anchor_synthetic) = self.cursor.resolve(token.id);
            let at = if anchor_synthetic && anchor_kind != TokenKind::Eof && token.range.length == 0
            {
                self.peek_span()
            } else {
                token
            };
            self.report(msg::expected_token("("), at);
        } else {
            let parsed = type_arg.parse_arguments(self);
            self.listener.handle_invalid_type_arguments(parsed);
        }
        if self.peek_kind() != TokenKind::OpenParen {
            // Official `insertParens(includeIdentifier: false)`: an empty
            // synthetic argument group, then `parseArgumentsRest` rides it.
            let offset = self.peek_span().range.offset;
            self.synthetic(TokenKind::OpenParen, offset);
            self.synthetic(TokenKind::CloseParen, offset);
        }
        self.parse_arguments_rest()
    }

    // ------------------------------------------------------------------
    // sends and function literals (official parseSendOrFunctionLiteral :8174)
    // ------------------------------------------------------------------

    /// Official `parseSendOrFunctionLiteral`: a function literal is only
    /// considered when allowed and not in a `.`-continuation position.
    pub(crate) fn parse_send_or_function_literal(
        &mut self,
        context: IdentifierContext,
    ) -> TokenSpan {
        if !self.may_parse_function_expressions
            || context == IdentifierContext::ExpressionContinuation
        {
            return self.parse_send(context);
        }
        let start = self.cursor.mark();
        let type_info = self.compute_type(false, false);
        type_info.skip_type_raw(self);
        let mut is_function_literal = false;
        if self.peek_is_identifier() {
            let type_param = self.compute_type_param_or_arg(false);
            let after = type_param.skip(self.peek_id());
            let after_pos = self.offset_of(after) + 1;
            if self.kind_at(after_pos) == TokenKind::OpenParen {
                let paren_id = self.real_id(after_pos);
                if let Some(close) = self.cursor.table_matching_end(paren_id) {
                    let body = self.kind_at(self.offset_of(close) + 1);
                    if Self::looks_like_function_body(body) {
                        is_function_literal = true;
                    }
                }
            }
            if is_function_literal {
                self.cursor.reset(start);
                return self.parse_function_literal(&type_info, &type_param);
            }
        }
        self.cursor.reset(start);
        self.parse_send(context)
    }

    /// Official `parseFunctionLiteral` :5819 (the named-function-expression
    /// recovery shape `T name<T>(...) { ... }`). Event order is the
    /// official one: type variables, begin event, return type, then the
    /// named rest; the cursor makes two passes over the header (the type
    /// variables are parsed for events first, then skipped without events
    /// on the way to the formals).
    fn parse_function_literal(
        &mut self,
        type_info: &TypeInfo,
        type_param: &TypeParamOrArg,
    ) -> TokenSpan {
        let start = self.cursor.mark();
        let begin = self.peek_span();
        // Pass 1: type variables (official step 1); the cursor walks past
        // the return type and name without events to reach the `<`.
        type_info.skip_type_raw(self);
        self.cursor.advance(); // the name
        if !matches!(type_param, TypeParamOrArg::None) {
            let _ = type_param.parse_variables(self, false);
        }
        // Pass 2 (official steps 2-5): begin event, return type replayed
        // from the start; the name and formals parse in the named rest.
        self.listener.begin_named_function_expression(begin);
        self.cursor.reset(start);
        let _ = type_info.parse_type(self);
        self.parse_named_function_expression_rest(type_param)
    }

    /// Official `parseNamedFunctionRest` :5854 with
    /// `isFunctionExpression: true` — the expression flavor of the
    /// statement-side parser in `statements.rs`. The cursor sits on the
    /// function name; the type-variable group (already evented in the
    /// detection pass) is skipped without events on the way to the formals.
    fn parse_named_function_expression_rest(&mut self, type_param: &TypeParamOrArg) -> TokenSpan {
        let begin_name = self.peek_span();
        self.listener.begin_function_name(begin_name);
        let name = self.ensure_identifier(IdentifierContext::LocalFunctionDeclaration);
        self.report(msg::named_function_expression(), name);
        let name_end = self.peek_span();
        self.listener.end_function_name(begin_name, name_end, true);
        if !matches!(type_param, TypeParamOrArg::None) {
            let gt_end = type_param.skip(name.id);
            while self.cursor.peek_id() != gt_end && !self.cursor.at_eof() {
                self.cursor.advance();
            }
            self.cursor.advance();
        }
        let _ = self.parse_formal_parameters_required_opt(crate::listener::MemberKind::Local);
        let _ = self.parse_initializers_opt();
        let saved_async = self.async_state;
        let _ = self.parse_async_modifier_opt();
        let end = self.parse_function_body(true, false);
        self.async_state = saved_async;
        self.listener.end_named_function_expression(end);
        end
    }

    /// Official `parseFunctionExpression` :5806: an anonymous function
    /// literal; the cursor sits on `(`.
    fn parse_function_expression(&mut self) -> TokenSpan {
        let begin = self.peek_span();
        self.listener.begin_function_expression(begin);
        let _ = self.parse_formal_parameters_required_opt(crate::listener::MemberKind::Local);
        let end = self.parse_async_opt_body(true, false);
        self.listener.end_function_expression(begin, end);
        end
    }

    /// Official `parseAsyncOptBody` :5895: async modifier then body, with
    /// the enclosing async state saved/restored.
    fn parse_async_opt_body(
        &mut self,
        of_function_expression: bool,
        allow_abstract: bool,
    ) -> TokenSpan {
        let saved = self.async_state;
        let _ = self.parse_async_modifier_opt();
        let end = self.parse_function_body(of_function_expression, allow_abstract);
        self.async_state = saved;
        end
    }

    /// Official `parseParenthesizedExpressionFunctionLiteralOrRecordLiteral`
    /// :7563: `(...)` is a function literal when a body shape follows the
    /// group (or an async/sync modifier, or a stray identifier the user is
    /// typing); otherwise a parenthesized expression or record literal.
    fn parse_parenthesized_expression_function_literal_or_record_literal(&mut self) -> TokenSpan {
        let open = self.peek_span();
        let close = self.cursor.table_matching_end(open.id);
        let after = close.map(|c| self.kind_at(self.offset_of(c) + 1));
        let mut is_function = false;
        if self.may_parse_function_expressions
            && let Some(after) = after
        {
            is_function = matches!(after, TokenKind::Function | TokenKind::OpenCurly)
                || matches!(after, TokenKind::Keyword(Keyword::Async | Keyword::Sync))
                || (is_identifier(after) && {
                    let after_pos = self.offset_of(close.expect("checked above")) + 2;
                    matches!(
                        self.kind_at(after_pos),
                        TokenKind::Function | TokenKind::OpenCurly
                    )
                });
        }
        if is_function {
            let next = self.peek_span();
            self.listener.handle_no_type_variables(next);
            return self.parse_function_expression();
        }
        // The parenthesized/record reading runs with function expressions
        // re-enabled (:8272-8279): the initializer-list gate only bars a
        // top-level `(params) {}` literal, not one nested inside a group —
        // `: d = ((old, _) => f(old))` must still parse the inner literal.
        let saved = self.may_parse_function_expressions;
        self.may_parse_function_expressions = true;
        let end = self.parse_parenthesized_or_record_literal(None);
        self.may_parse_function_expressions = saved;
        // `endParenthesizedExpression` (ast_builder.dart:2613-2624): the
        // parenthesized reading checks the inner expression — the exact
        // `(super)` shape carries the selector-missing row anchored on the
        // `super` (`g((super))`, `((super))` ride the recursion). Record
        // literals and selector tails fail the single-inner-token shape.
        // The check rides this caller (not the shared paren parser) because
        // the official face belongs to the expression reading alone, and
        // the id arithmetic is recovery-proof: an inserted synthetic closer
        // takes an out-of-table id that never equals `open + 2`.
        if end.id == open.id + 2
            && self.span_kind(open.id + 1) == TokenKind::Keyword(Keyword::Super)
        {
            self.report(msg::missing_assignable_selector(), self.span_of(open.id + 1));
        }
        end
    }

    /// `looksLikeFunctionBody` (:9296): `{`, `=>`, `async`, or `sync`.
    fn looks_like_function_body(kind: TokenKind) -> bool {
        matches!(
            kind,
            TokenKind::OpenCurly
                | TokenKind::Function
                | TokenKind::Keyword(Keyword::Async | Keyword::Sync)
        )
    }
}

impl<L: ParserListener> Parser<'_, '_, '_, L> {
    // ------------------------------------------------------------------
    // sends (official parseSend :8828)
    // ------------------------------------------------------------------

    /// Official `parseSend`: the least-costly `Map`/`Set`/`List`
    /// literal-with-class recovery, the name, optional method type
    /// arguments (only when `(` follows and the group is unrecovered),
    /// optional arguments, `handle_send`.
    pub(crate) fn parse_send(&mut self, context: IdentifierContext) -> TokenSpan {
        if is_identifier(self.peek_kind()) {
            let identifier = self.peek_span();
            let value = self.cursor.lexeme(identifier.id);
            let type_param = self.compute_type_param_or_arg(false);
            let after = type_param.skip(self.peek_id());
            let follower = self.kind_at(self.offset_of(after) + 1);
            let is_map_or_set = matches!(value, "Map" | "Set") && follower == TokenKind::OpenCurly;
            // `List[` is indexing, so the list reading requires the `<...>`
            // group; `List[]` is accepted outright (:8856-8862).
            let is_list = value == "List"
                && (follower == TokenKind::Index
                    || (!matches!(type_param, TypeParamOrArg::None)
                        && follower == TokenKind::OpenSquare));
            if is_map_or_set || is_list {
                self.report(
                    msg::literal_with_class(&value.to_lowercase(), value),
                    identifier,
                );
                self.cursor.advance();
                return self.parse_primary(IdentifierContext::Expression);
            }
        }
        let begin = self.ensure_identifier(context);
        self.parse_send_from_identifier(begin)
    }

    /// The tail of [`Self::parse_send`] once the head identifier is
    /// consumed: method type arguments (only when `(` follows and the group
    /// was not recovered, :8880-8889), arguments, `handle_send`. Also the
    /// continuation of a rewritten `new` (`A.new`, `.new`) — official
    /// `_tryRewriteNewToIdentifier` (:3684) makes it an identifier so it can
    /// name the unnamed constructor — and of recovery-spliced synthetic
    /// heads (the initializer synthetic-assignment stream).
    ///
    /// # Send-leaf span tiling (#585)
    ///
    /// For the bare shape (no argument list), the `handle_send` end — and
    /// this function's return — is the *follower* token handed back by
    /// [`Self::parse_arguments_opt`] (`;`, `=`, `)`, ...), left unconsumed.
    /// A listener tiling the leaf as
    /// `[begin.offset, end.offset + end.length)` therefore gets a span that
    /// extends through that follower token; never assume a Send leaf's end
    /// token is the send's own last token. This is load-bearing for anchors
    /// that adopt the completed operand subtree: the Assignment bridge
    /// reads `last_subtree_start` (the dartq-ast builder) over the
    /// just-completed RHS, so in `x = y;` the anchored Assignment inherits
    /// the RHS Send leaf's span and its extent includes the `;`. Tightening
    /// the event to the own-token boundary would shift spans under every
    /// such consumer (the r8 tiling evidence, #563).
    pub(crate) fn parse_send_from_identifier(&mut self, begin: TokenSpan) -> TokenSpan {
        let mut type_arg = TypeParamOrArg::None;
        if self.peek_kind() == TokenKind::Lt {
            let candidate = self.compute_type_param_or_arg_on_lt(false);
            let after = candidate.skip(begin.id);
            if !candidate.recovered()
                && self.kind_at(self.offset_of(after) + 1) == TokenKind::OpenParen
            {
                type_arg = candidate;
            }
        }
        if matches!(type_arg, TypeParamOrArg::None) {
            let next = self.peek_span();
            self.listener.handle_no_type_arguments(next);
        } else {
            let _ = type_arg.parse_arguments(self);
        }
        let end = if self.peek_kind() == TokenKind::OpenParen {
            self.parse_arguments_rest()
        } else {
            // Bare shape: the unconsumed follower token is the event end
            // (send-leaf span tiling, see this function's doc).
            self.parse_arguments_opt()
        };
        self.listener.handle_send(begin, end);
        end
    }

    /// Official `parseArgumentsOpt` :9494. Without an argument list, fires
    /// `handle_no_arguments` with the *peeked follower* token (whatever
    /// follows the send: `;`, `=`, `)`, ...) and returns that same span,
    /// leaving the token unconsumed.
    ///
    /// The return is the send's event end, so a bare-name Send leaf tiles
    /// through the follower — see [`Self::parse_send_from_identifier`] for
    /// the consumer contract (#585).
    pub(crate) fn parse_arguments_opt(&mut self) -> TokenSpan {
        if self.peek_kind() == TokenKind::OpenParen {
            return self.parse_arguments_rest();
        }
        let next = self.peek_span();
        self.listener.handle_no_arguments(next);
        next
    }

    /// Official `parseArgumentsRest` :9160. Function expressions are
    /// allowed inside arguments; a positional-looking token where `,` is
    /// expected splits the arguments (missing-comma recovery), and the
    /// do-not-emit riders hold (`positional_after_named_argument` is
    /// oracle-dead — the pinned oracle splits or closes generically).
    pub(crate) fn parse_arguments_rest(&mut self) -> TokenSpan {
        let begin = self.peek_span();
        self.cursor.advance();
        self.listener.begin_arguments(begin);
        let saved = self.may_parse_function_expressions;
        self.may_parse_function_expressions = true;
        let mut count = 0usize;
        let end;
        loop {
            if self.is_bailed() {
                // Terminal bailout: unwind without consumption (see
                // `parse_list_literal` in literals.rs).
                end = self.peek_span();
                break;
            }
            if self.peek_kind() == TokenKind::CloseParen {
                end = self.peek_span();
                self.cursor.advance();
                break;
            }
            // Named argument: `id:` (a stray leading `:` synthesizes the
            // name through ensure_identifier, :9188-9193). The
            // `handle_named_argument` event fires after the value parse
            // (:9211-9213).
            let mut colon: Option<TokenSpan> = None;
            if self.cursor.look_kind(1) == TokenKind::Colon || self.peek_kind() == TokenKind::Colon
            {
                let _ = self.ensure_identifier(IdentifierContext::NamedArgumentReference);
                colon = Some(self.expect_lexeme_but_got(":"));
            }
            let argument_starts_super =
                self.peek_kind() == TokenKind::Keyword(Keyword::Super);
            let argument_end = self.parse_expression();
            // `reportErrorIfSuper` per argument (ast_builder.dart:961): a
            // bare `super` argument carries the selector-missing row
            // (`g(super)`).
            if argument_starts_super && self.ends_bare_super(argument_end) {
                self.report(msg::missing_assignable_selector(), argument_end);
            }
            if let Some(colon) = colon {
                self.listener.handle_named_argument(colon);
            } else {
                // `handlePositionalArgument` fires once per positional
                // argument, after its expression parse (parser_impl.dart
                // :9608) — the const capture flushes one operand per event.
                self.listener.handle_positional_argument(self.peek_span());
            }
            count += 1;
            if self.peek_kind() != TokenKind::Comma {
                if self.peek_kind() == TokenKind::CloseParen {
                    end = self.peek_span();
                    self.cursor.advance();
                    break;
                }
                if crate::patterns::looks_like_expression_start(self, self.cursor.real_pos()) {
                    // Missing comma: report, insert, continue (:9214-9225).
                    self.report(msg::expected_but_got(","), self.peek_span());
                    self.synthetic(TokenKind::Comma, self.peek_span().range.offset);
                    self.cursor.advance();
                    continue;
                }
                end = self.ensure_close_paren(begin);
                break;
            }
            self.cursor.advance();
        }
        self.may_parse_function_expressions = saved;
        self.listener.end_arguments(count, begin, end);
        end
    }
}

impl<L: ParserListener> Parser<'_, '_, '_, L> {
    // ------------------------------------------------------------------
    // pattern assignment (official parsePatternAssignment :11776)
    // ------------------------------------------------------------------

    /// `looksLikeOuterPatternEquals` :11689 at the cursor (compute-only).
    fn looks_like_outer_pattern_equals(&self) -> bool {
        self.skip_outer_pattern_end()
            .is_some_and(|end| self.kind_at(self.offset_of(end) + 1) == TokenKind::Eq)
    }

    /// `skipOuterPattern` :11700: the id of the last token of an outer
    /// pattern starting at the cursor, or `None` when none starts here.
    /// `parenthesizedPattern | listPattern | mapPattern | recordPattern |
    /// objectPattern`.
    fn skip_outer_pattern_end(&self) -> Option<TokenId> {
        if is_identifier(self.peek_kind()) {
            // `id (.id)? typeArguments? '(' ... ')'` (object pattern). `pos`
            // tracks the last type-name token; the pattern group's `(` is the
            // next offset, except after a `<...>` skip where it lands on `pos`.
            let mut pos = self.cursor.real_pos();
            if self.kind_at(pos + 1) == TokenKind::Period && is_identifier(self.kind_at(pos + 2)) {
                pos += 2;
            }
            if self.kind_at(pos + 1) == TokenKind::Lt {
                let len = self.gt_group_len(pos + 1)?;
                pos += len;
            } else {
                pos += 1;
            }
            if self.kind_at(pos) != TokenKind::OpenParen {
                return None;
            }
            return self.cursor.table_matching_end(self.real_id(pos));
        }
        // `('<' typeArguments)?` then the pattern group; type arguments
        // followed by `(` are a call, not a pattern (:11731-11745).
        let pos = self.cursor.real_pos();
        if self.kind_at(pos) == TokenKind::Lt {
            let len = self.gt_group_len(pos)?;
            let after = pos + len;
            let follower = self.kind_at(after);
            if follower == TokenKind::Index {
                return Some(self.real_id(after));
            }
            if matches!(follower, TokenKind::OpenSquare | TokenKind::OpenCurly) {
                return self.cursor.table_matching_end(self.real_id(after));
            }
            return None;
        }
        match self.kind_at(pos) {
            TokenKind::Index => {
                // Empty list pattern.
                Some(self.real_id(pos))
            }
            TokenKind::OpenSquare | TokenKind::OpenCurly | TokenKind::OpenParen => {
                self.cursor.table_matching_end(self.real_id(pos))
            }
            _ => None,
        }
    }

    /// Official `parsePatternAssignment`: outer pattern, `=`, expression.
    /// The caller checked the `=` first; `expect_kind` keeps a missing one
    /// from desynchronizing the stream.
    fn parse_pattern_assignment(&mut self) -> TokenSpan {
        let _ = self.parse_pattern(crate::listener::PatternContext::Assignment);
        let equals = self.expect_kind(TokenKind::Eq);
        let end = self.parse_expression();
        self.listener.handle_pattern_assignment(equals);
        end
    }

    // ------------------------------------------------------------------
    // collection for-parts (official ForCondition.parse,
    // literal_entry_info_impl.dart:24-91)
    // ------------------------------------------------------------------

    /// The for-parts head of a collection control-flow element:
    /// `begin_for_control_flow`, the `(`-recovery of `parseForLoopPartsStart`
    /// (:9582, shared with the statement form), then the for-parts split.
    /// Returns whether the loop took the for-in shape.
    pub(crate) fn parse_for_control_flow_parts(
        &mut self,
        await_kw: Option<TokenSpan>,
        for_kw: TokenSpan,
    ) -> bool {
        self.listener.begin_for_control_flow(await_kw, for_kw);
        let left_paren = if self.peek_kind() == TokenKind::OpenParen {
            let paren = self.peek_span();
            // Official `parseForLoopPartsStart` consumes the `(` before the
            // initializer (mirrors the statement-side copy in
            // `parse_for_statement`).
            self.cursor.advance();
            paren
        } else {
            // Official parseForLoopPartsStart recovery: a fully synthetic
            // `(...)` group plus a synthetic variable declaration (mirrors
            // the statement-side copy in `parse_for_statement`).
            let at = self.peek_span();
            self.report(msg::expected_but_got("("), at);
            let offset = at.range.offset;
            self.synthetic(TokenKind::OpenParen, offset);
            self.cursor.advance();
            if await_kw.is_some() {
                self.synthetic(TokenKind::Identifier, offset);
                self.cursor.advance();
                self.synthetic(TokenKind::Keyword(Keyword::In), offset);
                self.cursor.advance();
                self.synthetic(TokenKind::Identifier, offset);
                self.cursor.advance();
            } else {
                self.synthetic(TokenKind::Semicolon, offset);
                self.cursor.advance();
                self.synthetic(TokenKind::Semicolon, offset);
                self.cursor.advance();
            }
            let close = self.synthetic(TokenKind::CloseParen, offset);
            self.cursor.advance();
            self.synthetic(TokenKind::Identifier, offset);
            self.cursor.advance();
            self.synthetic(TokenKind::Semicolon, offset);
            self.cursor.advance();
            close
        };
        // The landing report belongs to the C-style rest arm alone (the
        // statement form's landed-token gate, upstream :10329-10331) — the
        // for-in tail reports through `ensureCloseParen`/`moveSynthetic`.
        let outcome = self.parse_expression_statement_or_declaration(true);
        match outcome {
            crate::statements::ExprStmtOrDecl::PatternHead { pattern_keyword } => {
                if self.peek_kind() == TokenKind::Eq {
                    // `for (pattern = expression; ...; ...)` (:49-58).
                    let equals = self.peek_span();
                    self.cursor.advance();
                    let _ = self.parse_expression();
                    self.listener
                        .handle_for_initializer_pattern_variable_assignment(
                            pattern_keyword,
                            equals,
                        );
                    self.parse_for_loop_parts_rest(for_kw, left_paren);
                    false
                } else {
                    // `for (pattern in expression)` (:60-70).
                    self.parse_for_in_loop_parts_rest(
                        await_kw,
                        for_kw,
                        left_paren,
                        Some(pattern_keyword),
                        None,
                    );
                    true
                }
            }
            outcome => {
                let identifier = self.peek_span();
                let identifier_pos = self.cursor.real_pos();
                self.parse_for_loop_parts_mid(await_kw, &outcome);
                if matches!(
                    self.peek_kind(),
                    TokenKind::Keyword(Keyword::In) | TokenKind::Colon
                ) {
                    self.parse_for_in_loop_parts_rest(
                        await_kw,
                        for_kw,
                        left_paren,
                        None,
                        Some((identifier, identifier_pos)),
                    );
                    true
                } else {
                    self.parse_for_loop_parts_rest(for_kw, left_paren);
                    false
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // switch expression (official parseSwitchExpression :11790)
    // ------------------------------------------------------------------

    /// Official `parseSwitchExpression`: the condition, the case block, and
    /// the `pattern [when] => expression` cases with the comma/semicolon
    /// recovery family.
    pub(crate) fn parse_switch_expression(&mut self) -> TokenSpan {
        let switch_kw = self.peek_span();
        self.cursor.advance();
        let saved = self.may_parse_function_expressions;
        self.may_parse_function_expressions = true;
        self.listener.begin_switch_expression(switch_kw);
        self.ensure_parenthesized_condition(false);
        // `ensureBlock(token, BlockKind.switchExpression)` (:12433): a
        // missing body reports `expectedSwitchExpressionBody` — a live
        // pinned-oracle code (block_kind.dart:58, script
        // `method(Never n) => switch (n);`), so it emits, unlike the
        // oracle-dead rider codes; the report anchors on the `)` (the last
        // consumed token, official `reportRecoverableError(token, message)`).
        let had_open = self.peek_kind() == TokenKind::OpenCurly;
        let (begin_switch, close_switch) =
            self.ensure_block(crate::message::BlockKind::SwitchExpression);
        if had_open {
            // Official loop reads cases from `token.next` with `token` = the
            // `{` (beginSwitch); consume it so the cursor sits on the first
            // case.
            self.cursor.advance();
        }
        self.listener.begin_switch_expression_block(begin_switch);
        let mut case_count = 0usize;
        if !had_open {
            // Synthetic empty block: drain the inserted `{`/`}` pair; the
            // real token that triggered recovery stays for the enclosing
            // context (the parse_switch_block convention).
            self.skip_to_token(close_switch.id);
            self.cursor.advance();
            self.listener
                .end_switch_expression_block(0, begin_switch, close_switch);
            self.may_parse_function_expressions = saved;
            self.listener.end_switch_expression(switch_kw, close_switch);
            return close_switch;
        }
        if self.peek_kind() != TokenKind::CloseCurly {
            self.may_parse_function_expressions = false;
            loop {
                if self.is_bailed() {
                    // Terminal bailout: unwind without consumption (see
                    // `parse_list_literal` in literals.rs).
                    break;
                }
                self.listener.begin_switch_expression_case();
                let begin_token = self.peek_span();
                if self.peek_kind() == TokenKind::Keyword(Keyword::Default) {
                    // `default` (:11819-11825): report and parse the
                    // wildcard-shaped remainder.
                    let default_kw = self.peek_span();
                    self.report(msg::default_in_switch_expression(), default_kw);
                    self.listener.handle_no_type(default_kw);
                    self.listener.handle_wildcard_pattern(None, default_kw);
                    self.cursor.advance();
                } else {
                    if self.peek_kind() == TokenKind::Keyword(Keyword::Case) {
                        // `case` in a switch expression (:11827-11833).
                        let case_kw = self.peek_span();
                        self.report(
                            msg::unexpected_token(self.cursor.lexeme(case_kw.id)),
                            case_kw,
                        );
                        self.cursor.advance();
                    }
                    let _ = self.parse_pattern(crate::listener::PatternContext::Matching);
                }
                self.listener
                    .handle_switch_expression_case_pattern(self.peek_span());
                let mut when = None;
                if self.peek_kind() == TokenKind::Keyword(Keyword::When) {
                    let when_kw = self.peek_span();
                    self.cursor.advance();
                    when = Some(when_kw);
                    let _ = self.parse_expression();
                }
                let arrow = if self.peek_kind() == TokenKind::Colon {
                    // `:` in place of `=>` (:11844-11851).
                    let colon = self.peek_span();
                    self.report(msg::expected_but_got("=>"), colon);
                    self.cursor.advance();
                    colon
                } else {
                    self.expect_lexeme_but_got("=>")
                };
                self.may_parse_function_expressions = true;
                let _ = self.parse_expression();
                self.may_parse_function_expressions = false;
                self.listener.end_switch_expression_case(
                    begin_token,
                    when,
                    arrow,
                    self.peek_span(),
                );
                case_count += 1;
                // Case separator (:11858-11882): `,` continues; `;` reports
                // in place of `,`; a pattern-looking token without a
                // separator reports a missing comma; otherwise find the next
                // separator before the closing brace.
                let mut comma = false;
                if self.peek_kind() == TokenKind::Comma {
                    self.cursor.advance();
                    comma = true;
                } else if self.peek_kind() == TokenKind::Semicolon {
                    let semi = self.peek_span();
                    self.report(msg::expected_but_got(","), semi);
                    self.cursor.advance();
                    comma = true;
                }
                if self.peek_kind() == TokenKind::CloseCurly {
                    break;
                }
                if !comma {
                    if crate::patterns::looks_like_pattern_start(self, self.cursor.real_pos()) {
                        self.report(msg::expected_but_got(","), self.peek_span());
                        self.synthetic(TokenKind::Comma, self.peek_span().range.offset);
                        self.cursor.advance();
                    } else {
                        let close_pos = self
                            .cursor
                            .table_matching_end(begin_switch.id)
                            .map(|c| self.offset_of(c));
                        if let Some(sep_pos) = close_pos.and_then(|limit| {
                            self.find_next_comma_or_semicolon(self.cursor.real_pos(), limit)
                        }) {
                            // Official reports the missing comma at the
                            // current token once, whichever separator the
                            // search found (:11890-11897).
                            let at = self.peek_span();
                            self.report(msg::expected_but_got(","), at);
                            self.skip_to_token(self.real_id(sep_pos));
                            self.cursor.advance();
                        } else {
                            // #2024: the recovery row anchors on the token
                            // the skip lands on — the switch expression's
                            // closing `}` — not the blocker (the frozen
                            // oracle anchors the separator recovery row on
                            // the token following the recovery point).
                            let close = self.cursor.table_matching_end(begin_switch.id);
                            let anchor = match close {
                                Some(c) => self.span_of(c),
                                None => self.peek_span(),
                            };
                            self.report(msg::expected_but_got("}"), anchor);
                            if let Some(c) = close {
                                self.skip_to_token(c);
                            }
                            break;
                        }
                    }
                }
            }
        }
        if !self.is_bailed() && self.peek_kind() == TokenKind::CloseCurly {
            self.cursor.advance();
        }
        let end = self.peek_span();
        self.listener
            .end_switch_expression_block(case_count, begin_switch, end);
        self.may_parse_function_expressions = saved;
        self.listener.end_switch_expression(switch_kw, end);
        end
    }

    /// `findNextCommaOrSemicolon` (:11900): the next `,`/`;` at nesting
    /// depth zero, searching from `from` (inclusive) up to `limit`
    /// (exclusive). Group tokens jump past their matched ends.
    fn find_next_comma_or_semicolon(&self, from: usize, limit: usize) -> Option<usize> {
        let mut pos = from;
        while pos < limit {
            match self.kind_at(pos) {
                TokenKind::Comma | TokenKind::Semicolon => return Some(pos),
                TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                    let end = self.cursor.table_matching_end(self.real_id(pos));
                    match end {
                        Some(end) => pos = self.offset_of(end) + 1,
                        None => pos += 1,
                    }
                }
                TokenKind::Eof => return None,
                _ => pos += 1,
            }
        }
        None
    }
}

impl<L: ParserListener> Parser<'_, '_, '_, L> {
    // ------------------------------------------------------------------
    // `<...>` literal/function dispatch (official
    // parseLiteralListSetMapOrFunction :8310)
    // ------------------------------------------------------------------

    /// Official `parseLiteralListSetMapOrFunction`: `<` typeArguments then
    /// `(...)` (function literal), `{...}` (set/map), `[...]` (list), or
    /// recovery.
    pub(crate) fn parse_literal_list_set_map_or_function(
        &mut self,
        const_keyword: Option<TokenSpan>,
    ) -> TokenSpan {
        let type_param = self.compute_type_param_or_arg_on_lt(true);
        // The follower is read AFTER the group (official
        // `typeArg.skip(token).next!`); the cursor still sits on `<`, so the
        // follower is computed from the `<` offset, not peeked.
        let follower = self.type_arg_follower(&type_param, self.cursor.real_pos());
        if follower == TokenKind::OpenParen {
            if let Some(const_keyword) = const_keyword {
                // `const <T>() {}` (:8334-8341).
                let lexeme = self.cursor.lexeme(const_keyword.id).to_owned();
                self.report(msg::unexpected_token(&lexeme), const_keyword);
            }
            let _ = type_param.parse_variables(self, false);
            return self.parse_literal_function_suffix();
        }
        let _ = type_param.parse_arguments(self);
        match self.peek_kind() {
            TokenKind::OpenCurly => {
                // `setOrMapLiteralTooManyTypeArguments` has no pinned-oracle
                // surface, so >2 type arguments parse structurally.
                self.parse_set_or_map_literal(const_keyword)
            }
            TokenKind::OpenSquare | TokenKind::Index => self.parse_list_literal(const_keyword),
            _ => {
                // Recovery (:8360-8366): report, splice the synthetic `[]`
                // token after the type arguments (official
                // `insertSyntheticToken(token, INDEX)`), and parse the
                // empty-list arm over the queued synthetic itself — the
                // follower stays unconsumed (`<int> 3;` then ends the
                // declaration with the walk-back `;` expect and re-parses
                // `3 ;` as a junk member, the pinned oracle cascade).
                self.report(msg::expected_but_got("["), self.peek_span());
                let offset = self.peek_span().range.offset;
                let index = self.synthetic(TokenKind::Index, offset);
                let _ = index;
                self.parse_list_literal_index_arm(const_keyword)
            }
        }
    }

    /// Official `parseLiteralFunctionSuffix` :8749: the body-shape check
    /// reads the token after the formal parameter list
    /// (`token.next!.endGroup!.next!`) — `=>` is the scanner's FUNCTION
    /// token, so arrow bodies pass.
    fn parse_literal_function_suffix(&mut self) -> TokenSpan {
        let open = self.peek_span();
        if let Some(close) = self.cursor.table_matching_end(open.id) {
            let after = self.offset_of(close) + 1;
            let body = self.kind_at(after);
            if !matches!(
                body,
                TokenKind::Function
                    | TokenKind::OpenCurly
                    | TokenKind::Keyword(Keyword::Async | Keyword::Sync)
            ) {
                let id = self.real_id(after);
                let at = self.span_of(id);
                self.report(msg::unexpected_token(self.cursor.lexeme(at.id)), at);
            }
        }
        self.parse_function_expression()
    }
}

// SENTINEL-PRIMARY
