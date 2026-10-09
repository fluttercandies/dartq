//! Reduced constant-expression engine.
//!
//! Constant patterns, formal-parameter default values, and annotation
//! arguments all need *some* expression parser. The full precedence engine
//! (C1-C12: assignments, cascades, conditionals, dot-shorthand, anonymous
//! methods, implicit-creation disambiguation) is milestone #20; until then
//! this module parses the constant-relevant subset with official events:
//!
//! * literals (int/hex/double with separators, strings with interpolation,
//!   bool/null, symbols), `this`/`super`, `const`-prefixed collections and
//!   constructor references, `new`, parenthesized/record literals;
//! * identifier/qualified sends with type arguments, arguments, `.`/`?.`
//!   selectors, `[index]`, and postfix `!`;
//! * unary prefixes (`-`, `!`, `~`, `++`, `--`; `+` reports
//!   `missing_identifier` per `unsupportedPrefixPlus`);
//! * binary operator chains at precedence levels above the caller's floor
//!   (equality/relational/is/as included, with the
//!   `equalityCannotBeEqualityOperand` chain check);
//! * the constant-pattern over-collection diagnostics of
//!   `_checkForInvalidConstantPatternOperator` (`parser_impl.dart:7507`) so
//!   `case 1 + 2` reports exactly like the oracle.
//!
//! # Precedence table (scanner/token.dart:18-34)

use dartiq_scanner::{Keyword, TokenKind};

pub(crate) use crate::listener::ConstantPatternContext;
use crate::listener::{ConstructorReferenceContext, IdentifierContext, ParserListener};
use crate::message::msg;
use crate::parser::Parser;
use crate::token_id::TokenSpan;

pub(crate) const IF_NULL_PRECEDENCE: u8 = 4;
pub(crate) const LOGICAL_OR_PRECEDENCE: u8 = 5;
pub(crate) const LOGICAL_AND_PRECEDENCE: u8 = 6;
pub(crate) const EQUALITY_PRECEDENCE: u8 = 7;
pub(crate) const RELATIONAL_PRECEDENCE: u8 = 8;
pub(crate) const BITWISE_OR_PRECEDENCE: u8 = 9;
pub(crate) const BITWISE_XOR_PRECEDENCE: u8 = 10;
pub(crate) const BITWISE_AND_PRECEDENCE: u8 = 11;
pub(crate) const SHIFT_PRECEDENCE: u8 = 12;
pub(crate) const ADDITIVE_PRECEDENCE: u8 = 13;
pub(crate) const MULTIPLICATIVE_PRECEDENCE: u8 = 14;
pub(crate) const SELECTOR_PRECEDENCE: u8 = 17;

/// `_computePrecedence` for the binary loop (`forPattern: false`): the level
/// of an infix operator, `None` when the token cannot continue an expression.
pub(crate) fn binary_level(kind: TokenKind) -> Option<u8> {
    Some(match kind {
        TokenKind::QuestionQuestion => IF_NULL_PRECEDENCE,
        TokenKind::BarBar => LOGICAL_OR_PRECEDENCE,
        TokenKind::AmpersandAmpersand => LOGICAL_AND_PRECEDENCE,
        TokenKind::EqEq | TokenKind::BangEq => EQUALITY_PRECEDENCE,
        TokenKind::Lt
        | TokenKind::Gt
        | TokenKind::LtEq
        | TokenKind::GtEq
        | TokenKind::Keyword(Keyword::As | Keyword::Is) => RELATIONAL_PRECEDENCE,
        TokenKind::Bar => BITWISE_OR_PRECEDENCE,
        TokenKind::Caret => BITWISE_XOR_PRECEDENCE,
        TokenKind::Ampersand => BITWISE_AND_PRECEDENCE,
        TokenKind::LtLt | TokenKind::GtGt | TokenKind::GtGtGt => SHIFT_PRECEDENCE,
        TokenKind::Plus | TokenKind::Minus => ADDITIVE_PRECEDENCE,
        TokenKind::Star | TokenKind::Slash | TokenKind::TildeSlash | TokenKind::Percent => {
            MULTIPLICATIVE_PRECEDENCE
        }
        _ => return None,
    })
}

