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
//! oracle produces (docs/facts/corpus-expansion-plan.md §7 Group B). Codes
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
use crate::listener::{AssertKind, ConstructorReferenceContext, IdentifierContext, ParserListener};
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

impl<L: ParserListener> Parser<'_, '_, L> {
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
        let mut spec = self.fork_for_speculation();
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

    // ------------------------------------------------------------------
    // precedence engine (official parsePrecedenceExpression :6527)
    // ------------------------------------------------------------------

    /// Official `parsePrecedenceExpression`: the dot-shorthand head, the
    /// bang + method-type-arguments block, then the binary walk. The
    /// constant-pattern context is always `none` here (patterns parse
    /// through `expr_const`), so the official pattern-only arms drop out.
    fn parse_precedence(&mut self, precedence: u8, allow_cascades: bool) -> TokenSpan {
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
            self.listener.handle_dot_shorthand_context(dot);
            let end = self.precedence_loop(precedence, allow_cascades, type_arg, left, false);
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
            left = self.precedence_loop(SELECTOR_PRECEDENCE, allow_cascades, type_arg, left, true);
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

        let end = self.precedence_loop(precedence, allow_cascades, type_arg, left, false);
        self.leave_depth();
        end
    }

    /// The binary walk (`_parsePrecedenceExpressionLoop` :6617). `left` is
    /// the already-parsed operand span; `type_arg` carries a pending method
    /// type-argument group into the selector arms; `is_dot_shorthand` marks
    /// the first selector step as the dot-shorthand member.
    #[allow(clippy::too_many_lines)] // mirrors the official arm structure
    fn precedence_loop(
        &mut self,
        precedence: u8,
        allow_cascades: bool,
        mut type_arg: TypeParamOrArg,
        mut left: TokenSpan,
        mut is_dot_shorthand: bool,
    ) -> TokenSpan {
        // An identifier here has no precedence (`NO_PRECEDENCE`, :7079) —
        // both the `Some`-below-floor and `None` cases land in the entry
        // recovery attempt (:6644-6659).
        let mut level: i16 = match self.precedence_of(self.peek_kind()) {
            Some(l) => l as i16,
            None => {
                if let Some((new_left, _)) = self.attempt_written_out_operator(-1, allow_cascades) {
                    return self.precedence_loop(
                        precedence,
                        allow_cascades,
                        type_arg,
                        new_left,
                        false,
                    );
                }
                return left;
            }
        };
        if level < precedence as i16 {
            // Entry floor (:6644-6659), with the written-out-operator
            // recovery attempt (currentLevel = -1).
            if let Some((new_left, _)) = self.attempt_written_out_operator(-1, allow_cascades) {
                return self.precedence_loop(precedence, allow_cascades, type_arg, new_left, false);
            }
            return left;
        }
        let mut last_binary_level: i16 = -1;
        let mut last_cascade = false;
        loop {
            let kind = self.peek_kind();
            let Some(token_level) = self.precedence_of(kind) else {
                break;
            };
            if token_level as i16 > level {
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
            } else if token_level == ASSIGNMENT_PRECEDENCE {
                // AstBuilder assignment-target check (ast_builder.dart:4428):
                // only identifiers, property accesses, and index expressions
                // are assignable; any other LHS reports
                // `illegal_assignment_to_non_assignable`.
                // `Token.isOperator`-style proxy for AstBuilder
                // `handleAssignment`'s assignability check (:5856): the LHS
                // must end in an identifier (or `]` of an index target).
                // The final LHS token's kind: a table id is read back by
                // offset (a span id can alias an inserted token); an inserted
                // (synthetic) token is read by id — its offset points at the
                // real token it stands in for, never at itself.
                let lhs_assignable = self.cursor.last_consumed_id().is_some_and(|id| {
                    let kind = if (id as usize) < self.table.len() {
                        self.kind_at(self.offset_of(id))
                    } else {
                        self.cursor.resolve(id).0
                    };
                    matches!(kind, TokenKind::Identifier | TokenKind::CloseSquare)
                });
                if !lhs_assignable {
                    self.report(msg::illegal_assignment_to_non_assignable(), left);
                }
                // Right associative: recurse at the same level (:6671-6691).
                // The `>>=`-without-triple-shift recovery is dead at the pin
                // (3.13.1 ships with triple shift on, and the scanner lexes
                // `>>>=` natively then).
                let operator = self.peek_span();
                self.cursor.advance();
                let end = if self.peek_kind() == TokenKind::Keyword(Keyword::Throw) {
                    self.parse_throw_expression(allow_cascades)
                } else {
                    self.parse_precedence(level as u8, allow_cascades)
                };
                self.listener.handle_assignment_expression(operator, end);
                left = end;
            } else if token_level == POSTFIX_PRECEDENCE {
                match kind {
                    TokenKind::PlusPlus | TokenKind::MinusMinus => {
                        let operator = self.peek_span();
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
                        // AstBuilder `doDotExpression` (ast_builder.dart:918):
                        // the operand after `.` must be a name — a literal or
                        // bracket operand (`a?.[0]`) is reported as a missing
                        // identifier and replaced by a synthetic name.
                        let operand = self.peek_span();
                        let operand_kind = self.peek_kind();
                        left = self.parse_primary(IdentifierContext::ExpressionContinuation);
                        if !is_identifier(operand_kind)
                            && operand_kind != TokenKind::Keyword(Keyword::New)
                        {
                            self.report(msg::missing_identifier(), operand);
                        }
                        if is_dot_shorthand {
                            self.listener.handle_dot_shorthand_head(dot);
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
                    TokenKind::OpenParen | TokenKind::OpenSquare => {
                        let (end, ta) = self.parse_argument_or_index_star(left, type_arg, false);
                        left = end;
                        type_arg = ta;
                    }
                    TokenKind::Question => {
                        // Selector-level `?` is a null-aware index (decided
                        // by `precedence_of` via speculation).
                        let (end, ta) = self.parse_argument_or_index_star(left, type_arg, true);
                        left = end;
                        type_arg = ta;
                    }
                    TokenKind::Index => {
                        // Split `[]` so the index machinery sees `[`
                        // (official `rewriteSquareBrackets` :4743).
                        self.rewrite_square_brackets();
                        let (end, ta) = self.parse_argument_or_index_star(left, type_arg, false);
                        left = end;
                        type_arg = ta;
                    }
                    TokenKind::Bang => {
                        let bang = self.peek_span();
                        self.listener.handle_non_null_assert_expression(bang);
                        self.cursor.advance();
                        left = bang;
                    }
                    _ => {
                        // Recovery (:6796-6801).
                        let at = self.peek_span();
                        self.report(msg::unexpected_token(self.cursor.lexeme(at.id)), at);
                        self.cursor.advance();
                        left = at;
                    }
                }
            } else if kind == TokenKind::Keyword(Keyword::Is) {
                left = self.parse_is_operator_rest();
            } else if kind == TokenKind::Keyword(Keyword::As) {
                left = self.parse_as_operator_rest();
            } else if kind == TokenKind::Question {
                left = self.parse_conditional_expression_rest();
            } else {
                // Plain binary (:6818-6851). Equality/relational chains are
                // rejected once per tier.
                if level == crate::expr_const::EQUALITY_PRECEDENCE as i16
                    || level == crate::expr_const::RELATIONAL_PRECEDENCE as i16
                {
                    if last_binary_level == level {
                        self.report(msg::equality_cannot_be_equality_operand(), self.peek_span());
                    } else {
                        last_binary_level = level;
                    }
                }
                let operator = self.peek_span();
                self.listener.begin_binary_expression(operator);
                self.cursor.advance();
                let right = self.parse_precedence(level as u8 + 1, allow_cascades);
                self.listener.end_binary_expression(operator, right);
                left = right;
            }
            // Loop bookkeeping (:6874-6920). A `None` precedence is an
            // identifier position — exactly where the official recovery
            // flag fires — so the written-out attempt runs there.
            let next_level: i16 = match self.precedence_of(self.peek_kind()) {
                Some(l) => l as i16,
                None => {
                    if let Some((new_left, _replacement_level)) =
                        self.attempt_written_out_operator(-1, allow_cascades)
                    {
                        left = new_left;
                        // Official post-acceptance bookkeeping: recompute the
                        // follower's level and re-run the `<= level` check
                        // with it (:6905-6920).
                        match self.precedence_of(self.peek_kind()) {
                            Some(nl) if nl >= precedence => nl as i16,
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
            if next_level < precedence as i16 {
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
        let operator_lexeme = self.cursor.lexeme(operator_span.id).to_owned();
        for &replacement in candidates {
            let replacement_level = kind_precedence(replacement).expect("candidate operator");
            if current_level >= 0 && replacement_level as i16 > current_level {
                continue;
            }
            // Speculative: identifier consumed as the operator, right operand
            // parsed at the replacement's level; accepted when the parse made
            // progress without errors and stopped at a clean follower
            // (:6943-6966).
            let accepted = {
                let mut spec = self.fork_for_speculation();
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
                        &operator_lexeme,
                        replacement.fixed_lexeme().unwrap_or_default(),
                    ),
                    operator_span,
                );
                self.listener.begin_binary_expression(operator_span);
                self.cursor.advance();
                let right = self.parse_precedence(replacement_level + 1, allow_cascades);
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
            self.cursor
                .last_consumed_id()
                .expect("the block only runs after a parsed operand")
        };
        if type_arg.ends_on_raw_compound()
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

    /// `rewriteSquareBrackets` (:4743) for the linear cursor: consume the
    /// real `[]` token and queue synthetic `[`/`]` at its offset so the
    /// index machinery parses a bracket group.
    pub(crate) fn rewrite_square_brackets(&mut self) {
        let offset = self.peek_span().range.offset;
        self.cursor.advance();
        let open = self.synthetic(TokenKind::OpenSquare, offset);
        let close = self.synthetic(TokenKind::CloseSquare, offset);
        let _ = (open, close);
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

impl<L: ParserListener> Parser<'_, '_, L> {
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
                // `unsupportedPrefixPlus` (:7196-7204): report + synthetic
                // identifier operand; the `+` then falls to the binary loop.
                // The CFE code is pseudo-shared with the analyzer's
                // missing_identifier (error_converter.dart), which is what
                // the pinned oracle surfaces, so report that directly.
                let plus = self.peek_span();
                self.report(msg::missing_identifier(), plus);
                let synthetic = self.synthetic(TokenKind::Identifier, plus.range.offset);
                self.listener
                    .handle_identifier(synthetic, IdentifierContext::Expression);
                self.parse_primary(IdentifierContext::Expression)
            }
            TokenKind::Bang | TokenKind::Tilde => {
                let operator = self.peek_span();
                self.cursor.advance();
                let end = self.parse_precedence(POSTFIX_PRECEDENCE, allow_cascades);
                self.listener.handle_unary_prefix_expression(operator);
                end
            }
            TokenKind::Minus => {
                let operator = self.peek_span();
                self.cursor.advance();
                let end = self.parse_precedence(POSTFIX_PRECEDENCE, allow_cascades);
                self.listener.handle_unary_prefix_expression(operator);
                end
            }
            TokenKind::PlusPlus | TokenKind::MinusMinus => {
                let operator = self.peek_span();
                self.cursor.advance();
                let end = self.parse_precedence(POSTFIX_PRECEDENCE, allow_cascades);
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
        let end = self.parse_constructor_invocation_arguments();
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
        if self.async_state != AsyncModifier::Sync {
            self.listener.end_await_expression(await_kw, end);
        } else {
            self.listener.end_invalid_await_expression(await_kw, end);
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
    /// already resolved the `?[` ambiguity.
    fn parse_argument_or_index_star(
        &mut self,
        begin: TokenSpan,
        mut type_arg: TypeParamOrArg,
        checked_null_aware: bool,
    ) -> (TokenSpan, TypeParamOrArg) {
        let mut end = begin;
        loop {
            let mut potential_null_aware = self.peek_kind() == TokenKind::Question
                && self.cursor.look_kind(1) == TokenKind::OpenSquare;
            if potential_null_aware && !checked_null_aware {
                if self.can_parse_as_conditional() {
                    potential_null_aware = false;
                }
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
                let saved = self.may_parse_function_expressions;
                self.may_parse_function_expressions = true;
                let _ = self.parse_expression();
                self.may_parse_function_expressions = saved;
                if self.peek_kind() != TokenKind::CloseSquare {
                    // Recovery (:7349-7366): report; the scanner pairs the
                    // group, so jump to its closer (silently when the
                    // scanner synthesized it).
                    if let Some(close) = self.cursor.table_matching_end(open.id) {
                        let (_, synthetic) = self.cursor.resolve(close);
                        if !synthetic {
                            self.report(msg::expected_but_got("]"), self.peek_span());
                        }
                        self.skip_to_token(close);
                    } else {
                        self.report(msg::expected_but_got("]"), self.peek_span());
                        let at = self.peek_span();
                        self.synthetic(TokenKind::CloseSquare, at.range.offset);
                        self.cursor.advance();
                    }
                }
                let close = self.peek_span();
                self.cursor.advance();
                self.listener
                    .handle_indexed_expression(question, open, close);
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
                let (next_end, ta) = self.parse_method_type_args_block(end);
                end = next_end;
                type_arg = ta;
            } else {
                break;
            }
        }
        (end, type_arg)
    }

    // ------------------------------------------------------------------
    // conditional (official parseConditionalExpressionRest :6503)
    // ------------------------------------------------------------------

    /// Official `parseConditionalExpressionRest`; the cursor sits on the `?`.
    fn parse_conditional_expression_rest(&mut self) -> TokenSpan {
        let question = self.peek_span();
        self.cursor.advance();
        self.listener.begin_conditional_expression(question);
        let _ = self.parse_expression_without_cascade();
        let colon = self.expect_lexeme_but_got(":");
        self.listener.handle_conditional_expression_colon();
        let end = self.parse_expression_without_cascade();
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
        } else {
            let sent = self.parse_send(IdentifierContext::ExpressionContinuation);
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
                end = self.parse_send(IdentifierContext::ExpressionContinuation);
                self.listener.handle_dot_access(period, end, is_null_aware);
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
            let (sel_end, _) = self.parse_argument_or_index_star(end, type_arg, false);
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

impl<L: ParserListener> Parser<'_, '_, L> {
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
                    return self.parse_primary(context);
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
                    self.report(
                        msg::literal_with_class_and_new(
                            &value.to_lowercase(),
                            &self.cursor.lexeme(identifier.id),
                        ),
                        new_kw,
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
                let after = type_param.skip(self.peek_id());
                matches!(
                    self.kind_at(self.offset_of(after) + 1),
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
        let end = self.parse_constructor_invocation_arguments();
        self.listener.end_new_expression(new_kw);
        end
    }

    /// Official `parseConstructorInvocationArguments` :8641: arguments (or
    /// the invalid-type-arguments recovery). The pinned oracle never
    /// surfaces `constructor_with_type_arguments` (#96 cull), so the
    /// type-argument arm emits only `handle_invalid_type_arguments` and the
    /// generic `(`-missing recovery.
    pub(crate) fn parse_constructor_invocation_arguments(&mut self) -> TokenSpan {
        if self.peek_kind() == TokenKind::OpenParen {
            return self.parse_arguments_rest();
        }
        let at = self.peek_span();
        let type_arg = self.compute_type_param_or_arg(false);
        let end = if matches!(type_arg, TypeParamOrArg::None) {
            self.report(msg::expected_after_but_got("("), at);
            at
        } else {
            let parsed = type_arg.parse_arguments(self);
            self.listener.handle_invalid_type_arguments(parsed);
            self.peek_span()
        };
        if self.peek_kind() != TokenKind::OpenParen {
            // Official `insertParens(includeIdentifier: false)`: an empty
            // synthetic argument group.
            let offset = self.peek_span().range.offset;
            self.synthetic(TokenKind::OpenParen, offset);
            self.cursor.advance();
            self.synthetic(TokenKind::CloseParen, offset);
            self.cursor.advance();
            return self.peek_span();
        }
        end
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
                return self.parse_function_literal(type_info, type_param);
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
        type_info: TypeInfo,
        type_param: TypeParamOrArg,
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
    fn parse_named_function_expression_rest(&mut self, type_param: TypeParamOrArg) -> TokenSpan {
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
        self.parse_parenthesized_or_record_literal(None)
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

impl<L: ParserListener> Parser<'_, '_, L> {
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
    /// name the unnamed constructor.
    fn parse_send_from_identifier(&mut self, begin: TokenSpan) -> TokenSpan {
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
        if !matches!(type_arg, TypeParamOrArg::None) {
            let _ = type_arg.parse_arguments(self);
        } else {
            let next = self.peek_span();
            self.listener.handle_no_type_arguments(next);
        }
        let end = if self.peek_kind() == TokenKind::OpenParen {
            self.parse_arguments_rest()
        } else {
            self.parse_arguments_opt()
        };
        self.listener.handle_send(begin, end);
        end
    }

    /// Official `parseArgumentsOpt` :9150.
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
            let _ = self.parse_expression();
            if let Some(colon) = colon {
                self.listener.handle_named_argument(colon);
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

impl<L: ParserListener> Parser<'_, '_, L> {
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
            // `id (.id)? typeArguments? '(' ... ')'` (object pattern).
            let mut pos = self.cursor.real_pos();
            if self.kind_at(pos + 1) == TokenKind::Period && is_identifier(self.kind_at(pos + 2)) {
                pos += 2;
            }
            if self.kind_at(pos + 1) == TokenKind::Lt {
                let len = self.gt_group_len(pos + 1)?;
                pos += len;
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
            kind if matches!(
                kind,
                TokenKind::OpenSquare | TokenKind::OpenCurly | TokenKind::OpenParen
            ) =>
            {
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
            self.peek_span()
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
                self.parse_for_loop_parts_mid(await_kw, outcome);
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
    fn parse_switch_expression(&mut self) -> TokenSpan {
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
                        match close_pos.and_then(|limit| {
                            self.find_next_comma_or_semicolon(self.cursor.real_pos(), limit)
                        }) {
                            Some(sep_pos) => {
                                // Official reports the missing comma at the
                                // current token once, whichever separator the
                                // search found (:11890-11897).
                                let at = self.peek_span();
                                self.report(msg::expected_but_got(","), at);
                                self.skip_to_token(self.real_id(sep_pos));
                                self.cursor.advance();
                            }
                            None => {
                                let at = self.peek_span();
                                self.report(msg::expected_but_got("}"), at);
                                if let Some(close) = self.cursor.table_matching_end(begin_switch.id)
                                {
                                    self.skip_to_token(close);
                                }
                                break;
                            }
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
                kind if matches!(
                    kind,
                    TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly
                ) =>
                {
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

impl<L: ParserListener> Parser<'_, '_, L> {
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
                // Recovery (:8360-8366): report and parse as an empty list
                // through the synthetic `[]` token.
                self.report(msg::expected_but_got("["), self.peek_span());
                let offset = self.peek_span().range.offset;
                let index = self.synthetic(TokenKind::Index, offset);
                self.cursor.advance();
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
