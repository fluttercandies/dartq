//! Recovery primitives (core-spec §3.2 and §6).
//!
//! Official policy: every syntax error is recoverable — report through the
//! listener, insert a synthetic to keep the event stream well-formed, and
//! continue (`parser_impl.dart` class docs :248-273). The primitives here are
//! the Rust equivalents of the official `ensure*`/`rewriteAndRecover`/
//! `moveSynthetic`/skip family, adjusted for an immutable token table:
//!
//! * no `insertSyntheticIdentifier` into the stream — synthetics are queued
//!   in the cursor and referenced by id;
//! * `move_synthetic_close` — when the scanner already synthesized a closing
//!   token for an unclosed group (and already reported it), we adopt it by
//!   skipping to it instead of re-reporting;
//! * skip-to-sync — group-aware jump to a boundary set, with a no-progress
//!   guard at every loop level (official strategy C, `parser_impl.dart:445`).

use dartq_scanner::{Keyword, TokenKind};

use crate::members::is_keyword_or_identifier;
use crate::message::msg;
use crate::parser::{AsyncModifier, Parser};
use crate::token_id::TokenSpan;

impl<L: crate::listener::ParserListener> Parser<'_, '_, L> {
    /// Reports a recoverable error pinned to `at` (official
    /// `reportRecoverableError`, `parser_impl.dart:11399`; the official
    /// `findNonZeroLengthToken` adjustment is unnecessary here because the
    /// scanner keeps zero-width synthetics out of the way of normal
    /// reporting — our spans stay within bounds regardless).
    ///
    /// Counted against the per-file diagnostics cap: suppressed once the
    /// bailout latched.
    pub(crate) fn report(&mut self, message: crate::message::Message, at: TokenSpan) {
        if self.admit_diagnostic() {
            self.listener.handle_recoverable_error(message, at, at);
        }
    }

    /// Reports with an explicit span end (official
    /// `reportRecoverableErrorWithEnd`); same cap accounting as [`Self::report`].
    pub(crate) fn report_with_end(
        &mut self,
        message: crate::message::Message,
        start: TokenSpan,
        end: TokenSpan,
    ) {
        if self.admit_diagnostic() {
            self.listener.handle_recoverable_error(message, start, end);
        }
    }

    /// Consumes a required token or reports `Expected to find '<lexeme>'.`
    /// and synthesizes it (official `ensure*` family). Returns the consumed
    /// or synthetic id.
    pub(crate) fn expect_kind(&mut self, kind: TokenKind) -> TokenSpan {
        if let Some(id) = self.cursor.opt_kind(kind) {
            return self.span_of(id);
        }
        let expected = kind.fixed_lexeme().unwrap_or_default();
        let at = self.peek_span();
        self.report(msg::expected_token(expected), at);
        let id = self.cursor.insert_synthetic(kind, at.range.offset);
        self.cursor.advance();
        self.span_of(id)
    }

    /// `expectedButGot` flavor used where the official code chooses that
    /// message (`Expected '<x>' before this.`).
    pub(crate) fn expect_lexeme_but_got(&mut self, expected: &str) -> TokenSpan {
        if let Some(id) = self.cursor.opt_lexeme(expected) {
            return self.span_of(id);
        }
        let at = self.peek_span();
        self.report(msg::expected_but_got(expected), at);
        let kind = lexeme_kind(expected);
        let id = self.cursor.insert_synthetic(kind, at.range.offset);
        self.cursor.advance();
        self.span_of(id)
    }

