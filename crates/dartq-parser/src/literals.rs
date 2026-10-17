//! Literal collection/string/symbol parsing (official
//! `parseLiteralListSuffix` :8520 / `parseLiteralSetOrMapSuffix` :7893 /
//! `parseParenthesizedExpressionOrRecordLiteral` :7628 / string
//! interpolation :8732 / symbols :9244), plus the collection control-flow
//! element machinery (`literal_entry_info.dart` +
//! `literal_entry_info_impl.dart`: if/else, for/for-in, spread, null-aware
//! elements). Elements parse full expressions through the #20 engine
//! (official parity: the collection suffixes call `parseExpression`).

use dartq_scanner::{Keyword, TokenKind};

use crate::expr_const::is_user_definable_operator;
use crate::listener::{IdentifierContext, ParserListener};
use crate::message::msg;
use crate::parser::Parser;
use crate::token_id::TokenSpan;

/// A pending collection control-flow completer (the official
/// `ForComplete`/`IfComplete`/`IfElseComplete` chain, linearized).
enum Pending {
    If,
    IfElse,
    For { for_in: bool },
}

/// Result of unwinding pending completers: done (the entry ended), or an
/// `else` was consumed and the machine continues with the else body.
enum Unwind {
    Done(TokenSpan),
    Else,
}