impl<L: ParserListener> Parser<'_, '_, L> {
    /// Parses a constant-pattern/default-value/annotation operand at the
    /// cursor (the reduced `parsePrecedenceExpression` at
    /// `EQUALITY_PRECEDENCE`).
    pub(crate) fn parse_constant_expression(
        &mut self,
        context: ConstantPatternContext,
    ) -> TokenSpan {
        self.parse_precedence(EQUALITY_PRECEDENCE, context)
    }

    /// Operand parse with an explicit precedence floor (relational patterns
    /// use `BITWISE_OR_PRECEDENCE`, official `parser_impl.dart:11810`).
    pub(crate) fn parse_precedence_floor(&mut self, floor: u8) -> TokenSpan {
        self.parse_precedence(floor, ConstantPatternContext::None)
    }

    /// The precedence loop over binary operators (reduced
    /// `_parsePrecedenceExpressionLoop`, `parser_impl.dart:7203`).
    fn parse_precedence(&mut self, precedence: u8, context: ConstantPatternContext) -> TokenSpan {
        let mut context = context;
        let mut left = self.parse_unary(context);
        let mut saw_comparison = false;
        loop {
            let kind = self.peek_kind();
            // Constant-pattern exits (parser_impl.dart:7215-7232): a postfix
            // `!` (null-assert pattern) and `as` (cast pattern) belong to the
            // pattern layer.
            if context != ConstantPatternContext::None
                && (kind == TokenKind::Bang || kind == TokenKind::Keyword(Keyword::As))
            {
                break;
            }
            let Some(level) = binary_level(kind) else {
                break;
            };
            if level < precedence {
                break;
            }
            // Over-collection diagnostics (parser_impl.dart:7507-7543).
            if context != ConstantPatternContext::None && level < SELECTOR_PRECEDENCE {
                if context == ConstantPatternContext::Explicit {
                    self.report(msg::invalid_constant_const_prefix(), left);
                } else if level <= MULTIPLICATIVE_PRECEDENCE {
                    let operator = self.peek_span();
                    self.report(
                        msg::invalid_constant_pattern_binary(self.cursor.lexeme(operator.id)),
                        operator,
                    );
                }
                context = ConstantPatternContext::None;
            }
            // `==`/`!=` and relational operators do not chain.
            if level == EQUALITY_PRECEDENCE || level == RELATIONAL_PRECEDENCE {
                if saw_comparison {
                    self.report(msg::equality_cannot_be_equality_operand(), self.peek_span());
                }
                saw_comparison = true;
            } else {
                saw_comparison = false;
            }

            let operator = self.peek_span();
            self.cursor.advance();
            match kind {
                TokenKind::Keyword(Keyword::Is) => {
                    // `is` / `is!` type (official parseIsOperatorRest :9643).
                    let not = self
                        .cursor
                        .opt_kind(TokenKind::Bang)
                        .map(|id| self.span_of(id));
                    self.listener.begin_is_operator_type(operator);
                    let type_info = self.compute_type_after_is_or_as();
                    left = type_info.ensure_type_not_void(self);
                    self.listener.end_is_operator_type(operator);
                    self.listener.handle_is_operator(operator, not);
                }
                TokenKind::Keyword(Keyword::As) => {
                    // `as` type (official parseAsOperatorRest :9706).
                    self.listener.begin_as_operator_type(operator);
                    let type_info = self.compute_type_after_is_or_as();
                    left = type_info.ensure_type_not_void(self);
                    self.listener.end_as_operator_type(operator);
                    self.listener.handle_as_operator(operator);
                }
                _ => {
                    self.listener.begin_binary_expression(operator);
                    // Left associative: recurse one level up.
                    let right = self.parse_precedence(level + 1, ConstantPatternContext::None);
                    self.listener.end_binary_expression(operator, right);
                    left = right;
                }
            }
        }
        left
    }

