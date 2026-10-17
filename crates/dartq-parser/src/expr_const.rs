//! Reduced constant-expression engine (the constant-*pattern* lane).
//!
//! Constant patterns, relational-pattern operands, and the
//! `parseConstExpression` family parse here with official events and the
//! constant-pattern over-collection diagnostics
//! (`_checkForInvalidConstantPatternOperator`, `parser_impl.dart:7507`).
//! The general expression engine (assignment, cascades, conditionals,
//! function literals, dot shorthands, collections with if/for/spread) lives
//! in `expressions.rs`/`literals.rs`; this module keeps its own reduced
//! precedence walk so the pattern-side observable behavior (including the
//! #153 F2 equality-chain logic) stays pinned. Shared pieces:
//! collections/strings (`literals.rs`), arguments and constructor
//! references (the full engine), and the binary-level table below.

use dartq_scanner::{Keyword, TokenKind};

pub(crate) use crate::listener::ConstantPatternContext;
use crate::listener::{AssertKind, ConstructorReferenceContext, IdentifierContext, ParserListener};
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
        // `===`/`!==` carry EQUALITY_PRECEDENCE in the official token table
        // (token.dart:1767-1773): the scanner's `unsupported_operator`
        // ErrorToken is the only diagnostic and the expression still parses
        // (`var b = 1 !== 2;` — the oracle mints no parser rows).
        TokenKind::EqEq | TokenKind::BangEq | TokenKind::EqEqEq | TokenKind::BangEqEq => {
            EQUALITY_PRECEDENCE
        }
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

/// Official `isUserDefinableOperator` (`token.dart:1705+`): the operators
/// that may follow `#` in a symbol literal. Deliberately NOT
/// `binary_level` — keywords (`is`/`as`) are not operators and `~`/`[]`
/// are not binary levels.
pub(crate) fn is_user_definable_operator(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Ampersand
            | TokenKind::Bar
            | TokenKind::Caret
            | TokenKind::EqEq
            | TokenKind::Gt
            | TokenKind::GtEq
            | TokenKind::GtGt
            | TokenKind::GtGtGt
            | TokenKind::Index
            | TokenKind::IndexEq
            | TokenKind::Lt
            | TokenKind::LtEq
            | TokenKind::LtLt
            | TokenKind::Minus
            | TokenKind::Percent
            | TokenKind::Plus
            | TokenKind::Slash
            | TokenKind::Star
            | TokenKind::Tilde
            | TokenKind::TildeSlash
    )
}

/// `_isNewOrIdentifier` (`parser_impl.dart:4090`): `new` names the unnamed
/// constructor and reads as an identifier in constructor-name positions.
fn is_new_or_identifier(kind: TokenKind) -> bool {
    crate::recovery::is_identifier(kind) || kind == TokenKind::Keyword(Keyword::New)
}

// —— registry-backed messages ——————————————————————————————————————————
// The shared `msg` module lives in message.rs (another lane's file); these
// templates follow the same `_fe_analyzer_shared` texts against the same
// build-generated registry constants.

fn invalid_constant_pattern_unary(operator: &str) -> crate::message::Message {
    crate::message::Message::new(
        dartq_diagnostics::INVALID_CONSTANT_PATTERN_UNARY,
        format!("The unary operator {operator} is not supported as a constant pattern."),
    )
}

fn invalid_constant_pattern_negation() -> crate::message::Message {
    crate::message::Message::new(
        dartq_diagnostics::INVALID_CONSTANT_PATTERN_NEGATION,
        "Only negation of a numeric literal is supported as a constant pattern.",
    )
}

fn invalid_constant_pattern_generic() -> crate::message::Message {
    crate::message::Message::new(
        dartq_diagnostics::INVALID_CONSTANT_PATTERN_GENERIC,
        "This expression is not supported as a constant pattern.",
    )
}

fn invalid_constant_pattern_duplicate_const() -> crate::message::Message {
    crate::message::Message::new(
        dartq_diagnostics::INVALID_CONSTANT_PATTERN_DUPLICATE_CONST,
        "Duplicate 'const' keyword in constant expression.",
    )
}

