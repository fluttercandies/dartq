//! Literal collection/string/symbol parsing (reduced
//! `parseLiteralListSuffix` / `parseLiteralSetOrMapSuffix` /
//! `parseParenthesizedOrRecordLiteral` / string interpolation / symbols).
//! Control-flow elements (if/for/spread) are milestone #20.

use dartiq_scanner::TokenKind;

use crate::expr_const::ConstantPatternContext;
use crate::expr_const::binary_level;
use crate::listener::{IdentifierContext, ParserListener};
use crate::message::msg;
use crate::parser::Parser;
use crate::token_id::TokenSpan;

impl<L: ParserListener> Parser<'_, '_, L> {
    /// elements (if/for/spread) are milestone #20).
    pub(crate) fn parse_list_literal(&mut self, const_keyword: Option<TokenSpan>) -> TokenSpan {
        let left = self.peek_span();
        self.cursor.advance();
        let mut count = 0usize;
        let end;
        loop {
            if self.peek_kind() == TokenKind::CloseSquare {
                end = self.peek_span();
                self.cursor.advance();
                break;
            }
            if self.peek_kind() == TokenKind::Eof {
                end = self.peek_span();
                break;
            }
            self.parse_constant_expression(ConstantPatternContext::None);
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
                    self.report(msg::expected_but_got(","), self.peek_span());
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
            }
        }
        self.listener
            .handle_literal_list(count, left, const_keyword, end);
        end
    }

    /// Set/map literal (reduced `parseLiteralSetOrMapSuffix` :8568).
    pub(crate) fn parse_set_or_map_literal(
        &mut self,
        const_keyword: Option<TokenSpan>,
    ) -> TokenSpan {
        let left = self.peek_span();
        self.cursor.advance();
        let mut count = 0usize;
        let mut has_set_entry = false;
        let end;
        loop {
            if self.peek_kind() == TokenKind::CloseCurly {
                end = self.peek_span();
                self.cursor.advance();
                break;
            }
            if self.peek_kind() == TokenKind::Eof {
                end = self.peek_span();
                break;
            }
            // Entry vs element: look for `:` after the key expression.
            let mark = self.cursor.mark();
            self.parse_constant_expression(ConstantPatternContext::None);
            if self.peek_kind() == TokenKind::Colon {
                let colon = self.peek_span();
                self.cursor.advance();
                self.parse_constant_expression(ConstantPatternContext::None);
                let entry_end = self.peek_span();
                self.listener
                    .handle_literal_map_entry(colon, entry_end, None, None);
                has_set_entry = false;
            } else {
                self.cursor.reset(mark);
                self.parse_constant_expression(ConstantPatternContext::None);
                has_set_entry = true;
            }
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
                    self.report(msg::expected_but_got(","), self.peek_span());
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
        self.listener
            .handle_literal_set_or_map(count, left, const_keyword, end, has_set_entry);
        end
    }

    /// Parenthesized expression or record literal (reduced
    /// `parseParenthesizedExpressionOrRecordLiteral` :8307).
    pub(crate) fn parse_parenthesized_or_record_literal(
        &mut self,
        const_keyword: Option<TokenSpan>,
    ) -> TokenSpan {
        let begin = self.peek_span(); // `(`
        self.cursor.advance();
        self.listener
            .begin_parenthesized_expression_or_record_literal(begin);
        let mut count = 0usize;
        let mut was_record = false;
        let mut was_valid_record = false;
        let end;
        loop {
            let kind = self.peek_kind();
            if kind == TokenKind::Eof {
                end = self.peek_span();
                break;
            }
            if (count > 0 || was_record) && kind == TokenKind::CloseParen {
                end = self.peek_span();
                self.cursor.advance();
                break;
            }
            let mut colon: Option<TokenSpan> = None;
            if kind == TokenKind::Colon {
                // `(:` — named field without a name.
                was_record = true;
                was_valid_record = true;
                self.listener.handle_no_name(self.peek_span());
                colon = Some(self.peek_span());
                self.cursor.advance();
            } else if kind != TokenKind::OpenParen
                && self.cursor.peek_kind_ahead(1) == TokenKind::Colon
            {
                // Named field `id:`.
                was_record = true;
                was_valid_record = true;
                let name = self.ensure_identifier(IdentifierContext::NamedRecordFieldReference);
                colon = Some(name);
                self.cursor.advance();
            }
            if self.peek_kind() == TokenKind::CloseParen && count == 0 && !was_record {
                // `()`: empty record literal.
                was_record = true;
            }
            self.parse_constant_expression(ConstantPatternContext::None);
            count += 1;
            if self.peek_kind() == TokenKind::Comma {
                // A comma makes it a record.
                was_record = true;
                was_valid_record = true;
                self.cursor.advance();
            } else {
                let close = self.ensure_close_paren(begin);
                end = close;
                if was_record || colon.is_some() {
                    if let Some(c) = colon {
                        self.listener.handle_named_record_field(c);
                    } else {
                        self.listener.handle_positional_record_field(close);
                    }
                }
                break;
            }
        }
        if was_record {
            if count == 1 && !was_valid_record {
                self.report(msg::record_literal_one_positional_no_trailing_comma(), end);
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
        let start = self.peek_span();
        let mut literal_count = 0usize;
        let mut end;
        loop {
            end = self.parse_single_literal_string();
            literal_count += 1;
            if self.peek_kind() != TokenKind::String {
                break;
            }
        }
        if literal_count > 1 {
            self.listener
                .handle_adjacent_string_literals(start, literal_count);
        }
        end
    }

    /// One string token, consuming `${...}` interpolation groups (the linear
    /// flow from the scanner: `String` + `${` group + `}` + `String`...).
    fn parse_single_literal_string(&mut self) -> TokenSpan {
        let begin = self.peek_span();
        self.listener.begin_literal_string(begin);
        self.cursor.advance();
        let mut interpolation_count = 0usize;
        loop {
            match self.peek_kind() {
                TokenKind::StringInterpolationExpression => {
                    let left_id = self.peek_id();
                    let left = self.peek_span();
                    self.cursor.advance();
                    self.parse_constant_expression(ConstantPatternContext::None);
                    // The scanner pairs `${` with its `}`.
                    let right = if let Some(id) = self.cursor.table_matching_end(left_id) {
                        self.skip_to_token(id);
                        let span = self.span_of(id);
                        self.cursor.advance();
                        span
                    } else {
                        let at = self.peek_span();
                        self.synthetic(TokenKind::CloseCurly, at.range.offset)
                    };
                    self.listener
                        .handle_interpolation_expression(left, Some(right));
                    interpolation_count += 1;
                }
                TokenKind::StringInterpolationIdentifier => {
                    let dollar = self.peek_span();
                    self.cursor.advance();
                    let id = self.ensure_identifier(IdentifierContext::Expression);
                    self.listener
                        .handle_interpolation_expression(dollar, Some(id));
                    interpolation_count += 1;
                }
                _ => break,
            }
            if self.peek_kind() == TokenKind::String {
                self.cursor.advance();
            } else {
                break;
            }
        }
        self.listener.end_literal_string(interpolation_count, begin);
        begin
    }

    /// Symbol body: operator or `(id .)* id` / `void` (C20).
    pub(crate) fn parse_symbol_rest(&mut self, hash: TokenSpan) -> TokenSpan {
        if self.cursor.opt_lexeme("void").is_some() {
            let void_tok = self.peek_span();
            self.listener.handle_symbol_void(hash);
            return void_tok;
        }
        let mut count = 0usize;
        loop {
            let kind = self.peek_kind();
            if crate::recovery::is_identifier(kind) {
                let id = self.peek_span();
                self.cursor.advance();
                self.listener
                    .handle_identifier(id, IdentifierContext::Expression);
                count += 1;
                if self.peek_kind() == TokenKind::Period {
                    let period = self.peek_span();
                    self.cursor.advance();
                    self.listener.handle_dot_access(id, id, false);
                    let _ = period;
                    continue;
                }
            } else if let Some(level) = binary_level(kind) {
                // User-definable operator after `#`.
                let op = self.peek_span();
                self.cursor.advance();
                self.listener
                    .handle_identifier(op, IdentifierContext::Expression);
                count += 1;
                let _ = level;
            }
            break;
        }
        self.listener.end_literal_symbol(hash, count);
        hash
    }
}