    /// Prefix/unary position (reduced `parseUnaryExpression` :7881).
    fn parse_unary(&mut self, context: ConstantPatternContext) -> TokenSpan {
        match self.peek_kind() {
            TokenKind::Plus => {
                // Official `unsupportedPrefixPlus` (:7904-7916): report +
                // synthetic identifier, then keep parsing — the synthetic
                // becomes the operand and `+` falls through to the binary
                // loop (analyzer code `missing_identifier`).
                let plus = self.peek_span();
                self.report(msg::missing_identifier(), plus);
                let synthetic = self.synthetic(TokenKind::Identifier, plus.range.offset);
                self.listener
                    .handle_identifier(synthetic, IdentifierContext::Expression);
                self.parse_unary(context)
            }
            TokenKind::Minus
            | TokenKind::Bang
            | TokenKind::Tilde
            | TokenKind::PlusPlus
            | TokenKind::MinusMinus => {
                let operator = self.peek_span();
                self.cursor.advance();
                self.listener.handle_unary_prefix_expression(operator);
                self.parse_unary(context)
            }
            TokenKind::Keyword(Keyword::Await) => {
                let begin = self.peek_span();
                self.cursor.advance();
                self.listener.begin_await_expression(begin);
                let end = self.parse_unary(context);
                self.listener.end_await_expression(begin, end);
                end
            }
            TokenKind::Keyword(Keyword::Throw) => {
                let throw = self.peek_span();
                self.cursor.advance();
                let end = self.parse_unary(context);
                self.listener.handle_throw_expression(throw, end);
                end
            }
            _ => self.parse_primary(context),
        }
    }

    /// Primary position (reduced `parsePrimary` :8103 + literal/send
    /// productions C13-C31).
    #[allow(clippy::too_many_lines)] // mirrors the official branch structure
    fn parse_primary(&mut self, context: ConstantPatternContext) -> TokenSpan {
        match self.peek_kind() {
            TokenKind::Int | TokenKind::Hexadecimal => {
                let token = self.peek_span();
                self.cursor.advance();
                if self.cursor.lexeme(token.id).contains('_') {
                    self.listener.handle_literal_int_with_separators(token);
                } else {
                    self.listener.handle_literal_int(token);
                }
                self.parse_selector_chain(token)
            }
            TokenKind::Double => {
                let token = self.peek_span();
                self.cursor.advance();
                if self.cursor.lexeme(token.id).contains('_') {
                    self.listener.handle_literal_double_with_separators(token);
                } else {
                    self.listener.handle_literal_double(token);
                }
                self.parse_selector_chain(token)
            }
            TokenKind::String => self.parse_literal_string(),
            TokenKind::Keyword(Keyword::True | Keyword::False) => {
                let token = self.peek_span();
                self.cursor.advance();
                self.listener.handle_literal_bool(token);
                self.parse_selector_chain(token)
            }
            TokenKind::Keyword(Keyword::Null) => {
                let token = self.peek_span();
                self.cursor.advance();
                self.listener.handle_literal_null(token);
                self.parse_selector_chain(token)
            }
            TokenKind::Hash => {
                let hash = self.peek_span();
                self.cursor.advance();
                self.listener.begin_literal_symbol(hash);
                let end = self.parse_symbol_rest(hash);
                self.parse_selector_chain(end)
            }
            TokenKind::Keyword(Keyword::This) => {
                let token = self.peek_span();
                self.cursor.advance();
                self.listener
                    .handle_this_expression(token, IdentifierContext::Expression);
                self.parse_selector_chain(token)
            }
            TokenKind::Keyword(Keyword::Super) => {
                let token = self.peek_span();
                self.cursor.advance();
                self.listener
                    .handle_super_expression(token, IdentifierContext::Expression);
                self.parse_selector_chain(token)
            }
            TokenKind::Keyword(Keyword::Const) => {
                let end = self.parse_const_expression();
                self.parse_selector_chain(end)
            }
            TokenKind::Keyword(Keyword::New) => {
                let token = self.peek_span();
                self.cursor.advance();
                self.listener.begin_new_expression(token);
                self.parse_constructor_reference(ConstructorReferenceContext::New);
                let end = self.parse_arguments_opt();
                self.listener.end_new_expression(end);
                self.parse_selector_chain(end)
            }
            TokenKind::OpenParen => {
                let end = self.parse_parenthesized_or_record_literal(None);
                self.parse_selector_chain(end)
            }
            TokenKind::OpenSquare | TokenKind::Index => {
                let end = self.parse_list_literal(None);
                self.parse_selector_chain(end)
            }
            TokenKind::OpenCurly => {
                let end = self.parse_set_or_map_literal(None);
                self.parse_selector_chain(end)
            }
            TokenKind::Lt => {
                // `<T>[...]` / `<T>{...}` literal with type arguments.
                let type_param = self.compute_type_param_or_arg_on_lt(false);
                let _ = type_param.parse_arguments(self);
                match self.peek_kind() {
                    TokenKind::OpenSquare | TokenKind::Index => {
                        let end = self.parse_list_literal(None);
                        self.parse_selector_chain(end)
                    }
                    TokenKind::OpenCurly => {
                        let end = self.parse_set_or_map_literal(None);
                        self.parse_selector_chain(end)
                    }
                    _ => {
                        // Reduced engine does not parse function literals
                        // (milestone #20): report and stop.
                        let at = self.peek_span();
                        self.report(msg::expected_but_got("["), at);
                        at
                    }
                }
            }
            kind if crate::recovery::is_identifier(kind) => {
                let begin = self.peek_span();
                let end = self.parse_send(begin, context);
                self.parse_selector_chain(end)
            }
            _ => {
                // No primary here; report and stay (the caller's loop
                // no-progress guards unwind).
                let at = self.peek_span();
                self.report(msg::expected_but_got("an expression"), at);
                at
            }
        }
    }