impl<L: ParserListener> Parser<'_, '_, '_, L> {
    /// One collection element: a plain entry, or an if/for/spread/null-aware
    /// control-flow shape and its body chain
    /// (`literal_entry_info_impl.dart`, `computeLiteralEntry` :61 +
    /// the per-state `parse`/`computeNext` steps). Returns the last consumed
    /// span and the accumulated `ifConditionDelta` (for the
    /// `expected_else_or_comma` recovery message).
    ///
    /// Each pending machine frame is one active nesting unit and obeys the
    /// shared `MAX_DEPTH` gate (`crate::parser::MAX_DEPTH`) — the
    /// linearized counterpart of the official
    /// `expressionDepth` counter (`parseExpression`, `parser_impl.dart:6936`):
    /// past the cap the bailout latches with a single `stackOverflow` report
    /// and the enclosing loops unwind on the bail checks.
    fn parse_collection_entry(
        &mut self,
        set_map: bool,
        has_set_entry: &mut Option<bool>,
    ) -> (TokenSpan, i32) {
        let mut pending: Vec<Pending> = Vec::new();
        let mut if_count = 0i32;
        'outer: loop {
            // `?.` at an entry start splits into `?` + `.` (official
            // `_splitFollowingQuestionPeriod` :8237 at the entry
            // boundaries): the null-aware element reading wins over a
            // dot-shorthand start.
            if self.peek_kind() == TokenKind::QuestionPeriod {
                self.split_question_period();
            }
            match self.peek_kind() {
                TokenKind::Keyword(Keyword::If) => {
                    if !self.enter_depth() {
                        return (self.peek_span(), if_count);
                    }
                    if_count += 1;
                    pending.push(Pending::If);
                    let if_kw = self.peek_span();
                    self.cursor.advance();
                    self.listener.begin_if_control_flow(if_kw);
                    self.ensure_parenthesized_condition(self.features.patterns_enabled);
                    self.listener.handle_then_control_flow(self.peek_span());
                    continue 'outer;
                }
                kind if matches!(kind, TokenKind::Keyword(Keyword::For))
                    || (kind == TokenKind::Keyword(Keyword::Await)
                        && self.cursor.look_kind(1) == TokenKind::Keyword(Keyword::For)) =>
                {
                    if !self.enter_depth() {
                        return (self.peek_span(), if_count);
                    }
                    let await_kw = if kind == TokenKind::Keyword(Keyword::Await) {
                        let a = self.peek_span();
                        self.cursor.advance();
                        Some(a)
                    } else {
                        None
                    };
                    let for_kw = self.peek_span();
                    self.cursor.advance();
                    let for_in = self.parse_for_control_flow_parts(await_kw, for_kw);
                    pending.push(Pending::For { for_in });
                    continue 'outer;
                }
                TokenKind::PeriodPeriodPeriod | TokenKind::PeriodPeriodPeriodQuestion => {
                    // `SpreadOperator.parse` :314-328.
                    let operator = self.peek_span();
                    self.cursor.advance();
                    let _ = self.parse_expression();
                    self.listener.handle_spread_expression(operator);
                }
                TokenKind::Question => {
                    // `NullAwareEntry` :350-352 with the set/map key
                    // disambiguation (:7980-8012).
                    let marker = self.peek_span();
                    self.cursor.advance();
                    let _ = self.parse_expression();
                    if set_map && self.peek_kind() == TokenKind::Colon {
                        let (colon, value_marker) = self.parse_map_entry_rest();
                        self.listener.handle_literal_map_entry(
                            colon,
                            self.peek_span(),
                            Some(marker),
                            value_marker,
                        );
                    } else {
                        self.listener.handle_null_aware_element(marker);
                    }
                }
                _ => {
                    // Simple entry (official `simpleEntry`): expression,
                    // then the map-entry split in set/map mode (:7921-7965).
                    let _ = self.parse_expression();
                    if set_map {
                        if self.peek_kind() == TokenKind::Colon {
                            // `hasSetEntry ??= !isMapEntry` (:8593-8598) is
                            // touched only for a top-level simple entry
                            // (`info == simpleEntry`); a control-flow body
                            // rides the machine branch (:8606-8662), which
                            // never touches it. `pending` is empty exactly
                            // at the top level (else continuations re-enter
                            // with `Pending::IfElse` pushed).
                            if pending.is_empty() {
                                has_set_entry.get_or_insert(false);
                            }
                            let (colon, value_marker) = self.parse_map_entry_rest();
                            self.listener.handle_literal_map_entry(
                                colon,
                                self.peek_span(),
                                None,
                                value_marker,
                            );
                        } else if pending.is_empty() {
                            has_set_entry.get_or_insert(true);
                        }
                    }
                }
            }
            // A body leaf finished: unwind the pending completers. The `Else`
            // case is the loop body's tail — falling through continues the
            // outer loop exactly as the explicit `continue` did.
            match self.unwind_pending(&mut pending) {
                Unwind::Done(end) => return (end, if_count),
                Unwind::Else => {}
            }
        }
    }

    /// The `: value` tail of a map entry, with the null-aware value marker
    /// (`?v`, or `?.v` split into a marker + dot-shorthand value,
    /// :7950-7956).
    fn parse_map_entry_rest(&mut self) -> (TokenSpan, Option<TokenSpan>) {
        let colon = self.peek_span();
        self.cursor.advance();
        let value_marker = if self.peek_kind() == TokenKind::QuestionPeriod {
            self.split_question_period();
            let marker = self.peek_span();
            self.cursor.advance();
            Some(marker)
        } else {
            self.cursor
                .opt_kind(TokenKind::Question)
                .map(|id| self.span_of(id))
        };
        let _ = self.parse_expression();
        (colon, value_marker)
    }

    /// The pending-completer unwind (`ForComplete` :164-182,
    /// `IfComplete` :237-252, `IfElse` :255-283, `IfElseComplete`
    /// :303-311). An `If` completer stays open across an `else`, handing
    /// control back to the element step for the else body.
    fn unwind_pending(&mut self, pending: &mut Vec<Pending>) -> Unwind {
        while let Some(frame) = pending.pop() {
            // Every frame pushed under the depth gate leaves it here; the
            // `else` continuation re-enters below.
            self.leave_depth();
            match frame {
                Pending::For { for_in } => {
                    if for_in {
                        self.listener.end_for_in_control_flow(self.peek_span());
                    } else {
                        self.listener.end_for_control_flow(self.peek_span());
                    }
                }
                Pending::If => {
                    if self.peek_kind() == TokenKind::Keyword(Keyword::Else) {
                        // `IfComplete.computeNext` → `IfElse` :248-251.
                        let else_kw = self.peek_span();
                        self.cursor.advance();
                        self.listener.handle_else_control_flow(else_kw);
                        if !self.enter_depth() {
                            return Unwind::Done(self.peek_span());
                        }
                        pending.push(Pending::IfElse);
                        return Unwind::Else;
                    }
                    self.listener.end_if_control_flow(self.peek_span());
                }
                Pending::IfElse => {
                    self.listener.end_if_else_control_flow(self.peek_span());
                }
            }
        }
        Unwind::Done(self.peek_span())
    }

    /// The `[]`-token arm of `parseLiteralListSuffix` (:8541-8549): an
    /// empty list literal parsed from the combined token.
    pub(crate) fn parse_list_literal_index_arm(
        &mut self,
        const_keyword: Option<TokenSpan>,
    ) -> TokenSpan {
        let offset = self.peek_span().range.offset;
        self.cursor.advance();
        let open = self.synthetic(TokenKind::OpenSquare, offset);
        let close = self.synthetic(TokenKind::CloseSquare, offset);
        self.cursor.advance();
        self.cursor.advance();
        self.listener
            .handle_literal_list(0, open, const_keyword, close);
        close
    }

    /// This method parses the portion of a list literal starting with the
    /// left square bracket (official `parseLiteralListSuffix` :8520). The
    /// cursor sits on `[` (or the combined `[]`).
    pub(crate) fn parse_list_literal(&mut self, const_keyword: Option<TokenSpan>) -> TokenSpan {
        let left = self.peek_span();
        let was_index = self.peek_kind() == TokenKind::Index;
        self.cursor.advance();
        if was_index {
            // `[]` arm (:8541-8549): an empty list literal.
            return self.finish_list_literal_index(left, const_keyword);
        }
        let saved = self.may_parse_function_expressions;
        self.may_parse_function_expressions = true;
        let mut count = 0usize;
        let end;
        loop {
            if self.is_bailed() {
                // The terminal bailout stops all consumption (the official
                // stack-overflow recovery syncs forward instead); the
                // enclosing frames unwind without touching the stream.
                end = self.peek_span();
                break;
            }
            if self.peek_kind() == TokenKind::CloseSquare {
                end = self.peek_span();
                self.cursor.advance();
                break;
            }
            if self.peek_kind() == TokenKind::Eof {
                end = self.peek_span();
                break;
            }
            let (_, if_count) = self.parse_collection_entry(false, &mut None);
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
                TokenKind::Eof => {
                    end = self.peek_span();
                    break;
                }
                _ => {
                    // Latched depth fuse: the descent is spent and no entry
                    // parsing may re-enter (a fresh descent would trip the
                    // recovery cycle again). The official unwind consumes the
                    // remaining group through its closers silently, so jump
                    // to this literal's closer and close the level.
                    if self.depth_fuse_latched {
                        if let Some(close) = self.current_matching_end_for_open(left) {
                            self.skip_to_token(close);
                            end = self.peek_span();
                            self.cursor.advance();
                        } else {
                            let at = self.peek_span();
                            end = self.synthetic(TokenKind::CloseSquare, at.range.offset);
                        }
                        break;
                    }
                    // Recovery (:8569-8595): an entry-looking token means a
                    // missing comma (`expected_else_or_comma` after an `if`
                    // condition); otherwise jump to the group's closer,
                    // silently when the scanner synthesized it.
                    if crate::patterns::looks_like_literal_entry(self, self.cursor.real_pos()) {
                        let message = if if_count > 0 {
                            msg::expected_else_or_comma()
                        } else {
                            msg::expected_but_got(",")
                        };
                        self.report(message, self.peek_span());
                        self.synthetic(TokenKind::Comma, self.peek_span().range.offset);
                        // Official `rewriteAndRecover` + `token = next`
                        // (:8553-8559): the loop steps onto the inserted
                        // comma so the next element parses the real token.
                        self.cursor.advance();
                        continue;
                    }
                    if let Some(close) = self.current_matching_end_for_open(left) {
                        let (_, synthetic) = self.cursor.resolve(close);
                        if !synthetic {
                            self.report(msg::expected_but_got("]"), self.peek_span());
                        }
                        self.skip_to_token(close);
                        end = self.peek_span();
                        self.cursor.advance();
                    } else {
                        self.report(msg::expected_but_got("]"), self.peek_span());
                        let at = self.peek_span();
                        end = self.synthetic(TokenKind::CloseSquare, at.range.offset);
                    }
                    break;
                }
            }
        }
        self.may_parse_function_expressions = saved;
        self.listener
            .handle_literal_list(count, left, const_keyword, end);
        end
    }

    /// Event shape of the `[]` arm when the bracket token was already
    /// consumed by [`Self::parse_list_literal`].
    fn finish_list_literal_index(
        &mut self,
        index: TokenSpan,
        const_keyword: Option<TokenSpan>,
    ) -> TokenSpan {
        let offset = index.range.offset;
        let open = self.synthetic(TokenKind::OpenSquare, offset);
        let close = self.synthetic(TokenKind::CloseSquare, offset);
        self.cursor.advance();
        self.cursor.advance();
        self.listener
            .handle_literal_list(0, open, const_keyword, close);
        close
    }

    /// Set/map literal (official `parseLiteralSetOrMapSuffix` :7893). The
    /// cursor sits on `{`.
    pub(crate) fn parse_set_or_map_literal(
        &mut self,
        const_keyword: Option<TokenSpan>,
    ) -> TokenSpan {
        let left = self.peek_span();
        self.cursor.advance();
        self.listener.begin_literal_set_or_map(left, const_keyword);
        let mut count = 0usize;
        // Official `hasSetEntry ??=` (:7926): FIRST-wins, and only the
        // simple (non-control-flow) entries decide — the unified-collection
        // TODO in the official source keeps this shape.
        let mut has_set_entry: Option<bool> = None;
        let saved = self.may_parse_function_expressions;
        self.may_parse_function_expressions = true;
        let end;
        loop {
            if self.is_bailed() {
                // Terminal bailout: unwind without consumption (see
                // `parse_list_literal`).
                end = self.peek_span();
                break;
            }
            if self.peek_kind() == TokenKind::CloseCurly {
                end = self.peek_span();
                self.cursor.advance();
                break;
            }
            if self.peek_kind() == TokenKind::Eof {
                end = self.peek_span();
                break;
            }
            let (_, if_count) = self.parse_collection_entry(true, &mut has_set_entry);
            count += 1;
            match self.peek_kind() {
                TokenKind::Comma => {
                    self.cursor.advance();
                }
                TokenKind::CloseCurly => {
                    end = self.peek_span();
                    self.cursor.advance();
                    break;
                }
                TokenKind::Eof => {
                    end = self.peek_span();
                    break;
                }
                _ => {
                    // Latched depth fuse: no entry parsing may re-enter —
                    // jump to where the scanner guaranteed the `}` silently
                    // (see the list-literal arm).
                    if self.depth_fuse_latched {
                        if let Some(close) = self.current_matching_end_for_open(left) {
                            self.skip_to_token(close);
                            end = self.peek_span();
                            self.cursor.advance();
                        } else {
                            let at = self.peek_span();
                            end = self.synthetic(TokenKind::CloseCurly, at.range.offset);
                        }
                        break;
                    }
                    // Recovery (:8041-8066): an entry-looking token means a
                    // missing comma; otherwise report and jump to where the
                    // scanner guaranteed the `}`.
                    if crate::patterns::looks_like_literal_entry(self, self.cursor.real_pos()) {
                        let message = if if_count > 0 {
                            msg::expected_else_or_comma()
                        } else {
                            msg::expected_but_got(",")
                        };
                        self.report(message, self.peek_span());
                        self.synthetic(TokenKind::Comma, self.peek_span().range.offset);
                        // Official `rewriteAndRecover` + `token = next`
                        // (:8055-8060): step onto the inserted comma.
                        self.cursor.advance();
                        continue;
                    }
                    self.report(msg::expected_but_got("}"), self.peek_span());
                    if let Some(close) = self.current_matching_end_for_open(left) {
                        self.skip_to_token(close);
                        end = self.peek_span();
                        self.cursor.advance();
                    } else {
                        let at = self.peek_span();
                        end = self.synthetic(TokenKind::CloseCurly, at.range.offset);
                    }
                    break;
                }
            }
        }
        self.may_parse_function_expressions = saved;
        self.listener.handle_literal_set_or_map(
            count,
            left,
            const_keyword,
            end,
            has_set_entry.unwrap_or(false),
        );
        end
    }

    /// Parenthesized expression or record literal (official
    /// `parseParenthesizedExpressionOrRecordLiteral` :7628). Function
    /// literals are dispatched by the caller
    /// (`parse_parenthesized_expression_function_literal_or_record_literal`).
    /// `record_literal_zero_fields_with_trailing_comma` and the empty-record
    /// constant-pattern report have no non-pattern surface at the pin, so
    /// `(,)` parses with events only.
    pub(crate) fn parse_parenthesized_or_record_literal(
        &mut self,
        const_keyword: Option<TokenSpan>,
    ) -> TokenSpan {
        let begin = self.peek_span(); // `(`
        self.cursor.advance();
        self.listener
            .begin_parenthesized_expression_or_record_literal(begin);
        let mut count = 0usize;
        let mut was_record = const_keyword.is_some();
        let mut was_valid_record = false;
        // `(,)` marks the empty record literal (:7649-7655,
        // `recordLiteralZeroFieldsWithTrailingComma`).
        let mut illegal_trailing_comma: Option<TokenSpan> = None;
        let end;
        loop {
            if self.peek_kind() == TokenKind::CloseParen {
                if count == 0 {
                    was_record = true;
                }
                end = self.ensure_close_paren(begin);
                break;
            }
            if count == 0
                && self.peek_kind() == TokenKind::Comma
                && self.cursor.look_kind(1) == TokenKind::CloseParen
            {
                // Empty record literal with a comma `(,)` (:7649-7655).
                illegal_trailing_comma = Some(self.peek_span());
                was_record = true;
                self.cursor.advance();
                end = self.ensure_close_paren(begin);
                break;
            }
            let mut colon: Option<TokenSpan> = None;
            if self.cursor.look_kind(1) == TokenKind::Colon || self.peek_kind() == TokenKind::Colon
            {
                // Record with named expression (:7662-7670); `(:` takes the
                // same path (ensure_identifier reports the missing name).
                was_record = true;
                let name = self.ensure_identifier(IdentifierContext::NamedRecordFieldReference);
                let colon_token = self.expect_lexeme_but_got(":");
                let _ = name;
                colon = Some(colon_token);
                was_valid_record = true;
            }
            let _ = self.parse_expression();
            if let Some(colon) = colon {
                self.listener.handle_named_record_field(colon);
            }
            count += 1;
            if self.peek_kind() != TokenKind::Comma {
                end = self.ensure_close_paren(begin);
                break;
            }
            // It is a comma, i.e. it's a record (:7684-7689).
            was_record = true;
            was_valid_record = true;
            self.cursor.advance();
        }
        if was_record {
            if let (0, Some(comma)) = (count, illegal_trailing_comma) {
                // Empty record literal with a comma `(,)`
                // (`recordLiteralZeroFieldsWithTrailingComma`, the analyzer's
                // `empty_record_literal_with_comma`).
                self.report(msg::empty_record_literal_with_comma(), comma);
            } else if count == 1 && !was_valid_record {
                self.report(msg::record_literal_one_positional_no_trailing_comma(), end);
            } else if count == 0
                && self.constant_pattern_ctx != crate::listener::ConstantPatternContext::None
            {
                // `case const ()` / `case ()` (:8389-8394).
                self.report(msg::invalid_constant_pattern_empty_record_literal(), begin);
            }
            self.listener.end_record_literal(end, count, const_keyword);
        } else {
            self.listener.handle_positional_record_field(end);
            self.listener.end_parenthesized_expression(end);
        }
        end
    }

    /// String literal with adjacent merging and interpolation (C21).
    pub(crate) fn parse_literal_string(&mut self) -> TokenSpan {
        self.parse_literal_string_and_first_interpolation().0
    }

    /// [`Self::parse_literal_string`] plus the first interpolation
    /// expression's span — `Some` only when the literal is a SINGLE
    /// (non-adjacent) interpolated string: the `endConditionalUri` value
    /// check (ast_builder.dart:1509-1524) tests `is StringInterpolationImpl`,
    /// so an `AdjacentStrings` value never reports.
    pub(crate) fn parse_literal_string_and_first_interpolation(
        &mut self,
    ) -> (TokenSpan, Option<(TokenSpan, TokenSpan)>) {
        let start = self.peek_span();
        let saved = self.may_parse_function_expressions;
        self.may_parse_function_expressions = true;
        let mut literal_count = 0usize;
        let mut end;
        let mut first_interpolation = None;
        loop {
            let (literal, interpolation) = self.parse_single_literal_string();
            end = literal;
            first_interpolation = first_interpolation.or(interpolation);
            literal_count += 1;
            if self.peek_kind() != TokenKind::String {
                break;
            }
        }
        if literal_count > 1 {
            self.listener
                .handle_adjacent_string_literals(start, literal_count);
            first_interpolation = None;
        }
        self.may_parse_function_expressions = saved;
        (end, first_interpolation)
    }

    /// One string token, consuming `${...}` interpolation groups (official
    /// `parseSingleLiteralString` :8732: the expression parses in full and
    /// must stop at the paired `}`); also returns the first interpolation
    /// expression's `(begin, end)` spans.
    fn parse_single_literal_string(&mut self) -> (TokenSpan, Option<(TokenSpan, TokenSpan)>) {
        let begin = self.peek_span();
        // Upstream `parseSingleLiteralString` seals
        // `endLiteralString(interpolationCount, token)` with the LAST
        // consumed String token (parser_impl.dart:9305 — the closing-quote
        // segment `parseStringPart` returned; the plain path passes the
        // opening token itself, :9595). Interpolation-free literals never
        // update it, so `end == begin` there.
        let mut end = begin;
        self.listener.begin_literal_string(begin);
        self.cursor.advance();
        let mut interpolation_count = 0usize;
        let mut first_interpolation = None;
        loop {
            match self.peek_kind() {
                TokenKind::StringInterpolationExpression => {
                    let left_id = self.peek_id();
                    let left = self.peek_span();
                    self.cursor.advance();
                    let _ = self.parse_expression();
                    let right = if self.peek_kind() == TokenKind::CloseCurly {
                        self.peek_span()
                    } else {
                        // Recovery (:8745-8750): report and jump to the
                        // scanner-paired `}`.
                        self.report(msg::expected_but_got("}"), self.peek_span());
                        match self.cursor.table_matching_end(left_id) {
                            Some(id) => {
                                self.skip_to_token(id);
                                self.span_of(id)
                            }
                            None => self.synthetic(TokenKind::CloseCurly, left.range.offset),
                        }
                    };
                    self.cursor.advance();
                    if first_interpolation.is_none() {
                        first_interpolation = Some((left, right));
                    }
                    self.listener
                        .handle_interpolation_expression(left, Some(right));
                    interpolation_count += 1;
                }
                TokenKind::StringInterpolationIdentifier => {
                    let dollar = self.peek_span();
                    self.cursor.advance();
                    let id = self.ensure_identifier(IdentifierContext::Expression);
                    if first_interpolation.is_none() {
                        first_interpolation = Some((dollar, id));
                    }
                    self.listener
                        .handle_interpolation_expression(dollar, Some(id));
                    interpolation_count += 1;
                }
                _ => break,
            }
            if self.peek_kind() == TokenKind::String {
                end = self.peek_span();
                self.cursor.advance();
            } else {
                break;
            }
        }
        self.listener.end_literal_string(interpolation_count, end);
        (begin, first_interpolation)
    }

    /// Symbol body (official `parseLiteralSymbol` :9244-9269): a
    /// user-definable operator, `void`, or `(id .)* id`. Operator membership
    /// is `isUserDefinableOperator` — not the binary-level table — so `#is`
    /// / `#as` are rejected while `#~` / `#[]` are accepted.
    pub(crate) fn parse_symbol_rest(&mut self, hash: TokenSpan) -> TokenSpan {
        if is_user_definable_operator(self.peek_kind()) {
            let op = self.peek_span();
            self.cursor.advance();
            self.listener.handle_operator(op);
            self.listener.end_literal_symbol(hash, 1);
            return op;
        }
        if self.cursor.opt_lexeme("void").is_some() {
            let void_tok = self.peek_span();
            self.listener.handle_symbol_void(hash);
            self.listener.end_literal_symbol(hash, 1);
            return void_tok;
        }
        let mut count = 1usize;
        let first = self.ensure_identifier(IdentifierContext::LiteralSymbol);
        let mut end = first;
        while self.peek_kind() == TokenKind::Period {
            self.cursor.advance();
            end = self.ensure_identifier(IdentifierContext::LiteralSymbolContinuation);
            count += 1;
        }
        self.listener.end_literal_symbol(hash, count);
        end
    }
}