impl<L: ParserListener> Parser<'_, '_, '_, L> {
    /// Parses a constant-pattern/default-value/annotation operand at the
    /// cursor (the reduced `parsePrecedenceExpression` at
    /// `EQUALITY_PRECEDENCE`).
    pub(crate) fn parse_constant_expression(
        &mut self,
        context: ConstantPatternContext,
    ) -> TokenSpan {
        self.parse_pattern_precedence(EQUALITY_PRECEDENCE, context)
    }

    /// Operand parse with an explicit precedence floor (relational patterns
    /// use `BITWISE_OR_PRECEDENCE`, official `parser_impl.dart:11810`).
    pub(crate) fn parse_precedence_floor(&mut self, floor: u8) -> TokenSpan {
        self.parse_pattern_precedence(floor, ConstantPatternContext::None)
    }

    /// The precedence loop over binary operators (reduced
    /// `_parsePrecedenceExpressionLoop`, `parser_impl.dart:7203`).
    ///
    /// Depth-guarded: operands recurse back here through send arguments and
    /// parenthesized/grouped expressions, so a deeply nested constant is
    /// unbounded recursion without the guard (official
    /// `recoverFromStackOverflow`).
    fn parse_pattern_precedence(
        &mut self,
        precedence: u8,
        context: ConstantPatternContext,
    ) -> TokenSpan {
        // Keep [`Parser::constant_pattern_ctx`] in step with the threaded
        // parameter so the record-literal parse can read it.
        let saved = self.constant_pattern_ctx;
        self.constant_pattern_ctx = context;
        let end = self.parse_pattern_precedence_inner(precedence, context);
        self.constant_pattern_ctx = saved;
        end
    }

    fn parse_pattern_precedence_inner(
        &mut self,
        precedence: u8,
        context: ConstantPatternContext,
    ) -> TokenSpan {
        if !self.enter_depth() {
            return self.peek_span();
        }
        let mut context = context;
        let mut left = self.parse_pattern_unary(context);
        // Method type arguments the operand left unconsumed (official
        // `parsePrecedenceExpression` :7129-7147): `parseSend` discards a
        // `<...>` group that does not open an invocation, and this level
        // consumes it — a non-`(` follower is a generic tear-off, which a
        // constant-pattern context reports once
        // (`invalidConstantPatternGeneric`); the group never reaches the
        // binary loop, so no comparison chain is minted.
        if self.peek_kind() == TokenKind::Lt {
            let lt = self.peek_span();
            let candidate = self.compute_type_param_or_arg_on_lt(false);
            if !matches!(candidate, crate::types::TypeParamOrArg::None) {
                let _ = candidate.parse_arguments(self);
                if self.peek_kind() != TokenKind::OpenParen {
                    if context != ConstantPatternContext::None {
                        self.report(invalid_constant_pattern_generic(), lt);
                    }
                    self.listener.handle_type_argument_application(lt);
                }
                // The selector level continues past the group
                // (`C<int>.new`, official :7825-7841).
                left = self.parse_selector_chain(left, context);
            }
        }
        // `lastBinaryExpressionLevel` (`parser_impl.dart:7263`): the level of
        // the previous plain binary at the equality/relational tiers, used to
        // reject `a == b == c` chains. Only the plain-binary arm reads or
        // writes it — `is`/`as`/conditional never touch it.
        let mut last_binary_level: Option<u8> = None;
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
                    // Plain binary (official :7386-7393): only here do
                    // equality/relational chains get rejected — `x is int == y`
                    // and `x as int as double` are legal.
                    if level == EQUALITY_PRECEDENCE || level == RELATIONAL_PRECEDENCE {
                        if last_binary_level == Some(level) {
                            self.report(
                                msg::equality_cannot_be_equality_operand(),
                                self.peek_span(),
                            );
                        } else {
                            last_binary_level = Some(level);
                        }
                    }
                    self.listener.begin_binary_expression(operator);
                    // Left associative: recurse one level up.
                    let right =
                        self.parse_pattern_precedence(level + 1, ConstantPatternContext::None);
                    self.listener.end_binary_expression(operator, right);
                    left = right;
                }
            }
        }
        self.leave_depth();
        left
    }

    /// Prefix/unary position (reduced `parseUnaryExpression` :7881).
    ///
    /// Constant-pattern context handling mirrors the official arms: `!`/`~`
    /// report `invalidConstantPatternUnary` and reset the context, `-`
    /// reports `invalidConstantPatternConstPrefix` when explicit and
    /// otherwise switches to numeric-literal-only, and `++`/`--` reset the
    /// context (`parser_impl.dart:7892-7958`).
    fn parse_pattern_unary(&mut self, context: ConstantPatternContext) -> TokenSpan {
        match self.peek_kind() {
            TokenKind::Plus => {
                // Official `unsupportedPrefixPlus` (:7904-7916): report +
                // synthetic identifier, then keep parsing — the synthetic
                // becomes the operand and `+` falls through to the binary
                // loop. The CFE code is pseudo-shared with the analyzer's
                // missing_identifier (error_converter.dart), which is what
                // the oracle surfaces, so we report missing_identifier
                // directly.
                let plus = self.peek_span();
                self.report(msg::missing_identifier(), plus);
                let synthetic = self.synthetic(TokenKind::Identifier, plus.range.offset);
                self.listener
                    .handle_identifier(synthetic, IdentifierContext::Expression);
                self.parse_pattern_unary(context)
            }
            TokenKind::Bang | TokenKind::Tilde => {
                let operator = self.peek_span();
                if context != ConstantPatternContext::None {
                    self.report(
                        invalid_constant_pattern_unary(self.cursor.lexeme(operator.id)),
                        operator,
                    );
                }
                self.cursor.advance();
                self.listener.handle_unary_prefix_expression(operator);
                self.parse_pattern_unary(ConstantPatternContext::None)
            }
            TokenKind::Minus => {
                let operator = self.peek_span();
                let mut operand_context = context;
                if operand_context == ConstantPatternContext::Explicit {
                    self.report(msg::invalid_constant_const_prefix(), operator);
                    operand_context = ConstantPatternContext::None;
                }
                // Negation only supports numeric literals: the operand parses
                // with the negation checks of `parsePrimary` active.
                if operand_context != ConstantPatternContext::None {
                    operand_context = ConstantPatternContext::NumericLiteralOnly;
                }
                self.cursor.advance();
                self.listener.handle_unary_prefix_expression(operator);
                self.parse_pattern_unary(operand_context)
            }
            TokenKind::PlusPlus | TokenKind::MinusMinus => {
                let operator = self.peek_span();
                self.cursor.advance();
                self.listener.handle_unary_prefix_expression(operator);
                self.parse_pattern_unary(ConstantPatternContext::None)
            }
            TokenKind::Keyword(Keyword::Await) => {
                let begin = self.peek_span();
                self.cursor.advance();
                self.listener.begin_await_expression(begin);
                // Official parseAwaitExpression recurses with
                // ConstantPatternContext.none (:7934): the await-expression
                // operand carries no const-pattern checks (oracle
                // const_patterns 70 `case const await 0:` — one dead-code +
                // non-constant row, no const-prefix face on the `0`).
                let end = self.parse_pattern_unary(ConstantPatternContext::None);
                self.listener.end_await_expression(begin, end);
                end
            }
            TokenKind::Keyword(Keyword::Throw) => {
                let throw = self.peek_span();
                self.cursor.advance();
                let end = self.parse_pattern_unary(context);
                self.listener.handle_throw_expression(throw, end);
                end
            }
            _ => self.parse_pattern_primary(context),
        }
    }

    /// Primary checks of official `parsePrimary` for non-numeric literals
    /// (`parser_impl.dart:7440-7523`): an explicit `const` prefix reports
    /// `invalidConstantPatternConstPrefix`, negation of a non-numeric literal
    /// reports `invalidConstantPatternNegation`.
    fn check_constant_pattern_literal(&mut self, context: ConstantPatternContext, at: TokenSpan) {
        match context {
            ConstantPatternContext::Explicit => {
                self.report(msg::invalid_constant_const_prefix(), at);
            }
            ConstantPatternContext::NumericLiteralOnly => {
                self.report(invalid_constant_pattern_negation(), at);
            }
            ConstantPatternContext::None | ConstantPatternContext::Implicit => {}
        }
    }

    /// Primary position (reduced `parsePrimary` :8103 + literal/send
    /// productions C13-C31).
    #[allow(clippy::too_many_lines)] // mirrors the official branch structure
    fn parse_pattern_primary(&mut self, context: ConstantPatternContext) -> TokenSpan {
        match self.peek_kind() {
            TokenKind::Int | TokenKind::Hexadecimal => {
                let token = self.peek_span();
                if context == ConstantPatternContext::Explicit {
                    self.report(msg::invalid_constant_const_prefix(), token);
                }
                self.cursor.advance();
                if self.cursor.lexeme(token.id).contains('_') {
                    self.listener.handle_literal_int_with_separators(token);
                } else {
                    self.listener.handle_literal_int(token);
                }
                self.parse_selector_chain(token, context)
            }
            TokenKind::Double => {
                let token = self.peek_span();
                if context == ConstantPatternContext::Explicit {
                    self.report(msg::invalid_constant_const_prefix(), token);
                }
                self.cursor.advance();
                if self.cursor.lexeme(token.id).contains('_') {
                    self.listener.handle_literal_double_with_separators(token);
                } else {
                    self.listener.handle_literal_double(token);
                }
                self.parse_selector_chain(token, context)
            }
            TokenKind::String => {
                self.check_constant_pattern_literal(context, self.peek_span());
                let end = self.parse_literal_string();
                self.parse_selector_chain(end, context)
            }
            TokenKind::Keyword(Keyword::True | Keyword::False) => {
                let token = self.peek_span();
                self.check_constant_pattern_literal(context, token);
                self.cursor.advance();
                self.listener.handle_literal_bool(token);
                self.parse_selector_chain(token, context)
            }
            TokenKind::Keyword(Keyword::Null) => {
                let token = self.peek_span();
                self.check_constant_pattern_literal(context, token);
                self.cursor.advance();
                self.listener.handle_literal_null(token);
                self.parse_selector_chain(token, context)
            }
            TokenKind::Hash => {
                let hash = self.peek_span();
                self.check_constant_pattern_literal(context, hash);
                self.cursor.advance();
                self.listener.begin_literal_symbol(hash);
                let end = self.parse_symbol_rest(hash);
                self.parse_selector_chain(end, context)
            }
            TokenKind::Keyword(Keyword::This) => {
                let token = self.peek_span();
                self.cursor.advance();
                self.listener
                    .handle_this_expression(token, IdentifierContext::Expression);
                self.parse_selector_chain(token, context)
            }
            TokenKind::Keyword(Keyword::Super) => {
                let token = self.peek_span();
                self.cursor.advance();
                self.listener
                    .handle_super_expression(token, IdentifierContext::Expression);
                self.parse_selector_chain(token, context)
            }
            TokenKind::Keyword(Keyword::Void) => {
                // `void name() {}` named-function-expression recovery
                // (official parsePrimary `void` arm routes to
                // parseSendOrFunctionLiteral).
                let end = self.parse_send_or_function_literal(IdentifierContext::Expression);
                self.parse_selector_chain(end, context)
            }
            TokenKind::Keyword(Keyword::Const) => {
                let const_keyword = self.peek_span();
                if context == ConstantPatternContext::Explicit {
                    // `const const [...]` (official parsePrimary, duplicate
                    // `const` in a constant pattern).
                    self.report(invalid_constant_pattern_duplicate_const(), const_keyword);
                    // The duplicate-`const` face replaces the operand's own
                    // const-pattern faces (oracle const_patterns 99
                    // `case const const ():` — only the duplicate row, no
                    // empty-record face on the `()`); the record-literal
                    // parse reads the threaded field, so it resets here.
                    self.constant_pattern_ctx = ConstantPatternContext::None;
                }
                let end = self.parse_const_expression();
                self.parse_selector_chain(end, context)
            }
            TokenKind::Keyword(Keyword::Assert) => {
                // Official parsePrimary 'assert' arm (:8176): `assert(...)`
                // parses as an assert expression in constant-pattern
                // position with no parser row (oracle const_patterns 47/68 —
                // `case assert(false):` and `case const assert(false):` are
                // parser-silent; only the semantic layer rejects them).
                let end = self.parse_assert(AssertKind::Expression);
                self.parse_selector_chain(end, context)
            }
            TokenKind::Keyword(Keyword::New) => {
                let token = self.peek_span();
                self.cursor.advance();
                self.listener.begin_new_expression(token);
                self.parse_constructor_reference(ConstructorReferenceContext::New);
                let end = self.parse_pattern_arguments_opt();
                self.listener.end_new_expression(end);
                self.parse_selector_chain(end, context)
            }
            TokenKind::Keyword(Keyword::Switch) if self.features.patterns_enabled => {
                // The official fallthrough runs the full expression engine
                // (`parsePrecedenceExpression`, :11878-11890), so a `switch`
                // expression is a syntactically valid constant-pattern
                // operand; only the semantic layer rejects it.
                let end = self.parse_switch_expression();
                self.parse_selector_chain(end, context)
            }
            TokenKind::OpenParen => {
                let end = self.parse_parenthesized_or_record_literal(None);
                self.parse_selector_chain(end, context)
            }
            TokenKind::OpenSquare | TokenKind::Index => {
                let end = self.parse_list_literal(None);
                self.parse_selector_chain(end, context)
            }
            TokenKind::OpenCurly => {
                let end = self.parse_set_or_map_literal(None);
                self.parse_selector_chain(end, context)
            }
            TokenKind::Lt => {
                // `<T>[...]` / `<T>{...}` literal with type arguments.
                let type_param = self.compute_type_param_or_arg_on_lt(false);
                let _ = type_param.parse_arguments(self);
                let end = match self.peek_kind() {
                    TokenKind::OpenSquare | TokenKind::Index => self.parse_list_literal(None),
                    TokenKind::OpenCurly => self.parse_set_or_map_literal(None),
                    _ => {
                        // Reduced engine does not parse function literals
                        // (milestone #20): report and stop.
                        let at = self.peek_span();
                        self.report(msg::expected_but_got("["), at);
                        at
                    }
                };
                self.parse_selector_chain(end, context)
            }
            TokenKind::Period => {
                // Dot-shorthand head in pattern position: parsePrimaryPattern's
                // fallthrough reaches parsePrecedenceExpression, whose
                // `_isDotShorthand` entry check (:7042-7053) admits `.` +
                // identifier/`new` (shipped 3.10 — the gate lives in the
                // analyzer's listener, which this reports through
                // `report_dot_shorthands_gate`). The
                // head/context events bracket the shorthand (:7146-7198); no
                // `!` capture — in pattern context the bang stays for the
                // pattern loop (null-assert pattern), like every other
                // operand face here. Under a negation the member head reports
                // like the identifier arm (official parsePrimary :8112-8115,
                // which the shorthand route reaches through parsePrimary). An
                // incomplete face (`case .:`) takes the entry recovery
                // (:6539-6551): synthetic identifier + the same event pair.
                //
                // The member parses as a NAME (selector-chain `.` arm's
                // handling), not through parse_primary's send face: the
                // bridge's Send leaf tiles through the unconsumed follower
                // (#585), which would drag the case colon into the operand's
                // token window and break the constant-head face downstream.
                let dot = self.peek_span();
                if context == ConstantPatternContext::NumericLiteralOnly {
                    self.report(invalid_constant_pattern_negation(), dot);
                }
                self.cursor.advance();
                if crate::recovery::is_identifier(self.peek_kind())
                    || self.peek_kind() == TokenKind::Keyword(Keyword::New)
                {
                    let member = if self.peek_kind() == TokenKind::Keyword(Keyword::New) {
                        let span = self.peek_span();
                        self.cursor.advance();
                        self.listener
                            .handle_identifier(span, IdentifierContext::ExpressionContinuation);
                        span
                    } else {
                        self.ensure_identifier(IdentifierContext::ExpressionContinuation)
                    };
                    self.listener.handle_dot_shorthand_head(dot);
                    self.report_dot_shorthands_gate(dot);
                    let end = self.parse_selector_chain(member, context);
                    self.listener.handle_dot_shorthand_context(dot);
                    end
                } else {
                    let end = self.ensure_identifier(IdentifierContext::ExpressionContinuation);
                    self.listener.handle_dot_shorthand_head(dot);
                    self.report_dot_shorthands_gate(dot);
                    self.listener.handle_dot_shorthand_context(dot);
                    self.parse_selector_chain(end, context)
                }
            }
            kind if crate::recovery::is_identifier(kind) => {
                let begin = self.peek_span();
                if context == ConstantPatternContext::NumericLiteralOnly {
                    // Negated identifier is not a constant pattern
                    // (official parsePrimary identifier arm).
                    self.report(invalid_constant_pattern_negation(), begin);
                }
                let end = self.parse_pattern_send(begin, context);
                self.parse_selector_chain(end, context)
            }
            _ => {
                // Official parsePrimary's default arm routes every other
                // head through parseSend — the name's ensureIdentifier
                // recovery (expectedIdentifier family) reports and either
                // consumes the keyword or makes way for a synthetic.
                let end = self.parse_send(IdentifierContext::Expression);
                self.parse_selector_chain(end, context)
            }
        }
    }

    /// Postfix selector loop (`.`/`?.`/`(`/`[`/`!`/`++`/`--`) — the reduced
    /// `parseArgumentOrIndexStar` (:8004) plus the precedence loop's POSTFIX
    /// arm (:7312-7318). `context` is the live constant-pattern context: a
    /// `.`-access with method type arguments not followed by `(` reports
    /// `invalidConstantPatternGeneric` (`parser_impl.dart:6776-6791`).
    fn parse_selector_chain(
        &mut self,
        head: TokenSpan,
        context: ConstantPatternContext,
    ) -> TokenSpan {
        let mut end = head;
        loop {
            match self.peek_kind() {
                TokenKind::Period | TokenKind::QuestionPeriod => {
                    let is_null_aware = self.peek_kind() == TokenKind::QuestionPeriod;
                    self.cursor.advance();
                    // `_tryRewriteNewToIdentifier` (parser_impl.dart:4105):
                    // in a continuation context `new` after `.` names the
                    // unnamed constructor and is rewritten to an identifier
                    // with no report.
                    let name = if self.peek_kind() == TokenKind::Keyword(Keyword::New) {
                        let span = self.peek_span();
                        self.cursor.advance();
                        self.listener
                            .handle_identifier(span, IdentifierContext::ExpressionContinuation);
                        span
                    } else {
                        self.ensure_identifier(IdentifierContext::ExpressionContinuation)
                    };
                    self.listener.handle_dot_access(name, name, is_null_aware);
                    end = name;
                    // Official parsePrimary's identifier arm (:8112-8115): a
                    // member under a numeric-literal-only negation reports
                    // `invalidConstantPatternNegation` — and the official
                    // "reset" there is a no-op `==` comparison, so a dotted
                    // chain reports per member (oracle const_patterns 42
                    // `case -prefix.value:` — rows at `prefix` AND `value`).
                    if context == ConstantPatternContext::NumericLiteralOnly {
                        self.report(invalid_constant_pattern_negation(), name);
                    }
                    // Method type arguments after the name, optionally behind
                    // a `!` (`e.f!<T>(c)`); a lone `!` stays for the postfix
                    // arm below.
                    let mut member_type_args = false;
                    if self.peek_kind() == TokenKind::Bang
                        && self.cursor.look_kind(1) == TokenKind::Lt
                    {
                        let bang = self.peek_span();
                        self.listener.handle_non_null_assert_expression(bang);
                        self.cursor.advance();
                    }
                    if self.peek_kind() == TokenKind::Lt {
                        let lt = self.peek_span();
                        let type_param = self.compute_type_param_or_arg_on_lt(false);
                        if !matches!(type_param, crate::types::TypeParamOrArg::None) {
                            type_param.parse_arguments(self);
                            member_type_args = true;
                            if self.peek_kind() != TokenKind::OpenParen {
                                if context != ConstantPatternContext::None {
                                    self.report(invalid_constant_pattern_generic(), lt);
                                }
                                self.listener.handle_type_argument_application(lt);
                                end = self.span_of(
                                    self.cursor
                                        .last_consumed_id()
                                        .expect("type arguments parse before the end anchor"),
                                );
                            }
                        }
                    }
                    // Official parseSend's explicit-const gate (:9413-9423):
                    // with the context still explicit, only `.`/`(`/`<` can
                    // complete a const constructor invocation — any other
                    // follower reports once at the member head (oracle
                    // const_patterns 63 `case const prefix.value:` → row at
                    // `value`; 65 `case const prefix.Class.named:` → row at
                    // `named`). The gate reads `token.next` BEFORE any type
                    // arguments: a parsed `<...>` group keeps it silent (the
                    // generic tear-off report above is that face — oracle
                    // const_patterns 65prime `prefix.Class<int>` stays one
                    // row).
                    if context == ConstantPatternContext::Explicit
                        && !member_type_args
                        && !matches!(
                            self.peek_kind(),
                            TokenKind::Period | TokenKind::OpenParen | TokenKind::Lt
                        )
                    {
                        // The official reset is parseSend-local (a by-value
                        // parameter), so the postfix loop's context — this
                        // `context` — stays explicit for later members.
                        self.report(msg::invalid_constant_const_prefix(), name);
                    }
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
                TokenKind::PlusPlus | TokenKind::MinusMinus => {
                    // Official precedence-loop POSTFIX arm (:7312-7318):
                    // postfix `++`/`--` consume silently in any
                    // constant-pattern context (oracle const_patterns_binary
                    // 86/87 — `case o++:`/`case o--:` are parser-silent).
                    let operator = self.peek_span();
                    self.cursor.advance();
                    self.listener
                        .handle_unary_postfix_assignment_expression(operator);
                    end = operator;
                }
                // Postfix `!` is an expression operator only outside constant
                // patterns; in pattern context it stays for the pattern loop
                // (null-assert pattern, parser_impl.dart:11618-11626).
                TokenKind::Bang if context == ConstantPatternContext::None => {
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
    fn parse_pattern_send(
        &mut self,
        begin: TokenSpan,
        context: ConstantPatternContext,
    ) -> TokenSpan {
        self.cursor.advance();
        self.listener
            .handle_identifier(begin, IdentifierContext::Expression);
        // The name's `<...>` group has three official outcomes
        // (`parseUnaryExpression` :7995-8013 + `parseSend` :9396-9412):
        // `.` + (`new`|identifier) + `(` routes to the constructor-reference
        // form (`C<int>.new(...)`), a direct `(` keeps the group for the
        // invocation, and anything else leaves `<` in the stream for the
        // precedence level (the generic tear-off `C<int>`).
        if self.peek_kind() == TokenKind::Lt {
            let candidate = self.compute_type_param_or_arg_on_lt(false);
            if !matches!(candidate, crate::types::TypeParamOrArg::None) {
                let follower = self.type_arg_follower(&candidate, self.cursor.real_pos());
                if follower == TokenKind::Period {
                    // `parseImplicitCreationExpression` (:9009) →
                    // `parseConstructorReference` (:6414) with the
                    // precomputed group: type arguments, then the
                    // constructor name (`new` reads as the identifier via
                    // the parser-level `ensureIdentifier` rewrite
                    // :4076-4083), then the invocation arguments. The
                    // probe's `.` can never be a compound-closer remainder,
                    // so the tokens past the group sit at fixed offsets.
                    let after = candidate.skip(begin.id);
                    if is_new_or_identifier(self.kind_at(self.offset_of(after) + 2))
                        && self.kind_at(self.offset_of(after) + 3) == TokenKind::OpenParen
                    {
                        let open_angle = self.peek_span();
                        self.listener.begin_implicit_creation_expression(begin);
                        let _ = candidate.parse_arguments(self);
                        self.cursor.advance(); // `.`
                        let _ = self.ensure_identifier(
                            IdentifierContext::ConstructorReferenceContinuationAfterTypeArguments,
                        );
                        let end = self.parse_arguments();
                        self.listener
                            .end_implicit_creation_expression(begin, open_angle);
                        return end;
                    }
                } else if follower == TokenKind::OpenParen && !candidate.recovered() {
                    let _ = candidate.parse_arguments(self);
                    return self.parse_arguments();
                }
            }
        }
        // The explicit-const gate (:9413-9423): only `.`/`(`/`<` can still
        // complete a const constructor invocation; any other follower
        // reports the const-prefix error once (a bare `const value`).
        if context == ConstantPatternContext::Explicit
            && !matches!(
                self.peek_kind(),
                TokenKind::Period | TokenKind::OpenParen | TokenKind::Lt
            )
        {
            let at = self.span_of(
                self.cursor
                    .last_consumed_id()
                    .expect("constant-pattern send parses after its head identifier"),
            );
            self.report(msg::invalid_constant_const_prefix(), at);
        }
        let next = self.peek_span();
        self.listener
            .handle_send_without_arguments(begin, begin, next);
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
            let named = self.peek_is_identifier() && self.cursor.look_kind(1) == TokenKind::Colon;
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

    /// Optional argument list (`parseArgumentsOpt` :9494). The arguments
    /// themselves parse through the full engine (official
    /// `parseConstExpression` → `parseConstructorInvocationArguments` →
    /// `parseArgumentsRest`).
    fn parse_pattern_arguments_opt(&mut self) -> TokenSpan {
        if self.peek_kind() == TokenKind::OpenParen {
            return self.parse_arguments_rest();
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
        // Official order (:6414-6446): identifier, qualified rest, type
        // arguments, THEN the constructor name (`C<int>.new` — the `.name`
        // comes after the type arguments).
        self.ensure_identifier(IdentifierContext::ConstructorReference);
        self.parse_qualified_rest_opt(IdentifierContext::ConstructorReferenceContinuation);
        if self.peek_kind() == TokenKind::Lt {
            let type_param = self.compute_type_param_or_arg_on_lt(false);
            type_param.parse_arguments(self);
        }
        let mut period_before_name = None;
        if self.peek_kind() == TokenKind::Period {
            let period = self.peek_span();
            self.cursor.advance();
            let second = self.ensure_identifier(
                IdentifierContext::ConstructorReferenceContinuationAfterTypeArguments,
            );
            let _ = second;
            period_before_name = Some(period);
        } else {
            // Official :6430-6436 — the else of the period check fires the
            // no-continuation event for EVERY period-less reference, type
            // arguments or not: `C<int>(...)` (no `.name`) and `C.new(...)`
            // (whose `.new` rode the qualified rest) both fire it; `C.new`
            // leaves `period` null because `parseQualifiedRestOpt` consumed
            // the dot before this check.
            self.listener
                .handle_no_constructor_reference_continuation_after_type_arguments(start);
        }
        let end = self.peek_span();
        self.listener
            .end_constructor_reference(start, period_before_name, end, context);
        end
    }

    /// Token following the `<...>` type-argument group starting at table
    /// offset `lt_offset` (official `typeArg.skip(token).next!`). With
    /// [`crate::types::TypeParamOrArg::None`] there is no group, so the token
    /// at `lt_offset` (the `<` itself) follows. Combined closers
    /// (`>>`/`>=`) split on consume: a raw compound leaves the split
    /// remainder as the follower, while a compound shared with enclosing
    /// groups (its last `>` closes this group) is fully consumed and the
    /// real successor follows (official `skipEnd`, nested
    /// `Map<String, List<int>>{}` fires the literal-with-class recovery).
    pub(crate) fn type_arg_follower(
        &self,
        type_param: &crate::types::TypeParamOrArg,
        lt_offset: usize,
    ) -> TokenKind {
        match type_param {
            crate::types::TypeParamOrArg::None => self.kind_at(lt_offset),
            crate::types::TypeParamOrArg::Simple1 { closer } => match closer {
                crate::types::SimpleCloser::Gt => self.kind_at(lt_offset + 3),
                // `<id>=` / `<id>>`: the remainder token follows the group.
                crate::types::SimpleCloser::GtEq => TokenKind::Eq,
                crate::types::SimpleCloser::GtGt => TokenKind::Gt,
            },
            crate::types::TypeParamOrArg::Complex(g) => {
                let end = self.offset_of(g.end);
                if g.end_raw_compound {
                    match crate::cursor::split_remainders(self.kind_at(end)).first() {
                        Some((kind, _)) => *kind,
                        None => self.kind_at(end + 1),
                    }
                } else {
                    self.kind_at(end + 1)
                }
            }
        }
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
                // Special-case recovery (official :8418-8490): `const
                // Map<...>{...}`, `const Set<...>{...}` and `const
                // List<...>[...]` are collection literals with a bogus class
                // prefix. Report `literalWithClass`, drop the prefix, and
                // parse the literal.
                let class = match self.cursor.lexeme(self.peek_id()) {
                    "Map" => Some("map"),
                    "Set" => Some("set"),
                    "List" => Some("list"),
                    _ => None,
                };
                if let Some(class) = class
                    && self.cursor.look_kind(1) != TokenKind::Period
                {
                    // The token after the (optional) type-argument group must
                    // open the collection.
                    let type_param = self.compute_type_param_or_arg_at(1, false);
                    let follower = self.type_arg_follower(&type_param, self.cursor.real_pos() + 1);
                    let opens = match class {
                        "list" => matches!(follower, TokenKind::OpenSquare | TokenKind::Index),
                        _ => follower == TokenKind::OpenCurly,
                    };
                    if opens {
                        self.report(
                            msg::literal_with_class(class, self.cursor.lexeme(self.peek_id())),
                            self.peek_span(),
                        );
                        let end = if matches!(type_param, crate::types::TypeParamOrArg::None) {
                            // `const Map{...}`: no group; the opener follows
                            // the class name directly.
                            self.cursor.advance();
                            self.listener.begin_const_literal(self.peek_span());
                            self.listener.handle_no_type_arguments(self.peek_span());
                            match class {
                                "list" => self.parse_list_literal(Some(const_keyword)),
                                _ => self.parse_set_or_map_literal(Some(const_keyword)),
                            }
                        } else {
                            // `const Map<...>{...}`: type args then the
                            // collection (reduced `parseLiteralListSetMapOrFunction`).
                            self.cursor.advance();
                            self.listener.begin_const_literal(self.peek_span());
                            let _ = type_param.parse_arguments(self);
                            match self.peek_kind() {
                                TokenKind::OpenSquare | TokenKind::Index => {
                                    self.parse_list_literal(Some(const_keyword))
                                }
                                TokenKind::OpenCurly => {
                                    self.parse_set_or_map_literal(Some(const_keyword))
                                }
                                _ => {
                                    let at = self.peek_span();
                                    self.report(msg::expected_but_got("["), at);
                                    at
                                }
                            }
                        };
                        self.listener.end_const_literal(end);
                        return end;
                    }
                }
                // Handling const dot shorthands (official :8499-8519):
                // `const .new(1)` and friends.
                if self.peek_kind() == TokenKind::Period {
                    self.listener.begin_const_dot_shorthand(const_keyword);
                    let end = if self.cursor.look_kind(1) == TokenKind::Keyword(Keyword::New)
                        || crate::recovery::is_identifier(self.cursor.look_kind(1))
                    {
                        let dot = self.peek_span();
                        self.cursor.advance();
                        // The member parses through the primary dispatch so a
                        // `.new` head reads as an identifier (official
                        // `_tryRewriteNewToIdentifier`, :8510).
                        let end = self.parse_primary(IdentifierContext::ExpressionContinuation);
                        self.listener.handle_dot_shorthand_head(dot);
                        self.report_dot_shorthands_gate(dot);
                        self.listener.handle_dot_shorthand_context(dot);
                        end
                    } else {
                        // Recovery: an incomplete dot shorthand like
                        // `C c = const .;`. The pinned 3.13.1 oracle
                        // (trend-w4 gate raw output + probe reproduction,
                        // dot_shorthands/const_no_identifier_recovery.dart)
                        // reports `missing_identifier` on the follower's own
                        // real span (`;` / `}`) and leaves it unconsumed —
                        // the synthetic operand rides at the dot's offset,
                        // so the declaration's `ensureSemicolon` fails
                        // against it and its walked-back anchor lands on the
                        // dot (`Expected to find ';'` at the `.`). The
                        // leftover `;` parses as an empty statement, the
                        // `}` still closes the block.
                        let dot = self.peek_span();
                        self.cursor.advance();
                        let at = self.peek_span();
                        self.report(msg::missing_identifier(), at);
                        let synthetic = self.synthetic(TokenKind::Identifier, dot.range.offset);
                        self.listener.handle_dot_shorthand_head(dot);
                        self.report_dot_shorthands_gate(dot);
                        self.listener.handle_dot_shorthand_context(dot);
                        synthetic
                    };
                    self.report_dot_shorthands_gate(const_keyword);
                    self.listener.end_const_dot_shorthand(const_keyword);
                    return end;
                }
                // `const Foo.bar<int>(...)`.
                self.listener.begin_const_expression(const_keyword);
                self.parse_constructor_reference(ConstructorReferenceContext::Const);
                let end = self.parse_constructor_invocation_arguments(self.last_consumed_span());
                self.listener.end_const_expression(end);
                end
            }
        }
    }
}

/// Shared support for the inline per-area test modules: the crate's main
/// `tests::collector` module is private to the `tests` subtree, which these
/// inline modules are not part of.
#[cfg(test)]
pub(crate) mod test_support {
    use crate::listener::ParserListener;
    use crate::message::Message;
    use crate::token_id::TokenSpan;

    /// Scans `source` and asserts no scanner diagnostics; leaked lifetimes
    /// keep parser signatures simple (same shape as `tests::collector`).
    pub(crate) fn scanned(
        source: &'static str,
    ) -> (&'static dartq_scanner::TokenTable, &'static str) {
        let config = dartq_scanner::ScanConfig {
            enable_triple_shift: true,
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

    /// Latest-version feature set (patterns on).
    pub(crate) fn features() -> crate::ParserFeatures {
        crate::ParserFeatures::from_feature_set(dartq_language::FeatureSet::for_language_version(
            dartq_language::LanguageVersion::new(3, 15),
        ))
    }

    /// Builds a parser over `source` with a fresh `L`.
    ///
    /// The parser borrows its listener, and this fixture hands back the
    /// `'static` shape matching the leaked table/text, so the log is leaked
    /// too: tests read events through the parser and never reclaim it.
    pub(crate) fn parser<L: ParserListener>(
        source: &'static str,
        listener: L,
    ) -> crate::Parser<'static, 'static, 'static, L> {
        let (table, text) = scanned(source);
        crate::Parser::new(text, table, features(), Box::leak(Box::new(listener)))
    }

    /// Builds a parser over `source` borrowing a caller-owned listener —
    /// for fan-out fixtures whose [`crate::MultiplexListener`] borrows
    /// stack locals and so cannot take the leaked `'static` shape.
    pub(crate) fn parser_ref<'l, L: ParserListener>(
        source: &'static str,
        listener: &'l mut L,
    ) -> crate::Parser<'static, 'static, 'l, L> {
        let (table, text) = scanned(source);
        crate::Parser::new(text, table, features(), listener)
    }

    /// Error-code + event log covering the events the inline modules assert.
    #[derive(Default)]
    pub(crate) struct Log {
        errors: Vec<String>,
        events: Vec<(&'static str, &'static str)>,
    }

    impl Log {
        pub(crate) fn error_codes(&self) -> &[String] {
            &self.errors
        }

        pub(crate) fn errors_empty(&self) -> bool {
            self.errors.is_empty()
        }

        pub(crate) fn assert_events(&self, expected: &[(&'static str, &'static str)]) {
            let mut it = self.events.iter();
            for want in expected {
                assert!(
                    it.any(|got| got == want),
                    "expected {want:?} in order; events: {:?} (errors: {:?})",
                    self.events,
                    self.errors
                );
            }
        }
    }

    impl ParserListener for Log {
        fn handle_recoverable_error(&mut self, message: Message, _: TokenSpan, _: TokenSpan) {
            self.errors.push(message.code.name.to_owned());
        }

        fn handle_list_pattern(&mut self, _: usize, _: TokenSpan, _: TokenSpan) {
            self.events.push(("handle", "list_pattern"));
        }

        fn handle_map_pattern(&mut self, _: usize, _: TokenSpan, _: TokenSpan) {
            self.events.push(("handle", "map_pattern"));
        }

        fn handle_record_pattern(&mut self, _: TokenSpan, _: usize) {
            self.events.push(("handle", "record_pattern"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{Log, parser};
    use crate::listener::{IdentifierContext, PatternContext, Token};
    use crate::message::Message;
    use crate::token_id::TokenSpan;

    fn pattern_errors(source: &'static str) -> Vec<String> {
        let mut p = parser(source, Log::default());
        let _ = p.parse_pattern_entry(PatternContext::Matching);
        p.listener.error_codes().to_vec()
    }

    fn expression_errors(source: &'static str) -> Vec<String> {
        let mut p = parser(source, Log::default());
        let _ = p.parse_precedence_floor(crate::expr_const::EQUALITY_PRECEDENCE);
        p.listener.error_codes().to_vec()
    }

    #[test]
    fn is_then_equality_does_not_report_comparison_chain() {
        // F2: the equality-chain check lives in the plain-binary arm only
        // (official :7386-7393) — `x is int == y` is clean outside constant
        // patterns (a pattern context legitimately reports the `is`
        // over-collection like the oracle).
        assert!(expression_errors("x is int == y").is_empty());
    }

    #[test]
    fn as_chain_does_not_report_comparison_chain() {
        // F2: `as` never touches the chain flag; driven with no
        // constant-pattern context so `as` takes the relational arm
        // (a pattern context would exit at `as` for the cast pattern).
        assert!(expression_errors("x as int as double").is_empty());
    }

    #[test]
    fn equality_still_cannot_chain() {
        // F2 keeps the official report for plain chains (`a == b == c`),
        // exactly once.
        assert_eq!(
            expression_errors("a == b == c"),
            vec!["equality_cannot_be_equality_operand".to_string()]
        );
    }

    #[test]
    fn negation_accepts_numeric_literals_only() {
        // F6 (official :7941-7958 + parsePrimary :8110-8167): implicit `-`
        // switches to numeric-literal-only.
        assert!(pattern_errors("-1").is_empty());
        assert_eq!(
            pattern_errors("-x"),
            vec!["invalid_constant_pattern_negation".to_string()]
        );
        assert_eq!(
            pattern_errors("-\"s\""),
            vec!["invalid_constant_pattern_negation".to_string()]
        );
        assert_eq!(
            pattern_errors("-true"),
            vec!["invalid_constant_pattern_negation".to_string()]
        );
    }

    #[test]
    fn bang_and_tilde_report_invalid_constant_pattern_unary() {
        // F6 (official :7928-7939).
        assert_eq!(
            pattern_errors("!x"),
            vec!["invalid_constant_pattern_unary".to_string()]
        );
        assert_eq!(
            pattern_errors("~x"),
            vec!["invalid_constant_pattern_unary".to_string()]
        );
    }

    #[test]
    fn explicit_const_prefix_reports() {
        // F6: explicit `const` over a primary literal reports
        // invalid_constant_const_prefix (official parsePrimary numeric/string
        // arms); a nested `const` reports the duplicate-const code.
        assert_eq!(
            pattern_errors("const -1"),
            vec!["invalid_constant_const_prefix".to_string()]
        );
        assert_eq!(
            pattern_errors("const \"s\""),
            vec!["invalid_constant_const_prefix".to_string()]
        );
        assert_eq!(
            pattern_errors("const 1"),
            vec!["invalid_constant_const_prefix".to_string()]
        );
        assert_eq!(
            pattern_errors("const const []"),
            vec!["invalid_constant_pattern_duplicate_const".to_string()]
        );
    }

    #[test]
    fn selector_type_args_not_followed_by_paren_report_generic() {
        // F7 (official :6776-6791): `e.f<T>` without `(` under a
        // constant-pattern context reports invalid_constant_pattern_generic;
        // without the context it stays silent.
        assert_eq!(
            pattern_errors("Foo.bar<int>"),
            vec!["invalid_constant_pattern_generic".to_string()]
        );
        assert!(expression_errors("Foo.bar<int>").is_empty());
    }

    #[test]
    fn const_map_or_list_class_prefix_reports_literal_with_class() {
        // F8 (official :8418-8490): `const Map<...>{...}` and
        // `const List<...>[...]` report literalWithClass and parse as
        // literals; constructor references stay silent.
        for source in [
            "const Map<int, String>{}",
            "const List<int>[1]",
            "const Set{}",
        ] {
            let mut p = parser(source, Log::default());
            let _ = p.parse_const_expression();
            assert_eq!(p.listener.error_codes(), ["literal_with_class"], "{source}");
        }
        for source in ["const Foo<int>(1)", "const Map.new()"] {
            let mut p = parser(source, Log::default());
            let _ = p.parse_const_expression();
            assert!(p.listener.errors_empty(), "{source}");
        }
    }

    /// Local listener capturing the constructor-reference continuation
    /// face: whether the no-continuation event fired and whether
    /// `end_constructor_reference` carried a `.name` period (the #1328
    /// inverted-gate regression).
    #[derive(Default)]
    struct CtorRefLog {
        errors: Vec<String>,
        no_continuation: usize,
        ends_with_period: usize,
        ends_without_period: usize,
    }

    impl crate::listener::ParserListener for CtorRefLog {
        fn handle_recoverable_error(&mut self, message: Message, _: TokenSpan, _: TokenSpan) {
            self.errors.push(message.code.name.to_owned());
        }

        fn handle_no_constructor_reference_continuation_after_type_arguments(&mut self, _: Token) {
            self.no_continuation += 1;
        }

        fn end_constructor_reference(
            &mut self,
            _: Token,
            period_before_name: Option<Token>,
            _: Token,
            _: crate::listener::ConstructorReferenceContext,
        ) {
            if period_before_name.is_some() {
                self.ends_with_period += 1;
            } else {
                self.ends_without_period += 1;
            }
        }
    }

    #[test]
    fn generic_ctor_ref_named_continuation_takes_name_branch_only() {
        // #1328 (official :6427-6440): the no-continuation event fires only
        // when NO period follows the type arguments. `C<int>.new(...)` takes
        // the name branch — the old `!= (` gate fired the event AND the
        // name, a contradictory pair.
        let mut p = parser("const C<int>.new(1)", CtorRefLog::default());
        let _ = p.parse_const_expression();
        assert_eq!(p.listener.no_continuation, 0);
        assert_eq!(p.listener.ends_with_period, 1);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
    }

    #[test]
    fn generic_ctor_ref_invocation_fires_no_continuation() {
        // #1328: `C<int>(...)` has no period after the type arguments, so
        // the event fires (the old gate skipped it because `(` followed).
        let mut p = parser("const C<int>(1)", CtorRefLog::default());
        let _ = p.parse_const_expression();
        assert_eq!(p.listener.no_continuation, 1);
        assert_eq!(p.listener.ends_without_period, 1);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
    }

    #[test]
    fn non_generic_ctor_ref_faces_unchanged() {
        // Official :6414-6446: the qualified rest consumes `.new` BEFORE the
        // period check, so plain `C.new(...)` carries a null period AND fires
        // the no-continuation event (the else branch is unconditional on type
        // arguments); plain `C(...)` fires it too.
        let mut p = parser("const C.new(1)", CtorRefLog::default());
        let _ = p.parse_const_expression();
        assert_eq!(p.listener.no_continuation, 1);
        assert_eq!(p.listener.ends_without_period, 1);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);

        let mut p = parser("const C(1)", CtorRefLog::default());
        let _ = p.parse_const_expression();
        assert_eq!(p.listener.no_continuation, 1);
        assert_eq!(p.listener.ends_without_period, 1);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
    }

    /// Local listener capturing the dot-shorthand face: head dot offsets,
    /// member identifier offsets, and send spans (the #757 inverted-span
    /// regression).
    #[derive(Default)]
    struct ShorthandLog {
        errors: Vec<String>,
        head_dots: Vec<u32>,
        members: Vec<u32>,
        sends: Vec<(u32, u32)>,
    }

    impl crate::listener::ParserListener for ShorthandLog {
        fn handle_recoverable_error(&mut self, message: Message, _: TokenSpan, _: TokenSpan) {
            self.errors.push(message.code.name.to_owned());
        }

        fn handle_identifier(&mut self, token: Token, _: IdentifierContext) {
            self.members.push(token.range.offset);
        }

        fn handle_send(&mut self, begin: Token, end: Token) {
            self.sends.push((begin.range.offset, end.range.offset));
        }

        fn handle_dot_shorthand_head(&mut self, dot: Token) {
            self.head_dots.push(dot.range.offset);
        }
    }

    #[test]
    fn dot_shorthand_pattern_head_parses_clean() {
        // #757 (official parsePrimaryPattern fallthrough →
        // parsePrecedenceExpression `_isDotShorthand` :7042): `.e` and `.new`
        // heads are constant-pattern operands; no recovery noise (the old
        // parse_send fallthrough spliced a synthetic and reported
        // expected_identifier), and member invocations stay syntactically
        // parseable.
        for source in [".e", ".new", ".e(1)"] {
            let mut p = parser(source, ShorthandLog::default());
            let _ = p.parse_pattern_entry(PatternContext::Matching);
            assert_eq!(p.listener.head_dots, vec![0], "{source}");
            assert!(
                p.listener.errors.is_empty(),
                "{source}: {:?}",
                p.listener.errors
            );
        }
    }

    #[test]
    fn dot_shorthand_pattern_head_member_is_a_name_not_a_send() {
        // The member is an identifier leaf (`handle_identifier`); no Send —
        // the send leaf tiles through the unconsumed follower (#585) and
        // would drag the case colon into the operand's token window.
        let mut p = parser(".e", ShorthandLog::default());
        let _ = p.parse_pattern_entry(PatternContext::Matching);
        assert!(p.listener.sends.is_empty());
        assert_eq!(p.listener.head_dots, vec![0]);
        assert_eq!(p.listener.members, vec![1]);
        assert!(p.listener.errors.is_empty());
    }

    #[test]
    fn incomplete_dot_shorthand_pattern_takes_entry_recovery() {
        // Official :6539-6551: a `.` head with no identifier after reports
        // once through the expressionContinuation template and splices the
        // synthetic name; head/context events still fire.
        let mut p = parser(".", ShorthandLog::default());
        let _ = p.parse_pattern_entry(PatternContext::Matching);
        assert_eq!(p.listener.errors, ["missing_identifier"]);
        assert_eq!(p.listener.head_dots, vec![0]);
        assert!(p.listener.members.is_empty());
    }

    #[test]
    fn negated_dot_shorthand_reports_like_negated_identifier() {
        // Official parsePrimary :8112-8115: under numeric-literal-only the
        // member head reports invalidConstantPatternNegation (the shorthand
        // route reaches parsePrimary's identifier arm).
        assert_eq!(
            pattern_errors("-.e"),
            vec!["invalid_constant_pattern_negation".to_string()]
        );
    }
}