    /// Official `ensureIdentifier` (`parser_impl.dart:4076`) dispatched
    /// through the context's recovery template
    /// (`identifier_context_impl.dart`): consumes an identifier-shaped
    /// token; a reserved word in a keyword-tolerant context is reported
    /// (`expectedIdentifierButGotKeyword`) and consumed as the name; any
    /// other junk reports `missing_identifier` and inserts a synthetic
    /// identifier without consuming the junk.
    pub(crate) fn ensure_identifier(
        &mut self,
        context: crate::listener::IdentifierContext,
    ) -> TokenSpan {
        let peek = self.cursor.peek();
        // `_tryRewriteNewToIdentifier` (parser_impl.dart:4101-4111): in
        // continuation contexts `new` after `.` is rewritten to an
        // identifier (the unnamed constructor) with no report
        // (`allowsNewAsIdentifier => isContinuation`,
        // identifier_context_impl.dart:200/391).
        if peek.kind == TokenKind::Keyword(Keyword::New)
            && matches!(
                context,
                crate::listener::IdentifierContext::ConstructorReferenceContinuation
                    | crate::listener::IdentifierContext::ConstructorReferenceContinuationAfterTypeArguments
            )
        {
            let id = self.cursor.advance();
            let span = self.span_of(id);
            self.listener.handle_identifier(span, context);
            return span;
        }
        // Keyword names go through the context's official recovery template
        // first: class/mixin/extension names reject built-in identifiers
        // (`builtInIdentifierInDeclaration`), keyword-tolerant contexts
        // report `expectedIdentifierButGotKeyword` for reserved words and
        // consume them as the name; whatever the template rejects falls
        // through to the generic synthetic path.
        if matches!(peek.kind, TokenKind::Keyword(_))
            && let Some(message) = keyword_as_name_message(context, &peek, &self.cursor)
        {
            self.report(message, peek.span);
            let id = self.cursor.advance();
            let span = self.span_of(id);
            self.listener.handle_identifier(span, context);
            return span;
        }
        if is_identifier(peek.kind) {
            self.check_async_keyword_as_identifier(&peek, context);
            let id = self.cursor.advance();
            let span = self.span_of(id);
            self.listener.handle_identifier(span, context);
            return span;
        }
        // `TypeReference.ensureIdentifier` (identifier_context_impl.dart:1528):
        // `var` in type-reference position is reported and consumed as the
        // type name (the caller emits its own type event for the span).
        if matches!(
            context,
            crate::listener::IdentifierContext::TypeReference
                | crate::listener::IdentifierContext::PrefixedTypeReference
        ) && peek.kind == TokenKind::Keyword(Keyword::Var)
        {
            self.report(msg::var_as_type_name(), peek.span);
            let id = self.cursor.advance();
            return self.span_of(id);
        }
        if matches!(
            context,
            crate::listener::IdentifierContext::Expression
                | crate::listener::IdentifierContext::ExpressionContinuation
        ) && !is_identifier(peek.kind)
        {
            return self.ensure_expression_identifier(context);
        }
        // Official recovery template for identifiers: report and insert a
        // synthetic (identifier_context_impl.dart recovery templates); the
        // synthetic is consumed so the caller's next token is the real one.
        self.report(msg::missing_identifier(), peek.span);
        let id = self
            .cursor
            .insert_synthetic(TokenKind::Identifier, peek.span.range.offset);
        self.cursor.advance();
        self.span_of(id)
    }

    /// `ExpressionIdentifierContext.ensureIdentifier`
    /// (`identifier_context_impl.dart:393-472`): a keyword in expression
    /// position is consumed as the name with
    /// `expected_identifier_but_got_keyword` (except `as`/`is`/EOF in a
    /// non-continuation context); other junk either makes way for a
    /// synthetic or falls through to `expected_identifier` + synthetic.
    fn ensure_expression_identifier(
        &mut self,
        context: crate::listener::IdentifierContext,
    ) -> TokenSpan {
        let peek = self.cursor.peek();
        let is_continuation = context == crate::listener::IdentifierContext::ExpressionContinuation;
        let junk_kind = peek.kind;
        // `looksLikeStatementStart(identifier)` (identifier_context_impl.dart
        // :411): the candidate token itself is classified — a keyword that
        // starts a statement is left for the statement parser, not eaten as
        // a name. The cursor sits on the candidate.
        if !self.looks_like_statement_start_at(0) {
            if is_keyword_or_identifier(junk_kind) {
                let lexeme = self.cursor.lexeme(peek.span.id).to_owned();
                if is_continuation
                    || (!matches!(junk_kind, TokenKind::Eof) && lexeme != "as" && lexeme != "is")
                {
                    self.report(msg::expected_identifier_but_got_keyword(&lexeme), peek.span);
                    let id = self.cursor.advance();
                    let span = self.span_of(id);
                    self.listener.handle_identifier(span, context);
                    return span;
                }
            } else if !crate::members::is_user_definable_operator_kind(junk_kind)
                && !matches!(
                    junk_kind,
                    TokenKind::Eq
                        | TokenKind::Bang
                        | TokenKind::Comma
                        | TokenKind::Question
                        | TokenKind::Period
                        | TokenKind::QuestionPeriod
                        | TokenKind::OpenParen
                        | TokenKind::CloseParen
                        | TokenKind::OpenSquare
                        | TokenKind::CloseSquare
                        | TokenKind::OpenCurly
                        | TokenKind::CloseCurly
                        | TokenKind::Colon
                        | TokenKind::Semicolon
                        | TokenKind::Eof
                )
            {
                // When in doubt, consume the token to ensure progress; the
                // report anchors on it and the synthetic lands after.
                let lexeme = self.cursor.lexeme(peek.span.id).to_owned();
                self.report(msg::expected_identifier(&lexeme), peek.span);
                self.cursor.advance();
                let id = self
                    .cursor
                    .insert_synthetic(TokenKind::Identifier, peek.span.range.offset);
                self.cursor.advance();
                return self.span_of(id);
            }
        }
        let lexeme = self.cursor.lexeme(peek.span.id).to_owned();
        self.report(msg::expected_identifier(&lexeme), peek.span);
        let id = self
            .cursor
            .insert_synthetic(TokenKind::Identifier, peek.span.range.offset);
        self.cursor.advance();
        self.span_of(id)
    }