// SENTINEL-ENTRY-MACHINE

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Message;

    /// Local listener capturing set/map literal shape, element counts, and
    /// error codes (the shared Collector does not log these events).
    #[derive(Default)]
    struct LiteralLog {
        errors: Vec<String>,
        identifiers: usize,
        int_literals: usize,
        map_entries: usize,
        set_or_map: Vec<bool>,
        /// The `begin_literal_set_or_map` const keyword per literal, in
        /// open order, with the int-literal count at the event (the
        /// begin-before-elements order pin).
        begins: Vec<(bool, usize)>,
        no_name: usize,
        named_record_fields: usize,
        spreads: usize,
        if_control_flow: usize,
        end_if: usize,
        end_if_else: usize,
        for_control_flow: usize,
        /// The `(begin, end)` spans of each completed string literal, in
        /// completion order — the interpolation anchor pins.
        strings: Vec<(TokenSpan, TokenSpan)>,
        /// The begin token of the string literal currently open.
        pending_string_begin: Option<TokenSpan>,
    }

    impl crate::listener::ParserListener for LiteralLog {
        fn begin_literal_set_or_map(&mut self, _: TokenSpan, const_keyword: Option<TokenSpan>) {
            self.begins
                .push((const_keyword.is_some(), self.int_literals));
        }
        fn handle_recoverable_error(&mut self, message: Message, _: TokenSpan, _: TokenSpan) {
            self.errors.push(message.code.name.to_owned());
        }

        fn handle_identifier(&mut self, _: TokenSpan, _: IdentifierContext) {
            self.identifiers += 1;
        }

        fn handle_no_name(&mut self, _: TokenSpan) {
            self.no_name += 1;
        }

        fn handle_named_record_field(&mut self, _: TokenSpan) {
            self.named_record_fields += 1;
        }

        fn handle_literal_int(&mut self, _: TokenSpan) {
            self.int_literals += 1;
        }

        fn handle_literal_map_entry(
            &mut self,
            _: TokenSpan,
            _: TokenSpan,
            _: Option<TokenSpan>,
            _: Option<TokenSpan>,
        ) {
            self.map_entries += 1;
        }

        fn handle_literal_set_or_map(
            &mut self,
            _: usize,
            _: TokenSpan,
            _: Option<TokenSpan>,
            _: TokenSpan,
            has_set_entry: bool,
        ) {
            self.set_or_map.push(has_set_entry);
        }

        fn handle_spread_expression(&mut self, _: TokenSpan) {
            self.spreads += 1;
        }

        fn begin_if_control_flow(&mut self, _: TokenSpan) {
            self.if_control_flow += 1;
        }

        fn end_if_control_flow(&mut self, _: TokenSpan) {
            self.end_if += 1;
        }

        fn end_if_else_control_flow(&mut self, _: TokenSpan) {
            self.end_if_else += 1;
        }

        fn end_for_control_flow(&mut self, _: TokenSpan) {
            self.for_control_flow += 1;
        }

        fn begin_literal_string(&mut self, begin: TokenSpan) {
            self.pending_string_begin = Some(begin);
        }

        fn end_literal_string(&mut self, _: usize, end: TokenSpan) {
            let begin = self.pending_string_begin.take().expect("string begin seen");
            self.strings.push((begin, end));
        }
    }

    fn literal_parser(
        source: &'static str,
    ) -> crate::Parser<'static, 'static, 'static, LiteralLog> {
        crate::expr_const::test_support::parser(source, LiteralLog::default())
    }

    #[test]
    fn set_or_map_parses_each_entry_once() {
        // F4: the probe double-parse emitted every element twice; the
        // single pass parses `{1, 2, 3}` exactly once.
        let mut p = literal_parser("{1, 2, 3}");
        let _ = p.parse_set_or_map_literal(None);
        assert_eq!(p.listener.int_literals, 3);
        assert_eq!(p.listener.map_entries, 0);
        assert_eq!(p.listener.set_or_map, vec![true]);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
    }

    #[test]
    fn set_entry_wins_over_later_map_entry() {
        // F4: `hasSetEntry ??=` is first-wins (official :8598) — `{1, 2: 3}`
        // leans set even though a map entry follows. Both entries ride
        // `parseExpression` (:8596) and the map-entry tail parses its value
        // expression too (:8610), so three `handleLiteralInt` events fire.
        let mut p = literal_parser("{1, 2: 3}");
        let _ = p.parse_set_or_map_literal(None);
        assert_eq!(p.listener.int_literals, 3);
        assert_eq!(p.listener.map_entries, 1);
        assert_eq!(p.listener.set_or_map, vec![true]);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);

        let mut p = literal_parser("{1: 2, 3}");
        let _ = p.parse_set_or_map_literal(None);
        assert_eq!(p.listener.set_or_map, vec![false]);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
    }

    #[test]
    fn control_flow_entries_leave_has_set_entry_unset() {
        // Official :7921-8040: only *simple* entries touch hasSetEntry, so
        // `{if (b) 1}` reports the set-or-map with hasSetEntry=false.
        let mut p = literal_parser("{if (b) 1}");
        let _ = p.parse_set_or_map_literal(None);
        assert_eq!(p.listener.if_control_flow, 1);
        assert_eq!(p.listener.end_if, 1);
        assert_eq!(p.listener.set_or_map, vec![false]);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
    }

    #[test]
    fn top_level_simple_entry_still_sets_has_set_entry() {
        // The gate is per-entry (official `info == simpleEntry` :8593): a
        // literal holding a top-level simple entry still decides. `{1, if
        // (b) 2}` sets hasSetEntry=true before the if-element (which must
        // not overwrite it, first-wins :8598); `{if (b) 1, 2}` leaves it
        // unset through the gated body and decides on the trailing simple
        // entry. Both report the set-or-map with hasSetEntry=true.
        let mut p = literal_parser("{1, if (b) 2}");
        let _ = p.parse_set_or_map_literal(None);
        assert_eq!(p.listener.if_control_flow, 1);
        assert_eq!(p.listener.end_if, 1);
        assert_eq!(p.listener.int_literals, 2);
        assert_eq!(p.listener.set_or_map, vec![true]);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);

        let mut p = literal_parser("{if (b) 1, 2}");
        let _ = p.parse_set_or_map_literal(None);
        assert_eq!(p.listener.set_or_map, vec![true]);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
    }

    #[test]
    fn if_else_and_spread_elements_event_in_order() {
        let mut p = literal_parser("[if (b) 1 else 2, ...xs]");
        let _ = p.parse_list_literal(None);
        assert_eq!(p.listener.end_if_else, 1);
        assert_eq!(p.listener.spreads, 1);
        assert_eq!(p.listener.int_literals, 2);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
    }

    #[test]
    fn for_control_flow_collection_element() {
        let mut p = literal_parser("[for (var i = 0; i < 3; i++) i]");
        let _ = p.parse_list_literal(None);
        assert_eq!(p.listener.for_control_flow, 1);
        assert_eq!(p.listener.identifiers, 4);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
    }

    #[test]
    fn for_in_control_flow_collection_element() {
        let mut p = literal_parser("[for (final x in xs) x]");
        let _ = p.parse_list_literal(None);
        assert_eq!(p.listener.for_control_flow, 0);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
    }

    #[test]
    fn begin_set_or_map_fires_once_before_elements() {
        // #747: the begin event fires right after `{`, before any element
        // event (the semantic window must open before the elements fold),
        // once per literal — nested literals begin their own.
        let mut p = literal_parser("{1, 1}");
        let _ = p.parse_set_or_map_literal(None);
        assert_eq!(p.listener.begins, vec![(false, 0)]);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);

        let mut p = literal_parser("{{1}, {2}}");
        let _ = p.parse_set_or_map_literal(None);
        // Three literals, three begins, each before its own elements: the
        // outer (0 ints), `{1}` (still 0 — its begin precedes its int), and
        // `{2}` (1 int, from `{1}`'s element).
        assert_eq!(p.listener.begins, vec![(false, 0), (false, 0), (false, 1)]);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
    }

    #[test]
    fn begin_set_or_map_carries_const_keyword() {
        // `const {1, 1}` (official `parseConstExpression` :9043): the begin
        // event carries the const keyword so the semantic face can scope
        // the duplicate window to non-const literals only.
        let mut p = literal_parser("const {1, 1}");
        let _ = p.parse_const_expression();
        assert_eq!(p.listener.begins, vec![(true, 0)]);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
    }

    #[test]
    fn record_literal_colon_prefix_reports_missing_identifier() {
        // Official :7662-7670: `(:` takes the named-field path, where
        // ensureIdentifier(namedRecordFieldReference) reports
        // missing_identifier on the `:` and emits a synthetic name — the
        // record-literal path never emits handleNoName (that is record
        // *pattern* recovery, official :11568). The synthetic lands via
        // `insertSyntheticIdentifier` (parser_impl.dart:4056-4068), which
        // reports and rewrites WITHOUT a `handleIdentifier` event, so the
        // named-field name fires zero identifier events (and
        // `expectedIdentifier` pseudo-shares missingIdentifier,
        // diagnostic.g.dart:820-825).
        let mut p = literal_parser("(: 1)");
        let _ = p.parse_parenthesized_or_record_literal(None);
        assert_eq!(p.listener.errors, vec!["missing_identifier".to_string()]);
        assert_eq!(p.listener.no_name, 0);
        assert_eq!(p.listener.identifiers, 0);
        assert_eq!(p.listener.named_record_fields, 1);

        // A well-formed named field parses without diagnostics.
        let mut p = literal_parser("(a: 1)");
        let _ = p.parse_parenthesized_or_record_literal(None);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
        assert_eq!(p.listener.no_name, 0);
        assert_eq!(p.listener.identifiers, 1);
        assert_eq!(p.listener.named_record_fields, 1);
    }

    #[test]
    fn list_literal_missing_comma_splits_elements() {
        // F10 (official :8569-8580): `[1 2]` splits into two elements with
        // exactly one expected-`,` report.
        let mut p = literal_parser("[1 2]");
        let _ = p.parse_list_literal(None);
        assert_eq!(p.listener.int_literals, 2);
        assert_eq!(p.listener.errors, vec!["expected_token".to_string()]);
    }

    #[test]
    fn list_literal_missing_comma_after_if_reports_else_or_comma() {
        // Official :8576-8580: a missing comma after an if-element's
        // condition reports expected_else_or_comma.
        let mut p = literal_parser("[if (b) 1 2]");
        let _ = p.parse_list_literal(None);
        assert_eq!(
            p.listener.errors,
            vec!["expected_else_or_comma".to_string()]
        );
    }

    /// Local listener capturing the ordered symbol event sequence,
    /// identifier contexts, and error codes (the shared Collector does not
    /// log these events).
    #[derive(Default)]
    struct SymbolLog {
        errors: Vec<String>,
        events: Vec<String>,
        contexts: Vec<IdentifierContext>,
        dot_access: usize,
        symbol_ends: Vec<usize>,
    }

    impl crate::listener::ParserListener for SymbolLog {
        fn handle_recoverable_error(&mut self, message: Message, _: TokenSpan, _: TokenSpan) {
            self.errors.push(message.code.name.to_owned());
        }

        fn begin_literal_symbol(&mut self, _: TokenSpan) {
            self.events.push("begin".to_owned());
        }

        fn handle_operator(&mut self, _: TokenSpan) {
            self.events.push("operator".to_owned());
        }

        fn handle_symbol_void(&mut self, _: TokenSpan) {
            self.events.push("symbol_void".to_owned());
        }

        fn handle_identifier(&mut self, _: TokenSpan, context: IdentifierContext) {
            self.events.push("identifier".to_owned());
            self.contexts.push(context);
        }

        fn handle_dot_access(&mut self, _: TokenSpan, _: TokenSpan, _: bool) {
            self.dot_access += 1;
        }

        fn end_literal_symbol(&mut self, _: TokenSpan, identifier_count: usize) {
            self.events.push(format!("end:{identifier_count}"));
            self.symbol_ends.push(identifier_count);
        }
    }

    fn symbol_parser(source: &'static str) -> crate::Parser<'static, 'static, 'static, SymbolLog> {
        crate::expr_const::test_support::parser(source, SymbolLog::default())
    }

    fn parse_symbol(source: &'static str) -> crate::Parser<'static, 'static, 'static, SymbolLog> {
        let mut p = symbol_parser(source);
        let hash = p.peek_span();
        p.cursor.advance();
        p.listener.begin_literal_symbol(hash);
        let end = p.parse_symbol_rest(hash);
        let _ = end;
        p
    }

    #[test]
    fn symbol_operator_membership_is_user_definable_operator() {
        // F11 (official `parseLiteralSymbol` :9244-9269): `~` is a member
        // (count 1); keywords `is`/`as` are not — they take the identifier
        // path in the keyword-tolerant literalSymbol context, so a reserved
        // word reports `expected_identifier_but_got_keyword` and is
        // consumed (identifier_context_impl.dart recovery templates;
        // official `symbols.dart.expect` messages).
        let p = parse_symbol("#~");
        assert_eq!(p.listener.symbol_ends, vec![1]);
        assert!(p.listener.events.contains(&"operator".to_owned()));
        assert!(!p.listener.events.contains(&"identifier".to_owned()));
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);

        let p = parse_symbol("#is");
        assert_eq!(p.listener.symbol_ends, vec![1]);
        assert!(!p.listener.events.contains(&"operator".to_owned()));
        assert_eq!(
            p.listener.errors,
            vec!["expected_identifier_but_got_keyword".to_string()]
        );
    }

    #[test]
    fn symbol_identifier_chain_counts_dots() {
        let p = parse_symbol("#a.b");
        assert_eq!(p.listener.symbol_ends, vec![2]);
        assert_eq!(
            p.listener.contexts,
            vec![
                IdentifierContext::LiteralSymbol,
                IdentifierContext::LiteralSymbolContinuation
            ]
        );
        // No dot-access event between symbol segments (official emits
        // nothing but the identifiers).
        assert_eq!(p.listener.dot_access, 0);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
    }

    #[test]
    fn symbol_event_sequence_matches_official_parse_literal_symbol() {
        // Operator body (`parser_impl.dart:9251-9254`): handleOperator then
        // endLiteralSymbol(hash, 1).
        let p = parse_symbol("#~");
        assert_eq!(p.listener.events, vec!["begin", "operator", "end:1"]);

        // `#void` (`:9255-9257`): handleSymbolVoid then end(hash, 1).
        let p = parse_symbol("#void");
        assert_eq!(p.listener.events, vec!["begin", "symbol_void", "end:1"]);

        // Dotted chain (`:9258-9268`): first identifier in the
        // literalSymbol context, every `.`-followed one in
        // literalSymbolContinuation, then end(hash, count).
        let p = parse_symbol("#a");
        assert_eq!(p.listener.events, vec!["begin", "identifier", "end:1"]);
        assert_eq!(p.listener.contexts, vec![IdentifierContext::LiteralSymbol]);

        let p = parse_symbol("#a.b.c");
        assert_eq!(
            p.listener.events,
            vec!["begin", "identifier", "identifier", "identifier", "end:3"]
        );
        assert_eq!(
            p.listener.contexts,
            vec![
                IdentifierContext::LiteralSymbol,
                IdentifierContext::LiteralSymbolContinuation,
                IdentifierContext::LiteralSymbolContinuation,
            ]
        );
    }

    /// The triple-shift lexing gate decides the `#>>>` recovery shape
    /// (triple_shift_symbol corpus): the official `tokenizeGreaterThan`
    /// (upstream Dart SDK source, pkg/_fe_analyzer_shared/lib/src/scanner/
    /// abstract_scanner.dart :1493) merges `>>>`/`>>>=` only under the
    /// triple-shift switch, so at a pre-2.14 settled version the scanner
    /// hands the parser `>>` + `>` and `parseLiteralSymbol` (:9249) takes
    /// the whole `>>` as the symbol operator — the bare `>` then parses as
    /// the relational binary with a missing operand, reporting
    /// `missing_identifier` at the `)`. With the gate on (>= 2.14, the
    /// default face) the whole `>>>` is the symbol operator and the unit
    /// parse stays clean.
    #[test]
    fn triple_shift_gate_decides_the_hash_triple_gt_recovery_shape() {
        fn scan(
            source: &'static str,
            triple_shift: bool,
        ) -> (&'static dartq_scanner::TokenTable, &'static str) {
            let config = dartq_scanner::ScanConfig {
                enable_triple_shift: triple_shift,
                enable_augmentations: true,
            };
            let mut files = dartq_source::FileTable::new();
            let file = files.intern(std::path::Path::new("test.dart"));
            let result = dartq_scanner::scan(source, config, file);
            assert!(
                result.diagnostics.is_empty(),
                "scanner diagnostics in {source:?}: {:?}",
                result.diagnostics
            );
            let table: &'static dartq_scanner::TokenTable = Box::leak(Box::new(result.tokens));
            let text: &'static str = Box::leak(source.to_string().into_boxed_str());
            (table, text)
        }
        let source = "main() {\n  print(#>>>);\n}\n";

        let (table, text) = scan(source, false);
        let mut log = SymbolLog::default();
        let mut p = crate::Parser::new(
            text,
            table,
            crate::expr_const::test_support::features(),
            &mut log,
        );
        p.parse_unit();
        // The harness listener hears the contract's handleIdentifier events
        // ahead of the symbol (`main` as the declaration name, `print` as
        // the send target — listener.dart's identifier events); the symbol
        // face itself is unchanged: `>>` is the symbol operator, and the
        // bare `>` is the relational binary whose operand is missing at the
        // `)`.
        assert_eq!(
            p.listener.events,
            vec!["identifier", "identifier", "begin", "operator", "end:1"]
        );
        assert_eq!(p.listener.errors, vec!["missing_identifier".to_string()]);

        let (table, text) = scan(source, true);
        let mut log = SymbolLog::default();
        let mut p = crate::Parser::new(
            text,
            table,
            crate::expr_const::test_support::features(),
            &mut log,
        );
        p.parse_unit();
        // Gate on: the whole `>>>` is the symbol operator, the unit parse
        // stays clean, and the two contract identifier events still lead.
        assert_eq!(
            p.listener.events,
            vec!["identifier", "identifier", "begin", "operator", "end:1"]
        );
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
    }

    #[test]
    fn interpolated_literal_seals_at_the_closing_string_token() {
        // #1916: upstream `parseSingleLiteralString` seals
        // `endLiteralString(count, token)` with the LAST consumed String
        // token (parser_impl.dart:9305) — the closing quote — so the
        // literal's listener extent covers the whole interpolation run,
        // matching the oracle's `StringInterpolation` node. The returned
        // head stays the begin token (the adjacent-strings caller anchors
        // on it). Spans are UTF-16 units.

        // Single-line `$id` form: begin `"æbler` (6 units), closer `"` at
        // 10..11.
        let src = "\"æbler$bar\"";
        let mut p = literal_parser(src);
        let (head, first) = p.parse_literal_string_and_first_interpolation();
        let (begin, end) = p.listener.strings[0];
        assert_eq!(begin.range.offset, 0);
        assert_eq!(begin.range.length, 6);
        assert_eq!(end.range.offset, 10);
        assert_eq!(end.range.length, 1);
        assert_eq!(end.range.end(), 11);
        assert_eq!(head.range, begin.range);
        assert!(first.is_some());
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);

        // Multiline `"""` form: begin `"""æbler` (8 units — the row the
        // pre-fix anchor produced), closer `"""` ends at 15, the oracle's
        // whole-literal extent.
        let src = "\"\"\"æbler$bar\"\"\"";
        let mut p = literal_parser(src);
        let _ = p.parse_literal_string_and_first_interpolation();
        let (begin, end) = p.listener.strings[0];
        assert_eq!(begin.range.offset, 0);
        assert_eq!(begin.range.length, 8);
        assert_eq!(end.range.offset, 12);
        assert_eq!(end.range.length, 3);
        assert_eq!(end.range.end(), 15);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);

        // The `${expr}` form: the scanner parks `${` as its own
        // StringInterpolationExpression token, and the trailing String
        // segment after the group (`c"`) is the seal.
        let src = "\"a${b}c\"";
        let mut p = literal_parser(src);
        let _ = p.parse_literal_string_and_first_interpolation();
        let (begin, end) = p.listener.strings[0];
        assert_eq!(begin.range.length, 2); // `"a`
        assert_eq!(end.range.offset, 6);
        assert_eq!(end.range.length, 2); // `c"`
        assert_eq!(end.range.end(), 8);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);

        // Interpolation-free: the begin token IS the last consumed String
        // token, so the anchor pair collapses onto it.
        let mut p = literal_parser("'abc'");
        let _ = p.parse_literal_string_and_first_interpolation();
        let (begin, end) = p.listener.strings[0];
        assert_eq!(begin.range, end.range);
        assert_eq!(end.range.end(), 5);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);

        // EOF-truncated (`"abc$de` runs out before the closer): the
        // scanner's EOF cascade appends an EMPTY synthetic String segment
        // at the break (strings.rs "Interpolated string token sequence
        // shape") — a real token the parser consumes, so it IS the last
        // consumed String token and the anchor follows it. The scanner's
        // own `unterminated_string_literal` on this source is the
        // expressions.rs code pin's face, not the parser's — driven on the
        // raw scan here (the `literal_parser` harness refuses scanner
        // diagnostics).
        let source = "\"abc$de";
        let config = dartq_scanner::ScanConfig {
            enable_triple_shift: false,
            enable_augmentations: true,
        };
        let mut files = dartq_source::FileTable::new();
        let file = files.intern(std::path::Path::new("test.dart"));
        let result = dartq_scanner::scan(source, config, file);
        let table: &'static dartq_scanner::TokenTable = Box::leak(Box::new(result.tokens));
        let text: &'static str = Box::leak(source.to_string().into_boxed_str());
        let mut log = LiteralLog::default();
        let mut p = crate::Parser::new(
            text,
            table,
            crate::expr_const::test_support::features(),
            &mut log,
        );
        let _ = p.parse_literal_string_and_first_interpolation();
        let (begin, end) = p.listener.strings[0];
        assert_eq!(
            begin.range,
            dartq_source::SourceRange {
                offset: 0,
                length: 4
            }
        );
        assert_eq!(
            end.range,
            dartq_source::SourceRange {
                offset: 7,
                length: 0
            }
        );
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
    }
}
