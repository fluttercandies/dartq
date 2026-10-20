//! Full Dart 3 pattern grammar (official `parsePattern` family,
//! parser_impl.dart:11582-12382).
//!
//! Ported 1:1 from the reference: precedence-driven outer loop (cast /
//! null-assert / null-check / `&&` / `||`), the primary dispatch (list, map,
//! parenthesized/record, object, variable, wildcard, constant, relational),
//! and the recovery heuristics (`looksLikePatternStart`,
//! `looksLikeLiteralEntry`, no-progress forcing inside element loops).
//!
//! All productions are gated on `patterns_enabled` (feature folding at
//! construction); with the feature off the entry points report
//! `experiment_not_enabled` and consume nothing.

use dartq_scanner::{Keyword, TokenKind};

use crate::expr_const::SELECTOR_PRECEDENCE;
use crate::listener::{ConstantPatternContext, IdentifierContext, ParserListener, PatternContext};
use crate::message::msg;
use crate::parser::Parser;
use crate::recovery::is_identifier;
use crate::token_id::TokenSpan;

/// `_computePrecedence(next, forPattern: true)` (`parser_impl.dart:7773)`:
/// pattern-outer-level operators.
fn pattern_level(kind: TokenKind) -> Option<u8> {
    Some(match kind {
        TokenKind::AmpersandAmpersand => 6,   // LOGICAL_AND
        TokenKind::BarBar => 5,               // LOGICAL_OR
        TokenKind::Keyword(Keyword::As) => 8, // RELATIONAL
        TokenKind::Bang | TokenKind::Question => SELECTOR_PRECEDENCE, // 17
        _ => return None,
    })
}

/// `looksLikePatternStart` (`identifier_context.dart:372`).
pub(crate) fn looks_like_pattern_start(
    parser: &Parser<'_, '_, '_, impl ParserListener>,
    offset: usize,
) -> bool {
    let kind = parser.kind_at(offset);
    match kind {
        TokenKind::Identifier
        | TokenKind::Double
        | TokenKind::Hash
        | TokenKind::Hexadecimal
        | TokenKind::Int
        | TokenKind::String
        | TokenKind::OpenCurly
        | TokenKind::OpenParen
        | TokenKind::OpenSquare
        | TokenKind::Index
        | TokenKind::Lt
        | TokenKind::LtEq
        | TokenKind::Gt
        | TokenKind::GtEq
        | TokenKind::BangEq
        | TokenKind::EqEq => true,
        TokenKind::Keyword(kw) => matches!(
            kw,
            Keyword::Null
                | Keyword::False
                | Keyword::True
                | Keyword::Var
                | Keyword::Final
                | Keyword::Const
        ),
        _ => false,
    }
}

/// `looksLikeLiteralEntry` (`literal_entry_info.dart:79`) reduced to the
/// shapes the list/map pattern recovery loops need.
pub(crate) fn looks_like_literal_entry(
    parser: &Parser<'_, '_, '_, impl ParserListener>,
    offset: usize,
) -> bool {
    looks_like_expression_start(parser, offset)
        || matches!(
            parser.kind_at(offset),
            TokenKind::PeriodPeriodPeriod | TokenKind::PeriodPeriodPeriodQuestion
        )
        || matches!(
            parser.kind_at(offset),
            TokenKind::Keyword(Keyword::If | Keyword::For)
        )
        || (parser.kind_at(offset) == TokenKind::Keyword(Keyword::Await)
            && parser.kind_at(offset + 1) == TokenKind::Keyword(Keyword::For))
}

/// `looksLikeExpressionStart` (`identifier_context.dart:340`) reduced to the
/// token-kind part (statement-start keywords are #19's surface).
pub(crate) fn looks_like_expression_start(
    parser: &Parser<'_, '_, '_, impl ParserListener>,
    offset: usize,
) -> bool {
    let kind = parser.kind_at(offset);
    match kind {
        TokenKind::Identifier
        | TokenKind::Double
        | TokenKind::Hash
        | TokenKind::Hexadecimal
        | TokenKind::Int
        | TokenKind::String
        | TokenKind::OpenCurly
        | TokenKind::OpenParen
        | TokenKind::OpenSquare
        | TokenKind::Index
        | TokenKind::Lt
        | TokenKind::Bang
        | TokenKind::Minus
        | TokenKind::Tilde
        | TokenKind::PlusPlus
        | TokenKind::MinusMinus => true,
        TokenKind::Keyword(kw) => !matches!(
            kw,
            Keyword::Assert
                | Keyword::Break
                | Keyword::Continue
                | Keyword::Do
                | Keyword::Else
                | Keyword::Final
                | Keyword::For
                | Keyword::If
                | Keyword::Return
                | Keyword::Switch
                | Keyword::Try
                | Keyword::Var
                | Keyword::Void
                | Keyword::While
        ),
        _ => false,
    }
}