    /// `looksLikeStatementStart` (`identifier_context.dart:402`) evaluated
    /// at cursor-relative offset `n`.
    fn looks_like_statement_start_at(&self, n: usize) -> bool {
        matches!(
            self.cursor.look_kind(n),
            TokenKind::At
                | TokenKind::Keyword(
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
                )
                | TokenKind::Eof
        )
    }

    /// Official `checkAsyncAwaitYieldAsIdentifier`
    /// (`identifier_context_impl.dart:1632`): `await`/`yield` consumed as a
    /// plain name inside an async or generator function reports
    /// `async_keyword_used_as_identifier`.
    fn check_async_keyword_as_identifier(
        &mut self,
        view: &crate::cursor::TokenView,
        context: crate::listener::IdentifierContext,
    ) {
        if self.async_state == AsyncModifier::Sync || !context.checks_async_keyword() {
            return;
        }
        let lexeme = self.cursor.lexeme(view.span.id);
        if lexeme == "await" || lexeme == "yield" {
            self.report(msg::async_keyword_used_as_identifier(), view.span);
        }
    }

    /// `IdentifierContext.catchParameter.ensureIdentifier`
    /// (`identifier_context_impl.dart:25`): the catch template reports
    /// `catch_syntax` (not `missing_identifier`) for non-identifier junk.
    pub(crate) fn ensure_catch_parameter(&mut self) -> TokenSpan {
        let peek = self.cursor.peek();
        if is_identifier(peek.kind) {
            self.check_async_keyword_as_identifier(
                &peek,
                crate::listener::IdentifierContext::CatchParameter,
            );
            let id = self.cursor.advance();
            let span = self.span_of(id);
            self.listener
                .handle_identifier(span, crate::listener::IdentifierContext::CatchParameter);
            return span;
        }
        self.report(msg::catch_syntax(), peek.span);
        let id = self
            .cursor
            .insert_synthetic(TokenKind::Identifier, peek.span.range.offset);
        self.cursor.advance();
        self.span_of(id)
    }

    /// Official `ensureCloseParen` (`parser_impl.dart:5028`): consumes `)`
    /// and returns its span.
    ///
    /// Missing `)` with a scanner-synthesized closer for the group: adopt it
    /// (`moveSynthetic`) without re-reporting — the scanner already reported
    /// the unclosed group. A well-formed but distant `)` is reported once and
    /// jumped to.
    pub(crate) fn ensure_close_paren(&mut self, open: TokenSpan) -> TokenSpan {
        if let Some(id) = self.cursor.opt_kind(TokenKind::CloseParen) {
            return self.span_of(id);
        }
        // Where does the group actually close?
        if let Some(close_id) = self.cursor.table_matching_end(open.id) {
            let (_, synthetic) = self.cursor.resolve(close_id);
            if !synthetic {
                let at = self.peek_span();
                self.report(msg::expected_but_got(")"), at);
            }
            self.skip_to_token(close_id);
            let close = self.peek_span();
            self.cursor.advance();
            return close;
        }
        // No matching closer at all: report and synthesize here.
        let at = self.peek_span();
        self.report(msg::expected_but_got(")"), at);
        let id = self
            .cursor
            .insert_synthetic(TokenKind::CloseParen, at.range.offset);
        self.cursor.advance();
        self.span_of(id)
    }