    /// Postfix selector loop (`.`/`?.`/`(`/`[`/`!`) — the reduced
    /// `parseArgumentOrIndexStar` (:8004).
    fn parse_selector_chain(&mut self, head: TokenSpan) -> TokenSpan {
        let mut end = head;
        loop {
            match self.peek_kind() {
                TokenKind::Period | TokenKind::QuestionPeriod => {
                    let is_null_aware = self.peek_kind() == TokenKind::QuestionPeriod;
                    self.cursor.advance();
                    let name = self.ensure_identifier(IdentifierContext::ExpressionContinuation);
                    self.listener.handle_dot_access(name, name, is_null_aware);
                    end = name;
                }
                TokenKind::OpenParen => {
                    end = self.parse_arguments();
                }
                TokenKind::OpenSquare => {
                    let open = self.peek_span();
                    self.cursor.advance();
                    let _ = self.parse_constant_expression(ConstantPatternContext::None);
                    let close = self.expect_kind(TokenKind::CloseSquare);
                    self.listener.handle_indexed_expression(None, open, close);
                    end = close;
                }
                TokenKind::Bang => {
                    let bang = self.peek_span();
                    self.cursor.advance();
                    self.listener.handle_non_null_assert_expression(bang);
                    end = bang;
                }
                _ => return end,
            }
        }
    }

    /// Identifier-led send or implicit-creation expression (reduced
    /// `parseSend` :9348 / `parseImplicitCreationExpression` :9009).
    fn parse_send(&mut self, begin: TokenSpan, context: ConstantPatternContext) -> TokenSpan {
        self.cursor.advance();
        self.listener
            .handle_identifier(begin, IdentifierContext::Expression);
        // Type arguments directly followed by `(`.
        if self.peek_kind() == TokenKind::Lt {
            let type_param = self.compute_type_param_or_arg(false);
            if !matches!(type_param, crate::types::TypeParamOrArg::None) {
                let mark = self.cursor.mark();
                let open = self.peek_span();
                type_param.parse_arguments(self);
                if self.peek_kind() == TokenKind::OpenParen {
                    self.listener.handle_type_argument_application(open);
                    let end = self.parse_arguments();
                    return end;
                }
                self.cursor.reset(mark);
            }
        }
        if self.peek_kind() == TokenKind::OpenParen {
            return self.parse_arguments();
        }
        let next = self.peek_span();
        self.listener
            .handle_send_without_arguments(begin, begin, next);
        let _ = context;
        begin
    }

    /// Argument list (reduced `parseArgumentsRest` :9523).
    pub(crate) fn parse_arguments(&mut self) -> TokenSpan {
        let open = self.peek_span();
        self.cursor.advance();
        self.listener.begin_arguments(open);
        let mut count = 0usize;
        let end;
        loop {
            if self.peek_kind() == TokenKind::CloseParen {
                end = self.peek_span();
                self.cursor.advance();
                break;
            }
            // Named argument: `id:`.
            let named =
                self.peek_is_identifier() && self.cursor.peek_kind_ahead(1) == TokenKind::Colon;
            if named {
                let name = self.peek_span();
                self.cursor.advance();
                self.listener
                    .handle_identifier(name, IdentifierContext::NamedArgumentReference);
                let colon = {
                    let id = self.cursor.advance();
                    self.span_of(id)
                };
                self.listener.handle_named_argument(colon);
                self.parse_constant_expression(ConstantPatternContext::None);
            } else {
                self.parse_constant_expression(ConstantPatternContext::None);
                self.listener.handle_positional_argument(self.peek_span());
            }
            count += 1;
            match self.peek_kind() {
                TokenKind::Comma => {
                    self.cursor.advance();
                }
                TokenKind::CloseParen => {
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
                    if let Some(close) = self.current_matching_end_for_open(open) {
                        self.skip_to_token(close);
                        end = self.peek_span();
                        self.cursor.advance();
                    } else {
                        let at = self.peek_span();
                        end = self.synthetic(TokenKind::CloseParen, at.range.offset);
                    }
                    break;
                }
            }
        }
        self.listener.end_arguments(count, open, end);
        end
    }

