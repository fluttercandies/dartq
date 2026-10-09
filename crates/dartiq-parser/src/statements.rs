//! Statement grammar (official `parseStatement`/`parseStatementX`
//! `parser_impl.dart:6708`, `parseBlock` :10523, `parseExpressionStatement`
//! :6925, the `parseExpressionStatementOrDeclaration` family :9807-10096,
//! `parseVariablesDeclarationRest` :10096, `parseLabeledStatement` :6899,
//! `parseLocalFunctionDeclaration` via `looksLikeLocalFunction` :9739 and
//! `parseNamedFunctionRest` :6364, and the full-parser `parseFunctionBody`
//! :6537 that replaces the #18 diet-body mode for `{ ... }` bodies).
//!
//! Expression internals parse through the reduced constant-expression engine
//! (`expr_const`) until #20 lands the full precedence engine: assignment
//! operators, conditionals, cascades, function literals, and pattern
//! assignments in expression position are #20 scope. The statement-side
//! dispatch (including the `{`-group + `=` pattern-assignment check) mirrors
//! the official control flow exactly, so #20 replaces the expression entry
//! point without touching this module.
//!
//! Diagnostics follow the pinned-oracle emission table
//! (docs/facts/corpus-expansion-plan.md §7): the eight Group A statement
//! codes are oracle-dead — every recovery path here emits the generic
//! codes the pinned 3.13.1 oracle produces (`expected_token`,
//! `missing_identifier`, `extraneous_modifier`, ...), never a dedicated
//! Group A code.

use dartiq_scanner::{Keyword, KeywordStyle, TokenKind};

use crate::cursor::TokenCursor;
use crate::listener::{
    AssertKind, BlockKind, IdentifierContext, LoopState, ParserListener, PatternContext,
};
use crate::members::is_keyword_or_identifier;
use crate::message::msg;
use crate::parser::{AsyncModifier, MAX_DEPTH, Parser};
use crate::recovery::is_identifier;
use crate::token_id::TokenId;
use crate::token_id::TokenSpan;
use crate::type_args::TypeParamOrArg;
use crate::types::TypeInfo;

/// `AwaitOrYieldContext` (`parser_impl.dart:10621`): the position an
/// `await`/`yield` lookalike was seen in, deciding how greedy the
/// expression-lookahead is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AwaitYieldCtx {
    /// Statement position (`await`/`yield` followed by `;` or `,` can be a
    /// variable declaration).
    Statement,
    /// Unary-expression position.
    UnaryExpression,
}

/// What [`Parser::parse_expression_statement_or_declaration`] decided (the
/// official `ForPartsContext` split between statement and for-parts modes).
enum ExprStmtOrDecl {
    /// Statement mode: a complete statement was parsed.
    Parsed,
    /// For-parts mode: no declaration head; the cursor did not move and the
    /// caller parses an expression (or the empty form) itself.
    NotDecl,
    /// For-parts mode: modifiers and type were consumed; the cursor sits at
    /// the first variable name and the caller finishes with
    /// [`Parser::parse_variables_declaration_rest`] (`end_with_semicolon` =
    /// `false`).
    DeclHead,
    /// For-parts mode: a `var`/`final` outer pattern was parsed through the
    /// pattern grammar; the caller decides `=` vs `in`.
    PatternHead { pattern_keyword: TokenSpan },
}