impl<L: ParserListener> Parser<'_, '_, '_, L> {
    /// Official `parsePattern` (`parser_impl.dart:11582`). The cursor sits on
    /// the pattern's first token. Feature gate: with `patterns` disabled the
    /// construct takes the error path (official `reportExperimentNotEnabled`
    /// semantics) and consumes nothing.
    pub(crate) fn parse_pattern(&mut self, context: PatternContext) -> TokenSpan {
        if !self.features.patterns_enabled {
            let at = self.peek_span();
            if self.admit_diagnostic() {
                self.listener.handle_experiment_not_enabled(
                    crate::listener::ExperimentalFlag::Patterns,
                    at,
                    at,
                );
            }
            return at;
        }
        self.parse_pattern_at(context, 1)
    }

    fn parse_pattern_at(&mut self, context: PatternContext, precedence: u8) -> TokenSpan {
        if !self.enter_depth() {
            return self.peek_span();
        }
        let begin = self.peek_span();
        self.listener.begin_pattern(begin);
        let start = self.peek_span();
        let mut token = self.parse_primary_pattern(context);
        loop {
            let kind = self.peek_kind();
            let Some(level) = pattern_level(kind) else {
                self.listener.end_pattern(token);
                self.leave_depth();
                return token;
            };
            if level < precedence {
                self.listener.end_pattern(token);
                self.leave_depth();
                return token;
            }
            match kind {
                TokenKind::Keyword(Keyword::As) => {
                    // castPattern ::= primaryPattern 'as' type
                    self.check_unary_context(start, token);
                    let operator = self.peek_span();
                    self.cursor.advance();
                    self.listener.begin_as_operator_type(operator);
                    let type_info = self.compute_type_after_is_or_as();
                    let type_end = type_info.ensure_type_not_void(self);
                    self.listener.end_as_operator_type(operator);
                    self.listener.handle_cast_pattern(operator);
                    token = type_end;
                }
                TokenKind::Bang => {
                    // nullAssertPattern ::= primaryPattern '!'
                    self.check_unary_context(start, token);
                    let bang = self.peek_span();
                    self.cursor.advance();
                    self.listener.handle_null_assert_pattern(bang);
                    token = bang;
                }
                TokenKind::Question => {
                    // nullCheckPattern ::= primaryPattern '?'
                    self.check_unary_context(start, token);
                    let question = self.peek_span();
                    self.cursor.advance();
                    self.listener.handle_null_check_pattern(question);
                    token = question;
                }
                TokenKind::AmpersandAmpersand | TokenKind::BarBar => {
                    self.listener.begin_binary_pattern(self.peek_span());
                    let operator = self.peek_span();
                    self.cursor.advance();
                    // Left associative: RHS one level up.
                    token = self.parse_pattern_at(context, level + 1);
                    self.listener.end_binary_pattern(operator);
                }
                _ => {
                    // Some other operator that doesn't belong in a pattern.
                    self.listener.end_pattern(token);
                    self.leave_depth();
                    return token;
                }
            }
            // Nothing handled above is valid inside a unary pattern.
            self.is_last_pattern_allowed_inside_unary_pattern = false;
        }
    }

    /// `isLastPatternAllowedInsideUnaryPattern` guard with the
    /// `invalidInsideUnaryPattern` report (parser_impl.dart:11602-11638).
    fn check_unary_context(&mut self, start: TokenSpan, end: TokenSpan) {
        if !self.is_last_pattern_allowed_inside_unary_pattern {
            self.report_with_end(msg::invalid_inside_unary_pattern(), start, end);
        }
    }

    /// A constant pattern parsed in an irrefutable pattern context (a
    /// pattern variable declaration or a pattern assignment) is refutable
    /// and reports there at the pattern's own extent — from a leading
    /// `const` keyword through the operand (`final (o.f) = v;` anchors at
    /// `o.f`, `final (const c) = c;` at `const c`; the enclosing
    /// parenthesized/list/record container adds no row of its own — oracle
    /// probes, pinned in the semantic crate's ticket_2231 suite). Matching
    /// contexts (switch cases, if-case) are refutable and stay silent.
    fn report_refutable_constant_pattern(
        &mut self,
        context: PatternContext,
        begin: TokenSpan,
        end: TokenSpan,
    ) {
        if context.is_refutable() {
            return;
        }
        // The row spans the whole pattern, not just its first token. Never
        // below the begin token's own length, so a recovery end can yield
        // no zero-width row.
        let length = end
            .range
            .end()
            .saturating_sub(begin.range.offset)
            .max(begin.range.length);
        let full = TokenSpan::new(
            begin.id,
            dartq_source::SourceRange {
                offset: begin.range.offset,
                length,
            },
        );
        self.report_with_end(msg::refutable_pattern_in_irrefutable_context(), full, end);
    }

    /// Official `parsePrimaryPattern` (`parser_impl.dart:11691`).
    #[allow(clippy::too_many_lines)] // mirrors the official branch structure
    fn parse_primary_pattern(&mut self, context: PatternContext) -> TokenSpan {
        // Optional `<...>` type arguments directly ahead of `[`/`{`. The
        // follower check mirrors the official `typeArg.skip(token).next!`:
        // a combined closer (`>>`/`>=`) splits on consume, so the follower
        // is the split remainder, not the token after the combined lexeme.
        let type_param = if self.peek_kind() == TokenKind::Lt {
            self.compute_type_param_or_arg_on_lt(true)
        } else {
            crate::types::TypeParamOrArg::None
        };
        let follower = if matches!(type_param, crate::types::TypeParamOrArg::None) {
            self.peek_kind()
        } else {
            self.type_arg_follower(&type_param, self.cursor.real_pos())
        };
        match follower {
            TokenKind::OpenSquare | TokenKind::Index => {
                type_param.parse_arguments(self);
                let end = self.parse_list_pattern_suffix(context);
                self.is_last_pattern_allowed_inside_unary_pattern = true;
                return end;
            }
            TokenKind::OpenCurly => {
                type_param.parse_arguments(self);
                let end = self.parse_map_pattern_suffix(context);
                self.is_last_pattern_allowed_inside_unary_pattern = true;
                return end;
            }
            _ => {}
        }

        match self.peek_kind() {
            TokenKind::Keyword(Keyword::Var | Keyword::Final) => {
                self.is_last_pattern_allowed_inside_unary_pattern = true;
                return self.parse_variable_pattern(context, None);
            }
            TokenKind::OpenParen => {
                // `(int, int) x` record-type variable pattern vs record pattern.
                if self.record_type_variable_ahead() {
                    let type_info = self.compute_variable_pattern_type(true);
                    if let crate::types::TypeInfo::Complex(complex) = &type_info
                        && complex.is_record_type
                        && !complex.recovered
                    {
                        self.is_last_pattern_allowed_inside_unary_pattern = true;
                        return self.parse_variable_pattern(context, Some(type_info));
                    }
                }
                if self.cursor.look_kind(1) == TokenKind::CloseParen {
                    // `()`: empty record pattern.
                    let left = self.peek_span();
                    self.cursor.advance();
                    let close = self.peek_span();
                    self.cursor.advance();
                    self.listener.handle_record_pattern(left, 0);
                    self.is_last_pattern_allowed_inside_unary_pattern = true;
                    return close;
                }
                let end = self.parse_parenthesized_or_record_pattern(context);
                self.is_last_pattern_allowed_inside_unary_pattern = true;
                return end;
            }
            TokenKind::Keyword(Keyword::Const) => {
                // Constant pattern with an explicit `const` prefix: the
                // operand after `const` goes through the expression engine in
                // Explicit context (official :11791-11803).
                let const_keyword = self.peek_span();
                self.listener.begin_constant_pattern(Some(const_keyword));
                self.cursor.advance();
                let end = self.parse_constant_expression(ConstantPatternContext::Explicit);
                self.listener.end_constant_pattern(Some(const_keyword));
                self.report_refutable_constant_pattern(context, const_keyword, end);
                self.is_last_pattern_allowed_inside_unary_pattern = true;
                return end;
            }
            kind if is_relational_or_equality(kind) => {
                // Relational pattern: `< expr`, `== expr`, ... — the operand
                // parses from BITWISE_OR precedence with no constant context
                // (official :11810-11815).
                let operator = self.peek_span();
                self.cursor.advance();
                let end = self.parse_precedence_floor(crate::expr_const::BITWISE_OR_PRECEDENCE);
                self.listener.handle_relational_pattern(operator);
                self.is_last_pattern_allowed_inside_unary_pattern = false;
                return end;
            }
            _ => {}
        }

        // Typed variable pattern?
        let type_info = self.compute_variable_pattern_type(false);
        if !matches!(type_info, crate::types::TypeInfo::NoType) {
            self.is_last_pattern_allowed_inside_unary_pattern = true;
            return self.parse_variable_pattern(context, Some(type_info));
        }

        // objectPattern ::= typeName typeArguments? '(' patternFields? ')'
        if is_identifier(self.peek_kind()) {
            let probe = self.cursor.mark();
            let first_identifier = self.peek_span();
            self.cursor.advance();
            let mut dot = None;
            let mut second_identifier = None;
            if self.peek_kind() == TokenKind::Period {
                let period = self.peek_span();
                self.cursor.advance();
                dot = Some(period);
                if is_identifier(self.peek_kind()) {
                    second_identifier = Some(self.peek_span());
                    self.cursor.advance();
                } else {
                    second_identifier =
                        Some(self.ensure_identifier(IdentifierContext::ExpressionContinuation));
                }
            }
            let type_param = self.compute_type_param_or_arg_on_lt(false);
            // Follower check mirrors the official
            // `potentialTypeArg.skip(token).next!` (:11833-11835): the `(`
            // is looked up AFTER the type-argument group, not at the cursor
            // (which still sits on `<`).
            let follower = self.type_arg_follower(&type_param, self.cursor.real_pos());
            if follower == TokenKind::OpenParen && !type_param.recovered() {
                type_param.parse_arguments(self);
                let end = self.parse_object_pattern_rest(context);
                self.listener
                    .handle_object_pattern(first_identifier, dot, second_identifier);
                self.is_last_pattern_allowed_inside_unary_pattern = true;
                return end;
            }
            if dot.is_none() {
                // Single identifier: wildcard / irrefutable-context variable.
                // Borrow the source-tied lexeme (`TokenCursor::lexeme` hands
                // out `&'s str`) — the checks below need no owned copy.
                let name = self.cursor.lexeme(first_identifier.id);
                if !context.is_refutable() || name == "_" {
                    self.is_last_pattern_allowed_inside_unary_pattern = true;
                    self.cursor.reset(probe);
                    return self.parse_variable_pattern(context, None);
                }
                if matches!(name, "when" | "as") {
                    self.report(msg::illegal_pattern_identifier_name(name), first_identifier);
                }
            }
            // Constant pattern: an identifier (possibly qualified). Re-parse
            // from the identifier (official resets to beforeFirstIdentifier).
            self.cursor.reset(probe);
            self.listener.begin_constant_pattern(None);
            let end = self.parse_constant_expression(ConstantPatternContext::Implicit);
            self.listener.end_constant_pattern(None);
            self.report_refutable_constant_pattern(context, first_identifier, end);
            self.is_last_pattern_allowed_inside_unary_pattern = true;
            return end;
        }

        // Constant pattern (implicit const). Reserved words land here too:
        // official parsePrimaryPattern has no reserved-keyword arm —
        // `true`/`false`/`null` parse as boolean/null literals through the
        // expression engine (:11880-11897) and stay silent; any other
        // reserved word takes parse_send's ensure-identifier recovery.
        self.listener.begin_constant_pattern(None);
        let constant_begin = self.peek_span();
        let end = self.parse_constant_expression(ConstantPatternContext::Implicit);
        self.listener.end_constant_pattern(None);
        self.report_refutable_constant_pattern(context, constant_begin, end);
        self.is_last_pattern_allowed_inside_unary_pattern = true;
        end
    }

    /// `(...)` followed by an identifier (or `? identifier`): a record-type
    /// variable pattern candidate (parser_impl.dart:11735-11757).
    fn record_type_variable_ahead(&self) -> bool {
        if self.peek_kind() != TokenKind::OpenParen {
            return false;
        }
        let Some(close) = self.cursor.table_matching_end(self.peek_id()) else {
            return false;
        };
        let after = self.kind_at(self.offset_of(close) + 1);
        is_identifier(after)
            || (after == TokenKind::Question
                && is_identifier(self.kind_at(self.offset_of(close) + 2)))
    }

    /// Official `parseVariablePattern` (`parser_impl.dart:11902`).
    fn parse_variable_pattern(
        &mut self,
        context: PatternContext,
        type_info: Option<crate::types::TypeInfo>,
    ) -> TokenSpan {
        let mut is_bare_identifier = false;
        let mut keyword: Option<TokenSpan> = None;
        let mut has_type = false;
        match type_info {
            Some(info) => {
                info.parse_type(self);
                has_type = true;
            }
            None => {
                if matches!(
                    self.peek_kind(),
                    TokenKind::Keyword(Keyword::Var | Keyword::Final)
                ) {
                    let kw = self.peek_span();
                    keyword = Some(kw);
                    self.cursor.advance();
                    let next_is_paren = self.peek_kind() == TokenKind::OpenParen;
                    let info = self.compute_variable_pattern_type(next_is_paren);
                    has_type = !matches!(info, crate::types::TypeInfo::NoType);
                    info.parse_type(self);
                } else {
                    is_bare_identifier = true;
                }
            }
        }

        let name = if is_identifier(self.peek_kind()) {
            let span = self.peek_span();
            self.cursor.advance();
            span
        } else {
            self.ensure_identifier(IdentifierContext::LocalVariableDeclaration)
        };
        let variable_name = self.cursor.lexeme(name.id);

        match context {
            PatternContext::Declaration => {
                if let Some(kw) = keyword {
                    self.report(msg::variable_pattern_keyword_in_declaration_context(), kw);
                }
            }
            PatternContext::Matching => {
                // Redundant `var` before a type (`var T x`).
                if has_type
                    && matches!(keyword, Some(kw) if self.cursor.resolve(kw.id).0 == TokenKind::Keyword(Keyword::Var))
                {
                    self.report(msg::var_and_type(), keyword.unwrap());
                }
            }
            PatternContext::Assignment => {
                if !is_bare_identifier {
                    let display = if variable_name.is_empty() {
                        "(unnamed)"
                    } else {
                        variable_name
                    };
                    self.report(msg::pattern_assignment_declares_variable(display), name);
                }
            }
        }

        let in_assignment_pattern = context == PatternContext::Assignment;
        if variable_name == "_" {
            if is_bare_identifier {
                self.listener.handle_no_type(name);
            }
            self.listener.handle_wildcard_pattern(keyword, name);
        } else if in_assignment_pattern && is_bare_identifier {
            if matches!(variable_name, "when" | "as") {
                self.report(
                    msg::illegal_pattern_assignment_variable_name(variable_name),
                    name,
                );
            }
            self.listener.handle_assigned_variable_pattern(name);
        } else {
            if matches!(variable_name, "when" | "as") {
                self.report(msg::illegal_pattern_variable_name(variable_name), name);
            }
            if is_bare_identifier {
                self.listener.handle_no_type(name);
            }
            self.listener
                .handle_declared_variable_pattern(keyword, name, in_assignment_pattern);
        }
        name
    }

    /// Official `parseListPatternSuffix` (`parser_impl.dart:12004`). Cursor at
    /// `[` (or before an `[]` INDEX token).
    fn parse_list_pattern_suffix(&mut self, context: PatternContext) -> TokenSpan {
        let begin = self.peek_span();
        if self.peek_kind() == TokenKind::Index {
            // `[]` combined token: official `rewriteSquareBrackets` splits it
            // into `[` + `]` and the suffix returns `token.next!` — the token
            // AFTER the `]`. Both synthetics are drained so the cursor lands
            // there (the caller reads the real `=>`/`:` follower).
            self.cursor.advance(); // INDEX
            let open = self.synthetic(TokenKind::OpenSquare, begin.range.offset);
            let close = self.synthetic(TokenKind::CloseSquare, begin.range.offset + 1);
            self.listener.handle_list_pattern(0, open, close);
            self.cursor.advance(); // synthetic `[`
            self.cursor.advance(); // synthetic `]`
            return close;
        }
        self.cursor.advance(); // `[`
        let mut count = 0usize;
        let end;
        loop {
            if self.is_bailed() {
                // Terminal bailout: unwind without consumption (see
                // `parse_list_literal`); the per-element no-progress guard
                // alone cannot stop the missing-comma arm from spinning on
                // queued synthetics.
                end = self.peek_span();
                break;
            }
            let kind = self.peek_kind();
            if kind == TokenKind::CloseSquare {
                end = self.peek_span();
                self.cursor.advance();
                break;
            }
            if kind == TokenKind::PeriodPeriodPeriod {
                let dots = self.peek_span();
                self.cursor.advance();
                let has_sub_pattern = looks_like_pattern_start(self, self.cursor.real_pos());
                if has_sub_pattern {
                    self.parse_pattern(context);
                }
                self.listener.handle_rest_pattern(dots, has_sub_pattern);
            } else {
                let before = self.cursor.progress();
                self.parse_pattern(context);
                if self.no_progress(before) {
                    // Force progress past an unconsumable token.
                    self.force_advance();
                }
            }
            count += 1;
            match self.peek_kind() {
                TokenKind::Comma => {
                    self.cursor.advance();
                }
                TokenKind::CloseSquare => {
                    end = self.peek_span();
                    self.cursor.advance();
                    break;
                }
                _ => {
                    // Recovery: jump to the scanner-paired `]`.
                    if looks_like_literal_entry(self, self.cursor.real_pos()) {
                        self.report(msg::expected_but_got(","), self.peek_span());
                        self.synthetic(TokenKind::Comma, self.peek_span().range.offset);
                        // Official list-suffix recovery
                        // (`rewriteAndRecover` + `token = next`): step onto
                        // the inserted comma so the next element parses the
                        // real token.
                        self.cursor.advance();
                        continue;
                    }
                    end = self.group_end_or_here(begin, "]");
                    break;
                }
            }
        }
        self.listener.handle_list_pattern(count, begin, end);
        end
    }

    /// Official `parseMapPatternSuffix` (`parser_impl.dart:12087`). Cursor at
    /// `{`.
    ///
    /// The entry's no-progress skip mirrors the official
    /// `identical(next, token.next)` check: `next` is the loop's stale
    /// follower pointer (refreshed at the loop bottom, initialized to the
    /// first key), not the current pattern head. So the FIRST entry's
    /// unconsumed blocker survives to the `,`/`}` recovery (which inserts a
    /// comma and re-parses it as the next key), while later entries — whose
    /// stale `next` was refreshed onto the blocker — skip it.
    fn parse_map_pattern_suffix(&mut self, context: PatternContext) -> TokenSpan {
        let left = self.peek_span();
        self.cursor.advance();
        let mut count = 0usize;
        let end;
        let mut stale_next = self.peek_id();
        loop {
            if self.is_bailed() {
                // Terminal bailout: unwind without consumption (see
                // `parse_list_literal`).
                end = self.peek_span();
                break;
            }
            let kind = self.peek_kind();
            if kind == TokenKind::CloseCurly {
                end = self.peek_span();
                self.cursor.advance();
                break;
            }
            if kind == TokenKind::PeriodPeriodPeriod {
                let dots = self.peek_span();
                self.cursor.advance();
                let has_sub_pattern = looks_like_pattern_start(self, self.cursor.real_pos());
                if has_sub_pattern {
                    self.parse_pattern(context);
                }
                self.listener.handle_rest_pattern(dots, has_sub_pattern);
            } else {
                // entry: expression `:` pattern
                let key_before = self.cursor.progress();
                self.listener.begin_map_pattern_key();
                self.parse_constant_expression(ConstantPatternContext::None);
                // #2024 separator anchor: when the key IS the unrelocated
                // blocker (the `,` recovery re-parses it as the next key),
                // the `:` row anchors on the token the recovery lands
                // behind — the blocker's follower — not on the blocker
                // (frozen oracle 0c9fda4b: the separator recovery row rides
                // the token following the recovery point).
                let colon_anchor = if self.no_progress(key_before) {
                    self.look_span_at(1)
                } else {
                    self.peek_span()
                };
                let colon = self.expect_lexeme_but_got_anchored(":", colon_anchor);
                let value_head = self.peek_id();
                let before = self.cursor.progress();
                self.parse_pattern(context);
                if self.no_progress(before) && value_head == stale_next {
                    self.force_advance();
                }
                self.listener
                    .handle_map_pattern_entry(colon, self.peek_span());
            }
            count += 1;
            stale_next = self.peek_id();
            match self.peek_kind() {
                TokenKind::Comma => {
                    self.cursor.advance();
                }
                TokenKind::CloseCurly => {
                    end = self.peek_span();
                    self.cursor.advance();
                    break;
                }
                _ => {
                    if looks_like_literal_entry(self, self.cursor.real_pos()) {
                        self.report(msg::expected_but_got(","), self.peek_span());
                        self.synthetic(TokenKind::Comma, self.peek_span().range.offset);
                        // Official recovery shape: step onto the inserted
                        // comma (see the list-pattern arm).
                        self.cursor.advance();
                        continue;
                    }
                    end = self.group_end_or_here(left, "}");
                    break;
                }
            }
        }
        self.listener.handle_map_pattern(count, left, end);
        end
    }

    /// Official `parseParenthesizedPatternOrRecordPattern` (:12176). Cursor
    /// at `(` (empty `()` handled by the caller).
    fn parse_parenthesized_or_record_pattern(&mut self, context: PatternContext) -> TokenSpan {
        let begin = self.peek_span();
        self.cursor.advance();
        let mut count = 0usize;
        let mut was_record = false;
        let mut was_valid_record = false;
        let end;
        loop {
            let kind = self.peek_kind();
            if (count > 0 || was_record) && kind == TokenKind::CloseParen {
                end = self.peek_span();
                self.cursor.advance();
                break;
            }
            if kind == TokenKind::Eof {
                end = self.peek_span();
                break;
            }
            let mut colon: Option<TokenSpan> = None;
            if kind == TokenKind::Colon {
                was_record = true;
                was_valid_record = true;
                // Official `handleNoName(token)` (:12198): the payload is the
                // token BEFORE the colon, not the colon itself — the cursor's
                // last consumed token (queue-exact; `peek_id() - 1` breaks
                // once recovery synthetics drain ahead of the colon).
                let before = self.span_of(
                    self.cursor
                        .last_consumed_id()
                        .expect("record-pattern fields parse after the `(` is consumed"),
                );
                self.listener.handle_no_name(before);
                colon = Some(self.peek_span());
                self.cursor.advance();
            } else if kind != TokenKind::OpenParen && self.cursor.look_kind(1) == TokenKind::Colon {
                was_record = true;
                let name = self.ensure_identifier(IdentifierContext::NamedRecordFieldReference);
                colon = Some(name);
                // Consume the colon: the sub-pattern parses after it
                // (official `ensureIdentifier(...).next!`, :12205-12213).
                self.cursor.advance();
                was_valid_record = true;
            }
            // No no-progress skip here (official :12229-12231 has none): a
            // sub-pattern that consumes nothing (a keyword head) is left in
            // place so the `ensureCloseParen` below reports `expectedButGot
            // (')')` at it and jumps to the group's real closer.
            self.parse_pattern(context);
            if was_record || colon.is_some() {
                self.listener.handle_pattern_field(colon);
            }
            count += 1;
            if self.peek_kind() == TokenKind::Comma {
                if !was_record && colon.is_none() {
                    self.listener.handle_pattern_field(None);
                }
                was_record = true;
                was_valid_record = true;
                self.cursor.advance();
            } else {
                let close = self.ensure_close_paren(begin);
                end = close;
                break;
            }
        }
        if was_record {
            if count == 1 && !was_valid_record {
                self.report(msg::record_literal_one_positional_no_trailing_comma(), end);
            }
            self.listener.handle_record_pattern(begin, count);
        } else {
            self.listener.handle_parenthesized_pattern(begin);
        }
        end
    }

    /// Official `parseObjectPatternRest` (:12256). Cursor at `(`.
    fn parse_object_pattern_rest(&mut self, context: PatternContext) -> TokenSpan {
        let begin = self.peek_span();
        self.cursor.advance();
        let mut argument_count = 0usize;
        let end;
        loop {
            if self.is_bailed() {
                // Terminal bailout: unwind without consumption (see
                // `parse_list_literal`).
                end = self.peek_span();
                break;
            }
            let kind = self.peek_kind();
            if kind == TokenKind::CloseParen {
                end = self.peek_span();
                self.cursor.advance();
                break;
            }
            if kind == TokenKind::Eof {
                end = self.peek_span();
                break;
            }
            let mut colon: Option<TokenSpan> = None;
            if kind == TokenKind::Colon {
                // Official `handleNoName(token)` (:12270): the token BEFORE
                // the colon — the cursor's last consumed token (queue-exact,
                // see the record-pattern arm above).
                let before = self.span_of(
                    self.cursor
                        .last_consumed_id()
                        .expect("map-pattern entries parse after the `{` is consumed"),
                );
                self.listener.handle_no_name(before);
                colon = Some(self.peek_span());
                self.cursor.advance();
            } else if self.cursor.look_kind(1) == TokenKind::Colon {
                let name = self.ensure_identifier(IdentifierContext::NamedArgumentReference);
                colon = Some(name);
                self.cursor.advance();
            }
            let before = self.cursor.progress();
            self.parse_pattern(context);
            if self.no_progress(before) {
                self.force_advance();
            }
            self.listener.handle_pattern_field(colon);
            argument_count += 1;
            match self.peek_kind() {
                TokenKind::Comma => {
                    self.cursor.advance();
                }
                TokenKind::CloseParen => {
                    end = self.peek_span();
                    self.cursor.advance();
                    break;
                }
                _ => {
                    if looks_like_expression_start(self, self.cursor.real_pos()) {
                        self.report(msg::expected_but_got(","), self.peek_span());
                        self.synthetic(TokenKind::Comma, self.peek_span().range.offset);
                        // Official recovery shape: step onto the inserted
                        // comma (see the list-pattern arm).
                        self.cursor.advance();
                        continue;
                    }
                    let close = self.ensure_close_paren(begin);
                    end = close;
                    break;
                }
            }
        }
        self.listener
            .handle_object_pattern_fields(argument_count, begin, end);
        end
    }

    /// Jump to the scanner-paired group end for recovery, or stay. The jump
    /// consumes the closer where it sits — a scanner-synthesized closer keeps
    /// its parked position, so the parse-end unmatched pass reports its row
    /// there (official `moveSynthetic` moves only `ensureCloseParen`-style
    /// adoptions, `parser_impl.dart:12051-12064`); a real but distant closer
    /// reports `expectedButGot` here.
    fn group_end_or_here(&mut self, begin: TokenSpan, expected: &str) -> TokenSpan {
        if let Some(end) = self.cursor.table_matching_end(begin.id) {
            let (_, synthetic) = self.cursor.resolve(end);
            if !synthetic {
                self.report(msg::expected_but_got(expected), self.peek_span());
            }
            self.skip_to_token(end);
            let span = self.peek_span();
            self.cursor.advance();
            span
        } else {
            self.report(msg::expected_but_got(expected), self.peek_span());
            self.peek_span()
        }
    }
}