    pub(crate) fn current_matching_end_for_open(
        &self,
        open: TokenSpan,
    ) -> Option<crate::token_id::TokenId> {
        self.cursor.table_matching_end(open.id)
    }

    /// Optional argument list (`parseArgumentsOpt` :9494).
    fn parse_arguments_opt(&mut self) -> TokenSpan {
        if self.peek_kind() == TokenKind::OpenParen {
            return self.parse_arguments();
        }
        let next = self.peek_span();
        self.listener.handle_no_arguments(next);
        next
    }

    /// Constructor reference `id (.id)? typeArgs? (.id)?` (reduced
    /// `parseConstructorReference` :6414). Returns the last consumed span.
    pub(crate) fn parse_constructor_reference(
        &mut self,
        context: ConstructorReferenceContext,
    ) -> TokenSpan {
        let start = self.peek_span();
        self.listener.begin_constructor_reference(start);
        self.ensure_identifier(IdentifierContext::Expression);
        let mut period_before_name = None;
        if self.peek_kind() == TokenKind::Period {
            let period = self.peek_span();
            self.cursor.advance();
            let second = self.ensure_identifier(IdentifierContext::ExpressionContinuation);
            let _ = second;
            period_before_name = Some(period);
        }
        // Optional type arguments (must be followed by `(` at the use site).
        if self.peek_kind() == TokenKind::Lt {
            let open = self.peek_span();
            let type_param = self.compute_type_param_or_arg_on_lt(false);
            type_param.parse_arguments(self);
            if self.peek_kind() != TokenKind::OpenParen {
                self.listener
                    .handle_no_constructor_reference_continuation_after_type_arguments(open);
            }
        }
        let end = self.peek_span();
        self.listener
            .end_constructor_reference(start, period_before_name, end, context);
        end
    }

    /// `const` expression (official `parseConstExpression` :9043, List/Set
    /// special-case recovery reduced).
    pub(crate) fn parse_const_expression(&mut self) -> TokenSpan {
        let const_keyword = self.peek_span();
        self.cursor.advance();
        match self.peek_kind() {
            TokenKind::OpenSquare | TokenKind::Index => {
                self.listener.begin_const_literal(self.peek_span());
                let next = self.peek_span();
                self.listener.handle_no_type_arguments(next);
                let end = self.parse_list_literal(Some(const_keyword));
                self.listener.end_const_literal(end);
                end
            }
            TokenKind::OpenParen => {
                self.listener.begin_const_literal(self.peek_span());
                let end = self.parse_parenthesized_or_record_literal(Some(const_keyword));
                self.listener.end_const_literal(end);
                end
            }
            TokenKind::OpenCurly => {
                self.listener.begin_const_literal(self.peek_span());
                let next = self.peek_span();
                self.listener.handle_no_type_arguments(next);
                let end = self.parse_set_or_map_literal(Some(const_keyword));
                self.listener.end_const_literal(end);
                end
            }
            TokenKind::Lt => {
                self.listener.begin_const_literal(self.peek_span());
                let type_param = self.compute_type_param_or_arg_on_lt(false);
                let _ = type_param.parse_arguments(self);
                let end = match self.peek_kind() {
                    TokenKind::OpenSquare | TokenKind::Index => {
                        self.parse_list_literal(Some(const_keyword))
                    }
                    TokenKind::OpenCurly => self.parse_set_or_map_literal(Some(const_keyword)),
                    _ => {
                        let at = self.peek_span();
                        self.report(msg::expected_but_got("["), at);
                        at
                    }
                };
                self.listener.end_const_literal(end);
                end
            }
            _ => {
                // `const Foo.bar<int>(...)`.
                self.listener.begin_const_expression(const_keyword);
                self.parse_constructor_reference(ConstructorReferenceContext::Const);
                let end = self.parse_arguments_opt();
                self.listener.end_const_expression(end);
                end
            }
        }
    }
}