impl<'s, L: ParserListener> Parser<'s, '_, L> {
    // ------------------------------------------------------------------
    // statement dispatch
    // ------------------------------------------------------------------

    /// Official `parseStatement`: the statement-depth guard (official
    /// `statementDepth`, 500) wraps [`Self::parse_statement_x`].
    pub(crate) fn parse_statement(&mut self) {
        if self.statement_depth >= MAX_DEPTH {
            // Official `recoverFromStackOverflow`: report once and stop.
            self.bail(msg::stack_overflow(MAX_DEPTH));
            return;
        }
        self.statement_depth += 1;
        self.parse_statement_x();
        self.statement_depth -= 1;
    }

    /// Official `parseStatementX` dispatch table.
    fn parse_statement_x(&mut self) {
        if self.is_bailed() {
            return;
        }
        match self.peek_kind() {
            TokenKind::Identifier => {
                if self.cursor.look_kind(1) == TokenKind::Colon {
                    self.parse_labeled_statement();
                } else {
                    self.parse_expression_statement_or_declaration_after_modifiers(
                        None, None, false, false, true,
                    );
                }
            }
            TokenKind::OpenCurly => {
                // The scanner ensures `{` always has a closing `}`; a
                // `{...} =` group is a pattern-assignment expression
                // statement.
                let is_pattern_assignment = self.features.patterns_enabled
                    && self.current_matching_end().is_some_and(|close| {
                        self.kind_at(self.offset_of(close) + 1) == TokenKind::Eq
                    });
                if is_pattern_assignment {
                    let _ = self.parse_expression_statement();
                } else {
                    self.parse_block(BlockKind::Statement, crate::message::BlockKind::Statement);
                }
            }
            TokenKind::Keyword(Keyword::Return) => self.parse_return_statement(),
            TokenKind::Keyword(Keyword::Var | Keyword::Final) => {
                let var_or_final = self.peek_span();
                if !self.is_statement_modifier_at(1) {
                    self.cursor.advance();
                    self.parse_expression_statement_or_declaration_after_modifiers(
                        None,
                        Some(var_or_final),
                        false,
                        false,
                        false,
                    );
                } else {
                    self.parse_expression_statement_or_declaration(false);
                }
            }
            TokenKind::Keyword(Keyword::If) => self.parse_if_statement(),
            TokenKind::Keyword(Keyword::Await)
                if self.cursor.look_kind(1) == TokenKind::Keyword(Keyword::For) =>
            {
                let await_kw = self.peek_span();
                self.cursor.advance();
                self.parse_for_statement(Some(await_kw));
            }
            TokenKind::Keyword(Keyword::For) => self.parse_for_statement(None),
            TokenKind::Keyword(Keyword::Rethrow) => self.parse_rethrow_statement(),
            TokenKind::Keyword(Keyword::While) => self.parse_while_statement(),
            TokenKind::Keyword(Keyword::Do) => self.parse_do_while_statement(),
            TokenKind::Keyword(Keyword::Try) => self.parse_try_statement(),
            TokenKind::Keyword(Keyword::Switch) => self.parse_switch_statement(),
            TokenKind::Keyword(Keyword::Break) => self.parse_break_statement(),
            TokenKind::Keyword(Keyword::Continue) => self.parse_continue_statement(),
            TokenKind::Keyword(Keyword::Assert) => self.parse_assert_statement(),
            TokenKind::Semicolon => {
                let _ = self.parse_empty_statement();
            }
            TokenKind::Keyword(Keyword::Yield) => match self.async_state {
                AsyncModifier::Sync => {
                    if self.cursor.look_kind(1) == TokenKind::Colon {
                        self.parse_labeled_statement();
                    } else if self.looks_like_await_or_yield_expression(AwaitYieldCtx::Statement, 0)
                    {
                        // Recovery: looks like a `yield` statement outside a
                        // generator; parseYieldStatement reports (a
                        // resolution-domain pseudo code — no TSV emission).
                        self.parse_yield_statement();
                    } else {
                        self.parse_expression_statement_or_declaration(false);
                    }
                }
                AsyncModifier::SyncStar | AsyncModifier::AsyncStar | AsyncModifier::Async => {
                    self.parse_yield_statement();
                }
            },
            TokenKind::Keyword(Keyword::Const) => {
                self.parse_expression_statement_or_const_declaration();
            }
            TokenKind::Keyword(Keyword::Await) => {
                if self.async_state == AsyncModifier::Sync
                    && !self.looks_like_await_or_yield_expression(AwaitYieldCtx::Statement, 0)
                {
                    self.parse_expression_statement_or_declaration(false);
                } else {
                    // Fall through to the expression statement; the reduced
                    // await parse reports the sync-context misuse (#20 owns
                    // the full check).
                    self.parse_expression_statement();
                }
            }
            TokenKind::Keyword(Keyword::Set) if is_identifier(self.cursor.look_kind(1)) => {
                // Recovery: invalid use of `set` in statement position.
                let at = self.peek_span();
                self.report(msg::unexpected_token(self.cursor.lexeme(at.id)), at);
                self.force_advance();
                self.parse_statement_x();
            }
            kind if is_identifier(kind) => {
                if self.cursor.look_kind(1) == TokenKind::Colon {
                    self.parse_labeled_statement();
                } else {
                    self.parse_expression_statement_or_declaration(false);
                }
            }
            _ => {
                self.parse_expression_statement_or_declaration(false);
            }
        }
    }

    // ------------------------------------------------------------------
    // blocks
    // ------------------------------------------------------------------

    /// Official `parseBlock`: `ensureBlock`, `begin/endBlock` around the
    /// statement loop with the no-progress recovery.
    pub(crate) fn parse_block(
        &mut self,
        kind: BlockKind,
        message_kind: crate::message::BlockKind,
    ) -> TokenSpan {
        let had_open = self.peek_kind() == TokenKind::OpenCurly;
        let (begin, close) = self.ensure_block(message_kind);
        self.listener.begin_block(begin, kind);
        if !had_open {
            // Synthetic empty block: drain the inserted `{`/`}` pair; the
            // real token that triggered recovery stays for the enclosing
            // loop (official `ensureBlock` shape).
            self.skip_to_token(close.id);
            self.cursor.advance();
            self.listener.end_block(0, begin, close, kind);
            return close;
        }
        let count = self.parse_statements_until_close();
        let end = self.peek_span();
        if !self.is_bailed() && self.peek_kind() == TokenKind::CloseCurly {
            self.cursor.advance();
        }
        self.listener.end_block(count, begin, end, kind);
        end
    }

    /// The shared statement loop of `parseBlock`/`parseFunctionBody`: parse
    /// statements until `}`/EOF/bailout with the official no-progress guard
    /// (report `unexpectedToken` and force-advance). Returns the statement
    /// count.
    fn parse_statements_until_close(&mut self) -> usize {
        let mut count = 0usize;
        while !self.cursor.at_eof()
            && self.peek_kind() != TokenKind::CloseCurly
            && !self.is_bailed()
        {
            let start = self.cursor.peek_id();
            self.parse_statement();
            count += 1;
            if self.cursor.peek_id() == start && !self.is_bailed() {
                // No progress: report the current token and move forward.
                let at = self.peek_span();
                self.report(msg::unexpected_token(self.cursor.lexeme(at.id)), at);
                self.force_advance();
            }
        }
        count
    }

    // ------------------------------------------------------------------
    // expression statements
    // ------------------------------------------------------------------

    /// The statement-layer expression entry: the reduced engine at its
    /// lowest binary floor (`IF_NULL_PRECEDENCE`) until #20 lands the full
    /// `parseExpression`.
    pub(crate) fn parse_expression(&mut self) -> TokenSpan {
        self.parse_precedence_floor(crate::expr_const::IF_NULL_PRECEDENCE)
    }

    /// Official `parseExpressionStatement`: `expression? ';'`.
    pub(crate) fn parse_expression_statement(&mut self) -> TokenSpan {
        let begin = self.peek_span();
        let _ = self.parse_expression();
        let end = self.ensure_semicolon();
        self.listener.handle_expression_statement(begin, end);
        end
    }

    /// Official `parseExpressionStatementOrConstDeclaration`: `const`
    /// disambiguation in statement position.
    fn parse_expression_statement_or_const_declaration(&mut self) {
        if self.is_statement_modifier_at(1) {
            // `const` leads a modifier run: the wrapper walk consumes it.
            self.parse_expression_statement_or_declaration(false);
            return;
        }
        let const_kw = self.peek_span();
        self.cursor.advance();
        {
            let type_info = self.compute_type(false, true);
            if matches!(type_info, TypeInfo::NoType) {
                if !self.peek_is_identifier() {
                    self.parse_expression_statement();
                    return;
                }
                let follower = self.cursor.look_kind(1);
                if !matches!(
                    follower,
                    TokenKind::Eq | TokenKind::Semicolon | TokenKind::Comma | TokenKind::OpenCurly
                ) && !is_keyword_or_identifier(follower)
                {
                    self.parse_expression_statement();
                    return;
                }
            }
            self.parse_expression_statement_or_declaration_after_modifiers(
                None,
                Some(const_kw),
                false,
                false,
                false,
            );
        }
    }

    /// Official `parseExpressionStatementOrDeclaration` (both modes): the
    /// metadata + modifier walk, then the shared after-modifiers decision.
    fn parse_expression_statement_or_declaration(&mut self, for_parts: bool) -> ExprStmtOrDecl {
        let entry = self.cursor.mark();
        let mut metadata_done = false;
        if self.peek_kind() == TokenKind::At {
            self.parse_metadata_star();
            metadata_done = true;
        }

        let mut late_kw: Option<TokenSpan> = None;
        let mut var_final_or_const: Option<TokenSpan> = None;
        if self.is_statement_modifier_at(0) {
            if self.peek_kind() == TokenKind::Keyword(Keyword::Augment)
                && self.cursor.look_kind(1) == TokenKind::Keyword(Keyword::Super)
            {
                // `augment super` is an expression, not a declaration.
                if for_parts {
                    return ExprStmtOrDecl::NotDecl;
                }
                self.parse_expression_statement();
                return ExprStmtOrDecl::Parsed;
            }
            match self.peek_kind() {
                TokenKind::Keyword(Keyword::Var | Keyword::Final | Keyword::Const) => {
                    var_final_or_const = Some(self.peek_span());
                    self.cursor.advance();
                }
                TokenKind::Keyword(Keyword::Late) => {
                    late_kw = Some(self.peek_span());
                    self.cursor.advance();
                    if self.is_statement_modifier_at(0)
                        && matches!(
                            self.peek_kind(),
                            TokenKind::Keyword(Keyword::Var | Keyword::Final)
                        )
                    {
                        var_final_or_const = Some(self.peek_span());
                        self.cursor.advance();
                    }
                }
                _ => {}
            }
            if self.is_statement_modifier_at(0) {
                // Official `parseVariableDeclarationModifiers` recovery:
                // consume the modifier run, then report the declaration-only
                // modifiers as extraneous.
                let mut extraneous: Vec<TokenSpan> = Vec::new();
                let mut seen = [false; 6];
                while self.is_statement_modifier_at(0) {
                    let token = self.peek_span();
                    let extraneous_index = match self.peek_kind() {
                        TokenKind::Keyword(Keyword::Abstract) => Some(0),
                        TokenKind::Keyword(Keyword::Covariant) => Some(1),
                        TokenKind::Keyword(Keyword::External) => Some(2),
                        TokenKind::Keyword(Keyword::Required) => Some(3),
                        TokenKind::Keyword(Keyword::Static) => Some(4),
                        TokenKind::Keyword(Keyword::Augment) => Some(5),
                        _ => None,
                    };
                    match extraneous_index {
                        Some(i) => {
                            if !seen[i] {
                                extraneous.push(token);
                                seen[i] = true;
                            }
                        }
                        None => {
                            if matches!(self.peek_kind(), TokenKind::Keyword(Keyword::Late))
                                && late_kw.is_none()
                            {
                                late_kw = Some(token);
                            } else if matches!(
                                self.peek_kind(),
                                TokenKind::Keyword(Keyword::Var | Keyword::Final | Keyword::Const)
                            ) && var_final_or_const.is_none()
                            {
                                var_final_or_const = Some(token);
                            }
                        }
                    }
                    self.cursor.advance();
                }
                for token in extraneous {
                    self.report(
                        msg::extraneous_modifier(self.cursor.lexeme(token.id)),
                        token,
                    );
                }
            }
        }

        let no_modifiers = self.cursor.progress() == TokenCursor::progress_at(entry);
        self.parse_expression_statement_or_declaration_after_modifiers(
            late_kw,
            var_final_or_const,
            metadata_done,
            for_parts,
            no_modifiers,
        )
    }

    /// Official `parseExpressionStatementOrDeclarationAfterModifiers`.
    #[allow(clippy::too_many_arguments)]
    fn parse_expression_statement_or_declaration_after_modifiers(
        &mut self,
        late_kw: Option<TokenSpan>,
        var_final_or_const: Option<TokenSpan>,
        metadata_done: bool,
        for_parts: bool,
        no_modifiers: bool,
    ) -> ExprStmtOrDecl {
        let before_type = self.cursor.mark();

        // Pattern variable declarations (`var (a, b) = ...`, `final [a] = ...`).
        if self.features.patterns_enabled
            && let Some(vfc) = var_final_or_const
            && matches!(
                self.cursor.resolve(vfc.id).0,
                TokenKind::Keyword(Keyword::Var | Keyword::Final)
            )
            && let Some(after_pattern) = self.skip_outer_pattern_id()
        {
            let after_kind = self.kind_at(self.offset_of(after_pattern) + 1);
            if after_kind == TokenKind::Eq
                || (for_parts && after_kind == TokenKind::Keyword(Keyword::In))
            {
                if let Some(late) = late_kw {
                    self.report(msg::late_pattern_variable_declaration(), late);
                }
                if !metadata_done {
                    self.parse_metadata_star();
                }
                if for_parts {
                    return ExprStmtOrDecl::PatternHead {
                        pattern_keyword: vfc,
                    };
                }
                self.parse_pattern(PatternContext::Declaration);
                let equals = self.peek_span();
                self.cursor.advance();
                let _ = self.parse_expression();
                let semicolon = self.ensure_semicolon();
                self.listener
                    .handle_pattern_variable_declaration_statement(vfc, equals, semicolon);
                return ExprStmtOrDecl::Parsed;
            }
        }

        let type_info = self.compute_type(false, true);
        type_info.skip_type_raw(self);

        if for_parts && let Some(late) = late_kw {
            self.report(msg::extraneous_modifier("late"), late);
        }
        if !for_parts && self.looks_like_local_function() {
            // Local function declaration.
            if let Some(vfc) = var_final_or_const {
                self.report(msg::extraneous_modifier(self.cursor.lexeme(vfc.id)), vfc);
            } else if let Some(late) = late_kw {
                self.report(msg::extraneous_modifier("late"), late);
            }
            if !metadata_done {
                self.parse_metadata_star();
            }
            let start_next = self.span_of(self.real_id(TokenCursor::mark_pos(before_type)));
            let before_formals = self.compute_type_param_or_arg(true);
            let _ = before_formals.parse_variables(self, false);
            let after_type_params = self.cursor.mark();
            self.listener.begin_local_function_declaration(start_next);
            self.cursor.reset(before_type);
            let _ = type_info.parse_type(self);
            self.cursor.reset(after_type_params);
            self.parse_named_function_rest();
            return ExprStmtOrDecl::Parsed;
        }

        // `X?` vs conditional-expression disambiguation (official branch at
        // `parseExpressionStatementOrDeclarationAfterModifiers`, gated on
        // `beforeType == start`): with no modifiers and a nullable
        // could-be-expression type, decide between `X? y;` and `a ? b : ...`.
        // The official `=`-arm speculation needs the #20 cascade engine, so
        // that arm keeps the type.
        if no_modifiers && !for_parts && matches!(type_info, TypeInfo::SimpleNullable) {
            if !self.peek_is_identifier() {
                let next = self.peek_span();
                self.report(msg::expected_identifier(self.cursor.lexeme(next.id)), next);
                let id = self
                    .cursor
                    .insert_synthetic(TokenKind::Identifier, next.range.offset);
                self.cursor.advance();
                let _ = id;
            }
            let after_identifier = self.cursor.look_kind(1);
            if !matches!(
                after_identifier,
                TokenKind::Semicolon | TokenKind::Comma | TokenKind::CloseParen | TokenKind::Eof
            ) && !matches!(after_identifier, TokenKind::Keyword(_))
            {
                // Looks like part of a conditional expression: drop the type
                // and parse the statement from its start.
                self.cursor.reset(before_type);
                self.parse_expression_statement();
                return ExprStmtOrDecl::Parsed;
            }
        }

        if self.cursor.progress() == TokenCursor::progress_at(before_type) {
            // No annotation, modifier, or type: an expression statement (or
            // nothing, in for-parts mode).
            if for_parts {
                return ExprStmtOrDecl::NotDecl;
            }
            self.parse_expression_statement();
            return ExprStmtOrDecl::Parsed;
        }

        // Built-in `as`/`is` leading an expression (`as foo;`).
        if matches!(
            self.peek_kind(),
            TokenKind::Keyword(Keyword::As | Keyword::Is)
        ) {
            let follower = self.cursor.look_kind(1);
            if !matches!(
                follower,
                TokenKind::Eq | TokenKind::Semicolon | TokenKind::Comma
            ) {
                if for_parts && follower != TokenKind::Keyword(Keyword::In) {
                    return ExprStmtOrDecl::NotDecl;
                }
                if !for_parts {
                    self.parse_expression_statement();
                    return ExprStmtOrDecl::Parsed;
                }
            }
        }

        if self.peek_is_identifier() {
            if let Some(vfc) = var_final_or_const {
                if self.cursor.resolve(vfc.id).0 == TokenKind::Keyword(Keyword::Var)
                    && !matches!(type_info, TypeInfo::NoType)
                {
                    self.report(msg::type_after_var(), vfc);
                }
            } else if matches!(type_info, TypeInfo::NoType) {
                self.report(msg::missing_const_final_var_or_type(), self.peek_span());
            }
        }

        if !metadata_done {
            self.parse_metadata_star();
        }
        // Re-enter at the type start for the event-bearing parse (the cursor
        // sits after `skipType`; official `typeInfo.parseType(beforeType)`).
        self.cursor.reset(before_type);
        // `final <int> foo = ...`: missing type name before type arguments.
        let missing_type_name = self.peek_kind() == TokenKind::Lt
            && self.cursor.progress() == TokenCursor::progress_at(before_type);
        if missing_type_name {
            let at = self.peek_span();
            self.report(msg::expected_identifier(self.cursor.lexeme(at.id)), at);
            // The synthetic name queues ahead of `<`; the required type
            // compute below sees `id <...>`.
            let id = self
                .cursor
                .insert_synthetic(TokenKind::Identifier, at.range.offset);
            let _ = id;
        }
        let type_info = if missing_type_name {
            self.compute_type(true, true)
        } else {
            type_info
        };
        let _ = type_info.parse_type(self);
        let name = self.peek_span();
        self.listener
            .begin_variables_declaration(name, late_kw, var_final_or_const);
        if for_parts {
            return ExprStmtOrDecl::DeclHead;
        }
        self.parse_variables_declaration_rest(true);
        ExprStmtOrDecl::Parsed
    }

    /// Official `parseVariablesDeclarationRest`.
    pub(crate) fn parse_variables_declaration_rest(
        &mut self,
        end_with_semicolon: bool,
    ) -> TokenSpan {
        let mut count = 1usize;
        self.parse_optionally_initialized_identifier();
        while self.peek_kind() == TokenKind::Comma {
            self.cursor.advance();
            self.parse_optionally_initialized_identifier();
            count += 1;
        }
        if end_with_semicolon {
            let semicolon = self.ensure_semicolon();
            self.listener
                .end_variables_declaration(count, Some(semicolon));
            semicolon
        } else {
            let end = self.peek_span();
            self.listener.end_variables_declaration(count, None);
            end
        }
    }

    /// Official `parseOptionallyInitializedIdentifier`.
    fn parse_optionally_initialized_identifier(&mut self) {
        let name = self.ensure_identifier(IdentifierContext::LocalVariableDeclaration);
        self.listener.begin_initialized_identifier(name);
        self.parse_variable_initializer_opt();
        self.listener.end_initialized_identifier(name);
    }

    /// Official `parseVariableInitializerOpt`.
    fn parse_variable_initializer_opt(&mut self) {
        if self.peek_kind() != TokenKind::Eq {
            let next = self.peek_span();
            self.listener.handle_no_variable_initializer(next);
            return;
        }
        let assignment = self.peek_span();
        self.cursor.advance();
        self.listener.begin_variable_initializer(assignment);
        let _ = self.parse_expression();
        self.listener.end_variable_initializer(assignment);
    }

    // ------------------------------------------------------------------
    // local functions
    // ------------------------------------------------------------------

    /// Official `parseNamedFunctionRest` (statement flavor): name, formals,
    /// (recovery) initializers, async modifier, body.
    fn parse_named_function_rest(&mut self) {
        let begin = self.peek_span();
        self.listener.begin_function_name(begin);
        let name = self.ensure_identifier(IdentifierContext::LocalFunctionDeclaration);
        let _ = name;
        let name_end = self.peek_span();
        self.listener.end_function_name(begin, name_end, false);
        let _ = self.parse_formal_parameters_required_opt(crate::listener::MemberKind::Local);
        let _ = self.parse_initializers_opt();
        let saved_async = self.async_state;
        let _ = self.parse_async_modifier_opt();
        let end = self.parse_function_body(false);
        self.async_state = saved_async;
        self.listener.end_local_function_declaration(end);
    }

    /// Official `looksLikeLocalFunction` (`parser_impl.dart:9739`).
    fn looks_like_local_function(&mut self) -> bool {
        if !self.peek_is_identifier() {
            return false;
        }
        let mut after_name = 1usize;
        if self.cursor.look_kind(1) == TokenKind::Lt {
            let type_param = self.compute_type_param_or_arg(true);
            if matches!(type_param, TypeParamOrArg::None) {
                return false;
            }
            let after = type_param.skip(self.cursor.peek_id());
            after_name = self.offset_of(after) - self.cursor.real_pos() + 1;
        }
        let paren = self.cursor.look_kind(after_name);
        if paren == TokenKind::OpenParen {
            let paren_id = self.real_id(self.cursor.real_pos() + after_name);
            let Some(close) = self.cursor.table_matching_end(paren_id) else {
                return false;
            };
            let after = self.kind_at(self.offset_of(close) + 1);
            return matches!(
                after,
                TokenKind::OpenCurly
                    | TokenKind::Function
                    | TokenKind::Keyword(Keyword::Async | Keyword::Sync)
            );
        }
        if paren == TokenKind::Function {
            // Recovery: local function missing its parameter list.
            return true;
        }
        false
    }

    // ------------------------------------------------------------------
    // labels
    // ------------------------------------------------------------------

    /// Official `parseLabel`: `identifier ':'` with `handleLabel`.
    fn parse_label(&mut self) {
        let _ = self.ensure_identifier(IdentifierContext::LabelDeclaration);
        let colon = self.expect_kind(TokenKind::Colon);
        self.listener.handle_label(colon);
    }

    /// Official `parseLabeledStatement`: `label* nonLabelledStatement`.
    fn parse_labeled_statement(&mut self) {
        let mut label_count = 0usize;
        loop {
            self.parse_label();
            label_count += 1;
            if !(is_identifier(self.peek_kind()) && self.cursor.look_kind(1) == TokenKind::Colon) {
                break;
            }
        }
        let begin = self.peek_span();
        self.listener.begin_labeled_statement(begin, label_count);
        self.parse_statement();
        self.listener.end_labeled_statement(label_count);
    }

    /// Official `peekPastLabels`: table offset past `id ':'` label pairs.
    fn peek_past_labels_pos(&self, pos: usize) -> usize {
        let mut pos = pos;
        while is_identifier(self.kind_at(pos)) && self.kind_at(pos + 1) == TokenKind::Colon {
            pos += 2;
        }
        pos
    }

    // ------------------------------------------------------------------
    // if / while / do
    // ------------------------------------------------------------------

    /// Official `ensureParenthesizedCondition` +
    /// `parseExpressionInParenthesisRest`.
    fn ensure_parenthesized_condition(&mut self, allow_case: bool) {
        let open = self.peek_span();
        if self.peek_kind() != TokenKind::OpenParen {
            // Recovery: `insertParens(includeIdentifier: false)`.
            self.report(msg::expected_but_got("("), open);
            let offset = open.range.offset;
            self.synthetic(TokenKind::OpenParen, offset);
            self.cursor.advance();
            self.synthetic(TokenKind::CloseParen, offset);
            self.cursor.advance();
            let close = self.peek_span();
            let _ = close;
            self.listener
                .handle_parenthesized_condition(open, None, None);
            return;
        }
        self.cursor.advance(); // `(`
        let _ = self.parse_expression();
        if allow_case && self.peek_kind() == TokenKind::Keyword(Keyword::Case) {
            let case_kw = self.peek_span();
            self.cursor.advance();
            let _ = self.parse_pattern(PatternContext::Matching);
            let mut when = None;
            if self.peek_kind() == TokenKind::Keyword(Keyword::When) {
                let when_kw = self.peek_span();
                self.cursor.advance();
                self.listener.begin_pattern_guard(when_kw);
                let _ = self.parse_expression();
                self.listener.end_pattern_guard(self.peek_span());
                when = Some(when_kw);
            }
            self.ensure_close_paren(open);
            self.listener
                .handle_parenthesized_condition(open, Some(case_kw), when);
        } else {
            self.ensure_close_paren(open);
            self.listener
                .handle_parenthesized_condition(open, None, None);
        }
    }

    /// Official `parseIfStatement`.
    fn parse_if_statement(&mut self) {
        let if_kw = self.peek_span();
        self.cursor.advance();
        self.listener.begin_if_statement(if_kw);
        self.ensure_parenthesized_condition(self.features.patterns_enabled);
        let then_begin = self.peek_span();
        self.listener.begin_then_statement(then_begin);
        self.parse_statement();
        self.listener
            .end_then_statement(then_begin, self.peek_span());
        let mut else_kw = None;
        if self.peek_kind() == TokenKind::Keyword(Keyword::Else) {
            let else_token = self.peek_span();
            self.cursor.advance();
            else_kw = Some(else_token);
            self.listener.begin_else_statement(else_token);
            self.parse_statement();
            self.listener
                .end_else_statement(else_token, self.peek_span());
        }
        self.listener
            .end_if_statement(if_kw, else_kw, self.peek_span());
    }

    /// Official `parseWhileStatement`.
    fn parse_while_statement(&mut self) {
        let while_kw = self.peek_span();
        self.cursor.advance();
        self.listener.begin_while_statement(while_kw);
        self.ensure_parenthesized_condition(false);
        let body_begin = self.peek_span();
        self.listener.begin_while_statement_body(body_begin);
        let saved = self.loop_state;
        self.loop_state = LoopState::InsideLoop;
        self.parse_statement();
        self.loop_state = saved;
        let end = self.peek_span();
        self.listener.end_while_statement_body(end);
        self.listener.end_while_statement(while_kw, end);
    }

    /// Official `parseDoWhileStatement`.
    fn parse_do_while_statement(&mut self) {
        let do_kw = self.peek_span();
        self.cursor.advance();
        self.listener.begin_do_while_statement(do_kw);
        self.listener
            .begin_do_while_statement_body(self.peek_span());
        let saved = self.loop_state;
        self.loop_state = LoopState::InsideLoop;
        self.parse_statement();
        self.loop_state = saved;
        self.listener.end_do_while_statement_body(self.peek_span());
        let while_kw = if self.peek_kind() == TokenKind::Keyword(Keyword::While) {
            let kw = self.peek_span();
            self.cursor.advance();
            kw
        } else {
            let at = self.peek_span();
            self.report(msg::expected_but_got("while"), at);
            let synthetic = self.synthetic(TokenKind::Keyword(Keyword::While), at.range.offset);
            self.cursor.advance();
            synthetic
        };
        self.ensure_parenthesized_condition(false);
        let end = self.ensure_semicolon();
        self.listener.end_do_while_statement(do_kw, while_kw, end);
    }

    // ------------------------------------------------------------------
    // for / for-in
    // ------------------------------------------------------------------

    /// Official `parseForStatement`.
    fn parse_for_statement(&mut self, await_kw: Option<TokenSpan>) {
        let for_kw = self.peek_span();
        self.cursor.advance();
        self.listener.begin_for_statement(for_kw);
        let left_paren = if self.peek_kind() == TokenKind::OpenParen {
            self.peek_span()
        } else {
            // Official `parseForLoopPartsStart` recovery: a fully synthetic
            // `(...)` group plus a synthetic variable declaration.
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
            ExprStmtOrDecl::PatternHead { pattern_keyword } => {
                if self.peek_kind() == TokenKind::Eq {
                    let equals = self.peek_span();
                    self.cursor.advance();
                    let _ = self.parse_expression();
                    self.listener
                        .handle_for_initializer_pattern_variable_assignment(
                            pattern_keyword,
                            equals,
                        );
                    self.parse_for_rest(for_kw, left_paren);
                } else {
                    self.parse_for_in_rest(
                        await_kw,
                        for_kw,
                        left_paren,
                        Some(pattern_keyword),
                        None,
                    );
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
                    self.parse_for_in_rest(
                        await_kw,
                        for_kw,
                        left_paren,
                        None,
                        Some((identifier, identifier_pos)),
                    );
                } else {
                    self.parse_for_rest(for_kw, left_paren);
                }
            }
        }
    }

    /// Official `parseForLoopPartsMid`: completes the initializer part and
    /// checks the separators.
    fn parse_for_loop_parts_mid(&mut self, await_kw: Option<TokenSpan>, outcome: ExprStmtOrDecl) {
        match outcome {
            ExprStmtOrDecl::DeclHead => {
                let end = self.parse_variables_declaration_rest(false);
                let for_in = matches!(
                    self.peek_kind(),
                    TokenKind::Keyword(Keyword::In) | TokenKind::Colon
                );
                self.listener
                    .handle_for_initializer_local_variable_declaration(end, for_in);
            }
            _ if self.peek_kind() == TokenKind::Semicolon => {
                let at = self.peek_span();
                self.listener.handle_for_initializer_empty_statement(at);
            }
            _ => {
                let for_in = matches!(
                    self.peek_kind(),
                    TokenKind::Keyword(Keyword::In) | TokenKind::Colon
                ) || (await_kw.is_some() && self.peek_kind() == TokenKind::CloseParen);
                let end = self.parse_expression();
                self.listener
                    .handle_for_initializer_expression_statement(end, for_in);
            }
        }
        if self.peek_kind() == TokenKind::Semicolon {
            if let Some(await_token) = await_kw {
                self.report(msg::invalid_await_for(), await_token);
            }
        } else if self.peek_kind() != TokenKind::Keyword(Keyword::In) {
            if self.peek_kind() == TokenKind::Colon {
                let at = self.peek_span();
                self.report(msg::colon_in_place_of_in(), at);
            } else if await_kw.is_some() {
                // Official inserts a synthetic `in` ahead of `next`.
                let at = self.peek_span();
                self.report(msg::expected_but_got("in"), at);
                let synthetic = self.synthetic(TokenKind::Keyword(Keyword::In), at.range.offset);
                self.cursor.advance();
                let _ = synthetic;
            }
        }
    }

    /// Official `parseForRest`: `; expression? ';' expressionList?` then the
    /// loop body.
    fn parse_for_rest(&mut self, for_kw: TokenSpan, left_paren: TokenSpan) {
        self.parse_for_loop_parts_rest(for_kw, left_paren);
        let body_begin = self.peek_span();
        self.listener.begin_for_statement_body(body_begin);
        let saved = self.loop_state;
        self.loop_state = LoopState::InsideLoop;
        self.parse_statement();
        self.loop_state = saved;
        let end = self.peek_span();
        self.listener.end_for_statement_body(end);
        self.listener.end_for_statement(end);
    }

    /// Official `parseForLoopPartsRest`.
    fn parse_for_loop_parts_rest(&mut self, for_kw: TokenSpan, left_paren: TokenSpan) {
        let left_separator = self.ensure_semicolon();
        if self.peek_kind() == TokenKind::Semicolon {
            self.parse_empty_statement();
        } else {
            self.parse_expression_statement();
        }
        let right_separator = self.peek_span();
        let mut expression_count = 0usize;
        loop {
            if self.is_bailed() || self.peek_kind() == TokenKind::CloseParen {
                break;
            }
            if self.cursor.at_eof() {
                break;
            }
            let _ = self.parse_expression();
            expression_count += 1;
            if self.peek_kind() != TokenKind::Comma {
                break;
            }
            self.cursor.advance();
        }
        if let Some(close) = self.cursor.table_matching_end(left_paren.id) {
            if self.cursor.peek_id() != close {
                let at = self.peek_span();
                self.report(msg::unexpected_token(self.cursor.lexeme(at.id)), at);
                self.skip_to_token(close);
            }
        }
        self.listener.handle_for_loop_parts(
            for_kw,
            left_paren,
            left_separator,
            right_separator,
            expression_count,
        );
    }

    /// Official `parseForInRest`.
    fn parse_for_in_rest(
        &mut self,
        await_kw: Option<TokenSpan>,
        for_kw: TokenSpan,
        left_paren: TokenSpan,
        pattern_keyword: Option<TokenSpan>,
        identifier: Option<(TokenSpan, usize)>,
    ) {
        self.parse_for_in_loop_parts_rest(
            await_kw,
            for_kw,
            left_paren,
            pattern_keyword,
            identifier,
        );
        let body_begin = self.peek_span();
        self.listener.begin_for_in_body(body_begin);
        let saved = self.loop_state;
        self.loop_state = LoopState::InsideLoop;
        self.parse_statement();
        self.loop_state = saved;
        let end = self.peek_span();
        self.listener.end_for_in_body(end);
        self.listener.end_for_in(end);
    }

    /// Official `parseForInLoopPartsRest`.
    fn parse_for_in_loop_parts_rest(
        &mut self,
        await_kw: Option<TokenSpan>,
        for_kw: TokenSpan,
        left_paren: TokenSpan,
        pattern_keyword: Option<TokenSpan>,
        identifier: Option<(TokenSpan, usize)>,
    ) {
        let in_kw = self.peek_span();
        // `await for` outside an async function surfaces through the
        // resolver (pseudo-shared `asyncForInWrongContext`): events only.
        if let Some((identifier, identifier_pos)) = identifier {
            if !is_identifier(self.cursor.resolve(identifier.id).0) {
                let at = identifier;
                self.report(msg::expected_identifier(self.cursor.lexeme(at.id)), at);
            } else if self.cursor.real_pos() > identifier_pos + 1 {
                // The head consumed more than the lone identifier.
                let follower = self.kind_at(identifier_pos + 1);
                if follower == TokenKind::Eq {
                    let at = self.span_of(self.real_id(identifier_pos + 1));
                    self.report(msg::initialized_variable_in_for_each(), at);
                } else {
                    let at = self.span_of(self.real_id(identifier_pos + 1));
                    self.report(msg::unexpected_token(self.cursor.lexeme(at.id)), at);
                }
            }
        }
        self.cursor.advance(); // `in` (or the recovered `:`)
        let expr_begin = self.peek_span();
        self.listener.begin_for_in_expression(expr_begin);
        let _ = self.parse_expression();
        let end = self.ensure_close_paren(left_paren);
        self.listener.end_for_in_expression(end);
        self.listener.handle_for_in_loop_parts(
            await_kw,
            for_kw,
            left_paren,
            pattern_keyword,
            in_kw,
        );
    }

    // ------------------------------------------------------------------
    // return / yield / break / continue / rethrow / empty
    // ------------------------------------------------------------------

    /// Official `parseReturnStatement`.
    fn parse_return_statement(&mut self) {
        let begin = self.peek_span();
        self.cursor.advance();
        self.listener.begin_return_statement(begin);
        let generator = matches!(
            self.async_state,
            AsyncModifier::SyncStar | AsyncModifier::AsyncStar
        );
        if self.peek_kind() == TokenKind::Semicolon {
            let semicolon = self.peek_span();
            self.listener.end_return_statement(false, begin, semicolon);
            self.cursor.advance();
            if generator {
                // `generatorReturnsValue` is a resolution-domain pseudo code:
                // the structural event only, no TSV emission.
                self.listener.handle_invalid_statement(begin);
            }
            return;
        }
        let _ = self.parse_expression();
        let end = self.ensure_semicolon();
        self.listener.end_return_statement(true, begin, end);
        if generator {
            self.listener.handle_invalid_statement(begin);
        }
    }

    /// Official `parseYieldStatement` (dispatch guarantees the async state
    /// checks; the out-of-generator form ends as an invalid yield without a
    /// TSV emission — resolution-domain pseudo code).
    fn parse_yield_statement(&mut self) {
        let begin = self.peek_span();
        self.cursor.advance();
        self.listener.begin_yield_statement(begin);
        let mut star = None;
        if self.peek_kind() == TokenKind::Star {
            star = Some(self.peek_span());
            self.cursor.advance();
        }
        let _ = self.parse_expression();
        let end = self.ensure_semicolon();
        if matches!(
            self.async_state,
            AsyncModifier::SyncStar | AsyncModifier::AsyncStar
        ) {
            self.listener.end_yield_statement(begin, star, end);
        } else {
            self.listener.end_invalid_yield_statement(begin, star, end);
        }
    }

    /// Official `parseBreakStatement`.
    fn parse_break_statement(&mut self) {
        let break_kw = self.peek_span();
        self.cursor.advance();
        let mut has_target = false;
        if self.peek_is_identifier() {
            let _ = self.ensure_identifier(IdentifierContext::LabelReference);
            has_target = true;
        } else if self.loop_state == LoopState::OutsideLoop {
            self.report(msg::break_outside_of_loop(), break_kw);
        }
        let end = self.ensure_semicolon();
        self.listener
            .handle_break_statement(has_target, break_kw, end);
    }

    /// Official `parseContinueStatement`.
    fn parse_continue_statement(&mut self) {
        let continue_kw = self.peek_span();
        self.cursor.advance();
        let mut has_target = false;
        if self.peek_is_identifier() {
            let _ = self.ensure_identifier(IdentifierContext::LabelReference);
            has_target = true;
            if self.loop_state == LoopState::OutsideLoop {
                self.report(msg::continue_outside_of_loop(), continue_kw);
            }
        } else if self.loop_state != LoopState::InsideLoop {
            let message = if self.loop_state == LoopState::InsideSwitch {
                msg::continue_without_label_in_case()
            } else {
                msg::continue_outside_of_loop()
            };
            self.report(message, continue_kw);
        }
        let end = self.ensure_semicolon();
        self.listener
            .handle_continue_statement(has_target, continue_kw, end);
    }

    /// Official `parseRethrowStatement`.
    fn parse_rethrow_statement(&mut self) {
        let rethrow_kw = self.peek_span();
        self.cursor.advance();
        self.listener.begin_rethrow_statement(rethrow_kw);
        let end = self.ensure_semicolon();
        self.listener.end_rethrow_statement(rethrow_kw, end);
    }

    /// Official `parseEmptyStatement`.
    fn parse_empty_statement(&mut self) -> TokenSpan {
        let token = self.peek_span();
        self.cursor.advance();
        self.listener.handle_empty_statement(token);
        token
    }

    // ------------------------------------------------------------------
    // assert
    // ------------------------------------------------------------------

    /// Official `parseAssertStatement`.
    fn parse_assert_statement(&mut self) {
        self.parse_assert(AssertKind::Statement);
    }

    /// Official `parseAssert` (`assert.dart` kinds; expression/initializer
    /// kinds are #20 call sites).
    pub(crate) fn parse_assert(&mut self, kind: AssertKind) {
        let assert_kw = self.peek_span();
        self.cursor.advance();
        self.listener.begin_assert(assert_kw, kind);
        let open = if self.peek_kind() == TokenKind::OpenParen {
            self.peek_span()
        } else {
            // Recovery: `insertParens(includeIdentifier: true)`.
            let at = self.peek_span();
            self.report(msg::expected_but_got("("), at);
            let offset = at.range.offset;
            let synthetic_open = self.synthetic(TokenKind::OpenParen, offset);
            self.cursor.advance();
            self.synthetic(TokenKind::Identifier, offset);
            self.cursor.advance();
            self.synthetic(TokenKind::CloseParen, offset);
            self.cursor.advance();
            synthetic_open
        };
        self.cursor.advance(); // `(`
        let _ = self.parse_expression();
        let mut comma = None;
        if self.peek_kind() == TokenKind::Comma {
            self.cursor.advance();
            if self.peek_kind() != TokenKind::CloseParen {
                comma = Some(self.peek_span());
                let _ = self.parse_expression();
                if self.peek_kind() == TokenKind::Comma {
                    // Trailing comma is ignored.
                    self.cursor.advance();
                }
            }
        }
        let end = self.ensure_close_paren(open);
        if kind == AssertKind::Statement {
            self.ensure_semicolon();
        }
        self.listener.end_assert(assert_kw, kind, open, comma, end);
    }

    // ------------------------------------------------------------------
    // try
    // ------------------------------------------------------------------

    /// Official `parseTryStatement`: `try block (onPart+ finallyPart? |
    /// finallyPart)`.
    fn parse_try_statement(&mut self) {
        let try_kw = self.peek_span();
        self.cursor.advance();
        self.listener.begin_try_statement(try_kw);
        let mut last_consumed = self.parse_block(
            BlockKind::TryStatement,
            crate::message::BlockKind::TryStatement,
        );
        let mut catch_count = 0usize;
        loop {
            let is_on = self.peek_kind() == TokenKind::Keyword(Keyword::On);
            let is_catch = self.peek_kind() == TokenKind::Keyword(Keyword::Catch);
            if (!is_on && !is_catch) || self.is_bailed() {
                break;
            }
            let mut on_kw = None;
            let mut catch_kw = None;
            let mut comma = None;
            if is_on {
                // The `on` keyword unconditionally starts an on-clause here.
                on_kw = Some(self.peek_span());
                self.listener.begin_catch_clause(self.peek_span());
                self.cursor.advance();
                let type_info = self.compute_type(true, false);
                let _ = type_info.ensure_type_not_void(self);
            }
            if self.peek_kind() == TokenKind::Keyword(Keyword::Catch) {
                if on_kw.is_none() {
                    self.listener.begin_catch_clause(self.peek_span());
                }
                catch_kw = Some(self.peek_span());
                self.cursor.advance();
                let _ = self.parse_catch_parameters(&mut comma);
            }
            self.listener.end_catch_clause(self.peek_span());
            last_consumed = self.parse_block(
                BlockKind::CatchClause,
                crate::message::BlockKind::CatchClause,
            );
            catch_count += 1;
            self.listener.handle_catch_block(on_kw, catch_kw, comma);
        }
        let finally_kw = if self.peek_kind() == TokenKind::Keyword(Keyword::Finally) {
            Some(self.peek_span())
        } else {
            if catch_count == 0 {
                self.report(msg::only_try(), try_kw);
            }
            None
        };
        if let Some(finally_kw) = finally_kw {
            last_consumed = self.parse_block(
                BlockKind::FinallyClause,
                crate::message::BlockKind::FinallyClause,
            );
            self.listener.handle_finally_block(finally_kw);
        }
        self.listener
            .end_try_statement(catch_count, try_kw, finally_kw, last_consumed);
    }

    /// The `catch (...)` parameter scan of `parseTryStatement` (the
    /// `catchSyntax` recovery family), followed by the full formal-parameter
    /// parse. Returns the last consumed span; `comma` receives the
    /// exception/trace separator when present.
    fn parse_catch_parameters(&mut self, comma: &mut Option<TokenSpan>) -> TokenSpan {
        if self.peek_kind() != TokenKind::OpenParen {
            // Recovery: `insertParens(includeIdentifier: true)`. The official
            // parser then re-enters parseFormalParameters at the synthetic
            // `(` and parses the empty list; our cursor cannot re-serve
            // inserted tokens, so the same empty-list event shape is emitted
            // directly.
            let at = self.peek_span();
            self.report(msg::catch_syntax(), at);
            let offset = at.range.offset;
            let open = self.synthetic(TokenKind::OpenParen, offset);
            self.cursor.advance();
            self.synthetic(TokenKind::Identifier, offset);
            self.cursor.advance();
            let close = self.synthetic(TokenKind::CloseParen, offset);
            self.cursor.advance();
            self.listener
                .begin_formal_parameters(open, crate::listener::MemberKind::Catch);
            self.listener
                .end_formal_parameters(0, open, close, crate::listener::MemberKind::Catch);
            return close;
        }
        let open = self.peek_span();
        let close_id = self.cursor.table_matching_end(open.id);
        let open_group_synthetic = close_id.map_or(true, |close| self.cursor.resolve(close).1);
        let scan_mark = self.cursor.mark();
        let name_synthetic = if !self.peek_is_identifier() {
            let _ = self.ensure_identifier(IdentifierContext::CatchParameter);
            true
        } else {
            self.cursor.advance();
            false
        };
        if self.peek_kind() != TokenKind::CloseParen {
            let mut current_comma = None;
            if self.peek_kind() == TokenKind::Comma {
                current_comma = Some(self.peek_span());
                self.cursor.advance();
            } else {
                let at = self.peek_span();
                if !name_synthetic {
                    self.report(msg::catch_syntax(), at);
                }
                if open_group_synthetic && let Some(close) = close_id {
                    // The scanner placed the synthetic `)` elsewhere: adopt it.
                    self.skip_to_token(close);
                    self.cursor.advance();
                } else {
                    current_comma = Some(self.synthetic(TokenKind::Comma, at.range.offset));
                    self.cursor.advance();
                }
            }
            if let Some(comma_span) = current_comma {
                let trace_synthetic = if !self.peek_is_identifier() {
                    let _ = self.ensure_identifier(IdentifierContext::CatchParameter);
                    true
                } else {
                    self.cursor.advance();
                    false
                };
                if self.peek_kind() != TokenKind::CloseParen {
                    let at = self.peek_span();
                    if !trace_synthetic {
                        self.report(msg::catch_syntax_extra_parameters(), at);
                    }
                    if open_group_synthetic && let Some(close) = close_id {
                        self.skip_to_token(close);
                        self.cursor.advance();
                    }
                }
                *comma = Some(comma_span);
            }
        }
        // Re-parse the parameter list for the formal-parameter events (the
        // official parseFormalParameters re-enters at `(`).
        self.cursor.reset(scan_mark);
        self.parse_formal_parameters_rest(crate::listener::MemberKind::Catch)
    }

    // ------------------------------------------------------------------
    // switch
    // ------------------------------------------------------------------

    /// Official `parseSwitchStatement`.
    fn parse_switch_statement(&mut self) {
        let switch_kw = self.peek_span();
        self.cursor.advance();
        self.listener.begin_switch_statement(switch_kw);
        self.ensure_parenthesized_condition(false);
        let saved = self.loop_state;
        if self.loop_state == LoopState::OutsideLoop {
            self.loop_state = LoopState::InsideSwitch;
        }
        self.parse_switch_block();
        self.loop_state = saved;
        self.listener
            .end_switch_statement(switch_kw, self.peek_span());
    }

    /// Official `parseSwitchBlock`.
    fn parse_switch_block(&mut self) -> TokenSpan {
        let had_open = self.peek_kind() == TokenKind::OpenCurly;
        let (begin, close) = self.ensure_block(crate::message::BlockKind::SwitchStatement);
        self.listener.begin_switch_block(begin);
        let mut case_count = 0usize;
        let mut default_kw: Option<TokenSpan> = None;
        let mut colon_after_default: Option<TokenSpan> = None;
        if had_open {
            while !self.cursor.at_eof()
                && self.peek_kind() != TokenKind::CloseCurly
                && !self.is_bailed()
            {
                let begin_case = self.peek_span();
                let mut expression_count = 0usize;
                let mut label_count = 0usize;
                let mut peek_pos = self.peek_past_labels_pos(self.cursor.real_pos());
                loop {
                    let peek_kind = self.kind_at(peek_pos);
                    if peek_kind == TokenKind::Keyword(Keyword::Default) {
                        while self.cursor.real_pos() < peek_pos
                            && !self.cursor.at_eof()
                            && !self.is_bailed()
                        {
                            self.parse_label();
                            label_count += 1;
                        }
                        if let Some(_previous) = default_kw {
                            let at = self.peek_span();
                            self.report(msg::switch_has_multiple_defaults(), at);
                        }
                        default_kw = Some(self.peek_span());
                        self.cursor.advance();
                        colon_after_default = Some(self.expect_kind(TokenKind::Colon));
                        peek_pos = self.peek_past_labels_pos(self.cursor.real_pos());
                        break;
                    } else if peek_kind == TokenKind::Keyword(Keyword::Case) {
                        while self.cursor.real_pos() < peek_pos
                            && !self.cursor.at_eof()
                            && !self.is_bailed()
                        {
                            self.parse_label();
                            label_count += 1;
                        }
                        if default_kw.is_some() {
                            let at = self.peek_span();
                            self.report(msg::switch_has_case_after_default(), at);
                        }
                        let case_kw = self.peek_span();
                        self.cursor.advance();
                        self.listener.begin_case_expression(case_kw);
                        let _ = self.parse_pattern(PatternContext::Matching);
                        let mut when = None;
                        if self.peek_kind() == TokenKind::Keyword(Keyword::When) {
                            let when_kw = self.peek_span();
                            self.cursor.advance();
                            self.listener.begin_switch_case_when_clause(when_kw);
                            let _ = self.parse_expression();
                            self.listener.end_switch_case_when_clause(self.peek_span());
                            when = Some(when_kw);
                        } else {
                            self.listener
                                .handle_switch_case_no_when_clause(self.peek_span());
                        }
                        let colon = self.expect_kind(TokenKind::Colon);
                        self.listener.end_case_expression(case_kw, when, colon);
                        expression_count += 1;
                        peek_pos = self.peek_past_labels_pos(self.cursor.real_pos());
                    } else if expression_count > 0 {
                        break;
                    } else {
                        // Recovery: not a case; skip to the end of the block.
                        let at = self.span_of(self.real_id(peek_pos));
                        self.report(msg::expected_token("case"), at);
                        break;
                    }
                }
                self.parse_statements_in_switch_case(
                    peek_pos,
                    begin_case,
                    label_count,
                    expression_count,
                    default_kw,
                    colon_after_default,
                );
                case_count += 1;
            }
        } else {
            // Synthetic empty block: drain the inserted `{`/`}` pair; the
            // real token that triggered recovery stays for the enclosing
            // loop.
            self.skip_to_token(close.id);
            self.cursor.advance();
            self.listener.end_switch_block(0, begin, close);
            return close;
        }
        let end = self.peek_span();
        if !self.is_bailed() && self.peek_kind() == TokenKind::CloseCurly {
            self.cursor.advance();
        }
        self.listener.end_switch_block(case_count, begin, end);
        end
    }

    /// Official `parseStatementsInSwitchCase`.
    #[allow(clippy::too_many_arguments)]
    fn parse_statements_in_switch_case(
        &mut self,
        mut peek_pos: usize,
        begin_case: TokenSpan,
        label_count: usize,
        expression_count: usize,
        default_kw: Option<TokenSpan>,
        colon_after_default: Option<TokenSpan>,
    ) {
        self.listener
            .begin_switch_case(label_count, expression_count, begin_case);
        let mut statement_count = 0usize;
        while !self.cursor.at_eof() && !self.is_bailed() {
            let peek_kind = self.kind_at(peek_pos);
            let at_end = peek_kind == TokenKind::CloseCurly && self.cursor.real_pos() == peek_pos;
            if matches!(
                peek_kind,
                TokenKind::Keyword(Keyword::Case) | TokenKind::Keyword(Keyword::Default)
            ) || at_end
            {
                break;
            }
            let start = self.cursor.peek_id();
            self.parse_statement();
            statement_count += 1;
            if self.cursor.peek_id() == start && !self.is_bailed() {
                let at = self.peek_span();
                self.report(msg::unexpected_token(self.cursor.lexeme(at.id)), at);
                self.force_advance();
            }
            peek_pos = self.peek_past_labels_pos(self.cursor.real_pos());
        }
        self.listener.end_switch_case(
            label_count,
            expression_count,
            default_kw,
            colon_after_default,
            statement_count,
            begin_case,
            self.peek_span(),
        );
    }

    // ------------------------------------------------------------------
    // function bodies (full parser; replaces the #18 diet mode)
    // ------------------------------------------------------------------

    /// Official `parseFunctionBody` (`parser_impl.dart:6537`): `;` |
    /// `=>` expression `;` | `{ statements }`, with the official recovery
    /// family. The `=>` expression parses through the reduced engine until
    /// #20.
    pub(crate) fn parse_function_body(&mut self, allow_abstract: bool) -> TokenSpan {
        if self.peek_kind() == TokenKind::Keyword(Keyword::Native) {
            let native = self.peek_span();
            let has_name = self.cursor.peek_kind_ahead(1) == TokenKind::String;
            self.cursor.advance();
            if has_name {
                self.parse_literal_string();
            }
            self.listener.handle_native_clause(native, has_name);
            self.report(msg::native_clause_should_be_annotation(), native);
            if self.peek_kind() == TokenKind::Semicolon {
                let semi = self.peek_span();
                self.cursor.advance();
                self.listener
                    .handle_native_function_body_skipped(native, semi);
                return semi;
            }
            self.listener
                .handle_native_function_body_ignored(native, self.peek_span());
        }
        match self.peek_kind() {
            TokenKind::Semicolon => {
                let semi = self.peek_span();
                if !allow_abstract {
                    self.report(msg::expected_body(), semi);
                }
                self.cursor.advance();
                self.listener.handle_empty_function_body(semi);
                semi
            }
            TokenKind::Function => self.parse_expression_function_body(),
            TokenKind::Eq => {
                // Recover from a bad factory method.
                let at = self.peek_span();
                self.report(msg::expected_body(), at);
                let arrow = self.synthetic(TokenKind::Function, at.range.offset);
                self.cursor.advance();
                self.finish_expression_function_body(arrow)
            }
            _ => {
                let next = self.peek_kind();
                // `return` used instead of `=>`.
                if next == TokenKind::Keyword(Keyword::Return) {
                    let at = self.peek_span();
                    self.report(msg::expected_body(), at);
                    let arrow = self.synthetic(TokenKind::Function, at.range.offset);
                    self.cursor.advance();
                    return self.finish_expression_function_body(arrow);
                }
                // Stray keyword/identifier before `=>` (user is typing).
                if is_keyword_or_identifier(next)
                    && self.cursor.peek_kind_ahead(1) == TokenKind::Function
                {
                    let at = self.peek_span();
                    self.report(msg::unexpected_token(self.cursor.lexeme(at.id)), at);
                    self.force_advance();
                    return self.parse_expression_function_body();
                }
                // Stray keyword/identifier before `{`: skip and fall through.
                if is_keyword_or_identifier(next)
                    && self.cursor.peek_kind_ahead(1) == TokenKind::OpenCurly
                {
                    let at = self.peek_span();
                    self.report(msg::unexpected_token(self.cursor.lexeme(at.id)), at);
                    self.force_advance();
                } else if self.peek_kind() != TokenKind::OpenCurly {
                    let (open, close) = self.ensure_function_body_block();
                    self.listener.handle_invalid_function_body(open);
                    self.skip_to_token(close.id);
                    self.cursor.advance();
                    return close;
                }
                let begin = self.peek_span();
                self.listener.begin_block_function_body(begin);
                let saved_loop = self.loop_state;
                self.loop_state = LoopState::OutsideLoop;
                let count = self.parse_statements_until_close();
                let end = self.peek_span();
                if !self.is_bailed() && self.peek_kind() == TokenKind::CloseCurly {
                    self.cursor.advance();
                }
                self.listener.end_block_function_body(count, begin, end);
                self.loop_state = saved_loop;
                end
            }
        }
    }

    /// Official `parseExpressionFunctionBody` (member flavor: the semicolon
    /// is required).
    fn parse_expression_function_body(&mut self) -> TokenSpan {
        let arrow = self.peek_span();
        self.cursor.advance();
        self.finish_expression_function_body(arrow)
    }

    fn finish_expression_function_body(&mut self, arrow: TokenSpan) -> TokenSpan {
        let _ = self.parse_expression();
        let end = self.ensure_semicolon();
        self.listener
            .handle_expression_function_body(arrow, Some(end));
        if matches!(
            self.async_state,
            AsyncModifier::SyncStar | AsyncModifier::AsyncStar
        ) {
            // `generatorReturnsValue` is resolution-domain: event only.
            self.listener.handle_invalid_statement(arrow);
        }
        end
    }

    /// `ensureBlock` with the `ExpectedFunctionBody` template (official
    /// `BlockKind.functionBody`).
    fn ensure_function_body_block(&mut self) -> (TokenSpan, TokenSpan) {
        if self.peek_kind() == TokenKind::OpenCurly
            && let Some(close_id) = self.current_matching_end()
        {
            let open = self.peek_span();
            return (open, self.span_of(close_id));
        }
        let at = self.peek_span();
        let lexeme = self.cursor.lexeme(at.id).to_owned();
        self.report(msg::expected_function_body(&lexeme), at);
        let offset = at.range.offset;
        let open = self.synthetic(TokenKind::OpenCurly, offset);
        let close = self.synthetic(TokenKind::CloseCurly, offset);
        (open, close)
    }

    // ------------------------------------------------------------------
    // helpers
    // ------------------------------------------------------------------

    /// Official `isModifier` (`modifier_context.dart:13`) at cursor-relative
    /// offset `n`, including the built-in follower and record-type
    /// refinements.
    fn is_statement_modifier_at(&self, n: usize) -> bool {
        let keyword = match self.cursor.look_kind(n) {
            TokenKind::Keyword(kw) => kw,
            _ => return false,
        };
        let is_builtin = matches!(keyword.style(), KeywordStyle::BuiltIn);
        match keyword {
            Keyword::Abstract
            | Keyword::Augment
            | Keyword::Const
            | Keyword::Covariant
            | Keyword::External
            | Keyword::Final
            | Keyword::Late
            | Keyword::Required
            | Keyword::Static
            | Keyword::Var => {}
            _ => return false,
        }
        if !is_builtin {
            return true;
        }
        // A built-in keyword is only a modifier when followed by another
        // keyword or an identifier.
        let next = self.cursor.look_kind(n + 1);
        let next_is_keyword_or_identifier =
            matches!(next, TokenKind::Keyword(_)) || is_identifier(next);
        if next_is_keyword_or_identifier && next != TokenKind::Keyword(Keyword::In) {
            return true;
        }
        // Record type is a possibility: `[modifier] ( ... ) identifier`.
        if next != TokenKind::OpenParen {
            return false;
        }
        let open_id = self.real_id(self.cursor.real_pos() + n + 1);
        let Some(close) = self.cursor.table_matching_end(open_id) else {
            return false;
        };
        let name_like = |pos: usize| -> bool {
            let kind = self.kind_at(pos);
            is_identifier(kind)
                || (matches!(kind, TokenKind::Keyword(Keyword::This | Keyword::Super))
                    && self.kind_at(pos + 1) == TokenKind::Period)
        };
        let after = self.offset_of(close) + 1;
        if name_like(after) {
            return true;
        }
        if self.kind_at(after) == TokenKind::Question {
            return name_like(after + 1);
        }
        false
    }

    /// `skipOuterPattern` (compute-only): the id of the outer pattern's last
    /// token when the cursor starts one.
    fn skip_outer_pattern_id(&self) -> Option<TokenId> {
        match self.peek_kind() {
            TokenKind::OpenSquare | TokenKind::OpenCurly | TokenKind::OpenParen => {
                self.current_matching_end()
            }
            _ => None,
        }
    }

    /// Official `looksLikeExpressionAfterAwaitOrYield` (`parser_impl.dart:
    /// 10621`) at cursor-relative offset `n` (the `await`/`yield` token).
    fn looks_like_await_or_yield_expression(&self, ctx: AwaitYieldCtx, n: usize) -> bool {
        let first = self.cursor.look_kind(n + 1);
        if first == TokenKind::Keyword(Keyword::Null) {
            return true;
        }
        if !is_identifier(first) {
            return false;
        }
        let second = self.cursor.look_kind(n + 2);
        match second {
            TokenKind::OpenParen => {
                let open_id = self.real_id(self.cursor.real_pos() + n + 2);
                let Some(close) = self.cursor.table_matching_end(open_id) else {
                    return false;
                };
                let after = self.kind_at(self.offset_of(close) + 1);
                match after {
                    TokenKind::Semicolon
                    | TokenKind::Period
                    | TokenKind::Comma
                    | TokenKind::PeriodPeriod
                    | TokenKind::Question
                    | TokenKind::QuestionPeriod
                    | TokenKind::CloseParen => true,
                    kind if crate::expr_const::binary_level(kind).is_some() => true,
                    _ => false,
                }
            }
            TokenKind::Period | TokenKind::CloseParen | TokenKind::CloseSquare => true,
            TokenKind::Comma if ctx == AwaitYieldCtx::UnaryExpression => true,
            TokenKind::Semicolon if ctx == AwaitYieldCtx::UnaryExpression => true,
            kind if crate::expr_const::binary_level(kind).is_some() => true,
            _ => false,
        }
    }
}