fn is_relational_or_equality(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Lt
            | TokenKind::Gt
            | TokenKind::LtEq
            | TokenKind::GtEq
            | TokenKind::EqEq
            | TokenKind::BangEq
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr_const::test_support::{Log, parser};
    use crate::listener::Token;
    use crate::message::Message;

    #[test]
    fn record_pattern_named_field_advances_past_colon() {
        // F1: `(x: 1)` parses the sub-pattern after the colon
        // (official :12205-12213), with no recovery noise.
        let mut p = parser("(x: 1)", Log::default());
        let _ = p.parse_pattern_entry(PatternContext::Matching);
        p.listener.assert_events(&[("handle", "record_pattern")]);
        assert!(p.listener.errors_empty());
    }

    #[test]
    fn list_pattern_missing_comma_splits_elements() {
        // F10 (official :8549-8556 / pattern analog): `[1 2]` becomes two
        // elements with exactly one expected-`,` report.
        let mut p = parser("[1 2]", Log::default());
        let _ = p.parse_pattern_entry(PatternContext::Matching);
        p.listener.assert_events(&[("handle", "list_pattern")]);
        assert_eq!(p.listener.error_codes(), ["expected_token"]);
    }

    #[test]
    fn typed_list_pattern_smoke() {
        // F13: the leading `<...>` probe runs in declaration mode
        // (official :11066-11068, inDeclaration = true).
        for source in ["<int>[1, 2]", "<List<int>>[1]"] {
            let mut p = parser(source, Log::default());
            let _ = p.parse_pattern_entry(PatternContext::Matching);
            p.listener.assert_events(&[("handle", "list_pattern")]);
            assert!(p.listener.errors_empty(), "{source}");
        }
    }

    /// Local listener capturing `handleNoName` payloads (span offset/length).
    #[derive(Default)]
    struct NoNameLog {
        errors: Vec<String>,
        no_names: Vec<(u32, u32)>,
    }

    impl crate::listener::ParserListener for NoNameLog {
        fn handle_recoverable_error(&mut self, message: Message, _: TokenSpan, _: TokenSpan) {
            self.errors.push(message.code.name.to_owned());
        }

        fn handle_no_name(&mut self, token: Token) {
            self.no_names.push((token.range.offset, token.range.length));
        }
    }

    #[test]
    fn no_name_payload_is_the_token_before_the_colon() {
        // F14 (official :12198 record / :12270 object): `handleNoName`
        // receives the token before the colon — `(` at offset 0 here.
        let mut p = parser("(: 1, 2)", NoNameLog::default());
        let _ = p.parse_pattern_entry(PatternContext::Matching);
        assert_eq!(p.listener.no_names, vec![(0, 1)]);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);

        let mut p = parser("Foo(: 1)", NoNameLog::default());
        let _ = p.parse_pattern_entry(PatternContext::Matching);
        // The token before the `:` is the object pattern's `(` at offset 3.
        assert_eq!(p.listener.no_names, vec![(3, 1)]);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
    }
}