    /// Skips forward until `id` is the current token (group boundaries in the
    /// table are respected implicitly: ids are absolute). EOF pins the
    /// cursor, so a stale target cannot loop forever.
    pub(crate) fn skip_to_token(&mut self, id: crate::token_id::TokenId) {
        while self.cursor.peek_id() != id && !self.cursor.at_eof() {
            self.cursor.advance();
        }
    }

    /// Skip-to-sync over a boundary set (official `skip*` strategy B). Group
    /// tokens are entered and left via their matched ends so the scan does
    /// not stop inside a nested construct. Terminates: each iteration
    /// advances at least one real token, EOF pins. Public recovery surface
    /// for the declaration/statement/expression milestones (#18/#19/#20).
    pub fn skip_to_sync(&mut self, boundaries: &[TokenKind]) {
        while !self.cursor.at_eof() {
            let kind = self.cursor.peek().kind;
            if boundaries.contains(&kind) {
                return;
            }
            if let Some(end) = self.current_matching_end() {
                self.skip_to_token(end);
                continue;
            }
            self.cursor.advance();
        }
    }

    /// Official `ensureIdentifierPotentiallyRecovered` (recovered-names
    /// flavor, `identifier_context_impl.dart:1491`): for every context this
    /// crate reaches it with, the recovered flavor consumes the keyword with
    /// the same report as the normal template, so it shares
    /// [`Self::ensure_identifier`].
    pub(crate) fn ensure_identifier_recovered(
        &mut self,
        context: crate::listener::IdentifierContext,
    ) -> TokenSpan {
        self.ensure_identifier(context)
    }

    /// Official `ensureSemicolon`: consumes `;` or reports
    /// `Expected ';' after this.` and synthesizes one.
    pub(crate) fn ensure_semicolon(&mut self) -> TokenSpan {
        if let Some(id) = self.cursor.opt_kind(TokenKind::Semicolon) {
            return self.span_of(id);
        }
        let at = self.peek_span();
        self.report(msg::expected_after_but_got(";"), at);
        let id = self
            .cursor
            .insert_synthetic(TokenKind::Semicolon, at.range.offset);
        self.cursor.advance();
        self.span_of(id)
    }

    /// Official `ensureBlock` (declaration `BlockKind`s): consumes the
    /// scanner-paired `{`...`}` group or reports + synthesizes an empty
    /// zero-width block. Returns `(open, close)`; for synthetic blocks the
    /// caller jumps to `close.id` (inserted ids resolve in the cursor).
    pub(crate) fn ensure_block(
        &mut self,
        kind: crate::message::BlockKind,
    ) -> (TokenSpan, TokenSpan) {
        if self.peek_kind() == TokenKind::OpenCurly {
            let open = self.peek_span();
            if let Some(close_id) = self.current_matching_end() {
                let close = self.span_of(close_id);
                return (open, close);
            }
        }
        // Message-carrying kinds report on the token BEFORE the missing `{`
        // (official ensureBlock: `reportRecoverableError(token, message)`).
        // The switch-expression lane pins this anchor (`switch (n)` reports
        // on the `)`); the other message kinds keep the follower anchor of
        // their own lanes (#18/#19) pending their differential passes. The
        // synthetic `{}` still lands at the follower (insertBlock uses
        // `next.charOffset`).
        let follower = self.peek_span();
        let at = if kind == crate::message::BlockKind::SwitchExpression {
            self.cursor
                .last_consumed_id()
                .map(|id| self.span_of(id))
                .unwrap_or(follower)
        } else {
            follower
        };
        let lexeme = self.cursor.lexeme(follower.id).to_owned();
        self.report(msg::expected_block_body(kind, &lexeme), at);
        let offset = follower.range.offset;
        let open = self.synthetic(TokenKind::OpenCurly, offset);
        let close = self.synthetic(TokenKind::CloseCurly, offset);
        (open, close)
    }

    /// Diet-mode block consumption (`skipBlock`): `ensure_block` and a jump
    /// to the closing `}`.
    pub(crate) fn skip_block(&mut self, kind: crate::message::BlockKind) -> TokenSpan {
        let (_, close) = self.ensure_block(kind);
        self.skip_to_token(close.id);
        let close_span = self.peek_span();
        self.cursor.advance();
        close_span
    }

    /// No-progress guard (official strategy C): returns `true` when the
    /// caller made no progress since `before` and must force-advance.
    pub(crate) fn no_progress(&self, before: u64) -> bool {
        self.cursor.progress() == before
    }

    /// Forces one token of progress (official :445-456 / :10530-10537).
    pub(crate) fn force_advance(&mut self) {
        self.cursor.advance();
    }
}

/// Token kind for a fixed lexeme, for synthesizing recovery tokens. Only
/// the lexemes this crate synthesizes are mapped; the scanner's
/// `TokenKind::fixed_lexeme` is the forward direction.
pub(crate) fn lexeme_kind(lexeme: &str) -> TokenKind {
    match lexeme {
        "," => TokenKind::Comma,
        ":" => TokenKind::Colon,
        "}" => TokenKind::CloseCurly,
        ")" => TokenKind::CloseParen,
        "]" => TokenKind::CloseSquare,
        ">" => TokenKind::Gt,
        ";" => TokenKind::Semicolon,
        "=>" => TokenKind::Function,
        _ => TokenKind::Identifier,
    }
}

/// Identifier-shaped tokens: identifiers plus built-in/pseudo keywords in
/// identifier positions (official `token.isIdentifier` accepts
/// pseudo/built-in keywords; reserved words never).
pub(crate) fn is_identifier(kind: TokenKind) -> bool {
    match kind {
        TokenKind::Identifier => true,
        TokenKind::Keyword(kw) => kw.style() != dartq_scanner::KeywordStyle::Reserved,
        _ => false,
    }
}

/// Contexts whose official recovery template keeps the identifier-position
/// keyword sequence intact: a reserved word is reported
/// (`expectedIdentifierButGotKeyword`) and consumed as the name. The
/// exceptions (`identifier_context_impl.dart`) are the expression contexts
/// (their own `as`/`is`/`EOF`-sensitive template), the type-reference
/// contexts (`expectedType` family), `catchParameter` (`catchSyntax`
/// template) and `fieldInitializer` (always `expectedIdentifier`).
fn context_tolerates_keyword_as_name(context: crate::listener::IdentifierContext) -> bool {
    !matches!(
        context,
        crate::listener::IdentifierContext::Expression
            | crate::listener::IdentifierContext::ExpressionContinuation
            | crate::listener::IdentifierContext::TypeReference
            | crate::listener::IdentifierContext::PrefixedTypeReference
            | crate::listener::IdentifierContext::TypeReferenceContinuation
            | crate::listener::IdentifierContext::CatchParameter
            | crate::listener::IdentifierContext::FieldInitializer
    )
}

/// The report (if any) for consuming `view` as an identifier in `context`,
/// mirroring the official context templates: class/mixin/extension names
/// reject built-in identifiers (`builtInIdentifierInDeclaration`);
/// keyword-tolerant contexts report `expectedIdentifierButGotKeyword` for
/// reserved words; everything else falls back to the generic
/// `missing_identifier` path.
fn keyword_as_name_message(
    context: crate::listener::IdentifierContext,
    view: &crate::cursor::TokenView,
    cursor: &crate::cursor::TokenCursor<'_, '_>,
) -> Option<crate::message::Message> {
    let TokenKind::Keyword(kw) = view.kind else {
        return None;
    };
    let lexeme = cursor.lexeme(view.span.id);
    match kw.style() {
        dartq_scanner::KeywordStyle::Reserved => {
            if context_tolerates_keyword_as_name(context) {
                Some(msg::expected_identifier_but_got_keyword(lexeme))
            } else {
                None
            }
        }
        dartq_scanner::KeywordStyle::BuiltIn
            if context
                == crate::listener::IdentifierContext::ClassOrMixinOrExtensionDeclaration =>
        {
            Some(msg::built_in_identifier_in_declaration(lexeme))
        }
        // Type-variable names reject built-in keywords outright: the oracle
        // reports `expectedIdentifierButGotKeyword` for `Function` in
        // `<...>` type parameters (issue_45703 line 4) and consumes it as
        // the name.
        dartq_scanner::KeywordStyle::BuiltIn
            if context == crate::listener::IdentifierContext::TypeVariableDeclaration =>
        {
            Some(msg::expected_identifier_but_got_keyword(lexeme))
        }
        _ => None,
    }
}
